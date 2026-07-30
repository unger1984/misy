//! FIFO submission scheduling with exactly one session-mutating agent task.

use super::{ActiveSubmission, CoreError, CoreEvent, CoreState, MisyCore, SubmissionId};
use crate::{ImageAttachment, Message, ModelRef};
use std::{
    collections::VecDeque,
    sync::{Arc, atomic::Ordering},
};

pub(super) struct QueuedSubmission {
    pub(super) id: SubmissionId,
    pub(super) model: ModelRef,
    pub(super) message: Message,
    pub(super) attachments: Vec<ImageAttachment>,
    pub(super) active: Arc<ActiveSubmission>,
}

#[derive(Default)]
pub(crate) struct SubmissionQueue {
    pending: VecDeque<QueuedSubmission>,
    current: Option<(SubmissionId, Arc<ActiveSubmission>)>,
    worker_running: bool,
}

impl SubmissionQueue {
    pub(super) fn snapshot(&self) -> (Option<SubmissionId>, Vec<SubmissionId>) {
        let active_submission = self
            .current
            .as_ref()
            .and_then(|(id, active)| (!active.cancelled.load(Ordering::Acquire)).then_some(*id));
        let queued_submissions = self
            .pending
            .iter()
            .filter(|submission| !submission.active.cancelled.load(Ordering::Acquire))
            .map(|submission| submission.id)
            .collect();
        (active_submission, queued_submissions)
    }
}

impl MisyCore {
    /// Enqueues one user or system message for deterministic asynchronous processing.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::NoModelSelected`] when no direct-interaction model is selected, or
    /// [`CoreError::Shutdown`] after shutdown.
    pub async fn submit(&self, message: Message) -> Result<SubmissionId, CoreError> {
        self.submit_with_attachments(message, Vec::new()).await
    }

    /// Enqueues one message with validated image attachments for deterministic processing.
    ///
    /// # Errors
    ///
    /// Returns an error when no model is selected, the provider or model lacks image input,
    /// the attachment count exceeds the limit, or the core has shut down.
    ///
    /// # Panics
    ///
    /// Panics if the internal instruction-state mutex is poisoned.
    pub async fn submit_with_attachments(
        &self,
        message: Message,
        attachments: Vec<ImageAttachment>,
    ) -> Result<SubmissionId, CoreError> {
        self.inner.state.ensure_running()?;
        if let Some(error) = self
            .inner
            .state
            .instructions
            .lock()
            .expect("instruction runtime mutex must not be poisoned")
            .root()
            .base_error()
        {
            return Err(CoreError::InstructionBlocked(error));
        }
        let model = self
            .selected_model()
            .await
            .ok_or(CoreError::NoModelSelected)?;
        let validation = self.inner.state.validate_image_input(&model, &attachments);
        if matches!(&validation, Err(CoreError::UnsupportedInput { .. })) {
            // A cached catalog can predate provider modality metadata. Refresh once at the
            // authoritative async submission boundary before rejecting the preserved draft.
            self.list_models(&model.provider).await?;
            self.inner
                .state
                .validate_image_input(&model, &attachments)?;
        } else {
            validation?;
        }
        // Holding the worker slot through enqueue and spawn linearizes worker ownership with
        // shutdown admission closure without keeping the queue lock across an await.
        let mut worker = self
            .inner
            .state
            .submission_worker
            .lock()
            .expect("submission worker mutex must not be poisoned");
        let (id, start_worker) =
            self.inner
                .state
                .enqueue_submission(message, attachments, model)?;
        if start_worker {
            let state = Arc::clone(&self.inner.state);
            *worker = Some(self.inner.runtime.handle.spawn(async move {
                state.drain_submission_queue().await;
            }));
        }
        Ok(id)
    }

    /// Cancels every running or queued submission without shutting down the core.
    pub async fn cancel_all_submissions(&self) {
        self.inner.state.cancel_all_submissions().await;
    }

    /// Cancels the submission currently owning the session.
    ///
    /// Returns `false` when no submission is running. Queued submissions remain FIFO work until
    /// they are explicitly cancelled or the active submission completes.
    pub async fn cancel_current_submission(&self) -> bool {
        self.inner.state.cancel_current_submission().await
    }
}

