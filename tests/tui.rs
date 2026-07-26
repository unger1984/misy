//! Deterministic terminal-client integration tests.

mod support;

use misy::{
    ModelId, ModelRef, ProviderId,
    tui::{TranscriptRow, TuiClient, TuiControl, UiAction, UiKey, UiMode, UiState, map_input},
};
use ratatui::style::Color;
use serde_json::json;
use std::{fs, thread, time::Duration};
use support::tui::{
    RecordingBrowser, authorize_first_provider, buffer_lines, core_with_providers, render_buffer,
    select_first_model, start_first_provider_auth, test_client, wait_for,
};

#[test]
fn input_mapping_and_reducer_keep_state_explicit() {
    assert_eq!(map_input("/provider"), Ok(UiAction::ShowProviders));
    assert_eq!(map_input("/usage"), Ok(UiAction::ShowUsage));
    assert_eq!(map_input("/exit"), Ok(UiAction::CancelAndExit));
    assert!(map_input("/exit now").is_err());
    assert!(map_input("/model fixture/model").is_err());
    assert_eq!(
        misy::tui::map_key(UiMode::Input, UiKey::PageUp),
        UiAction::Noop
    );
    let mut state = UiState::default();
    state.reduce(UiAction::AppendAssistantText("hello".to_owned()));
    assert_eq!(
        state.transcript(),
        [TranscriptRow::AssistantText("hello".to_owned())]
    );
}

