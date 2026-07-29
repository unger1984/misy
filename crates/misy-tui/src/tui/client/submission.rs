//! Composer attachment validation and acceptance-aware submission.

use super::{BrowserHandoff, SubmissionRequest, TuiClient, TuiError};
use crate::tui::action::{UiAction, UiMode, map_input};
use misy_core::{CoreError, ImageAttachment};

impl<B: BrowserHandoff> TuiClient<B> {
    /// Validates and inserts one clipboard RGBA image into the focused composer.
    ///
    /// Image and capability failures are rendered without changing the current draft.
    pub fn paste_image_rgba(&mut self, width: u32, height: u32, rgba: Vec<u8>) {
        self.state.clear_quit_shortcut();
        if self.state.mode() != UiMode::Input {
            self.state
                .add_error("images can only be pasted into the prompt composer");
            return;
        }
        if self.composer_submission_pending {
            return;
        }
        if self.core.snapshot().selected_model.is_none() {
            self.state.add_error(CoreError::NoModelSelected);
            return;
        }
        let image = match ImageAttachment::from_rgba(width, height, rgba) {
            Ok(image) => image,
            Err(error) => {
                self.state
                    .add_error(format!("could not paste clipboard image: {error}"));
                return;
            }
        };
        if let Err(error) = self.state.composer.insert_image(image) {
            self.state.add_error(error);
        }
    }

    /// Submits the composer or accepts its slash-command completion.
    ///
    /// # Errors
    ///
    /// Returns core or browser-handoff errors produced by the selected action.
    pub fn submit_composer(&mut self) -> Result<(), TuiError> {
        if self.state.mode() != UiMode::Input {
            return Ok(());
        }
        if self.state.composer.popup_visible()
            && let Some(command) = self.state.composer.selected_command()
        {
            return self.submit_completed_command(command);
        }
        if self.composer_submission_pending {
            return Ok(());
        }
        let draft = self.state.composer.draft();
        match map_input(&draft.text) {
            Ok(UiAction::Noop) if !draft.images.is_empty() => self.enqueue_composer_draft(draft),
            Ok(UiAction::Noop) => Ok(()),
            Ok(UiAction::SubmitPrompt(_)) => self.enqueue_composer_draft(draft),
            Ok(action) => self.execute_local_composer_action(action),
            Err(error) => {
                self.state.add_error(error);
                Ok(())
            }
        }
    }

    /// Parses and executes submitted text.
    ///
    /// # Errors
    ///
    /// Returns failures from core calls initiated by the resulting action.
    pub fn handle_input(&mut self, input: &str) -> Result<(), TuiError> {
        self.state.clear_quit_shortcut();
        self.handle_submitted_input(input).0
    }

    fn enqueue_composer_draft(
        &mut self,
        draft: crate::tui::composer_attachment::ComposerDraft,
    ) -> Result<(), TuiError> {
        let history_text = self.state.composer.history_text();
        let request = SubmissionRequest {
            draft,
            clear_composer: true,
            history_text: (!history_text.is_empty()).then_some(history_text),
            history_snapshot: Some(self.state.composer.history_snapshot()),
        };
        if self.submission_sender.send(request).is_err() {
            self.state
                .add_error("prompt submission worker stopped unexpectedly");
        } else {
            self.composer_submission_pending = true;
        }
        Ok(())
    }

    fn execute_local_composer_action(&mut self, action: UiAction) -> Result<(), TuiError> {
        let result = self
            .execute(action)
            .inspect_err(|error| self.state.add_error(error));
        if result.is_ok() {
            let history_text = self.state.composer.history_text();
            self.state.composer.clear();
            self.record_prompt_history(&history_text);
        }
        result
    }

    fn submit_completed_command(&mut self, command: &str) -> Result<(), TuiError> {
        let (result, accepted) = self.handle_submitted_input(command);
        if accepted {
            self.state.composer.clear();
            self.record_prompt_history(command);
        }
        result
    }

    fn handle_submitted_input(&mut self, input: &str) -> (Result<(), TuiError>, bool) {
        match map_input(input) {
            Ok(UiAction::Noop) => (Ok(()), false),
            Ok(action) => {
                let result = self
                    .execute(action)
                    .inspect_err(|error| self.state.add_error(error));
                let accepted = result.is_ok();
                (result, accepted)
            }
            Err(error) => {
                self.state.add_error(error);
                (Ok(()), false)
            }
        }
    }

    pub(super) fn record_prompt_history(&mut self, text: &str) {
        if !self.state.composer.record_submitted(text) {
            return;
        }
        self.append_prompt_history(text);
    }

    pub(super) fn append_prompt_history(&mut self, text: &str) {
        if let Some(history) = &self.prompt_history
            && let Err(error) = history.append(text)
        {
            self.state
                .add_error(format!("could not save prompt history: {error}"));
        }
    }
}
