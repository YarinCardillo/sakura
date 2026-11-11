//! How a code line too long for the pane wraps: onto continuation rows that start under the code column,
//! indented by a hanging indent. Rows are counted in columns of the monospaced font (a wide character takes
//! two, a combining mark none and so never leaves its letter), so the height of every line is known
//! without shaping it and the virtual list stays exact. A row holds no more columns than fit the width. It
//! breaks after a space when one sits in its second half, else at the column limit; the rows of a line
//! always add up to the whole line, so nothing is ever cut.

use super::line::char_columns;
use crate::theme;
use gpui::Pixels;
use std::ops::Range;

/// How many columns the first row of a line and each continuation row hold, and how many columns the
/// continuation rows are indented by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Wrap {
    pub first: usize,
    pub rest: usize,
    pub hang: usize,
}

impl Wrap {
    /// The wrap for code `width` wide in a font whose column is `advance` wide: as many columns as fit
    /// and never more, so no row is drawn past the list's edge; one at least, so every line ends. The
    /// hanging indent is kept only where it leaves room for a column.
    pub fn for_width(width: Pixels, advance: Pixels) -> Self {
        let fitting = (f32::from(width) / f32::from(advance)).floor().max(0.0) as usize;
        let first = fitting.max(1);
        let hang = match first > theme::DIFF_HANG_COLUMNS {
            true => theme::DIFF_HANG_COLUMNS,
            false => 0,
        };
        Self {
            first,
            rest: first - hang,
            hang,
        }
    }

    /// Before the list's width is known: no line wraps, so no line is read to count its rows.
    pub fn unknown() -> Self {
        Self {
            first: usize::MAX,
            rest: usize::MAX,
            hang: 0,
        }
    }

    /// How many columns row `row` of a line holds.
    pub fn columns(&self, row: usize) -> usize {
        if row == 0 {
            self.first
        } else {
            self.rest
        }
    }
}

/// What is known of a line before it is wrapped: its width in columns.
pub fn columns(text: &str) -> usize {
    text.chars().map(char_columns).sum()
}

/// The byte ranges of the rows `text` wraps onto: at least one, in order, adding up to the whole text.
pub fn rows(text: &str, wrap: Wrap) -> Vec<Range<usize>> {
    let mut rows = Vec::new();
    let mut row = Row::starting_at(0, 0);
    for (index, c) in text.char_indices() {
        let width = char_columns(c);
        let limit = wrap.columns(rows.len());
        if row.used + width > limit && index > row.start {
            let cut = row.break_in_second_half(limit).unwrap_or(index);
            rows.push(row.start..cut);
            row = Row::starting_at(cut, columns(&text[cut..index]));
        }
        row.used += width;
        if c == ' ' {
            row.after_space = Some((index + 1, row.used));
        }
    }
    rows.push(row.start..text.len());
    rows
}

/// The row being filled: where it starts, the columns used, and the place after its last space with the
/// columns used up to there.
struct Row {
    start: usize,
    used: usize,
    after_space: Option<(usize, usize)>,
}

impl Row {
    fn starting_at(start: usize, used: usize) -> Self {
        Self {
            start,
            used,
            after_space: None,
        }
    }

    /// The place after the row's last space, when it leaves at least half of `limit` on the row.
    fn break_in_second_half(&self, limit: usize) -> Option<usize> {
        self.after_space
            .filter(|(_, used)| used * 2 >= limit)
            .map(|(index, _)| index)
    }
}

