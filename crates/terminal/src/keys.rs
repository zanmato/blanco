//! Keystrokes to the byte sequences a VT-style terminal expects (xterm
//! conventions). Anything a shell or TUI program would receive from a real
//! terminal is encoded here; keystrokes with the platform (command/super)
//! modifier are left to the application.

use alacritty_terminal::term::TermMode;
use gpui::{Keystroke, Modifiers};

const ESCAPE: u8 = 0x1b;

/// The xterm modifier parameter (`CSI 1 ; <modifier> X`): 1 plus shift (1),
/// alt (2) and control (4). `None` when no modifier is held, since plain keys
/// use the short form.
fn xterm_modifier(modifiers: &Modifiers) -> Option<u8> {
    let value = 1
        + u8::from(modifiers.shift)
        + 2 * u8::from(modifiers.alt)
        + 4 * u8::from(modifiers.control);
    (value > 1).then_some(value)
}

/// Cursor keys and Home/End: `ESC [ X`, or `ESC O X` in application cursor
/// mode, or `ESC [ 1 ; m X` with a modifier.
fn cursor_key(final_byte: u8, modifiers: &Modifiers, mode: TermMode) -> Vec<u8> {
    match xterm_modifier(modifiers) {
        Some(modifier) => format!("\x1b[1;{modifier}{}", final_byte as char).into_bytes(),
        None if mode.contains(TermMode::APP_CURSOR) => vec![ESCAPE, b'O', final_byte],
        None => vec![ESCAPE, b'[', final_byte],
    }
}

/// Editing keys and F5 and up: `ESC [ n ~`, or `ESC [ n ; m ~` with a modifier.
fn tilde_key(number: u8, modifiers: &Modifiers) -> Vec<u8> {
    match xterm_modifier(modifiers) {
        Some(modifier) => format!("\x1b[{number};{modifier}~").into_bytes(),
        None => format!("\x1b[{number}~").into_bytes(),
    }
}

/// F1 to F4: `ESC O X`, or `ESC [ 1 ; m X` with a modifier.
fn function_key(final_byte: u8, modifiers: &Modifiers) -> Vec<u8> {
    match xterm_modifier(modifiers) {
        Some(modifier) => format!("\x1b[1;{modifier}{}", final_byte as char).into_bytes(),
        None => vec![ESCAPE, b'O', final_byte],
    }
}

/// The control character a `ctrl` chord produces, following the ASCII layout
/// where control clears the two high bits of the letter.
fn control_byte(character: char) -> Option<u8> {
    match character {
        'a'..='z' => Some(character as u8 & 0x1f),
        'A'..='Z' => Some(character.to_ascii_lowercase() as u8 & 0x1f),
        ' ' | '@' | '2' => Some(0x00),
        '[' | '3' => Some(0x1b),
        '\\' | '4' => Some(0x1c),
        ']' | '5' => Some(0x1d),
        '^' | '6' => Some(0x1e),
        '_' | '7' | '-' | '/' => Some(0x1f),
        '8' | '?' => Some(0x7f),
        _ => None,
    }
}

/// Prefix with ESC when alt is held, the "meta sends escape" convention.
fn with_alt(mut bytes: Vec<u8>, modifiers: &Modifiers) -> Vec<u8> {
    if modifiers.alt {
        bytes.insert(0, ESCAPE);
    }
    bytes
}

