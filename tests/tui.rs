use misy::{
    MisyCore, MisyPaths, ModelId, ModelRef, ProviderId,
    tui::{
        BrowserHandoff, BrowserPlatform, TranscriptRow, TuiClient, TuiControl, UiAction, UiState,
        browser_command, map_input, render, validate_authorization_url,
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
    client.pump_events();
    assert_eq!(
        client.state().transcript(),
        &[TranscriptRow::Provider {
            id: "fixture".to_owned(),
            authenticated: false,
        }]
    );

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
    client.pump_events();
    assert_eq!(
        client
            .state()
            .transcript()
            .iter()
            .filter(|row| matches!(
                row,
                TranscriptRow::Provider {
                    authenticated: true,
                    ..
                }
            ))
            .count(),
        1
    );

    client.handle_input("/model").expect("list models");
    client.pump_events();
    let models_before_selection = client
        .state()
        .transcript()
        .iter()
        .filter(|row| matches!(row, TranscriptRow::Model { .. }))
        .count();
    assert_eq!(models_before_selection, 2);
    client
        .handle_input("/model fixture/fixture-model")
        .expect("select model");
    client.pump_events();
    assert_eq!(
        client
            .state()
            .transcript()
            .iter()
            .filter(|row| matches!(row, TranscriptRow::Model { .. }))
            .count(),
        2
    );
    assert_eq!(
        client
            .state()
            .transcript()
            .iter()
            .filter(|row| matches!(row, TranscriptRow::Model { selected: true, .. }))
            .count(),
        1
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
    assert!(
        rows[(separator_row + 1)..]
            .iter()
            .all(|row| row.trim().is_empty())
    );
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
    client
        .handle_input("/model fixture/fixture-model")
        .expect("select model");
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
