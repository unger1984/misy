//! Filterable activity list shared by tasks and future agent sessions.

use super::list::{ListRow, ListRowDisplay, ListView};
use misy_core::{ActivityId, ActivityKind, ActivityStatus, ActivitySummary};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ActivityChoice {
    Main,
    Activity(ActivityId),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ActivityTab {
    All,
    Agents,
    Tasks,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ActivityPicker {
    activities: Vec<ActivitySummary>,
    tab: ActivityTab,
    view: ListView<ActivityChoice>,
}

impl ActivityPicker {
    pub(super) fn new(activities: Vec<ActivitySummary>) -> Self {
        let tab = ActivityTab::All;
        let view = ListView::new("Activities", rows(&activities, tab));
        Self {
            activities,
            tab,
            view,
        }
    }

    pub(super) fn refresh(&mut self, activities: Vec<ActivitySummary>) {
        self.activities = activities;
        self.view.replace_rows(rows(&self.activities, self.tab));
    }

    pub(super) fn insert_filter(&mut self, text: &str) {
        self.view.insert_filter(text);
    }

    pub(super) fn backspace_filter(&mut self) {
        self.view.backspace_filter();
    }

    pub(super) fn move_up(&mut self) {
        self.view.move_up();
    }

    pub(super) fn move_down(&mut self) {
        self.view.move_down();
    }

    pub(super) fn tab_left(&mut self) {
        self.set_tab(match self.tab {
            ActivityTab::All => ActivityTab::Tasks,
            ActivityTab::Agents => ActivityTab::All,
            ActivityTab::Tasks => ActivityTab::Agents,
        });
    }

    pub(super) fn tab_right(&mut self) {
        self.set_tab(match self.tab {
            ActivityTab::All => ActivityTab::Agents,
            ActivityTab::Agents => ActivityTab::Tasks,
            ActivityTab::Tasks => ActivityTab::All,
        });
    }

    pub(super) fn tabs(&self) -> Vec<(String, bool)> {
        [
            ("All", ActivityTab::All),
            ("Agents", ActivityTab::Agents),
            ("Tasks", ActivityTab::Tasks),
        ]
        .into_iter()
        .map(|(label, tab)| (label.to_owned(), tab == self.tab))
        .collect()
    }

    pub(super) fn visible_rows(&self, maximum: usize) -> Vec<ListRowDisplay> {
        self.view.visible_rows(maximum)
    }

    pub(super) fn selected(&self) -> Option<ActivityChoice> {
        self.view.selected_value().copied()
    }

    pub(super) fn select_number(&mut self, number: usize) -> bool {
        self.view.select_number(number)
    }

    fn set_tab(&mut self, tab: ActivityTab) {
        self.tab = tab;
        self.view.replace_rows(rows(&self.activities, tab));
    }
}

fn rows(activities: &[ActivitySummary], tab: ActivityTab) -> Vec<ListRow<ActivityChoice>> {
    let mut rows = Vec::new();
    if tab != ActivityTab::Tasks {
        rows.push(ListRow::current(
            ActivityChoice::Main,
            "Main",
            Some("current session".to_owned()),
        ));
    }
    rows.extend(
        activities
            .iter()
            .filter(|activity| included(activity.kind, tab))
            .map(activity_row),
    );
    if rows.is_empty() {
        rows.push(ListRow::informational("No matching activities"));
    }
    rows
}

fn included(kind: ActivityKind, tab: ActivityTab) -> bool {
    match tab {
        ActivityTab::All => true,
        ActivityTab::Agents => kind == ActivityKind::Agent,
        ActivityTab::Tasks => kind == ActivityKind::Task,
    }
}

fn activity_row(activity: &ActivitySummary) -> ListRow<ActivityChoice> {
    let status = match activity.status {
        ActivityStatus::Queued => "queued",
        ActivityStatus::Running => "running",
        ActivityStatus::Waiting => "waiting",
        ActivityStatus::Completed => "completed",
        ActivityStatus::Failed => "failed",
        ActivityStatus::Stopped => "stopped",
    };
    ListRow::selectable_with_search(
        ActivityChoice::Activity(activity.id),
        format!("{} {}", status_marker(activity.status), activity.title),
        Some(format!("{} · {status}", activity.id)),
        activity.cwd.clone().unwrap_or_default(),
    )
}

fn status_marker(status: ActivityStatus) -> &'static str {
    match status {
        ActivityStatus::Queued | ActivityStatus::Running | ActivityStatus::Waiting => "○",
        ActivityStatus::Completed => "●",
        ActivityStatus::Failed => "×",
        ActivityStatus::Stopped => "■",
    }
}

#[cfg(test)]
mod tests {
    use super::ActivityPicker;
    use misy_core::ActivitySummary;

    fn activity(status: &str) -> ActivitySummary {
        serde_json::from_value(serde_json::json!({
            "id": 1,
            "kind": "task",
            "status": status,
            "title": "fixture task",
            "cwd": null,
            "started_at_ms": 1,
            "exit_code": null
        }))
        .expect("deserialize activity fixture")
    }

    #[test]
    fn distinguishes_running_completed_and_failed_tasks_with_markers() {
        let running = ActivityPicker::new(vec![activity("running")]);
        assert_eq!(running.visible_rows(8)[1].label, "○ fixture task");
        let completed = ActivityPicker::new(vec![activity("completed")]);
        assert_eq!(completed.visible_rows(8)[1].label, "● fixture task");
        let failed = ActivityPicker::new(vec![activity("failed")]);
        assert_eq!(failed.visible_rows(8)[1].label, "× fixture task");
    }
}
