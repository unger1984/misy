//! Bounded semantic transcript storage for child-agent sessions.

use super::{AgentSummary, AgentTranscript, AgentTranscriptEntry, AgentTranscriptEntryKind};
use crate::{ToolCall, ToolResult};
use std::collections::VecDeque;

const MAX_TRANSCRIPT_BYTES: usize = 1024 * 1024;
const TRUNCATION_MARKER_BYTES: usize = 64;

/// Mutable bounded transcript retained independently from provider history.
#[derive(Debug, Default)]
pub(super) struct TranscriptBuffer {
    entries: VecDeque<AgentTranscriptEntry>,
    retained_bytes: usize,
    truncated: bool,
}

impl TranscriptBuffer {
    pub(super) fn assignment(task: &str, attachment_count: usize) -> Self {
        let mut transcript = Self::default();
        transcript.push(entry(
            AgentTranscriptEntryKind::Assignment,
            task,
            None,
            None,
            attachment_count,
        ));
        transcript
    }

    pub(super) fn push_user_message(&mut self, message: &str) {
        self.push(entry(
            AgentTranscriptEntryKind::UserMessage,
            message,
            None,
            None,
            0,
        ));
    }

    pub(super) fn push_assistant(&mut self, text: &str) {
        self.push(entry(
            AgentTranscriptEntryKind::Assistant,
            text,
            None,
            None,
            0,
        ));
    }

    pub(super) fn push_tool_call(&mut self, call: &ToolCall) {
        self.push(entry(
            AgentTranscriptEntryKind::ToolCall,
            &call.name,
            Some(call.clone()),
            None,
            0,
        ));
    }

    pub(super) fn push_tool_result(&mut self, result: &ToolResult) {
        let attachment_count = result.attachments.len();
        let mut result = result.clone();
        result.attachments.clear();
        let content = result.content.clone();
        self.push(entry(
            AgentTranscriptEntryKind::ToolResult,
            &content,
            None,
            Some(result),
            attachment_count,
        ));
    }

    pub(super) fn push_terminal(&mut self, message: &str) {
        self.push(entry(
            AgentTranscriptEntryKind::Terminal,
            message,
            None,
            None,
            0,
        ));
    }

    pub(super) fn snapshot(&self, agent: AgentSummary) -> AgentTranscript {
        AgentTranscript {
            agent,
            entries: self.entries.iter().cloned().collect(),
            truncated: self.truncated,
        }
    }

    fn push(&mut self, mut entry: AgentTranscriptEntry) {
        fit_single_entry(&mut entry);
        let entry_bytes = encoded_len(&entry);
        self.entries.push_back(entry);
        self.retained_bytes = self.retained_bytes.saturating_add(entry_bytes);
        self.trim();
    }

    fn trim(&mut self) {
        while self.retained_bytes.saturating_add(TRUNCATION_MARKER_BYTES) > MAX_TRANSCRIPT_BYTES {
            let removal_index = usize::from(self.entries.len() > 1);
            let Some(removed) = self.entries.remove(removal_index) else {
                break;
            };
            self.retained_bytes = self.retained_bytes.saturating_sub(encoded_len(&removed));
            self.truncated = true;
        }
    }
}

fn entry(
    kind: AgentTranscriptEntryKind,
    content: &str,
    tool_call: Option<ToolCall>,
    tool_result: Option<ToolResult>,
    attachment_count: usize,
) -> AgentTranscriptEntry {
    AgentTranscriptEntry {
        kind,
        content: content.to_owned(),
        tool_call,
        tool_result,
        attachment_count,
    }
}

fn fit_single_entry(entry: &mut AgentTranscriptEntry) {
    if encoded_len(entry) <= MAX_TRANSCRIPT_BYTES / 2 {
        return;
    }
    entry.tool_call = None;
    entry.tool_result = None;
    let maximum_chars = MAX_TRANSCRIPT_BYTES / 4;
    entry.content = entry.content.chars().take(maximum_chars).collect();
    entry
        .content
        .push_str("\n[... transcript entry truncated ...]");
}

fn encoded_len(entry: &AgentTranscriptEntry) -> usize {
    serde_json::to_vec(entry).map_or(entry.content.len(), |encoded| encoded.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ActivityId, ActivityStatus, AgentId, ModelId, ModelRef, ProviderId};

    fn summary() -> AgentSummary {
        AgentSummary {
            id: AgentId::new(1),
            activity_id: ActivityId::new(1),
            title: "fixture".to_owned(),
            model: ModelRef::new(ProviderId::new("fixture"), ModelId::new("model")),
            status: ActivityStatus::Running,
            run_in_background: true,
            started_at_ms: 1,
            finished_at_ms: None,
            terminal_message: None,
        }
    }

    #[test]
    fn keeps_assignment_and_bounds_large_semantic_entries() {
        let mut transcript = TranscriptBuffer::assignment("assignment", 0);
        for _ in 0..8 {
            transcript.push_assistant(&"x".repeat(MAX_TRANSCRIPT_BYTES / 2));
        }
        let snapshot = transcript.snapshot(summary());
        assert_eq!(snapshot.entries[0].content, "assignment");
        assert!(snapshot.truncated);
        assert!(
            serde_json::to_vec(&snapshot)
                .expect("encode transcript")
                .len()
                < 2 * 1024 * 1024
        );
    }

    #[test]
    fn strips_image_payloads_from_tool_results() {
        let mut transcript = TranscriptBuffer::assignment("assignment", 0);
        let result = ToolResult::success("call", "done");
        transcript.push_tool_result(&result);
        let snapshot = transcript.snapshot(summary());
        assert!(snapshot.entries[1].tool_result.as_ref().is_some());
        assert_eq!(snapshot.entries[1].attachment_count, 0);
    }
}
