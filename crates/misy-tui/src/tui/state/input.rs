//! Composer geometry and command-popup projections used by rendering.

use super::UiState;

impl UiState {
    pub(in crate::tui) fn composer_cursor_position(&self) -> (u16, u16) {
        self.composer.cursor_position()
    }

    pub(in crate::tui) fn composer_line_count(&self) -> usize {
        self.composer.text().split('\n').count()
    }

    pub(in crate::tui) fn command_popup_rows_for_render(
        &self,
    ) -> Vec<crate::tui::composer::CommandPopupRow> {
        if self.view.is_some() {
            return Vec::new();
        }
        self.composer.popup_rows_for_render()
    }
}
