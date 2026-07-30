//! Secret-aware state for protocol v2 prompt authentication.

use super::{
    action::UiKey,
    auth_flow::PromptField,
    display_width::text_width,
    list::ListRowDisplay,
    state::{ActiveView, UiState},
};
use misy_core::{ProviderDisplayName, ProviderId};
use serde_json::{Map, Value};
use std::fmt;

#[derive(Clone, Eq, PartialEq)]
struct FieldBuffer {
    value: Vec<char>,
    cursor: usize,
}

impl FieldBuffer {
    fn new() -> Self {
        Self {
            value: Vec::new(),
            cursor: 0,
        }
    }

    fn insert(&mut self, text: &str) {
        let inserted = text.chars().collect::<Vec<_>>();
        let count = inserted.len();
        self.value.splice(self.cursor..self.cursor, inserted);
        self.cursor += count;
    }

    fn backspace(&mut self) {
        if self.cursor != 0 {
            self.cursor -= 1;
            self.value.remove(self.cursor);
        }
    }

    fn delete(&mut self) {
        if self.cursor < self.value.len() {
            self.value.remove(self.cursor);
        }
    }

    fn text(&self) -> String {
        self.value.iter().collect()
    }

    fn display_cursor(&self, secret: bool) -> usize {
        if secret {
            return self.cursor;
        }
        text_width(&self.value[..self.cursor].iter().collect::<String>())
    }
}

/// One generic authentication form whose debug output never contains field values.
#[derive(Clone, Eq, PartialEq)]
pub(super) struct AuthPromptForm {
    fields: Vec<PromptField>,
    buffers: Vec<FieldBuffer>,
    active: usize,
}

impl fmt::Debug for AuthPromptForm {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AuthPromptForm")
            .field("field_count", &self.fields.len())
            .field("active", &self.active)
            .finish_non_exhaustive()
    }
}

impl AuthPromptForm {
    pub(super) fn new(fields: Vec<PromptField>) -> Self {
        let buffers = fields.iter().map(|_| FieldBuffer::new()).collect();
        Self {
            fields,
            buffers,
            active: 0,
        }
    }

    pub(super) fn insert(&mut self, text: &str) {
        if let Some(buffer) = self.buffers.get_mut(self.active) {
            buffer.insert(text);
        }
    }

    pub(super) fn backspace(&mut self) {
        if let Some(buffer) = self.buffers.get_mut(self.active) {
            buffer.backspace();
        }
    }

    pub(super) fn delete(&mut self) {
        if let Some(buffer) = self.buffers.get_mut(self.active) {
            buffer.delete();
        }
    }

    pub(super) fn move_left(&mut self) {
        if let Some(buffer) = self.buffers.get_mut(self.active) {
            buffer.cursor = buffer.cursor.saturating_sub(1);
        }
    }

    pub(super) fn move_right(&mut self) {
        if let Some(buffer) = self.buffers.get_mut(self.active) {
            buffer.cursor = (buffer.cursor + 1).min(buffer.value.len());
        }
    }

    pub(super) fn move_home(&mut self) {
        if let Some(buffer) = self.buffers.get_mut(self.active) {
            buffer.cursor = 0;
        }
    }

    pub(super) fn move_end(&mut self) {
        if let Some(buffer) = self.buffers.get_mut(self.active) {
            buffer.cursor = buffer.value.len();
        }
    }

    pub(super) fn previous(&mut self) {
        self.active = self.active.saturating_sub(1);
    }

    pub(super) fn next(&mut self) {
        self.active = (self.active + 1).min(self.fields.len().saturating_sub(1));
    }

    pub(super) fn advance_or_ready(&mut self) -> bool {
        if self.active + 1 < self.fields.len() {
            self.active += 1;
            false
        } else {
            true
        }
    }

    pub(super) fn into_completion(self) -> Map<String, Value> {
        self.fields
            .into_iter()
            .zip(self.buffers)
            .map(|(field, buffer)| (field.id, Value::String(buffer.text())))
            .collect()
    }

    fn secret_values(&self) -> Vec<String> {
        let mut values = self
            .fields
            .iter()
            .zip(&self.buffers)
            .filter(|(field, buffer)| field.secret && !buffer.value.is_empty())
            .map(|(_, buffer)| buffer.text())
            .collect::<Vec<_>>();
        values.sort_by_key(|value| std::cmp::Reverse(value.len()));
        values
    }

    pub(super) fn rows(&self) -> Vec<ListRowDisplay> {
        self.fields
            .iter()
            .zip(&self.buffers)
            .enumerate()
            .map(|(index, (field, buffer))| {
                let value = if field.secret {
                    "•".repeat(buffer.value.len())
                } else {
                    buffer.text()
                };
                ListRowDisplay {
                    number: index + 1,
                    label: field.label.clone(),
                    context: None,
                    pricing: None,
                    provider: None,
                    description: Some(if value.is_empty() {
                        "[ ]".to_owned()
                    } else {
                        format!("[{value}]")
                    }),
                    selected: index == self.active,
                    current: false,
                }
            })
            .collect()
    }

