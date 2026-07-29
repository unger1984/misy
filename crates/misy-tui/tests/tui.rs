//! Deterministic terminal-client integration tests.

mod support;

use misy_core::{MisyPaths, ModelId, ModelRef, ProviderId};
use misy_tui::{TranscriptRow, TuiClient, UiAction, UiKey, UiMode, UiState, map_input};
use ratatui::style::Color;
use serde_json::json;
use std::{fs, time::Duration};
use support::tui::{
    RecordingBrowser, authorize_first_provider, buffer_lines, core_with_providers, render_buffer,
    select_first_model, start_first_provider_auth, test_client, wait_for, wait_for_within,
};

#[tokio::test(flavor = "current_thread")]
async fn input_mapping_and_reducer_keep_state_explicit() {
    assert_eq!(map_input("/provider"), Ok(UiAction::ShowProviders));
    assert_eq!(map_input("/status"), Ok(UiAction::ShowUsage));
    assert_eq!(map_input("/usage"), Ok(UiAction::ShowUsage));
    assert_eq!(map_input("/tasks"), Ok(UiAction::ShowActivities));
    assert_eq!(map_input("/new"), Ok(UiAction::NewSession));
    assert_eq!(map_input("/resume"), Ok(UiAction::ShowSessions));
    assert_eq!(
        map_input("/resume abc123"),
        Ok(UiAction::ResumeSession("abc123".to_owned()))
    );
    assert_eq!(map_input("/exit"), Ok(UiAction::CancelAndExit));
    assert_eq!(
        map_input("/status now"),
        Err("use `/status` without arguments".to_owned())
    );
    assert!(map_input("/exit now").is_err());
    assert!(map_input("/model fixture/model").is_err());
    assert_eq!(
        misy_tui::map_key(UiMode::Input, UiKey::PageUp),
        UiAction::Noop
    );
    let mut state = UiState::default();
    state.reduce(&UiAction::CancelAndExit);
    assert!(state.should_exit());
}