#[test]
fn composer_edits_at_a_unicode_cursor_and_supports_multiline_input() {
    let (_temporary, mut client, _) = test_client();
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

#[test]
fn bracketed_paste_is_atomic_multiline_input_at_the_cursor() {
    let (_temporary, mut client, _) = test_client();
    client.insert_text("beforeafter");
    for _ in 0..5 {
        client.handle_key(UiKey::Left).expect("move paste cursor");
    }

    client.paste_text("one\r\ntwo\0");

    assert_eq!(client.state().composer_input(), "beforeone\ntwoafter");
    assert!(client.state().transcript().is_empty());
}

#[test]
fn slash_popup_filters_selects_and_dismisses_without_changing_text() {
    let (_temporary, mut client, _) = test_client();
    client.insert_text("/");
    assert_eq!(client.state().command_popup_rows().len(), 4);
    let cursor = client.state().composer_cursor();
    client.handle_key(UiKey::Up).expect("wrap to last command");
    assert!(client.state().command_popup_rows()[3].contains("/exit"));
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

#[test]
fn slash_popup_accepts_a_command_and_unknown_commands_report_errors() {
    let (_temporary, mut client, _) = test_client();
    client.insert_text("/");
    client
        .handle_key(UiKey::Down)
        .expect("select model command");
    assert_eq!(
        client
            .handle_key(UiKey::Tab)
            .expect("complete model command"),
        TuiControl::Continue
    );
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
}

#[test]
fn provider_list_uses_display_names_and_never_starts_processes() {
    let (_temporary, mut client, _) = test_client();
    client.handle_input("/provider").expect("providers");
    assert_eq!(client.state().picker_labels(), ["Fixture AI"]);
    assert_eq!(client.running_provider_count(), 0);
    client.handle_key(UiKey::Enter).expect("settings");
    assert_eq!(client.state().picker_labels(), ["Authorize"]);
    assert_eq!(client.running_provider_count(), 0);
    assert!(client.browser().opened.is_empty());
}

#[test]
fn provider_settings_show_declared_method_and_logout_updates_both_levels() {
    let (_temporary, mut client, _) = test_client();
    authorize_first_provider(&mut client);
    assert_eq!(
        client.browser().opened,
        ["https://example.test/auth".to_owned()]
    );
    client.handle_key(UiKey::Enter).expect("log out");
    wait_for(&mut client, |client| {
        client.state().picker_labels() == ["Authorize"]
    });
    client.handle_key(UiKey::Escape).expect("provider list");
    assert_eq!(client.state().picker_labels(), ["Fixture AI"]);
}

#[test]
fn browser_auth_opens_the_provider_url_and_completes() {
    let (_temporary, core) = core_with_providers(&[("fixture", "Fixture AI", "auth-browser")]);
    let mut client = TuiClient::new(core, RecordingBrowser::default());

    authorize_first_provider(&mut client);

    assert_eq!(client.browser().opened, ["https://example.test/auth"]);
}

#[test]
fn device_auth_shows_the_code_and_opens_the_complete_url() {
    let (_temporary, core) = core_with_providers(&[("fixture", "Fixture AI", "auth-device")]);
    let mut client = TuiClient::new(core, RecordingBrowser::default());

    start_first_provider_auth(&mut client);
    wait_for(&mut client, |client| {
        client
            .state()
            .picker_labels()
            .iter()
            .any(|label| label.contains("Code: WDJB-MJHT · waiting…"))
    });

    assert_eq!(
        client.browser().opened,
        ["https://example.test/device?user_code=WDJB-MJHT"]
    );
}

#[test]
fn no_auth_flow_marks_the_provider_authenticated_without_a_browser() {
    let (_temporary, core) = core_with_providers(&[("fixture", "Fixture AI", "auth-none")]);
    let mut client = TuiClient::new(core, RecordingBrowser::default());

    start_first_provider_auth(&mut client);
    wait_for(&mut client, |client| {
        client.state().picker_labels() == ["Log out"]
    });

    assert!(client.browser().opened.is_empty());
}

#[test]
fn prompt_auth_reports_unsupported_input_without_authenticating() {
    let (_temporary, core) = core_with_providers(&[("fixture", "Fixture AI", "auth-prompt")]);
    let mut client = TuiClient::new(core, RecordingBrowser::default());

    start_first_provider_auth(&mut client);
    wait_for(&mut client, |client| {
        client.state().transcript().iter().any(
            |row| matches!(row, TranscriptRow::Error(message) if message.contains("not supported")),
        )
    });

    assert_eq!(client.state().picker_labels(), ["Authorize"]);
    assert!(client.browser().opened.is_empty());
}

#[test]
fn unknown_auth_kind_reports_a_parse_error_without_panicking() {
    let (_temporary, core) = core_with_providers(&[("fixture", "Fixture AI", "auth-unknown")]);
    let mut client = TuiClient::new(core, RecordingBrowser::default());

    start_first_provider_auth(&mut client);
    wait_for(&mut client, |client| {
        client.state().transcript().iter().any(|row| {
            matches!(
                row,
                TranscriptRow::Error(message) if message.contains("unknown kind `future`")
            )
        })
    });

    assert_eq!(client.state().picker_labels(), ["Authorize"]);
    assert!(client.browser().opened.is_empty());
}

#[test]
fn provider_settings_cannot_abandon_an_auth_start_with_escape() {
    let (_temporary, core) = core_with_providers(&[("fixture", "Fixture AI", "slow-start")]);
    let mut client = TuiClient::new(core, RecordingBrowser::default());
    client.handle_input("/provider").expect("providers");
    client.handle_key(UiKey::Enter).expect("settings");
    client
        .handle_key(UiKey::Enter)
        .expect("start authorization");
    client.handle_key(UiKey::Escape).expect("escape is ignored");
    assert_eq!(client.state().mode(), UiMode::ProviderDetail);
    assert_eq!(client.state().picker_labels(), ["Opening browser…"]);
    client.handle_ctrl_c();
}

#[test]
fn both_modal_lists_filter_navigate_and_restore_the_composer_draft() {
    let (temporary, core) = core_with_providers(&[
        ("fixture", "Fixture AI", "one"),
        ("fixture-two", "Second AI", "two"),
    ]);
    let mut client = TuiClient::new(core, RecordingBrowser::default());
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

#[test]
fn numbered_modal_rows_support_direct_selection() {
    let (_temporary, core) = core_with_providers(&[
        ("fixture", "Fixture AI", "one"),
        ("fixture-two", "Second AI", "two"),
    ]);
    let mut client = TuiClient::new(core, RecordingBrowser::default());
    client.handle_input("/provider").expect("providers");
    client
        .handle_key(UiKey::SelectIndex(2))
        .expect("select second provider");
    assert_eq!(client.state().mode(), UiMode::ProviderDetail);
}

#[test]
fn model_picker_shows_partial_results_and_skips_unconfigured_provider() {
    let (_temporary, core) = core_with_providers(&[
        ("fixture", "Working AI", "good-models"),
        ("failed-provider", "Broken AI", "bad-models"),
        ("unconfigured", "Unused AI", "unused-models"),
    ]);
    for provider in ["fixture", "failed-provider"] {
        core.complete_auth(
            &ProviderId::new(provider),
            json!({"id": "fixture-session"}),
            json!({"code": "opaque"}),
        )
        .expect("store credentials");
    }
    let mut client = TuiClient::new(core, RecordingBrowser::default());
    client.handle_input("/model").expect("models");
    wait_for(&mut client, |client| {
        client.state().picker_labels().len() == 3
    });
    let labels = client.state().picker_labels();
    assert!(labels.iter().any(|label| label == "fixture-model"));
    assert!(
        labels
            .iter()
            .any(|label| label.contains("Broken AI — error"))
    );
    assert_eq!(client.running_provider_count(), 2);
    client.handle_ctrl_c();
}

#[test]
fn model_picker_filters_and_confirms_with_a_transcript_message() {
    let (_temporary, mut client, _) = test_client();
    authorize_first_provider(&mut client);
    client.handle_key(UiKey::Escape).expect("provider list");
    client.handle_key(UiKey::Escape).expect("composer");
    client.handle_input("/model").expect("models");
    wait_for(&mut client, |client| {
        client.state().picker_labels().len() == 2
    });
    client.insert_text("model-b");
    assert_eq!(client.state().picker_labels().len(), 1);
    client
        .handle_key(UiKey::Enter)
        .expect("select filtered model");
    wait_for(&mut client, |client| client.state().mode() == UiMode::Input);
    assert!(client.state().transcript().iter().any(
        |row| matches!(row, TranscriptRow::Info(message) if message == "model: fixture-model-b")
    ));
}

#[test]
fn model_selection_cannot_be_abandoned_while_it_is_persisting() {
    let (_temporary, core) = core_with_providers(&[("fixture", "Fixture AI", "slow-models")]);
    core.complete_auth(
        &ProviderId::new("fixture"),
        json!({"id": "fixture-session"}),
        json!({"code": "opaque"}),
    )
    .expect("store credentials");
    let mut client = TuiClient::new(core, RecordingBrowser::default());
    client.handle_input("/model").expect("models");
    wait_for(&mut client, |client| {
        client.state().picker_labels().len() == 2
    });
    client.handle_key(UiKey::Enter).expect("start selection");
    client.handle_key(UiKey::Escape).expect("escape is ignored");
    assert_eq!(client.state().mode(), UiMode::ModelList);
    thread::sleep(Duration::from_millis(1_100));
    client.pump_events();
    assert_eq!(
        client.state().selected_model(),
        Some(ModelRef::new(
            ProviderId::new("fixture"),
            ModelId::new("fixture-model")
        ))
    );
    assert!(
        client.state().transcript().iter().any(
            |row| matches!(row, TranscriptRow::Info(message) if message.starts_with("model:"))
        )
    );
}

#[test]
fn cancelled_model_loading_ignores_its_late_error() {
    let (_temporary, core) = core_with_providers(&[("fixture", "Broken AI", "slow-bad-models")]);
    core.complete_auth(
        &ProviderId::new("fixture"),
        json!({"id": "fixture-session"}),
        json!({"code": "opaque"}),
    )
    .expect("store credentials");
    let mut client = TuiClient::new(core, RecordingBrowser::default());
    client.handle_input("/model").expect("models");
    client.handle_key(UiKey::Escape).expect("cancel loading");
    thread::sleep(Duration::from_millis(1_100));
    client.pump_events();
    assert!(client.state().transcript().is_empty());
}

#[test]
fn composer_input_is_preserved_while_stream_events_arrive() {
    let (_temporary, mut client, _) = test_client();
    select_first_model(&mut client);
    client.handle_input("burst").expect("submit burst");
    client.insert_text("next draft");
    wait_for(&mut client, |client| {
        client.state().active_submission().is_none()
    });
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

#[test]
fn usage_command_reports_limits_for_the_selected_models_provider() {
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
    .expect("authenticate selected provider");
    core.select_model(selected).expect("select second provider");
    let mut client = TuiClient::new(core, RecordingBrowser::default());

    client.handle_input("/usage").expect("request usage");
    wait_for(&mut client, |client| {
        client.state().transcript().iter().any(
            |row| matches!(row, TranscriptRow::Info(message) if message == "Usage · Second AI"),
        )
    });

    assert!(client.state().transcript().iter().any(
        |row| matches!(row, TranscriptRow::Info(message) if message.contains("5 hour limit: 42% used"))
    ));
}

#[test]
fn completed_response_remains_in_the_fullscreen_transcript() {
    let (_temporary, mut client, _) = test_client();
    select_first_model(&mut client);
    client.handle_input("late-next").expect("submit response");
    wait_for(&mut client, |client| {
        client
            .state()
            .transcript()
            .iter()
            .any(|row| matches!(row, TranscriptRow::AssistantText(text) if text == "next"))
    });
    let live = buffer_lines(&render_buffer(client.state(), 72, 14), 72);
    assert!(live.iter().any(|line| line.contains("next")));
    assert!(live.iter().any(|line| line.contains("Responding…")));
    assert!(live.iter().any(|line| line.contains('╭')));
    wait_for(&mut client, |client| {
        client.state().active_submission().is_none()
    });
    assert!(!client.state().transcript().iter().any(
        |row| matches!(row, TranscriptRow::Info(message) if message == "submission completed")
    ));
    let finalized = buffer_lines(&render_buffer(client.state(), 72, 14), 72);
    assert!(finalized.iter().any(|line| line.contains("next")));
}

#[test]
fn end_to_end_auth_model_tool_and_shutdown_flow() {
    let (_temporary, mut client, target) = test_client();
    select_first_model(&mut client);
    client.handle_input("tool-round-trip").expect("prompt");
    wait_for(&mut client, |client| {
        client.state().active_submission().is_none()
    });
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
    assert_eq!(client.handle_ctrl_c(), TuiControl::Continue);
    let armed = render_buffer(client.state(), 72, 14);
    assert!(
        buffer_lines(&armed, 72)
            .iter()
            .any(|line| line.contains("press Ctrl+C again to exit"))
    );
    assert!(armed.content().iter().any(|cell| cell.fg == Color::Cyan));
    assert_eq!(client.handle_ctrl_c(), TuiControl::Exit);
    assert!(client.state().should_exit());
    assert_eq!(client.running_provider_count(), 0);
}

#[test]
fn exit_command_shuts_down_providers_and_requests_terminal_exit() {
    let (_temporary, mut client, _) = test_client();
    select_first_model(&mut client);
    assert_eq!(
        client.handle_input("/exit").expect("exit command"),
        TuiControl::Exit
    );
    assert!(client.state().should_exit());
    assert_eq!(client.running_provider_count(), 0);
}

#[test]
fn ctrl_c_requires_a_second_press_from_popup_and_every_modal_surface() {
    for setup in ["popup", "providers", "settings", "models"] {
        let (_temporary, mut client, _) = test_client();
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
        assert_eq!(client.handle_ctrl_c(), TuiControl::Continue);
        assert_eq!(client.handle_ctrl_c(), TuiControl::Exit);
        assert_eq!(client.running_provider_count(), 0);
    }
}
