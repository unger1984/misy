use misy::{
    MisyCore, MisyPaths,
    tui::{BrowserHandoff, TuiClient, UiKey, UiMode, UiState, render},
};
use ratatui::{Terminal, backend::TestBackend, buffer::Buffer};
use std::{
    fs,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

#[derive(Default)]
pub(crate) struct RecordingBrowser {
    pub(crate) opened: Vec<String>,
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
  "protocol_version": 2,
  "description": "TUI fixture",
  "capabilities": {{"usage": {{"version": 1}}}},
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

pub(crate) fn core_with_providers(
    specifications: &[(&str, &str, &str)],
) -> (tempfile::TempDir, MisyCore) {
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

pub(crate) fn test_client() -> (tempfile::TempDir, TuiClient<RecordingBrowser>, PathBuf) {
    let (temporary, core) = core_with_providers(&[("fixture", "Fixture AI", "tool-output.txt")]);
    let target = temporary.path().join("tool-output.txt");
    (
        temporary,
        TuiClient::new(core, RecordingBrowser::default()),
        target,
    )
}

pub(crate) fn wait_for(
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

pub(crate) fn render_buffer(state: &UiState, width: u16, height: u16) -> Buffer {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| render(frame, state))
        .expect("render state");
    terminal.backend().buffer().clone()
}

pub(crate) fn buffer_lines(buffer: &Buffer, width: u16) -> Vec<String> {
    buffer
        .content()
        .chunks(usize::from(width))
        .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
        .collect()
}

pub(crate) fn authorize_first_provider(client: &mut TuiClient<RecordingBrowser>) {
    start_first_provider_auth(client);
    wait_for(client, |client| {
        client.state().picker_labels() == ["Log out"]
    });
}

pub(crate) fn start_first_provider_auth(client: &mut TuiClient<RecordingBrowser>) {
    client.handle_input("/provider").expect("providers");
    client.handle_key(UiKey::Enter).expect("provider settings");
    assert_eq!(client.state().picker_labels(), ["Authorize"]);
    client.handle_key(UiKey::Enter).expect("authorize");
}

pub(crate) fn select_first_model(client: &mut TuiClient<RecordingBrowser>) {
    authorize_first_provider(client);
    client.handle_key(UiKey::Escape).expect("provider list");
    client.handle_key(UiKey::Escape).expect("composer");
    client.handle_input("/model").expect("models");
    wait_for(client, |client| client.state().picker_labels().len() == 2);
    client.handle_key(UiKey::Enter).expect("select model");
    wait_for(client, |client| client.state().mode() == UiMode::Input);
}