    pub(super) fn cursor_position(&self) -> Option<(usize, usize)> {
        let field = self.fields.get(self.active)?;
        let buffer = self.buffers.get(self.active)?;
        Some((self.active, 1 + buffer.display_cursor(field.secret)))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct AuthPromptView {
    pub(super) provider: ProviderId,
    pub(super) display_name: ProviderDisplayName,
    pub(super) method: String,
    pub(super) session: Value,
    pub(super) form: AuthPromptForm,
}

pub(super) struct AuthPromptSubmission {
    pub(super) provider: ProviderId,
    pub(super) method: String,
    pub(super) session: Value,
    pub(super) completion: Value,
    pub(super) secret_values: Vec<String>,
}

impl UiState {
    pub(super) fn open_auth_prompt(
        &mut self,
        provider: ProviderId,
        display_name: ProviderDisplayName,
        method: String,
        session: Value,
        fields: Vec<PromptField>,
    ) {
        self.view = Some(ActiveView::AuthPrompt(AuthPromptView {
            provider,
            display_name,
            method,
            session,
            form: AuthPromptForm::new(fields),
        }));
    }

    pub(super) fn finish_auth_prompt_field(&mut self) -> Option<AuthPromptSubmission> {
        let Some(ActiveView::AuthPrompt(view)) = &mut self.view else {
            return None;
        };
        if !view.form.advance_or_ready() {
            return None;
        }
        let Some(ActiveView::AuthPrompt(view)) = self.view.take() else {
            return None;
        };
        let submission = AuthPromptSubmission {
            provider: view.provider.clone(),
            method: view.method,
            session: view.session,
            secret_values: view.form.secret_values(),
            completion: Value::Object(view.form.into_completion()),
        };
        self.open_provider_settings(&view.provider);
        Some(submission)
    }

    pub(super) fn cancel_auth_prompt(&mut self) -> Option<ProviderId> {
        let Some(ActiveView::AuthPrompt(view)) = self.view.take() else {
            return None;
        };
        let provider = view.provider;
        self.open_provider_settings(&provider);
        Some(provider)
    }

    pub(super) fn auth_prompt_insert(&mut self, text: &str) {
        if let Some(ActiveView::AuthPrompt(view)) = &mut self.view {
            view.form.insert(text);
        }
    }

    pub(super) fn auth_prompt_key(&mut self, key: UiKey) {
        let Some(ActiveView::AuthPrompt(view)) = &mut self.view else {
            return;
        };
        match key {
            UiKey::Left => view.form.move_left(),
            UiKey::Right => view.form.move_right(),
            UiKey::Home => view.form.move_home(),
            UiKey::End => view.form.move_end(),
            UiKey::Backspace => view.form.backspace(),
            UiKey::Delete => view.form.delete(),
            UiKey::Up => view.form.previous(),
            UiKey::Down | UiKey::Tab => view.form.next(),
            _ => {}
        }
    }

    pub(super) fn auth_prompt_cursor_position(&self) -> Option<(usize, usize)> {
        let Some(ActiveView::AuthPrompt(view)) = &self.view else {
            return None;
        };
        view.form.cursor_position()
    }
}

#[cfg(test)]
mod tests {
    use super::AuthPromptForm;
    use crate::tui::auth_flow::PromptField;
    use serde_json::json;

    fn form() -> AuthPromptForm {
        AuthPromptForm::new(vec![
            PromptField {
                id: "secret".to_owned(),
                label: "Secret".to_owned(),
                secret: true,
            },
            PromptField {
                id: "tenant".to_owned(),
                label: "Tenant".to_owned(),
                secret: false,
            },
        ])
    }

    #[test]
    fn edits_unicode_fields_and_masks_only_secret_values() {
        let mut form = form();
        form.insert("a🔑c");
        form.move_left();
        form.backspace();
        form.insert("b");
        assert_eq!(form.rows()[0].description.as_deref(), Some("[•••]"));
        assert_eq!(form.cursor_position(), Some((0, 3)));
        assert!(!form.advance_or_ready());
        form.insert("acme");
        assert_eq!(form.rows()[1].description.as_deref(), Some("[acme]"));
        assert_eq!(form.cursor_position(), Some((1, 5)));
        assert!(form.advance_or_ready());
        assert_eq!(
            form.into_completion(),
            json!({"secret": "abc", "tenant": "acme"})
                .as_object()
                .cloned()
                .expect("completion object")
        );
    }

    #[test]
    fn debug_output_does_not_include_values() {
        let mut form = form();
        form.insert("sensitive-value");
        assert!(!format!("{form:?}").contains("sensitive-value"));
    }
}
