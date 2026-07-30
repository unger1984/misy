//! Shared filtering, selection, and scrolling for bottom-pane lists.

use std::cell::Cell;

/// One row in a modal list.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ListRow<T> {
    pub(super) value: Option<T>,
    pub(super) label: String,
    pub(super) context: Option<String>,
    pub(super) pricing: Option<String>,
    pub(super) provider: Option<String>,
    pub(super) description: Option<String>,
    pub(super) current: bool,
    search_terms: Vec<String>,
}

/// A list row ready for a renderer to lay out into aligned columns.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ListRowDisplay {
    pub(super) number: usize,
    pub(super) label: String,
    pub(super) context: Option<String>,
    pub(super) pricing: Option<String>,
    pub(super) provider: Option<String>,
    pub(super) description: Option<String>,
    pub(super) selected: bool,
    pub(super) current: bool,
}

impl<T> ListRow<T> {
    pub(super) fn selectable(
        value: T,
        label: impl Into<String>,
        description: Option<String>,
    ) -> Self {
        Self {
            value: Some(value),
            label: label.into(),
            context: None,
            pricing: None,
            provider: None,
            description,
            current: false,
            search_terms: Vec::new(),
        }
    }

    pub(super) fn with_model_columns(
        mut self,
        context: impl Into<String>,
        pricing: impl Into<String>,
        provider: impl Into<String>,
    ) -> Self {
        self.context = Some(context.into());
        self.pricing = Some(pricing.into());
        self.provider = Some(provider.into());
        self
    }

    pub(super) fn selectable_with_search(
        value: T,
        label: impl Into<String>,
        description: Option<String>,
        search_term: impl Into<String>,
    ) -> Self {
        let mut row = Self::selectable(value, label, description);
        row.search_terms.push(search_term.into());
        row
    }

    pub(super) fn current(value: T, label: impl Into<String>, description: Option<String>) -> Self {
        Self {
            value: Some(value),
            label: label.into(),
            context: None,
            pricing: None,
            provider: None,
            description,
            current: true,
            search_terms: Vec::new(),
        }
    }

    pub(super) fn current_with_search(
        value: T,
        label: impl Into<String>,
        description: Option<String>,
        search_term: impl Into<String>,
    ) -> Self {
        let mut row = Self::current(value, label, description);
        row.search_terms.push(search_term.into());
        row
    }

    pub(super) fn informational(label: impl Into<String>) -> Self {
        Self {
            value: None,
            label: label.into(),
            context: None,
            pricing: None,
            provider: None,
            description: None,
            current: false,
            search_terms: Vec::new(),
        }
    }
}

/// Generic state used by provider, provider-settings, and model views.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ListView<T> {
    title: String,
    rows: Vec<ListRow<T>>,
    filtered: Vec<usize>,
    query: String,
    selected: Option<usize>,
    // Rendering supplies the real viewport height, so this cache keeps navigation stable without
    // coupling input actions to terminal geometry.
    scroll_top: Cell<usize>,
}

impl<T> ListView<T> {
    pub(super) fn new(title: impl Into<String>, rows: Vec<ListRow<T>>) -> Self {
        let mut view = Self {
            title: title.into(),
            rows,
            filtered: Vec::new(),
            query: String::new(),
            selected: None,
            scroll_top: Cell::new(0),
        };
        view.apply_filter();
        view
    }

    pub(super) fn title(&self) -> &str {
        &self.title
    }

    pub(super) fn insert_filter(&mut self, text: &str) {
        self.query.push_str(text);
        self.apply_filter();
    }

    pub(super) fn set_filter(&mut self, query: String) {
        self.query = query;
        self.apply_filter();
    }

    /// Swaps the row set in place after a background refresh.
    ///
    /// Rebuilding the view from scratch would drop the typed filter and reset
    /// the highlight, so the query survives and the selection follows its
    /// value when that value still exists in the refreshed rows.
    pub(super) fn replace_rows(&mut self, rows: Vec<ListRow<T>>)
    where
        T: Clone + PartialEq,
    {
        let selected = self.selected_value().cloned();
        self.rows = rows;
        self.apply_filter();
        if let Some(value) = selected {
            self.select_value(&value);
            if self.selected.is_none() {
                self.selected = self.first_selectable();
            }
        }
    }

    pub(super) fn backspace_filter(&mut self) {
        self.query.pop();
        self.apply_filter();
    }

    pub(super) fn move_up(&mut self) {
        self.move_selection(-1);
    }

    pub(super) fn move_down(&mut self) {
        self.move_selection(1);
    }

    pub(super) fn selected_value(&self) -> Option<&T> {
        self.selected_row().and_then(|row| row.value.as_ref())
    }

    pub(super) fn select_value(&mut self, value: &T)
    where
        T: PartialEq,
    {
        self.selected = self
            .filtered
            .iter()
            .position(|index| self.rows[*index].value.as_ref() == Some(value));
    }

    pub(super) fn labels(&self) -> Vec<String> {
        self.filtered
            .iter()
            .map(|index| self.rows[*index].label.clone())
            .collect()
    }

    pub(super) fn visible_rows(&self, visible_rows: usize) -> Vec<ListRowDisplay> {
        let scroll_top = self.visible_scroll_top(visible_rows);
        self.filtered
            .iter()
            .enumerate()
            .skip(scroll_top)
            .take(visible_rows)
            .map(|(filtered_index, source_index)| {
                let row = &self.rows[*source_index];
                ListRowDisplay {
                    number: filtered_index + 1,
                    label: row.label.clone(),
                    context: row.context.clone(),
                    pricing: row.pricing.clone(),
                    provider: row.provider.clone(),
                    description: row.description.clone(),
                    selected: self.selected == Some(filtered_index),
                    current: row.current,
                }
            })
            .collect()
    }

