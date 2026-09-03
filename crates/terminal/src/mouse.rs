//! Mouse reports for programs that enable mouse tracking (`CSI ? 1000 h` and
//! friends): the classic `ESC [ M` three-byte form and the SGR
//! `ESC [ < b ; x ; y M` form, chosen by the terminal's current mode.

use alacritty_terminal::term::TermMode;
use gpui::{Modifiers, MouseButton};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MouseReport {
    Press(MouseButton),
    Release(MouseButton),
    /// Pointer moved with `button` held.
    Drag(MouseButton),
    /// Pointer moved with no button held (only sent in all-motion mode).
    Motion,
    WheelUp,
    WheelDown,
}

fn button_code(button: MouseButton) -> Option<u8> {
    match button {
        MouseButton::Left => Some(0),
        MouseButton::Middle => Some(1),
        MouseButton::Right => Some(2),
        _ => None,
    }
}

fn modifier_bits(modifiers: &Modifiers) -> u8 {
    4 * u8::from(modifiers.shift) + 8 * u8::from(modifiers.alt) + 16 * u8::from(modifiers.control)
}

/// Encode `report` at zero-based grid position (`column`, `row`), or `None`
/// when the terminal's mode does not ask for this kind of event.
pub(crate) fn encode(
    report: MouseReport,
    column: usize,
    row: usize,
    modifiers: &Modifiers,
    mode: TermMode,
) -> Option<Vec<u8>> {
    if !mode.intersects(TermMode::MOUSE_MODE) {
        return None;
    }

    let (code, release) = match report {
        MouseReport::Press(button) => (button_code(button)?, false),
        MouseReport::Release(button) => (button_code(button)?, true),
        MouseReport::Drag(button) => {
            if !mode.intersects(TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION) {
                return None;
            }
            (button_code(button)? + 32, false)
        }
        MouseReport::Motion => {
            if !mode.contains(TermMode::MOUSE_MOTION) {
                return None;
            }
            (3 + 32, false)
        }
        MouseReport::WheelUp => (64, false),
        MouseReport::WheelDown => (65, false),
    };
    let code = code + modifier_bits(modifiers);

    if mode.contains(TermMode::SGR_MOUSE) {
        let suffix = if release { 'm' } else { 'M' };
        return Some(format!("\x1b[<{code};{};{}{suffix}", column + 1, row + 1).into_bytes());
    }

    // The classic form has no release button: 3 means "a button went up".
    let code = if release {
        3 + modifier_bits(modifiers)
    } else {
        code
    };
    let mut bytes = b"\x1b[M".to_vec();
    bytes.push(32 + code);
    if mode.contains(TermMode::UTF8_MOUSE) {
        push_utf8_coordinate(&mut bytes, column);
        push_utf8_coordinate(&mut bytes, row);
    } else {
        // Coordinates beyond 223 cannot be expressed in one byte; clamp rather
        // than wrap into control characters.
        bytes.push((32 + column + 1).min(255) as u8);
        bytes.push((32 + row + 1).min(255) as u8);
    }
    Some(bytes)
}

fn push_utf8_coordinate(bytes: &mut Vec<u8>, coordinate: usize) {
    let value = (32 + coordinate + 1).min(2015) as u32;
    let mut buffer = [0u8; 4];
    if let Some(character) = char::from_u32(value) {
        bytes.extend_from_slice(character.encode_utf8(&mut buffer).as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silent_without_mouse_mode() {
        assert_eq!(
            encode(
                MouseReport::Press(MouseButton::Left),
                0,
                0,
                &Modifiers::default(),
                TermMode::empty()
            ),
            None
        );
    }

    #[test]
    fn classic_reports() {
        let mode = TermMode::MOUSE_REPORT_CLICK;
        assert_eq!(
            encode(
                MouseReport::Press(MouseButton::Left),
                0,
                0,
                &Modifiers::default(),
                mode
            ),
            Some(b"\x1b[M\x20\x21\x21".to_vec())
        );
        assert_eq!(
            encode(
                MouseReport::Release(MouseButton::Left),
                4,
                2,
                &Modifiers::default(),
                mode
            ),
            Some(b"\x1b[M\x23\x25\x23".to_vec())
        );
        assert_eq!(
            encode(MouseReport::WheelUp, 0, 0, &Modifiers::default(), mode),
            Some(b"\x1b[M\x60\x21\x21".to_vec())
        );
    }

    #[test]
    fn sgr_reports_and_modifiers() {
        let mode = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;
        assert_eq!(
            encode(
                MouseReport::Press(MouseButton::Right),
                9,
                4,
                &Modifiers::default(),
                mode
            ),
            Some(b"\x1b[<2;10;5M".to_vec())
        );
        assert_eq!(
            encode(
                MouseReport::Release(MouseButton::Left),
                0,
                0,
                &Modifiers::shift(),
                mode
            ),
            Some(b"\x1b[<4;1;1m".to_vec())
        );
        assert_eq!(
            encode(
                MouseReport::Drag(MouseButton::Left),
                0,
                0,
                &Modifiers::default(),
                mode
            ),
            None,
            "drag needs button-motion tracking"
        );
        assert_eq!(
            encode(
                MouseReport::Drag(MouseButton::Left),
                0,
                0,
                &Modifiers::default(),
                mode | TermMode::MOUSE_DRAG
            ),
            Some(b"\x1b[<32;1;1M".to_vec())
        );
    }
}
