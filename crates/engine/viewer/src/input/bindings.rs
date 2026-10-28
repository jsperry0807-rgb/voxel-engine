//! Bindings from [`Action`] to a key or mouse button, and their file format.
//!
//! One action has at most one binding; one key may drive several actions (a
//! player who binds sprint to `W` walks and sprints at once, as vanilla's
//! `KeyMapping` does). Rebinding is a plain assignment, and conflicts are
//! reported rather than refused, so a player can stage a rebind in two steps.
//!
//! The file is line-oriented and hand-editable:
//!
//! ```text
//! # voxel-viewer bindings
//! forward = key.keyboard.w
//! toggle_capture = key.mouse.right
//! ```
//!
//! Unknown lines and keys are reported as warnings and skipped; the rest of the
//! file still applies. `key.keyboard.unknown` leaves an action unbound.

use std::fmt;

use winit::event::MouseButton;
use winit::keyboard::KeyCode;

use super::Action;
use super::keynames;

/// The value that leaves an action unbound.
pub const UNBOUND: &str = "key.keyboard.unknown";

/// The happy-path default bindings file, for documentation and tests.
#[cfg(test)]
const SAMPLE: &str = "\
# voxel-viewer bindings
forward = key.keyboard.w
sprint = key.mouse.4
quit = key.keyboard.unknown
";

/// A physical input a binding can name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    /// A keyboard key, matched by physical position so layouts agree.
    Keyboard(KeyCode),
    /// A mouse button.
    Mouse(MouseButton),
}

impl Key {
    /// Parses a name such as `key.keyboard.left.shift` or `key.mouse.4`.
    /// [`UNBOUND`] is not a key: it is handled by the caller.
    pub fn parse(name: &str) -> Option<Self> {
        if name.starts_with("key.mouse.") {
            return mouse_button(name).map(Self::Mouse);
        }
        keynames::code(name).map(Self::Keyboard)
    }

    /// The name this key is written as.
    pub fn name(self) -> String {
        match self {
            Self::Keyboard(code) => keynames::name(code).unwrap_or(UNBOUND).to_owned(),
            Self::Mouse(button) => mouse_name(button),
        }
    }
}

impl fmt::Display for Key {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.name())
    }
}

/// The bindings-file name of a mouse button.
pub fn mouse_name(button: MouseButton) -> String {
    match button {
        MouseButton::Left => "key.mouse.left".to_owned(),
        MouseButton::Right => "key.mouse.right".to_owned(),
        MouseButton::Middle => "key.mouse.middle".to_owned(),
        MouseButton::Back => "key.mouse.4".to_owned(),
        MouseButton::Forward => "key.mouse.5".to_owned(),
        MouseButton::Other(number) => format!("key.mouse.{}", number + 1),
    }
}

/// The button named `name`, when it has a fixed name.
pub fn mouse_button(name: &str) -> Option<MouseButton> {
    Some(match name {
        "key.mouse.left" => MouseButton::Left,
        "key.mouse.right" => MouseButton::Right,
        "key.mouse.middle" => MouseButton::Middle,
        "key.mouse.4" => MouseButton::Back,
        "key.mouse.5" => MouseButton::Forward,
        _ => return None,
    })
}

/// One key (or nothing) per action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bindings {
    keys: [Option<Key>; Action::COUNT],
}

impl Default for Bindings {
    fn default() -> Self {
        let mut keys = [None; Action::COUNT];
        for action in Action::ALL {
            keys[action.index()] = Some(action.default_key());
        }
        Self { keys }
    }
}

impl Bindings {
    /// The key bound to `action`, or `None` when it is unbound.
    pub fn get(&self, action: Action) -> Option<Key> {
        self.keys[action.index()]
    }

    /// Binds `action` to `key`, or unbinds it with `None`.
    pub fn set(&mut self, action: Action, key: Option<Key>) {
        self.keys[action.index()] = key;
    }

    /// The key the action starts on; every action has one.
    pub fn default_of(action: Action) -> Key {
        action.default_key()
    }

    /// Whether the action is still on its default key.
    pub fn is_default(&self, action: Action) -> bool {
        self.get(action) == Some(Self::default_of(action))
    }

