//! Words the engine wrote, shown as they are: no Markdown parser reads them, so an underscore stays an underscore
//! and a path of a project stays whole, in the code face of the app.

use crate::theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    div, Div, Font, Hsla, InteractiveElement as _, ParentElement as _, SharedString, Styled as _,
    StyledText, TextRun,
};
use std::ops::Range;

/// The punctuation that may stand against a word without being a part of it.
const EDGE: &[char] = &[',', ';', ':', '(', ')', '\'', '"', '`', '.'];
/// The endings that make a word with a dot in it a file name.
const FILE_ENDINGS: [&str; 18] = [
    "py", "pyc", "rs", "md", "json", "toml", "yml", "yaml", "txt", "js", "ts", "tsx", "sh", "lock",
    "cfg", "ini", "html", "css",
];

/// Where the paths are in `text`: a word with a slash in it, or a file name with a known ending, without the
/// punctuation around it.
pub fn path_ranges(text: &str) -> Vec<Range<usize>> {
    let mut found = Vec::new();
    let mut start = None;
    for (at, letter) in text.char_indices().chain([(text.len(), ' ')]) {
        match (letter.is_whitespace(), start) {
            (false, None) => start = Some(at),
            (true, Some(from)) => {
                start = None;
                if let Some(range) = path_in(text, from..at) {
                    found.push(range);
                }
            }
            _ => {}
        }
    }
    found
}

/// The path a word is, with the edges cut off, when it is one.
fn path_in(text: &str, word: Range<usize>) -> Option<Range<usize>> {
    let whole = &text[word.clone()];
    let cut_start = whole.len() - whole.trim_start_matches(EDGE).len();
    let kept = whole.trim_matches(EDGE);
    if kept.is_empty() {
        return None;
    }
    let ending = kept.rsplit_once('.').map(|(_, ending)| ending);
    let is_file = ending.is_some_and(|ending| FILE_ENDINGS.contains(&ending));
    let is_path = kept.contains('/') && kept.chars().any(char::is_alphanumeric);
    (is_path || is_file).then(|| {
        let from = word.start + cut_start;
        from..from + kept.len()
    })
}

/// `text` without the backticks of its code spans, and where those spans are in what is left. A backtick that
/// is not closed stays as it is.
pub fn code_spans(text: &str) -> (String, Vec<Range<usize>>) {
    let mut shown = String::with_capacity(text.len());
    let mut spans = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find('`') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('`') else { break };
        shown.push_str(&rest[..open]);
        let from = shown.len();
        shown.push_str(&after[..close]);
        spans.push(from..shown.len());
        rest = &after[close + 1..];
    }
    shown.push_str(rest);
    (shown, spans)
}

/// Wraps the engine's text between words, keeping the code face for paths and backticked text.
pub fn plain_with_paths(text: &str, ink: Hsla, path_ink: Hsla) -> Div {
    let (text, mut spans) = code_spans(text);
    spans.extend(path_ranges(&text));
    spans.sort_by_key(|range| range.start);
    let code = merge_spans(spans);
    let mut offset = 0;
    div().w_full().children(text.split('\n').map(|line| {
        let start = offset;
        offset += line.len() + 1;
        let atoms = word_ranges(line).into_iter().map(|word| {
            div()
                .flex_none()
                .max_w_full()
                .debug_selector(|| "path-text-word".into())
                .truncate()
                .child(styled_word(
                    &text,
                    start + word.start..start + word.end,
                    &code,
                    ink,
                    path_ink,
                ))
        });
        div()
            .w_full()
            .flex()
            .flex_wrap()
            .children(atoms)
            .when(line.is_empty(), |row| row.child(" "))
    }))
}

fn merge_spans(spans: Vec<Range<usize>>) -> Vec<Range<usize>> {
    let mut merged: Vec<Range<usize>> = Vec::new();
    for span in spans {
        match merged.last_mut() {
            Some(previous) if span.start <= previous.end => {
                previous.end = previous.end.max(span.end)
            }
            _ => merged.push(span),
        }
    }
    merged
}

