//! Activity-specific state transitions and modal presentation.

use super::{ActiveView, ModalPresentation, UiState};
use crate::tui::{
    activity_picker::{ActivityChoice, ActivityPicker},
    list::ListRowDisplay,
};
use misy_core::{
    ActivityId, ActivityKind, ActivityOutput, ActivityOutputStream, ActivityStatus, AgentId,
    AgentTranscript, AgentTranscriptEntryKind,
};

const PREVIEW_LINES: usize = 6;
const PAGE_SCROLL_LINES: usize = 10;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::tui) struct ActivityLogView {
    pub(super) picker: ActivityPicker,
    pub(super) id: ActivityId,
    pub(super) agent_id: Option<AgentId>,
    pub(super) output: Option<ActivityOutput>,
    transcript: Option<AgentTranscriptView>,
    from_tail: usize,
    follow_tail: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct AgentTranscriptView {
    title: String,
    labels: Vec<String>,
}

impl From<&AgentTranscript> for AgentTranscriptView {
    fn from(transcript: &AgentTranscript) -> Self {
        Self {
            title: transcript.agent.title.clone(),
            labels: agent_transcript_labels(transcript),
        }
    }
}

impl ActivityLogView {
    fn new(picker: ActivityPicker, id: ActivityId, output: Option<ActivityOutput>) -> Self {
        Self {
            picker,
            id,
            agent_id: None,
            output,
            transcript: None,
            from_tail: 0,
            follow_tail: true,
        }
    }

    pub(super) fn scroll_up(&mut self, lines: usize) {
        self.follow_tail = false;
        self.from_tail = self.from_tail.saturating_add(lines);
    }

    pub(super) fn scroll_down(&mut self, lines: usize) {
        self.from_tail = self.from_tail.saturating_sub(lines);
        if self.from_tail == 0 {
            self.follow_tail = true;
        }
    }

    fn visible_lines(&self, rows: usize) -> Vec<String> {
        let labels = if let Some(transcript) = &self.transcript {
            transcript.labels.clone()
        } else if let Some(output) = &self.output {
            output_labels(output)
        } else {
            vec!["Loading output…".to_owned()]
        };
        let maximum_top = labels.len().saturating_sub(rows);
        let top = if self.follow_tail {
            maximum_top
        } else {
            maximum_top.saturating_sub(self.from_tail)
        };
        labels.into_iter().skip(top).take(rows).collect()
    }
}

impl UiState {
    pub(in crate::tui) fn open_activities(&mut self) {
        self.activity_bar_focused = false;
        self.activity_preview = None;
        self.view = Some(ActiveView::Activities(ActivityPicker::new(
            self.snapshot.activities.clone(),
            self.snapshot.agents.clone(),
        )));
    }

    pub(in crate::tui) fn activity_counts(&self) -> (usize, usize, usize) {
        self.snapshot.activities.iter().fold(
            (0, 0, 0),
            |(running, completed, agents), item| match (item.kind, item.status.is_terminal()) {
                (ActivityKind::Task, false) => (running + 1, completed, agents),
                (ActivityKind::Task, true) => (running, completed + 1, agents),
                (ActivityKind::Agent, false) => (running, completed, agents + 1),
                (ActivityKind::Agent, true) => (running, completed, agents),
            },
        )
    }

    pub(in crate::tui) fn activity_bar_visible(&self) -> bool {
        !self.snapshot.activities.is_empty()
    }

    pub(in crate::tui) fn activity_bar_label(&self) -> String {
        let (running, completed, agents) = self.activity_counts();
        let marker = if self.activity_bar_focused {
            "›"
        } else {
            "↓"
        };
        format!("  {marker} ({running} running tasks · {completed} completed · {agents} agents)")
    }

    pub(in crate::tui) fn selected_activity_choice(&self) -> Option<ActivityChoice> {
        match &self.view {
            Some(ActiveView::Activities(picker)) => picker.selected(),
            _ => None,
        }
    }

    pub(in crate::tui) fn open_activity_detail(&mut self, id: ActivityId) {
        let Some(ActiveView::Activities(picker)) = self.view.take() else {
            return;
        };
        let output = self
            .activity_preview
            .take()
            .filter(|output| output.activity.id == id);
        self.view = Some(ActiveView::ActivityLog(Box::new(ActivityLogView::new(
            picker, id, output,
        ))));
    }

    pub(in crate::tui) fn open_agent_detail(&mut self, id: ActivityId, agent_id: AgentId) {
        let Some(ActiveView::Activities(picker)) = self.view.take() else {
            return;
        };
        let transcript = self
            .agent_preview
            .take()
            .filter(|transcript| transcript.agent.id == agent_id);
        let mut view = ActivityLogView::new(picker, id, None);
        view.agent_id = Some(agent_id);
        view.transcript = transcript.as_ref().map(AgentTranscriptView::from);
        self.view = Some(ActiveView::ActivityLog(Box::new(view)));
    }

    pub(in crate::tui) fn activity_detail_id(&self) -> Option<ActivityId> {
        match &self.view {
            Some(ActiveView::ActivityLog(view)) => Some(view.id),
            _ => None,
        }
    }

    pub(in crate::tui) fn activity_output_target(&self) -> Option<ActivityId> {
        match &self.view {
            Some(ActiveView::Activities(picker)) => match picker.selected() {
                Some(ActivityChoice::Activity(id)) => Some(id),
                Some(ActivityChoice::Agent(_, _)) | Some(ActivityChoice::Main) | None => None,
            },
            Some(ActiveView::ActivityLog(view)) if view.agent_id.is_none() => Some(view.id),
            _ => None,
        }
    }

    pub(in crate::tui) fn agent_transcript_target(&self) -> Option<AgentId> {
        match &self.view {
            Some(ActiveView::Activities(picker)) => match picker.selected() {
                Some(ActivityChoice::Agent(_, id)) => Some(id),
                _ => None,
            },
            Some(ActiveView::ActivityLog(view)) => view.agent_id,
            _ => None,
        }
    }

    pub(in crate::tui) fn set_activity_output(&mut self, output: ActivityOutput) {
        match &mut self.view {
            Some(ActiveView::Activities(picker))
                if picker.selected() == Some(ActivityChoice::Activity(output.activity.id)) =>
            {
                self.activity_preview = Some(output);
            }
            Some(ActiveView::ActivityLog(view)) if view.id == output.activity.id => {
                view.output = Some(output);
            }
            _ => {}
        }
    }

    pub(in crate::tui) fn set_agent_transcript(&mut self, transcript: AgentTranscript) {
        match &mut self.view {
            Some(ActiveView::Activities(picker))
                if matches!(
                    picker.selected(),
                    Some(ActivityChoice::Agent(_, id)) if id == transcript.agent.id
                ) =>
            {
                self.agent_preview = Some(transcript);
            }
            Some(ActiveView::ActivityLog(view)) if view.agent_id == Some(transcript.agent.id) => {
                view.transcript = Some(AgentTranscriptView::from(&transcript));
            }
            _ => {}
        }
    }

    pub(in crate::tui) fn selected_activity_to_stop(&self) -> Option<ActivityId> {
        let id = match &self.view {
            Some(ActiveView::Activities(picker)) => match picker.selected() {
                Some(ActivityChoice::Activity(id)) => id,
                Some(ActivityChoice::Agent(id, _)) => id,
                Some(ActivityChoice::Main) | None => return None,
            },
            Some(ActiveView::ActivityLog(view)) => view.id,
            _ => return None,
        };
        self.snapshot
            .activities
            .iter()
            .find(|activity| activity.id == id && !activity.status.is_terminal())
            .map(|activity| activity.id)
    }
}

pub(super) fn activity_presentation(
    picker: &ActivityPicker,
    preview: Option<&ActivityOutput>,
    agent_preview: Option<&AgentTranscript>,
    visible_rows: usize,
    stop_hint: &str,
) -> ModalPresentation {
    let preview = preview
        .filter(|output| picker.selected() == Some(ActivityChoice::Activity(output.activity.id)));
    let agent_preview = agent_preview.filter(|transcript| {
        matches!(
            picker.selected(),
            Some(ActivityChoice::Agent(_, id)) if id == transcript.agent.id
        )
    });
    let preview_rows = if preview.is_some() || agent_preview.is_some() {
        PREVIEW_LINES.saturating_add(1)
    } else {
        0
    };
    let list_rows = visible_rows.saturating_sub(preview_rows).max(1);
    let mut rows = picker.visible_rows(list_rows);
    if let Some(output) = preview {
        rows.push(information_row(format!(
            "Preview · {} · {}",
            output.activity.id,
            status_label(output.activity.status)
        )));
        let labels = output_labels(output);
        let tail = labels
            .into_iter()
            .rev()
            .take(PREVIEW_LINES)
            .collect::<Vec<_>>();
        rows.extend(tail.into_iter().rev().map(information_row));
    } else if let Some(transcript) = agent_preview {
        rows.push(information_row(format!(
            "Preview · {} · {}",
            transcript.agent.id,
            status_label(transcript.agent.status)
        )));
        let labels = agent_transcript_labels(transcript);
        let tail = labels
            .into_iter()
            .rev()
            .take(PREVIEW_LINES)
            .collect::<Vec<_>>();
        rows.extend(tail.into_iter().rev().map(information_row));
    }
    ModalPresentation {
        title: "Activities".to_owned(),
        rows,
        operation: None,
        back_hint: false,
        tabs: picker.tabs(),
        loading: false,
        help_hint: Some(format!(
            "↑↓ select  ←→ section  enter open  {stop_hint} stop  esc close"
        )),
    }
}

fn information_row(label: String) -> ListRowDisplay {
    ListRowDisplay {
        number: 0,
        label,
        context: None,
        pricing: None,
        provider: None,
        description: None,
        selected: false,
        current: false,
    }
}

fn status_label(status: ActivityStatus) -> &'static str {
    match status {
        ActivityStatus::Queued => "queued",
        ActivityStatus::Running => "running",
        ActivityStatus::Waiting => "waiting",
        ActivityStatus::Completed => "completed",
        ActivityStatus::Failed => "failed",
        ActivityStatus::Stopped => "stopped",
    }
}

