//! Deterministic token estimates and context-window maintenance thresholds.

use crate::{CompactionConfig, HistoryEntry, ImageAttachment, InputModality, Message, ModelRef};
use serde_json::{Value, json};

const MIN_RESERVE_TOKENS: usize = 8_000;
const MAX_RESERVE_TOKENS: usize = 50_000;
const MIN_SUMMARY_OUTPUT_TOKENS: usize = 2_000;
const MAX_SUMMARY_OUTPUT_TOKENS: usize = 16_000;

pub(super) fn estimate_entries(entries: &[HistoryEntry]) -> usize {
    serde_json::to_string(entries).map_or(0, |text| text.chars().count().saturating_add(3) / 4)
}

#[cfg(test)]
pub(super) fn projected_request_tokens(
    entries: &[HistoryEntry],
    message: &Message,
    attachments: &[ImageAttachment],
) -> usize {
    let mut projected = entries.to_vec();
    projected.push(HistoryEntry {
        message: message.clone(),
        attachments: attachments.to_vec(),
        tool_calls: Vec::new(),
        tool_results: Vec::new(),
        provider_metadata: Value::Null,
    });
    estimate_entries(&projected)
}

pub(super) fn projected_root_request_tokens(
    core: &crate::core::CoreState,
    model: &ModelRef,
    entries: &[HistoryEntry],
    message: &Message,
    attachments: &[ImageAttachment],
) -> usize {
    let supports_images = core.model_supports(model, InputModality::Image);
    let mut messages = vec![json!({
        "role": "system",
        "content": core
            .instructions
            .lock()
            .expect("instruction runtime mutex must not be poisoned")
            .main()
            .lock()
            .expect("instruction session mutex must not be poisoned")
            .rendered(false)
            .system_prompt,
        "tool_calls": [],
        "tool_results": [],
        "provider_metadata": Value::Null,
    })];
    let mut projected = entries.to_vec();
    projected.push(HistoryEntry {
        message: message.clone(),
        attachments: attachments.to_vec(),
        tool_calls: Vec::new(),
        tool_results: Vec::new(),
        provider_metadata: Value::Null,
    });
    messages.extend(
        projected
            .iter()
            .map(|entry| crate::core::turn::serialize_history_entry(entry, supports_images)),
    );
    let roles = core.discover_roles();
    let role_catalog = crate::core::roles::parent_catalog_description(&roles);
    let mut tools = core.dispatcher.definitions_for_client(
        supports_images,
        core.client_capabilities.question_request == Some(1),
    );
    if let Some(spawn) = tools.iter_mut().find(|tool| tool.name == "spawn_agent")
        && !role_catalog.is_empty()
    {
        spawn.description.push_str(" Effective roles: ");
        spawn.description.push_str(&role_catalog);
    }
    serde_json::to_string(&json!({ "messages": messages, "tools": tools }))
        .map_or(0, |text| text.chars().count().saturating_add(3) / 4)
}

pub(in crate::core) fn reserve_tokens(config: &CompactionConfig, context_window: u32) -> usize {
    let window = context_window as usize;
    if window == 0 {
        return 0;
    }
    let requested = config.reserve_tokens.map_or_else(
        || (context_window as usize / 5).clamp(MIN_RESERVE_TOKENS, MAX_RESERVE_TOKENS),
        |tokens| tokens as usize,
    );
    requested.min(window / 2)
}

pub(in crate::core) fn compaction_threshold(
    config: &CompactionConfig,
    context_window: u32,
) -> usize {
    let window = context_window as usize;
    let ratio = (f64::from(context_window) * config.trigger_ratio).floor() as usize;
    ratio.min(window.saturating_sub(reserve_tokens(config, context_window)))
}

pub(super) fn should_compact(
    config: &CompactionConfig,
    context_window: u32,
    projected_tokens: usize,
) -> bool {
    context_window > 0 && projected_tokens >= compaction_threshold(config, context_window)
}

pub(super) fn summary_output_tokens(context_window: u32, config: &CompactionConfig) -> u32 {
    let tokens = (reserve_tokens(config, context_window) / 2)
        .clamp(MIN_SUMMARY_OUTPUT_TOKENS, MAX_SUMMARY_OUTPUT_TOKENS);
    u32::try_from(tokens).unwrap_or(MAX_SUMMARY_OUTPUT_TOKENS as u32)
}

#[cfg(test)]
mod tests {
    use super::{
        compaction_threshold, projected_request_tokens, reserve_tokens, summary_output_tokens,
    };
    use crate::{CompactionConfig, HistoryEntry, Message};

    #[test]
    fn default_reserve_threshold_and_summary_budget_follow_the_window_contract() {
        let config = CompactionConfig::default();
        assert_eq!(reserve_tokens(&config, 20_000), 8_000);
        assert_eq!(reserve_tokens(&config, 100_000), 20_000);
        assert_eq!(reserve_tokens(&config, 400_000), 50_000);
        assert_eq!(compaction_threshold(&config, 100_000), 80_000);
        assert_eq!(summary_output_tokens(20_000, &config), 4_000);
        assert_eq!(summary_output_tokens(400_000, &config), 16_000);
    }

    #[test]
    fn explicit_reserve_is_bounded_by_half_the_concrete_window() {
        let config = CompactionConfig {
            reserve_tokens: Some(1_000_000),
            ..CompactionConfig::default()
        };
        assert_eq!(reserve_tokens(&config, 20_000), 10_000);
        assert_eq!(compaction_threshold(&config, 20_000), 10_000);
        assert_eq!(summary_output_tokens(20_000, &config), 5_000);
        assert_eq!(reserve_tokens(&config, 0), 0);
        assert!(!super::should_compact(&config, 0, usize::MAX));
    }

    #[test]
    fn threshold_uses_the_projected_pending_request() {
        let history = Vec::<HistoryEntry>::new();
        let short = projected_request_tokens(&history, &Message::user("x"), &[]);
        let long = projected_request_tokens(&history, &Message::user("x".repeat(40_000)), &[]);
        assert!(long > short + 9_000);
    }
}