fn word_ranges(line: &str) -> Vec<Range<usize>> {
    let mut start = 0;
    let mut words = Vec::new();
    let mut characters = line.char_indices().peekable();
    while let Some((at, character)) = characters.next() {
        let is_separator = character == ':' && !line[at..].starts_with("::");
        let next_is_space = characters
            .peek()
            .is_some_and(|(_, next)| next.is_whitespace());
        if is_separator || (character.is_whitespace() && !next_is_space) {
            let end = at + character.len_utf8();
            words.push(start..end);
            start = end;
        }
    }
    if start < line.len() {
        words.push(start..line.len());
    }
    words
}

fn styled_word(
    text: &str,
    word: Range<usize>,
    code: &[Range<usize>],
    ink: Hsla,
    path_ink: Hsla,
) -> StyledText {
    let run = |len, family, color| TextRun {
        len,
        font: Font {
            family: SharedString::from(family),
            ..Font::default()
        },
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let mut runs = Vec::new();
    let mut at = word.start;
    let start = code.partition_point(|span| span.end <= at);
    for span in code[start..]
        .iter()
        .take_while(|span| span.start < word.end)
    {
        if span.start > at {
            runs.push(run(span.start - at, theme::FONT_UI, ink));
        }
        let end = span.end.min(word.end);
        runs.push(run(end - span.start.max(at), theme::FONT_MONO, path_ink));
        at = end;
    }
    if at < word.end {
        runs.push(run(word.end - at, theme::FONT_UI, ink));
    }
    StyledText::new(text[word].to_string()).with_runs(runs)
}

#[cfg(test)]
mod tests {
    use super::{code_spans, path_ranges};

    #[test]
    fn a_span_in_backticks_is_code_without_its_backticks_and_a_lone_backtick_stays() {
        let (text, spans) =
            code_spans("It said `git commit -m x` failed in tests/__pycache__/a.pyc and `unclosed");
        assert_eq!(
            text,
            "It said git commit -m x failed in tests/__pycache__/a.pyc and `unclosed"
        );
        let code: Vec<&str> = spans.iter().map(|span| &text[span.clone()]).collect();
        assert_eq!(code, ["git commit -m x"]);
        assert_eq!(
            path_ranges(&text)
                .into_iter()
                .map(|range| &text[range])
                .collect::<Vec<_>>(),
            ["tests/__pycache__/a.pyc"],
            "the paths stay intact"
        );
    }

    fn paths(text: &str) -> Vec<&str> {
        path_ranges(text)
            .into_iter()
            .map(|range| &text[range])
            .collect()
    }

    #[test]
    fn a_path_stays_whole_with_its_underscores_and_the_punctuation_around_it_is_left_out() {
        let text = "plan reviewer changed the workspace during arbiter: __pycache__/tally.cpython-314.pyc, tests/__pycache__/test_reverse.cpython-314.pyc";
        assert_eq!(
            paths(text),
            [
                "__pycache__/tally.cpython-314.pyc",
                "tests/__pycache__/test_reverse.cpython-314.pyc"
            ]
        );
    }

    #[test]
    fn a_file_name_is_a_path_and_a_sentence_is_not() {
        assert_eq!(
            paths("Add CONTRIBUTING.md and edit tally.py."),
            ["CONTRIBUTING.md", "tally.py"]
        );
        assert!(paths("Python 3.10+; e.g. the end. Done.").is_empty());
    }
}

#[cfg(test)]
#[test]
fn a_relative_path_with_intraword_underscores_is_drawn_as_one_word() {
    let path = "e2e/json_option/check_json_option.py";
    let line = format!("Check {path} and run_measure.");
    let words: Vec<_> = word_ranges(&line)
        .into_iter()
        .map(|range| line[range].trim_end())
        .collect();
    assert_eq!(words, ["Check", path, "and", "run_measure."]);
}

#[cfg(test)]
#[gpui::test]
fn a_long_reason_token_fits_the_reading_column(cx: &mut gpui::TestAppContext) {
    use gpui::{px, Context, Render, Window};

    struct Reason;

    impl Render for Reason {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
            let token = "TemporaryDirectory(ignore_cleanup_errors=True)".repeat(10);
            div()
                .w(px(140.))
                .child(plain_with_paths(&token, gpui::black(), gpui::black()))
        }
    }

    let (_, cx) = cx.add_window_view(|_, _| Reason);
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let word = cx.debug_bounds("path-text-word").unwrap();
    assert!(word.right() <= px(140.), "{word:?}");
}
