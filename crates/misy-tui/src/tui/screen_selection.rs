//! Full-screen terminal buffer selection for mouse-driven copy operations.

use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    style::Modifier,
};
use unicode_width::UnicodeWidthStr;

/// Holds the rendered terminal surface and the active mouse selection.
#[derive(Debug, Default)]
pub(super) struct ScreenSelection {
    base_buffer: Option<Buffer>,
    anchor: Option<Position>,
    cursor: Option<Position>,
    dragged: bool,
}

impl ScreenSelection {
    /// Saves the unmodified surface used to extract text when a drag completes.
    pub(super) fn capture_buffer(&mut self, buffer: &Buffer) {
        self.base_buffer = Some(buffer.clone());
    }

    /// Starts a new inclusive selection at the mouse position.
    pub(super) fn begin(&mut self, position: Position) {
        self.anchor = Some(position);
        self.cursor = Some(position);
        self.dragged = false;
    }

    /// Extends the active selection to the current mouse position.
    pub(super) fn drag(&mut self, position: Position) {
        if let Some(anchor) = self.anchor {
            self.dragged |= position != anchor;
            self.cursor = Some(position);
        }
    }

    /// Captures the unmodified frame and overlays any active drag selection.
    pub(super) fn render(&mut self, buffer: &mut Buffer) {
        if self.anchor.is_some() {
            self.capture_buffer(buffer);
            self.highlight(buffer);
        }
    }

    /// Applies the active selection highlight to a newly rendered terminal surface.
    pub(super) fn highlight(&self, buffer: &mut Buffer) {
        let Some(selection) = self
            .dragged
            .then(|| self.selection_for(buffer.area))
            .flatten()
        else {
            return;
        };

        for y in selection.start.y..=selection.end.y {
            let (start_x, end_x) = selection.row_bounds(y);
            highlight_row(buffer, y, start_x, end_x);
        }
    }

    /// Extracts the selected text from the latest base surface and clears the selection.
    #[must_use]
    pub(super) fn finish(&mut self, position: Position) -> Option<String> {
        self.drag(position);
        let text = self
            .dragged
            .then(|| {
                self.base_buffer.as_ref().and_then(|buffer| {
                    self.selection_for(buffer.area)
                        .map(|selection| extract_selection(buffer, selection))
                })
            })
            .flatten()
            .filter(|text| !text.is_empty());
        self.clear();
        text
    }

    fn clear(&mut self) {
        self.anchor = None;
        self.cursor = None;
        self.dragged = false;
    }

    fn selection_for(&self, area: Rect) -> Option<Selection> {
        let anchor = clamp_to_area(self.anchor?, area)?;
        let cursor = clamp_to_area(self.cursor?, area)?;
        Some(Selection::new(anchor, cursor, area))
    }
}

#[derive(Debug, Clone, Copy)]
struct Selection {
    start: Position,
    end: Position,
    area: Rect,
}

impl Selection {
    fn new(anchor: Position, cursor: Position, area: Rect) -> Self {
        let (start, end) = if comes_before(anchor, cursor) {
            (anchor, cursor)
        } else {
            (cursor, anchor)
        };
        Self { start, end, area }
    }

    fn row_bounds(self, y: u16) -> (u16, u16) {
        let start_x = if y == self.start.y {
            self.start.x
        } else {
            self.area.left()
        };
        let end_x = if y == self.end.y {
            self.end.x
        } else {
            self.area.right() - 1
        };
        (start_x, end_x)
    }
}

fn comes_before(left: Position, right: Position) -> bool {
    (left.y, left.x) <= (right.y, right.x)
}

fn clamp_to_area(position: Position, area: Rect) -> Option<Position> {
    if area.is_empty() {
        return None;
    }
    Some(Position::new(
        position.x.clamp(area.left(), area.right() - 1),
        position.y.clamp(area.top(), area.bottom() - 1),
    ))
}

fn extract_selection(buffer: &Buffer, selection: Selection) -> String {
    let mut lines = Vec::new();
    for y in selection.start.y..=selection.end.y {
        let (start_x, end_x) = selection.row_bounds(y);
        lines.push(extract_row(buffer, y, start_x, end_x));
    }
    lines.join("\n")
}

