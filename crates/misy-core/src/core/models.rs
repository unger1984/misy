//! Provider model-catalog validation for the core.

use super::CoreError;
use crate::{InputModality, ModelId, ModelInfo, ModelRef, ProviderId, ThinkingInfo, ThinkingLevel};
use serde_json::Value;

pub(super) fn parse_models(
    provider: &ProviderId,
    response: &Value,
) -> Result<Vec<ModelInfo>, CoreError> {
    let values = response
        .get("models")
        .and_then(Value::as_array)
        .ok_or_else(|| CoreError::InvalidModels("missing models array".to_owned()))?;
    values
        .iter()
        .map(|value| parse_model(provider, value))
        .collect()
}

fn parse_model(provider: &ProviderId, value: &Value) -> Result<ModelInfo, CoreError> {
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| CoreError::InvalidModels("model is missing string id".to_owned()))?;
    let display_name = value
        .get("display_name")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            CoreError::InvalidModels("model is missing string display_name".to_owned())
        })?;
    let context_window = value
        .get("context_window")
        .and_then(Value::as_u64)
        .and_then(|window| u32::try_from(window).ok())
        .ok_or_else(|| {
            CoreError::InvalidModels("model is missing u32 context_window".to_owned())
        })?;
    Ok(ModelInfo::new(
        ModelRef::new(provider.clone(), ModelId::new(id)),
        display_name,
        context_window,
    )
    .with_input_modalities(input_modalities(value))
    .with_optional_metadata(
        bounded_text(value.get("description")),
        bounded_text(value.get("pricing")),
        thinking(value)?,
    ))
}

fn bounded_text(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .and_then(|text| (text.len() <= 4_096 && !text.trim().is_empty()).then(|| text.to_owned()))
}

fn thinking(value: &Value) -> Result<Option<ThinkingInfo>, CoreError> {
    let Some(value) = value.get("thinking") else {
        return Ok(None);
    };
    let default = value
        .get("default")
        .and_then(Value::as_str)
        .ok_or_else(|| CoreError::InvalidModels("thinking is missing string default".to_owned()))?;
    let levels = value
        .get("levels")
        .and_then(Value::as_array)
        .ok_or_else(|| CoreError::InvalidModels("thinking is missing levels array".to_owned()))?;
    if levels.is_empty() || levels.len() > 64 {
        return Err(CoreError::InvalidModels(
            "thinking levels must contain 1..=64 entries".to_owned(),
        ));
    }
    let mut normalized = Vec::with_capacity(levels.len());
    for level in levels {
        let id = level
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| valid_thinking_id(id))
            .ok_or_else(|| CoreError::InvalidModels("thinking level has invalid id".to_owned()))?;
        let description = bounded_text(level.get("description")).ok_or_else(|| {
            CoreError::InvalidModels("thinking level has invalid description".to_owned())
        })?;
        if normalized
            .iter()
            .any(|level: &ThinkingLevel| level.id == id)
        {
            return Err(CoreError::InvalidModels(
                "thinking levels must have unique ids".to_owned(),
            ));
        }
        normalized.push(ThinkingLevel {
            id: id.to_owned(),
            description,
        });
    }
    if !normalized.iter().any(|level| level.id == default) {
        return Err(CoreError::InvalidModels(
            "thinking default is not a declared level".to_owned(),
        ));
    }
    Ok(Some(ThinkingInfo {
        default: default.to_owned(),
        levels: normalized,
    }))
}

fn valid_thinking_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    (1..=64).contains(&bytes.len())
        && matches!(bytes.first(), Some(b'a'..=b'z' | b'0'..=b'9'))
        && bytes
            .iter()
            .all(|byte| matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-'))
}

fn input_modalities(value: &Value) -> Vec<InputModality> {
    let Some(values) = value.get("input_modalities").and_then(Value::as_array) else {
        return vec![InputModality::Text];
    };
    let mut modalities = Vec::new();
    for value in values {
        match value.as_str() {
            Some("text") if !modalities.contains(&InputModality::Text) => {
                modalities.push(InputModality::Text);
            }
            Some("image") if !modalities.contains(&InputModality::Image) => {
                modalities.push(InputModality::Image);
            }
            Some("text" | "image") => {}
            _ => return vec![InputModality::Text],
        }
    }
    if modalities.contains(&InputModality::Text) {
        modalities
    } else {
        vec![InputModality::Text]
    }
}

pub(super) fn select_catalog_default(
    provider: &ProviderId,
    response: &Value,
    models: &[ModelInfo],
) -> Result<ModelRef, CoreError> {
    let Some(default_id) = response.get("default_model") else {
        return models
            .first()
            .map(|model| model.model.clone())
            .ok_or_else(|| CoreError::InvalidModels("provider returned no models".to_owned()));
    };
    let default_id = default_id.as_str().ok_or_else(|| {
        CoreError::InvalidModels("default_model must be a string when present".to_owned())
    })?;
    models
        .iter()
        .find(|model| model.model.model.as_str() == default_id)
        .map(|model| model.model.clone())
        .ok_or_else(|| {
            CoreError::InvalidModels(format!(
                "provider `{}` default_model `{default_id}` is not in its model catalog",
                provider.as_str()
            ))
        })
}
