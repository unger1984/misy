//! Bounded rendering of cached instruction sources into one system prefix.

use super::*;

pub(super) fn render_project_chain(
    sources: &[Arc<CachedSource>],
    owner: InstructionOwner,
) -> RenderedProject {
    let mut remaining = PROJECT_CHAIN_LIMIT;
    let mut retained = vec![None; sources.len()];
    let mut has_deeper_section = false;
    for (index, source) in sources.iter().enumerate().rev() {
        let Some(content) = &source.content else {
            continue;
        };
        let header = source_header(&source.summary);
        let separator = usize::from(has_deeper_section).saturating_mul(2);
        let overhead = header.len().saturating_add(separator);
        if overhead >= remaining {
            continue;
        }
        let available = remaining - overhead;
        let truncated = content.len() > available;
        let (body, retained_content_bytes) = if truncated {
            truncated_body(&source.summary, content, available)
        } else {
            (content.to_string(), content.len())
        };
        remaining = remaining.saturating_sub(
            separator
                .saturating_add(header.len())
                .saturating_add(body.len()),
        );
        retained[index] = Some((header, body, retained_content_bytes, truncated));
        has_deeper_section = true;
    }
    summarize_project(sources, owner, &retained)
}

fn truncated_body(
    summary: &InstructionSourceSummary,
    content: &str,
    available: usize,
) -> (String, usize) {
    let marker = truncated_marker(summary, content.len(), available);
    if marker.len() >= available {
        return (utf8_prefix(&marker, available).to_owned(), 0);
    }
    let kept = utf8_prefix(content, available - marker.len());
    (format!("{kept}{marker}"), kept.len())
}

fn summarize_project(
    sources: &[Arc<CachedSource>],
    owner: InstructionOwner,
    retained: &[Option<(String, String, usize, bool)>],
) -> RenderedProject {
    let mut rendered = RenderedProject::default();
    for (index, source) in sources.iter().enumerate() {
        let mut summary = source.summary.clone();
        if let Some(reason) = &source.warning {
            rendered
                .warnings
                .push(warning(source, reason.clone(), owner));
        }
        if let Some((header, body, retained_content_bytes, truncated)) = &retained[index] {
            summary.retained_bytes = *retained_content_bytes;
            summary.truncated = *truncated;
            if *truncated {
                summary.status = InstructionSourceStatus::Truncated;
                rendered.warnings.push(InstructionWarning {
                    source: summary.clone(),
                    reason: InstructionWarningReason::Truncated,
                    owner,
                });
            }
            rendered.sections.push(format!("{header}{body}"));
        } else if source.content.is_some() {
            summary.retained_bytes = 0;
            summary.truncated = true;
            summary.status = InstructionSourceStatus::Truncated;
        }
        rendered.summaries.push(summary);
    }
    rendered
}

#[derive(Default)]
pub(super) struct RenderedProject {
    pub(super) sections: Vec<String>,
    pub(super) summaries: Vec<InstructionSourceSummary>,
    pub(super) warnings: Vec<InstructionWarning>,
}

pub(super) fn append_unbudgeted(
    source: &Arc<CachedSource>,
    owner: InstructionOwner,
    sections: &mut Vec<String>,
    summaries: &mut Vec<InstructionSourceSummary>,
    warnings: &mut Vec<InstructionWarning>,
) {
    if let Some(content) = &source.content {
        sections.push(format!("{}{content}", source_header(&source.summary)));
    }
    if let Some(reason) = &source.warning {
        warnings.push(warning(source, reason.clone(), owner));
    }
    summaries.push(source.summary.clone());
}

fn source_header(summary: &InstructionSourceSummary) -> String {
    let kind = match summary.kind {
        InstructionSourceKind::Global => "global",
        InstructionSourceKind::ProjectRoot => "project-root",
        InstructionSourceKind::Nested => "nested",
    };
    let scope = match &summary.scope {
        InstructionScope::Global => "global",
        InstructionScope::ProjectRoot(directory) | InstructionScope::Nested(directory) => directory,
    };
    format!(
        "[AGENTS.md kind={kind} source={:?} scope={:?}]\n",
        bounded_metadata(&summary.display_path),
        bounded_metadata(scope),
    )
}

fn bounded_metadata(value: &str) -> String {
    if value.len() <= MAX_METADATA_FIELD_BYTES {
        return value.to_owned();
    }
    format!(
        "{}…",
        utf8_prefix(value, MAX_METADATA_FIELD_BYTES - '…'.len_utf8())
    )
}

fn truncated_marker(
    summary: &InstructionSourceSummary,
    original: usize,
    retained: usize,
) -> String {
    format!(
        "\n[AGENTS.md truncated: {} had {original} bytes; retained about {retained} bytes]",
        summary.display_path
    )
}
