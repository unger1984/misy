//! In-memory supervision and bounded output for local command activities.

use crate::{ActivityId, ActivityOutput, ActivityStatus, ActivitySummary, activity::ActivityOwner};
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

const MAX_ACTIVE_TASKS: usize = 64;
const MAX_ACTIVE_CHILD_TASKS: usize = 48;
const MAX_ACTIVE_TASKS_PER_CHILD: usize = 16;
const MAX_RECENT_TASKS: usize = 20;
mod interaction;
mod launch;
pub(super) mod output;
mod pending;
mod process;
pub(super) mod pty;
mod record;
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
    next_id: Arc<AtomicU64>,
    admission: Mutex<ActivityAdmission>,
    shutting_down: AtomicBool,
    // Terminal output must not be dropped: the single core router drains this channel, and the
    // four-process admission limit bounds how quickly command producers can enqueue events.
    events: mpsc::UnboundedSender<ActivityEvent>,
    event_receiver: Mutex<Option<mpsc::UnboundedReceiver<ActivityEvent>>>,
}

#[derive(Debug)]
struct ActivityRecord {
    id: ActivityId,
    owner: ActivityOwner,
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
    owner: ActivityOwner,
}

#[derive(Debug, Default)]
struct ActivityAdmission {
    active: usize,
    active_children: usize,
    per_child: BTreeMap<crate::AgentId, usize>,
}

impl Drop for ProcessPermit {
    fn drop(&mut self) {
        if let Some(manager) = self.manager.upgrade() {
            manager.release_slot(self.owner);
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
        Self::with_ids(Arc::new(AtomicU64::new(1)))
    }

    pub(super) fn with_ids(next_id: Arc<AtomicU64>) -> Self {
        let (events, event_receiver) = mpsc::unbounded_channel();
        Self {
            inner: Arc::new(ActivityManagerInner {
                records: Mutex::new(BTreeMap::new()),
                recent: Mutex::new(VecDeque::new()),
                next_id,
                admission: Mutex::new(ActivityAdmission::default()),
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

    pub(super) fn stop_for_owner(&self, owner: ActivityOwner, id: ActivityId) -> bool {
        let Some(record) = self.record_for_owner(owner, id) else {
            return false;
        };
        if record.summary().status.is_terminal() {
            return false;
        }
        record.cancel_poll();
        record.stop.send_replace(Some(StopReason::Requested));
        true
    }

    pub(super) fn activities_for_owner(&self, owner: ActivityOwner) -> Vec<ActivitySummary> {
        let records = self
            .inner
            .records
            .lock()
            .expect("activity records mutex must not be poisoned");
        let mut summaries: Vec<_> = records
            .values()
            .filter(|record| record.owner == owner && record.is_published())
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

    pub(super) async fn stop_owner(&self, owner: ActivityOwner) {
        let records: Vec<_> = self
            .inner
            .records
            .lock()
            .expect("activity records mutex must not be poisoned")
            .values()
            .filter(|record| record.owner == owner)
            .cloned()
            .collect();
        for record in &records {
            if !record.summary().status.is_terminal() {
                record.cancel_poll();
                record.stop.send_replace(Some(StopReason::Requested));
            }
        }
        for record in records {
            record.join_supervisor().await;
        }
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

    fn record_for_owner(
        &self,
        owner: ActivityOwner,
        id: ActivityId,
    ) -> Option<Arc<ActivityRecord>> {
        self.record(id).filter(|record| record.owner == owner)
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

    fn reserve_slot(&self, owner: ActivityOwner) -> Result<ProcessPermit, String> {
        self.inner.reserve_slot(owner)?;
        Ok(ProcessPermit {
            manager: Arc::downgrade(&self.inner),
            owner,
        })
    }
}

impl ActivityManagerInner {
    fn reserve_slot(&self, owner: ActivityOwner) -> Result<(), String> {
        let mut admission = self
            .admission
            .lock()
            .expect("activity admission mutex must not be poisoned");
        if admission.active >= MAX_ACTIVE_TASKS {
            return Err(format!(
                "cannot run command: {MAX_ACTIVE_TASKS} processes are already active"
            ));
        }
        if let ActivityOwner::Agent(agent) = owner {
            let child_active = admission.per_child.get(&agent).copied().unwrap_or_default();
            if admission.active_children >= MAX_ACTIVE_CHILD_TASKS
                || child_active >= MAX_ACTIVE_TASKS_PER_CHILD
            {
                return Err("cannot run command: child-agent process quota reached".to_owned());
            }
            admission.active_children += 1;
            admission.per_child.insert(agent, child_active + 1);
        }
        admission.active += 1;
        Ok(())
    }

    fn release_slot(&self, owner: ActivityOwner) {
        let mut admission = self
            .admission
            .lock()
            .expect("activity admission mutex must not be poisoned");
        admission.active = admission.active.saturating_sub(1);
        if let ActivityOwner::Agent(agent) = owner {
            admission.active_children = admission.active_children.saturating_sub(1);
            let remaining = admission
                .per_child
                .get(&agent)
                .copied()
                .unwrap_or_default()
                .saturating_sub(1);
            if remaining == 0 {
                admission.per_child.remove(&agent);
            } else {
                admission.per_child.insert(agent, remaining);
            }
        }
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
