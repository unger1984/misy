//! Independent child-agent lifecycle integration coverage.

use super::*;
use misy_core::{ActivityKind, ActivityStatus, CoreError};

async fn prepare_agent_core(name: &str) -> (tempfile::TempDir, MisyCore, PathBuf) {
    let fixture = test_core(name);
    let provider = ProviderId::new("fixture");
    fixture
        .1
        .complete_auth(
            &provider,
            json!({"id": "fixture-session"}),
            json!({"code": "opaque"}),
        )
        .await
        .expect("authenticate fixture provider");
    fixture
        .1
        .list_models(&provider)
        .await
        .expect("cache models");
    fixture
        .1
        .select_model(fixture_model())
        .await
        .expect("select model");
    fixture
}

async fn wait_for_agent_finish(core: &MisyCore, events: &mut UnboundedReceiver<CoreEvent>) {
    loop {
        if core
            .snapshot()
            .agents
            .first()
            .is_some_and(|agent| agent.status.is_terminal())
        {
            return;
        }
        if matches!(receive_event(events).await, CoreEvent::AgentFinished { .. }) {
            return;
        }
    }
}

#[tokio::test]
async fn child_fallback_retries_only_a_safe_retryable_failure() {
    let (_temporary, core, _) = prepare_agent_core("fallback-retryable").await;
    let mut events = core.subscribe_lossless();
    let submission = core
        .submit(Message::user("spawn-fallback-retryable"))
        .await
        .expect("submit fallback request");
    receive_until(&mut events, submission, |_| false).await;

    let agent = core.snapshot().agents[0].clone();
    assert_eq!(agent.status, ActivityStatus::Completed);
    assert_eq!(agent.attempts.len(), 2);
    assert_eq!(agent.attempts[0].status, "failed");
    assert_eq!(agent.attempts[1].status, "completed");
    assert_eq!(
        agent.terminal_message.as_deref(),
        Some("fallback-recovered")
    );
    core.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn exhausted_fallback_reports_each_bounded_profile_failure() {
    let (_temporary, core, _) = prepare_agent_core("fallback-exhausted").await;
    let mut events = core.subscribe_lossless();
    let submission = core
        .submit(Message::user("spawn-fallback-exhausted"))
        .await
        .expect("submit exhausted fallback request");
    receive_until(&mut events, submission, |_| false).await;
    wait_for_agent_finish(&core, &mut events).await;

    let agent = core.snapshot().agents[0].clone();
    assert_eq!(agent.status, ActivityStatus::Failed);
    assert_eq!(agent.attempts.len(), 2);
    let result = agent.terminal_message.expect("terminal failure");
    assert!(
        result.contains("fixture/fixture-model: rate limit exceeded"),
        "terminal result: {result}"
    );
    assert!(
        result.contains("fixture/fixture-model-b: server unavailable"),
        "terminal result: {result}"
    );
    assert!(result.len() <= 2_048);
    core.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn refusal_and_post_output_failure_do_not_cross_the_fallback_boundary() {
    for (name, prompt) in [
        ("fallback-refusal", "spawn-fallback-refusal"),
        ("fallback-after-output", "spawn-fallback-after-output"),
        ("fallback-after-tool", "spawn-fallback-after-tool"),
    ] {
        let (_temporary, core, _) = prepare_agent_core(name).await;
        let mut events = core.subscribe_lossless();
        let submission = core
            .submit(Message::user(prompt))
            .await
            .expect("submit terminal fallback request");
        receive_until(&mut events, submission, |_| false).await;
        wait_for_agent_finish(&core, &mut events).await;

        let agent = core.snapshot().agents[0].clone();
        assert_eq!(agent.status, ActivityStatus::Failed);
        assert_eq!(agent.attempts.len(), 1);
        core.shutdown().await.expect("shutdown");
    }
}

#[tokio::test]
async fn synchronous_agent_returns_inline_without_mutating_parent_with_its_assignment() {
    let (_temporary, core, _) = prepare_agent_core("spawn-sync").await;
    let mut events = core.subscribe_lossless();
    let submission = core
        .submit(Message::user("spawn-sync"))
        .await
        .expect("submit sync spawn");
    let received = receive_until(
        &mut events,
        submission,
        |event| matches!(event, CoreEvent::Completed { submission: id } if *id == submission),
    )
    .await;
    assert!(received.iter().any(|event| {
        matches!(event, CoreEvent::TextDelta { delta, .. } if delta == "sync-observed")
    }));
    assert!(
        core.history()
            .await
            .iter()
            .all(|entry| entry.message.content != "child-sync-task")
    );
    let snapshot = core.snapshot();
    assert_eq!(snapshot.agents.len(), 1);
    assert_eq!(snapshot.agents[0].status, ActivityStatus::Completed);
    assert_eq!(
        snapshot.agents[0].terminal_message.as_deref(),
        Some("child-sync-answer")
    );
    assert!(snapshot.activities.iter().any(|activity| {
        activity.kind == ActivityKind::Agent && activity.agent_id == Some(snapshot.agents[0].id)
    }));
    core.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn background_agent_completion_is_pullable_and_remains_available_after_consumption() {
    let (_temporary, core, _) = prepare_agent_core("spawn-background").await;
    let mut events = core.subscribe_lossless();
    let submission = core
        .submit(Message::user("spawn-background"))
        .await
        .expect("submit background spawn");
    let received = receive_until(
        &mut events,
        submission,
        |event| matches!(event, CoreEvent::Completed { submission: id } if *id == submission),
    )
    .await;
    assert!(
        received
            .iter()
            .any(|event| matches!(event, CoreEvent::AgentFinished { .. }))
    );
    let agent = core.snapshot().agents[0].clone();
    assert_eq!(agent.status, ActivityStatus::Completed);
    let transcript = core
        .agent_transcript(agent.id)
        .expect("retained transcript");
    assert!(
        transcript
            .entries
            .iter()
            .any(|entry| entry.content.contains("child-background-answer"))
    );
    assert!(matches!(
        core.new_session(),
        Err(CoreError::SessionAgentStatePending { .. })
    ));
    core.discard_agent_state()
        .await
        .expect("discard agent state");
    core.new_session().expect("new session after discard");
    core.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn accepted_agent_message_produces_another_child_turn() {
    let (_temporary, core, _) = prepare_agent_core("spawn-message").await;
    let mut events = core.subscribe_lossless();
    let submission = core
        .submit(Message::user("spawn-message"))
        .await
        .expect("submit message flow");
    receive_until(
        &mut events,
        submission,
        |event| matches!(event, CoreEvent::Completed { submission: id } if *id == submission),
    )
    .await;
    let agent = core.snapshot().agents[0].clone();
    assert_eq!(agent.terminal_message.as_deref(), Some("follow-up-answer"));
    let transcript = core.agent_transcript(agent.id).expect("agent transcript");
    assert!(
        transcript
            .entries
            .iter()
            .any(|entry| entry.content == "follow-up child")
    );
    core.shutdown().await.expect("shutdown");
}
