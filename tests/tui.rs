//! Deterministic terminal-client integration tests.

use misy::{
    MisyCore, MisyPaths, ModelId, ModelRef, ProviderId,
    tui::{
        BrowserHandoff, BrowserPlatform, TranscriptRow, TuiClient, TuiControl, UiAction, UiKey,
        UiMode, UiState, browser_command, map_input, render, validate_authorization_url,
    },
};
use ratatui::{
    Terminal,
    backend::TestBackend,
    buffer::Buffer,
    style::{Color, Modifier},
};
use serde_json::json;
use std::{
    fs,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

#[derive(Default)]
struct RecordingBrowser {
    opened: Vec<String>,
}

impl BrowserHandoff for RecordingBrowser {
    fn open(&mut self, url: &str) -> Result<(), String> {
        self.opened.push(url.to_owned());
        Ok(())
    }
}

fn write_fixture_manifest(root: &Path, id: &str, display_name: &str, target: &Path) {
    let package = root.join(id);
    fs::create_dir_all(&package).expect("package directory");
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/core_provider_fixture.sh");
    fs::write(
        package.join("misy-plugin.json"),
        format!(
            r#"{{
  "id": "{id}",
  "display_name": "{display_name}",
  "version": "1.0.0",
  "kind": "provider",
  "protocol_version": 1,
  "description": "TUI fixture",
  "author": "Misy",
  "homepage": "https://example.test/plugin",
  "repository": "https://example.test/repository",
  "license": "MIT",
  "command": "{}",
  "args": ["{}"],
  "auth_methods": [{{"id": "oauth", "display_name": "Browser OAuth"}}]
}}"#,
            fixture.display(),
            target.display(),
        ),
    )
    .expect("manifest");
}

fn core_with_providers(specifications: &[(&str, &str, &str)]) -> (tempfile::TempDir, MisyCore) {
    let temporary = tempfile::tempdir().expect("temporary root");
    let bundled = temporary.path().join("bundled");
    for (id, display_name, target) in specifications {
        write_fixture_manifest(&bundled, id, display_name, &temporary.path().join(target));
    }
    let core = MisyCore::discover(
        MisyPaths::from_root(temporary.path().join("misy")),
        &bundled,
    )
    .expect("core discovery");
    (temporary, core)
}

fn test_client() -> (tempfile::TempDir, TuiClient<RecordingBrowser>, PathBuf) {
    let (temporary, core) = core_with_providers(&[("fixture", "Fixture AI", "tool-output.txt")]);
    let target = temporary.path().join("tool-output.txt");
    (
        temporary,
        TuiClient::new(core, RecordingBrowser::default()),
        target,
    )
}

fn wait_for(
    client: &mut TuiClient<RecordingBrowser>,
    condition: impl Fn(&TuiClient<RecordingBrowser>) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !condition(client) && Instant::now() < deadline {
        client.pump_events();
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        condition(client),
        "condition did not become true before timeout"
    );
}

fn render_buffer(state: &UiState, width: u16, height: u16) -> Buffer {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| render(frame, state))
        .expect("render state");
    terminal.backend().buffer().clone()
}

fn buffer_lines(buffer: &Buffer, width: u16) -> Vec<String> {
    buffer
        .content()
        .chunks(usize::from(width))
        .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
        .collect()
}

fn authorize_first_provider(client: &mut TuiClient<RecordingBrowser>) {
    client.handle_input("/provider").expect("providers");
    client.handle_key(UiKey::Enter).expect("provider settings");
    assert_eq!(client.state().picker_labels(), ["Authorize"]);
    client.handle_key(UiKey::Enter).expect("authorize");
    wait_for(client, |client| {
        client.state().picker_labels() == ["Log out"]
    });
}

fn select_first_model(client: &mut TuiClient<RecordingBrowser>) {
    authorize_first_provider(client);
    client.handle_key(UiKey::Escape).expect("provider list");
    client.handle_key(UiKey::Escape).expect("composer");
    client.handle_input("/model").expect("models");
    wait_for(client, |client| client.state().picker_labels().len() == 2);
    client.handle_key(UiKey::Enter).expect("select model");
    wait_for(client, |client| client.state().mode() == UiMode::Input);
}

