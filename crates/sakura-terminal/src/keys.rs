//! Keystrokes and pasted text as the bytes a terminal program expects.
//!
//! The escape sequence tables are adapted from Zed's `crates/terminal/src/mappings/keys.rs` (zed@d2a4fb6,
//! GPL-3.0-or-later, Copyright Zed Industries), which derives them from Alacritty's bindings.
//!
//! Modified for Sakura in November 2026: this file is a changed adaptation, not Zed's original.

use alacritty_terminal::term::TermMode;
use gpui::Keystroke;
use std::borrow::Cow;

const BRACKETED_PASTE_START: &str = "\x1b[200~";
const BRACKETED_PASTE_END: &str = "\x1b[201~";

#[derive(Debug, PartialEq, Eq)]
enum Mods {
    None,
    Alt,
    Ctrl,
    Shift,
    CtrlShift,
    Other,
}

impl Mods {
    fn of(keystroke: &Keystroke) -> Self {
        let m = &keystroke.modifiers;
        match (m.alt, m.control, m.shift, m.platform) {
            (false, false, false, false) => Mods::None,
            (true, false, false, false) => Mods::Alt,
            (false, true, false, false) => Mods::Ctrl,
            (false, false, true, false) => Mods::Shift,
            (false, true, true, false) => Mods::CtrlShift,
            _ => Mods::Other,
        }
    }
}

/// The bytes for `keystroke`, or `None` when it is plain text the input handler will insert.
///
/// With `option_as_meta`, Option plus an ASCII key sends ESC and the key (Alt as Meta); without it, Option
/// composes characters as macOS does (on an Italian keyboard Option+ò is `@`).
pub(crate) fn encode_keystroke(
    keystroke: &Keystroke,
    mode: TermMode,
    option_as_meta: bool,
) -> Option<Cow<'static, str>> {
    let mods = Mods::of(keystroke);
    let key = keystroke.key.as_str();
    if let Some(sequence) = special_key(key, &mods, mode) {
        return Some(Cow::Borrowed(sequence));
    }
    if let Some(sequence) = control_character(key, &mods) {
        return Some(Cow::Borrowed(sequence));
    }
    if mods != Mods::None {
        if let Some(sequence) = modified_key(key, modifier_code(keystroke)) {
            return Some(Cow::Owned(sequence));
        }
    }
    meta_key(keystroke, &mods, option_as_meta).map(Cow::Owned)
}

/// Text from the clipboard as the program should receive it.
///
/// When the program requested bracketed paste the text is wrapped in the brackets, with every escape
/// character removed so it cannot end the bracket early. Otherwise nothing in the paste can execute: every
/// control character except tab is removed and each line break becomes one space.
pub(crate) fn encode_paste(text: &str, mode: TermMode) -> Vec<u8> {
    if mode.contains(TermMode::BRACKETED_PASTE) {
        let body = text.replace('\x1b', "");
        format!("{BRACKETED_PASTE_START}{body}{BRACKETED_PASTE_END}").into_bytes()
    } else {
        inert_text(text).into_bytes()
    }
}

/// `text` without control characters (tab stays) and with each line break (CR, LF or CRLF) as one space.
fn inert_text(text: &str) -> String {
    let mut inert = String::with_capacity(text.len());
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '\r' => {
                characters.next_if_eq(&'\n');
                inert.push(' ');
            }
            '\n' => inert.push(' '),
            '\t' => inert.push('\t'),
            other if other.is_control() => {}
            other => inert.push(other),
        }
    }
    inert
}

fn special_key(key: &str, mods: &Mods, mode: TermMode) -> Option<&'static str> {
    let app_cursor = mode.contains(TermMode::APP_CURSOR);
    let sequence = match (key, mods) {
        ("tab", Mods::None) => "\x09",
        ("escape", Mods::None) => "\x1b",
        ("enter", Mods::None) => "\x0d",
        ("enter", Mods::Shift) => "\x0a",
        ("enter", Mods::Alt) => "\x1b\x0d",
        ("backspace", Mods::None | Mods::Shift) => "\x7f",
        ("tab", Mods::Shift) => "\x1b[Z",
        ("backspace", Mods::Ctrl) => "\x08",
        ("backspace", Mods::Alt) => "\x1b\x7f",
        ("space", Mods::Ctrl) => "\x00",
        ("home", Mods::None) => pick(app_cursor, "\x1bOH", "\x1b[H"),
        ("end", Mods::None) => pick(app_cursor, "\x1bOF", "\x1b[F"),
        ("up", Mods::None) => pick(app_cursor, "\x1bOA", "\x1b[A"),
        ("down", Mods::None) => pick(app_cursor, "\x1bOB", "\x1b[B"),
        ("right", Mods::None) => pick(app_cursor, "\x1bOC", "\x1b[C"),
        ("left", Mods::None) => pick(app_cursor, "\x1bOD", "\x1b[D"),
        ("insert", Mods::None) => "\x1b[2~",
        ("delete", Mods::None) => "\x1b[3~",
        ("pageup", Mods::None) => "\x1b[5~",
        ("pagedown", Mods::None) => "\x1b[6~",
        (function, Mods::None) => function_key(function)?,
        _ => return None,
    };
    Some(sequence)
}

