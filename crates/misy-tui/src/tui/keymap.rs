//! Configurable named shortcuts for terminal actions.

use crate::tui::action::UiKey;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::BTreeMap;

const STOP_ACTIVITY: &str = "activities.stop";
const EXPAND_TRANSCRIPT: &str = "transcript.expand";

#[derive(Clone, Debug)]
pub(super) struct Keymap {
    bindings: Vec<(Binding, UiKey)>,
    stop_hint: String,
    expand_hint: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Binding {
    code: KeyCode,
    modifiers: KeyModifiers,
    display: String,
}

impl Keymap {
    pub(super) fn from_overrides(overrides: &BTreeMap<String, Vec<String>>) -> (Self, Vec<String>) {
        let mut warnings = Vec::new();
        for action in overrides.keys() {
            if action != STOP_ACTIVITY && action != EXPAND_TRANSCRIPT {
                warnings.push(format!("unknown keybinding action `{action}`"));
            }
        }
        let stop = bindings_for(overrides, STOP_ACTIVITY, "Ctrl+X", &mut warnings);
        let expand = bindings_for(overrides, EXPAND_TRANSCRIPT, "Ctrl+O", &mut warnings);
        let mut bindings = Vec::new();
        add_action_bindings(
            &mut bindings,
            stop.iter().cloned(),
            STOP_ACTIVITY,
            UiKey::StopActivity,
            &mut warnings,
        );
        let effective_expand = add_action_bindings(
            &mut bindings,
            expand.iter().cloned(),
            EXPAND_TRANSCRIPT,
            UiKey::ToggleToolOutput,
            &mut warnings,
        );
        let stop_hint = stop
            .first()
            .map_or_else(|| "unbound".to_owned(), |item| item.display.clone());
        let expand_hint = effective_expand
            .first()
            .map_or_else(|| "unbound".to_owned(), |item| item.display.clone());
        (
            Self {
                bindings,
                stop_hint,
                expand_hint,
            },
            warnings,
        )
    }

    pub(super) fn resolve(&self, event: KeyEvent) -> Option<UiKey> {
        self.bindings
            .iter()
            .find(|(binding, _)| {
                key_code_matches(binding.code, event.code) && binding.modifiers == event.modifiers
            })
            .map(|(_, key)| *key)
    }

    pub(super) fn stop_hint(&self) -> &str {
        &self.stop_hint
    }

    pub(super) fn expand_hint(&self) -> &str {
        &self.expand_hint
    }
}

fn add_action_bindings(
    bindings: &mut Vec<(Binding, UiKey)>,
    candidates: impl Iterator<Item = Binding>,
    action: &str,
    key: UiKey,
    warnings: &mut Vec<String>,
) -> Vec<Binding> {
    let mut accepted = Vec::new();
    for candidate in candidates {
        if let Some((_, winner)) = bindings
            .iter()
            .find(|(binding, _)| binding.same_key(&candidate))
        {
            warnings.push(format!(
                "keybinding `{}` for `{action}` conflicts with `{}`; `{}` wins",
                candidate.display,
                action_name(*winner),
                action_name(*winner)
            ));
            continue;
        }
        bindings.push((candidate.clone(), key));
        accepted.push(candidate);
    }
    accepted
}

impl Binding {
    fn same_key(&self, other: &Self) -> bool {
        key_code_matches(self.code, other.code) && self.modifiers == other.modifiers
    }
}

fn action_name(key: UiKey) -> &'static str {
    match key {
        UiKey::StopActivity => STOP_ACTIVITY,
        UiKey::ToggleToolOutput => EXPAND_TRANSCRIPT,
        _ => "unknown",
    }
}

fn key_code_matches(left: KeyCode, right: KeyCode) -> bool {
    match (left, right) {
        (KeyCode::Char(left), KeyCode::Char(right)) => left.eq_ignore_ascii_case(&right),
        _ => left == right,
    }
}

