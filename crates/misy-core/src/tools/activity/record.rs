//! State projections and supervisor ownership for one command record.

use super::{ActivityInput, ActivityRecord, ActivityState, SupervisorTask, output};
use crate::{ActivityKind, ActivityOutput, ActivityStatus, ActivitySummary};
use std::io::Write;
use tokio::task::JoinHandle;

impl ActivityRecord {
    pub(super) fn release_permit(&self) {
        self.permit
            .lock()
            .expect("activity permit mutex must not be poisoned")
            .take();
    }

    pub(super) fn install_pty_writer(&self, writer: Box<dyn Write + Send>) {
        *self
            .input
            .lock()
            .expect("activity input mutex must not be poisoned") = ActivityInput::Pty(writer);
    }

    pub(super) fn close_input(&self) {
        *self
            .input
            .lock()
            .expect("activity input mutex must not be poisoned") = ActivityInput::Closed;
    }

    pub(super) fn cancel_poll(&self) {
        let next = self.poll_cancel.borrow().saturating_add(1);
        self.poll_cancel.send_replace(next);
    }

    pub(super) fn is_published(&self) -> bool {
        self.state
            .lock()
            .expect("activity state mutex must not be poisoned")
            .published
    }

    pub(super) fn summary(&self) -> ActivitySummary {
        let state = self
            .state
            .lock()
            .expect("activity state mutex must not be poisoned");
        self.summary_from_state(&state)
    }

    pub(super) fn summary_from_state(&self, state: &ActivityState) -> ActivitySummary {
        ActivitySummary {
            id: self.id,
            kind: ActivityKind::Task,
            agent_id: None,
            status: state.status,
            title: self.title.clone(),
            cwd: self.cwd.clone(),
            started_at_ms: self.started_at_ms,
            exit_code: state.exit_code,
            interactive: self.interactive,
        }
    }

    pub(super) fn output(&self) -> ActivityOutput {
        let state = self
            .state
            .lock()
            .expect("activity state mutex must not be poisoned");
        self.output_from_state(&state)
    }

    pub(super) fn model_output(&self, max_output_tokens: usize) -> ActivityOutput {
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
        self.output_from_projection(state, state.output.snapshot())
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

    pub(super) fn output_with_message(&self, message: String) -> ActivityOutput {
        let mut output = self.output();
        output.activity.status = ActivityStatus::Failed;
        output.message = Some(message);
        output
    }

    pub(super) fn take_terminal_output(&self, state: &mut ActivityState) -> Option<ActivityOutput> {
        if !state.published || !state.status.is_terminal() || state.terminal_event_sent {
            return None;
        }
        state.terminal_event_sent = true;
        Some(self.output_from_state(state))
    }

    pub(super) fn set_supervisor(&self, supervisor: JoinHandle<()>) {
        *self
            .supervisor
            .lock()
            .expect("activity supervisor mutex must not be poisoned") =
            SupervisorTask::Spawned(supervisor);
        self.supervisor_changed.notify_waiters();
    }

    pub(super) fn mark_supervisor_unavailable(&self) {
        *self
            .supervisor
            .lock()
            .expect("activity supervisor mutex must not be poisoned") = SupervisorTask::Unavailable;
        self.supervisor_changed.notify_waiters();
    }

    pub(super) async fn join_supervisor(&self) {
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