#[test]
fn input_mapping_and_reducer_keep_state_explicit() {
    assert_eq!(map_input("/provider"), Ok(UiAction::ShowProviders));
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
    assert_eq!(multiline_footer, single_line_footer + 1);
    client.insert_text("next");
    assert_eq!(client.state().composer_input(), "bc\nnext");
}

#[test]
fn slash_popup_filters_selects_and_dismisses_without_changing_text() {
    let (_temporary, mut client, _) = test_client();
    client.insert_text("/");
    assert_eq!(client.state().command_popup_rows().len(), 2);
    let cursor = client.state().composer_cursor();
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
    client.insert_text("/mo");
    client.handle_key(UiKey::Tab).expect("accept model command");
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
        core.complete_auth(&ProviderId::new(provider), json!({"code": "opaque"}))
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
    core.complete_auth(&ProviderId::new("fixture"), json!({"code": "opaque"}))
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
    core.complete_auth(&ProviderId::new("fixture"), json!({"code": "opaque"}))
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
fn active_response_stays_in_the_inline_pane_until_it_finishes() {
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
    assert!(live.iter().any(|line| line.contains('╭')));
    wait_for(&mut client, |client| {
        client.state().active_submission().is_none()
    });
    let finalized = buffer_lines(&render_buffer(client.state(), 72, 14), 72);
    assert!(!finalized.iter().any(|line| line.contains("next")));
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
    assert_eq!(client.handle_ctrl_c(), TuiControl::Exit);
    assert!(client.state().should_exit());
    assert_eq!(client.running_provider_count(), 0);
}

#[test]
fn ctrl_c_exits_from_popup_and_every_modal_surface() {
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
        assert_eq!(client.handle_ctrl_c(), TuiControl::Exit);
        assert_eq!(client.running_provider_count(), 0);
    }
}

#[test]
fn accepted_command_history_survives_a_new_client_and_restores_its_draft() {
    let (temporary, core) = core_with_providers(&[("fixture", "Fixture AI", "history-output.txt")]);
    let paths = MisyPaths::from_root(temporary.path().join("misy"));
    core.select_model(ModelRef::new(
        ProviderId::new("fixture"),
        ModelId::new("fixture-model"),
    ))
    .expect("select fixture model");
    let mut first =
        TuiClient::with_persistent_history(core.clone(), RecordingBrowser::default(), &paths);
    first.insert_text("/provider");
    first.submit_composer().expect("accepted provider command");
    first.handle_key(UiKey::Escape).expect("close providers");
    first.insert_text("session-one");
    first.submit_composer().expect("accepted prompt");
    wait_for(&mut first, |client| {
        client.state().active_submission().is_none()
    });
    drop(first);

    let mut second = TuiClient::with_persistent_history(core, RecordingBrowser::default(), &paths);
    second.insert_text("draft");
    second.handle_key(UiKey::Up).expect("previous command");
    assert_eq!(second.state().composer_input(), "session-one");
    second.handle_key(UiKey::Up).expect("older command");
    assert_eq!(second.state().composer_input(), "/provider");
    second.handle_key(UiKey::Down).expect("newer prompt");
    assert_eq!(second.state().composer_input(), "session-one");
    second.handle_key(UiKey::Down).expect("restore draft");
    assert_eq!(second.state().composer_input(), "draft");
    second.handle_ctrl_c();
}

#[test]
fn renderer_shows_transcript_status_popup_and_cursor() {
    let (_temporary, mut client, _) = test_client();
    client
        .handle_input("/unknown")
        .expect("visible command error");
    client.insert_text("/");
    let backend = TestBackend::new(60, 14);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| render(frame, client.state()))
        .expect("render");
    let rendered = format!("{:?}", terminal.backend().buffer());
    assert!(rendered.contains("model not selected"));
    assert!(rendered.contains("/provider"));
    assert!(rendered.contains("/model"));
}

