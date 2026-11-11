//! The rows under an open file: hunk headers (muted), code lines (Geist Mono, old and new numbers in a muted
//! gutter, added and removed lines tinted at 10 percent of the success and danger tones with a bar at the left
//! edge, drawn by [`CodeLine`]), and the one sentence of a file without lines. Code is never cut: a line too
//! long for the pane wraps (see [`super::wrap`]).

use super::code_line::{CodeLine, CodeRow, LineStyle};
use crate::theme::{self, Palette};
use crate::ui::chrome::mono_features;
use gpui::{div, px, AnyElement, Hsla, IntoElement, ParentElement, Pixels, ShapedLine, Styled};
use sakura_core::patch::{Line, LineKind};
use std::ops::Range;
use std::rc::Rc;
use unicode_width::UnicodeWidthChar;

/// What every row of one frame shares.
pub struct CodeLayout {
    pub palette: Palette,
    /// The colours and metrics of code lines.
    pub line: Rc<LineStyle>,
}

impl CodeLayout {
    /// Where code starts, from the row's left edge: the two number columns and the padding.
    pub fn code_start(&self) -> Pixels {
        self.line.code_start()
    }
}

/// A code line's shaped rows and its old and new numbers.
pub type ShapedNumbers = (Rc<[CodeRow]>, Option<ShapedLine>, Option<ShapedLine>);

/// One code line, `selected` where the list's selection covers it.
pub fn code_line(
    line: &Line,
    shaped: ShapedNumbers,
    selected: Option<(Range<usize>, bool)>,
    layout: &CodeLayout,
) -> AnyElement {
    let (rows, old, new) = shaped;
    CodeLine {
        rows,
        old,
        new,
        tone: line_tone(line.kind, &layout.palette),
        selected,
        has_no_newline: line.has_no_newline,
        style: layout.line.clone(),
    }
    .into_any_element()
}

/// A hunk's `@@` line, muted, aligned with the code.
pub fn hunk_header(text: &str, layout: &CodeLayout) -> AnyElement {
    div()
        .w_full()
        .h(px(theme::HUNK_HEADER_HEIGHT))
        .flex()
        .items_center()
        .pl(layout.code_start())
        .pr(theme::DIFF_CODE_PADDING)
        .font_family(theme::FONT_MONO)
        .font_features(mono_features())
        .text_size(theme::DIFF_TEXT)
        .text_color(layout.palette.text_tertiary)
        .child(div().min_w_0().truncate().child(display_text(text)))
        .into_any_element()
}

/// The sentence of a file that has no lines to show.
pub fn note(sentence: String, layout: &CodeLayout) -> AnyElement {
    div()
        .w_full()
        .h(px(theme::DIFF_NOTE_HEIGHT))
        .flex()
        .items_center()
        .pl(layout.code_start())
        .pr(theme::DIFF_CODE_PADDING)
        .text_size(theme::TEXT_SMALL)
        .text_color(layout.palette.text_tertiary)
        .child(div().min_w_0().truncate().child(sentence))
        .into_any_element()
}

/// The tone of a changed line; none for context.
fn line_tone(kind: LineKind, palette: &Palette) -> Option<Hsla> {
    match kind {
        LineKind::Context => None,
        LineKind::Added => Some(palette.success),
        LineKind::Removed => Some(palette.danger),
    }
}

/// The columns one character takes in the monospaced font: two when wide, none for a mark that combines
/// with the letter before it, else one (the shown text has no control characters). Tab stops, wrapping
/// and the count of a line's columns all read this one measure.
pub(super) fn char_columns(c: char) -> usize {
    c.width().unwrap_or(1)
}

/// The text a line, path or subject shows: tabs expanded to the next stop, and what cannot be seen made
/// visible as a replacement mark, by the one rule for untrusted text (`sakura_core::display`).
pub fn display_text(text: &str) -> String {
    let mut shown = String::with_capacity(text.len());
    let mut column = 0;
    for c in text.chars() {
        match c {
            '\t' => {
                let spaces = theme::DIFF_TAB_WIDTH - column % theme::DIFF_TAB_WIDTH;
                shown.extend(std::iter::repeat_n(' ', spaces));
                column += spaces;
            }
            c if sakura_core::display::is_hidden(c) => {
                shown.push(char::REPLACEMENT_CHARACTER);
                column += 1;
            }
            c => {
                shown.push(c);
                column += char_columns(c);
            }
        }
    }
    shown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_expand_to_the_next_stop() {
        assert_eq!(display_text("\tx"), "    x");
        assert_eq!(display_text("ab\tc"), "ab  c");
    }

    #[test]
    fn a_tab_stop_is_counted_in_columns_not_in_characters() {
        assert_eq!(
            display_text("日\tx"),
            "日  x",
            "a wide character took two columns"
        );
        assert_eq!(
            display_text("e\u{301}\tx"),
            "e\u{301}   x",
            "a mark took none"
        );
    }

    #[test]
    fn control_characters_are_shown_as_replacement_marks() {
        assert_eq!(display_text("a\u{1b}[31mb"), "a\u{fffd}[31mb");
        assert_eq!(display_text("\u{0}"), "\u{fffd}");
    }

    #[test]
    fn bidi_and_invisible_characters_are_made_visible() {
        let trojan = "if (admin\u{202e} \u{2066}) {\u{2069}\u{200b}\u{feff}";
        assert_eq!(
            display_text(trojan),
            "if (admin\u{fffd} \u{fffd}) {\u{fffd}\u{fffd}\u{fffd}"
        );
        assert_eq!(display_text("a\u{200e}b\u{061c}"), "a\u{fffd}b\u{fffd}");
    }

    #[test]
    fn plain_text_is_unchanged() {
        assert_eq!(display_text("cafés = 0;"), "cafés = 0;");
    }
}