    pub(super) fn select_number(&mut self, number: usize) -> bool {
        let Some(index) = number.checked_sub(1) else {
            return false;
        };
        let Some(source_index) = self.filtered.get(index) else {
            return false;
        };
        if self.rows[*source_index].value.is_none() {
            return false;
        }
        self.selected = Some(index);
        true
    }

    fn apply_filter(&mut self) {
        let query = self.query.to_lowercase();
        self.filtered = self
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| {
                query.is_empty()
                    || row.label.to_lowercase().contains(&query)
                    || row
                        .description
                        .as_ref()
                        .is_some_and(|description| description.to_lowercase().contains(&query))
                    || row
                        .search_terms
                        .iter()
                        .any(|term| term.to_lowercase().contains(&query))
            })
            .map(|(index, _)| index)
            .collect();
        self.selected = self.first_selectable();
        self.scroll_top.set(0);
    }

    fn selected_row(&self) -> Option<&ListRow<T>> {
        self.selected
            .and_then(|index| self.filtered.get(index))
            .and_then(|index| self.rows.get(*index))
    }

    fn first_selectable(&self) -> Option<usize> {
        self.filtered
            .iter()
            .position(|source_index| self.rows[*source_index].value.is_some())
    }

    fn move_selection(&mut self, direction: isize) {
        if self.filtered.is_empty() {
            self.selected = None;
            return;
        }
        let start = self.selected.unwrap_or(0);
        for step in 1..=self.filtered.len() {
            let index = if direction < 0 {
                (start + self.filtered.len() - (step % self.filtered.len())) % self.filtered.len()
            } else {
                (start + step) % self.filtered.len()
            };
            if self.rows[self.filtered[index]].value.is_some() {
                self.selected = Some(index);
                return;
            }
        }
    }

    fn visible_scroll_top(&self, visible_rows: usize) -> usize {
        let Some(selected) = self.selected else {
            self.scroll_top.set(0);
            return 0;
        };
        if visible_rows == 0 {
            return self.scroll_top.get();
        }
        let scroll_top = self.scroll_top.get();
        let next = if selected < scroll_top {
            selected
        } else if selected >= scroll_top.saturating_add(visible_rows) {
            selected + 1 - visible_rows
        } else {
            scroll_top
        };
        self.scroll_top.set(next);
        next
    }
}

#[cfg(test)]
mod tests {
    use super::{ListRow, ListView};

    #[test]
    fn filters_and_skips_informational_rows() {
        let mut view = ListView::new(
            "models",
            vec![
                ListRow::selectable(1, "alpha", None),
                ListRow::informational("provider failed"),
                ListRow::selectable(2, "beta", None),
            ],
        );
        view.move_down();
        assert_eq!(view.selected_value(), Some(&2));
        view.insert_filter("alp");
        assert_eq!(view.selected_value(), Some(&1));
    }

    #[test]
    fn keeps_eight_rows_visible_while_selection_scrolls() {
        let rows = (1..=9)
            .map(|number| ListRow::selectable(number, format!("Item {number}"), None))
            .collect();
        let mut view = ListView::new("items", rows);
        for _ in 0..8 {
            view.move_down();
        }
        let visible = view.visible_rows(8);
        assert_eq!(visible.first().map(|row| row.number), Some(2));
        assert_eq!(visible.last().map(|row| row.number), Some(9));
        assert!(visible.last().is_some_and(|row| row.selected));
    }

    #[test]
    fn selection_moves_inside_the_viewport_before_scrolling_back_up() {
        let rows = (1..=9)
            .map(|number| ListRow::selectable(number, format!("Item {number}"), None))
            .collect();
        let mut view = ListView::new("items", rows);
        for _ in 0..8 {
            view.move_down();
        }
        assert_eq!(view.visible_rows(4)[0].number, 6);

        for expected in [8, 7, 6] {
            view.move_up();
            let visible = view.visible_rows(4);
            assert_eq!(visible[0].number, 6);
            assert!(
                visible
                    .iter()
                    .any(|row| row.number == expected && row.selected)
            );
        }

        view.move_up();
        let visible = view.visible_rows(4);
        assert_eq!(visible[0].number, 5);
        assert!(visible[0].selected);
    }

    #[test]
    fn replace_rows_keeps_the_filter_and_follows_the_selected_value() {
        let mut view = ListView::new(
            "items",
            vec![
                ListRow::selectable(1, "alpha", None),
                ListRow::selectable(2, "alpine", None),
                ListRow::selectable(3, "beta", None),
            ],
        );
        view.insert_filter("alp");
        view.move_down();
        assert_eq!(view.selected_value(), Some(&2));

        view.replace_rows(vec![
            ListRow::selectable(1, "alpha", Some("updated".to_owned())),
            ListRow::selectable(2, "alpine", None),
            ListRow::selectable(3, "beta", None),
        ]);

        assert_eq!(view.labels(), ["alpha", "alpine"]);
        assert_eq!(view.selected_value(), Some(&2));
    }

    #[test]
    fn replace_rows_falls_back_to_the_first_row_when_the_value_disappears() {
        let mut view = ListView::new(
            "items",
            vec![
                ListRow::selectable(1, "alpha", None),
                ListRow::selectable(2, "beta", None),
            ],
        );
        view.move_down();
        assert_eq!(view.selected_value(), Some(&2));

        view.replace_rows(vec![ListRow::selectable(1, "alpha", None)]);

        assert_eq!(view.selected_value(), Some(&1));
    }
}
