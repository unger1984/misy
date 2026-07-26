//! Queue cancellation and stream-boundary regressions.

use super::*;

#[test]
fn cancelling_a_submission_queued_on_the_session_gate_does_not_append_its_prompt() {
    let (_temporary, core, _) = test_core("queued-cancel");
    core.select_model(fixture_model()).expect("select model");
    let events = core.subscribe();
    let blocking = core
        .submit(Message::user("block-session"))
        .expect("blocking submit");
    receive_until(&events, blocking, |event| {
        matches!(
            event,
            CoreEvent::SubmissionStarted { submission, .. } if *submission == blocking
        )
    });
    let queued = core
        .submit(Message::user("queued-cancel"))
        .expect("queued submit");
    core.cancel(queued).expect("cancel queued submission");

    receive_until(
        &events,
        blocking,
        |event| matches!(event, CoreEvent::Completed { submission } if *submission == blocking),
    );
    receive_until(
        &events,
        queued,
        |event| matches!(event, CoreEvent::Cancelled { submission } if *submission == queued),
    );
    assert!(
        !core
            .history()
            .iter()
            .any(|entry| entry.message.content == "queued-cancel")
    );
    core.shutdown().expect("shutdown");
}

#[test]
fn late_stream_events_from_a_cancelled_request_do_not_reach_the_next_request() {
    let (_temporary, core, _) = test_core("late-events");
    core.select_model(fixture_model()).expect("select model");
    let events = core.subscribe();
    let cancelled = core
        .submit(Message::user("late-cancel"))
        .expect("cancelled submission");
    receive_until(&events, cancelled, |event| {
        matches!(
            event,
            CoreEvent::SubmissionStarted { submission, .. } if *submission == cancelled
        )
    });
    core.cancel(cancelled).expect("cancel");
    receive_until(
        &events,
        cancelled,
        |event| matches!(event, CoreEvent::Cancelled { submission } if *submission == cancelled),
    );

    let next = core
        .submit(Message::user("late-next"))
        .expect("next submission");
    let received = receive_until(
        &events,
        next,
        |event| matches!(event, CoreEvent::Completed { submission } if *submission == next),
    );
    assert!(received.iter().any(|event| {
        matches!(
            event,
            CoreEvent::TextDelta { delta, submission, .. }
                if *submission == next && delta == "next"
        )
    }));
    assert!(!received.iter().any(|event| {
        matches!(
            event,
            CoreEvent::TextDelta { delta, submission, .. }
                if *submission == next && delta == "late"
        )
    }));
    core.shutdown().expect("shutdown");
}

#[test]
fn stream_terminal_event_without_request_id_fails_the_active_submission_promptly() {
    let (_temporary, core, _) = test_core("missing-terminal-id");
    core.select_model(fixture_model()).expect("select model");
    let events = core.subscribe();
    let submission = core
        .submit(Message::user("no-id-terminal"))
        .expect("submission");
    let received = receive_until(&events, submission, |event| {
        matches!(
            event,
            CoreEvent::Failed { submission: id, message }
                if *id == submission && message.contains("request_id")
        )
    });
    assert!(received.iter().any(
        |event| matches!(event, CoreEvent::Failed { message, .. } if message.contains("request_id"))
    ));
    core.shutdown().expect("shutdown");
}
