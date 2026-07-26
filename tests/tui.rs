use misy::{
    MisyCore, MisyPaths, ModelId, ModelRef, ProviderId,
    tui::{
        BrowserHandoff, BrowserPlatform, TranscriptRow, TuiClient, TuiControl, UiAction, UiKey,
        UiMode, UiState, browser_command, map_input, render, validate_authorization_url,
    },
};
use ratatui::{Terminal, backend::TestBackend};
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

fn write_fixture_manifest(root: &Path, id: &str, fixture: &Path, target: &Path) {
    let package = root.join(id);
    fs::create_dir_all(&package).expect("package directory");
    fs::write(
        package.join("misy-plugin.json"),
        format!(
            r#"{{
  "id": "{id}",
  "version": "1.0.0",
  "kind": "provider",
  "protocol_version": 1,
  "description": "TUI fixture",
  "author": "Misy",
  "homepage": "https://example.test/plugin",
  "repository": "https://example.test/repository",
  "license": "MIT",
  "command": "{}",
  "args": ["{}"]
}}"#,
            fixture.display(),
            target.display(),
        ),
    )
    .expect("manifest");
}

fn test_client() -> (tempfile::TempDir, TuiClient<RecordingBrowser>, PathBuf) {
    let temporary = tempfile::tempdir().expect("temporary root");
    let bundled = temporary.path().join("bundled");
    let target = temporary.path().join("tool-output.txt");
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/core_provider_fixture.sh");
    write_fixture_manifest(&bundled, "fixture", &fixture, &target);
    let core = MisyCore::discover(
        MisyPaths::from_root(temporary.path().join("misy")),
        &bundled,
    )
    .expect("core discovery");
    (
        temporary,
        TuiClient::new(core, RecordingBrowser::default()),
        target,
    )
}

fn select_first_model(client: &mut TuiClient<RecordingBrowser>) {
    client.handle_input("/model").expect("open model picker");
    client.handle_key(UiKey::Enter).expect("select first model");
}