fn pick(condition: bool, yes: &'static str, no: &'static str) -> &'static str {
    if condition {
        yes
    } else {
        no
    }
}

fn function_key(key: &str) -> Option<&'static str> {
    Some(match key {
        "f1" => "\x1bOP",
        "f2" => "\x1bOQ",
        "f3" => "\x1bOR",
        "f4" => "\x1bOS",
        "f5" => "\x1b[15~",
        "f6" => "\x1b[17~",
        "f7" => "\x1b[18~",
        "f8" => "\x1b[19~",
        "f9" => "\x1b[20~",
        "f10" => "\x1b[21~",
        "f11" => "\x1b[23~",
        "f12" => "\x1b[24~",
        _ => return None,
    })
}

/// Ctrl plus a letter or symbol: the C0 control character in caret notation.
fn control_character(key: &str, mods: &Mods) -> Option<&'static str> {
    const LETTERS: [&str; 26] = [
        "\x01", "\x02", "\x03", "\x04", "\x05", "\x06", "\x07", "\x08", "\x09", "\x0a", "\x0b",
        "\x0c", "\x0d", "\x0e", "\x0f", "\x10", "\x11", "\x12", "\x13", "\x14", "\x15", "\x16",
        "\x17", "\x18", "\x19", "\x1a",
    ];
    if !matches!(mods, Mods::Ctrl | Mods::CtrlShift) {
        return None;
    }
    let mut chars = key.chars();
    let (Some(character), None) = (chars.next(), chars.next()) else {
        return None;
    };
    if character.is_ascii_alphabetic() {
        let index = (character.to_ascii_lowercase() as u8 - b'a') as usize;
        return Some(LETTERS[index]);
    }
    match (character, mods) {
        ('@', Mods::Ctrl) => Some("\x00"),
        ('[', Mods::Ctrl) => Some("\x1b"),
        ('\\', Mods::Ctrl) => Some("\x1c"),
        (']', Mods::Ctrl) => Some("\x1d"),
        ('^', Mods::Ctrl) => Some("\x1e"),
        ('_', Mods::Ctrl) => Some("\x1f"),
        ('?', Mods::Ctrl) => Some("\x7f"),
        _ => None,
    }
}

/// Cursor, editing and function keys with modifiers, in the xterm `CSI 1;<mod>` form.
fn modified_key(key: &str, code: u32) -> Option<String> {
    let csi_letter = |letter: char| format!("\x1b[1;{code}{letter}");
    let csi_tilde = |number: u32| format!("\x1b[{number};{code}~");
    Some(match key {
        "up" => csi_letter('A'),
        "down" => csi_letter('B'),
        "right" => csi_letter('C'),
        "left" => csi_letter('D'),
        "end" => csi_letter('F'),
        "home" => csi_letter('H'),
        "f1" => csi_letter('P'),
        "f2" => csi_letter('Q'),
        "f3" => csi_letter('R'),
        "f4" => csi_letter('S'),
        "insert" => csi_tilde(2),
        "delete" => csi_tilde(3),
        "pageup" => csi_tilde(5),
        "pagedown" => csi_tilde(6),
        "f5" => csi_tilde(15),
        "f6" => csi_tilde(17),
        "f7" => csi_tilde(18),
        "f8" => csi_tilde(19),
        "f9" => csi_tilde(20),
        "f10" => csi_tilde(21),
        "f11" => csi_tilde(23),
        "f12" => csi_tilde(24),
        _ => return None,
    })
}

/// xterm's modifier parameter: 1 plus shift (1), alt (2) and control (4).
fn modifier_code(keystroke: &Keystroke) -> u32 {
    let m = &keystroke.modifiers;
    1 + u32::from(m.shift) + 2 * u32::from(m.alt) + 4 * u32::from(m.control)
}

