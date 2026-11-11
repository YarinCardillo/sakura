//! Cutting an answer's markdown into parts that can be drawn as separate rows, so a long answer costs what
//! is on screen and a streaming one lays out only its last part again. A cut is made only where drawing the
//! two sides separately gives the same picture as drawing them together:
//!
//! - at a blank line, outside a fenced code block;
//! - between two blocks that are neither a heading nor a rule on either side (the kit spaces those by what
//!   stands before them; every other pair of blocks is one paragraph gap apart, which the rows keep);
//! - never before a list item, a quote, indented text or an HTML line, which may continue the block above;
//! - never once the text defines a link reference or a footnote, which the parts would not share.
//!
//! The decision reads only complete lines before the cut and the start of the line after it, so a streamed
//! answer is cut at the same places as the same answer replayed whole. While the start of a line does not
//! yet say what it opens (`*` may open a list, a rule or bold text), that line is held back rather than
//! shown in a part it may not belong to; the blank lines before a cut are dropped.
//!
//! A block that goes on past what a part may hold (`ANSWER_PART_MAX_BYTES`: a file printed whole, one
//! endless line) is cut all the same, so that no part grows without end: before a line, once the part holds
//! half of what it may, and inside a line where the part reaches its limit. Such a cut can be seen (a paragraph
//! gap where there was none); a code block cut this way is closed before the cut and opened again after
//! it, so both halves are drawn as code. Where the cuts fall depends on the text alone, not on how it
//! arrived. A text that defines references is one part whatever its size.
//!
//! Known exception: a setext heading (a line of text with `===` or `---` under it) right after a blank line
//! is cut like the paragraph it looks like until its underline arrives, so it loses the wider gap a heading
//! gets above it. Models write headings with `#`.

use std::ops::Range;

/// The shortest run of backticks or tildes that opens a fence.
const FENCE_MIN: usize = 3;
/// The longest opening line of a fence that is written again, whole, after a cut inside its block.
const FENCE_LINE_MAX: usize = 120;

/// What to do with a growing answer, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Step {
    /// These bytes of the text belong to the current part.
    Text(Range<usize>),
    /// The current part is complete: what follows starts a new one.
    Cut,
    /// Text that is not in the answer, added so that a part stands on its own: the fence that closes a
    /// code block cut in two, and the one that opens its second half.
    Literal(String),
}

/// What the start of a line says about the block it opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Opening {
    /// More of the line is needed to tell.
    Undecided,
    /// A block that can stand in its own part: a paragraph, a table, a fenced code block.
    Cuttable,
    /// A block that must stay with what is above it, or is spaced by what is above it.
    Attached,
    /// A link reference or a footnote: the text is never cut again.
    Definition,
}

/// Reads an answer as it grows and says where it can be cut.
#[derive(Debug)]
pub(super) struct Segmenter {
    /// The most bytes a part holds before it is cut wherever it stands.
    part_limit: usize,
    /// How far the unfinished line was searched for its end: the next reading goes on from there.
    scanned: usize,
    /// Bytes searched for line ends so far.
    #[cfg(test)]
    searched: usize,
    /// Bytes of the text read so far: always the start of a line.
    read: usize,
    /// Bytes of the text handed out in steps so far.
    given: usize,
    /// Where the blank lines of the current gap start.
    gap_start: usize,
    /// Where the current part starts in the text.
    part_start: usize,
    /// The fence that is open, as its character and length, and the line that opened it.
    fence: Option<(char, usize)>,
    fence_line: String,
    /// Whether the last complete line was blank, and the non-blank line before it allows a cut after it.
    is_after_gap: bool,
    can_cut_after_last: bool,
    /// The text defines references: it is one part from here on.
    is_whole: bool,
}

impl Default for Segmenter {
    fn default() -> Self {
        Self::with_part_limit(crate::theme::thread::ANSWER_PART_MAX_BYTES)
    }
}

impl Segmenter {
    /// A reading that lets a part hold at most `part_limit` bytes.
    pub(super) fn with_part_limit(part_limit: usize) -> Self {
        Self {
            part_limit,
            scanned: 0,
            #[cfg(test)]
            searched: 0,
            read: 0,
            given: 0,
            gap_start: 0,
            part_start: 0,
            fence: None,
            fence_line: String::new(),
            is_after_gap: false,
            can_cut_after_last: false,
            is_whole: false,
        }
    }