    /// Every action bound to `key`, in [`Action::ALL`] order. One press drives
    /// all of them.
    pub fn actions_for(&self, key: Key) -> impl Iterator<Item = Action> + '_ {
        Action::ALL
            .into_iter()
            .filter(move |action| self.get(*action) == Some(key))
    }

    /// The other actions sharing this action's key, for a conflict mark.
    ///
    /// Two actions on the same key are not a conflict when both are still on
    /// their defaults: as in vanilla, the second press would be the same
    /// button twice, not a mistake.
    pub fn conflicts(&self, action: Action) -> impl Iterator<Item = Action> + '_ {
        let key = self.get(action);
        Action::ALL.into_iter().filter(move |other| {
            *other != action
                && key.is_some()
                && self.get(*other) == key
                && (!self.is_default(*other) || !self.is_default(action))
        })
    }

    /// Serializes the bindings in [`Action::ALL`] order. The trailing comment
    /// names the action; [`Bindings::from_text`] ignores it.
    pub fn to_text(self) -> String {
        let mut text = String::from("# voxel-viewer bindings\n");
        for action in Action::ALL {
            let key = self
                .get(action)
                .map_or_else(|| UNBOUND.to_owned(), Key::name);
            text.push_str(action.name());
            text.push_str(" = ");
            text.push_str(&key);
            text.push_str("  # ");
            text.push_str(action.label());
            text.push('\n');
        }
        text
    }

    /// Reads bindings from `text`, starting from the defaults.
    ///
    /// Returns the bindings and one message per line or conflict that was not
    /// applied. A malformed file never fails outright: whatever parsed is
    /// usable and the defaults cover the rest.
    pub fn from_text(text: &str) -> (Self, Vec<String>) {
        let mut bindings = Self::default();
        let mut warnings = Vec::new();
        for (number, line) in text.lines().enumerate() {
            let number = number + 1;
            let line = line.split('#').next().unwrap_or_default().trim();
            if line.is_empty() {
                continue;
            }
            let Some((name, value)) = line.split_once('=') else {
                warnings.push(format!("line {number}: expected `action = key`"));
                continue;
            };
            let name = name.trim();
            let value = value.trim();
            let Some(action) = Action::from_name(name) else {
                warnings.push(format!("line {number}: unknown action `{name}`"));
                continue;
            };
            if value == UNBOUND {
                bindings.set(action, None);
                continue;
            }
            let Some(key) = Key::parse(value) else {
                warnings.push(format!("line {number}: unknown key `{value}`"));
                continue;
            };
            bindings.set(action, Some(key));
        }
        for action in Action::ALL {
            for other in bindings.conflicts(action) {
                // One message per pair, whichever order the action was set in.
                if other.index() > action.index() {
                    warnings.push(format!(
                        "`{}` and `{}` are both bound to {}",
                        action.name(),
                        other.name(),
                        bindings
                            .get(action)
                            .map_or_else(|| UNBOUND.to_owned(), Key::name)
                    ));
                }
            }
        }
        (bindings, warnings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_the_actions_own_keys() {
        let bindings = Bindings::default();
        for action in Action::ALL {
            assert_eq!(bindings.get(action), Some(action.default_key()));
            assert!(bindings.is_default(action));
        }
    }

    #[test]
    fn text_round_trips() {
        let bindings = Bindings::default();
        let (parsed, warnings) = Bindings::from_text(&bindings.to_text());
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(parsed, bindings);
    }

    #[test]
    fn sample_edits_apply_and_comment_out() {
        let (bindings, warnings) = Bindings::from_text(SAMPLE);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(
            bindings.get(Action::Forward),
            Some(Action::Forward.default_key())
        );
        assert_eq!(
            bindings.get(Action::Sprint),
            Some(Key::Mouse(MouseButton::Back))
        );
        assert_eq!(bindings.get(Action::Quit), None);
        assert!(!bindings.is_default(Action::Sprint));
    }

    #[test]
    fn unbound_and_unknown_entries_are_reported() {
        let (bindings, warnings) = Bindings::from_text(
            "forward = key.keyboard.nonexistent\n\
             jump = key.keyboard.space\n\
             backward = key.keyboard.s\n",
        );
        assert_eq!(warnings.len(), 3, "{warnings:?}");
        assert_eq!(bindings, Bindings::default());
    }

    #[test]
    fn one_key_can_drive_two_actions() {
        let mut bindings = Bindings::default();
        bindings.set(Action::Sprint, bindings.get(Action::Forward));
        let forward = bindings.get(Action::Forward).unwrap();
        let actions: Vec<_> = bindings.actions_for(forward).collect();
        assert_eq!(actions, [Action::Forward, Action::Sprint]);
        let conflicts: Vec<_> = bindings.conflicts(Action::Forward).collect();
        assert_eq!(conflicts, [Action::Sprint]);
    }

    #[test]
    fn conflicts_are_reported_once_per_pair() {
        let (_, warnings) = Bindings::from_text(
            "forward = key.keyboard.r\n\
             back = key.keyboard.r\n",
        );
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("forward") && warnings[0].contains("back"));
    }

    #[test]
    fn mouse_names_round_trip() {
        for button in [
            MouseButton::Left,
            MouseButton::Right,
            MouseButton::Middle,
            MouseButton::Back,
            MouseButton::Forward,
            MouseButton::Other(7),
        ] {
            let key = Key::Mouse(button);
            if button == MouseButton::Other(7) {
                // Only the five named buttons parse back.
                assert_eq!(Key::parse(&key.name()), None);
                assert_eq!(key.name(), "key.mouse.8");
            } else {
                assert_eq!(Key::parse(&key.name()), Some(key));
            }
        }
    }

    #[test]
    fn unbound_actions_do_not_conflict() {
        let mut bindings = Bindings::default();
        bindings.set(Action::Forward, None);
        bindings.set(Action::Back, None);
        assert_eq!(bindings.conflicts(Action::Forward).count(), 0);
        assert_eq!(
            bindings.actions_for(Key::Keyboard(KeyCode::KeyW)).count(),
            0
        );
    }
}
