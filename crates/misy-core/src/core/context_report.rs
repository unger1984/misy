//! In-memory estimation of the next provider request for frontend diagnostics.

use super::{
    ContextCategory, ContextCategoryUsage, ContextReport, MisyCore,
    instructions::BUILTIN_INSTRUCTIONS,
};
use crate::{HistoryEntry, InputModality};

impl MisyCore {
    /// Returns a content-free estimate of the context used by the next request.
    ///
    /// This operation performs no filesystem, provider, credential, or session I/O.
    ///
    /// # Panics
    ///
    /// Panics if a prior core task panicked while holding an in-memory state mutex.
    pub fn context_report(&self) -> ContextReport {
        let core = &self.inner.state;
        let selected = core
            .selected_model
            .lock()
            .expect("selected model mutex must not be poisoned")
            .clone();
        let model_info = selected.as_ref().and_then(|selected| {
            core.model_cache
                .load()
                .into_iter()
                .find(|candidate| &candidate.model == selected)
        });
        let supports_images = selected
            .as_ref()
            .is_some_and(|model| core.model_supports(model, InputModality::Image));
        let history = core
            .history
            .lock()
            .expect("history mutex must not be poisoned")
            .clone();
        let instruction_session = core
            .instructions
            .lock()
            .expect("instruction runtime mutex must not be poisoned")
            .main();
        let session = instruction_session
            .lock()
            .expect("instruction session mutex must not be poisoned");
        let rendered = session.rendered(false);
        let (state, sources, warnings) = session.report_sources();
        let system_tokens = estimate_tokens(&rendered.system_prompt);
        let misy_tokens = estimate_tokens(BUILTIN_INSTRUCTIONS);
        let agent_tokens = system_tokens.saturating_sub(misy_tokens);
        let tools = core.dispatcher.definitions_for_client(
            supports_images,
            core.client_capabilities.question_request == Some(1),
        );
        let tool_tokens = encoded_tokens(&tools);
        let serialized = history
            .iter()
            .map(|entry| super::turn::serialize_history_entry(entry, supports_images))
            .collect::<Vec<_>>();
        let message_tokens = encoded_tokens(&serialized);
        let estimated_tokens = misy_tokens
            .saturating_add(agent_tokens)
            .saturating_add(tool_tokens)
            .saturating_add(message_tokens);
        let context_window = model_info.as_ref().map_or(0, |model| model.context_window);
        let free_tokens = usize::try_from(context_window)
            .unwrap_or(usize::MAX)
            .saturating_sub(estimated_tokens);
        let (image_count, image_bytes) = image_usage(&history);
        let mut categories = vec![
            usage(ContextCategory::MisyPrompt, misy_tokens),
            usage(ContextCategory::AgentsMd, agent_tokens),
            usage(ContextCategory::ToolDefinitions, tool_tokens),
            usage(ContextCategory::Messages, message_tokens),
        ];
        if context_window > 0 {
            categories.push(usage(ContextCategory::FreeSpace, free_tokens));
        }
        ContextReport {
            model: selected,
            model_display_name: model_info.as_ref().map(|model| model.display_name.clone()),
            context_window,
            estimated_tokens,
            categories,
            state,
            sources,
            image_count,
            image_bytes,
            warnings,
        }
    }
}

fn usage(category: ContextCategory, estimated_tokens: usize) -> ContextCategoryUsage {
    ContextCategoryUsage {
        category,
        estimated_tokens,
    }
}

fn encoded_tokens(value: &impl serde::Serialize) -> usize {
    serde_json::to_string(value).map_or(0, |text| estimate_tokens(&text))
}

fn estimate_tokens(text: &str) -> usize {
    let (ascii, non_ascii) = text.chars().fold((0_usize, 0_usize), |counts, character| {
        if character.is_ascii() {
            (counts.0 + 1, counts.1)
        } else {
            (counts.0, counts.1 + 1)
        }
    });
    ascii.saturating_add(3) / 4 + non_ascii
}

fn image_usage(history: &[HistoryEntry]) -> (usize, usize) {
    history.iter().fold((0_usize, 0_usize), |totals, entry| {
        let totals = entry.attachments.iter().fold(totals, |current, image| {
            (current.0 + 1, current.1.saturating_add(image.bytes().len()))
        });
        entry.tool_results.iter().fold(totals, |current, result| {
            result.attachments.iter().fold(current, |nested, image| {
                (nested.0 + 1, nested.1.saturating_add(image.bytes().len()))
            })
        })
    })
}

#[cfg(test)]
mod tests {
    use super::estimate_tokens;

    #[test]
    fn estimate_counts_ascii_quarters_and_unicode_scalars() {
        assert_eq!(estimate_tokens("abcdefgh"), 2);
        assert_eq!(estimate_tokens("abcdeж🙂"), 4);
    }
}