#[test]
fn renderer_matches_composer_popup_and_footer_layout() {
    let (_temporary, mut client, _) = test_client();
    let empty = render_buffer(client.state(), 60, 12);
    let empty_lines = buffer_lines(&empty, 60);
    assert!(empty_lines[1].contains("> Ask anything, / for commands"));
    assert!(empty_lines[3].starts_with("  ? for shortcuts"));
    assert!(empty_lines[3].ends_with("model not selected"));
    assert!(empty_lines[0].contains('╭'));
    assert!(empty_lines[2].contains('╰'));

    client.insert_text("/mo");
    let popup = render_buffer(client.state(), 60, 12);
    let popup_lines = buffer_lines(&popup, 60);
    let composer = popup_lines
        .iter()
        .position(|line| line.contains("> /mo"))
        .expect("composer row");
    let suggestion = popup_lines
        .iter()
        .position(|line| line.contains("/model") && line.contains("Choose a model"))
        .expect("popup row");
    assert!(suggestion > composer);
    assert!(
        popup
            .content()
            .iter()
            .any(|cell| cell.bg == Color::DarkGray)
    );
    assert!(popup.content().iter().any(|cell| cell.fg == Color::Cyan));
    assert!(
        popup
            .content()
            .iter()
            .any(|cell| cell.modifier.contains(Modifier::DIM))
    );
}

#[test]
fn renderer_numbers_and_styles_provider_rows_in_eight_row_viewport() {
    let (_temporary, mut client, _) = test_client();
    authorize_first_provider(&mut client);
    client.handle_key(UiKey::Escape).expect("provider list");
    let buffer = render_buffer(client.state(), 72, 18);
    let lines = buffer_lines(&buffer, 72);
    let title = lines
        .iter()
        .position(|line| line.trim() == "Providers")
        .expect("provider title");
    let composer = lines
        .iter()
        .position(|line| line.contains("> Ask anything"))
        .expect("persistent composer");
    let row = lines
        .iter()
        .position(|line| line.contains("1.") && line.contains("Fixture AI"))
        .expect("numbered provider row");
    let footer = lines
        .iter()
        .position(|line| line.starts_with("  ? for shortcuts"))
        .expect("footer");
    assert_eq!(row, title + 2);
    assert_eq!(footer, title + 10);
    assert!(composer < title);
    assert!(lines[row].contains("✓ authenticated"));
    assert!(buffer.content().iter().any(|cell| cell.fg == Color::Green));
}

#[test]
fn renderer_shows_and_removes_the_single_line_busy_indicator() {
    let (_temporary, mut client, _) = test_client();
    select_first_model(&mut client);
    client.handle_input("block-session").expect("start work");
    let busy = buffer_lines(&render_buffer(client.state(), 72, 14), 72);
    assert!(
        busy.iter()
            .any(|line| line.contains("Working…") && line.contains("esc to interrupt"))
    );
    client.handle_key(UiKey::Escape).expect("interrupt work");
    wait_for(&mut client, |client| {
        client.state().active_submission().is_none()
    });
    let idle = buffer_lines(&render_buffer(client.state(), 72, 14), 72);
    assert!(!idle.iter().any(|line| line.contains("Working…")));
}

#[test]
fn authorization_urls_are_validated_and_windows_uses_direct_arguments() {
    let url = "https://example.test/authorize?client=misy&state=opaque";
    validate_authorization_url(url).expect("valid URL");
    assert!(validate_authorization_url("javascript:alert(1)").is_err());
    assert!(validate_authorization_url("https://example.test/a\nnext").is_err());
    let command = browser_command(BrowserPlatform::Windows, url).expect("browser command");
    assert_eq!(command.program, "rundll32");
    assert_eq!(command.args, ["url.dll,FileProtocolHandler", url]);
}

#[test]
fn bounded_event_pump_keeps_ctrl_c_responsive() {
    let (_temporary, mut client, _) = test_client();
    select_first_model(&mut client);
    client
        .handle_input("continuous-stream")
        .expect("continuous stream");
    thread::sleep(Duration::from_millis(100));
    assert_eq!(client.pump_events(), 256);
    let started = Instant::now();
    assert_eq!(client.handle_ctrl_c(), TuiControl::Exit);
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[test]
fn selected_model_remains_provider_scoped() {
    let model = ModelRef::new(ProviderId::new("openai"), ModelId::new("gpt-5"));
    assert_eq!(model.provider.as_str(), "openai");
    assert_eq!(model.model.as_str(), "gpt-5");
}
