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
async fn todo_snapshot_survives_session_resume() {
    let (temporary, core, _) = test_core("todo-round-trip");
    core.select_model(fixture_model())
        .await
        .expect("select model");
    complete_submission(&core, "todo-round-trip").await;
    let id = core.current_session_id().expect("current session id");
    assert_eq!(core.snapshot().todos.len(), 1);

    core.new_session().expect("new session");
    assert!(core.snapshot().todos.is_empty());
    let outcome = core.resume_session(&id).expect("resume todo session");
    assert_eq!(outcome.todos.len(), 1);
    assert_eq!(core.snapshot().todos, outcome.todos);
    assert!(only_session_file(&temporary).exists());
    core.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn unknown_record_advances_the_next_append_ordinal() {
    use std::io::Write;

    let (temporary, core, _) = test_core("unknown-session-record");
    core.select_model(fixture_model())
        .await
        .expect("select model");
    complete_submission(&core, "session-one").await;
    let id = core.current_session_id().expect("current session id");
    let file = only_session_file(&temporary);
    writeln!(
        fs::OpenOptions::new()
            .append(true)
            .open(&file)
            .expect("open session"),
        "{}",
        json!({
            "ordinal": 999,
            "timestamp": 1,
            "record": {"type": "future_record", "payload": true}
        })
    )
    .expect("append unknown record");
    core.new_session().expect("detach session");
    core.resume_session(&id).expect("resume session");
    complete_submission(&core, "session-two").await;

    let last = fs::read_to_string(file)
        .expect("session file")
        .lines()
        .last()
        .and_then(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .expect("last record");
    assert!(
        last["ordinal"]
            .as_u64()
            .is_some_and(|ordinal| ordinal >= 1_000)
    );
    core.shutdown().await.expect("shutdown");
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

#[tokio::test]
async fn overflow_compacts_once_then_retries_the_same_profile() {
    let (temporary, core, _) = test_core("overflow-compaction");
    core.select_model(fixture_model())
        .await
        .expect("select model");
    complete_submission(&core, "session-one").await;
    complete_submission(&core, "session-two").await;
    let mut events = core.subscribe_lossless();
    let submission = core
        .submit(Message::user("overflow-retry"))
        .await
        .expect("submit overflow request");
    let received = receive_until(&mut events, submission, |_| false).await;

    assert!(received.iter().any(|event| {
        matches!(event, CoreEvent::TextDelta { delta, .. } if delta == "overflow-recovered")
    }));
    let completed = received
        .iter()
        .filter_map(|event| match event {
            CoreEvent::CompactionChanged { compaction } if compaction.status == "completed" => {
                compaction.checkpoint.as_ref()
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(completed.len(), 1);
    assert_eq!(completed[0].trigger, "overflow");
    let file = fs::read_to_string(only_session_file(&temporary)).expect("session contents");
    assert_eq!(file.matches("\"type\":\"compaction\"").count(), 1);
    core.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn downshift_compacts_with_the_old_profile_before_selecting_the_new_model() {
    let (_temporary, core, _) = test_core("model-downshift");
    core.select_model(fixture_model())
        .await
        .expect("select large model");
    complete_submission(&core, &format!("session-downshift {}", "x".repeat(220_000))).await;
    complete_submission(&core, "session-two").await;
    complete_submission(&core, "session-three").await;
    let report = core.context_report();
    assert!(
        report.estimated_tokens > 51_200,
        "context report: {report:?}"
    );
    let history = core.history().await;
    let history_tokens = serde_json::to_string(&history)
        .expect("encode history")
        .chars()
        .count()
        .div_ceil(4);
    assert!(history_tokens > 51_200, "history tokens: {history_tokens}");
    assert_eq!(core.snapshot().selected_model, Some(fixture_model()));
    let mut events = core.subscribe_lossless();
    let smaller = ModelRef::new(ProviderId::new("fixture"), ModelId::new("fixture-model-b"));

    core.select_model(smaller.clone())
        .await
        .expect("select smaller model");

    let emitted = std::iter::from_fn(|| events.try_recv().ok()).collect::<Vec<_>>();
    let completed = emitted.iter().find_map(|event| match event {
        CoreEvent::CompactionChanged { compaction } if compaction.status == "completed" => {
            compaction.checkpoint.clone()
        }
        _ => None,
    });
    let checkpoint = completed.unwrap_or_else(|| panic!("completed downshift event: {emitted:?}"));
    assert_eq!(checkpoint.trigger, "model_downshift");
    assert_eq!(checkpoint.profile.model, fixture_model());
    assert_eq!(core.snapshot().selected_model, Some(smaller));
    core.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn compaction_is_cancellable_and_replays_a_persisted_service_checkpoint() {
    let (_temporary, core, _) = test_core("compaction-cancel");
    core.select_model(fixture_model())
        .await
        .expect("select model");
    complete_submission(&core, "session-one").await;
    complete_submission(&core, "session-two").await;
    complete_submission(&core, "session-three").await;
    let mut events = core.subscribe_lossless();
    let compacting_core = core.clone();
    let task = tokio::spawn(async move { compacting_core.compact(None).await });
    loop {
        let event = receive_event(&mut events).await;
        if matches!(
            event,
            CoreEvent::CompactionChanged { ref compaction }
                if compaction.status == "compacting" && compaction.cancellable
        ) {
            break;
        }
    }
    assert_eq!(
        core.snapshot()
            .compaction
            .as_ref()
            .map(|item| item.status.as_str()),
        Some("compacting")
    );
    assert!(core.cancel_compaction().await);
    assert!(task.await.expect("compaction task").is_err());
    assert!(core.snapshot().compaction.is_none());
    core.shutdown().await.expect("shutdown");

    let (_temporary, core, _) = test_core("compaction-replay");
    core.select_model(fixture_model())
        .await
        .expect("select model");
    complete_submission(&core, "session-one").await;
    complete_submission(&core, "session-two").await;
    complete_submission(&core, "session-three").await;
    let checkpoint = core.compact(None).await.expect("compact history");
    let id = core.current_session_id().expect("session id");
    core.new_session().expect("detach session");
    let resumed = core.resume_session(&id).expect("resume compacted session");
    assert_eq!(resumed.compactions, vec![checkpoint]);
    assert!(
        resumed
            .history
            .iter()
            .any(|entry| entry.message.content == "session-one")
    );
    core.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn thinking_only_and_atomic_profile_changes_use_distinct_session_records() {
    let (temporary, core, _) = test_core("thinking-persistence");
    core.select_model(fixture_model())
        .await
        .expect("select model");
    complete_submission(&core, "session-one").await;
    core.select_thinking(Some("low".to_owned()))
        .await
        .expect("select thinking");
    let second = ModelRef::new(ProviderId::new("fixture"), ModelId::new("fixture-model-b"));
    core.select_profile(misy_core::ModelProfile::new(
        second,
        Some("medium".to_owned()),
    ))
    .await
    .expect("select atomic profile");

    let contents = fs::read_to_string(only_session_file(&temporary)).expect("session contents");
    assert_eq!(contents.matches("\"type\":\"thinking_change\"").count(), 1);
    assert!(contents.contains(
        "\"type\":\"model_change\",\"model\":{\"provider\":\"fixture\",\"model\":\
         \"fixture-model-b\"},\"thinking\":\"medium\""
    ));
    core.shutdown().await.expect("shutdown");
}
