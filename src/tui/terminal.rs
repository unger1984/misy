//! Crossterm event loop for the inline terminal client.

use super::{
    action::UiKey,
    browser::SystemBrowser,
    client::TuiClient,
    render::{render, transcript_lines},
};
use crate::{MisyCore, MisyPaths};
use crossterm::{
    event::{
        self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags,
        PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, supports_keyboard_enhancement},
};
use ratatui::{
    Terminal, TerminalOptions, Viewport,
    backend::CrosstermBackend,
    text::Text,
    widgets::{Paragraph, Widget, Wrap},
};
use std::{io, io::Write, time::Duration};

const INLINE_VIEWPORT_HEIGHT: u16 = 18;
const ENABLE_MODIFY_OTHER_KEYS: &str = "\u{1b}[>4;2m";
const DISABLE_MODIFY_OTHER_KEYS: &str = "\u{1b}[>4m";

/// Starts the interactive client without replacing the user's terminal screen.
///
/// Finalized rows are inserted ahead of Ratatui's inline viewport, making them ordinary terminal
/// scrollback that remains visible after exit. The fixed viewport is deliberately modest; widgets
/// occupy only their content-driven prefix inside it.
///
/// # Errors
///
/// Returns terminal setup, event-read, history-insertion, or draw failures.
pub fn run(core: MisyCore, paths: &MisyPaths) -> Result<(), io::Error> {
    let stdout = io::stdout();
    let mut guard = TerminalGuard::enter()?;
    let mut client = TuiClient::with_persistent_history(core, SystemBrowser, paths);
    let result = (|| {
        let backend = CrosstermBackend::new(stdout.lock());
        let mut terminal = Terminal::with_options(
            backend,
            TerminalOptions {
                viewport: Viewport::Inline(INLINE_VIEWPORT_HEIGHT),
            },
        )?;
        let mut printed_rows = 0;
        while !client.state().should_exit() {
            insert_finalized_history(&mut terminal, client.state(), &mut printed_rows)?;
            terminal.draw(|frame| render(frame, client.state()))?;
            if event::poll(Duration::from_millis(50))?
                && let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
            {
                route_key(&mut client, key);
            }
            client.pump_events();
        }
        insert_finalized_history(&mut terminal, client.state(), &mut printed_rows)?;
        Ok(())
    })();
    client.handle_ctrl_c();
    guard.restore();
    result
}

fn insert_finalized_history(
    terminal: &mut Terminal<CrosstermBackend<std::io::StdoutLock<'_>>>,
    state: &super::state::UiState,
    printed_rows: &mut usize,
) -> Result<(), io::Error> {
    let finalized = state.finalized_transcript_len();
    if finalized <= *printed_rows {
        return Ok(());
    }
    let lines = transcript_lines(&state.transcript()[*printed_rows..finalized]);
    let width = terminal.size()?.width;
    let paragraph = Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false });
    let height = u16::try_from(paragraph.line_count(width)).unwrap_or(u16::MAX);
    terminal.insert_before(height, |buffer| paragraph.render(buffer.area, buffer))?;
    *printed_rows = finalized;
    Ok(())
}

fn route_key(client: &mut TuiClient<SystemBrowser>, key: KeyEvent) {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        client.handle_ctrl_c();
        return;
    }
    if let KeyCode::Char(character) = key.code
        && !key.modifiers.contains(KeyModifiers::CONTROL)
        && !key.modifiers.contains(KeyModifiers::ALT)
    {
        if client.state().mode() != super::action::UiMode::Input
            && let Some(index) = character.to_digit(10)
            && index != 0
        {
            let _ = client.handle_key(UiKey::SelectIndex(index as usize));
            return;
        }
        let mut encoded = [0; 4];
        client.insert_text(character.encode_utf8(&mut encoded));
        return;
    }
    if let Some(normalized) = normalized_key(key) {
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
}

impl TerminalGuard {
    fn enter() -> Result<Self, io::Error> {
        enable_raw_mode()?;
        let mut guard = Self {
            restored: false,
            keyboard_enhancement_enabled: false,
            modify_other_keys_enabled: false,
        };
        let mut stdout = io::stdout();
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
        if self.keyboard_enhancement_enabled {
            let _ = execute!(stdout, PopKeyboardEnhancementFlags);
        }
        if self.modify_other_keys_enabled {
            let _ = write_escape(&mut stdout, DISABLE_MODIFY_OTHER_KEYS);
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
    KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
        | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
        | KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES
}

fn write_escape(stdout: &mut io::Stdout, sequence: &str) -> Result<(), io::Error> {
    stdout.write_all(sequence.as_bytes())?;
    stdout.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn keyboard_protocol_requests_modified_plain_keys() {
        assert_eq!(
            keyboard_enhancement_flags(),
            KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
                | KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES
        );
    }
}