pub(super) fn output_labels(output: &ActivityOutput) -> Vec<String> {
    let mut labels = vec![format!(
        "status: {:?}{}",
        output.activity.status,
        output
            .activity
            .exit_code
            .map(|code| format!(" · exit {code}"))
            .unwrap_or_default()
    )];
    labels.extend(output_content_labels(output));
    labels
}

pub(in crate::tui) fn output_content_labels(output: &ActivityOutput) -> Vec<String> {
    let mut labels = Vec::new();
    if output.fragments.is_empty() {
        labels.extend(output.stdout.lines().map(ToOwned::to_owned));
        labels.extend(output.stderr.lines().map(|line| format!("stderr: {line}")));
    } else {
        for fragment in &output.fragments {
            match fragment.stream {
                ActivityOutputStream::Stdout | ActivityOutputStream::System => {
                    labels.extend(fragment.text.lines().map(ToOwned::to_owned));
                }
                ActivityOutputStream::Stderr => {
                    labels.extend(fragment.text.lines().map(|line| format!("stderr: {line}")));
                }
            }
        }
    }
    if let Some(message) = &output.message {
        labels.push(message.clone());
    }
    labels
}

fn agent_transcript_labels(transcript: &AgentTranscript) -> Vec<String> {
    let mut labels = Vec::new();
    for entry in &transcript.entries {
        let prefix = match entry.kind {
            AgentTranscriptEntryKind::Assignment => "assignment",
            AgentTranscriptEntryKind::UserMessage => "message",
            AgentTranscriptEntryKind::Assistant => "assistant",
            AgentTranscriptEntryKind::ToolCall => "tool call",
            AgentTranscriptEntryKind::ToolResult => "tool result",
            AgentTranscriptEntryKind::Terminal => "terminal",
        };
        let attachment = if entry.attachment_count == 0 {
            String::new()
        } else {
            format!(" [{} attachment(s)]", entry.attachment_count)
        };
        let content = if entry.content.is_empty() {
            attachment.trim_start().to_owned()
        } else {
            format!("{}{attachment}", entry.content)
        };
        labels.extend(content.lines().map(|line| format!("{prefix}: {line}")));
    }
    if transcript.truncated {
        labels.insert(0, "… older transcript entries omitted …".to_owned());
    }
    labels
}

