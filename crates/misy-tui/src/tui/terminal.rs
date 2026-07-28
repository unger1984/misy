//! Crossterm event loop for the fullscreen terminal client.

use super::{
    action::UiKey,
    browser::SystemBrowser,
    client::TuiClient,
    clipboard::{Clipboard, SystemClipboard, write_osc52_copy},
    render::render_with_composer_area,
    screen_selection::ScreenSelection,
};
use crossterm::{
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags,
        MouseButton, MouseEvent, MouseEventKind, PopKeyboardEnhancementFlags,
        PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{
        EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
        supports_keyboard_enhancement,
    },
};
use misy_core::{MisyCore, MisyPaths};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Position, Rect},
};
use std::{io, io::Write, time::Duration};

const ENABLE_MODIFY_OTHER_KEYS: &str = "\u{1b}[>4;2m";
const DISABLE_MODIFY_OTHER_KEYS: &str = "\u{1b}[>4m";

/// Starts the fullscreen interactive client and restores the prior terminal screen on exit.
///
/// # Errors
///
/// Returns terminal setup, event-read, clipboard-transfer, or draw failures.
pub async fn run(core: MisyCore, paths: &MisyPaths) -> Result<(), io::Error> {
    let stdout = io::stdout();
    let mut guard = TerminalGuard::enter()?;
    let mut client = TuiClient::with_persistent_history(core, SystemBrowser, paths).await;
    let result = (|| {
        let backend = CrosstermBackend::new(stdout.lock());
        let mut terminal = Terminal::new(backend)?;
        let mut clipboard = SystemClipboard::default();
        let mut selection = ScreenSelection::default();
        let mut composer_area = Rect::default();
        let mut composer_click_started = false;
        while !client.state().should_exit() {
            terminal.draw(|frame| {
                composer_area = render_with_composer_area(frame, client.state());
                selection.render(frame.buffer_mut());
            })?;
            if event::poll(Duration::from_millis(50))? {
                let copied = route_event(
                    &mut client,
                    &mut clipboard,
                    &mut selection,
                    composer_area,
                    &mut composer_click_started,
                    event::read()?,
                );
                if let Some(text) = copied
                    && let Err(error) = write_osc52_copy(terminal.backend_mut(), &text)
                {
                    client.report_terminal_error(error);
                }
            }
            client.pump_events();
        }
        Ok(())
    })();
    client.shutdown().await;
    guard.restore();
    result
}

fn route_event(
    client: &mut TuiClient<SystemBrowser>,
    clipboard: &mut impl Clipboard,
    selection: &mut ScreenSelection,
    composer_area: Rect,
    composer_click_started: &mut bool,
    event: Event,
) -> Option<String> {
    match event {
        Event::Key(key) if key.kind == KeyEventKind::Press => {
            route_key(client, clipboard, key);
            None
        }
        Event::Paste(text) => {
            client.paste_text(&text);
            None
        }
        Event::Mouse(mouse) => route_mouse(
            client,
            selection,
            composer_area,
            composer_click_started,
            mouse,
        ),
        _ => None,
    }
}

fn route_mouse(
    client: &mut TuiClient<SystemBrowser>,
    selection: &mut ScreenSelection,
    composer_area: Rect,
    composer_click_started: &mut bool,
    mouse: MouseEvent,
) -> Option<String> {
    let position = Position::new(mouse.column, mouse.row);
    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            selection.begin(position);
            *composer_click_started = composer_area.contains(position);
            None
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            selection.drag(position);
            None
        }
        MouseEventKind::Up(MouseButton::Left) => {
            let copied = selection.finish(position);
            if copied.is_none() && *composer_click_started && composer_area.contains(position) {
                let row = mouse.row.saturating_sub(composer_area.y.saturating_add(1));
                let column = mouse
                    .column
                    .saturating_sub(composer_area.x.saturating_add(3));
                client.position_composer_cursor(row, column);
            }
            *composer_click_started = false;
            copied
        }
        _ => None,
    }
}

fn route_key(client: &mut TuiClient<SystemBrowser>, clipboard: &mut impl Clipboard, key: KeyEvent) {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        client.handle_ctrl_c();
        return;
    }
    if key.modifiers.contains(KeyModifiers::SUPER) && key.code == KeyCode::Char('v') {
        match clipboard.paste() {
            Ok(text) => client.paste_text(&text),
            Err(error) => client.report_terminal_error(error),
        }
        return;
    }
    if let KeyCode::Char(character) = key.code
        && !key.modifiers.contains(KeyModifiers::CONTROL)
        && !key.modifiers.contains(KeyModifiers::ALT)
        && !key.modifiers.contains(KeyModifiers::SUPER)
    {
        if client.state().mode() != super::action::UiMode::Input
            && let Some(index) = character.to_digit(10)
            && index != 0
        {
            // `handle_key` already records failures into the UI state, so this is a duplicate.
            let _ = client.handle_key(UiKey::SelectIndex(index as usize));
            return;
        }
        let mut encoded = [0; 4];
        client.insert_text(character.encode_utf8(&mut encoded));
        return;
    }
    if let Some(normalized) = normalized_key(key) {
        // `handle_key` already records failures into the UI state, so this is a duplicate.
        let _ = client.handle_key(normalized);
    }
}

