use std::io;

use crossterm::{
    cursor::{Hide, Show},
    event::{self, Event},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Layout},
    widgets::Paragraph,
};

fn main() -> io::Result<()> {
    run_app()
}

#[derive(Debug, PartialEq, Eq)]
enum CleanupStep {
    ShowCursor,
    LeaveAlternateScreen,
    DisableRawMode,
}

fn restoration_plan(
    raw_mode_enabled: bool,
    alternate_screen_entered: bool,
    cursor_hidden: bool,
) -> Vec<CleanupStep> {
    let mut steps = Vec::with_capacity(3);

    if cursor_hidden {
        steps.push(CleanupStep::ShowCursor);
    }
    if alternate_screen_entered {
        steps.push(CleanupStep::LeaveAlternateScreen);
    }
    if raw_mode_enabled {
        steps.push(CleanupStep::DisableRawMode);
    }

    steps
}

fn draw_ui(frame: &mut Frame, state: &AppState) {
    let copy = screen_copy(state);
    let [title_area, body_area, footer_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    frame.render_widget(Paragraph::new(copy.title), title_area);
    frame.render_widget(Paragraph::new(copy.body), body_area);
    frame.render_widget(Paragraph::new(copy.footer), footer_area);
}

fn restore_terminal(
    raw_mode_enabled: bool,
    alternate_screen_entered: bool,
    cursor_hidden: bool,
) -> io::Result<()> {
    let mut first_error = None;
    let mut stdout = io::stdout();

    for step in restoration_plan(raw_mode_enabled, alternate_screen_entered, cursor_hidden) {
        let result = match step {
            CleanupStep::ShowCursor => execute!(stdout, Show),
            CleanupStep::LeaveAlternateScreen => execute!(stdout, LeaveAlternateScreen),
            CleanupStep::DisableRawMode => disable_raw_mode(),
        };

        if let Err(error) = result
            && first_error.is_none()
        {
            first_error = Some(error);
        }
    }

    first_error.map_or(Ok(()), Err)
}

fn run_app() -> io::Result<()> {
    enable_raw_mode()?;
    let raw_mode_enabled = true;

    let mut stdout = io::stdout();
    if let Err(error) = execute!(stdout, EnterAlternateScreen) {
        let _ = restore_terminal(raw_mode_enabled, false, false);
        return Err(error);
    }
    let alternate_screen_entered = true;

    if let Err(error) = execute!(stdout, Hide) {
        let _ = restore_terminal(raw_mode_enabled, alternate_screen_entered, false);
        return Err(error);
    }
    let cursor_hidden = true;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = match Terminal::new(backend) {
        Ok(terminal) => terminal,
        Err(error) => {
            let _ = restore_terminal(raw_mode_enabled, alternate_screen_entered, cursor_hidden);
            return Err(error);
        }
    };

    let app_result = (|| {
        if std::env::var_os("MISY_FAIL_AFTER_TAKEOVER").is_some() {
            return Err(io::Error::other(
                "MISY_FAIL_AFTER_TAKEOVER requested a post-takeover failure",
            ));
        }

        let mut state = AppState::new();
        while state.running {
            terminal.draw(|frame| draw_ui(frame, &state))?;

            if let Event::Key(key) = event::read()?
                && let Some(action) = action_from_key(key.code)
            {
                state.apply(action);
            }
        }

        Ok(())
    })();

    drop(terminal);
    let restoration_result =
        restore_terminal(raw_mode_enabled, alternate_screen_entered, cursor_hidden);

    match (app_result, restoration_result) {
        (Err(error), _) | (Ok(()), Err(error)) => Err(error),
        (Ok(()), Ok(())) => Ok(()),
    }
}
#[allow(dead_code)]
#[derive(Debug, PartialEq, Eq)]
enum Action {
    Quit,
}

fn action_from_key(code: crossterm::event::KeyCode) -> Option<Action> {
    match code {
        crossterm::event::KeyCode::Char('q') => Some(Action::Quit),
        _ => None,
    }
}

#[allow(dead_code)]
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Continue,
    Quit,
}

#[allow(dead_code)]
struct AppState {
    running: bool,
}

#[allow(dead_code)]
impl AppState {
    fn new() -> Self {
        Self { running: true }
    }

    fn apply(&mut self, action: Action) -> Outcome {
        match action {
            Action::Quit => {
                self.running = false;
                Outcome::Quit
            }
        }
    }
}

#[allow(dead_code)]
struct ScreenCopy {
    title: &'static str,
    body: &'static str,
    footer: &'static str,
}

#[allow(dead_code)]
fn screen_copy(_state: &AppState) -> ScreenCopy {
    ScreenCopy {
        title: "misy",
        body: "TUI shell booted",
        footer: "Press q to quit",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyCode;

    #[test]
    fn restoration_plan_reverses_all_completed_terminal_setup_steps() {
        assert_eq!(
            restoration_plan(true, true, true),
            vec![
                CleanupStep::ShowCursor,
                CleanupStep::LeaveAlternateScreen,
                CleanupStep::DisableRawMode,
            ]
        );
    }

    #[test]
    fn restoration_plan_omits_cleanup_for_setup_steps_that_did_not_complete() {
        assert_eq!(
            restoration_plan(false, true, false),
            vec![CleanupStep::LeaveAlternateScreen]
        );
    }

    #[test]
    fn new_creates_a_running_state() {
        assert!(AppState::new().running);
    }

    #[test]
    fn quit_action_stops_the_state_and_returns_quit() {
        let mut state = AppState::new();

        let outcome = state.apply(Action::Quit);

        assert!(!state.running);
        assert_eq!(outcome, Outcome::Quit);
    }

    #[test]
    fn screen_copy_returns_bootstrap_text() {
        let copy = screen_copy(&AppState::new());

        assert_eq!(copy.title, "misy");
        assert_eq!(copy.body, "TUI shell booted");
        assert_eq!(copy.footer, "Press q to quit");
    }

    #[test]
    fn q_key_maps_to_quit_action() {
        assert_eq!(action_from_key(KeyCode::Char('q')), Some(Action::Quit));
    }

    #[test]
    fn unrelated_key_does_not_map_to_an_action() {
        assert_eq!(action_from_key(KeyCode::Enter), None);
    }
}
