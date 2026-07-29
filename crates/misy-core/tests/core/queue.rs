//! Queue cancellation and stream-boundary regressions.

use super::*;

async fn receive_terminal_and_assert_finished_submission_is_absent(
    core: &MisyCore,
    events: &mut UnboundedReceiver<CoreEvent>,
    submission: SubmissionId,
) -> CoreEvent {
    loop {
        let event = receive_event(events).await;
        let terminal = matches!(
            &event,
            CoreEvent::Completed { submission: id }
                | CoreEvent::Cancelled { submission: id }
                | CoreEvent::Failed { submission: id, .. }
                if *id == submission
        );
        if terminal {
            let snapshot = core.snapshot();
            assert_ne!(snapshot.active_submission, Some(submission));
            assert!(!snapshot.queued_submissions.contains(&submission));
            return event;
        }
    }
}

#[tokio::test]
async fn submission_accepted_arrives_in_fifo_order_with_the_message_before_any_turn_event() {
    let (_temporary, core, _) = test_core("accepted-order");
    core.select_model(fixture_model())
        .await
        .expect("select model");
    let mut events = core.subscribe_lossless();
    let blocking = core
        .submit(Message::user("block-session"))
        .await
        .expect("blocking submit");
    let mut received = receive_until(&mut events, blocking, |event| {
        matches!(
            event,
            CoreEvent::SubmissionStarted { submission, .. } if *submission == blocking
        )
    })
    .await;
    let first = core
        .submit(Message::user("session-one"))
        .await
        .expect("first queued submit");
    let second = core
        .submit(Message::user("session-two"))
        .await
        .expect("second queued submit");

    received.extend(
        receive_until(&mut events, blocking, |event| {
            matches!(
                event,
                CoreEvent::SubmissionAccepted { submission, .. } if *submission == second
            )
        })
        .await,
    );
    let accepted = received
        .iter()
        .filter_map(|event| match event {
            CoreEvent::SubmissionAccepted {
                submission,
                message,
                ..
            } => Some((*submission, message)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        accepted,
        vec![
            (blocking, &Message::user("block-session")),
            (first, &Message::user("session-one")),
            (second, &Message::user("session-two")),
        ]
    );
    // Both queued submissions are still behind the blocking one, so no turn event for them may
    // have preceded its acceptance on the ordered event stream.
    assert!(
        !received.iter().any(|event| matches!(
            event,
            CoreEvent::SubmissionStarted { submission, .. }
                if *submission == first || *submission == second
        )),
        "acceptance must arrive before the submission starts",
    );
    assert_eq!(core.snapshot().queued_submissions, vec![first, second]);
    core.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn completed_event_exposes_an_empty_queue_snapshot() {
    let (_temporary, core, _) = test_core("terminal-snapshot-empty");
    core.select_model(fixture_model())
        .await
        .expect("select model");
    let mut events = core.subscribe_lossless();
    let submission = core
        .submit(Message::user("session-one"))
        .await
        .expect("submission");

    assert!(matches!(
        receive_terminal_and_assert_finished_submission_is_absent(
            &core,
            &mut events,
            submission,
        )
        .await,
        CoreEvent::Completed { submission: id } if id == submission
    ));
    let snapshot = core.snapshot();
    assert!(snapshot.active_submission.is_none());
    assert!(snapshot.queued_submissions.is_empty());
    core.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn cancelled_event_excludes_the_finished_submission_with_a_queued_successor() {
    let (_temporary, core, _) = test_core("terminal-snapshot-successor");
    core.select_model(fixture_model())
        .await
        .expect("select model");
    let mut events = core.subscribe_lossless();
    let active = core
        .submit(Message::user("block-session"))
        .await
        .expect("active submission");
    receive_until(&mut events, active, |event| {
        matches!(
            event,
            CoreEvent::SubmissionStarted { submission, .. } if *submission == active
        )
    })
    .await;
    let successor = core
        .submit(Message::user("session-one"))
        .await
        .expect("queued successor");

    assert!(core.cancel_current_submission().await);
    assert!(matches!(
        receive_terminal_and_assert_finished_submission_is_absent(&core, &mut events, active).await,
        CoreEvent::Cancelled { submission } if submission == active
    ));
    assert!(matches!(
        receive_terminal_and_assert_finished_submission_is_absent(&core, &mut events, successor)
            .await,
        CoreEvent::Completed { submission } if submission == successor
    ));
    core.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn cancelling_the_active_submission_preserves_queued_work_and_emits_one_terminal_event() {
    let (_temporary, core, _) = test_core("cancel-active-only");
    core.select_model(fixture_model())
        .await
        .expect("select model");
    let mut events = core.subscribe_lossless();
    let active = core
        .submit(Message::user("block-session"))
        .await
        .expect("active submission");
    receive_until(&mut events, active, |event| {
        matches!(
            event,
            CoreEvent::SubmissionStarted { submission, .. } if *submission == active
        )
    })
    .await;
    let first_queued = core
        .submit(Message::user("session-one"))
        .await
        .expect("first queued submission");
    let second_queued = core
        .submit(Message::user("session-two"))
        .await
        .expect("second queued submission");

    assert!(core.cancel_current_submission().await);
    assert!(core.cancel_current_submission().await);
    let snapshot = core.snapshot();
    assert_eq!(snapshot.active_submission, None);
    assert_eq!(
        snapshot.queued_submissions,
        vec![first_queued, second_queued]
    );

    let mut terminal_events = Vec::new();
    while terminal_events.len() < 3 {
        let event = receive_event(&mut events).await;
        if matches!(
            event,
            CoreEvent::Cancelled { .. } | CoreEvent::Completed { .. } | CoreEvent::Failed { .. }
        ) {
            terminal_events.push(event);
        }
    }
    assert_eq!(
        terminal_events,
        vec![
            CoreEvent::Cancelled { submission: active },
            CoreEvent::Completed {
                submission: first_queued
            },
            CoreEvent::Completed {
                submission: second_queued
            },
        ]
    );
    assert_eq!(
        core.history()
            .await
            .iter()
            .filter(|entry| entry.message.role == misy_core::MessageRole::User)
            .map(|entry| entry.message.content.as_str())
            .collect::<Vec<_>>(),
        vec!["block-session", "session-one", "session-two"]
    );
    assert!(
        !core.cancel_current_submission().await,
        "a completed queue has no active submission"
    );
    core.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn cancelling_a_submission_queued_on_the_session_gate_does_not_append_its_prompt() {
    let (_temporary, core, _) = test_core("queued-cancel");
    core.select_model(fixture_model())
        .await
        .expect("select model");
    let mut events = core.subscribe_lossless();
    let blocking = core
        .submit(Message::user("block-session"))
        .await
        .expect("blocking submit");
    receive_until(&mut events, blocking, |event| {
        matches!(
            event,
            CoreEvent::SubmissionStarted { submission, .. } if *submission == blocking
        )
    })
    .await;
    let queued = core
        .submit(Message::user("queued-cancel"))
        .await
        .expect("queued submit");
    core.cancel(queued).await.expect("cancel queued submission");

    receive_until(
        &mut events,
        blocking,
        |event| matches!(event, CoreEvent::Completed { submission } if *submission == blocking),
    )
    .await;
    receive_until(
        &mut events,
        queued,
        |event| matches!(event, CoreEvent::Cancelled { submission } if *submission == queued),
    )
    .await;
    assert!(
        !core
            .history()
            .await
            .iter()
            .any(|entry| entry.message.content == "queued-cancel")
    );
    core.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn late_stream_events_from_a_cancelled_request_do_not_reach_the_next_request() {
    let (_temporary, core, _) = test_core("late-events");
    core.select_model(fixture_model())
        .await
        .expect("select model");
    let mut events = core.subscribe_lossless();
    let cancelled = core
        .submit(Message::user("late-cancel"))
        .await
        .expect("cancelled submission");
    receive_until(&mut events, cancelled, |event| {
        matches!(
            event,
            CoreEvent::SubmissionStarted { submission, .. } if *submission == cancelled
        )
    })
    .await;
    core.cancel(cancelled).await.expect("cancel");
    receive_until(
        &mut events,
        cancelled,
        |event| matches!(event, CoreEvent::Cancelled { submission } if *submission == cancelled),
    )
    .await;

    let next = core
        .submit(Message::user("late-next"))
        .await
        .expect("next submission");
    let received = receive_until(
        &mut events,
        next,
        |event| matches!(event, CoreEvent::Completed { submission } if *submission == next),
    )
    .await;
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
    core.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn stream_terminal_event_without_request_id_fails_the_active_submission_promptly() {
    let (_temporary, core, _) = test_core("missing-terminal-id");
    core.select_model(fixture_model())
        .await
        .expect("select model");
    let mut events = core.subscribe_lossless();
    let submission = core
        .submit(Message::user("no-id-terminal"))
        .await
        .expect("submission");
    let received = receive_until(&mut events, submission, |event| {
        matches!(
            event,
            CoreEvent::Failed { submission: id, message }
                if *id == submission && message.contains("request_id")
        )
    })
    .await;
    assert!(received.iter().any(
        |event| matches!(event, CoreEvent::Failed { message, .. } if message.contains("request_id"))
    ));
    core.shutdown().await.expect("shutdown");
}
