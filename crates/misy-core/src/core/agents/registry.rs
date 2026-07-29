//! Process-local child-agent admission, addressing, inbox, and mailbox state.

use super::{AgentId, AgentSummary, AgentTranscript, TranscriptBuffer};
use crate::{ActivityId, ActivityStatus, CoreError, ModelRef, core::ActiveSubmission};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore, watch};

const MAX_LIVE_AGENTS: usize = 4;
const MAX_MAILBOX_RESULTS: usize = 8;
const MAX_RECENT_AGENTS: usize = 20;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ConversationGeneration(u64);

#[derive(Debug)]
pub(crate) enum InboxDecision {
    Continue(Vec<String>),
    Close,
}

#[derive(Debug)]
pub(crate) enum MailboxWait {
    Ready(Vec<AgentSummary>),
    TimedOut,
    Cancelled,
}

#[derive(Debug)]
pub(crate) struct AgentRecord {
    generation: ConversationGeneration,
    pub(super) state: Mutex<AgentRecordState>,
    pub(super) terminal_changed: Notify,
}

#[derive(Debug)]
pub(super) struct AgentRecordState {
    pub(super) summary: AgentSummary,
    pub(super) transcript: TranscriptBuffer,
    pub(super) inbox: VecDeque<String>,
    pub(super) inbox_open: bool,
    pub(super) final_result: Option<String>,
    pub(super) live_permit: Option<OwnedSemaphorePermit>,
    pub(super) mailbox_permit: Option<OwnedSemaphorePermit>,
    pub(super) mailbox_pending: bool,
    pub(super) active: Option<Arc<ActiveSubmission>>,
}

#[derive(Clone, Debug)]
pub(crate) struct AgentRegistry {
    inner: Arc<AgentRegistryInner>,
}

#[derive(Debug)]
struct AgentRegistryInner {
    records: Mutex<BTreeMap<AgentId, Arc<AgentRecord>>>,
    recent: Mutex<VecDeque<AgentId>>,
    mailbox: Mutex<VecDeque<AgentId>>,
    live_slots: Arc<Semaphore>,
    mailbox_slots: Arc<Semaphore>,
    next_id: AtomicU64,
    generation: AtomicU64,
    accepting: AtomicBool,
    mailbox_changed: Notify,
}

