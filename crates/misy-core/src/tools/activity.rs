//! In-memory supervision and bounded output for local command activities.

use crate::{ActivityId, ActivityKind, ActivityOutput, ActivityStatus, ActivitySummary};
use std::{
    collections::{BTreeMap, VecDeque},
    fmt,
    io::Write,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    sync::{Notify, mpsc, oneshot, watch},
    task::JoinHandle,
    time::timeout,
};

use super::command::CommandRequest;

const MAX_ACTIVE_TASKS: usize = 64;
const MAX_RECENT_TASKS: usize = 20;
mod interaction;
pub(super) mod output;
mod pending;
mod process;
pub(super) mod pty;
#[cfg(test)]
mod tests;

pub(super) use interaction::InteractionError;
pub(super) use pending::PendingCommand;

#[derive(Debug)]
pub(crate) enum ActivityEvent {
    Changed(ActivitySummary),
    Finished(ActivityOutput),
    Flush(oneshot::Sender<()>),
}

#[derive(Clone, Debug)]
pub(super) struct ActivityManager {
    inner: Arc<ActivityManagerInner>,
}

#[derive(Debug)]
struct ActivityManagerInner {
    records: Mutex<BTreeMap<ActivityId, Arc<ActivityRecord>>>,
    recent: Mutex<VecDeque<ActivityId>>,
    next_id: AtomicU64,
    active_slots: AtomicU64,
    shutting_down: AtomicBool,
    // Terminal output must not be dropped: the single core router drains this channel, and the
    // four-process admission limit bounds how quickly command producers can enqueue events.
    events: mpsc::UnboundedSender<ActivityEvent>,
    event_receiver: Mutex<Option<mpsc::UnboundedReceiver<ActivityEvent>>>,
}

#[derive(Debug)]
struct ActivityRecord {
    id: ActivityId,
    title: String,
    cwd: Option<String>,
    started_at_ms: u64,
    interactive: bool,
    permit: Mutex<Option<ProcessPermit>>,
    input: Mutex<ActivityInput>,
    state: Mutex<ActivityState>,
    delivery_cursor: Mutex<output::DeliveryCursor>,
    changed: Notify,
    stop: watch::Sender<Option<StopReason>>,
    // Shutdown takes this handle and waits for bounded process cleanup before flushing events.
    supervisor: Mutex<SupervisorTask>,
    supervisor_changed: Notify,
    interaction: tokio::sync::Mutex<()>,
    poll_cancel: watch::Sender<u64>,
    session_open: AtomicBool,
}

enum ActivityInput {
    Closed,
    Pty(Box<dyn Write + Send>),
}

impl fmt::Debug for ActivityInput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Closed => "Closed",
            Self::Pty(_) => "Pty",
        })
    }
}

#[derive(Debug)]
enum SupervisorTask {
    Pending,
    Spawned(JoinHandle<()>),
    Unavailable,
}

#[derive(Debug)]
struct ActivityState {
    status: ActivityStatus,
    exit_code: Option<i32>,
    output: output::OrderedOutput,
    message: Option<String>,
    published: bool,
    terminal_event_sent: bool,
}

#[derive(Debug)]
struct ProcessPermit {
    manager: std::sync::Weak<ActivityManagerInner>,
}

