//! The viewer's input layer: bindings, per-action press/click state and mouse
//! look.
//!
//! The shape follows `horizon-client-input`, cut down to what a camera needs:
//!
//! - [`Bindings`] maps a named [`Action`] to a physical key or mouse button,
//!   with defaults and a hand-editable file format.
//! - [`Input`] turns key, button and motion events into held actions, clicks
//!   and a per-frame [`Look`]. One key may drive several actions; clicks are
//!   queued, so a press that arrives between two frames is not lost.
//! - [`MouseLook`] accumulates relative motion and applies vanilla's
//!   sensitivity curve, optional cinematic smoothing and wheel steps.
//!
//! Gameplay code asks for actions, never keys, so a rebind needs no other
//! change. The window layer decides when the pointer is captured; motion that
//! arrives while it is free is dropped rather than banked.
//!
//! The defaults are `W`/`A`/`S`/`D`, `Space`/`Shift`, `Ctrl` to sprint, the
//! arrow keys to look, `Esc` to free the pointer (and capture it again on the
//! next press), `F11` for fullscreen, and `Q` to quit. A bindings file can be
//! loaded from
//! the path in `VOXEL_BINDINGS`; see [`Bindings::from_text`]. The
//! viewer reads the look options from `VOXEL_SENSITIVITY`,
//! `VOXEL_SMOOTH_CAMERA`, `VOXEL_INVERT_X` and `VOXEL_INVERT_Y`; see
//! [`LookOptions`].
//!
//! Adding an action means one variant in [`Action`], its entry in
//! [`Action::ALL`], and its name, label and default key; the bindings file
//! gains a row automatically.

pub mod action;
pub mod bindings;
mod keynames;
pub mod mouse;

pub use action::Action;
pub use bindings::{Bindings, Key};
pub use mouse::{
    CONFINED_SWEEP_DEGREES, Look, LookOptions, MouseLook, PIXELS_PER_STEP, sensitivity_for_sweep,
};

use std::time::Duration;

use winit::event::MouseButton;
use winit::keyboard::KeyCode;

/// What a key or button event does.
///
/// A held key repeats: `winit` reports the repeats as their own events, which
/// are [`KeyAction::Repeat`] and do not count as another click.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyAction {
    /// The key went down.
    Press,
    /// A held key's auto-repeat.
    Repeat,
    /// The key came up.
    Release,
}

/// One action's runtime state.
#[derive(Clone, Copy, Debug, Default)]
struct State {
    /// Held down, or latched when `toggle` is set.
    down: bool,
    /// Presses not yet consumed by [`Input::clicked`].
    clicks: u32,
    /// A press flips `down` instead of setting it; a release is ignored.
    toggle: bool,
}

impl State {
    fn apply(&mut self, event: KeyAction) {
        match event {
            KeyAction::Press => {
                self.down = if self.toggle { !self.down } else { true };
                self.clicks = self.clicks.saturating_add(1);
            }
            // A repeat is still "down": the press already set that.
            KeyAction::Repeat => {}
            KeyAction::Release => {
                if !self.toggle {
                    self.down = false;
                }
            }
        }
    }
}

/// Bindings plus what is currently held, clicked and looked.
#[derive(Clone, Debug)]
pub struct Input {
    bindings: Bindings,
    states: [State; Action::COUNT],
    look: MouseLook,
    captured: bool,
}

impl Default for Input {
    fn default() -> Self {
        Self::new(Bindings::default())
    }
}

impl Input {
    /// Creates input state for `bindings`, with the default look options.
    pub fn new(bindings: Bindings) -> Self {
        Self::with_look(bindings, LookOptions::default())
    }

    /// Creates input state for `bindings` with `options` for mouse look.
    pub fn with_look(bindings: Bindings, options: LookOptions) -> Self {
        Self {
            bindings,
            states: [State::default(); Action::COUNT],
            look: MouseLook::new(options),
            captured: false,
        }
    }

    /// Replaces the look settings.
    pub fn set_look_options(&mut self, options: LookOptions) {
        self.look.set_options(options);
    }

    /// Whether `action` is held (or latched, when it is toggleable).
    pub fn is_down(&self, action: Action) -> bool {
        self.states[action.index()].down
    }

    /// Consumes one pending click of `action`, if there is one.
    ///
    /// Edge-triggered actions (quit, capture) use this instead of `is_down`:
    /// a press that happens between frames still fires once.
    pub fn clicked(&mut self, action: Action) -> bool {
        let state = &mut self.states[action.index()];
        if state.clicks == 0 {
            false
        } else {
            state.clicks -= 1;
            true
        }
    }

    /// A keyboard event, for every action bound to that key.
    pub fn key(&mut self, code: KeyCode, event: KeyAction) {
        self.press(Key::Keyboard(code), event);
    }

    /// A mouse-button event, for every action bound to that button.
    pub fn mouse_button(&mut self, button: MouseButton, event: KeyAction) {
        self.press(Key::Mouse(button), event);
    }

    fn press(&mut self, key: Key, event: KeyAction) {
        // Split the borrow: the iterator borrows the bindings while the states
        // are written.
        let Self {
            bindings, states, ..
        } = self;
        for action in bindings.actions_for(key) {
            states[action.index()].apply(event);
        }
    }

    /// Relative pointer motion, in physical pixels, for the next
    /// [`Input::look`] call.
    pub fn motion(&mut self, dx: f64, dy: f64) {
        self.look.add_motion(dx, dy);
    }

    /// Wheel motion in steps (see [`mouse::PIXELS_PER_STEP`]), returning the
    /// whole vertical steps available.
    pub fn scroll(&mut self, x: f64, y: f64) -> i32 {
        self.look.scroll(x, y)
    }

