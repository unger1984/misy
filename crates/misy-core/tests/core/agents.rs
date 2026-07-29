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