    /// Reads what `text` has gained since the last call and says what to do with it. `text` only ever
    /// grows. A line whose start does not yet say what it opens is held back until it does.
    pub(super) fn feed(&mut self, text: &str) -> Vec<Step> {
        let mut steps = Vec::new();
        let mut held_from = None;
        while !self.is_whole {
            let line_end = self.line_end(text);
            let line = &text[self.read..line_end.map_or(text.len(), |end| self.read + end)];
            // One more blank line only makes the gap longer.
            let is_blank = line_end.is_some() && line.trim().is_empty();
            let is_judged = self.is_after_gap && self.fence.is_none() && self.can_cut_after_last;
            if is_judged && !is_blank {
                match opening(line, line_end.is_some()) {
                    Opening::Undecided => {
                        held_from = Some(self.gap_start);
                        break;
                    }
                    Opening::Cuttable => {
                        self.hand(&mut steps, self.gap_start);
                        steps.push(Step::Cut);
                        self.given = self.read;
                        self.part_start = self.read;
                    }
                    Opening::Attached => {}
                    Opening::Definition => self.is_whole = true,
                }
                // The line is judged once: until the next gap nothing is cut.
                self.is_after_gap = false;
            }
            if !self.is_whole {
                self.keep_part_within_limit(text, &mut steps);
            }
            let Some(end) = self.line_end(text) else {
                break;
            };
            self.read_line(&text[self.read..self.read + end]);
            self.read += end + 1;
        }
        self.hand(&mut steps, held_from.unwrap_or(text.len()));
        steps
    }

    /// Where the line that starts at `self.read` ends, from there; `None` while its end has not arrived.
    /// The search goes on from where the last one stopped, so a long line that arrives in small pieces is
    /// searched once.
    fn line_end(&mut self, text: &str) -> Option<usize> {
        let from = self.scanned.max(self.read);
        let found = text[from..].find('\n');
        #[cfg(test)]
        {
            self.searched += found.map_or(text.len() - from, |at| at + 1);
        }
        self.scanned = from + found.unwrap_or(text.len() - from);
        found.map(|at| from + at - self.read)
    }

    /// Cuts so that the part never holds more than its limit. Before the line at `self.read`, when the
    /// part already holds half of what it may: lines are short, so this is where a long block is cut.
    /// And inside that line, wherever the part reaches its limit, as often as the line is long. Both
    /// places depend only on where the part started, never on how much of the line has arrived.
    fn keep_part_within_limit(&mut self, text: &str, steps: &mut Vec<Step>) {
        // A part cut inside this very line starts after `self.read`: it holds nothing before the line.
        let held = self.read.saturating_sub(self.part_start);
        if held > 0 && held >= self.part_limit / 2 {
            self.cut_at(self.read, false, steps);
        }
        let line_stop = match self.line_end(text) {
            Some(end) => self.read + end + 1,
            None => text.len(),
        };
        while line_stop - self.part_start > self.part_limit {
            let mut at = self.part_start + self.part_limit;
            while !text.is_char_boundary(at) {
                at -= 1;
            }
            self.cut_at(at.max(self.part_start + 1), true, steps);
        }
    }

    /// Ends the part at `at`. Inside a code block the fence is closed before the cut and opened again
    /// after it; `is_inside_line` says the cut falls in the middle of a line.
    fn cut_at(&mut self, at: usize, is_inside_line: bool, steps: &mut Vec<Step>) {
        self.hand(steps, at);
        let fence = self
            .fence
            .map(|(mark, length)| mark.to_string().repeat(length));
        if let Some(fence) = &fence {
            let line_break = if is_inside_line { "\n" } else { "" };
            steps.push(Step::Literal(format!("{line_break}{fence}\n")));
        }
        steps.push(Step::Cut);
        if fence.is_some() {
            steps.push(Step::Literal(format!("{}\n", self.fence_line)));
        }
        self.part_start = at;
    }

    /// The answer is complete: whatever was held back belongs to the last part.
    pub(super) fn finish(&mut self, text: &str) -> Vec<Step> {
        let mut steps = Vec::new();
        self.hand(&mut steps, text.len());
        steps
    }

