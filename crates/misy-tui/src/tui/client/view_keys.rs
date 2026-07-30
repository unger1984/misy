//! Keyboard routing for modal and picker views.

use super::{BrowserHandoff, ProviderOperationKind, TuiClient, TuiError, UiAction, UiKey, UiMode};

impl<B: BrowserHandoff> TuiClient<B> {
    pub(super) fn handle_view_key(&mut self, key: UiKey) -> Result<(), TuiError> {
        if self.state.mode() == UiMode::Question {
            return self.handle_question_key(key);
        }
        if self.state.mode() == UiMode::Context {
            match key {
                UiKey::Up => self.state.reduce(&UiAction::PickerUp),
                UiKey::Down => self.state.reduce(&UiAction::PickerDown),
                UiKey::PageUp => self.state.scroll_context_page(false),
                UiKey::PageDown => self.state.scroll_context_page(true),
                UiKey::Home => self.state.scroll_context_edge(false),
                UiKey::End => self.state.scroll_context_edge(true),
                UiKey::Escape => self.state.reduce(&UiAction::PickerBack),
                _ => {}
            }
            return Ok(());
        }
        if self.state.mode() == UiMode::AuthPrompt {
            match key {
                UiKey::Escape => {
                    if let Some(provider) = self.state.cancel_auth_prompt() {
                        self.cancel_authentication(provider, None);
                    }
                }
                UiKey::Enter => {
                    if let Some(submission) = self.state.finish_auth_prompt_field() {
                        self.complete_prompt_auth(submission);
                    }
                }
                _ => self.state.auth_prompt_key(key),
            }
            return Ok(());
        }
        if self.state.mode() == UiMode::ActivityDetail {
            match key {
                UiKey::Up => self.state.scroll_activity_log_up(false),
                UiKey::Down => self.state.scroll_activity_log_down(false),
                UiKey::PageUp => self.state.scroll_activity_log_up(true),
                UiKey::PageDown => self.state.scroll_activity_log_down(true),
                UiKey::Escape => self.state.reduce(&UiAction::PickerBack),
                UiKey::StopActivity => self.stop_selected_activity(),
                _ => {}
            }
            return Ok(());
        }
        match key {
            UiKey::Up => self.state.reduce(&UiAction::PickerUp),
            UiKey::Down => self.state.reduce(&UiAction::PickerDown),
            UiKey::Left => self.state.reduce(&UiAction::PickerTabLeft),
            UiKey::Right => self.state.reduce(&UiAction::PickerTabRight),
            UiKey::Backspace => self.state.backspace_filter(),
            UiKey::Escape => {
                if self.cancel_active_authentication() {
                    return Ok(());
                }
                if matches!(
                    self.state.provider_operation,
                    Some((
                        _,
                        ProviderOperationKind::CancelAuth
                            | ProviderOperationKind::Logout
                            | ProviderOperationKind::SelectModel
                    ))
                ) {
                    return Ok(());
                }
                self.state.reduce(&UiAction::PickerBack);
            }
            UiKey::Enter => return self.confirm_view(),
            UiKey::SelectIndex(index) => {
                if self.state.select_picker_number(index) {
                    return self.confirm_view();
                }
            }
            _ => {}
        }
        Ok(())
    }
}