impl CoreState {
    fn enqueue_submission(
        &self,
        message: Message,
        attachments: Vec<ImageAttachment>,
        model: ModelRef,
    ) -> Result<(SubmissionId, bool), CoreError> {
        let mut queue = self
            .submission_queue
            .lock()
            .expect("submission queue mutex must not be poisoned");
        // Acceptance must be linearized with shutdown after acquiring queue ownership.
        self.ensure_running()?;
        let id = SubmissionId(self.next_submission.fetch_add(1, Ordering::Relaxed));
        let active = Arc::new(ActiveSubmission::new());
        self.active
            .lock()
            .expect("active submissions mutex must not be poisoned")
            .insert(id.get(), Arc::clone(&active));
        // Acceptance is published under the queue lock, before the submission is enqueued, so
        // subscribers observe `SubmissionAccepted` in FIFO order and always before any later
        // event or snapshot that references the submission.
        self.emit(&CoreEvent::SubmissionAccepted {
            submission: id,
            message: message.clone(),
            attachment_count: attachments.len(),
        });
        queue.pending.push_back(QueuedSubmission {
            id,
            model,
            message,
            attachments,
            active,
        });
        if queue.worker_running {
            Ok((id, false))
        } else {
            queue.worker_running = true;
            Ok((id, true))
        }
    }

    pub(super) async fn cancel_submission(
        &self,
        submission: SubmissionId,
    ) -> Result<(), CoreError> {
        let active = self
            .active
            .lock()
            .expect("active submissions mutex must not be poisoned")
            .get(&submission.get())
            .cloned()
            .ok_or(CoreError::UnknownSubmission(submission))?;
        self.cancel_active_submission(active).await;
        Ok(())
    }

    pub(super) async fn cancel_all_submissions(&self) {
        let submissions = self
            .active
            .lock()
            .expect("active submissions mutex must not be poisoned")
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for submission in submissions {
            self.cancel_active_submission(submission).await;
        }
    }

    pub(super) async fn cancel_current_submission(&self) -> bool {
        let active = {
            let queue = self
                .submission_queue
                .lock()
                .expect("submission queue mutex must not be poisoned");
            queue.current.as_ref().map(|(_, active)| Arc::clone(active))
        };
        let Some(active) = active else {
            return false;
        };
        self.cancel_active_submission(active).await;
        true
    }

    pub(super) async fn finish_submission_worker(&self, timeout: std::time::Duration) {
        let worker = self
            .submission_worker
            .lock()
            .expect("submission worker mutex must not be poisoned")
            .take();
        let Some(mut worker) = worker else {
            return;
        };
        if tokio::time::timeout(timeout, &mut worker).await.is_err() {
            worker.abort();
            let _ = worker.await;
        }
    }

    async fn cancel_active_submission(&self, active: Arc<ActiveSubmission>) {
        if !active.cancel() {
            return;
        }
        let request = active
            .request
            .lock()
            .expect("active request mutex must not be poisoned")
            .clone();
        if let Some((provider, request)) = request {
            // Local cancellation is authoritative; remote cancellation only accelerates cleanup.
            let _ = self.host.cancel_request(&provider, request).await;
        }
    }

    pub(super) async fn drain_submission_queue(self: Arc<Self>) {
        loop {
            let next = {
                let mut queue = self
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
            let terminal_event = self
                .run_submission(
                    next.id,
                    &next.model,
                    next.message,
                    next.attachments,
                    &next.active,
                )
                .await;
            self.finish_submission(next.id);
            self.emit(&terminal_event);
        }
    }

    fn finish_submission(&self, submission: SubmissionId) {
        // Terminal events promise that a fresh snapshot cannot still expose this submission as
        // active. Keep the queue lock through active-state removal so cancellation cannot find a
        // finished submission in the gap before that event is published.
        let mut queue = self
            .submission_queue
            .lock()
            .expect("submission queue mutex must not be poisoned");
        if queue.current.as_ref().map(|(id, _)| *id) == Some(submission) {
            queue.current = None;
        }
        self.active
            .lock()
            .expect("active submissions mutex must not be poisoned")
            .remove(&submission.get());
    }
}
