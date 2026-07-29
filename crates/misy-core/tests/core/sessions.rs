//! Persistent conversation-session behavior through the public core contract.

use super::*;

fn only_session_file(temporary: &tempfile::TempDir) -> PathBuf {
    fs::read_dir(temporary.path().join("misy/sessions"))
        .expect("sessions directory")
        .next()
        .expect("session file")
        .expect("session entry")
        .path()
}

async fn complete_submission(core: &MisyCore, text: &str) {
    let mut events = core.subscribe_lossless();
    let submission = core
        .submit(Message::user(text))
        .await
        .expect("submit message");
    receive_until(&mut events, submission, |_| false).await;
}

#[tokio::test]
async fn completed_conversation_can_be_started_over_and_resumed() {
    let (_temporary, core, _) = test_core("persistent-session");
    core.select_model(fixture_model())
        .await
        .expect("select model");
    complete_submission(&core, "session-one").await;

    let sessions = core.list_sessions().expect("list sessions");
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].preview, "session-one");
    let id = sessions[0].id.clone();

    core.new_session().expect("start new session");
    assert!(core.history().await.is_empty());
    assert_eq!(core.current_session_id(), None);

    let resumed = core.resume_session(&id).expect("resume session");
    assert_eq!(resumed.session.id, id);
    assert!(resumed.history_len >= 2);
    assert_eq!(core.current_session_id().as_deref(), Some(id.as_str()));
    assert_eq!(core.history().await[0].message.content, "session-one");

    complete_submission(&core, "session-two").await;
    assert!(core.history().await.len() > resumed.history_len);
    assert_eq!(
        core.list_sessions().expect("list resumed sessions").len(),
        1
    );
    core.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn session_switch_is_rejected_while_submission_is_active() {
    let (_temporary, core, _) = test_core("session-switch-busy");
    core.select_model(fixture_model())
        .await
        .expect("select model");
    let mut events = core.subscribe_lossless();
    let submission = core
        .submit(Message::user("block-session"))
        .await
        .expect("submit blocking message");
    receive_until(&mut events, submission, |event| {
        matches!(event, CoreEvent::SubmissionStarted { submission: id, .. } if *id == submission)
    })
    .await;

    assert!(
        core.new_session()
            .expect_err("active session must not switch")
            .to_string()
            .contains("active")
    );
    core.cancel(submission).await.expect("cancel submission");
    core.shutdown().await.expect("shutdown");
}

