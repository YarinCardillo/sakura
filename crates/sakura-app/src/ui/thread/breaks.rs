//! The line breaks of the agent's words. Markdown reads a single line break inside a paragraph as a
//! space, so three short lines the agent wrote under each other would be drawn as one. Claude's own apps
//! keep them, and the agent writes for those: here every line break that ends a line of text is made a
//! hard one (two spaces before it), which the markdown view draws as a break. Code inside a fence is left
//! as it is, and so are blank lines and the fences' own lines.
//!
//! The text only ever grows and may arrive cut anywhere, so the reading keeps the line it is in and adds
//! the two spaces when the line's break arrives: what was already given never changes.

use super::segments::{closes, fence_start};

/// The start of a line that is kept to tell a fence from text: a fence's own line is short.
const LINE_START_MAX: usize = 64;
/// What makes a line break a hard one in markdown.
const HARD_BREAK: &str = "  ";

/// Reads the agent's words as they grow and keeps their line breaks.
#[derive(Debug, Default)]
pub(super) struct Breaks {
    /// The fence that is open, as its character and length.
    fence: Option<(char, usize)>,
    /// The start of the line being read, and whether the line went on past what is kept of it.
    line: String,
    is_long: bool,
}

impl Breaks {
    /// `more` of the text, with the line breaks that end a line of text made hard.
    pub(super) fn keep(&mut self, more: &str) -> String {
        let mut kept = String::with_capacity(more.len());
        for piece in more.split_inclusive('\n') {
            let (body, is_ended) = match piece.strip_suffix('\n') {
                Some(body) => (body, true),
                None => (piece, false),
            };
            self.read(body);
            kept.push_str(body);
            if is_ended {
                if self.ends_a_line_of_text() {
                    kept.push_str(HARD_BREAK);
                }
                kept.push('\n');
                self.line.clear();
                self.is_long = false;
            }
        }
        kept
    }

    /// Notes more of the line being read.
    fn read(&mut self, body: &str) {
        let room = LINE_START_MAX.saturating_sub(self.line.len());
        let end = (0..=room.min(body.len()))
            .rev()
            .find(|at| body.is_char_boundary(*at))
            .unwrap_or(0);
        self.line.push_str(&body[..end]);
        self.is_long |= end < body.len();
    }

    /// Whether the line that just ended is text whose break is kept; a fence that opens or closes with
    /// it is noted.
    fn ends_a_line_of_text(&mut self) -> bool {
        let line = self.line.trim_end_matches('\r');
        if let Some(open) = self.fence {
            if !self.is_long && closes(line, open) {
                self.fence = None;
            }
            return false;
        }
        self.fence = fence_start(line);
        self.fence.is_none() && !line.trim().is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kept(feeds: &[&str]) -> String {
        let mut breaks = Breaks::default();
        feeds.iter().map(|feed| breaks.keep(feed)).collect()
    }

    #[test]
    fn keep_lines_of_text_under_each_other_stay_under_each_other() {
        assert_eq!(kept(&["one\ntwo\nthree"]), "one  \ntwo  \nthree");
    }

    #[test]
    fn keep_blank_lines_and_lists_are_what_they_were_but_for_the_hard_breaks() {
        let text = "First.\n\n- one\n- two\n\nSecond.\n";
        assert_eq!(kept(&[text]), "First.  \n\n- one  \n- two  \n\nSecond.  \n");
    }

    #[test]
    fn keep_code_inside_a_fence_and_the_fences_own_lines_are_untouched() {
        let text = "Here:\n```rust\nfn a() {}\n\nfn b() {}\n```\nDone.\nthus\n";
        assert_eq!(
            kept(&[text]),
            "Here:  \n```rust\nfn a() {}\n\nfn b() {}\n```\nDone.  \nthus  \n"
        );
    }

    #[test]
    fn keep_a_long_line_of_backticks_inside_a_fence_does_not_close_it() {
        let long = "`".repeat(LINE_START_MAX + 8);
        let text = format!("~~~\n{long} x\nmore code\n~~~\ntext\n");
        assert_eq!(
            kept(&[&text]),
            format!("~~~\n{long} x\nmore code\n~~~\ntext  \n")
        );
    }

    #[test]
    fn keep_text_cut_anywhere_gives_what_the_whole_text_gives() {
        let text = "First\nsecond café line\n\n```sh\nls -la\n```\nthird\n- a\n- b\nend";
        let whole = kept(&[text]);
        for at in (0..=text.len()).filter(|at| text.is_char_boundary(*at)) {
            assert_eq!(kept(&[&text[..at], &text[at..]]), whole, "cut at {at}");
        }
        let bytes: Vec<String> = text.chars().map(String::from).collect();
        let pieces: Vec<&str> = bytes.iter().map(String::as_str).collect();
        assert_eq!(kept(&pieces), whole, "a character at a time");
    }
}
