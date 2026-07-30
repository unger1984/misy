//! Side-effect routing from the deterministic question dialog to core operations.

use super::{BrowserHandoff, TuiClient, TuiError};
use crate::tui::action::{UiAction, UiKey};

impl<B: BrowserHandoff> TuiClient<B> {
    pub(super) fn handle_question_key(&mut self, key: UiKey) -> Result<(), TuiError> {
        match key {
            UiKey::Up => self.state.reduce(&UiAction::PickerUp),
            UiKey::Down => self.state.reduce(&UiAction::PickerDown),
            UiKey::Left => self.state.reduce(&UiAction::PickerTabLeft),
            UiKey::Right | UiKey::Tab => self.state.reduce(&UiAction::PickerTabRight),
            UiKey::Backspace => self.state.backspace_filter(),
            UiKey::SelectIndex(index) => {
                if let Some(response) = self.state.choose_question_number(index) {
                    self.core.answer_question(response)?;
                    self.refresh_core_projection();
                }
            }
            UiKey::Enter => self.submit_question_if_complete()?,
            UiKey::Escape => {
                if let Some(request_id) = self.state.escape_question() {
                    self.core.dismiss_question(request_id)?;
                    self.refresh_core_projection();
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn submit_question_if_complete(&mut self) -> Result<(), TuiError> {
        if let Some(response) = self.state.confirm_question() {
            self.core.answer_question(response)?;
            self.refresh_core_projection();
        }
        Ok(())
    }
}
