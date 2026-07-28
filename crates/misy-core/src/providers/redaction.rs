//! Redaction of provider-supplied error text at the wire boundary.
//!
//! A provider plugin (or the SDK inside it) may embed a serialized request in an error message,
//! which would leak `Authorization` headers into the user's transcript, or return a portal HTML
//! page that floods it. Every remote error message entering the core — JSON-RPC error responses
//! and `failed` stream notifications — passes through [`sanitize_remote_message`] so the core
//! never retains secret-shaped fragments or unbounded text.

/// Upper bound for one provider-supplied error message; larger payloads are truncated with an
/// explicit marker. One KiB keeps portal error pages and request dumps out of the transcript
/// while leaving ordinary diagnostics intact.
const MAX_REMOTE_MESSAGE_BYTES: usize = 1024;

/// Placeholder substituted for secret-shaped fragments.
const REDACTED: &str = "[redacted]";

/// Minimum length of a `Bearer` token run that is redacted. Shorter runs are usually prose
/// ("Bearer tokens are …"), and false positives destroy legitimate diagnostics.
const MIN_BEARER_TOKEN_CHARS: usize = 16;

/// Authentication headers whose values are redacted through the end of the line. The list is
/// deliberately short: only unambiguous secret carriers, matched in their HTTP header form.
const SECRET_HEADERS: &[&str] = &["authorization", "x-api-key"];

/// Redacts secret-shaped fragments from a provider-supplied error `message` and caps its length
/// at [`MAX_REMOTE_MESSAGE_BYTES`]. Text without secret-shaped fragments passes through
/// unchanged apart from the size cap.
pub(crate) fn sanitize_remote_message(message: &str) -> String {
    let mut sanitized = redact_bearer_tokens(message);
    for header in SECRET_HEADERS {
        sanitized = redact_header_value(&sanitized, header);
    }
    truncate_message(&sanitized)
}

fn redact_bearer_tokens(message: &str) -> String {
    const KEYWORD: &str = "bearer";
    let mut sanitized = String::with_capacity(message.len());
    let mut cursor = 0;
    while let Some(start) = find_ascii_keyword(message, KEYWORD, cursor) {
        match bearer_token_span(message, start + KEYWORD.len()) {
            Some((token_start, token_end)) => {
                sanitized.push_str(&message[cursor..token_start]);
                sanitized.push_str(REDACTED);
                cursor = token_end;
            }
            None => {
                sanitized.push_str(&message[cursor..start + KEYWORD.len()]);
                cursor = start + KEYWORD.len();
            }
        }
    }
    sanitized.push_str(&message[cursor..]);
    sanitized
}

fn redact_header_value(message: &str, header: &str) -> String {
    let mut sanitized = String::with_capacity(message.len());
    let mut cursor = 0;
    while let Some(start) = find_ascii_keyword(message, header, cursor) {
        match header_value_start(message, start + header.len()) {
            Some(value_start) => {
                sanitized.push_str(&message[cursor..value_start]);
                sanitized.push_str(REDACTED);
                // HTTP header values end at the line break; a request dump keeps one header
                // per line, so nothing past the line end belongs to this value.
                cursor = message[value_start..]
                    .find(['\r', '\n'])
                    .map_or(message.len(), |offset| value_start + offset);
            }
            None => {
                sanitized.push_str(&message[cursor..start + header.len()]);
                cursor = start + header.len();
            }
        }
    }
    sanitized.push_str(&message[cursor..]);
    sanitized
}

/// Finds the next ASCII-case-insensitive occurrence of `keyword` at or after byte `from` that
/// does not continue a longer word (for example `x-authorization` must not match
/// `authorization`), returning its byte index.
fn find_ascii_keyword(message: &str, keyword: &str, from: usize) -> Option<usize> {
    let haystack = message.as_bytes();
    let needle = keyword.as_bytes();
    let mut start = from;
    while start + needle.len() <= haystack.len() {
        let found = haystack[start..]
            .windows(needle.len())
            .position(|window| window.eq_ignore_ascii_case(needle))
            .map(|offset| start + offset)?;
        // Keyword bytes are pure ASCII, so a match can never start mid-UTF-8-sequence.
        if found == 0 || !is_word_byte(haystack[found - 1]) {
            return Some(found);
        }
        start = found + 1;
    }
    None
}