fn bindings_for(
    overrides: &BTreeMap<String, Vec<String>>,
    action: &str,
    default: &str,
    warnings: &mut Vec<String>,
) -> Vec<Binding> {
    let configured = overrides
        .get(action)
        .cloned()
        .unwrap_or_else(|| vec![default.to_owned()]);
    let parsed = configured
        .iter()
        .filter_map(|value| match parse_binding(value) {
            Ok(binding) => Some(binding),
            Err(message) => {
                warnings.push(format!("invalid `{action}` binding `{value}`: {message}"));
                None
            }
        })
        .collect::<Vec<_>>();
    if parsed.is_empty() && !configured.is_empty() {
        parse_binding(default).into_iter().collect()
    } else {
        parsed
    }
}

fn parse_binding(value: &str) -> Result<Binding, String> {
    let parts = value.split('+').map(str::trim).collect::<Vec<_>>();
    let Some(key) = parts.last().filter(|part| !part.is_empty()) else {
        return Err("missing key".to_owned());
    };
    let mut modifiers = KeyModifiers::NONE;
    for modifier in &parts[..parts.len().saturating_sub(1)] {
        match modifier.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => modifiers.insert(KeyModifiers::CONTROL),
            "alt" | "option" => modifiers.insert(KeyModifiers::ALT),
            "shift" => modifiers.insert(KeyModifiers::SHIFT),
            "super" | "cmd" => modifiers.insert(KeyModifiers::SUPER),
            _ => return Err(format!("unknown modifier `{modifier}`")),
        }
    }
    let code = match key.to_ascii_lowercase().as_str() {
        "enter" => KeyCode::Enter,
        "escape" | "esc" => KeyCode::Esc,
        "tab" => KeyCode::Tab,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        value if value.chars().count() == 1 => KeyCode::Char(
            value
                .chars()
                .next()
                .ok_or_else(|| "missing key".to_owned())?,
        ),
        _ => return Err(format!("unknown key `{key}`")),
    };
    Ok(Binding {
        code,
        modifiers,
        display: value.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::Keymap;
    use crate::tui::action::UiKey;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::collections::BTreeMap;

    #[test]
    fn overrides_named_stop_action() {
        let overrides = BTreeMap::from([("activities.stop".to_owned(), vec!["Ctrl+K".to_owned()])]);
        let (keymap, warnings) = Keymap::from_overrides(&overrides);
        assert!(warnings.is_empty());
        assert_eq!(
            keymap.resolve(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL)),
            Some(UiKey::StopActivity)
        );
        assert_eq!(keymap.stop_hint(), "Ctrl+K");
    }

    #[test]
    fn defaults_named_actions() {
        let (keymap, warnings) = Keymap::from_overrides(&BTreeMap::new());
        assert!(warnings.is_empty());
        assert_eq!(
            keymap.resolve(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL)),
            Some(UiKey::StopActivity)
        );
        assert_eq!(
            keymap.resolve(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::ALT)),
            None
        );
        assert_eq!(
            keymap.resolve(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL)),
            Some(UiKey::ToggleToolOutput)
        );
        assert_eq!(keymap.expand_hint(), "Ctrl+O");
    }

    #[test]
    fn earlier_action_wins_binding_collisions() {
        let overrides = BTreeMap::from([
            ("activities.stop".to_owned(), vec!["Ctrl+K".to_owned()]),
            ("transcript.expand".to_owned(), vec!["Ctrl+K".to_owned()]),
        ]);
        let (keymap, warnings) = Keymap::from_overrides(&overrides);

        assert_eq!(
            keymap.resolve(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL)),
            Some(UiKey::StopActivity)
        );
        assert_eq!(keymap.expand_hint(), "unbound");
        assert!(warnings[0].contains("`activities.stop` wins"));
    }

    #[test]
    fn override_and_invalid_fallback_set_the_effective_expand_hint() {
        let override_map =
            BTreeMap::from([("transcript.expand".to_owned(), vec!["Alt+E".to_owned()])]);
        let (overridden, warnings) = Keymap::from_overrides(&override_map);
        assert!(warnings.is_empty());
        assert_eq!(overridden.expand_hint(), "Alt+E");
        assert_eq!(
            overridden.resolve(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::ALT)),
            Some(UiKey::ToggleToolOutput)
        );

        let invalid_map =
            BTreeMap::from([("transcript.expand".to_owned(), vec!["Hyper+E".to_owned()])]);
        let (fallback, warnings) = Keymap::from_overrides(&invalid_map);
        assert_eq!(fallback.expand_hint(), "Ctrl+O");
        assert_eq!(warnings.len(), 1);
    }
}