    /// The part of `text` handed out so far.
    pub(super) fn given<'a>(&self, text: &'a str) -> &'a str {
        &text[..self.given]
    }

    /// Whether the text defines references, so it must be drawn as one part.
    pub(super) fn is_whole(&self) -> bool {
        self.is_whole
    }

    /// Hands out the text up to `end` that has not been handed out yet.
    fn hand(&mut self, steps: &mut Vec<Step>, end: usize) {
        if end > self.given {
            steps.push(Step::Text(self.given..end));
            self.given = end;
        }
    }

    /// Notes a complete line: fences opening and closing, gaps, and whether a cut may follow the line.
    fn read_line(&mut self, line: &str) {
        let trimmed = line.trim_end_matches('\r');
        if let Some(open) = self.fence {
            if closes(trimmed, open) {
                self.fence = None;
                self.can_cut_after_last = true;
                self.is_after_gap = false;
            }
            return;
        }
        if trimmed.trim().is_empty() {
            if !self.is_after_gap {
                self.gap_start = self.read;
            }
            self.is_after_gap = true;
            return;
        }
        self.is_after_gap = false;
        self.fence = fence_start(trimmed);
        if let Some((mark, length)) = self.fence {
            // A fence's own line is short; one that is not is opened again without what follows its marks.
            self.fence_line = match trimmed.len() <= FENCE_LINE_MAX {
                true => trimmed.trim_start().to_string(),
                false => mark.to_string().repeat(length),
            };
        }
        self.can_cut_after_last = self.fence.is_none() && !ends_a_spaced_block(trimmed);
        if is_definition(trimmed) {
            self.is_whole = true;
        }
    }
}

/// The fence a line opens: three or more backticks or tildes after at most three spaces.
pub(super) fn fence_start(line: &str) -> Option<(char, usize)> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    let body = &line[indent..];
    let mark = body.chars().next().filter(|c| *c == '`' || *c == '~')?;
    let length = body.chars().take_while(|c| *c == mark).count();
    (indent < 4 && length >= FENCE_MIN).then_some((mark, length))
}

/// Whether a line closes the open fence: at least as many of its characters and nothing else.
pub(super) fn closes(line: &str, (mark, length): (char, usize)) -> bool {
    let body = line.trim();
    body.chars().count() >= length && body.chars().all(|c| c == mark)
}

/// Whether a block that ends with this line is spaced by its own rule rather than by a paragraph gap: a
/// heading (`#`, or the underline of a setext heading), a rule, or an HTML line.
fn ends_a_spaced_block(line: &str) -> bool {
    let body = line.trim();
    let is_underline_or_rule = body
        .chars()
        .all(|c| matches!(c, '=' | '-' | '*' | '_' | ' '));
    body.starts_with('#') || body.starts_with('<') || is_underline_or_rule
}

/// Whether a line defines a link reference or a footnote: `[label]: target`.
fn is_definition(line: &str) -> bool {
    let body = line.trim_start();
    body.starts_with('[') && body.find("]:").is_some_and(|at| !body[1..at].contains(']'))
}

/// What the start of the line after a gap opens. `is_complete` says the whole line is there.
fn opening(line: &str, is_complete: bool) -> Opening {
    let more = |decided: Opening| match is_complete {
        true => decided,
        false => Opening::Undecided,
    };
    let Some(first) = line.chars().next() else {
        return more(Opening::Attached);
    };
    match first {
        ' ' | '\t' | '#' | '>' | '<' => Opening::Attached,
        '`' | '~' => fence_or_text(line, first, is_complete),
        '[' if !is_complete => Opening::Undecided,
        '[' if is_definition(line) => Opening::Definition,
        '0'..='9' => numbered(line, is_complete),
        '-' | '*' | '+' | '_' => marked(line, first, is_complete),
        _ => Opening::Cuttable,
    }
}

/// A line that starts with a backtick or a tilde: a fence, or text that starts with code.
fn fence_or_text(line: &str, mark: char, is_complete: bool) -> Opening {
    let run = line.chars().take_while(|c| *c == mark).count();
    let is_all_marks = run == line.chars().count();
    match (run >= FENCE_MIN, is_all_marks, is_complete) {
        (true, _, _) => Opening::Cuttable,
        (false, true, false) => Opening::Undecided,
        _ => Opening::Cuttable,
    }
}

