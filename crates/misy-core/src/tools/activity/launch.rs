//! Command launch and publication setup for the activity manager.

use super::{
    ActivityInput, ActivityManager, ActivityRecord, ActivityState, PendingCommand, SupervisorTask,
    now_millis, output, process, pty,
};
use crate::{ActivityId, ActivityStatus, activity::ActivityOwner};
use std::sync::{Arc, Mutex, atomic::Ordering};
use tokio::sync::{Notify, watch};

use crate::tools::command::CommandRequest;

impl ActivityManager {
    #[cfg(test)]
    pub(super) fn start(&self, request: &CommandRequest) -> Result<PendingCommand, String> {
        self.start_for_owner(ActivityOwner::Main, request)
    }

    pub(in crate::tools) fn start_for_owner(
        &self,
        owner: ActivityOwner,
        request: &CommandRequest,
    ) -> Result<PendingCommand, String> {
        let permit = self.reserve_slot(owner)?;
        let id = ActivityId::new(self.inner.next_id.fetch_add(1, Ordering::Relaxed));
        let (stop, stop_receiver) = watch::channel(None);
        let (poll_cancel, _) = watch::channel(0);
        let record = Arc::new(ActivityRecord {
            id,
            owner,
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
            session_open: std::sync::atomic::AtomicBool::new(true),
        });
        self.register_record(id, &record)?;
        let spawned = request.spawn().map_err(|error| {
            self.remove_failed_record(id);
            record.mark_supervisor_unavailable();
            format!("could not run {}: {error}", request.program_display())
        })?;
        let supervisor = match spawned {
            crate::tools::command::SpawnedCommand::Pipe {
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
            crate::tools::command::SpawnedCommand::Pty(spawned) => pty::spawn_supervisor(
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

    fn register_record(&self, id: ActivityId, record: &Arc<ActivityRecord>) -> Result<(), String> {
        let mut records = self
            .inner
            .records
            .lock()
            .expect("activity records mutex must not be poisoned");
        if self.inner.shutting_down.load(Ordering::Acquire) {
            return Err("cannot run command: activity manager is shutting down".to_owned());
        }
        records.insert(id, Arc::clone(record));
        Ok(())
    }

    fn remove_failed_record(&self, id: ActivityId) {
        self.inner
            .records
            .lock()
            .expect("activity records mutex must not be poisoned")
            .remove(&id);
    }
}
