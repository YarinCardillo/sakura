use unicode_properties::{GeneralCategory, UnicodeGeneralCategory};

pub const CHARACTER_LIMIT: usize = 2000;
pub const REFUSAL_PREFIX: &str = "invalid note:";
pub const HIDDEN_CHARACTER_REASON: &str =
    "refused text with a hidden character (bidi, zero-width, tag or unassigned)";

pub enum Problem {
    TooLong,
    HiddenCharacter,
    ControlCharacter,
}

/// Checks the cockpit Resume input. An empty draft represents an omitted note.
pub fn problem(note: &str) -> Option<Problem> {
    if note.chars().take(CHARACTER_LIMIT + 1).count() > CHARACTER_LIMIT {
        return Some(Problem::TooLong);
    }
    note.chars().find_map(|character| match character {
        '\n' | '\t' => None,
        c if c.is_control() => Some(Problem::ControlCharacter),
        c if crate::display::is_hidden(c)
            || matches!(c, '\u{e0000}'..='\u{e007f}')
            || c.general_category() == GeneralCategory::Unassigned =>
        {
            Some(Problem::HiddenCharacter)
        }
        _ => None,
    })
}
