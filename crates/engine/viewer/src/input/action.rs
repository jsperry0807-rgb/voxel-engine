//! The actions the viewer listens for, and their defaults.
//!
//! Game code asks for an [`Action`], never for a key: `Forward` is whatever the
//! player bound it to. The order of [`Action::ALL`] is the order of the states
//! in [`crate::input::Input`], so [`Action::index`] must match it; a test
//! asserts that.
//!
//! This mirrors `horizon-client-input`'s key mappings in miniature: a closed
//! set of named actions, each with one default key, resolved through
//! [`crate::input::bindings::Bindings`].

use winit::keyboard::KeyCode;

use super::bindings::Key;

/// Something the viewer can react to, independent of the key bound to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Move along the view direction.
    Forward,
    /// Move against the view direction.
    Back,
    /// Strafe left.
    Left,
    /// Strafe right.
    Right,
    /// Fly straight up.
    Up,
    /// Fly straight down.
    Down,
    /// Turn left without moving the pointer.
    LookLeft,
    /// Turn right without moving the pointer.
    LookRight,
    /// Look up without moving the pointer.
    LookUp,
    /// Look down without moving the pointer.
    LookDown,
    /// Move faster while held (or toggled, see
    /// [`crate::input::Input::set_toggleable`]).
    Sprint,
    /// Release or recapture the pointer. Edge-triggered: one click per press.
    ToggleCapture,
    /// Switch between windowed and borderless fullscreen. Edge-triggered.
    ToggleFullscreen,
    /// Stop the viewer. Edge-triggered.
    Quit,
}

impl Action {
    /// Every action, in [`Action::index`] order.
    pub const ALL: [Self; 14] = [
        Self::Forward,
        Self::Back,
        Self::Left,
        Self::Right,
        Self::Up,
        Self::Down,
        Self::LookLeft,
        Self::LookRight,
        Self::LookUp,
        Self::LookDown,
        Self::Sprint,
        Self::ToggleCapture,
        Self::ToggleFullscreen,
        Self::Quit,
    ];

    /// Number of actions; the size of the per-action state arrays.
    pub const COUNT: usize = Self::ALL.len();

    /// The name used in the bindings file.
    pub fn name(self) -> &'static str {
        match self {
            Self::Forward => "forward",
            Self::Back => "back",
            Self::Left => "left",
            Self::Right => "right",
            Self::Up => "up",
            Self::Down => "down",
            Self::LookLeft => "look_left",
            Self::LookRight => "look_right",
            Self::LookUp => "look_up",
            Self::LookDown => "look_down",
            Self::Sprint => "sprint",
            Self::ToggleCapture => "toggle_capture",
            Self::ToggleFullscreen => "fullscreen",
            Self::Quit => "quit",
        }
    }

    /// A human-readable name for logs and documentation.
    pub fn label(self) -> &'static str {
        match self {
            Self::Forward => "Forward",
            Self::Back => "Back",
            Self::Left => "Left",
            Self::Right => "Right",
            Self::Up => "Up",
            Self::Down => "Down",
            Self::LookLeft => "Look left",
            Self::LookRight => "Look right",
            Self::LookUp => "Look up",
            Self::LookDown => "Look down",
            Self::Sprint => "Sprint",
            Self::ToggleCapture => "Toggle capture",
            Self::ToggleFullscreen => "Fullscreen",
            Self::Quit => "Quit",
        }
    }

    /// The key this action starts on.
    pub fn default_key(self) -> Key {
        match self {
            Self::Forward => Key::Keyboard(KeyCode::KeyW),
            Self::Back => Key::Keyboard(KeyCode::KeyS),
            Self::Left => Key::Keyboard(KeyCode::KeyA),
            Self::Right => Key::Keyboard(KeyCode::KeyD),
            Self::Up => Key::Keyboard(KeyCode::Space),
            Self::Down => Key::Keyboard(KeyCode::ShiftLeft),
            Self::LookLeft => Key::Keyboard(KeyCode::ArrowLeft),
            Self::LookRight => Key::Keyboard(KeyCode::ArrowRight),
            Self::LookUp => Key::Keyboard(KeyCode::ArrowUp),
            Self::LookDown => Key::Keyboard(KeyCode::ArrowDown),
            Self::Sprint => Key::Keyboard(KeyCode::ControlLeft),
            Self::ToggleCapture => Key::Keyboard(KeyCode::F11),
            Self::ToggleFullscreen => Key::Keyboard(KeyCode::F12),
            Self::Quit => Key::Keyboard(KeyCode::Escape),
        }
    }

    /// Whether the action can be switched between hold and toggle.
    ///
    /// Only [`Action::Sprint`] is offered: flight is the one continuous
    /// action a player may want to latch. Edge-triggered actions have no held
    /// state to toggle.
    pub fn is_toggleable(self) -> bool {
        matches!(self, Self::Sprint)
    }

    /// Index into the per-action arrays; see [`Action::ALL`].
    pub fn index(self) -> usize {
        self as usize
    }

    /// Looks up an action by its [`Action::name`].
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|action| action.name() == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_matches_all_order() {
        for (index, action) in Action::ALL.into_iter().enumerate() {
            assert_eq!(action.index(), index, "{action:?}");
        }
    }

    #[test]
    fn names_round_trip_and_are_unique() {
        for action in Action::ALL {
            assert_eq!(Action::from_name(action.name()), Some(action));
        }
        for (index, action) in Action::ALL.into_iter().enumerate() {
            assert!(
                !Action::ALL[index + 1..]
                    .iter()
                    .any(|other| other.name() == action.name()),
                "duplicate name {}",
                action.name()
            );
        }
    }

    #[test]
    fn default_keys_are_distinct() {
        for (index, action) in Action::ALL.into_iter().enumerate() {
            for other in Action::ALL[index + 1..].iter() {
                assert_ne!(
                    action.default_key(),
                    other.default_key(),
                    "{action:?} and {other:?}"
                );
            }
        }
    }
}
