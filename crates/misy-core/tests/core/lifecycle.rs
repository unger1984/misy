//! Additional headless-core integration coverage.

use super::*;

#[tokio::test]
async fn core_returns_tool_errors_to_the_provider_for_malformed_and_unknown_calls() {
    for prompt in ["bad-tool-arguments", "unknown-tool"] {
        let (_temporary, core, _) = test_core(prompt);
        core.select_model(fixture_model())
            .await
            .expect("select model");
        let mut events = core.subscribe_lossless();
        let submission = core.submit(Message::user(prompt)).await.expect("submit");
        let received = receive_until(
            &mut events,
            submission,
            |event| matches!(event, CoreEvent::Completed { submission: id } if *id == submission),
        )
        .await;
        assert!(
            received.iter().any(
                |event| matches!(event, CoreEvent::ToolResult { result, .. } if result.is_error)
            )
        );
        core.shutdown().await.expect("shutdown");
    }
}

#[tokio::test]
async fn core_emits_provider_failure_and_honours_cancellation() {
    let (_temporary, core, _) = test_core("failure-cancel");
    core.select_model(fixture_model())
        .await
        .expect("select model");
    let mut events = core.subscribe_lossless();

    let failed = core
        .submit(Message::user("provider-failure"))
        .await
        .expect("submit");
    let received = receive_until(
        &mut events,
        failed,
        |event| matches!(event, CoreEvent::Failed { submission, .. } if *submission == failed),
    )
    .await;
    assert!(
        received
            .iter()
            .any(|event| matches!(event, CoreEvent::Failed { .. }))
    );

    let cancelled = core
        .submit(Message::user("cancel-me"))
        .await
        .expect("submit");
    core.cancel(cancelled).await.expect("cancel");
    let received = receive_until(
        &mut events,
        cancelled,
        |event| matches!(event, CoreEvent::Cancelled { submission } if *submission == cancelled),
    )
    .await;
    assert!(
        received
            .iter()
            .any(|event| matches!(event, CoreEvent::Cancelled { .. }))
    );
    core.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn cancellation_before_tool_dispatch_prevents_tool_side_effects() {
    let (_temporary, core, target) = test_core("cancel-before-tool");
    core.select_model(fixture_model())
        .await
        .expect("select model");
    let mut events = core.subscribe_lossless();
    let submission = core
        .submit(Message::user("cancel-before-tool"))
        .await
        .expect("submit");

    receive_until(
        &mut events,
        submission,
        |event| matches!(event, CoreEvent::ToolCall { submission: id, .. } if *id == submission),
    )
    .await;
    core.cancel(submission).await.expect("cancel");
    receive_until(
        &mut events,
        submission,
        |event| matches!(event, CoreEvent::Cancelled { submission: id } if *id == submission),
    )
    .await;
    assert!(!target.exists(), "cancelled tool must not write a file");
    core.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn foreign_current_thread_runtime_receives_core_events() {
    let (_temporary, core, _) = test_core("cross-runtime-events");
    let foreign_core = core.clone();
    let (ready_sender, ready_receiver) = tokio::sync::oneshot::channel();
    let (event_sender, event_receiver) = tokio::sync::oneshot::channel();
    let foreign_runtime = std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("foreign current-thread runtime")
            .block_on(async move {
                let mut events = foreign_core.subscribe_lossless();
                let snapshot = receive_event(&mut events).await;
                assert!(matches!(snapshot, CoreEvent::ProviderDiscovered { .. }));
                ready_sender
                    .send(())
                    .expect("signal foreign subscription readiness");
                event_sender
                    .send(receive_event(&mut events).await)
                    .expect("return foreign runtime event");
            });
    });

    tokio::time::timeout(Duration::from_secs(3), ready_receiver)
        .await
        .expect("foreign subscription readiness deadline")
        .expect("foreign runtime dropped readiness signal");
    core.select_model(fixture_model())
        .await
        .expect("select model");
    let event = tokio::time::timeout(Duration::from_secs(3), event_receiver)
        .await
        .expect("foreign event deadline")
        .expect("foreign runtime dropped event");
    assert!(matches!(event, CoreEvent::ModelSelected { model } if model == fixture_model()));
    foreign_runtime.join().expect("foreign runtime thread");
    core.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn concurrent_shutdown_is_idempotent_and_rejects_new_submissions() {
    let (_temporary, core, _) = test_core("concurrent-shutdown");
    core.select_model(fixture_model())
        .await
        .expect("select model");

    let first = {
        let core = core.clone();
        tokio::spawn(async move { core.shutdown().await })
    };
    let second = {
        let core = core.clone();
        tokio::spawn(async move { core.shutdown().await })
    };
    first
        .await
        .expect("first shutdown task")
        .expect("first shutdown");
    second
        .await
        .expect("second shutdown task")
        .expect("second shutdown");
    assert!(matches!(
        core.submit(Message::user("after shutdown")).await,
        Err(misy_core::CoreError::Shutdown)
    ));
}

#[tokio::test]
async fn shutdown_closes_submission_admission_before_queue_cleanup() {
    let (_temporary, core, _) = test_core("shutdown-admission");
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
    let queued = core
        .submit(Message::user("queued-before-shutdown"))
        .await
        .expect("queued submission");

    let shutdown = {
        let core = core.clone();
        tokio::spawn(async move { core.shutdown().await })
    };
    loop {
        match core.submit(Message::user("race-shutdown")).await {
            Err(misy_core::CoreError::Shutdown) => break,
            Ok(_) => tokio::task::yield_now().await,
            Err(error) => panic!("unexpected submission result while shutting down: {error}"),
        }
    }
    assert!(matches!(
        core.submit(Message::user("after-close")).await,
        Err(misy_core::CoreError::Shutdown)
    ));
    shutdown.await.expect("shutdown task").expect("shutdown");
    let snapshot = core.snapshot();
    assert_eq!(snapshot.active_submission, None);
    assert!(snapshot.queued_submissions.is_empty());
    assert!(matches!(
        core.cancel(queued).await,
        Err(misy_core::CoreError::Shutdown)
    ));
}

#[tokio::test]
async fn dropping_the_last_core_handle_inside_tokio_does_not_panic() {
    tokio::time::timeout(
        Duration::from_secs(3),
        tokio::spawn(async {
            let (_temporary, core, _) = test_core("drop-in-tokio");
            drop(core);
        }),
    )
    .await
    .expect("drop deadline")
    .expect("drop task must not panic");
}