/// Encode a keystroke for the terminal, `None` when it is not terminal input
/// (a bare modifier, a platform shortcut, an unmapped named key).
pub(crate) fn encode(keystroke: &Keystroke, mode: TermMode) -> Option<Vec<u8>> {
    let modifiers = &keystroke.modifiers;
    if modifiers.platform || modifiers.function {
        return None;
    }

    let named = match keystroke.key.as_str() {
        "enter" => Some(with_alt(vec![b'\r'], modifiers)),
        "backspace" => {
            let byte = if modifiers.control { 0x08 } else { 0x7f };
            Some(with_alt(vec![byte], modifiers))
        }
        "tab" => Some(if modifiers.shift {
            b"\x1b[Z".to_vec()
        } else {
            with_alt(vec![b'\t'], modifiers)
        }),
        "escape" => Some(with_alt(vec![ESCAPE], modifiers)),
        "space" => Some(if modifiers.control {
            with_alt(vec![0x00], modifiers)
        } else {
            with_alt(vec![b' '], modifiers)
        }),
        "insert" => Some(tilde_key(2, modifiers)),
        "delete" => Some(tilde_key(3, modifiers)),
        "pageup" => Some(tilde_key(5, modifiers)),
        "pagedown" => Some(tilde_key(6, modifiers)),
        "home" => Some(cursor_key(b'H', modifiers, mode)),
        "end" => Some(cursor_key(b'F', modifiers, mode)),
        "up" => Some(cursor_key(b'A', modifiers, mode)),
        "down" => Some(cursor_key(b'B', modifiers, mode)),
        "right" => Some(cursor_key(b'C', modifiers, mode)),
        "left" => Some(cursor_key(b'D', modifiers, mode)),
        "f1" => Some(function_key(b'P', modifiers)),
        "f2" => Some(function_key(b'Q', modifiers)),
        "f3" => Some(function_key(b'R', modifiers)),
        "f4" => Some(function_key(b'S', modifiers)),
        "f5" => Some(tilde_key(15, modifiers)),
        "f6" => Some(tilde_key(17, modifiers)),
        "f7" => Some(tilde_key(18, modifiers)),
        "f8" => Some(tilde_key(19, modifiers)),
        "f9" => Some(tilde_key(20, modifiers)),
        "f10" => Some(tilde_key(21, modifiers)),
        "f11" => Some(tilde_key(23, modifiers)),
        "f12" => Some(tilde_key(24, modifiers)),
        _ => None,
    };
    if named.is_some() {
        return named;
    }

    // Everything else is a printable key. `key_char` is the text the platform
    // produced for the chord (layout and shift applied); the bare key name is
    // the fallback for single characters when the platform gave none.
    let text = keystroke
        .key_char
        .clone()
        .or_else(|| (keystroke.key.chars().count() == 1).then(|| keystroke.key.clone()))?;

    if modifiers.control {
        let mut characters = keystroke.key.chars();
        let character = characters.next()?;
        if characters.next().is_some() {
            return None;
        }
        return Some(with_alt(vec![control_byte(character)?], modifiers));
    }

    Some(with_alt(text.into_bytes(), modifiers))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keystroke(text: &str) -> Keystroke {
        Keystroke::parse(text).expect("keystroke parses")
    }

    fn encoded(text: &str, mode: TermMode) -> Vec<u8> {
        let mut keystroke = keystroke(text);
        if keystroke.key.chars().count() == 1 && !keystroke.modifiers.control {
            keystroke.key_char = Some(keystroke.key.clone());
        }
        encode(&keystroke, mode).expect("keystroke encodes")
    }

    #[test]
    fn printable_keys_pass_through() {
        assert_eq!(encoded("a", TermMode::empty()), b"a");
        assert_eq!(encoded("alt-a", TermMode::empty()), b"\x1ba");
        assert_eq!(encoded("enter", TermMode::empty()), b"\r");
        assert_eq!(encoded("space", TermMode::empty()), b" ");
    }

    #[test]
    fn control_chords_become_control_characters() {
        assert_eq!(encoded("ctrl-c", TermMode::empty()), vec![0x03]);
        assert_eq!(encoded("ctrl-z", TermMode::empty()), vec![0x1a]);
        assert_eq!(encoded("ctrl-[", TermMode::empty()), vec![0x1b]);
        assert_eq!(encoded("ctrl-space", TermMode::empty()), vec![0x00]);
        assert_eq!(encoded("ctrl-alt-d", TermMode::empty()), vec![0x1b, 0x04]);
    }

    #[test]
    fn cursor_keys_honor_application_mode_and_modifiers() {
        assert_eq!(encoded("up", TermMode::empty()), b"\x1b[A");
        assert_eq!(encoded("up", TermMode::APP_CURSOR), b"\x1bOA");
        assert_eq!(encoded("shift-left", TermMode::APP_CURSOR), b"\x1b[1;2D");
        assert_eq!(encoded("ctrl-right", TermMode::empty()), b"\x1b[1;5C");
        assert_eq!(encoded("home", TermMode::empty()), b"\x1b[H");
    }

    #[test]
    fn editing_and_function_keys() {
        assert_eq!(encoded("delete", TermMode::empty()), b"\x1b[3~");
        assert_eq!(encoded("shift-delete", TermMode::empty()), b"\x1b[3;2~");
        assert_eq!(encoded("pageup", TermMode::empty()), b"\x1b[5~");
        assert_eq!(encoded("f1", TermMode::empty()), b"\x1bOP");
        assert_eq!(encoded("ctrl-f1", TermMode::empty()), b"\x1b[1;5P");
        assert_eq!(encoded("f5", TermMode::empty()), b"\x1b[15~");
        assert_eq!(encoded("f12", TermMode::empty()), b"\x1b[24~");
        assert_eq!(encoded("shift-tab", TermMode::empty()), b"\x1b[Z");
        assert_eq!(encoded("backspace", TermMode::empty()), vec![0x7f]);
        assert_eq!(encoded("ctrl-backspace", TermMode::empty()), vec![0x08]);
    }

    #[test]
    fn platform_shortcuts_are_left_alone() {
        let mut keystroke = keystroke("cmd-c");
        keystroke.key_char = Some("c".to_string());
        assert_eq!(encode(&keystroke, TermMode::empty()), None);
    }
}