#[cfg(unix)]
#[tokio::test]
async fn session_file_is_private_jsonl() {
    use std::os::unix::fs::PermissionsExt;

    let (temporary, core, _) = test_core("session-permissions");
    core.select_model(fixture_model())
        .await
        .expect("select model");
    complete_submission(&core, "session-one").await;

    let file = only_session_file(&temporary);
    let contents = fs::read_to_string(&file).expect("session contents");
    assert!(contents.lines().count() >= 3);
    assert_eq!(
        fs::metadata(file)
            .expect("session metadata")
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    core.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn resume_accepts_prefix_and_skips_a_truncated_tail() {
    use std::io::Write;

    let (temporary, core, _) = test_core("session-truncated-tail");
    core.select_model(fixture_model())
        .await
        .expect("select model");
    complete_submission(&core, "session-one").await;
    let id = core.current_session_id().expect("current session id");
    writeln!(
        fs::OpenOptions::new()
            .append(true)
            .open(only_session_file(&temporary))
            .expect("open session tail"),
        "{{truncated"
    )
    .expect("write truncated tail");

    core.new_session().expect("new session");
    let prefix: String = id.chars().take(12).collect();
    let outcome = core.resume_session(&prefix).expect("resume by prefix");

    assert_eq!(outcome.session.id, id);
    assert_eq!(outcome.history[0].message.content, "session-one");
    core.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn resume_rejects_an_unknown_session_version() {
    let (temporary, core, _) = test_core("session-version");
    core.select_model(fixture_model())
        .await
        .expect("select model");
    complete_submission(&core, "session-one").await;
    let id = core.current_session_id().expect("current session id");
    let file = only_session_file(&temporary);
    let contents = fs::read_to_string(&file).expect("session contents");
    let mut lines = contents.lines();
    let mut header: serde_json::Value =
        serde_json::from_str(lines.next().expect("session header")).expect("parse header");
    header["version"] = json!(99);
    let mut rewritten = serde_json::to_string(&header).expect("encode header");
    rewritten.push('\n');
    rewritten.push_str(&lines.collect::<Vec<_>>().join("\n"));
    rewritten.push('\n');
    fs::write(file, rewritten).expect("rewrite session");

    core.new_session().expect("new session");
    let error = core
        .resume_session(&id)
        .expect_err("unknown version must fail");

    assert!(error.to_string().contains("unsupported session version 99"));
    core.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn resume_restores_the_last_saved_model() {
    let (_temporary, core, _) = test_core("session-model-change");
    core.complete_auth(
        &ProviderId::new("fixture"),
        json!({"id": "fixture-session"}),
        json!({"code": "opaque"}),
    )
    .await
    .expect("authenticate provider");
    core.select_model(fixture_model())
        .await
        .expect("select first model");
    complete_submission(&core, "session-one").await;
    let second = ModelRef::new(ProviderId::new("fixture"), ModelId::new("fixture-model-b"));
    core.select_model(second.clone())
        .await
        .expect("select second model");
    let id = core.current_session_id().expect("current session id");
    core.new_session().expect("new session");
    core.select_model(fixture_model())
        .await
        .expect("change current model");

    let outcome = core.resume_session(&id).expect("resume session");

    assert_eq!(outcome.model_warning, None);
    assert_eq!(core.snapshot().selected_model, Some(second));
    core.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn empty_sessions_are_not_written_and_other_working_directories_are_filtered() {
    let (temporary, core, _) = test_core("session-cwd-filter");
    assert!(core.list_sessions().expect("empty session list").is_empty());
    assert!(!temporary.path().join("misy/sessions").exists());

    core.select_model(fixture_model())
        .await
        .expect("select model");
    complete_submission(&core, "session-one").await;
    let file = only_session_file(&temporary);
    let contents = fs::read_to_string(&file).expect("session contents");
    let mut lines = contents.lines();
    let mut header: serde_json::Value =
        serde_json::from_str(lines.next().expect("session header")).expect("parse header");
    header["cwd"] = json!(temporary.path().join("another-project"));
    let mut rewritten = serde_json::to_string(&header).expect("encode header");
    rewritten.push('\n');
    rewritten.push_str(&lines.collect::<Vec<_>>().join("\n"));
    rewritten.push('\n');
    fs::write(file, rewritten).expect("rewrite session cwd");

    assert!(
        core.list_sessions()
            .expect("filtered session list")
            .is_empty()
    );
    core.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn resume_rejects_an_ambiguous_id_prefix() {
    let (temporary, core, _) = test_core("session-ambiguous-prefix");
    core.select_model(fixture_model())
        .await
        .expect("select model");
    complete_submission(&core, "session-one").await;
    let id = core.current_session_id().expect("current session id");
    let prefix: String = id.chars().take(8).collect();
    let source = only_session_file(&temporary);
    let duplicate = source
        .parent()
        .expect("session parent")
        .join(format!("{prefix}-duplicate.jsonl"));
    fs::copy(source, duplicate).expect("duplicate session file");
    core.new_session().expect("new session");

    let error = core
        .resume_session(&prefix)
        .expect_err("ambiguous prefix must fail");

    assert!(error.to_string().contains("ambiguous"));
    core.shutdown().await.expect("shutdown");
}

#[cfg(unix)]
#[tokio::test]
async fn persistence_failure_is_reported_without_failing_the_turn() {
    use std::os::unix::fs::PermissionsExt;

    let (temporary, core, _) = test_core("session-write-failure");
    core.select_model(fixture_model())
        .await
        .expect("select model");
    complete_submission(&core, "session-one").await;
    let file = only_session_file(&temporary);
    fs::set_permissions(&file, fs::Permissions::from_mode(0o400)).expect("make session read-only");
    let mut events = core.subscribe_lossless();
    let submission = core
        .submit(Message::user("session-two"))
        .await
        .expect("submit second message");

    let received = receive_until(&mut events, submission, |_| false).await;

    assert!(
        received
            .iter()
            .any(|event| matches!(event, CoreEvent::SessionPersistenceFailed { .. }))
    );
    assert!(received.iter().any(
        |event| matches!(event, CoreEvent::Completed { submission: id } if *id == submission)
    ));
    fs::set_permissions(file, fs::Permissions::from_mode(0o600)).expect("restore permissions");
    core.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn unavailable_saved_model_warns_and_keeps_the_current_selection() {
    use std::io::Write;

    let (temporary, core, _) = test_core("session-model-fallback");
    core.complete_auth(
        &ProviderId::new("fixture"),
        json!({"id": "fixture-session"}),
        json!({"code": "opaque"}),
    )
    .await
    .expect("authenticate provider");
    core.select_model(fixture_model())
        .await
        .expect("select model");
    complete_submission(&core, "session-one").await;
    let id = core.current_session_id().expect("current session id");
    writeln!(
        fs::OpenOptions::new()
            .append(true)
            .open(only_session_file(&temporary))
            .expect("open session"),
        "{}",
        json!({
            "ordinal": 999,
            "timestamp": 1,
            "record": {
                "type": "model_change",
                "model": {"provider": "missing", "model": "gone"}
            }
        })
    )
    .expect("append unavailable model");
    core.new_session().expect("new session");

    let outcome = core.resume_session(&id).expect("resume session");

    assert!(outcome.model_warning.is_some());
    assert_eq!(core.snapshot().selected_model, Some(fixture_model()));
    core.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn session_survives_a_core_process_reopen() {
    let (temporary, core, _) = test_core("session-reopen");
    core.select_model(fixture_model())
        .await
        .expect("select model");
    complete_submission(&core, "session-one").await;
    let id = core.current_session_id().expect("current session id");
    core.shutdown().await.expect("shutdown first core");
    drop(core);

    let reopened = MisyCore::discover(
        MisyPaths::from_root(temporary.path().join("misy")),
        temporary.path().join("bundled"),
    )
    .expect("reopen core");
    let outcome = reopened.resume_session(&id).expect("resume after reopen");

    assert_eq!(outcome.history[0].message.content, "session-one");
    reopened.shutdown().await.expect("shutdown reopened core");
}

#[tokio::test]
async fn initial_storage_failure_emits_once_and_does_not_fail_the_turn() {
    let (temporary, core, _) = test_core("session-create-failure");
    core.select_model(fixture_model())
        .await
        .expect("select model");
    fs::create_dir_all(temporary.path().join("misy")).expect("create Misy root");
    fs::write(temporary.path().join("misy/sessions"), "not a directory")
        .expect("block sessions directory");
    let mut events = core.subscribe_lossless();
    let submission = core
        .submit(Message::user("session-one"))
        .await
        .expect("submit message");

    let received = receive_until(&mut events, submission, |_| false).await;

    assert_eq!(
        received
            .iter()
            .filter(|event| matches!(event, CoreEvent::SessionPersistenceFailed { .. }))
            .count(),
        1
    );
    assert!(received.iter().any(
        |event| matches!(event, CoreEvent::Completed { submission: id } if *id == submission)
    ));
    core.shutdown().await.expect("shutdown");
}
