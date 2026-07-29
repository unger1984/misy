//! Session commands and transcript replay over the core-owned store.

use super::{BrowserHandoff, TuiClient, TuiError};
use crate::tui::terminal::SessionStart;

impl<B: BrowserHandoff> TuiClient<B> {
    pub(crate) fn apply_session_start(&mut self, start: SessionStart) -> Result<(), TuiError> {
        match start {
            SessionStart::Fresh => Ok(()),
            SessionStart::ResumeId(id) => self.resume_session(&id),
            SessionStart::ResumePicker => self.show_sessions(),
            SessionStart::ResumeLatest => {
                let Some(session) = self.core.list_sessions()?.into_iter().next() else {
                    self.state.set_startup_notice(Some(
                        "No saved session in this directory; started a new one".to_owned(),
                    ));
                    return Ok(());
                };
                self.resume_session(&session.id)
            }
        }
    }

    pub(super) fn show_sessions(&mut self) -> Result<(), TuiError> {
        let sessions = self.core.list_sessions()?;
        self.state.set_session_id(self.core.current_session_id());
        self.state.open_sessions(sessions);
        Ok(())
    }

    pub(super) fn start_new_session(&mut self) -> Result<(), TuiError> {
        self.core.new_session()?;
        self.state.clear_conversation();
        self.state.set_session_id(None);
        self.state.add_info("New session started");
        self.refresh_core_projection();
        Ok(())
    }

    pub(super) fn resume_session(&mut self, id: &str) -> Result<(), TuiError> {
        let outcome = self.core.resume_session(id)?;
        self.state.replay_history(&outcome.history);
        self.state.set_session_id(Some(outcome.session.id.clone()));
        self.refresh_core_projection();
        let snapshot = self.core.snapshot();
        let directory = std::env::current_dir().ok();
        self.state
            .set_startup_header(snapshot.selected_model.as_ref(), directory.as_deref());
        if let Some(warning) = outcome.model_warning {
            self.state.set_startup_notice(Some(warning.clone()));
            self.state.add_error(warning);
        }
        self.state.add_info(format!(
            "Resumed session {}",
            outcome.session.id.chars().take(16).collect::<String>()
        ));
        Ok(())
    }
}
