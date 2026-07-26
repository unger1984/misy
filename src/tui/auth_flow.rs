//! Typed validation of provider-owned `auth.start` responses.

use super::browser::validate_authorization_url;
use serde_json::Value;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct PromptField {
    pub(super) id: String,
    pub(super) label: String,
    pub(super) secret: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum AuthFlow {
    Browser {
        url: String,
        session: Value,
    },
    Device {
        url: String,
        user_code: String,
        expires_at: Option<u64>,
        session: Value,
    },
    Prompt {
        fields: Vec<PromptField>,
        session: Value,
    },
    None,
}

pub(super) fn parse_auth_flow(response: &Value) -> Result<AuthFlow, String> {
    let Some(kind) = response.get("kind").and_then(Value::as_str) else {
        return Err(format!(
            "provider auth.start returned invalid kind {}",
            response
                .get("kind")
                .map_or_else(|| "<missing>".to_owned(), Value::to_string)
        ));
    };
    match kind {
        "browser" => parse_browser(response),
        "device" => parse_device(response),
        "prompt" => parse_prompt(response),
        "none" => Ok(AuthFlow::None),
        unknown => Err(format!(
            "provider auth.start returned unknown kind `{unknown}`"
        )),
    }
}

fn parse_browser(response: &Value) -> Result<AuthFlow, String> {
    let url = required_string(response, "url", "browser")?;
    validate_authorization_url(url)
        .map_err(|error| format!("provider auth.start browser URL is invalid: {error}"))?;
    Ok(AuthFlow::Browser {
        url: url.to_owned(),
        session: required_session(response, "browser")?,
    })
}

fn parse_device(response: &Value) -> Result<AuthFlow, String> {
    let url = required_string(response, "url", "device")?;
    validate_authorization_url(url)
        .map_err(|error| format!("provider auth.start device URL is invalid: {error}"))?;
    let expires_at = response
        .get("expires_at")
        .map(|value| {
            value.as_u64().ok_or_else(|| {
                "provider auth.start device expires_at must be epoch milliseconds".to_owned()
            })
        })
        .transpose()?;
    Ok(AuthFlow::Device {
        url: url.to_owned(),
        user_code: required_string(response, "user_code", "device")?.to_owned(),
        expires_at,
        session: required_session(response, "device")?,
    })
}

fn parse_prompt(response: &Value) -> Result<AuthFlow, String> {
    let fields = response
        .get("fields")
        .and_then(Value::as_array)
        .filter(|fields| !fields.is_empty())
        .ok_or_else(|| {
            "provider auth.start prompt flow requires a non-empty fields array".to_owned()
        })?
        .iter()
        .map(parse_prompt_field)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(AuthFlow::Prompt {
        fields,
        session: required_session(response, "prompt")?,
    })
}

fn parse_prompt_field(field: &Value) -> Result<PromptField, String> {
    Ok(PromptField {
        id: required_string(field, "id", "prompt field")?.to_owned(),
        label: required_string(field, "label", "prompt field")?.to_owned(),
        secret: field
            .get("secret")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

fn required_string<'a>(response: &'a Value, field: &str, flow: &str) -> Result<&'a str, String> {
    response
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("provider auth.start {flow} flow requires string {field}"))
}

fn required_session(response: &Value, flow: &str) -> Result<Value, String> {
    response
        .get("session")
        .cloned()
        .ok_or_else(|| format!("provider auth.start {flow} flow requires a session"))
}

#[cfg(test)]
mod tests {
    use super::{AuthFlow, parse_auth_flow};
    use serde_json::json;

    #[test]
    fn parses_all_supported_flows() {
        assert!(matches!(
            parse_auth_flow(&json!({
                "kind": "browser",
                "url": "https://example.test/auth",
                "session": {"id": 1}
            })),
            Ok(AuthFlow::Browser { .. })
        ));
        assert!(matches!(
            parse_auth_flow(&json!({
                "kind": "device",
                "url": "https://example.test/device",
                "user_code": "ABCD-EFGH",
                "session": {"id": 2}
            })),
            Ok(AuthFlow::Device { .. })
        ));
        assert!(matches!(
            parse_auth_flow(&json!({
                "kind": "prompt",
                "fields": [{"id": "key", "label": "Key", "secret": true}],
                "session": {"id": 3}
            })),
            Ok(AuthFlow::Prompt { .. })
        ));
        assert_eq!(
            parse_auth_flow(&json!({"kind": "none"})),
            Ok(AuthFlow::None)
        );
    }

    #[test]
    fn rejects_invalid_discriminants_and_required_fields() {
        for response in [
            json!({}),
            json!({"kind": "future"}),
            json!({"kind": "browser", "session": {}}),
            json!({"kind": "browser", "url": "javascript:alert(1)", "session": {}}),
            json!({"kind": "device", "url": "https://example.test", "session": {}}),
            json!({"kind": "prompt", "fields": [], "session": {}}),
        ] {
            assert!(parse_auth_flow(&response).is_err(), "accepted {response}");
        }
    }
}