/// Option plus an ASCII key as ESC followed by the key, when Option acts as Meta.
fn meta_key(keystroke: &Keystroke, mods: &Mods, option_as_meta: bool) -> Option<String> {
    if !option_as_meta || !keystroke.modifiers.alt || keystroke.modifiers.platform {
        return None;
    }
    let mut chars = keystroke.key.chars();
    let (Some(key), None) = (chars.next(), chars.next()) else {
        return None;
    };
    if !key.is_ascii() {
        return None;
    }
    if *mods == Mods::Alt {
        Some(format!("\x1b{key}"))
    } else if keystroke.modifiers.shift {
        Some(format!("\x1b{}", key.to_ascii_uppercase()))
    } else if keystroke.modifiers.control && key.is_ascii_lowercase() {
        Some(format!("\x1b{}", (key as u8 - b'a' + 1) as char))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(text: &'static str) -> Keystroke {
        Keystroke::parse(text).expect("valid keystroke")
    }

    fn encode(text: &'static str, mode: TermMode) -> Option<String> {
        encode_keystroke(&key(text), mode, false).map(Cow::into_owned)
    }

    #[test]
    fn keys_arrows_switch_with_application_cursor_mode() {
        assert_eq!(encode("up", TermMode::NONE).as_deref(), Some("\x1b[A"));
        assert_eq!(encode("left", TermMode::NONE).as_deref(), Some("\x1b[D"));
        assert_eq!(
            encode("up", TermMode::APP_CURSOR).as_deref(),
            Some("\x1bOA")
        );
        assert_eq!(
            encode("home", TermMode::APP_CURSOR).as_deref(),
            Some("\x1bOH")
        );
        assert_eq!(encode("end", TermMode::NONE).as_deref(), Some("\x1b[F"));
    }

    #[test]
    fn keys_modified_arrows_and_function_keys_use_xterm_parameters() {
        assert_eq!(
            encode("shift-up", TermMode::NONE).as_deref(),
            Some("\x1b[1;2A")
        );
        assert_eq!(
            encode("alt-left", TermMode::NONE).as_deref(),
            Some("\x1b[1;3D")
        );
        assert_eq!(
            encode("ctrl-right", TermMode::NONE).as_deref(),
            Some("\x1b[1;5C")
        );
        assert_eq!(encode("f1", TermMode::NONE).as_deref(), Some("\x1bOP"));
        assert_eq!(encode("f5", TermMode::NONE).as_deref(), Some("\x1b[15~"));
        assert_eq!(encode("f12", TermMode::NONE).as_deref(), Some("\x1b[24~"));
        assert_eq!(
            encode("shift-f5", TermMode::NONE).as_deref(),
            Some("\x1b[15;2~")
        );
        assert_eq!(encode("pageup", TermMode::NONE).as_deref(), Some("\x1b[5~"));
        assert_eq!(encode("delete", TermMode::NONE).as_deref(), Some("\x1b[3~"));
    }

    #[test]
    fn keys_control_letters_and_editing_keys() {
        assert_eq!(encode("ctrl-c", TermMode::NONE).as_deref(), Some("\x03"));
        assert_eq!(encode("ctrl-a", TermMode::NONE).as_deref(), Some("\x01"));
        assert_eq!(encode("ctrl-[", TermMode::NONE).as_deref(), Some("\x1b"));
        assert_eq!(encode("enter", TermMode::NONE).as_deref(), Some("\r"));
        assert_eq!(encode("backspace", TermMode::NONE).as_deref(), Some("\x7f"));
        assert_eq!(encode("escape", TermMode::NONE).as_deref(), Some("\x1b"));
        assert_eq!(
            encode("shift-tab", TermMode::NONE).as_deref(),
            Some("\x1b[Z")
        );
    }

    #[test]
    fn keys_plain_text_is_left_to_the_input_handler() {
        assert_eq!(encode("a", TermMode::NONE), None);
        assert_eq!(encode("shift-a", TermMode::NONE), None);
        assert_eq!(encode("cmd-c", TermMode::NONE), None);
    }

    #[test]
    fn keys_option_composes_unless_option_as_meta() {
        assert_eq!(encode("alt-b", TermMode::NONE), None);
        let meta = |text: &'static str| {
            encode_keystroke(&key(text), TermMode::NONE, true).map(Cow::into_owned)
        };
        assert_eq!(meta("alt-b").as_deref(), Some("\x1bb"));
        assert_eq!(meta("alt-shift-b").as_deref(), Some("\x1bB"));
        assert_eq!(meta("alt-ctrl-b").as_deref(), Some("\x1b\x02"));
        assert_eq!(meta("alt-up").as_deref(), Some("\x1b[1;3A"));
    }

    #[test]
    fn paste_without_bracketing_never_contains_a_line_break_or_control_character() {
        assert_eq!(encode_paste("a\nb", TermMode::NONE), b"a b");
        assert_eq!(encode_paste("a\r\nb\rc", TermMode::NONE), b"a b c");
        assert_eq!(
            encode_paste("printf pwned >/tmp/p\n", TermMode::NONE),
            b"printf pwned >/tmp/p "
        );
        assert_eq!(
            encode_paste("a\tb\x03c\x1b[Ad\x7fe\u{85}f\u{9b}g\0h", TermMode::NONE),
            "a\tbc[Adefgh".as_bytes()
        );
    }

    #[test]
    fn paste_in_bracketed_mode_keeps_lines_and_cannot_close_the_bracket() {
        assert_eq!(
            encode_paste("x\ny\r\nz", TermMode::BRACKETED_PASTE),
            b"\x1b[200~x\ny\r\nz\x1b[201~"
        );
        assert_eq!(
            encode_paste("x\x1b[201~y\n\x1b", TermMode::BRACKETED_PASTE),
            b"\x1b[200~x[201~y\n\x1b[201~"
        );
    }
}
