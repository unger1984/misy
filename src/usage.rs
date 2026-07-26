//! Strict normalized values returned by the optional provider usage capability.

use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeSet;

const MAX_REPORT_BYTES: usize = 64 * 1024;
const MAX_LIMITS: usize = 64;
const MAX_NOTES: usize = 16;
const MAX_TEXT_BYTES: usize = 512;

/// A provider-normalized snapshot of account limits.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct UsageReport {
    /// Unix epoch milliseconds when the provider fetched the snapshot.
    pub fetched_at: u64,
    /// Independent quota buckets or time windows.
    pub limits: Vec<UsageLimit>,
    /// Display-safe provider-wide caveats.
    #[serde(default)]
    pub notes: Vec<String>,
}

impl UsageReport {
    pub(crate) fn parse_provider_value(value: Value) -> Result<Self, String> {
        let size = serde_json::to_vec(&value)
            .map_err(|error| error.to_string())?
            .len();
        if size > MAX_REPORT_BYTES {
            return Err("report exceeds 64 KiB".to_owned());
        }
        let report: Self = serde_json::from_value(value).map_err(|error| error.to_string())?;
        report.validate()?;
        Ok(report)
    }

    fn validate(&self) -> Result<(), String> {
        if self.limits.len() > MAX_LIMITS {
            return Err("report contains more than 64 limits".to_owned());
        }
        validate_notes(&self.notes)?;
        let mut identifiers = BTreeSet::new();
        for limit in &self.limits {
            limit.validate()?;
            if !identifiers.insert(limit.id.as_str()) {
                return Err(format!("duplicate limit ID `{}`", limit.id));
            }
        }
        Ok(())
    }
}

/// One normalized quota bucket.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct UsageLimit {
    /// Stable provider-local identifier unique within this report.
    pub id: String,
    /// Human-readable quota label.
    pub label: String,
    /// Quantities reported by the provider.
    pub amount: UsageAmount,
    /// Optional rolling or fixed reset window.
    pub window: Option<UsageWindow>,
    /// Optional provider-derived health classification.
    pub status: Option<UsageStatus>,
    /// Display-safe caveats specific to this limit.
    #[serde(default)]
    pub notes: Vec<String>,
}

impl UsageLimit {
    fn validate(&self) -> Result<(), String> {
        validate_text("limit ID", &self.id)?;
        validate_text("limit label", &self.label)?;
        validate_notes(&self.notes)?;
        self.amount.validate()?;
        if let Some(window) = &self.window
            && window.duration_ms == Some(0)
        {
            return Err(format!("limit `{}` has a zero duration", self.id));
        }
        Ok(())
    }
}

/// Generic quantitative usage that remains independent of provider wire formats.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct UsageAmount {
    /// Provider-reported amount already consumed.
    pub used: Option<f64>,
    /// Provider-reported maximum for this quota.
    pub limit: Option<f64>,
    /// Provider-reported amount still available.
    pub remaining: Option<f64>,
    /// Unit shared by every populated amount.
    pub unit: UsageUnit,
}

impl UsageAmount {
    fn validate(&self) -> Result<(), String> {
        if self.used.is_none() && self.remaining.is_none() {
            return Err("usage amount requires `used` or `remaining`".to_owned());
        }
        for (name, value) in [
            ("used", self.used),
            ("limit", self.limit),
            ("remaining", self.remaining),
        ] {
            if value.is_some_and(|number| !number.is_finite() || number < 0.0) {
                return Err(format!(
                    "usage amount `{name}` must be finite and non-negative"
                ));
            }
        }
        if self.limit == Some(0.0) {
            return Err("usage amount `limit` must be positive".to_owned());
        }
        Ok(())
    }
}

/// Units supported by usage capability version 1.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum UsageUnit {
    /// Percentage points from zero to one hundred, with overage allowed.
    Percent,
    /// Model tokens.
    Tokens,
    /// Provider requests.
    Requests,
    /// United States dollars.
    Usd,
    /// Time measured in minutes.
    Minutes,
    /// Storage or transfer bytes.
    Bytes,
    /// Provider quantity without a more specific shared unit.
    Unknown,
}

/// Optional reset timing for one quota bucket.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct UsageWindow {
    /// Window duration in milliseconds, when known.
    pub duration_ms: Option<u64>,
    /// Absolute reset time as Unix epoch milliseconds, when known.
    pub resets_at: Option<u64>,
}

/// Provider-derived quota health.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum UsageStatus {
    /// Usage is below the provider's warning threshold.
    Ok,
    /// Usage is near exhaustion.
    Warning,
    /// The quota is exhausted.
    Exhausted,
    /// The provider did not expose a meaningful classification.
    Unknown,
}

fn validate_notes(notes: &[String]) -> Result<(), String> {
    if notes.len() > MAX_NOTES {
        return Err("usage notes contain more than 16 entries".to_owned());
    }
    for note in notes {
        validate_text("usage note", note)?;
    }
    Ok(())
}

fn validate_text(name: &str, value: &str) -> Result<(), String> {
    if value.trim().is_empty()
        || value.len() > MAX_TEXT_BYTES
        || value.chars().any(char::is_control)
    {
        return Err(format!("{name} must be non-empty display-safe text"));
    }
    Ok(())
}