/// How many rows a line of `columns` columns takes; `text` is read only when it does not fit one row.
pub fn row_count(columns: usize, text: impl FnOnce() -> String, wrap: Wrap) -> usize {
    if columns <= wrap.first {
        1
    } else {
        rows(&text(), wrap).len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::px;

    fn wrap(first: usize, rest: usize) -> Wrap {
        Wrap {
            first,
            rest,
            hang: first - rest,
        }
    }

    fn parts(text: &str, wrap: Wrap) -> Vec<&str> {
        rows(text, wrap).into_iter().map(|row| &text[row]).collect()
    }

    #[test]
    fn a_line_that_fits_is_one_row_and_an_empty_line_too() {
        assert_eq!(parts("let x = 1;", wrap(20, 18)), vec!["let x = 1;"]);
        assert_eq!(rows("", wrap(20, 18)), vec![0..0]);
        assert_eq!(parts("exactly-20-columns!!", wrap(20, 18)).len(), 1);
    }

    #[test]
    fn a_long_word_breaks_at_the_column_limit_and_continuation_rows_are_shorter() {
        assert_eq!(
            parts("abcdefghijklm", wrap(5, 3)),
            vec!["abcde", "fgh", "ijk", "lm"]
        );
    }

    #[test]
    fn a_row_breaks_after_a_space_in_its_second_half() {
        let text = "const value = compute(first, second);";
        assert_eq!(
            parts(text, wrap(16, 14)),
            vec!["const value = ", "compute(first,", " second);"]
        );
    }

    #[test]
    fn a_space_too_early_in_the_row_is_not_a_break() {
        assert_eq!(
            parts("a bcdefghijklmnop", wrap(8, 6)),
            vec!["a bcdefg", "hijklm", "nop"]
        );
    }

    #[test]
    fn wide_characters_take_two_columns() {
        assert_eq!(columns("日本語"), 6);
        assert_eq!(columns("cafés"), 5);
        assert_eq!(
            parts("日本語テキスト", wrap(4, 4)),
            vec!["日本", "語テ", "キス", "ト"]
        );
        assert_eq!(
            parts("a日本", wrap(4, 4)),
            vec!["a日", "本"],
            "never half a character"
        );
    }

    #[test]
    fn the_rows_of_a_line_add_up_to_the_whole_line() {
        let lines = [
            "",
            " ",
            "    return items.filter((item) => item.visible && item.owner === currentUser.id);",
            "cafés = «più» – 日本語 mixed with 🙂 and tabs    expanded",
            "x".repeat(600).as_str().to_owned().leak(),
        ];
        for line in lines {
            for first in [1, 2, 7, 40, 120] {
                let wrap = wrap(first, first.saturating_sub(2).max(1));
                let rows = rows(line, wrap);
                let joined: String = rows.iter().map(|row| &line[row.clone()]).collect();
                assert_eq!(joined, line, "first {first}");
                assert!(!rows.is_empty());
                let fits = |(index, row): (usize, &Range<usize>)| {
                    let text = &line[row.clone()];
                    columns(text) <= wrap.columns(index) || text.chars().count() == 1
                };
                assert!(rows.iter().enumerate().all(fits), "{line:?} first {first}");
            }
        }
    }

    #[test]
    fn the_row_count_of_a_short_line_never_reads_its_text() {
        let unread = || -> String { panic!("a line that fits is not read") };
        assert_eq!(row_count(10, unread, wrap(10, 8)), 1);
        assert_eq!(row_count(600, || "x".repeat(600), wrap(100, 98)), 7);
    }

    #[test]
    fn the_wrap_follows_the_width_with_a_hanging_indent() {
        let wrap = Wrap::for_width(px(725.), px(7.2));
        assert_eq!(wrap.first, 100);
        assert_eq!(wrap.rest, 100 - theme::DIFF_HANG_COLUMNS);
    }

    #[test]
    fn the_wrap_never_holds_more_columns_than_fit_the_width() {
        let narrow = Wrap::for_width(px(72.), px(7.2));
        assert_eq!(narrow.first, 10, "ten columns fit: ten are held, not more");
        assert_eq!(narrow.rest, 10 - theme::DIFF_HANG_COLUMNS);
        let tiny = Wrap::for_width(px(3.), px(7.2));
        assert_eq!(
            (tiny.first, tiny.rest),
            (1, 1),
            "always one, so the line still ends"
        );
    }

    #[test]
    fn a_combining_mark_takes_no_column_and_stays_on_the_row_of_its_letter() {
        assert_eq!(columns("e\u{301}"), 1);
        assert_eq!(parts("abe\u{301}", wrap(3, 3)), vec!["abe\u{301}"]);
        assert_eq!(
            parts("abce\u{301}d", wrap(3, 3)),
            vec!["abc", "e\u{301}d"],
            "the break falls before the letter, never between it and its mark"
        );
    }
}