impl UiState {
    pub(in crate::tui) fn scroll_activity_log_up(&mut self, page: bool) {
        if let Some(ActiveView::ActivityLog(view)) = &mut self.view {
            view.scroll_up(if page { PAGE_SCROLL_LINES } else { 1 });
        }
    }

    pub(in crate::tui) fn scroll_activity_log_down(&mut self, page: bool) {
        if let Some(ActiveView::ActivityLog(view)) = &mut self.view {
            view.scroll_down(if page { PAGE_SCROLL_LINES } else { 1 });
        }
    }

    pub(in crate::tui) fn activity_log_lines(&self, rows: usize) -> Vec<String> {
        match &self.view {
            Some(ActiveView::ActivityLog(view)) => view.visible_lines(rows),
            _ => Vec::new(),
        }
    }

    pub(in crate::tui) fn activity_log_title(&self) -> Option<String> {
        let ActiveView::ActivityLog(view) = self.view.as_ref()? else {
            return None;
        };
        let title = view
            .transcript
            .as_ref()
            .map(|transcript| transcript.title.as_str())
            .or_else(|| {
                view.output
                    .as_ref()
                    .map(|output| output.activity.title.as_str())
            })
            .unwrap_or(if view.agent_id.is_some() {
                "Agent transcript"
            } else {
                "Task output"
            });
        let id = view
            .agent_id
            .map_or_else(|| view.id.to_string(), |id| id.to_string());
        Some(format!("{id} — {title}"))
    }
}