impl Drop for ProcessPermit {
    fn drop(&mut self) {
        if let Some(manager) = self.manager.upgrade() {
            manager.active_slots.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum StopReason {
    Requested,
    Shutdown,
}

impl ActivityManager {
    pub(super) fn new() -> Self {
        let (events, event_receiver) = mpsc::unbounded_channel();
        Self {
            inner: Arc::new(ActivityManagerInner {
                records: Mutex::new(BTreeMap::new()),
                recent: Mutex::new(VecDeque::new()),
                next_id: AtomicU64::new(1),
                active_slots: AtomicU64::new(0),
                shutting_down: AtomicBool::new(false),
                events,
                event_receiver: Mutex::new(Some(event_receiver)),
            }),
        }
    }

    pub(super) fn subscribe(&self) -> mpsc::UnboundedReceiver<ActivityEvent> {
        self.inner
            .event_receiver
            .lock()
            .expect("activity event receiver mutex must not be poisoned")
            .take()
            .expect("activity event receiver must only be subscribed once")
    }

    pub(super) fn activities(&self) -> Vec<ActivitySummary> {
        let records = self
            .inner
            .records
            .lock()
            .expect("activity records mutex must not be poisoned");
        let mut summaries: Vec<_> = records
            .values()
            .filter(|record| record.is_published())
            .map(|record| record.summary())
            .collect();
        summaries.sort_by_key(|summary| {
            (
                summary.status.is_terminal(),
                std::cmp::Reverse(summary.started_at_ms),
            )
        });
        summaries
    }

    pub(super) fn start(&self, request: &CommandRequest) -> Result<PendingCommand, String> {
        let permit = self.reserve_slot()?;
        let id = ActivityId::new(self.inner.next_id.fetch_add(1, Ordering::Relaxed));
        let (stop, stop_receiver) = watch::channel(None);
        let (poll_cancel, _) = watch::channel(0);
        let record = Arc::new(ActivityRecord {
            id,
            title: request.description.clone(),
            cwd: request.cwd.clone(),
            started_at_ms: now_millis(),
            interactive: request.tty,
            permit: Mutex::new(Some(permit)),
            input: Mutex::new(ActivityInput::Closed),
            state: Mutex::new(ActivityState {
                status: ActivityStatus::Running,
                exit_code: None,
                output: output::OrderedOutput::default(),
                message: None,
                published: false,
                terminal_event_sent: false,
            }),
            delivery_cursor: Mutex::new(output::DeliveryCursor::default()),
            changed: Notify::new(),
            stop,
            supervisor: Mutex::new(SupervisorTask::Pending),
            supervisor_changed: Notify::new(),
            interaction: tokio::sync::Mutex::new(()),
            poll_cancel,
            session_open: AtomicBool::new(true),
        });
        let mut records = self
            .inner
            .records
            .lock()
            .expect("activity records mutex must not be poisoned");
        if self.inner.shutting_down.load(Ordering::Acquire) {
            return Err("cannot run command: activity manager is shutting down".to_owned());
        }
        records.insert(id, Arc::clone(&record));
        drop(records);
        let spawned = request.spawn().map_err(|error| {
            self.inner
                .records
                .lock()
                .expect("activity records mutex must not be poisoned")
                .remove(&id);
            record.mark_supervisor_unavailable();
            format!("could not run {}: {error}", request.program_display())
        })?;
        let supervisor = match spawned {
            super::command::SpawnedCommand::Pipe {
                child,
                stdout,
                stderr,
            } => process::spawn_supervisor(
                Arc::downgrade(&self.inner),
                Arc::clone(&record),
                child,
                stdout,
                stderr,
                stop_receiver,
                request.timeout,
            ),
            super::command::SpawnedCommand::Pty(spawned) => pty::spawn_supervisor(
                Arc::downgrade(&self.inner),
                Arc::clone(&record),
                spawned,
                stop_receiver,
                request.timeout,
            ),
        };
        record.set_supervisor(supervisor);
        let mut pending = PendingCommand {
            manager: self.clone(),
            record,
            promoted: false,
        };
        if request.run_in_background {
            pending.promote()?;
        }
        Ok(pending)
    }

    pub(super) async fn output(
        &self,
        id: ActivityId,
        wait: Option<Duration>,
    ) -> Option<ActivityOutput> {
        let record = self.record(id)?;
        if let Some(wait) = wait {
            let changed = record.changed.notified();
            tokio::pin!(changed);
            if !record.summary().status.is_terminal() {
                let _ = timeout(wait, &mut changed).await;
            }
        }
        Some(record.output())
    }

    pub(super) fn stop(&self, id: ActivityId) -> bool {
        let Some(record) = self.record(id) else {
            return false;
        };
        if record.summary().status.is_terminal() {
            return false;
        }
        record.cancel_poll();
        record.stop.send_replace(Some(StopReason::Requested));
        true
    }

    pub(super) async fn shutdown(&self) {
        let records: Vec<_> = {
            let records = self
                .inner
                .records
                .lock()
                .expect("activity records mutex must not be poisoned");
            self.inner.shutting_down.store(true, Ordering::Release);
            records.values().cloned().collect()
        };
        for record in &records {
            if !record.summary().status.is_terminal() {
                record.cancel_poll();
                record.stop.send_replace(Some(StopReason::Shutdown));
            }
        }
        for record in records {
            record.join_supervisor().await;
        }
    }

    pub(super) async fn flush_events(&self) {
        if self
            .inner
            .event_receiver
            .lock()
            .expect("activity event receiver mutex must not be poisoned")
            .is_some()
        {
            return;
        }
        let (flushed, completion) = oneshot::channel();
        if self
            .inner
            .events
            .send(ActivityEvent::Flush(flushed))
            .is_err()
        {
            return;
        }
        let _ = completion.await;
    }

    fn record(&self, id: ActivityId) -> Option<Arc<ActivityRecord>> {
        self.inner
            .records
            .lock()
            .expect("activity records mutex must not be poisoned")
            .get(&id)
            .filter(|record| record.is_published())
            .cloned()
    }

    fn publish(&self, record: &Arc<ActivityRecord>) -> Result<(), String> {
        let mut records = self
            .inner
            .records
            .lock()
            .expect("activity records mutex must not be poisoned");
        if self.inner.shutting_down.load(Ordering::Acquire) {
            return Err("cannot background command: activity manager is shutting down".to_owned());
        }
        {
            let mut state = record
                .state
                .lock()
                .expect("activity state mutex must not be poisoned");
            if state.published {
                return Ok(());
            }
            state.published = true;
            if let Some(output) = record.take_terminal_output(&mut state) {
                let _ = self.inner.events.send(ActivityEvent::Finished(output));
            } else {
                let activity = record.summary_from_state(&state);
                let _ = self.inner.events.send(ActivityEvent::Changed(activity));
            }
        }
        records.insert(record.id, Arc::clone(record));
        if record.summary().status.is_terminal() {
            drop(records);
            retain_recent(&self.inner, record.id);
        }
        record.changed.notify_waiters();
        Ok(())
    }

    fn reserve_slot(&self) -> Result<ProcessPermit, String> {
        self.inner
            .active_slots
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |active| {
                (active < MAX_ACTIVE_TASKS as u64).then_some(active + 1)
            })
            .map(|_| ProcessPermit {
                manager: Arc::downgrade(&self.inner),
            })
            .map_err(|_| {
                format!("cannot run command: {MAX_ACTIVE_TASKS} processes are already active")
            })
    }
}

impl Default for ActivityManager {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for ActivityManagerInner {
    fn drop(&mut self) {
        let records = self
            .records
            .get_mut()
            .expect("activity records mutex must not be poisoned");
        for record in records.values() {
            record.stop.send_replace(Some(StopReason::Shutdown));
        }
    }
}

impl ActivityRecord {
    fn release_permit(&self) {
        self.permit
            .lock()
            .expect("activity permit mutex must not be poisoned")
            .take();
    }

    fn install_pty_writer(&self, writer: Box<dyn Write + Send>) {
        *self
            .input
            .lock()
            .expect("activity input mutex must not be poisoned") = ActivityInput::Pty(writer);
    }

    fn close_input(&self) {
        *self
            .input
            .lock()
            .expect("activity input mutex must not be poisoned") = ActivityInput::Closed;
    }

    fn cancel_poll(&self) {
        let next = self.poll_cancel.borrow().saturating_add(1);
        self.poll_cancel.send_replace(next);
    }

    fn is_published(&self) -> bool {
        self.state
            .lock()
            .expect("activity state mutex must not be poisoned")
            .published
    }

    fn summary(&self) -> ActivitySummary {
        let state = self
            .state
            .lock()
            .expect("activity state mutex must not be poisoned");
        self.summary_from_state(&state)
    }

    fn summary_from_state(&self, state: &ActivityState) -> ActivitySummary {
        ActivitySummary {
            id: self.id,
            kind: ActivityKind::Task,
            status: state.status,
            title: self.title.clone(),
            cwd: self.cwd.clone(),
            started_at_ms: self.started_at_ms,
            exit_code: state.exit_code,
            interactive: self.interactive,
        }
    }

    fn output(&self) -> ActivityOutput {
        let state = self
            .state
            .lock()
            .expect("activity state mutex must not be poisoned");
        self.output_from_state(&state)
    }

    fn model_output(&self, max_output_tokens: usize) -> ActivityOutput {
        let state = self
            .state
            .lock()
            .expect("activity state mutex must not be poisoned");
        let mut cursor = self
            .delivery_cursor
            .lock()
            .expect("activity delivery cursor mutex must not be poisoned");
        let projection = state.output.deliver(*cursor, max_output_tokens);
        *cursor = projection.cursor;
        self.output_from_projection(&state, projection)
    }

    fn output_from_state(&self, state: &ActivityState) -> ActivityOutput {
        let projection = state.output.snapshot();
        self.output_from_projection(state, projection)
    }

    fn output_from_projection(
        &self,
        state: &ActivityState,
        projection: output::OutputProjection,
    ) -> ActivityOutput {
        ActivityOutput {
            activity: self.summary_from_state(state),
            stdout: projection.stdout,
            stderr: projection.stderr,
            stdout_truncated: projection.stdout_truncated,
            stderr_truncated: projection.stderr_truncated,
            fragments: projection.fragments,
            message: state.message.clone(),
        }
    }

    fn output_with_message(&self, message: String) -> ActivityOutput {
        let mut output = self.output();
        output.activity.status = ActivityStatus::Failed;
        output.message = Some(message);
        output
    }

    fn take_terminal_output(&self, state: &mut ActivityState) -> Option<ActivityOutput> {
        if !state.published || !state.status.is_terminal() || state.terminal_event_sent {
            return None;
        }
        state.terminal_event_sent = true;
        Some(self.output_from_state(state))
    }

    fn set_supervisor(&self, supervisor: JoinHandle<()>) {
        *self
            .supervisor
            .lock()
            .expect("activity supervisor mutex must not be poisoned") =
            SupervisorTask::Spawned(supervisor);
        self.supervisor_changed.notify_waiters();
    }

    fn mark_supervisor_unavailable(&self) {
        *self
            .supervisor
            .lock()
            .expect("activity supervisor mutex must not be poisoned") = SupervisorTask::Unavailable;
        self.supervisor_changed.notify_waiters();
    }

    async fn join_supervisor(&self) {
        loop {
            let changed = self.supervisor_changed.notified();
            tokio::pin!(changed);
            let supervisor = {
                let mut task = self
                    .supervisor
                    .lock()
                    .expect("activity supervisor mutex must not be poisoned");
                match &*task {
                    SupervisorTask::Pending => None,
                    SupervisorTask::Spawned(_) => {
                        let SupervisorTask::Spawned(supervisor) =
                            std::mem::replace(&mut *task, SupervisorTask::Unavailable)
                        else {
                            unreachable!("matched spawned activity supervisor")
                        };
                        Some(supervisor)
                    }
                    SupervisorTask::Unavailable => return,
                }
            };
            if let Some(supervisor) = supervisor {
                let _ = supervisor.await;
                return;
            }
            changed.await;
        }
    }
}

fn retain_recent(manager: &ActivityManagerInner, id: ActivityId) {
    let mut recent = manager
        .recent
        .lock()
        .expect("recent activities mutex must not be poisoned");
    recent.retain(|candidate| *candidate != id);
    recent.push_front(id);
    while recent.len() > MAX_RECENT_TASKS {
        let Some(expired) = recent.pop_back() else {
            break;
        };
        manager
            .records
            .lock()
            .expect("activity records mutex must not be poisoned")
            .remove(&expired);
    }
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
        })
}

pub(super) fn parse_activity_id(value: &str) -> Option<ActivityId> {
    value
        .strip_prefix("task-")?
        .parse()
        .ok()
        .map(ActivityId::new)
}
