use misy::{
    MisyCore, MisyPaths, ModelId, ModelRef, ProviderId,
    tui::{
        BrowserHandoff, TranscriptRow, TuiClient, TuiControl, UiAction, UiState, map_input, render,
        validate_authorization_url,
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

fn write_fixture_manifest(root: &Path, fixture: &Path, target: &Path) {
    let package = root.join("fixture");
    fs::create_dir_all(&package).expect("package directory");
    fs::write(
        package.join("misy-plugin.json"),
        format!(
            r#"{{
  "id": "fixture",
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
    write_fixture_manifest(&bundled, &fixture, &target);
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

#[test]
fn input_mapping_and_reducer_keep_rendering_state_explicit() {
    assert_eq!(
        map_input("/provider fixture auth").expect("provider auth command"),
        UiAction::StartAuth(ProviderId::new("fixture"))
    );
    assert_eq!(
        map_input("/model fixture/fixture-model").expect("model command"),
        UiAction::SelectModel(ModelRef::new(
            ProviderId::new("fixture"),
            ModelId::new("fixture-model"),
        ))
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
fn tui_client_runs_the_configure_authenticate_tool_and_shutdown_flow() {
    let (_temporary, mut client, target) = test_client();

    client.handle_input("/provider").expect("list providers");
    assert!(client.state().transcript().iter().any(
        |row| matches!(row, TranscriptRow::Provider { id, authenticated: false } if id == "fixture")
    ));

    client
        .handle_input("/provider fixture auth")
        .expect("start auth");
    assert_eq!(
        client.browser().opened,
        vec!["https://example.test/auth".to_owned()]
    );
    client
        .handle_input(r#"/provider fixture complete {"code":"opaque"}"#)
        .expect("complete auth");
    assert!(client.state().transcript().iter().any(
        |row| matches!(row, TranscriptRow::Provider { id, authenticated: true } if id == "fixture")
    ));

    client.handle_input("/model").expect("list models");
    client
        .handle_input("/model fixture/fixture-model")
        .expect("select model");
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
fn ctrl_c_cancels_an_active_submission_and_exits_even_when_provider_is_busy() {
    let (_temporary, mut client, _) = test_client();
    client
        .handle_input("/model fixture/fixture-model")
        .expect("select model");
    client.handle_input("cancel-me").expect("submit prompt");

    assert_eq!(client.handle_ctrl_c(), TuiControl::Exit);
    assert!(client.state().should_exit());
}

#[test]
fn client_records_core_action_errors_in_the_transcript() {
    let (_temporary, mut client, _) = test_client();

    let error = client
        .handle_input("/provider missing auth")
        .expect_err("unknown provider must fail");

    assert!(error.to_string().contains("missing"));
    assert!(
        client
            .state()
            .transcript()
            .iter()
            .any(|row| matches!(row, TranscriptRow::Error(message) if message.contains("missing")))
    );
}

#[test]
fn tui_client_keeps_burst_streams_and_their_terminal_event() {
    let (_temporary, mut client, _) = test_client();
    client
        .handle_input("/model fixture/fixture-model")
        .expect("select model");
    client.handle_input("burst").expect("submit burst");

    thread::sleep(Duration::from_millis(250));
    client.pump_events();

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
fn renderer_uses_one_plain_separator_without_a_transcript_or_global_border() {
    let mut state = UiState::default();
    state.reduce(UiAction::AppendAssistantText("answer".to_owned()));
    let backend = TestBackend::new(20, 5);
    let mut terminal = Terminal::new(backend).expect("test terminal");

    terminal
        .draw(|frame| render(frame, &state))
        .expect("render state");

    let buffer = terminal.backend().buffer();
    let line = |row| {
        (0..20)
            .map(|column| buffer[(column, row)].symbol())
            .collect::<String>()
    };
    assert_eq!(line(0), "answer              ");
    assert_eq!(line(3), "────────────────────");
    assert_eq!(line(4), "                    ");
    assert!(
        !buffer
            .content()
            .iter()
            .any(|cell| matches!(cell.symbol(), "┌" | "┐" | "└" | "┘" | "│"))
    );
}

#[test]
fn authorization_handoff_accepts_web_urls_and_rejects_non_web_locations() {
    validate_authorization_url("https://example.test/authorize?client=misy&state=opaque")
        .expect("web authorization URL");
    assert!(validate_authorization_url("javascript:alert(1)").is_err());
    assert!(validate_authorization_url("https://example.test/authorize\nnext").is_err());
}
