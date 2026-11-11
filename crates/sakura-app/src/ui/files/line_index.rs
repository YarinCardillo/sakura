//! Where the lines of an open file start, to go between a line's number and its place in the text: the
//! marks of the gutter are given by line and the kit's editor takes them by byte, and a selection is a
//! byte range the person reads as lines.

use std::ops::Range;

/// The byte each line of a text starts at. A text that ends with a line break has no line after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineIndex {
    starts: Vec<usize>,
    len: usize,
    ends_with_break: bool,
}

impl LineIndex {
    /// The index of `text`.
    pub fn of(text: &str) -> Self {
        let after_breaks = text.match_indices('\n').map(|(at, _)| at + 1);
        let mut starts: Vec<usize> = std::iter::once(0).chain(after_breaks).collect();
        if starts.len() > 1 && starts.last() == Some(&text.len()) {
            starts.pop();
        }
        Self {
            starts,
            len: text.len(),
            ends_with_break: text.ends_with('\n'),
        }
    }

    /// How many lines the text has: one at least.
    #[cfg(test)]
    pub fn line_count(&self) -> u32 {
        u32::try_from(self.starts.len()).unwrap_or(u32::MAX)
    }

    /// The byte line `line` (numbered from 1) starts at; the text's end for a line past the last.
    pub fn start(&self, line: u32) -> usize {
        let index = usize::try_from(line.saturating_sub(1)).unwrap_or(usize::MAX);
        self.starts.get(index).copied().unwrap_or(self.len)
    }

    /// The bytes of lines `first..=last` with the line breaks between them, never empty unless the
    /// text ends there: a line with nothing on it is its own line break.
    pub fn lines(&self, first: u32, last: u32) -> Range<usize> {
        let start = self.start(first);
        let next = self.start(last.max(first).saturating_add(1));
        let has_break = next < self.len || (next > start && self.ends_with_break);
        let end = if has_break { next - 1 } else { next };
        start..end.max((start + 1).min(self.len))
    }

    /// The line, numbered from 1, the byte `offset` is on.
    pub fn line_of(&self, offset: usize) -> u32 {
        let after = self.starts.partition_point(|start| *start <= offset);
        u32::try_from(after.max(1)).unwrap_or(u32::MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_start_after_each_break_and_a_final_break_starts_no_line() {
        let index = LineIndex::of("one\ntwo\n\nfour\n");
        assert_eq!(index.line_count(), 4);
        assert_eq!([index.start(1), index.start(2), index.start(3)], [0, 4, 8]);
        assert_eq!(index.start(4), 9);
        assert_eq!(index.start(9), 14, "past the last line: the end");
        assert_eq!(LineIndex::of("").line_count(), 1);
        assert_eq!(LineIndex::of("no break").line_count(), 1);
    }

    #[test]
    fn a_run_of_lines_is_their_text_and_an_empty_line_is_its_break() {
        let text = "one\ntwo\n\nfour\n";
        let index = LineIndex::of(text);
        assert_eq!(&text[index.lines(1, 1)], "one");
        assert_eq!(&text[index.lines(1, 2)], "one\ntwo");
        assert_eq!(&text[index.lines(3, 3)], "\n", "never an empty range");
        assert_eq!(&text[index.lines(4, 4)], "four");
        assert_eq!(&text[index.lines(2, 9)], "two\n\nfour", "to the last line");
        let unbroken = LineIndex::of("a\nb");
        assert_eq!(unbroken.lines(2, 2), 2..3);
        assert_eq!(LineIndex::of("").lines(1, 1), 0..0, "nothing to cover");
    }

    #[test]
    fn a_byte_is_on_the_line_that_starts_at_or_before_it() {
        let index = LineIndex::of("one\ntwo\n\nfour\n");
        assert_eq!(
            [index.line_of(0), index.line_of(3), index.line_of(4)],
            [1, 1, 2]
        );
        assert_eq!(
            [index.line_of(8), index.line_of(9), index.line_of(13)],
            [3, 4, 4]
        );
        assert_eq!(index.line_of(999), 4);
    }
}