fn normalized_key(key: KeyEvent) -> Option<UiKey> {
    match key.code {
        KeyCode::Up => Some(UiKey::Up),
        KeyCode::Down => Some(UiKey::Down),
        KeyCode::PageUp => Some(UiKey::PageUp),
        KeyCode::PageDown => Some(UiKey::PageDown),
        KeyCode::Left => Some(UiKey::Left),
        KeyCode::Right => Some(UiKey::Right),
        KeyCode::Home => Some(UiKey::Home),
        KeyCode::End => Some(UiKey::End),
        KeyCode::Backspace => Some(UiKey::Backspace),
        KeyCode::Delete => Some(UiKey::Delete),
        KeyCode::Tab => Some(UiKey::Tab),
        KeyCode::Esc => Some(UiKey::Escape),
        KeyCode::Enter if key.modifiers.contains(KeyModifiers::SHIFT) => Some(UiKey::Newline),
        KeyCode::Enter => Some(UiKey::Enter),
        KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::CONTROL) => Some(UiKey::Newline),
        _ => None,
    }
}

struct TerminalGuard {
    restored: bool,
    keyboard_enhancement_enabled: bool,
    modify_other_keys_enabled: bool,
    bracketed_paste_enabled: bool,
    alternate_screen_enabled: bool,
    mouse_capture_enabled: bool,
}

impl TerminalGuard {
    fn enter() -> Result<Self, io::Error> {
        enable_raw_mode()?;
        let mut guard = Self {
            restored: false,
            keyboard_enhancement_enabled: false,
            modify_other_keys_enabled: false,
            bracketed_paste_enabled: false,
            alternate_screen_enabled: false,
            mouse_capture_enabled: false,
        };
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        guard.alternate_screen_enabled = true;
        execute!(stdout, EnableMouseCapture)?;
        guard.mouse_capture_enabled = true;
        execute!(stdout, EnableBracketedPaste)?;
        guard.bracketed_paste_enabled = true;
        if supports_keyboard_enhancement().unwrap_or(false) {
            execute!(
                stdout,
                PushKeyboardEnhancementFlags(keyboard_enhancement_flags())
            )?;
            guard.keyboard_enhancement_enabled = true;
        } else {
            write_escape(&mut stdout, ENABLE_MODIFY_OTHER_KEYS)?;
            guard.modify_other_keys_enabled = true;
        }
        Ok(guard)
    }

    fn restore(&mut self) {
        if self.restored {
            return;
        }
        let mut stdout = io::stdout();
        // Teardown is best-effort: a failed escape leaves a mode the next shell prompt
        // redraw overrides, and there is no recovery to attempt while exiting.
        if self.keyboard_enhancement_enabled {
            let _ = execute!(stdout, PopKeyboardEnhancementFlags);
        }
        if self.modify_other_keys_enabled {
            let _ = write_escape(&mut stdout, DISABLE_MODIFY_OTHER_KEYS);
        }
        if self.bracketed_paste_enabled {
            let _ = execute!(stdout, DisableBracketedPaste);
        }
        if self.mouse_capture_enabled {
            let _ = execute!(stdout, DisableMouseCapture);
        }
        if self.alternate_screen_enabled {
            let _ = execute!(stdout, LeaveAlternateScreen);
        }
        let _ = disable_raw_mode();
        self.restored = true;
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        self.restore();
    }
}

fn keyboard_enhancement_flags() -> KeyboardEnhancementFlags {
    // Alternate codes preserve layout-resolved text while disambiguation keeps Shift+Enter.
    KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
        | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
}

fn write_escape(stdout: &mut io::Stdout, sequence: &str) -> Result<(), io::Error> {
    stdout.write_all(sequence.as_bytes())?;
    stdout.flush()
}

#[cfg(test)]
mod tests {
    use super::super::{clipboard::FailingClipboard, state::TranscriptRow};
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn paste_failure_is_reported_in_the_transcript_without_ending_the_session() {
        let temporary = tempfile::tempdir().expect("temporary root");
        let bundled = temporary.path().join("bundled");
        std::fs::create_dir_all(&bundled).expect("bundled providers directory");
        let core = MisyCore::discover(
            MisyPaths::from_root(temporary.path().join("misy")),
            &bundled,
        )
        .expect("core discovery without providers");
        let mut client = TuiClient::new(core, SystemBrowser).await;

        route_key(
            &mut client,
            &mut FailingClipboard,
            KeyEvent::new(KeyCode::Char('v'), KeyModifiers::SUPER),
        );

        let transcript = client.state().transcript();
        assert!(
            transcript.iter().any(|row| {
                matches!(row, TranscriptRow::Error(message) if message.contains("could not access the clipboard"))
            }),
            "clipboard failure must surface as a transcript error, got {transcript:?}"
        );
        assert!(!client.state().should_exit());
        client.insert_text("still typing");
        assert_eq!(client.state().composer_input(), "still typing");
    }

    #[test]
    fn shift_enter_and_ctrl_j_insert_a_composer_newline() {
        assert_eq!(
            normalized_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)),
            Some(UiKey::Newline)
        );
        assert_eq!(
            normalized_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL)),
            Some(UiKey::Newline)
        );
        assert_eq!(
            normalized_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Some(UiKey::Enter)
        );
    }

    #[test]
    fn keyboard_protocol_preserves_layout_text_and_modified_enter() {
        assert_eq!(
            keyboard_enhancement_flags(),
            KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
        );
        assert!(
            !keyboard_enhancement_flags()
                .contains(KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES)
        );
    }
}
