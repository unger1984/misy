//! Deterministic FIFO scheduling for one in-memory agent session.

use super::{ActiveSubmission, CoreError, MisyCore, SubmissionId};
use crate::{Message, ModelRef};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, atomic::Ordering},
    thread,
};

pub(super) struct QueuedSubmission {
    id: SubmissionId,
    model: ModelRef,
    message: Message,
    active: Arc<ActiveSubmission>,
}

#[derive(Default)]
pub(super) struct SubmissionQueue {
    pending: VecDeque<QueuedSubmission>,
    current: Option<(SubmissionId, Arc<ActiveSubmission>)>,
    worker_running: bool,
}

impl MisyCore {
    /// Enqueues one user or system message for deterministic asynchronous processing.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::NoModelSelected`] when no direct-interaction model is selected, or
    /// [`CoreError::Shutdown`] after shutdown.
    ///
    /// # Panics
    ///
    /// Panics if an internal submission mutex was poisoned by an earlier core-thread panic.
    pub fn submit(&self, message: Message) -> Result<SubmissionId, CoreError> {
        self.ensure_running()?;
        let model = self.selected_model().ok_or(CoreError::NoModelSelected)?;
        let mut queue = self
            .inner
            .submission_queue
            .lock()
            .expect("submission queue mutex must not be poisoned");
        // Linearize acceptance with shutdown after model lookup, which may acquire provider locks.
        self.ensure_running()?;
        let id = SubmissionId(self.inner.next_submission.fetch_add(1, Ordering::Relaxed));
        let active = Arc::new(ActiveSubmission {
            cancelled: std::sync::atomic::AtomicBool::new(false),
            request: Mutex::new(None),
        });
        self.inner
            .active
            .lock()
            .expect("active submissions mutex must not be poisoned")
            .insert(id.get(), Arc::clone(&active));
        queue.pending.push_back(QueuedSubmission {
            id,
            model,
            message,
            active,
        });
        let start_worker = if queue.worker_running {
            false
        } else {
            queue.worker_running = true;
            true
        };
        drop(queue);
        if start_worker {
            let core = self.clone();
            thread::spawn(move || core.drain_submission_queue());
        }
        Ok(id)
    }

    /// Cancels every running or queued submission without shutting down the core.
    ///
    /// # Panics
    ///
    /// Panics if an internal submission mutex was poisoned by an earlier core-thread panic.
    pub(crate) fn cancel_all_submissions(&self) {
        let submissions = {
            let _queue = self
                .inner
                .submission_queue
                .lock()
                .expect("submission queue mutex must not be poisoned");
            self.inner
                .active
                .lock()
                .expect("active submissions mutex must not be poisoned")
                .values()
                .cloned()
                .collect::<Vec<_>>()
        };
        for submission in submissions {
            submission.cancelled.store(true, Ordering::Release);
            if let Some((provider, request)) = submission
                .request
                .lock()
                .expect("active request mutex must not be poisoned")
                .clone()
            {
                // Local cancellation is authoritative; the worker re-checks it after remote I/O.
                let _ = self.inner.host.cancel_request(&provider, request);
            }
        }
    }

    /// Cancels the submission currently owning the session, or the next queued submission.
    ///
    /// Returns `false` when no submission is running or queued.
    ///
    /// # Panics
    ///
    /// Panics if an internal submission mutex was poisoned by an earlier core-thread panic.
    pub(crate) fn cancel_current_submission(&self) -> bool {
        let submission = {
            let queue = self
                .inner
                .submission_queue
                .lock()
                .expect("submission queue mutex must not be poisoned");
            let active = self
                .inner
                .active
                .lock()
                .expect("active submissions mutex must not be poisoned");
            queue
                .current
                .as_ref()
                .filter(|(id, _)| active.contains_key(&id.get()))
                .map(|(_, submission)| Arc::clone(submission))
                .or_else(|| {
                    queue
                        .pending
                        .front()
                        .map(|queued| Arc::clone(&queued.active))
                })
        };
        let Some(submission) = submission else {
            return false;
        };
        submission.cancelled.store(true, Ordering::Release);
        if let Some((provider, request)) = submission
            .request
            .lock()
            .expect("active request mutex must not be poisoned")
            .clone()
        {
            // Local cancellation is authoritative; the worker re-checks it after remote I/O.
            let _ = self.inner.host.cancel_request(&provider, request);
        }
        true
    }

    fn drain_submission_queue(&self) {
        loop {
            let next = {
                let mut queue = self
                    .inner
                    .submission_queue
                    .lock()
                    .expect("submission queue mutex must not be poisoned");
                let next = queue.pending.pop_front();
                if let Some(next) = &next {
                    queue.current = Some((next.id, Arc::clone(&next.active)));
                } else {
                    queue.current = None;
                    queue.worker_running = false;
                }
                next
            };
            let Some(next) = next else {
                return;
            };
            self.run_submission(next.id, &next.model, next.message, &next.active);
            let mut queue = self
                .inner
                .submission_queue
                .lock()
                .expect("submission queue mutex must not be poisoned");
            if queue.current.as_ref().map(|(id, _)| *id) == Some(next.id) {
                queue.current = None;
            }
        }
    }
}
