fn main() {}
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