    /// This frame's turn in radians, or `None` while the pointer is free.
    ///
    /// Must be called once per frame even when the result is unused: it
    /// consumes the accumulated motion and advances the smoothed turn.
    pub fn look(&mut self, delta: Duration) -> Option<Look> {
        self.look.turn(delta.as_secs_f64(), self.captured)
    }

    /// Drops every held action and pending click.
    ///
    /// For focus loss: a key released while another window had focus would
    /// otherwise stay down, and a click from before the switch should not fire
    /// when focus returns.
    pub fn release_all(&mut self) {
        for state in &mut self.states {
            *state = State {
                toggle: state.toggle,
                ..State::default()
            };
        }
    }

    /// Makes `action` latch instead of hold: a press flips it, a release does
    /// nothing. Only continuous actions such as [`Action::Sprint`] can be
    /// latched; returns `false` for the others, which are edge-triggered.
    pub fn set_toggleable(&mut self, action: Action) -> bool {
        if !action.is_toggleable() {
            return false;
        }
        self.states[action.index()].toggle = true;
        true
    }

    /// Records whether the pointer is captured. Mouse look only turns while it
    /// is.
    pub fn set_captured(&mut self, captured: bool) {
        self.captured = captured;
    }

    /// Whether the pointer is captured.
    pub fn captured(&self) -> bool {
        self.captured
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::keyboard::KeyCode;

    fn press(input: &mut Input, code: KeyCode) {
        input.key(code, KeyAction::Press);
    }

    #[test]
    fn a_key_holds_until_release() {
        let mut input = Input::default();
        press(&mut input, KeyCode::KeyW);
        assert!(input.is_down(Action::Forward));
        input.key(KeyCode::KeyW, KeyAction::Repeat);
        assert!(input.is_down(Action::Forward));
        input.key(KeyCode::KeyW, KeyAction::Release);
        assert!(!input.is_down(Action::Forward));
    }

    #[test]
    fn repeats_do_not_queue_more_clicks() {
        let mut input = Input::default();
        press(&mut input, KeyCode::KeyQ);
        input.key(KeyCode::KeyQ, KeyAction::Repeat);
        assert!(input.clicked(Action::Quit));
        assert!(!input.clicked(Action::Quit));
    }

    #[test]
    fn unbound_keys_do_nothing() {
        let mut input = Input::default();
        press(&mut input, KeyCode::KeyZ);
        assert!(!input.is_down(Action::Forward));
        assert!(!input.clicked(Action::Forward));
    }

    #[test]
    fn one_key_drives_every_action_bound_to_it() {
        let mut bindings = Bindings::default();
        bindings.set(Action::Sprint, bindings.get(Action::Forward));
        let mut input = Input::new(bindings);
        press(&mut input, KeyCode::KeyW);
        assert!(input.is_down(Action::Forward));
        assert!(input.is_down(Action::Sprint));
        assert!(input.clicked(Action::Forward));
        assert!(input.clicked(Action::Sprint));
    }

    #[test]
    fn mouse_buttons_are_bindings_too() {
        let mut bindings = Bindings::default();
        bindings.set(Action::ToggleCapture, Some(Key::Mouse(MouseButton::Right)));
        let mut input = Input::new(bindings);
        input.mouse_button(MouseButton::Right, KeyAction::Press);
        assert!(input.clicked(Action::ToggleCapture));
        input.mouse_button(MouseButton::Right, KeyAction::Release);
        assert!(!input.is_down(Action::ToggleCapture));
    }

    #[test]
    fn toggleable_actions_latch_until_pressed_again() {
        let mut input = Input::default();
        input.set_toggleable(Action::Sprint);
        press(&mut input, KeyCode::ControlLeft);
        assert!(input.is_down(Action::Sprint));
        input.key(KeyCode::ControlLeft, KeyAction::Release);
        assert!(
            input.is_down(Action::Sprint),
            "a release must not clear a latch"
        );
        press(&mut input, KeyCode::ControlLeft);
        assert!(!input.is_down(Action::Sprint));
    }

    #[test]
    fn release_all_drops_holds_clicks_and_latches() {
        let mut input = Input::default();
        input.set_toggleable(Action::Sprint);
        press(&mut input, KeyCode::KeyW);
        press(&mut input, KeyCode::KeyQ);
        press(&mut input, KeyCode::ControlLeft);
        input.release_all();
        assert!(!input.is_down(Action::Forward));
        assert!(!input.is_down(Action::Sprint));
        assert!(!input.clicked(Action::Quit));
        // The toggle option survives, so sprint still latches afterwards.
        press(&mut input, KeyCode::ControlLeft);
        assert!(input.is_down(Action::Sprint));
    }

    #[test]
    fn look_only_turns_while_captured() {
        let mut input = Input::default();
        input.motion(100.0, 0.0);
        assert_eq!(input.look(Duration::from_secs_f32(1.0 / 60.0)), None);
        input.set_captured(true);
        // Motion from before capture was dropped, not banked.
        let look = input.look(Duration::from_secs_f32(1.0 / 60.0)).unwrap();
        assert_eq!(look.yaw, 0.0);
        input.motion(100.0, 0.0);
        let look = input.look(Duration::from_secs_f32(1.0 / 60.0)).unwrap();
        assert!(look.yaw > 0.0);
    }

    #[test]
    fn only_continuous_actions_can_be_toggleable() {
        let mut input = Input::default();
        assert!(input.set_toggleable(Action::Sprint));
        assert!(!input.set_toggleable(Action::Quit));
    }
}
