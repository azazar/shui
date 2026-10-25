use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::Deserialize;
use std::collections::{BTreeMap, HashSet};

#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize)]
#[serde(try_from = "String")]
pub struct Key {
    code: KeyCode,
    modifiers: KeyModifiers,
}
impl TryFrom<String> for Key {
    type Error = String;
    fn try_from(value: String) -> Result<Self, String> {
        let invalid = || format!("invalid key: {value}");
        let normalized = value.to_ascii_lowercase();
        let mut parts: Vec<_> = normalized.split('+').map(str::trim).collect();
        let name = parts.pop().ok_or_else(invalid)?;
        let mut modifiers = KeyModifiers::NONE;
        for part in parts {
            let flag = match part {
                "ctrl" => KeyModifiers::CONTROL,
                "alt" => KeyModifiers::ALT,
                "shift" => KeyModifiers::SHIFT,
                _ => return Err(invalid()),
            };
            if modifiers.contains(flag) {
                return Err(invalid());
            }
            modifiers |= flag;
        }
        let code = match name {
            "enter" => KeyCode::Enter,
            "esc" | "escape" => KeyCode::Esc,
            "tab" => KeyCode::Tab,
            "backtab" => {
                modifiers |= KeyModifiers::SHIFT;
                KeyCode::Tab
            }
            "up" => KeyCode::Up,
            "down" => KeyCode::Down,
            "left" => KeyCode::Left,
            "right" => KeyCode::Right,
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            "pageup" | "pgup" => KeyCode::PageUp,
            "pagedown" | "pgdn" => KeyCode::PageDown,
            "backspace" => KeyCode::Backspace,
            "delete" | "del" => KeyCode::Delete,
            "space" => KeyCode::Char(' '),
            s if s.starts_with('f') && s.len() > 1 => KeyCode::F(
                s[1..]
                    .parse::<u8>()
                    .ok()
                    .filter(|n| (1..=24).contains(n))
                    .ok_or_else(invalid)?,
            ),
            s if s.len() == 1 && s.is_ascii() => KeyCode::Char(s.chars().next().unwrap()),
            _ => return Err(invalid()),
        };
        if matches!(code, KeyCode::Char(_)) && modifiers.contains(KeyModifiers::SHIFT) {
            return Err("Shift is supported with named keys, not text characters".into());
        }
        Ok(Self::from_event(KeyEvent::new(code, modifiers)))
    }
}
impl Key {
    fn from_event(event: KeyEvent) -> Self {
        let mut code = event.code;
        let mut modifiers = event.modifiers;
        if code == KeyCode::BackTab {
            code = KeyCode::Tab;
            modifiers |= KeyModifiers::SHIFT;
        }
        if let KeyCode::Char(c) = code {
            // Punctuation carries its own keyboard-layout Shift (e.g. Alt+?).
            // For letters, preserve an explicitly reported Shift modifier so
            // Alt+Shift+S cannot activate an Alt-only action.
            if c.is_ascii_punctuation() {
                modifiers.remove(KeyModifiers::SHIFT);
            }
            code = KeyCode::Char(c.to_ascii_lowercase());
            // Legacy control bytes alias these keys only in the Ctrl-only
            // form. Explicit extra modifiers must not dispatch a smaller
            // group (e.g. Ctrl+Shift+I becoming Shift+Tab).
            if modifiers == KeyModifiers::CONTROL {
                match c.to_ascii_lowercase() {
                    'i' => {
                        code = KeyCode::Tab;
                        modifiers.remove(KeyModifiers::CONTROL);
                    }
                    'j' | 'm' => {
                        code = KeyCode::Enter;
                        modifiers.remove(KeyModifiers::CONTROL);
                    }
                    'h' => code = KeyCode::Backspace,
                    _ => {}
                }
            }
        }
        Self { code, modifiers }
    }
    fn event(&self) -> KeyEvent {
        if self.code == KeyCode::Tab && self.modifiers.contains(KeyModifiers::SHIFT) {
            KeyEvent::new(KeyCode::BackTab, self.modifiers)
        } else {
            KeyEvent::new(self.code, self.modifiers)
        }
    }
    fn typing(&self) -> bool {
        matches!(self.code, KeyCode::Char(_))
            && self.modifiers.difference(KeyModifiers::SHIFT).is_empty()
    }
}
impl std::fmt::Display for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (flag, name) in [
            (KeyModifiers::CONTROL, "Ctrl+"),
            (KeyModifiers::ALT, "Alt+"),
            (KeyModifiers::SHIFT, "Shift+"),
        ] {
            if self.modifiers.contains(flag) {
                write!(f, "{name}")?;
            }
        }
        let name = match self.code {
            KeyCode::Char(' ') => "Space".into(),
            KeyCode::Char(c) => c.to_ascii_uppercase().to_string(),
            KeyCode::Left => "←".into(),
            KeyCode::Right => "→".into(),
            KeyCode::Up => "↑".into(),
            KeyCode::Down => "↓".into(),
            KeyCode::F(n) => format!("F{n}"),
            KeyCode::Esc => "Esc".into(),
            KeyCode::PageUp => "PgUp".into(),
            KeyCode::PageDown => "PgDn".into(),
            other => format!("{other:?}"),
        };
        write!(f, "{name}")
    }
}

