//! Adaptive aligned-column rendering for model catalog rows.

use super::{
    display_width::{text_width, truncate_to_width},
    list::ListRowDisplay,
    render::padded_line,
    style,
};
use ratatui::{
    style::Style,
    text::{Line, Span},
};

const PREFIX_WIDTH: usize = 4;
const COLUMN_GAP: &str = "  ";

#[derive(Clone, Copy)]
pub(super) struct ModelColumns {
    label: usize,
    context: usize,
    pricing: usize,
    provider: usize,
    description: usize,
}

impl ModelColumns {
    pub(super) fn for_rows(rows: &[ListRowDisplay], width: u16) -> Self {
        let width = usize::from(width);
        let widest = |header: &str, value: fn(&ListRowDisplay) -> Option<&str>| {
            rows.iter()
                .filter_map(value)
                .map(text_width)
                .max()
                .unwrap_or(0)
                .max(text_width(header))
        };
        let context = usize::from(width >= 24) * widest("Context", |row| row.context.as_deref());
        let pricing =
            usize::from(width >= 58) * widest("Cost $/M", |row| row.pricing.as_deref()).min(14);
        let provider =
            usize::from(width >= 40) * widest("Provider", |row| row.provider.as_deref()).min(14);
        let show_description = width >= 76;
        let visible_metadata = [context, pricing, provider]
            .into_iter()
            .filter(|column| *column > 0)
            .count();
        let fixed =
            PREFIX_WIDTH + context + pricing + provider + visible_metadata * text_width(COLUMN_GAP);
        let available = width.saturating_sub(fixed);
        let desired_label = widest("Model", |row| Some(row.label.as_str()));
        let description_reserve = usize::from(show_description) * 12;
        let label = desired_label
            .min(available.saturating_sub(description_reserve))
            .max(available.min(1));
        let description = usize::from(show_description)
            * available.saturating_sub(label + text_width(COLUMN_GAP));
        Self {
            label,
            context,
            pricing,
            provider,
            description,
        }
    }
}

pub(super) fn header_line(width: u16, columns: ModelColumns) -> Line<'static> {
    let mut spans = vec![Span::raw(" ".repeat(PREFIX_WIDTH))];
    push_column(
        &mut spans,
        "Model",
        columns.label,
        false,
        false,
        style::muted(),
    );
    push_column(
        &mut spans,
        "Context",
        columns.context,
        true,
        true,
        style::muted(),
    );
    push_column(
        &mut spans,
        "Cost $/M",
        columns.pricing,
        true,
        true,
        style::muted(),
    );
    push_column(
        &mut spans,
        "Provider",
        columns.provider,
        true,
        false,
        style::muted(),
    );
    push_column(
        &mut spans,
        "Description",
        columns.description,
        true,
        false,
        style::muted(),
    );
    padded_line(spans, width, Style::default())
}

pub(super) fn list_line(row: &ListRowDisplay, width: u16, columns: ModelColumns) -> Line<'static> {
    let marker_style = if row.selected {
        style::accent()
    } else {
        Style::default()
    };
    let current_style = if row.current {
        style::accent()
    } else {
        Style::default()
    };
    let mut spans = vec![
        Span::styled(if row.selected { "› " } else { "  " }, marker_style),
        Span::styled(if row.current { "✓ " } else { "  " }, current_style),
    ];
    if row.context.is_none() {
        spans.push(Span::raw(padded_label(
            &row.label,
            usize::from(width).saturating_sub(PREFIX_WIDTH),
        )));
        return padded_line(spans, width, Style::default());
    }
    spans.push(Span::raw(padded_label(&row.label, columns.label)));
    push_column(
        &mut spans,
        row.context.as_deref().unwrap_or_default(),
        columns.context,
        true,
        true,
        style::muted(),
    );
    push_column(
        &mut spans,
        row.pricing.as_deref().unwrap_or_default(),
        columns.pricing,
        true,
        true,
        style::muted(),
    );
    push_column(
        &mut spans,
        row.provider.as_deref().unwrap_or_default(),
        columns.provider,
        true,
        false,
        style::muted(),
    );
    push_column(
        &mut spans,
        row.description.as_deref().unwrap_or_default(),
        columns.description,
        true,
        false,
        style::muted(),
    );
    padded_line(spans, width, Style::default())
}

fn push_column(
    spans: &mut Vec<Span<'static>>,
    value: &str,
    width: usize,
    separated: bool,
    right_aligned: bool,
    style: Style,
) {
    if width == 0 {
        return;
    }
    if separated {
        spans.push(Span::raw(COLUMN_GAP));
    }
    let value = if right_aligned {
        padded_label_right(value, width)
    } else {
        padded_label(value, width)
    };
    spans.push(Span::styled(value, style));
}

fn padded_label(label: &str, width: usize) -> String {
    let label = truncate_to_width(label, width);
    let padding = width.saturating_sub(text_width(&label));
    format!("{label}{}", " ".repeat(padding))
}

fn padded_label_right(label: &str, width: usize) -> String {
    let label = truncate_to_width(label, width);
    let padding = width.saturating_sub(text_width(&label));
    format!("{}{label}", " ".repeat(padding))
}