/// A line that starts with a digit: an ordered list item (`1. ` or `1) `), or a paragraph.
fn numbered(line: &str, is_complete: bool) -> Opening {
    let after_digits = line.trim_start_matches(|c: char| c.is_ascii_digit());
    let mut rest = after_digits.chars();
    match (rest.next(), rest.next()) {
        (Some('.' | ')'), Some(' ' | '\t')) => Opening::Attached,
        (None, _) | (Some('.' | ')'), None) if !is_complete => Opening::Undecided,
        (Some('.' | ')'), None) => Opening::Attached,
        _ => Opening::Cuttable,
    }
}

/// A line that starts with `-`, `*`, `+` or `_`: a list item, a rule, or a paragraph that starts with
/// emphasis or a sign.
fn marked(line: &str, mark: char, is_complete: bool) -> Opening {
    let mut rest = line.chars().skip(1);
    let is_bullet = mark != '_';
    match rest.next() {
        None if is_complete => Opening::Attached,
        None => Opening::Undecided,
        Some(' ' | '\t') if is_bullet => Opening::Attached,
        Some(_) if line.chars().all(|c| c == mark || c == ' ') => match is_complete {
            true => Opening::Attached,
            false => Opening::Undecided,
        },
        Some(_) => Opening::Cuttable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The parts the steps of `feeds` (the text as it grew) add up to.
    fn parts_of(feeds: &[&str]) -> Vec<String> {
        let mut segmenter = Segmenter::default();
        let mut parts = vec![String::new()];
        let mut take = |steps: Vec<Step>, text: &str| {
            for step in steps {
                match step {
                    Step::Text(range) => parts.last_mut().unwrap().push_str(&text[range]),
                    Step::Cut => parts.push(String::new()),
                    Step::Literal(more) => parts.last_mut().unwrap().push_str(&more),
                }
            }
        };
        for text in feeds {
            take(segmenter.feed(text), text);
        }
        let whole = feeds.last().copied().unwrap_or("");
        take(segmenter.finish(whole), whole);
        parts
    }

    fn cut(text: &str) -> Vec<String> {
        parts_of(&[text])
    }

    #[test]
    fn feed_paragraphs_are_cut_at_the_blank_lines_between_them() {
        assert_eq!(
            cut("One.\n\nTwo.\n\n\nThree.\n"),
            ["One.\n", "Two.\n", "Three.\n"]
        );
        assert_eq!(cut("One line\nonly.\n"), ["One line\nonly.\n"]);
        assert_eq!(cut(""), [""]);
    }

    #[test]
    fn feed_headings_and_rules_stay_with_their_neighbours() {
        let text = "Before.\n\n## Heading\n\nAfter.\n\n---\n\nDone.\n\nLast.\n";
        assert_eq!(
            cut(text),
            [
                "Before.\n\n## Heading\n\nAfter.\n\n---\n\nDone.\n",
                "Last.\n"
            ]
        );
    }

    #[test]
    fn feed_a_setext_heading_after_a_gap_is_cut_like_a_paragraph_the_known_exception() {
        let setext = "Before.\n\nHeading\n======\n\nAfter.\n\nThen.\n";
        assert_eq!(
            cut(setext),
            ["Before.\n", "Heading\n======\n\nAfter.\n", "Then.\n"]
        );
    }

    #[test]
    fn feed_lists_quotes_and_indented_text_stay_with_what_is_above() {
        let text = "Text.\n\n- one\n\n- two\n\n    continues\n\n> quote\n\n1. first\n\nAfter.\n";
        let together = "Text.\n\n- one\n\n- two\n\n    continues\n\n> quote\n\n1. first\n";
        assert_eq!(cut(text), [together, "After.\n"]);
    }

    #[test]
    fn feed_fenced_code_is_its_own_part_and_is_never_cut_inside() {
        let text = "Here.\n\n```rust\nfn a() {}\n\nfn b() {}\n```\n\nDone.\n";
        assert_eq!(
            cut(text),
            [
                "Here.\n",
                "```rust\nfn a() {}\n\nfn b() {}\n```\n",
                "Done.\n"
            ]
        );
        let tildes = "Here.\n\n~~~~\n```\n\nmore code\n~~~~\n\nDone.\n";
        assert_eq!(
            cut(tildes),
            ["Here.\n", "~~~~\n```\n\nmore code\n~~~~\n", "Done.\n"]
        );
        let open = "Here.\n\n```\ncodice\n\nancora\n";
        assert_eq!(cut(open), ["Here.\n", "```\ncodice\n\nancora\n"]);
    }

    #[test]
    fn feed_paragraphs_that_start_with_emphasis_code_numbers_or_signs_are_cut() {
        let text = "A.\n\n**Grassetto** poi.\n\n`codice` poi.\n\n2024 fu.\n\n-5 gradi.\n\n_corsivo_ poi.\n\n|a|b|\n|-|-|\n";
        assert_eq!(
            cut(text),
            [
                "A.\n",
                "**Grassetto** poi.\n",
                "`codice` poi.\n",
                "2024 fu.\n",
                "-5 gradi.\n",
                "_corsivo_ poi.\n",
                "|a|b|\n|-|-|\n"
            ]
        );
    }

    #[test]
    fn feed_reference_definitions_keep_the_rest_whole_and_say_so() {
        let text = "See [here][1].\n\nOther.\n\n[1]: https://example.com\n\nAfter.\n\nDone.\n";
        let mut segmenter = Segmenter::default();
        let steps = segmenter.feed(text);
        assert!(segmenter.is_whole());
        assert_eq!(steps.iter().filter(|step| **step == Step::Cut).count(), 1);
        let link = "One.\n\n[a link](https://example.com) here.\n\nTwo.\n";
        assert_eq!(
            cut(link),
            ["One.\n", "[a link](https://example.com) here.\n", "Two.\n"]
        );
    }

    #[test]
    fn feed_a_streamed_text_is_cut_where_the_whole_text_is() {
        let text = "## Heading\n\nFirst paragraph **with** `code`.\n\n- one\n- two\n\n```rust\nfn a() {}\n\n```\n\n\
            **Second** paragraph.\n\n1. first\n\n2024 was a year.\n\n---\n\nDone.\n\n*** \n\nIndeed.\n\nLast.";
        let expected = cut(text);
        assert!(expected.len() >= 5, "{expected:?}");
        assert!(
            expected.iter().all(|part| !part.trim().is_empty()),
            "{expected:?}"
        );
        for step in [1, 2, 3, 7] {
            let mut feeds = Vec::new();
            let mut end = 0;
            while end < text.len() {
                end = (end + step).min(text.len());
                while !text.is_char_boundary(end) {
                    end += 1;
                }
                feeds.push(&text[..end]);
            }
            assert_eq!(parts_of(&feeds), expected, "in steps of {step}");
        }
    }

    #[test]
    fn feed_holds_back_a_line_until_its_start_says_what_it_opens() {
        let mut segmenter = Segmenter::default();
        assert_eq!(
            segmenter.feed("One.\n\n"),
            [Step::Text(0..5)],
            "the blank line waits"
        );
        assert_eq!(segmenter.feed("One.\n\nD"), [Step::Cut, Step::Text(6..7)]);
        let grown = "One.\n\nTwo.\n\n*";
        assert_eq!(
            segmenter.feed(grown),
            [Step::Text(7..11)],
            "a star may open a list or a rule"
        );
        let bold = "One.\n\nTwo.\n\n**g";
        assert_eq!(segmenter.feed(bold), [Step::Cut, Step::Text(12..15)]);
    }

    #[test]
    fn finish_hands_out_what_was_held_back() {
        let mut segmenter = Segmenter::default();
        let text = "One.\n\n**";
        assert_eq!(segmenter.feed(text), [Step::Text(0..5)]);
        assert_eq!(segmenter.finish(text), [Step::Text(5..8)]);
        assert!(segmenter.finish(text).is_empty());
    }

    /// Every text `text` was while it grew by `step` bytes at a time.
    fn grown_by(text: &str, step: usize) -> Vec<&str> {
        let mut feeds = Vec::new();
        let mut end = 0;
        while end < text.len() {
            end = (end + step).min(text.len());
            while !text.is_char_boundary(end) {
                end += 1;
            }
            feeds.push(&text[..end]);
        }
        feeds
    }

    /// The parts of `feeds` for a reading whose parts hold at most `limit` bytes.
    fn limited_parts(feeds: &[&str], limit: usize) -> Vec<String> {
        let mut segmenter = Segmenter::with_part_limit(limit);
        let mut parts = vec![String::new()];
        let whole = feeds.last().copied().unwrap_or("");
        let finish = |segmenter: &mut Segmenter| segmenter.finish(whole);
        let mut steps: Vec<(Vec<Step>, &str)> = Vec::new();
        for text in feeds {
            steps.push((segmenter.feed(text), text));
        }
        steps.push((finish(&mut segmenter), whole));
        for (steps, text) in steps {
            for step in steps {
                match step {
                    Step::Text(range) => parts.last_mut().unwrap().push_str(&text[range]),
                    Step::Cut => parts.push(String::new()),
                    Step::Literal(more) => parts.last_mut().unwrap().push_str(&more),
                }
            }
        }
        parts
    }

    #[test]
    fn feed_a_long_line_that_arrives_in_small_chunks_is_searched_once_not_at_every_chunk() {
        let line = "word ".repeat(30_000);
        let mut segmenter = Segmenter::with_part_limit(usize::MAX);
        for text in grown_by(&line, 512) {
            segmenter.feed(text);
        }
        assert!(
            segmenter.searched <= line.len() * 2,
            "{} bytes searched for the end of a line of {}",
            segmenter.searched,
            line.len()
        );
    }

    #[test]
    fn feed_a_block_that_goes_on_past_the_limit_is_cut_at_a_line_and_no_part_passes_the_limit() {
        let block: String = (0..200).map(|n| format!("line numero {n}\n")).collect();
        let parts = limited_parts(&[&block], 256);
        assert!(parts.len() > 8, "{} parts", parts.len());
        assert!(parts.iter().all(|part| part.len() <= 256), "{parts:?}");
        assert!(
            parts.iter().all(|part| part.ends_with('\n')),
            "cut between lines"
        );
        assert_eq!(
            parts.concat(),
            block,
            "nothing is lost or added outside code"
        );
    }

    #[test]
    fn feed_a_code_block_cut_by_the_limit_is_closed_and_opened_again_so_both_halves_are_code() {
        let lines: String = (0..60).map(|n| format!("let x{n} = {n};\n")).collect();
        let text = format!("```rust\n{lines}```\n\nDone.\n");
        let parts = limited_parts(&[&text], 300);
        let code: Vec<&String> = parts
            .iter()
            .filter(|part| part.starts_with("```rust\n"))
            .collect();
        assert!(code.len() >= 3, "{parts:?}");
        for part in &code {
            assert!(
                part.trim_end().ends_with("```"),
                "an open fence is left: {part:?}"
            );
            assert_eq!(part.matches("```").count(), 2, "{part:?}");
        }
        assert_eq!(parts.last().map(String::as_str), Some("Done.\n"));
        let kept: String = parts.concat().replace("```rust\n", "").replace("```\n", "");
        assert!(
            kept.contains(&lines),
            "every line of code is kept, in order"
        );
    }

    #[test]
    fn feed_one_endless_line_is_cut_where_it_passes_the_limit() {
        let line = "x".repeat(1000);
        let parts = limited_parts(&[&line], 300);
        assert_eq!(
            parts.iter().map(String::len).collect::<Vec<_>>(),
            [300, 300, 300, 100]
        );
    }

    #[test]
    fn feed_a_text_cut_by_the_limit_is_cut_alike_however_it_arrives() {
        let lines: String = (0..40).map(|n| format!("line {n} di text\n")).collect();
        let long = "y".repeat(700);
        let text = format!("Inizio.\n\n```sh\n{lines}```\n\n{lines}\n{long}\n\nDone.");
        let whole = limited_parts(&[&text], 200);
        for step in [1, 3, 64, 333] {
            assert_eq!(
                limited_parts(&grown_by(&text, step), 200),
                whole,
                "in steps of {step}"
            );
        }
    }
}
