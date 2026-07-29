//! Hierarchical instruction preflight and context-report integration coverage.

use super::*;

#[tokio::test]
async fn nested_scopes_retry_the_whole_batch_before_any_write() {
    let (temporary, core, frontend, backend) = test_core_in_workspace("instruction-preflight");
    core.select_model(fixture_model())
        .await
        .expect("select model");
    let mut events = core.subscribe_lossless();
    let submission = core
        .submit(Message::user("instruction-preflight"))
        .await
        .expect("submit");
    let mut retry_results = 0;
    while retry_results < 2 {
        let event = receive_event(&mut events).await;
        if let CoreEvent::ToolResult { result, .. } = event
            && result.content.contains("instruction_scope_retry_required")
        {
            retry_results += 1;
        }
    }
    assert!(!frontend.exists());
    assert!(!backend.exists());
    fs::write(
        frontend
            .parent()
            .expect("frontend parent")
            .parent()
            .expect("workspace")
            .join("allow-retry"),
        "allow",
    )
    .expect("release fixture");
    let terminal = receive_until(
        &mut events,
        submission,
        |event| matches!(event, CoreEvent::Completed { submission: id } if *id == submission),
    )
    .await;
    assert!(terminal.iter().any(
        |event| matches!(event, CoreEvent::Completed { submission: id } if *id == submission)
    ));
    assert_eq!(
        fs::read_to_string(&frontend).expect("frontend write"),
        "frontend"
    );
    assert_eq!(
        fs::read_to_string(&backend).expect("backend write"),
        "backend"
    );

    let report = core.context_report();
    assert!(
        report
            .sources
            .iter()
            .any(|source| source.display_path.ends_with("frontend/AGENTS.md"))
    );
    assert!(
        report
            .sources
            .iter()
            .any(|source| source.display_path.ends_with("backend/AGENTS.md"))
    );
    let history = core.history().await;
    assert!(history.iter().all(|entry| {
        entry
            .tool_results
            .iter()
            .all(|result| !result.content.contains("instruction_scope_retry_required"))
    }));
    let session = fs::read_to_string(temporary.path().join("misy/sessions").join(format!(
        "{}.jsonl",
        core.current_session_id().expect("current session")
    )))
    .expect("session file");
    assert!(!session.contains("instruction_scope_retry_required"));
    assert!(!session.contains("global rules"));
    assert!(!session.contains("root rules"));
    assert!(!session.contains("frontend rules"));
    assert!(!session.contains("backend rules"));
    core.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn new_and_resume_reload_base_instructions_transactionally() {
    let (temporary, core, frontend, _) = test_core_in_workspace("instruction-session-reload");
    core.select_model(fixture_model())
        .await
        .expect("select model");
    let mut events = core.subscribe_lossless();
    let submission = core
        .submit(Message::user("session-one"))
        .await
        .expect("submit");
    receive_until(&mut events, submission, |_| false).await;
    let session_id = core.current_session_id().expect("persisted session");
    let workspace = frontend.parent().and_then(Path::parent).expect("workspace");

    fs::write(workspace.join("AGENTS.md"), "new root rules").expect("change root rules");
    core.new_session().expect("new session");
    assert!(core.context_report().sources.iter().any(|source| {
        source.display_path.ends_with("workspace/AGENTS.md")
            && source.original_bytes == "new root rules".len()
    }));

    fs::write(workspace.join("AGENTS.md"), "resumed root rules").expect("change root again");
    core.resume_session(&session_id).expect("resume session");
    assert!(core.context_report().sources.iter().any(|source| {
        source.display_path.ends_with("workspace/AGENTS.md")
            && source.original_bytes == "resumed root rules".len()
    }));

    let history_before_failure = core.history().await;
    fs::write(workspace.join("AGENTS.md"), [0xff]).expect("break root rules");
    assert!(matches!(
        core.new_session(),
        Err(misy_core::CoreError::InstructionBlocked(_))
    ));
    assert_eq!(core.history().await, history_before_failure);
    assert!(matches!(
        core.resume_session(&session_id),
        Err(misy_core::CoreError::InstructionBlocked(_))
    ));
    assert_eq!(core.history().await, history_before_failure);
    assert!(core.context_report().sources.iter().any(|source| {
        source.display_path.ends_with("workspace/AGENTS.md")
            && source.original_bytes == "resumed root rules".len()
    }));
    core.shutdown().await.expect("shutdown");

    drop(temporary);
}
