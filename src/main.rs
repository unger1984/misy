fn main() {}
#[allow(dead_code)]
#[derive(Debug, PartialEq, Eq)]
enum Action {
    Quit,
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