type Actions = BTreeMap<String, Vec<Key>>;
#[derive(Debug, Default, Deserialize)]
#[serde(transparent)]
pub struct Bindings(BTreeMap<String, Actions>);

// Canonical events are internal action identifiers, independent of user bindings.
fn defaults(scope: &str) -> Vec<(&'static str, &'static str, Vec<&'static str>)> {
    let mut result = vec![
        ("help", "F1", vec!["F1"]),
        ("reveal_alt", "F21", vec!["Alt+?"]),
        ("reveal_ctrl_alt", "F22", vec!["Ctrl+Alt+?"]),
        ("reveal_ctrl", "F23", vec!["Ctrl+?"]),
        ("reveal_shift", "F24", vec!["Shift+F1"]),
    ];
    match scope {
        "menu" => result.extend([
            ("quit", "Esc", vec!["Esc", "Ctrl+C"]),
            ("connect", "Enter", vec!["Enter"]),
            ("connect_and_return", "Alt+Enter", vec!["Alt+Enter"]),
            ("connection_settings", "Ctrl+Enter", vec!["Ctrl+Enter"]),
            ("configure_remote", "F4", vec!["F4"]),
            ("new_remote", "Shift+F4", vec!["Shift+F4"]),
            ("edit_config", "F9", vec!["F9"]),
            ("refresh", "F5", vec!["F5"]),
            ("clear_search", "Ctrl+U", vec!["Ctrl+U"]),
            ("home", "Home", vec!["Home"]),
            ("end", "End", vec!["End"]),
            ("toggle_transport", "F8", vec!["F8"]),
        ]),
        "dialog" => result.extend([
            ("cancel", "Esc", vec!["Esc", "Ctrl+C"]),
            ("connect", "Enter", vec!["Enter"]),
            ("connect_and_return", "Alt+Enter", vec!["Alt+Enter"]),
            ("next_field", "Tab", vec!["Tab", "Down"]),
            ("previous_field", "Shift+Tab", vec!["Shift+Tab", "Up"]),
            ("toggle_recipe", "F6", vec!["Space"]),
        ]),
        "help" => result.extend([
            ("close", "Esc", vec!["Esc", "Ctrl+C", "Q"]),
            ("next_topic", "Right", vec!["Right", "Tab"]),
            ("previous_topic", "Left", vec!["Left", "Shift+Tab"]),
            ("home", "Home", vec!["Home"]),
            ("end", "End", vec!["End"]),
        ]),
        "editor" => result.extend([
            ("cancel", "Esc", vec!["Esc", "Ctrl+C"]),
            ("edit", "Enter", vec!["Enter", "F9"]),
        ]),
        _ => {}
    }
    if scope == "menu" || scope == "help" {
        result.extend([
            ("up", "Up", vec!["Up"]),
            ("down", "Down", vec!["Down"]),
            ("page_up", "PgUp", vec!["PgUp"]),
            (
                "page_down",
                "PgDn",
                if scope == "help" {
                    vec!["PgDn", "Space"]
                } else {
                    vec!["PgDn"]
                },
            ),
        ]);
    }
    if scope == "menu" || scope == "dialog" {
        result.extend([
            ("left", "Left", vec!["Left"]),
            ("right", "Right", vec!["Right"]),
            ("erase_character", "Backspace", vec!["Backspace", "Delete"]),
            (
                "erase_word",
                "Ctrl+Backspace",
                vec!["Ctrl+Backspace", "Ctrl+Delete"],
            ),
        ]);
    }
    result
}
impl Bindings {
    #[cfg(test)]
    pub(crate) fn test_actions(&self, scope: &str) -> Vec<(String, Vec<KeyEvent>)> {
        defaults(scope)
            .into_iter()
            .map(|(action, _, keys)| {
                (
                    action.to_owned(),
                    self.keys(scope, action, &keys)
                        .iter()
                        .map(Key::event)
                        .collect(),
                )
            })
            .collect()
    }

