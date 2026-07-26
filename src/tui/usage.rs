//! Display formatting for provider-normalized account-limit reports.

use crate::{UsageAmount, UsageReport, UsageStatus, UsageUnit};
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) fn format_usage_report(provider: &str, report: &UsageReport) -> Vec<String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis());
    format_usage_report_at(provider, report, now)
}

fn format_usage_report_at(provider: &str, report: &UsageReport, now: u128) -> Vec<String> {
    let mut lines = vec![format!("Usage · {provider}")];
    lines.extend(report.notes.iter().map(|note| format!("  {note}")));
    if report.limits.is_empty() {
        lines.push("  no limits reported".to_owned());
        return lines;
    }
    for limit in &report.limits {
        let mut detail = format_amount(&limit.amount);
        if limit.status == Some(UsageStatus::Exhausted) {
            detail.push_str(" · exhausted");
        }
        if let Some(resets_at) = limit.window.as_ref().and_then(|window| window.resets_at)
            && u128::from(resets_at) > now
        {
            detail.push_str(&format!(
                " · resets in {}",
                format_duration(u128::from(resets_at) - now)
            ));
        }
        lines.push(format!("  {}: {detail}", limit.label));
        lines.extend(limit.notes.iter().map(|note| format!("    {note}")));
    }
    lines
}

fn format_amount(amount: &UsageAmount) -> String {
    let used = amount.used.map(format_number);
    let limit = amount.limit.map(format_number);
    let remaining = amount.remaining.map(format_number);
    let unit = unit_label(amount.unit);
    let mut text = match (amount.unit, used, limit) {
        (UsageUnit::Percent, Some(used), _) => format!("{used}% used"),
        (_, Some(used), Some(limit)) => format!("{used}/{limit}{unit} used"),
        (_, Some(used), None) => format!("{used}{unit} used"),
        (_, None, _) => "usage unavailable".to_owned(),
    };
    if let Some(remaining) = remaining {
        text.push_str(&format!(" · {remaining}{unit} left"));
    }
    text
}

fn unit_label(unit: UsageUnit) -> &'static str {
    match unit {
        UsageUnit::Percent => "%",
        UsageUnit::Tokens => " tokens",
        UsageUnit::Requests => " requests",
        UsageUnit::Usd => " USD",
        UsageUnit::Minutes => " minutes",
        UsageUnit::Bytes => " bytes",
        UsageUnit::Unknown => "",
    }
}

fn format_number(value: f64) -> String {
    let rendered = format!("{value:.2}");
    rendered
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_owned()
}

fn format_duration(milliseconds: u128) -> String {
    let minutes = milliseconds / 60_000;
    let days = minutes / (24 * 60);
    let hours = minutes / 60 % 24;
    let minutes = minutes % 60;
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{minutes}m")
    }
}