#[tokio::test(flavor = "current_thread")]
async fn new_and_resume_commands_switch_persisted_conversations() {
    let (_temporary, core) =
        core_with_providers(&[("fixture", "Fixture AI", "session-command-output.txt")]);
    let mut client = TuiClient::new(core.clone(), RecordingBrowser::default()).await;
    select_first_model(&mut client).await;
    client.insert_text("session-one");
    client.submit_composer().expect("submit session message");
    wait_for(&mut client, |client| {
        let snapshot = core.snapshot();
        snapshot.active_submission.is_none()
            && snapshot.queued_submissions.is_empty()
            && client
                .state()
                .transcript()
                .iter()
                .any(|row| matches!(row, TranscriptRow::AssistantText(text) if text == "one"))
    })
    .await;
    client.pump_events();

    client.handle_input("/new").expect("start new session");
    assert!(
        !client
            .state()
            .transcript()
            .iter()
            .any(|row| matches!(row, TranscriptRow::UserPrompt(text) if text == "session-one"))
    );

    client.handle_input("/resume").expect("open session picker");
    assert_eq!(client.state().mode(), UiMode::SessionList);
    assert!(
        client
            .state()
            .picker_labels()
            .iter()
            .any(|label| label == "session-one")
    );
    client
        .handle_key(UiKey::Enter)
        .expect("resume selected session");

    assert_eq!(client.state().mode(), UiMode::Input);
    assert!(
        client
            .state()
            .transcript()
            .iter()
            .any(|row| matches!(row, TranscriptRow::UserPrompt(text) if text == "session-one"))
    );
    assert!(
        client
            .state()
            .transcript()
            .iter()
            .any(|row| matches!(row, TranscriptRow::AssistantText(text) if text == "one"))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn composer_edits_at_a_unicode_cursor_and_supports_multiline_input() {
    let (_temporary, mut client, _) = test_client().await;
    client.insert_text("aйc");
    client.handle_key(UiKey::Left).expect("left");
    client.handle_key(UiKey::Backspace).expect("backspace");
    client.insert_text("b");
    assert_eq!(client.state().composer_input(), "abc");
    assert_eq!(client.state().composer_cursor(), 2);
    client.handle_key(UiKey::Home).expect("home");
    client.handle_key(UiKey::Delete).expect("delete");
    client.handle_key(UiKey::End).expect("end");
    let single_line_footer = buffer_lines(&render_buffer(client.state(), 72, 12), 72)
        .iter()
        .position(|line| line.contains("? for shortcuts"))
        .expect("single-line footer");
    client.handle_key(UiKey::Newline).expect("newline");
    let multiline_footer = buffer_lines(&render_buffer(client.state(), 72, 12), 72)
        .iter()
        .position(|line| line.contains("? for shortcuts"))
        .expect("multiline footer");
    assert_eq!(multiline_footer, single_line_footer);
    client.insert_text("next");
    assert_eq!(client.state().composer_input(), "bc\nnext");
}

#[tokio::test(flavor = "current_thread")]
async fn tool_output_toggle_is_global_and_preserves_the_draft() {
    let (_temporary, mut client, _) = test_client().await;
    client.insert_text("unfinished draft");

    client
        .handle_key(UiKey::ToggleToolOutput)
        .expect("expand transcript output");
    assert!(client.state().tool_output_expanded());
    assert_eq!(client.state().composer_input(), "unfinished draft");

    client.handle_input("/provider").expect("open modal");
    client
        .handle_key(UiKey::ToggleToolOutput)
        .expect("collapse output behind modal");
    assert!(!client.state().tool_output_expanded());
    assert_eq!(client.state().mode(), UiMode::ProviderList);
}

#[tokio::test(flavor = "current_thread")]
async fn bracketed_paste_is_atomic_multiline_input_at_the_cursor() {
    let (_temporary, mut client, _) = test_client().await;
    client.insert_text("beforeafter");
    for _ in 0..5 {
        client.handle_key(UiKey::Left).expect("move paste cursor");
    }

    client.paste_text("one\r\ntwo\0");

    assert_eq!(client.state().composer_input(), "beforeone\ntwoafter");
    assert!(client.state().transcript().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn bracketed_image_marker_remains_plain_text() {
    let (_temporary, mut client, _) = test_client().await;

    client.paste_text("[Image #1]\nplain paste");

    assert_eq!(client.state().composer_attachment_count(), 0);
    assert_eq!(client.state().composer_input(), "[Image #1]\nplain paste");
}

#[tokio::test(flavor = "current_thread")]
async fn image_only_submission_clears_only_after_core_acceptance() {
    let (_temporary, mut client, _) = test_client().await;
    select_first_model(&mut client).await;
    client.paste_image_rgba(1, 1, vec![255, 0, 0, 255]);
    assert_eq!(client.state().composer_attachment_count(), 1);

    client.submit_composer().expect("submit image-only draft");

    assert_eq!(client.state().composer_attachment_count(), 1);
    assert_eq!(client.state().composer_input(), "[Image #1] ");
    wait_for(&mut client, |client| {
        client.state().composer_attachment_count() == 0
            && client.state().composer_input().is_empty()
    })
    .await;
    assert_eq!(client.state().history_len(), 1);
    client
        .handle_key(UiKey::Up)
        .expect("recall image-only prompt");
    assert_eq!(client.state().composer_input(), "[Image #1] ");
    assert_eq!(client.state().composer_attachment_count(), 1);
    assert!(
        client.state().transcript().iter().any(
            |row| matches!(row, TranscriptRow::UserPrompt(text) if text.contains("[Image #1]"))
        )
    );
}

#[tokio::test(flavor = "current_thread")]
async fn pending_acceptance_blocks_edits_that_would_duplicate_the_submitted_prefix() {
    let (_temporary, mut client, _) = test_client().await;
    select_first_model(&mut client).await;
    client.insert_text("session-one");
    client.submit_composer().expect("submit first prompt");

    client.insert_text(" typed too soon");
    client.paste_text(" pasted too soon");
    client.insert_newline();
    client.backspace();
    client.handle_key(UiKey::Delete).expect("blocked delete");
    client
        .handle_key(UiKey::Up)
        .expect("blocked history recall");
    client.paste_image_rgba(1, 1, vec![255, 0, 0, 255]);

    assert_eq!(client.state().composer_input(), "session-one");
    assert_eq!(client.state().composer_attachment_count(), 0);
    wait_for(&mut client, |client| {
        client.state().composer_input().is_empty()
    })
    .await;

    client.insert_text("session-two");
    client.submit_composer().expect("submit second prompt");
    wait_for(&mut client, |client| {
        client
            .state()
            .transcript()
            .iter()
            .any(|row| matches!(row, TranscriptRow::UserPrompt(text) if text == "session-two"))
    })
    .await;
    let submitted: Vec<&str> = client
        .state()
        .transcript()
        .iter()
        .filter_map(|row| match row {
            TranscriptRow::UserPrompt(text) => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(submitted, ["session-one", "session-two"]);
}

#[tokio::test(flavor = "current_thread")]
async fn persistent_history_records_only_text_from_an_image_prompt() {
    let (temporary, core) = core_with_providers(&[("fixture", "Fixture AI", "image-history.txt")]);
    let paths = MisyPaths::from_root(temporary.path().join("misy"));
    let mut client =
        TuiClient::with_persistent_history(core, RecordingBrowser::default(), &paths).await;
    select_first_model(&mut client).await;
    client.paste_image_rgba(1, 1, vec![255, 0, 0, 255]);
    client.insert_text("describe this");

    client.submit_composer().expect("submit image prompt");
    wait_for(&mut client, |client| {
        client.state().composer_input().is_empty()
    })
    .await;

    let stored = fs::read_to_string(paths.root().join("prompt-history.jsonl"))
        .expect("read persistent prompt history");
    assert!(stored.contains("describe this"));
    assert!(!stored.contains("[Image #"));
    assert!(!stored.contains("data_base64"));

    client.handle_key(UiKey::Up).expect("recall image prompt");
    assert_eq!(client.state().composer_input(), "[Image #1] describe this");
    assert_eq!(client.state().composer_attachment_count(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn image_validation_error_preserves_the_existing_draft() {
    let (_temporary, mut client, _) = test_client().await;
    select_first_model(&mut client).await;
    client.insert_text("keep this draft");

    client.paste_image_rgba(2, 2, vec![0, 0, 0, 255]);

    assert_eq!(client.state().composer_input(), "keep this draft");
    assert_eq!(client.state().composer_attachment_count(), 0);
    assert!(client.state().transcript().iter().any(|row| {
        matches!(row, TranscriptRow::Error(message) if message.contains("clipboard image"))
    }));
}

#[tokio::test(flavor = "current_thread")]
async fn capability_rejection_retains_text_and_images_for_retry() {
    let (_temporary, mut client, _) = test_client().await;
    select_first_model(&mut client).await;
    client.paste_image_rgba(1, 1, vec![255, 0, 0, 255]);
    client.insert_text("retry me");
    let draft = client.state().composer_input().to_owned();

    client.handle_input("/model").expect("open models");
    wait_for(&mut client, |client| {
        client.state().picker_labels().len() == 2
    })
    .await;
    client.handle_key(UiKey::Down).expect("select text model");
    client.handle_key(UiKey::Enter).expect("apply text model");
    wait_for(&mut client, |client| client.state().mode() == UiMode::Input).await;
    client.submit_composer().expect("submit unsupported image");
    wait_for(&mut client, |client| {
        client.state().transcript().iter().any(
            |row| matches!(row, TranscriptRow::Error(message) if message.contains("Image input")),
        )
    })
    .await;

    assert_eq!(client.state().composer_input(), draft);
    assert_eq!(client.state().composer_attachment_count(), 1);
    client.insert_text(" after rejection");
    assert!(
        client
            .state()
            .composer_input()
            .ends_with(" after rejection")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn slash_popup_filters_selects_and_dismisses_without_changing_text() {
    let (_temporary, mut client, _) = test_client().await;
    client.insert_text("/");
    assert_eq!(client.state().command_popup_rows().len(), 8);
    let cursor = client.state().composer_cursor();
    client.handle_key(UiKey::Up).expect("wrap to last command");
    assert!(client.state().command_popup_rows()[7].contains("/exit"));
    client
        .handle_key(UiKey::Down)
        .expect("wrap to first command");
    assert!(client.state().command_popup_rows()[0].contains("/provider"));
    client.handle_key(UiKey::Down).expect("select next command");
    client
        .handle_key(UiKey::Up)
        .expect("select previous command");
    assert_eq!(client.state().composer_cursor(), cursor);
    client.insert_text("mo");
    assert_eq!(client.state().command_popup_rows().len(), 1);
    assert_eq!(client.state().composer_input(), "/mo");
    client.handle_key(UiKey::Escape).expect("dismiss popup");
    assert!(!client.state().command_popup_visible());
    assert_eq!(client.state().composer_input(), "/mo");

    client.handle_key(UiKey::Home).expect("home");
    client.insert_text("x");
    assert!(!client.state().command_popup_visible());
}

#[tokio::test(flavor = "current_thread")]
async fn activities_action_opens_the_shared_tabbed_picker_at_main() {
    let (_temporary, mut client, _) = test_client().await;

    client
        .handle_key(UiKey::OpenActivities)
        .expect("open activities");

    assert_eq!(client.state().mode(), UiMode::ActivityList);
    assert_eq!(client.state().picker_labels(), ["Main"]);
    assert_eq!(
        client.state().picker_tabs(),
        [
            ("All".to_owned(), true),
            ("Agents".to_owned(), false),
            ("Tasks".to_owned(), false)
        ]
    );
    client.handle_key(UiKey::Enter).expect("return to main");
    assert_eq!(client.state().mode(), UiMode::Input);
}

#[tokio::test(flavor = "current_thread")]
async fn slash_popup_accepts_a_command_and_unknown_commands_report_errors() {
    let (_temporary, mut client, _) = test_client().await;
    client.insert_text("/");
    client
        .handle_key(UiKey::Down)
        .expect("select model command");
    client
        .handle_key(UiKey::Tab)
        .expect("complete model command");
    assert_eq!(client.state().mode(), UiMode::Input);
    assert_eq!(client.state().composer_input(), "/model ");
    assert!(!client.state().command_popup_visible());
    client
        .handle_key(UiKey::Enter)
        .expect("accept model command");
    assert_eq!(client.state().mode(), UiMode::ModelList);
    client.handle_key(UiKey::Escape).expect("cancel models");
    assert_eq!(client.state().composer_input(), "");
    client.insert_text("/unknown");
    client.submit_composer().expect("submit unknown command");
    assert!(client.state().transcript().iter().any(
        |row| matches!(row, TranscriptRow::Error(message) if message.contains("unknown command"))
    ));
    assert_eq!(client.state().composer_input(), "/unknown");
}

#[tokio::test(flavor = "current_thread")]
async fn provider_list_uses_display_names_and_never_starts_processes() {
    let (_temporary, mut client, _) = test_client().await;
    client.handle_input("/provider").expect("providers");
    assert_eq!(client.state().picker_labels(), ["Fixture AI"]);
    assert_eq!(client.running_provider_count().await, 0);
    client.handle_key(UiKey::Enter).expect("settings");
    assert_eq!(client.state().picker_labels(), ["Authorize"]);
    assert_eq!(client.running_provider_count().await, 0);
    assert!(client.browser().opened.is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn provider_settings_show_declared_method_and_logout_updates_both_levels() {
    let (_temporary, mut client, _) = test_client().await;
    authorize_first_provider(&mut client).await;
    assert_eq!(
        client.browser().opened,
        ["https://example.test/auth".to_owned()]
    );
    client.handle_key(UiKey::Enter).expect("log out");
    wait_for(&mut client, |client| {
        client.state().picker_labels() == ["Authorize"]
    })
    .await;
    client.handle_key(UiKey::Escape).expect("provider list");
    assert_eq!(client.state().picker_labels(), ["Fixture AI"]);
}

#[tokio::test(flavor = "current_thread")]
async fn browser_auth_opens_the_provider_url_and_completes() {
    let (_temporary, core) = core_with_providers(&[("fixture", "Fixture AI", "auth-browser")]);
    let mut client = TuiClient::new(core, RecordingBrowser::default()).await;

    authorize_first_provider(&mut client).await;

    assert_eq!(client.browser().opened, ["https://example.test/auth"]);
}

#[tokio::test(flavor = "current_thread")]
async fn device_auth_shows_the_code_and_opens_the_complete_url() {
    let (_temporary, core) = core_with_providers(&[("fixture", "Fixture AI", "auth-device")]);
    let mut client = TuiClient::new(core, RecordingBrowser::default()).await;

    start_first_provider_auth(&mut client).await;
    wait_for(&mut client, |client| {
        client
            .state()
            .picker_labels()
            .iter()
            .any(|label| label.contains("Code: WDJB-MJHT · waiting…"))
    })
    .await;

    assert_eq!(
        client.browser().opened,
        ["https://example.test/device?user_code=WDJB-MJHT"]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn no_auth_flow_marks_the_provider_authenticated_without_a_browser() {
    let (_temporary, core) = core_with_providers(&[("fixture", "Fixture AI", "auth-none")]);
    let mut client = TuiClient::new(core, RecordingBrowser::default()).await;

    start_first_provider_auth(&mut client).await;
    wait_for(&mut client, |client| {
        client.state().picker_labels() == ["Log out"]
    })
    .await;

    assert!(client.browser().opened.is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn prompt_auth_reports_unsupported_input_without_authenticating() {
    let (_temporary, core) = core_with_providers(&[("fixture", "Fixture AI", "auth-prompt")]);
    let mut client = TuiClient::new(core, RecordingBrowser::default()).await;

    start_first_provider_auth(&mut client).await;
    wait_for(&mut client, |client| {
        client.state().transcript().iter().any(
            |row| matches!(row, TranscriptRow::Error(message) if message.contains("not supported")),
        )
    })
    .await;

    assert_eq!(client.state().picker_labels(), ["Authorize"]);
    assert!(client.browser().opened.is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn unknown_auth_kind_reports_a_parse_error_without_panicking() {
    let (_temporary, core) = core_with_providers(&[("fixture", "Fixture AI", "auth-unknown")]);
    let mut client = TuiClient::new(core, RecordingBrowser::default()).await;

    start_first_provider_auth(&mut client).await;
    wait_for(&mut client, |client| {
        client.state().transcript().iter().any(|row| {
            matches!(
                row,
                TranscriptRow::Error(message) if message.contains("unknown kind `future`")
            )
        })
    })
    .await;

    assert_eq!(client.state().picker_labels(), ["Authorize"]);
    assert!(client.browser().opened.is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn escape_cancels_an_auth_start_and_restores_provider_actions() {
    let (_temporary, core) = core_with_providers(&[("fixture", "Fixture AI", "slow-start")]);
    let mut client = TuiClient::new(core, RecordingBrowser::default()).await;
    client.handle_input("/provider").expect("providers");
    client.handle_key(UiKey::Enter).expect("settings");
    client
        .handle_key(UiKey::Enter)
        .expect("start authorization");
    client.handle_key(UiKey::Escape).expect("cancel auth start");
    assert_eq!(client.state().mode(), UiMode::ProviderDetail);
    assert_eq!(
        client.state().picker_labels(),
        ["Cancelling authentication…"]
    );
    wait_for(&mut client, |client| {
        client.state().picker_labels() == ["Authorize"]
    })
    .await;
    let lines = buffer_lines(&render_buffer(client.state(), 72, 18), 72);
    assert!(
        lines
            .iter()
            .any(|line| line.contains("┌") && line.contains("┐"))
    );
    assert!(lines.iter().any(|line| line.contains("Fixture AI")));
    assert!(!lines.iter().any(|line| line.contains("Opening browser…")));
}

#[tokio::test(flavor = "current_thread")]
async fn escape_cancels_a_browser_auth_wait_and_restores_provider_actions() {
    let (temporary, core) = core_with_providers(&[("fixture", "Fixture AI", "hanging-auth")]);
    let mut client = TuiClient::new(core, RecordingBrowser::default()).await;

    start_first_provider_auth(&mut client).await;
    let started = temporary.path().join("hanging-auth.complete-started");
    wait_for_within(&mut client, Duration::from_secs(3), |_| started.exists()).await;
    assert_eq!(client.state().picker_labels(), ["Waiting for browser…"]);

    client
        .handle_key(UiKey::Escape)
        .expect("cancel authentication");

    assert_eq!(client.state().mode(), UiMode::ProviderDetail);
    assert_eq!(
        client.state().picker_labels(),
        ["Cancelling authentication…"]
    );
    wait_for(&mut client, |client| {
        client.state().picker_labels() == ["Authorize"]
            && client.state().transcript().iter().any(|row| {
                matches!(
                    row,
                    TranscriptRow::Info(message)
                        if message == "authentication cancelled for Fixture AI"
                )
            })
    })
    .await;
    assert_eq!(client.running_provider_count().await, 0);

    client
        .handle_key(UiKey::Enter)
        .expect("retry authorization");
    wait_for(&mut client, |client| {
        client.state().picker_labels() == ["Log out"]
    })
    .await;
    assert_eq!(client.browser().opened.len(), 2);
}

#[tokio::test(flavor = "current_thread")]
async fn both_modal_lists_filter_navigate_and_restore_the_composer_draft() {
    let (temporary, core) = core_with_providers(&[
        ("fixture", "Fixture AI", "one"),
        ("fixture-two", "Second AI", "two"),
    ]);
    let mut client = TuiClient::new(core, RecordingBrowser::default()).await;
    client.insert_text("draft");
    client.handle_key(UiKey::Left).expect("cursor left");
    let cursor = client.state().composer_cursor();
    client.handle_input("/provider").expect("providers");
    client.insert_text("Second");
    assert_eq!(client.state().picker_labels(), ["Second AI"]);
    client.handle_key(UiKey::Escape).expect("cancel providers");
    assert_eq!(client.state().composer_input(), "draft");
    assert_eq!(client.state().composer_cursor(), cursor);

    client.handle_input("/model").expect("models");
    assert_eq!(client.state().mode(), UiMode::ModelList);
    client.handle_key(UiKey::Escape).expect("cancel models");
    assert_eq!(client.state().composer_input(), "draft");
    assert_eq!(client.state().composer_cursor(), cursor);

    drop(temporary);
}

#[tokio::test(flavor = "current_thread")]
async fn numbered_modal_rows_support_direct_selection() {
    let (_temporary, core) = core_with_providers(&[
        ("fixture", "Fixture AI", "one"),
        ("fixture-two", "Second AI", "two"),
    ]);
    let mut client = TuiClient::new(core, RecordingBrowser::default()).await;
    client.handle_input("/provider").expect("providers");
    client
        .handle_key(UiKey::SelectIndex(2))
        .expect("select second provider");
    assert_eq!(client.state().mode(), UiMode::ProviderDetail);
}

#[tokio::test(flavor = "current_thread")]
async fn provider_picker_keeps_filter_and_selection_when_submission_events_arrive() {
    let (_temporary, core) = core_with_providers(&[
        ("fixture", "Fixture AI", "one"),
        ("fixture-two", "Second AI", "two"),
    ]);
    let mut client = TuiClient::new(core, RecordingBrowser::default()).await;
    select_first_model(&mut client).await;

    client.handle_input("/provider").expect("providers");
    client.insert_text("Second");
    assert_eq!(client.state().picker_labels(), ["Second AI"]);
    client.handle_input("session-one").expect("submit prompt");
    wait_for(&mut client, |client| {
        client.state().active_submission().is_none()
            && client
                .state()
                .transcript()
                .iter()
                .any(|row| matches!(row, TranscriptRow::AssistantText(_)))
    })
    .await;
    assert_eq!(client.state().mode(), UiMode::ProviderList);
    assert_eq!(client.state().picker_labels(), ["Second AI"]);

    for _ in 0.."Second".len() {
        client.backspace();
    }
    assert_eq!(client.state().picker_labels(), ["Fixture AI", "Second AI"]);
    client.handle_key(UiKey::Down).expect("select second row");
    client.handle_input("session-two").expect("submit prompt");
    wait_for(&mut client, |client| {
        client.state().active_submission().is_none()
            && client
                .state()
                .transcript()
                .iter()
                .any(|row| matches!(row, TranscriptRow::UserPrompt(text) if text == "session-two"))
    })
    .await;
    assert_eq!(client.state().picker_labels(), ["Fixture AI", "Second AI"]);
    client.handle_key(UiKey::Enter).expect("open settings");
    assert_eq!(client.state().mode(), UiMode::ProviderDetail);
    let settings = buffer_lines(&render_buffer(client.state(), 72, 14), 72);
    assert!(settings.iter().any(|line| line.contains("Second AI")));
}

#[tokio::test(flavor = "current_thread")]
async fn provider_picker_keeps_selection_when_authentication_changes() {
    let (_temporary, core) = core_with_providers(&[
        ("fixture", "Fixture AI", "one"),
        ("fixture-two", "Second AI", "two"),
    ]);
    let core_handle = core.clone();
    let mut client = TuiClient::new(core, RecordingBrowser::default()).await;
    client.handle_input("/provider").expect("providers");
    client.handle_key(UiKey::Down).expect("select second row");

    core_handle
        .complete_auth(
            &ProviderId::new("fixture"),
            json!({"id": "fixture-session"}),
            json!({"code": "opaque"}),
        )
        .await
        .expect("authenticate first provider");
    wait_for(&mut client, |client| {
        buffer_lines(&render_buffer(client.state(), 72, 14), 72)
            .iter()
            .any(|line| line.contains("✓ authenticated"))
    })
    .await;

    assert_eq!(client.state().picker_labels(), ["Fixture AI", "Second AI"]);
    client.handle_key(UiKey::Enter).expect("open settings");
    assert_eq!(client.state().mode(), UiMode::ProviderDetail);
    let settings = buffer_lines(&render_buffer(client.state(), 72, 14), 72);
    assert!(settings.iter().any(|line| line.contains("Second AI")));
}

#[tokio::test(flavor = "current_thread")]
async fn composer_input_is_preserved_while_stream_events_arrive() {
    let (_temporary, mut client, _) = test_client().await;
    select_first_model(&mut client).await;
    client.handle_input("burst").expect("submit burst");
    client.insert_text("next draft");
    wait_for(&mut client, |client| {
        client.state().active_submission().is_none()
            && client
                .state()
                .transcript()
                .iter()
                .any(|row| matches!(row, TranscriptRow::AssistantText(_)))
    })
    .await;
    assert_eq!(client.state().composer_input(), "next draft");
    assert_eq!(
        client
            .state()
            .transcript()
            .iter()
            .filter_map(|row| match row {
                TranscriptRow::AssistantText(delta) => Some(delta.len()),
                _ => None,
            })
            .sum::<usize>(),
        4_096
    );
}

#[tokio::test(flavor = "current_thread")]
async fn streamed_deltas_merge_into_one_assistant_row_per_submission() {
    let (_temporary, mut client, _) = test_client().await;
    select_first_model(&mut client).await;
    client.handle_input("burst").expect("submit burst");
    wait_for(&mut client, |client| {
        client.state().active_submission().is_none()
            && client
                .state()
                .transcript()
                .iter()
                .any(|row| matches!(row, TranscriptRow::AssistantText(text) if text.len() == 4_096))
    })
    .await;

    let assistant_rows = client
        .state()
        .transcript()
        .iter()
        .filter(|row| matches!(row, TranscriptRow::AssistantText(_)))
        .count();
    assert_eq!(assistant_rows, 1);
}

#[tokio::test(flavor = "current_thread")]
async fn tool_rows_carry_arguments_and_result_content_from_core_events() {
    let (_temporary, mut client, target) = test_client().await;
    select_first_model(&mut client).await;
    client.handle_input("tool-round-trip").expect("prompt");
    wait_for(&mut client, |client| {
        client.state().active_submission().is_none()
            && client
                .state()
                .transcript()
                .iter()
                .any(|row| matches!(row, TranscriptRow::AssistantText(text) if text == "done"))
    })
    .await;

    let path = target.display().to_string();
    let resolved_path = target
        .canonicalize()
        .expect("canonical tool target")
        .display()
        .to_string();
    let rows = client.state().transcript();
    assert!(
        rows.contains(&TranscriptRow::ToolCall {
            id: "write-1".to_owned(),
            name: "write_file".to_owned(),
            arguments: Some(format!(
                r#"{{"content":"written by tool","path":"{path}"}}"#
            )),
        }),
        "write_file call must carry its compact JSON arguments"
    );
    assert!(
        rows.contains(&TranscriptRow::ToolCall {
            id: "read-1".to_owned(),
            name: "read_file".to_owned(),
            arguments: Some(format!(r#"{{"path":"{path}"}}"#)),
        }),
        "read_file call must carry its compact JSON arguments"
    );
    assert!(
        rows.contains(&TranscriptRow::ToolResult {
            id: "write-1".to_owned(),
            is_error: false,
            content: Some(format!("wrote {resolved_path}")),
        }),
        "write_file result must retain its content"
    );
    assert!(
        rows.contains(&TranscriptRow::ToolResult {
            id: "read-1".to_owned(),
            is_error: false,
            content: Some("written by tool".to_owned()),
        }),
        "read_file result must retain its content"
    );
    assert!(
        rows.iter()
            .any(|row| matches!(row, TranscriptRow::WorkSeparator { .. })),
        "successful tool work must end with a semantic separator"
    );
    let rendered = buffer_lines(&render_buffer(client.state(), 72, 30), 72);
    assert!(rendered.iter().any(|line| line.starts_with("  ● Write ")));
    assert!(rendered.iter().any(|line| line.starts_with("  ● Read ")));
    assert!(rendered.iter().any(|line| line.contains("written by tool")));
}

#[tokio::test(flavor = "current_thread")]
async fn usage_command_reports_limits_for_the_selected_models_provider() {
    let (_temporary, core) = core_with_providers(&[
        ("fixture", "First AI", "first-usage.txt"),
        ("fixture-two", "Second AI", "second-usage.txt"),
    ]);
    let selected = ModelRef::new(
        ProviderId::new("fixture-two"),
        ModelId::new("fixture-model"),
    );
    core.complete_auth(
        &selected.provider,
        json!({"id": "fixture-session"}),
        json!({"code": "opaque"}),
    )
    .await
    .expect("authenticate selected provider");
    core.select_model(selected)
        .await
        .expect("select second provider");
    let mut client = TuiClient::new(core, RecordingBrowser::default()).await;

    client.handle_input("/status").expect("request status");
    wait_for(&mut client, |client| {
        client.state().transcript().iter().any(
            |row| matches!(row, TranscriptRow::Info(message) if message == "Usage · Second AI"),
        )
    })
    .await;

    assert!(client.state().transcript().iter().any(|row| {
        matches!(row, TranscriptRow::Info(message) if message.contains("5 hour limit: 42% used"))
    }));
}

#[tokio::test(flavor = "current_thread")]
async fn completed_response_remains_in_the_fullscreen_transcript() {
    let (_temporary, mut client, _) = test_client().await;
    select_first_model(&mut client).await;
    client.handle_input("late-next").expect("submit response");
    wait_for(&mut client, |client| {
        client
            .state()
            .transcript()
            .iter()
            .any(|row| matches!(row, TranscriptRow::AssistantText(text) if text == "next"))
    })
    .await;
    let live = buffer_lines(&render_buffer(client.state(), 72, 14), 72);
    assert!(live.iter().any(|line| line.contains("next")));
    assert!(live.iter().any(|line| line.contains("Responding…")));
    assert!(live.iter().any(|line| line.contains('╭')));
    // The fixture sleeps 2s between the "next" delta and completion; the
    // default deadline would leave only ~1s of headroom under CI load.
    wait_for_within(&mut client, Duration::from_secs(10), |client| {
        client.state().active_submission().is_none()
    })
    .await;
    assert!(!client.state().transcript().iter().any(
        |row| matches!(row, TranscriptRow::Info(message) if message == "submission completed")
    ));
    let finalized = buffer_lines(&render_buffer(client.state(), 72, 14), 72);
    assert!(finalized.iter().any(|line| line.contains("next")));
}

#[tokio::test(flavor = "current_thread")]
async fn end_to_end_auth_model_tool_and_shutdown_flow() {
    let (_temporary, mut client, target) = test_client().await;
    select_first_model(&mut client).await;
    client.handle_input("tool-round-trip").expect("prompt");
    wait_for(&mut client, |client| {
        client.state().active_submission().is_none()
            && client.state().transcript().iter().any(
                |row| matches!(row, TranscriptRow::ToolCall { name, .. } if name == "write_file"),
            )
    })
    .await;
    assert_eq!(
        fs::read_to_string(target).expect("tool output"),
        "written by tool"
    );
    assert!(
        client
            .state()
            .transcript()
            .iter()
            .any(|row| matches!(row, TranscriptRow::ToolCall { name, .. } if name == "write_file"))
    );
    client.handle_ctrl_c();
    assert!(!client.state().should_exit());
    let armed = render_buffer(client.state(), 72, 14);
    assert!(
        buffer_lines(&armed, 72)
            .iter()
            .any(|line| line.contains("press Ctrl+C again to exit"))
    );
    assert!(armed.content().iter().any(|cell| cell.fg == Color::Cyan));
    client.handle_ctrl_c();
    assert!(client.state().should_exit());
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    assert_eq!(client.running_provider_count().await, 0);
}

#[tokio::test(flavor = "current_thread")]
async fn exit_command_shuts_down_providers_and_requests_terminal_exit() {
    let (_temporary, mut client, _) = test_client().await;
    select_first_model(&mut client).await;
    client.handle_input("/exit").expect("exit command");
    assert!(client.state().should_exit());
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    assert_eq!(client.running_provider_count().await, 0);
}

#[tokio::test(flavor = "current_thread")]
async fn ctrl_c_requires_a_second_press_from_popup_and_every_modal_surface() {
    for setup in ["popup", "providers", "settings", "models"] {
        let (_temporary, mut client, _) = test_client().await;
        match setup {
            "popup" => client.insert_text("/"),
            "providers" => {
                client.handle_input("/provider").expect("providers");
            }
            "settings" => {
                client.handle_input("/provider").expect("providers");
                client.handle_key(UiKey::Enter).expect("settings");
            }
            "models" => {
                client.handle_input("/model").expect("models");
            }
            _ => unreachable!(),
        }
        client.handle_ctrl_c();
        assert!(!client.state().should_exit(), "{setup}: first press arms");
        client.handle_ctrl_c();
        assert!(client.state().should_exit(), "{setup}: second press exits");
        assert_eq!(client.running_provider_count().await, 0);
    }
}