    fn keys(&self, scope: &str, action: &str, fallback: &[&str]) -> Vec<Key> {
        self.0
            .get(scope)
            .and_then(|map| map.get(action))
            .cloned()
            .unwrap_or_else(|| {
                fallback
                    .iter()
                    .map(|s| Key::try_from((*s).to_owned()).unwrap())
                    .collect()
            })
    }
    pub fn label(&self, scope: &str, action: &str) -> String {
        defaults(scope)
            .into_iter()
            .find(|(a, _, _)| *a == action)
            .map(|(_, _, keys)| {
                self.keys(scope, action, &keys)
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("/")
            })
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "—".into())
    }
    pub fn key_labels(&self, scope: &str, action: &str) -> Vec<String> {
        defaults(scope)
            .into_iter()
            .find(|(a, _, _)| *a == action)
            .map(|(_, _, keys)| {
                self.keys(scope, action, &keys)
                    .iter()
                    .map(ToString::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }
    pub fn actions(&self, scope: &str) -> Vec<(String, Vec<String>)> {
        defaults(scope)
            .into_iter()
            .map(|(action, _, _)| (action.to_owned(), self.key_labels(scope, action)))
            .collect()
    }
    pub fn hint(&self, scope: &str, action: &str) -> String {
        self.key_labels(scope, action)
            .into_iter()
            .next()
            .unwrap_or_else(|| "—".into())
    }
    pub fn reference(&self, scope: &str) -> String {
        let mut text = String::from(
            "# Active keybindings\n\nThese reflect your current configuration. An em dash means disabled.\n\n",
        );
        for scope in [scope, "help"] {
            text.push_str(&format!("## {scope}\n\n| Action | Keys |\n| --- | --- |\n"));
            for (action, _, _) in defaults(scope) {
                text.push_str(&format!("| `{action}` | {} |\n", self.label(scope, action)));
            }
            text.push('\n');
        }
        text
    }
    pub fn translate(&self, scope: &str, event: KeyEvent) -> Option<KeyEvent> {
        self.translate_context(scope, event, false)
    }
    pub fn translate_dialog(&self, event: KeyEvent, recipe_selector: bool) -> Option<KeyEvent> {
        self.translate_context("dialog", event, recipe_selector)
    }
    fn translate_context(
        &self,
        scope: &str,
        event: KeyEvent,
        recipe_selector: bool,
    ) -> Option<KeyEvent> {
        let key = Key::from_event(event);
        for (action, canonical, fallback) in defaults(scope) {
            if action == "toggle_recipe" && !recipe_selector {
                continue;
            }
            if self.keys(scope, action, &fallback).contains(&key) {
                return Some(Key::try_from(canonical.to_owned()).unwrap().event());
            }
        }
        // Text entry is not an action and must remain available in editable screens.
        if scope != "help" && key.typing() {
            Some(event)
        } else {
            None
        }
    }
    pub fn validate(&self, profiles: &[crate::launch::Profile]) -> Result<(), String> {
        for (scope, actions) in &self.0 {
            if !["menu", "dialog", "help", "editor"].contains(&scope.as_str()) {
                return Err(format!("unknown keybindings scope: {scope}"));
            }
            for action in actions.keys() {
                if !defaults(scope).iter().any(|(a, _, _)| a == action) {
                    return Err(format!("unknown {scope} action: {action}"));
                }
            }
        }
        for scope in ["menu", "dialog", "help", "editor"] {
            let mut used = HashSet::new();
            for (action, _, fallback) in defaults(scope) {
                for key in self.keys(scope, action, &fallback) {
                    if scope != "help"
                        && key.typing()
                        && !(scope == "dialog"
                            && action == "toggle_recipe"
                            && key.code == KeyCode::Char(' '))
                    {
                        return Err(format!(
                            "{scope}.{action}: unmodified text keys are reserved for typing"
                        ));
                    }
                    if !used.insert(key.clone()) {
                        return Err(format!("duplicate key {key} in {scope}"));
                    }
                }
            }
            if scope == "menu" {
                for profile in profiles {
                    if let Some(key) = &profile.key {
                        let key = Key::try_from(key.to_string())?;
                        if !used.insert(key.clone()) {
                            return Err(format!(
                                "profile key {key} conflicts with a menu action or profile"
                            ));
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config(overrides: &str) -> Result<crate::launch::Config, std::io::Error> {
        crate::launch::Config::parse(&format!(
            "default: SSH\nprofiles: [{{name: SSH, key: Alt+S, command: 'ssh %h'}}]\n{overrides}"
        ))
    }
    #[test]
    fn validates_collisions_aliases_and_typing() {
        for override_ in [
            "keybindings: {menu: {connect: [Alt+S]}}",
            "keybindings: {menu: {connect: [Ctrl+M], quit: [Enter]}}",
            "keybindings: {menu: {connect: [X]}}",
            "keybindings: {dialog: {help: [Tab]}}",
            "keybindings: {menu: {typo: [F3]}}",
            "keybindings: {unknown: {help: [F3]}}",
            "keybindings: {menu: {connect: [F25]}}",
            "keybindings: {menu: {connect: [Shift+X]}}",
            "return_to_menu: true",
        ] {
            assert!(config(override_).is_err(), "{override_}");
        }
        assert!(
            config("keybindings: {menu: {connect: [F3], help: []}, dialog: {connect: [F3]}}")
                .is_ok()
        );
        assert!(crate::launch::Config::parse("default: SSH\nprofiles: [{name: SSH, key: Ctrl+C, command: 'ssh %h'}]\nkeybindings: {menu: {quit: [Esc]}}").is_ok());
    }
    #[test]
    fn replaced_keys_are_inactive_and_terminal_aliases_match() {
        let config =
            config("keybindings: {menu: {connect: [F3], connect_and_return: [Ctrl+R]}}").unwrap();
        let bindings = config.keybindings;
        assert!(
            bindings
                .translate("menu", KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
                .is_none()
        );
        assert_eq!(
            bindings
                .translate("menu", KeyEvent::new(KeyCode::F(3), KeyModifiers::NONE))
                .unwrap()
                .code,
            KeyCode::Enter
        );
        let action = bindings
            .translate(
                "menu",
                KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL),
            )
            .unwrap();
        assert_eq!(action, KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT));
        assert_eq!(
            bindings
                .translate(
                    "dialog",
                    KeyEvent::new(KeyCode::Char('h'), KeyModifiers::CONTROL)
                )
                .unwrap()
                .code,
            KeyCode::Backspace
        );
        assert!(bindings.reference("menu").contains("Ctrl+R"));
    }

    #[test]
    fn explicit_extra_modifiers_do_not_collapse_terminal_aliases() {
        let bindings = Bindings::default();
        for (letter, canonical, modifiers) in [
            ('i', KeyCode::Tab, KeyModifiers::NONE),
            ('j', KeyCode::Enter, KeyModifiers::NONE),
            ('m', KeyCode::Enter, KeyModifiers::NONE),
            ('h', KeyCode::Backspace, KeyModifiers::CONTROL),
        ] {
            let translated = bindings
                .translate(
                    "dialog",
                    KeyEvent::new(KeyCode::Char(letter), KeyModifiers::CONTROL),
                )
                .unwrap();
            assert_eq!(translated, KeyEvent::new(canonical, modifiers));
            for extra in [
                KeyModifiers::SHIFT,
                KeyModifiers::ALT,
                KeyModifiers::SHIFT | KeyModifiers::ALT,
                KeyModifiers::SUPER,
                KeyModifiers::HYPER,
                KeyModifiers::META,
            ] {
                assert!(
                    bindings
                        .translate(
                            "dialog",
                            KeyEvent::new(
                                KeyCode::Char(letter.to_ascii_uppercase()),
                                KeyModifiers::CONTROL | extra
                            )
                        )
                        .is_none(),
                    "Ctrl+{extra:?}+{letter} collapsed into another group"
                );
            }
        }
        let bindings: Bindings = serde_yaml_ng::from_str("menu: {help: [Ctrl+Alt+M]}").unwrap();
        bindings.validate(&[]).unwrap();
        assert_eq!(
            bindings
                .translate(
                    "menu",
                    KeyEvent::new(
                        KeyCode::Char('m'),
                        KeyModifiers::CONTROL | KeyModifiers::ALT
                    )
                )
                .unwrap()
                .code,
            KeyCode::F(1)
        );
        assert_eq!(bindings.hint("menu", "help"), "Ctrl+Alt+M");
    }

    #[test]
    fn system_modified_letters_do_not_fall_through_to_text_entry() {
        let bindings = Bindings::default();
        for scope in ["menu", "dialog", "editor"] {
            for modifiers in [KeyModifiers::NONE, KeyModifiers::SHIFT] {
                let event = KeyEvent::new(KeyCode::Char('A'), modifiers);
                assert_eq!(bindings.translate(scope, event), Some(event));
            }
            for modifier in [KeyModifiers::SUPER, KeyModifiers::HYPER, KeyModifiers::META] {
                for extra in [
                    KeyModifiers::NONE,
                    KeyModifiers::SHIFT,
                    KeyModifiers::CONTROL,
                    KeyModifiers::ALT,
                ] {
                    assert!(
                        bindings
                            .translate(scope, KeyEvent::new(KeyCode::Char('a'), modifier | extra))
                            .is_none()
                    );
                }
            }
        }
    }
}