/// Returns the byte span of the token following a `bearer` keyword match: ASCII whitespace,
/// then a run of token characters at least [`MIN_BEARER_TOKEN_CHARS`] long.
fn bearer_token_span(message: &str, after_keyword: usize) -> Option<(usize, usize)> {
    let bytes = message.as_bytes();
    let mut token_start = after_keyword;
    if token_start >= bytes.len() || !bytes[token_start].is_ascii_whitespace() {
        return None;
    }
    while token_start < bytes.len() && bytes[token_start].is_ascii_whitespace() {
        token_start += 1;
    }
    let mut token_end = token_start;
    while token_end < bytes.len() && is_token_byte(bytes[token_end]) {
        token_end += 1;
    }
    (token_end - token_start >= MIN_BEARER_TOKEN_CHARS).then_some((token_start, token_end))
}

/// Returns the byte index where a header value starts after a header-name match: optional
/// horizontal whitespace, a colon, optional horizontal whitespace, then at least one value
/// character. A match in any other shape is prose, not a header, and stays untouched.
fn header_value_start(message: &str, after_name: usize) -> Option<usize> {
    let bytes = message.as_bytes();
    let mut cursor = after_name;
    while cursor < bytes.len() && matches!(bytes[cursor], b' ' | b'\t') {
        cursor += 1;
    }
    if bytes.get(cursor) != Some(&b':') {
        return None;
    }
    cursor += 1;
    while cursor < bytes.len() && matches!(bytes[cursor], b' ' | b'\t') {
        cursor += 1;
    }
    match bytes.get(cursor) {
        Some(byte) if !matches!(byte, b'\r' | b'\n') => Some(cursor),
        _ => None,
    }
}

fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')
}

fn is_token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-')
}

/// Caps `message` at [`MAX_REMOTE_MESSAGE_BYTES`]; the cut lands on a UTF-8 char boundary and
/// carries an explicit marker so a truncated error is never mistaken for a complete one.
fn truncate_message(message: &str) -> String {
    if message.len() <= MAX_REMOTE_MESSAGE_BYTES {
        return message.to_owned();
    }
    let mut end = MAX_REMOTE_MESSAGE_BYTES;
    while !message.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}\n[... truncated: showing the first {end} of {} bytes ...]",
        &message[..end],
        message.len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_bearer_tokens() {
        let sanitized =
            sanitize_remote_message("401 from gateway: Bearer sk-live-token-0123456789abcdef");
        assert!(sanitized.contains("[redacted]"));
        assert!(!sanitized.contains("sk-live-token-0123456789abcdef"));
    }

    #[test]
    fn keeps_bearer_prose_intact() {
        let message = "Bearer tokens are required for this endpoint";
        assert_eq!(sanitize_remote_message(message), message);
    }

    #[test]
    fn redacts_authorization_header_case_insensitively_to_end_of_line() {
        let sanitized = sanitize_remote_message(
            "POST /v1/chat failed\nAUTHORIZATION: Bearer abc123\ncontent-type: application/json",
        );
        assert!(sanitized.contains("AUTHORIZATION: [redacted]"));
        assert!(sanitized.contains("content-type: application/json"));
        assert!(!sanitized.contains("abc123"));
    }

    #[test]
    fn redacts_x_api_key_header() {
        let sanitized = sanitize_remote_message("dump: x-api-key: abcdef123456\nnext line");
        assert!(sanitized.contains("x-api-key: [redacted]"));
        assert!(sanitized.contains("next line"));
        assert!(!sanitized.contains("abcdef123456"));
    }

    #[test]
    fn ignores_words_that_merely_contain_a_header_name() {
        let message = "deauthorization: not a header";
        assert_eq!(sanitize_remote_message(message), message);
    }

    #[test]
    fn passes_plain_errors_through_unchanged() {
        let message = "rate limit exceeded, retry after 30 seconds";
        assert_eq!(sanitize_remote_message(message), message);
    }

    #[test]
    fn truncates_oversized_messages_with_a_marker() {
        let sanitized = sanitize_remote_message(&"h".repeat(50 * 1024));
        assert!(sanitized.len() <= MAX_REMOTE_MESSAGE_BYTES + 80);
        assert!(sanitized.contains("truncated: showing the first"));
    }

    #[test]
    fn truncation_respects_utf8_boundaries() {
        let mut message = "x".repeat(MAX_REMOTE_MESSAGE_BYTES - 1);
        message.push('€');
        message.push_str(&"y".repeat(16));
        let sanitized = sanitize_remote_message(&message);
        assert!(sanitized.len() < message.len() + 80);
        assert!(sanitized.contains("truncated"));
    }
}
