//! How text that `box`, git or an agent wrote is shown. A task's name, a branch, a file's path are data:
//! they are shown, never trusted. Control characters (a newline in a file name), bidirectional controls
//! and zero-width characters can hide part of a name or make a warning read differently from what it says
//! ("Trojan Source"), so every label, tooltip and warning shows them as a visible mark. This is for the
//! eye only: what identifies a thing or is passed to `box` stays as it was written.

/// Whether `c` cannot be seen or changes how the text around it reads: a control character, a
/// bidirectional control or a zero-width character.
pub fn is_hidden(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{061c}'
                | '\u{200b}'..='\u{200f}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2060}'..='\u{2064}'
                | '\u{2066}'..='\u{2069}'
                | '\u{feff}'
        )
}

/// `text` as a label shows it: every hidden character as the replacement mark, on one line.
pub fn label(text: &str) -> String {
    let shown = |c: char| match is_hidden(c) {
        true => char::REPLACEMENT_CHARACTER,
        false => c,
    };
    text.chars().map(shown).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_that_hides_part_of_itself_is_shown_with_marks() {
        assert_eq!(label("safe\u{202e}txt.exe"), "safe\u{fffd}txt.exe");
        assert_eq!(label("a\u{200b}b\u{feff}"), "a\u{fffd}b\u{fffd}");
    }

    #[test]
    fn a_name_with_a_line_break_or_an_escape_stays_on_one_line() {
        assert_eq!(label("spec.ts\nNo problem"), "spec.ts\u{fffd}No problem");
        assert_eq!(label("a\u{1b}[31mb\tc"), "a\u{fffd}[31mb\u{fffd}c");
    }

    #[test]
    fn ordinary_names_are_shown_as_they_are() {
        assert_eq!(label("box/cafés-26"), "box/cafés-26");
    }
}
