//! Interactive question and checklist behavior across the public core boundary.

use super::*;
use misy_core::{CoreError, QuestionResponse, TodoItem, TodoStatus};
use std::collections::BTreeMap;

#[tokio::test]
async fn question_waits_for_answer_and_resolves_before_tool_result() {
    let (_temporary, core, _) = test_core_with_questions("question-round-trip");
    core.select_model(fixture_model())
        .await
        .expect("select model");
    let mut events = core.subscribe_lossless();
    let submission = core
        .submit(Message::user("question-round-trip"))
        .await
        .expect("submit");

    let requested = receive_until(&mut events, submission, |event| {
        matches!(event, CoreEvent::QuestionRequested { .. })
    })
    .await
    .into_iter()
    .find_map(|event| match event {
        CoreEvent::QuestionRequested { request } => Some(request),
        _ => None,
    })
    .expect("question request");
    assert_eq!(core.snapshot().pending_questions, vec![requested.clone()]);

    core.answer_question(QuestionResponse {
        request_id: requested.id,
        answers: BTreeMap::from([("Choose a mode".to_owned(), "Safe".to_owned())]),
    })
    .expect("answer question");
    let received = receive_until(&mut events, submission, |event| {
        matches!(event, CoreEvent::Completed { .. })
    })
    .await;
    let resolved = received
        .iter()
        .position(|event| matches!(event, CoreEvent::QuestionResolved { .. }))
        .expect("resolved event");
    let result = received
        .iter()
        .position(|event| matches!(event, CoreEvent::ToolResult { .. }))
        .expect("tool result");
    assert!(resolved < result);
    assert!(core.snapshot().pending_questions.is_empty());
    core.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn invalid_question_answer_keeps_request_pending() {
    let (_temporary, core, _) = test_core_with_questions("question-round-trip");
    core.select_model(fixture_model())
        .await
        .expect("select model");
    let mut events = core.subscribe_lossless();
    let submission = core
        .submit(Message::user("question-round-trip"))
        .await
        .expect("submit");
    let request = receive_until(&mut events, submission, |event| {
        matches!(event, CoreEvent::QuestionRequested { .. })
    })
    .await
    .into_iter()
    .find_map(|event| match event {
        CoreEvent::QuestionRequested { request } => Some(request),
        _ => None,
    })
    .expect("question request");

    let error = core
        .answer_question(QuestionResponse {
            request_id: request.id,
            answers: BTreeMap::new(),
        })
        .expect_err("incomplete answers must fail");
    assert!(matches!(error, CoreError::InvalidQuestionResponse(_)));
    assert_eq!(core.snapshot().pending_questions.len(), 1);
    core.dismiss_question(request.id).expect("dismiss cleanup");
    core.cancel_all_submissions().await;
    core.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn shutdown_wakes_a_pending_question_within_bound() {
    let (_temporary, core, _) = test_core_with_questions("question-round-trip");
    core.select_model(fixture_model())
        .await
        .expect("select model");
    let mut events = core.subscribe_lossless();
    let submission = core
        .submit(Message::user("question-round-trip"))
        .await
        .expect("submit");
    receive_until(&mut events, submission, |event| {
        matches!(event, CoreEvent::QuestionRequested { .. })
    })
    .await;

    tokio::time::timeout(Duration::from_secs(5), core.shutdown())
        .await
        .expect("shutdown deadline")
        .expect("shutdown");
}

#[tokio::test]
async fn todo_update_queries_and_projects_root_snapshot() {
    let (_temporary, core, _) = test_core("todo-round-trip");
    core.select_model(fixture_model())
        .await
        .expect("select model");
    let mut events = core.subscribe_lossless();
    let submission = core
        .submit(Message::user("todo-round-trip"))
        .await
        .expect("submit");
    receive_until(&mut events, submission, |event| {
        matches!(event, CoreEvent::Completed { .. })
    })
    .await;

    assert_eq!(
        core.snapshot().todos,
        vec![TodoItem {
            title: "Implement lifecycle".to_owned(),
            status: TodoStatus::Done,
        }]
    );
    let history = core.history().await;
    assert!(history.iter().any(|entry| {
        entry
            .tool_results
            .iter()
            .any(|result| result.content.contains("Current todo list"))
    }));
    core.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn question_tool_is_hidden_without_capability() {
    let (_temporary, core, _) = test_core("question-capability-gate");
    core.select_model(fixture_model())
        .await
        .expect("select model");
    let mut events = core.subscribe_lossless();
    let submission = core
        .submit(Message::user("question-capability-gate"))
        .await
        .expect("submit");
    let received = receive_until(&mut events, submission, |event| {
        matches!(event, CoreEvent::Completed { .. })
    })
    .await;
    assert!(
        !received
            .iter()
            .any(|event| matches!(event, CoreEvent::Failed { .. }))
    );
    core.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn three_dismissals_suppress_further_dialogs_for_the_owner() {
    let (_temporary, core, _) = test_core_with_questions("question-dismiss-limit");
    core.select_model(fixture_model())
        .await
        .expect("select model");
    let mut events = core.subscribe_lossless();
    let submission = core
        .submit(Message::user("question-dismiss-limit"))
        .await
        .expect("submit");
    let mut requested = 0;
    loop {
        let event = receive_event(&mut events).await;
        match event {
            CoreEvent::QuestionRequested { request } => {
                requested += 1;
                core.dismiss_question(request.id).expect("dismiss question");
            }
            CoreEvent::Completed { submission: id } if id == submission => break,
            CoreEvent::Failed { message, .. } => panic!("unexpected failure: {message}"),
            _ => {}
        }
    }
    assert_eq!(requested, 3);
    core.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn child_question_routes_through_the_same_client_registry() {
    let (_temporary, core, _) = test_core_with_questions("spawn-child-question");
    let provider = ProviderId::new("fixture");
    core.complete_auth(
        &provider,
        json!({"id": "fixture-session"}),
        json!({"code": "opaque"}),
    )
    .await
    .expect("authenticate");
    core.list_models(&provider).await.expect("cache models");
    core.select_model(fixture_model())
        .await
        .expect("select model");
    let mut events = core.subscribe_lossless();
    let submission = core
        .submit(Message::user("spawn-child-question"))
        .await
        .expect("submit");
    let request = receive_until(&mut events, submission, |event| {
        matches!(event, CoreEvent::QuestionRequested { .. })
    })
    .await
    .into_iter()
    .find_map(|event| match event {
        CoreEvent::QuestionRequested { request } => Some(request),
        _ => None,
    })
    .expect("child question");
    assert!(matches!(
        request.source,
        misy_core::QuestionSource::Agent(_)
    ));
    core.answer_question(QuestionResponse {
        request_id: request.id,
        answers: BTreeMap::from([("Child choice".to_owned(), "A".to_owned())]),
    })
    .expect("answer child");
    let received = receive_until(&mut events, submission, |event| {
        matches!(event, CoreEvent::Completed { .. })
    })
    .await;
    assert!(received.iter().any(|event| {
        matches!(event, CoreEvent::TextDelta { delta, .. } if delta == "parent-question-observed")
    }));
    core.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn child_todo_is_isolated_from_the_root_snapshot() {
    let (_temporary, core, _) = test_core("spawn-child-todo");
    let provider = ProviderId::new("fixture");
    core.complete_auth(
        &provider,
        json!({"id": "fixture-session"}),
        json!({"code": "opaque"}),
    )
    .await
    .expect("authenticate");
    core.list_models(&provider).await.expect("cache models");
    core.select_model(fixture_model())
        .await
        .expect("select model");
    let mut events = core.subscribe_lossless();
    let submission = core
        .submit(Message::user("spawn-child-todo"))
        .await
        .expect("submit");
    receive_until(&mut events, submission, |event| {
        matches!(event, CoreEvent::Completed { .. })
    })
    .await;

    assert_eq!(core.snapshot().todos[0].title, "Root item");
    let agent = core.snapshot().agents[0].id;
    let transcript = core.agent_transcript(agent).expect("child transcript");
    assert!(
        transcript.entries.iter().any(|entry| {
            entry
                .tool_result
                .as_ref()
                .is_some_and(|result| result.content.contains("Child item"))
        }),
        "{transcript:#?}"
    );
    core.shutdown().await.expect("shutdown");
}