#[test]
fn input_mapping_and_reducer_keep_rendering_state_explicit() {
    assert!(
        map_input("/provider fixture auth")
            .unwrap_err()
            .contains("/provider")
    );
    assert!(
        map_input(r#"/provider fixture complete {"code":"x"}"#)
            .unwrap_err()
            .contains("/provider")
    );
    assert!(
        map_input("/model fixture/fixture-model")
            .unwrap_err()
            .contains("/model")
    );
    assert_eq!(
        misy::tui::map_key(UiMode::Input, UiKey::Up),
        UiAction::HistoryPrevious
    );
    assert_eq!(
        misy::tui::map_key(UiMode::Input, UiKey::PageUp),
        UiAction::ScrollUp
    );
    assert_eq!(
        misy::tui::map_key(UiMode::ProviderList, UiKey::Up),
        UiAction::PickerUp
    );

    let mut state = UiState::default();
    state.reduce(UiAction::AppendAssistantText("hello".to_owned()));
    state.reduce(UiAction::AppendToolCall {
        id: "call-1".to_owned(),
        name: "read_file".to_owned(),
    });
    state.reduce(UiAction::ScrollUp);

    assert_eq!(state.scroll_offset(), 1);
    assert_eq!(
        state.transcript(),
        &[
            TranscriptRow::AssistantText("hello".to_owned()),
            TranscriptRow::ToolCall {
                id: "call-1".to_owned(),
                name: "read_file".to_owned(),
            },
        ]
    );
}

#[test]
fn model_picker_merges_models_from_two_providers_without_overwrite() {
    let temporary = tempfile::tempdir().expect("temporary root");
    let bundled = temporary.path().join("bundled");
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/core_provider_fixture.sh");
    write_fixture_manifest(&bundled, "fixture", &fixture, &temporary.path().join("one"));
    write_fixture_manifest(
        &bundled,
        "fixture-two",
        &fixture,
        &temporary.path().join("two"),
    );
    let core = MisyCore::discover(
        MisyPaths::from_root(temporary.path().join("misy")),
        &bundled,
    )
    .expect("core");
    let mut client = TuiClient::new(core, RecordingBrowser::default());

    client.handle_input("/model").expect("model picker");
    client.pump_events();

    assert_eq!(client.state().picker_labels().len(), 4);
    assert_eq!(client.state().mode(), UiMode::ModelList);
    client.handle_ctrl_c();
}

#[test]
fn tui_client_runs_the_configure_authenticate_tool_and_shutdown_flow() {
    let (_temporary, mut client, target) = test_client();

    client.handle_input("/provider").expect("list providers");
    assert_eq!(client.state().mode(), UiMode::ProviderList);
    assert!(client.state().transcript().is_empty());
    assert!(client.browser().opened.is_empty());
    assert_eq!(client.running_provider_count(), 0);
    client.handle_key(UiKey::Enter).expect("provider detail");
    assert_eq!(client.state().mode(), UiMode::ProviderDetail);
    assert!(client.browser().opened.is_empty());
    assert_eq!(client.state().picker_labels(), ["Checking status…"]);
    let status_deadline = Instant::now() + Duration::from_secs(2);
    while client.state().picker_labels() != ["Authorize"] && Instant::now() < status_deadline {
        client.pump_events();
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(client.running_provider_count(), 1);
    assert_eq!(client.state().picker_labels(), ["Authorize"]);
    client.handle_key(UiKey::Enter).expect("start auth");
    assert_eq!(client.state().picker_labels(), ["Starting authorization…"]);
    let auth_deadline = Instant::now() + Duration::from_secs(2);
    while client.state().picker_labels() != ["Log out"] && Instant::now() < auth_deadline {
        client.pump_events();
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        client.browser().opened,
        vec!["https://example.test/auth".to_owned()]
    );
    assert_eq!(client.state().picker_labels(), ["Log out"]);
    assert_eq!(
        client.state().selected_model(),
        Some(ModelRef::new(
            ProviderId::new("fixture"),
            ModelId::new("fixture-model")
        ))
    );
    client.handle_key(UiKey::Enter).expect("log out");
    client.pump_events();
    assert_eq!(client.state().picker_labels(), ["Authorize"]);
    client.handle_key(UiKey::Escape).expect("provider list");
    client.handle_key(UiKey::Escape).expect("normal input");

    client.handle_input("/model").expect("list models");
    assert_eq!(client.state().mode(), UiMode::ModelList);
    assert_eq!(client.state().picker_labels().len(), 2);
    assert!(client.state().transcript().iter().all(|row| !matches!(
        row,
        TranscriptRow::Provider { .. } | TranscriptRow::Model { .. }
    )));
    client
        .handle_key(UiKey::Down)
        .expect("highlight next model");
    assert_eq!(client.state().highlighted_index(), 1);
    client.handle_key(UiKey::Up).expect("highlight first model");
    assert_eq!(client.state().highlighted_index(), 0);
    client.handle_key(UiKey::Enter).expect("select model");
    assert_eq!(client.state().mode(), UiMode::Input);
    client.pump_events();
    assert_eq!(
        client.state().selected_model(),
        Some(ModelRef::new(
            ProviderId::new("fixture"),
            ModelId::new("fixture-model")
        ))
    );
    client
        .handle_input("tool-round-trip")
        .expect("submit prompt");

    let deadline = Instant::now() + Duration::from_secs(3);
    while client.state().active_submission().is_some() && Instant::now() < deadline {
        client.pump_events();
        thread::sleep(Duration::from_millis(10));
    }
    client.pump_events();
    assert!(client.state().active_submission().is_none());
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
    assert!(client
        .state()
        .transcript()
        .iter()
        .any(|row| matches!(row, TranscriptRow::AssistantText(text) if text.contains("writing") || text.contains("done"))));

    assert_eq!(client.handle_ctrl_c(), TuiControl::Exit);
    assert!(client.state().should_exit());
}

#[test]
fn slow_provider_detail_status_does_not_block_ctrl_c() {
    let temporary = tempfile::tempdir().expect("temporary root");
    let bundled = temporary.path().join("bundled");
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/core_provider_fixture.sh");
    write_fixture_manifest(
        &bundled,
        "fixture",
        &fixture,
        &temporary.path().join("slow-status"),
    );
    let core = MisyCore::discover(
        MisyPaths::from_root(temporary.path().join("misy")),
        &bundled,
    )
    .expect("core");
    let mut client = TuiClient::new(core, RecordingBrowser::default());
    client.handle_input("/provider").expect("provider picker");

    let started = Instant::now();
    client.handle_key(UiKey::Enter).expect("start status check");
    assert!(started.elapsed() < Duration::from_millis(200));
    assert_eq!(client.state().picker_labels(), ["Checking status…"]);
    assert_eq!(client.handle_ctrl_c(), TuiControl::Exit);
    assert!(client.state().should_exit());
}

#[test]
fn slow_auth_start_does_not_block_ctrl_c() {
    let temporary = tempfile::tempdir().expect("temporary root");
    let bundled = temporary.path().join("bundled");
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/core_provider_fixture.sh");
    write_fixture_manifest(
        &bundled,
        "fixture",
        &fixture,
        &temporary.path().join("slow-start"),
    );
    let core = MisyCore::discover(
        MisyPaths::from_root(temporary.path().join("misy")),
        &bundled,
    )
    .expect("core");
    let mut client = TuiClient::new(core, RecordingBrowser::default());
    client.handle_input("/provider").expect("provider picker");
    client.handle_key(UiKey::Enter).expect("start status check");
    let status_deadline = Instant::now() + Duration::from_secs(1);
    while client.state().picker_labels() != ["Authorize"] && Instant::now() < status_deadline {
        client.pump_events();
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(client.state().picker_labels(), ["Authorize"]);

    let started = Instant::now();
    client
        .handle_key(UiKey::Enter)
        .expect("start authorization");
    assert!(started.elapsed() < Duration::from_millis(200));
    assert_eq!(client.state().picker_labels(), ["Starting authorization…"]);
    assert_eq!(client.handle_ctrl_c(), TuiControl::Exit);
    assert!(client.state().should_exit());
}

#[test]
fn picker_escape_navigation_never_triggers_provider_side_effects() {
    let (_temporary, mut client, _) = test_client();

    client.handle_input("/provider").expect("provider picker");
    client.handle_key(UiKey::Enter).expect("provider detail");
    assert!(client.browser().opened.is_empty());
    client.handle_key(UiKey::Escape).expect("provider list");
    client.handle_key(UiKey::Escape).expect("normal input");

    assert_eq!(client.state().mode(), UiMode::Input);
    assert!(client.browser().opened.is_empty());
}

#[test]
fn ctrl_c_cancels_an_active_submission_and_exits_even_when_provider_is_busy() {
    let (_temporary, mut client, _) = test_client();
    select_first_model(&mut client);
    client.handle_input("cancel-me").expect("submit prompt");

    assert_eq!(client.handle_ctrl_c(), TuiControl::Exit);
    assert!(client.state().should_exit());
}

#[test]
fn client_records_picker_help_for_obsolete_commands_in_the_transcript() {
    let (_temporary, mut client, _) = test_client();

    client
        .handle_input("/provider missing auth")
        .expect("obsolete form reports inline help");

    assert!(
        client
            .state()
            .transcript()
            .iter()
            .any(|row| matches!(row, TranscriptRow::Error(message) if message.contains("/provider") && message.contains("picker")))
    );
}

#[test]
fn tui_client_keeps_burst_streams_and_their_terminal_event() {
    let (_temporary, mut client, _) = test_client();
    select_first_model(&mut client);
    client.handle_input("burst").expect("submit burst");

    thread::sleep(Duration::from_millis(250));
    let deadline = Instant::now() + Duration::from_secs(3);
    while client.state().active_submission().is_some() && Instant::now() < deadline {
        client.pump_events();
    }

    assert!(client.state().active_submission().is_none());
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
fn renderer_smoke_shows_startup_identity_status_separator_and_input() {
    let mut state = UiState::default();
    state.reduce(UiAction::AppendAssistantText("answer".to_owned()));
    let backend = TestBackend::new(52, 16);
    let mut terminal = Terminal::new(backend).expect("test terminal");

    terminal
        .draw(|frame| render(frame, &state))
        .expect("render state");

    let buffer = terminal.backend().buffer();
    let rows = (0..16)
        .map(|row| {
            (0..52)
                .map(|column| buffer[(column, row)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>();
    let rendered = rows.join("\n");
    assert!(rendered.contains("MISY"));
    assert!(rendered.contains("agent: misy"));
    assert!(rendered.contains("provider: none"));
    assert!(rendered.contains("model: none"));
    let answer_row = rows
        .iter()
        .position(|row| row.trim() == "answer")
        .expect("transcript row");
    let separator_row = rows
        .iter()
        .position(|row| row.starts_with("────────────────────"))
        .expect("separator row");
    assert_eq!(separator_row, answer_row + 1);
    assert!(rows[separator_row + 1].starts_with("› "));
    terminal
        .backend_mut()
        .assert_cursor_position((2, u16::try_from(separator_row + 1).unwrap()));
    assert!(
        rows[(separator_row + 2)..]
            .iter()
            .all(|row| row.trim().is_empty())
    );
}

#[test]
fn composer_history_navigates_and_restores_the_original_draft() {
    let (_temporary, mut client, _) = test_client();

    client.insert_text("/first");
    client.submit_composer().expect("submit first command");
    client.insert_text("/second");
    client.submit_composer().expect("submit second command");
    client.insert_text("draft");

    client.handle_key(UiKey::Up).expect("older input");
    assert_eq!(client.state().composer_input(), "/second");
    client.handle_key(UiKey::Up).expect("oldest input");
    assert_eq!(client.state().composer_input(), "/first");
    client.handle_key(UiKey::Down).expect("newer input");
    assert_eq!(client.state().composer_input(), "/second");
    client.handle_key(UiKey::Down).expect("restore draft");
    assert_eq!(client.state().composer_input(), "draft");

    client.handle_key(UiKey::Up).expect("recall editable input");
    client.insert_text("!");
    assert_eq!(client.state().composer_input(), "/second!");
}

#[test]
fn composer_history_is_bounded_and_skips_consecutive_duplicates() {
    let (_temporary, mut client, _) = test_client();
    for index in 0..105 {
        client.insert_text(&format!("/command-{index}"));
        client.submit_composer().expect("submit command");
    }
    client.insert_text("/command-104");
    client.submit_composer().expect("submit duplicate");

    assert_eq!(client.state().history_len(), 100);
}

#[test]
fn authorization_handoff_accepts_web_urls_and_rejects_non_web_locations() {
    validate_authorization_url("https://example.test/authorize?client=misy&state=opaque")
        .expect("web authorization URL");
    assert!(validate_authorization_url("javascript:alert(1)").is_err());
    assert!(validate_authorization_url("https://example.test/authorize\nnext").is_err());
}

#[test]
fn windows_browser_command_passes_the_url_as_a_direct_argument() {
    let url = "https://example.test/authorize?client=misy&state=opaque";
    let command = browser_command(BrowserPlatform::Windows, url).expect("browser command");

    assert_eq!(command.program, "rundll32");
    assert_eq!(command.args, ["url.dll,FileProtocolHandler", url]);
}

#[test]
fn event_pump_is_bounded_so_continuous_streaming_cannot_block_ctrl_c() {
    let (_temporary, mut client, _) = test_client();
    select_first_model(&mut client);
    client
        .handle_input("continuous-stream")
        .expect("continuous submission");
    thread::sleep(Duration::from_millis(100));

    assert_eq!(client.pump_events(), 256);
    let started = Instant::now();
    assert_eq!(client.handle_ctrl_c(), TuiControl::Exit);
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(client.state().should_exit());
}