impl AgentRegistry {
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(AgentRegistryInner {
                records: Mutex::new(BTreeMap::new()),
                recent: Mutex::new(VecDeque::new()),
                mailbox: Mutex::new(VecDeque::new()),
                live_slots: Arc::new(Semaphore::new(MAX_LIVE_AGENTS)),
                mailbox_slots: Arc::new(Semaphore::new(MAX_MAILBOX_RESULTS)),
                next_id: AtomicU64::new(1),
                generation: AtomicU64::new(1),
                accepting: AtomicBool::new(true),
                mailbox_changed: Notify::new(),
            }),
        }
    }

    pub(super) fn register(
        &self,
        activity_id: ActivityId,
        title: String,
        model: ModelRef,
        task: &str,
        run_in_background: bool,
    ) -> Result<Arc<AgentRecord>, CoreError> {
        if !self.inner.accepting.load(Ordering::Acquire) {
            return Err(CoreError::Shutdown);
        }
        let live_permit = Arc::clone(&self.inner.live_slots)
            .try_acquire_owned()
            .map_err(|_| CoreError::AgentLimitReached)?;
        let mailbox_permit = if run_in_background {
            Some(
                Arc::clone(&self.inner.mailbox_slots)
                    .try_acquire_owned()
                    .map_err(|_| CoreError::AgentMailboxFull)?,
            )
        } else {
            None
        };
        let id = AgentId::new(self.inner.next_id.fetch_add(1, Ordering::Relaxed));
        let generation = self.current_generation();
        let record = Arc::new(AgentRecord {
            generation,
            state: Mutex::new(AgentRecordState {
                summary: AgentSummary {
                    id,
                    activity_id,
                    title,
                    model,
                    status: ActivityStatus::Running,
                    run_in_background,
                    started_at_ms: now_millis(),
                    finished_at_ms: None,
                    terminal_message: None,
                },
                transcript: TranscriptBuffer::assignment(task, 0),
                inbox: VecDeque::new(),
                inbox_open: true,
                final_result: None,
                live_permit: Some(live_permit),
                mailbox_permit,
                mailbox_pending: false,
                active: None,
            }),
            terminal_changed: Notify::new(),
        });
        self.inner
            .records
            .lock()
            .expect("agent records mutex must not be poisoned")
            .insert(id, Arc::clone(&record));
        Ok(record)
    }

    pub(crate) fn list(&self) -> Vec<AgentSummary> {
        let generation = self.current_generation();
        let records = self
            .inner
            .records
            .lock()
            .expect("agent records mutex must not be poisoned");
        let mut summaries: Vec<_> = records
            .values()
            .filter(|record| record.generation == generation)
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

    pub(super) fn record(&self, id: AgentId) -> Result<Arc<AgentRecord>, CoreError> {
        self.inner
            .records
            .lock()
            .expect("agent records mutex must not be poisoned")
            .get(&id)
            .filter(|record| record.generation == self.current_generation())
            .cloned()
            .ok_or(CoreError::UnknownAgent(id))
    }

    pub(crate) fn transcript(&self, id: AgentId) -> Result<AgentTranscript, CoreError> {
        Ok(self.record(id)?.transcript())
    }

    pub(super) fn message(&self, id: AgentId, message: &str) -> Result<(), CoreError> {
        self.record(id)?.message(message)
    }

    pub(crate) fn stop(&self, id: AgentId) -> Result<(), CoreError> {
        let record = self.record(id)?;
        if record.summary().status.is_terminal() {
            return Err(CoreError::AgentAlreadyFinished(id));
        }
        record.cancel();
        Ok(())
    }

    pub(super) fn finish(
        &self,
        record: &AgentRecord,
        status: ActivityStatus,
        result: &str,
    ) -> bool {
        if !record.finish(status, result) {
            return false;
        }
        let id = record.summary().id;
        if record.mark_mailbox_pending() {
            self.inner
                .mailbox
                .lock()
                .expect("agent mailbox mutex must not be poisoned")
                .push_back(id);
            self.inner.mailbox_changed.notify_waiters();
        }
        self.retain_recent(id);
        true
    }

    pub(super) async fn wait_mailbox(
        &self,
        ids: Option<&[AgentId]>,
        duration: Duration,
        mut cancellation: watch::Receiver<bool>,
    ) -> Result<MailboxWait, CoreError> {
        let filter = self.validate_filter(ids)?;
        let deadline = tokio::time::Instant::now() + duration;
        loop {
            let changed = self.inner.mailbox_changed.notified();
            tokio::pin!(changed);
            let ready = self.consume_mailbox(&filter);
            if !ready.is_empty() {
                return Ok(MailboxWait::Ready(ready));
            }
            let step = tokio::time::timeout_at(deadline, async {
                tokio::select! {
                    () = &mut changed => false,
                    result = cancellation.changed() => {
                        result.is_err() || *cancellation.borrow()
                    }
                }
            })
            .await;
            match step {
                Ok(true) => return Ok(MailboxWait::Cancelled),
                Ok(false) => {}
                Err(_) => return Ok(MailboxWait::TimedOut),
            }
        }
    }

    pub(crate) fn pending_state(&self) -> (usize, usize) {
        let live = self
            .list()
            .into_iter()
            .filter(|agent| !agent.status.is_terminal())
            .count();
        let pending = self
            .inner
            .mailbox
            .lock()
            .expect("agent mailbox mutex must not be poisoned")
            .len();
        (live, pending)
    }

    pub(crate) fn pinned_activity_ids(&self) -> BTreeSet<ActivityId> {
        let generation = self.current_generation();
        self.inner
            .records
            .lock()
            .expect("agent records mutex must not be poisoned")
            .values()
            .filter(|record| record.generation == generation && record.mailbox_pending())
            .map(|record| record.summary().activity_id)
            .collect()
    }

    pub(crate) fn close_admission(&self) {
        self.inner.accepting.store(false, Ordering::Release);
    }

    pub(crate) fn reopen_admission(&self) {
        self.inner.accepting.store(true, Ordering::Release);
    }

    pub(crate) async fn stop_all(&self, duration: Duration) -> bool {
        let generation = self.current_generation();
        let records: Vec<_> = self
            .inner
            .records
            .lock()
            .expect("agent records mutex must not be poisoned")
            .values()
            .filter(|record| record.generation == generation)
            .cloned()
            .collect();
        for record in &records {
            if !record.summary().status.is_terminal() {
                record.cancel();
            }
        }
        let deadline = tokio::time::Instant::now() + duration;
        for record in records {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if !record.wait_terminal(remaining).await {
                return false;
            }
        }
        true
    }

    pub(crate) fn discard_current(&self) {
        let generation = self.current_generation();
        let discarded: BTreeSet<_> = self
            .inner
            .records
            .lock()
            .expect("agent records mutex must not be poisoned")
            .iter()
            .filter_map(|(id, record)| (record.generation == generation).then_some(*id))
            .collect();
        self.inner
            .records
            .lock()
            .expect("agent records mutex must not be poisoned")
            .retain(|id, _| !discarded.contains(id));
        self.inner
            .mailbox
            .lock()
            .expect("agent mailbox mutex must not be poisoned")
            .retain(|id| !discarded.contains(id));
        self.inner
            .recent
            .lock()
            .expect("recent agents mutex must not be poisoned")
            .retain(|id| !discarded.contains(id));
        self.inner.generation.fetch_add(1, Ordering::AcqRel);
    }

    fn current_generation(&self) -> ConversationGeneration {
        ConversationGeneration(self.inner.generation.load(Ordering::Acquire))
    }

    fn validate_filter(
        &self,
        ids: Option<&[AgentId]>,
    ) -> Result<Option<BTreeSet<AgentId>>, CoreError> {
        let Some(ids) = ids else {
            return Ok(None);
        };
        let filter: BTreeSet<_> = ids.iter().copied().collect();
        for id in &filter {
            self.record(*id)?;
        }
        Ok(Some(filter))
    }

    fn consume_mailbox(&self, filter: &Option<BTreeSet<AgentId>>) -> Vec<AgentSummary> {
        let mut mailbox = self
            .inner
            .mailbox
            .lock()
            .expect("agent mailbox mutex must not be poisoned");
        let mut ready = Vec::new();
        let mut retained = VecDeque::new();
        while let Some(id) = mailbox.pop_front() {
            if filter.as_ref().is_none_or(|filter| filter.contains(&id)) {
                if let Ok(record) = self.record(id) {
                    record.consume_mailbox();
                    ready.push(record.summary());
                }
            } else {
                retained.push_back(id);
            }
        }
        *mailbox = retained;
        ready
    }

    fn retain_recent(&self, id: AgentId) {
        let mut recent = self
            .inner
            .recent
            .lock()
            .expect("recent agents mutex must not be poisoned");
        recent.retain(|candidate| *candidate != id);
        recent.push_front(id);
        while recent.len() > MAX_RECENT_AGENTS {
            let Some(expired) = recent.pop_back() else {
                break;
            };
            let removable = self
                .inner
                .records
                .lock()
                .expect("agent records mutex must not be poisoned")
                .get(&expired)
                .is_some_and(|record| !record.mailbox_pending());
            if removable {
                self.inner
                    .records
                    .lock()
                    .expect("agent records mutex must not be poisoned")
                    .remove(&expired);
            } else {
                recent.push_front(expired);
                break;
            }
        }
    }
}

impl Default for AgentRegistry {
    fn default() -> Self {
        Self::new()
    }
}

pub(super) fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
        })
}