fn extract_row(buffer: &Buffer, y: u16, start_x: u16, end_x: u16) -> String {
    let mut text = String::new();
    let mut x = buffer.area.left();
    while x <= end_x {
        let Some(cell) = buffer.cell(Position::new(x, y)) else {
            break;
        };
        let symbol = cell.symbol();
        let width = u16::try_from(symbol.width()).unwrap_or(u16::MAX).max(1);
        let symbol_end = x.saturating_add(width.saturating_sub(1));
        if symbol_end >= start_x {
            text.push_str(symbol);
        }
        let next_x = x.saturating_add(width);
        if next_x <= x {
            break;
        }
        x = next_x;
    }
    text.trim_end_matches(' ').to_owned()
}

fn highlight_row(buffer: &mut Buffer, y: u16, start_x: u16, end_x: u16) {
    let mut x = buffer.area.left();
    while x <= end_x {
        let Some(cell) = buffer.cell(Position::new(x, y)) else {
            break;
        };
        let width = u16::try_from(cell.symbol().width())
            .unwrap_or(u16::MAX)
            .max(1);
        let symbol_end = x.saturating_add(width.saturating_sub(1));
        if symbol_end >= start_x {
            for selected_x in x.max(start_x)..=symbol_end.min(buffer.area.right() - 1) {
                if let Some(selected) = buffer.cell_mut(Position::new(selected_x, y)) {
                    selected.set_style(selected.style().add_modifier(Modifier::REVERSED));
                }
            }
        }
        let next_x = x.saturating_add(width);
        if next_x <= x {
            break;
        }
        x = next_x;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selection_with(lines: &[&str]) -> (ScreenSelection, Buffer) {
        let buffer = Buffer::with_lines(lines.iter().copied());
        let mut selection = ScreenSelection::default();
        selection.capture_buffer(&buffer);
        (selection, buffer)
    }

    #[test]
    fn highlights_the_inclusive_normalized_drag_range() {
        let (mut selection, mut buffer) = selection_with(&["abcd", "efgh", "ijkl"]);
        selection.begin(Position::new(2, 2));
        selection.drag(Position::new(1, 0));

        selection.highlight(&mut buffer);

        for position in [
            Position::new(1, 0),
            Position::new(2, 0),
            Position::new(3, 0),
            Position::new(0, 1),
            Position::new(3, 1),
            Position::new(0, 2),
            Position::new(2, 2),
        ] {
            assert!(
                buffer
                    .cell(position)
                    .expect("selected cell")
                    .modifier
                    .contains(Modifier::REVERSED)
            );
        }
        assert!(
            !buffer
                .cell(Position::new(0, 0))
                .expect("unselected cell")
                .modifier
                .contains(Modifier::REVERSED)
        );
        assert!(
            !buffer
                .cell(Position::new(3, 2))
                .expect("unselected cell")
                .modifier
                .contains(Modifier::REVERSED)
        );
    }

    #[test]
    fn finish_extracts_rows_without_right_padding_and_clears_the_selection() {
        let (mut selection, _) = selection_with(&["alpha  ", "bravo  ", "charlie"]);
        selection.begin(Position::new(2, 0));
        selection.drag(Position::new(2, 2));

        assert_eq!(
            selection.finish(Position::new(2, 2)),
            Some("pha\nbravo\ncha".to_owned())
        );
        assert!(selection.anchor.is_none());
    }

    #[test]
    fn finish_skips_cells_covered_by_wide_symbols() {
        let (mut selection, _) = selection_with(&["A猫B"]);
        selection.begin(Position::new(0, 0));
        selection.drag(Position::new(3, 0));

        assert_eq!(
            selection.finish(Position::new(3, 0)),
            Some("A猫B".to_owned())
        );
    }

    #[test]
    fn selecting_a_wide_symbol_continuation_includes_the_whole_symbol() {
        let (mut selection, mut buffer) = selection_with(&["A猫B"]);
        selection.begin(Position::new(1, 0));
        selection.drag(Position::new(2, 0));
        selection.highlight(&mut buffer);

        for x in 1..=2 {
            assert!(
                buffer
                    .cell(Position::new(x, 0))
                    .expect("wide symbol cell")
                    .modifier
                    .contains(Modifier::REVERSED)
            );
        }
        assert_eq!(selection.finish(Position::new(2, 0)), Some("猫".to_owned()));
    }

    #[test]
    fn a_plain_click_does_not_copy_or_highlight() {
        let (mut selection, mut buffer) = selection_with(&["abcd"]);
        selection.begin(Position::new(1, 0));

        selection.highlight(&mut buffer);

        assert_eq!(selection.finish(Position::new(1, 0)), None);
        assert!(
            !buffer
                .cell(Position::new(1, 0))
                .expect("clicked cell")
                .modifier
                .contains(Modifier::REVERSED)
        );
    }
}
