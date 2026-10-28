//! Names for bindable keys, in `horizon-client-input`'s format:
//! `key.keyboard.w`, `key.keyboard.left.shift`, `key.mouse.right`.
//!
//! The names are the file format of the bindings file, so they are stable;
//! `winit`'s `KeyCode` names are not. Only keys a camera or the viewer can
//! reasonably use are listed; a key missing here cannot be written to the file,
//! and the defaults above it are covered.
//!
//! Mouse names live in [`super::bindings::mouse_name`] and
//! [`super::bindings::mouse_button`].

use winit::keyboard::KeyCode;

/// `(name, key)` pairs. Names follow vanilla's `InputConstants` spelling so a
/// Horizon bindings file reads the same as a voxel-viewer one.
const KEYS: &[(&str, KeyCode)] = &[
    ("key.keyboard.a", KeyCode::KeyA),
    ("key.keyboard.b", KeyCode::KeyB),
    ("key.keyboard.c", KeyCode::KeyC),
    ("key.keyboard.d", KeyCode::KeyD),
    ("key.keyboard.e", KeyCode::KeyE),
    ("key.keyboard.f", KeyCode::KeyF),
    ("key.keyboard.g", KeyCode::KeyG),
    ("key.keyboard.h", KeyCode::KeyH),
    ("key.keyboard.i", KeyCode::KeyI),
    ("key.keyboard.j", KeyCode::KeyJ),
    ("key.keyboard.k", KeyCode::KeyK),
    ("key.keyboard.l", KeyCode::KeyL),
    ("key.keyboard.m", KeyCode::KeyM),
    ("key.keyboard.n", KeyCode::KeyN),
    ("key.keyboard.o", KeyCode::KeyO),
    ("key.keyboard.p", KeyCode::KeyP),
    ("key.keyboard.q", KeyCode::KeyQ),
    ("key.keyboard.r", KeyCode::KeyR),
    ("key.keyboard.s", KeyCode::KeyS),
    ("key.keyboard.t", KeyCode::KeyT),
    ("key.keyboard.u", KeyCode::KeyU),
    ("key.keyboard.v", KeyCode::KeyV),
    ("key.keyboard.w", KeyCode::KeyW),
    ("key.keyboard.x", KeyCode::KeyX),
    ("key.keyboard.y", KeyCode::KeyY),
    ("key.keyboard.z", KeyCode::KeyZ),
    ("key.keyboard.0", KeyCode::Digit0),
    ("key.keyboard.1", KeyCode::Digit1),
    ("key.keyboard.2", KeyCode::Digit2),
    ("key.keyboard.3", KeyCode::Digit3),
    ("key.keyboard.4", KeyCode::Digit4),
    ("key.keyboard.5", KeyCode::Digit5),
    ("key.keyboard.6", KeyCode::Digit6),
    ("key.keyboard.7", KeyCode::Digit7),
    ("key.keyboard.8", KeyCode::Digit8),
    ("key.keyboard.9", KeyCode::Digit9),
    ("key.keyboard.escape", KeyCode::Escape),
    ("key.keyboard.enter", KeyCode::Enter),
    ("key.keyboard.tab", KeyCode::Tab),
    ("key.keyboard.backspace", KeyCode::Backspace),
    ("key.keyboard.space", KeyCode::Space),
    ("key.keyboard.minus", KeyCode::Minus),
    ("key.keyboard.equal", KeyCode::Equal),
    ("key.keyboard.left.bracket", KeyCode::BracketLeft),
    ("key.keyboard.right.bracket", KeyCode::BracketRight),
    ("key.keyboard.backslash", KeyCode::Backslash),
    ("key.keyboard.semicolon", KeyCode::Semicolon),
    ("key.keyboard.apostrophe", KeyCode::Quote),
    ("key.keyboard.grave.accent", KeyCode::Backquote),
    ("key.keyboard.comma", KeyCode::Comma),
    ("key.keyboard.period", KeyCode::Period),
    ("key.keyboard.slash", KeyCode::Slash),
    ("key.keyboard.caps.lock", KeyCode::CapsLock),
    ("key.keyboard.insert", KeyCode::Insert),
    ("key.keyboard.delete", KeyCode::Delete),
    ("key.keyboard.home", KeyCode::Home),
    ("key.keyboard.end", KeyCode::End),
    ("key.keyboard.page.up", KeyCode::PageUp),
    ("key.keyboard.page.down", KeyCode::PageDown),
    ("key.keyboard.up", KeyCode::ArrowUp),
    ("key.keyboard.down", KeyCode::ArrowDown),
    ("key.keyboard.left", KeyCode::ArrowLeft),
    ("key.keyboard.right", KeyCode::ArrowRight),
    ("key.keyboard.f1", KeyCode::F1),
    ("key.keyboard.f2", KeyCode::F2),
    ("key.keyboard.f3", KeyCode::F3),
    ("key.keyboard.f4", KeyCode::F4),
    ("key.keyboard.f5", KeyCode::F5),
    ("key.keyboard.f6", KeyCode::F6),
    ("key.keyboard.f7", KeyCode::F7),
    ("key.keyboard.f8", KeyCode::F8),
    ("key.keyboard.f9", KeyCode::F9),
    ("key.keyboard.f10", KeyCode::F10),
    ("key.keyboard.f11", KeyCode::F11),
    ("key.keyboard.f12", KeyCode::F12),
    ("key.keyboard.left.shift", KeyCode::ShiftLeft),
    ("key.keyboard.right.shift", KeyCode::ShiftRight),
    ("key.keyboard.left.control", KeyCode::ControlLeft),
    ("key.keyboard.right.control", KeyCode::ControlRight),
    ("key.keyboard.left.alt", KeyCode::AltLeft),
    ("key.keyboard.right.alt", KeyCode::AltRight),
    ("key.keyboard.left.win", KeyCode::SuperLeft),
    ("key.keyboard.right.win", KeyCode::SuperRight),
    ("key.keyboard.print.screen", KeyCode::PrintScreen),
    ("key.keyboard.scroll.lock", KeyCode::ScrollLock),
    ("key.keyboard.pause", KeyCode::Pause),
    ("key.keyboard.num.lock", KeyCode::NumLock),
    ("key.keyboard.menu", KeyCode::ContextMenu),
    ("key.keyboard.keypad.divide", KeyCode::NumpadDivide),
    ("key.keyboard.keypad.multiply", KeyCode::NumpadMultiply),
    ("key.keyboard.keypad.subtract", KeyCode::NumpadSubtract),
    ("key.keyboard.keypad.add", KeyCode::NumpadAdd),
    ("key.keyboard.keypad.enter", KeyCode::NumpadEnter),
    ("key.keyboard.keypad.0", KeyCode::Numpad0),
    ("key.keyboard.keypad.1", KeyCode::Numpad1),
    ("key.keyboard.keypad.2", KeyCode::Numpad2),
    ("key.keyboard.keypad.3", KeyCode::Numpad3),
    ("key.keyboard.keypad.4", KeyCode::Numpad4),
    ("key.keyboard.keypad.5", KeyCode::Numpad5),
    ("key.keyboard.keypad.6", KeyCode::Numpad6),
    ("key.keyboard.keypad.7", KeyCode::Numpad7),
    ("key.keyboard.keypad.8", KeyCode::Numpad8),
    ("key.keyboard.keypad.9", KeyCode::Numpad9),
    ("key.keyboard.keypad.period", KeyCode::NumpadDecimal),
];

/// The key named `name`, if it is in the table.
pub fn code(name: &str) -> Option<KeyCode> {
    KEYS.iter()
        .find(|(key_name, _)| *key_name == name)
        .map(|(_, code)| *code)
}

/// The name of `code`, if it is in the table.
pub fn name(code: KeyCode) -> Option<&'static str> {
    KEYS.iter()
        .find(|(_, key_code)| *key_code == code)
        .map(|(name, _)| *name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_name_round_trips() {
        for (name, code) in KEYS {
            assert_eq!(self::name(*code), Some(*name));
            assert_eq!(self::code(name), Some(*code));
        }
    }

    #[test]
    fn names_are_unique() {
        for (index, (name, _)) in KEYS.iter().enumerate() {
            assert!(
                !KEYS[index + 1..].iter().any(|(other, _)| other == name),
                "duplicate {name}"
            );
        }
    }

    #[test]
    fn unknown_names_are_rejected() {
        assert_eq!(code("key.keyboard.nonexistent"), None);
        assert_eq!(code("w"), None);
    }
}
