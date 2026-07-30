use super::{TurnFailureKind, classify_stream_failure};

#[test]
fn fallback_classifier_separates_context_transient_and_terminal_failures() {
    assert_eq!(
        classify_stream_failure("maximum context length exceeded").kind,
        TurnFailureKind::ContextLimit
    );
    assert_eq!(
        classify_stream_failure("rate limit exceeded").kind,
        TurnFailureKind::RetryableProfile
    );
    assert_eq!(
        classify_stream_failure("request refused by policy").kind,
        TurnFailureKind::Terminal
    );
}

#[test]
fn ordinary_output_closes_the_profile_fallback_boundary() {
    let failure = classify_stream_failure("rate limit exceeded").after_output(true);
    assert!(!failure.allows_fallback());
}
