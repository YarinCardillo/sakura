//! A tool call: one row with what it does, the agent's words for it and where it stands (a glyph and a
//! word), and under it what it shows: a diff in the review list's style, or text in a monospaced block
//! with a long output folded behind its line count.

use super::segments::{closes, fence_start};
use super::{copy, diff, glyphs, RowState, ThreadView};
use crate::l10n::{say, tr, tr_with, Key};
use crate::theme::thread as tokens;
use crate::theme::{self, Palette};
use crate::ui::chrome::{mono, text};
use gpui::{
    div, AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement as _, Styled, Window,
};
use sakura_acp::model::{ToolContent, ToolKind, ToolStatus};
use sakura_acp::thread::{Entry, ToolCall};
use sakura_core::said::Said;
use std::borrow::Cow;

/// Whether a call's content is open before the person chooses: what the agent changed or ran is, and so
/// is whatever a failed call left; a file that was only read is not.
fn opens_by_default(call: &ToolCall) -> bool {
    let has_diff = call
        .content
        .iter()
        .any(|content| matches!(content, ToolContent::Diff(_)));
    has_diff || call.kind == ToolKind::Execute || call.status == ToolStatus::Failed
}

/// The lines of `text` a folded block shows: the end of a command's output, the start of anything else.
fn folded_lines(text: &str, kind: ToolKind) -> (Vec<&str>, usize) {
    let lines: Vec<&str> = text.lines().collect();
    let total = lines.len();
    if total <= tokens::OUTPUT_FOLDED_LINES {
        return (lines, total);
    }
    let shown = match kind {
        ToolKind::Execute => lines[total - tokens::OUTPUT_FOLDED_LINES..].to_vec(),
        _ => lines[..tokens::OUTPUT_FOLDED_LINES].to_vec(),
    };
    (shown, total)
}

/// What a tool wrote, without the markdown fence some agents wrap it in: the block already draws it as
/// output, so the fence's own lines would be read as part of it. A fence is taken off only when it wraps
/// the whole text; of an output that was cut short the closing line may be missing.
fn unfenced(text: &str, is_truncated: bool) -> &str {
    let body = text.trim_matches(['\n', '\r']);
    let Some((first, rest)) = body.split_once('\n') else {
        return text;
    };
    let Some(open) = fence_start(first) else {
        return text;
    };
    match rest.rsplit_once('\n') {
        Some((inside, last)) if closes(last, open) => inside,
        None if closes(rest, open) => "",
        _ if is_truncated => rest,
        _ => text,
    }
}

/// The tags an agent's protocol wraps a tool's whole output in, and whether each says the call
/// failed: the Claude adapter passes Claude Code's own wrappers through as text.
const WRAPPERS: [(&str, bool); 3] = [
    ("tool_use_error", true),
    ("local-command-stderr", true),
    ("local-command-stdout", false),
];

/// The notes a harness adds to a tool's output for the model, never for the person: the adapter drops
/// them from a replayed message, and they are dropped here from a tool's output.
const INJECTED: [&str; 2] = ["system-reminder", "task-notification"];

/// What a tool wrote, without the notes for the model in it. Text that loses one loses the blank end
/// it leaves behind; a note that never closes is part of the text.
fn without_injected(text: &str) -> Cow<'_, str> {
    let mut kept = Cow::Borrowed(text);
    for tag in INJECTED {
        let (open, close) = (format!("<{tag}>"), format!("</{tag}>"));
        while let Some(start) = kept.find(&open) {
            let Some(length) = kept[start..].find(&close) else {
                break;
            };
            kept.to_mut()
                .replace_range(start..start + length + close.len(), "");
        }
    }
    match kept {
        Cow::Owned(left) => Cow::Owned(left.trim_end().to_string()),
        borrowed => borrowed,
    }
}

/// What a tool wrote, without a tag of the protocol around the whole of it, and whether that tag says
/// the call failed. A tag that stands anywhere else is part of the text.
fn unwrapped(text: &str) -> (&str, bool) {
    let body = text.trim();
    let inside = |(tag, is_error): &(&str, bool)| {
        let rest = body
            .strip_prefix('<')?
            .strip_prefix(*tag)?
            .strip_prefix('>')?;
        let rest = rest
            .strip_suffix('>')?
            .strip_suffix(*tag)?
            .strip_suffix("</")?;
        Some((rest.trim_matches(['\n', '\r']), *is_error))
    };
    WRAPPERS.iter().find_map(inside).unwrap_or((text, false))
}

/// Whether `call` tried to change something and was stopped: the write was refused or failed, so nothing changed.
fn is_blocked_change(call: &ToolCall) -> bool {
    let is_change = matches!(
        call.kind,
        ToolKind::Edit | ToolKind::Delete | ToolKind::Move
    );
    is_change && matches!(call.status, ToolStatus::Failed | ToolStatus::Rejected)
}

/// What a run of tool calls did, in words: "Read 6 files, searched 2 times".
fn group_words(calls: &[&ToolCall]) -> String {
    let count = |kinds: &[ToolKind]| {
        calls
            .iter()
            .filter(|call| kinds.contains(&call.kind) && !is_blocked_change(call))
            .count()
    };
    let blocked = calls.iter().filter(|call| is_blocked_change(call)).count();
    let parts = [
        ("thread.group.read", count(&[ToolKind::Read])),
        ("thread.group.search", count(&[ToolKind::Search])),
        (
            "thread.group.edit",
            count(&[ToolKind::Edit, ToolKind::Delete, ToolKind::Move]),
        ),
        ("thread.group.blocked", blocked),
        ("thread.group.run", count(&[ToolKind::Execute])),
        ("thread.group.fetch", count(&[ToolKind::Fetch])),
        (
            "thread.group.other",
            count(&[ToolKind::Think, ToolKind::SwitchMode, ToolKind::Other]),
        ),
    ];
    let said: Vec<String> = parts
        .into_iter()
        .filter(|(_, number)| *number > 0)
        .map(|(key, number)| say(&Said::new(key).count(number as u64)).to_string())
        .collect();
    let mut words = said.join(", ");
    if let Some(first) = words.get_mut(..1) {
        first.make_ascii_uppercase();
    }
    words
}

impl ThreadView {
    /// The calls of the run the entry at `head` begins: it and the calls after it that are folded with it.
    fn run_from(&self, head: usize) -> Vec<&Entry> {
        let entries = self.thread.entries();
        let following = (head + 1..entries.len()).take_while(|index| self.is_grouped(*index));
        std::iter::once(head)
            .chain(following)
            .map(|index| &entries[index])
            .collect()
    }

    /// A run of the master's tool calls as one quiet line, a chevron and what the calls did; a press opens
    /// the line of each call under it.
    pub(super) fn tool_group(
        &self,
        head: usize,
        palette: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let run = self.run_from(head);
        let id = run[0].id;
        let calls: Vec<&ToolCall> = run
            .iter()
            .filter_map(|entry| match &entry.kind {
                sakura_acp::thread::EntryKind::Tool(call) => Some(call),
                _ => None,
            })
            .collect();
        let is_open = self.opened_groups.contains(&id);
        let words = text(theme::TEXT_SMALL, palette.text_secondary)
            .line_height(tokens::QUIET_LINE_HEIGHT)
            .child(group_words(&calls));
        let header = div()
            .id(SharedString::from(format!("thread-tools-{id}")))
            .debug_selector(move || format!("thread-tools-{id}"))
            .w_full()
            .min_h(tokens::LINE_ROW_HEIGHT)
            .py(theme::GAP_TIGHT)
            .flex()
            .items_center()
            .gap(tokens::LINE_ROW_GAP)
            .child(glyphs::chevron(is_open, palette.text_tertiary))
            .child(words)
            .on_click(cx.listener(move |view, _, _, cx| view.toggle_group(id, cx)));
        let lines = is_open.then(|| {
            let rows = run
                .iter()
                .zip(&calls)
                .map(|(entry, call)| self.tool(entry, call, palette, window, cx));
            div()
                .w_full()
                .pl(tokens::LINE_ROW_INDENT)
                .flex()
                .flex_col()
                .children(rows.collect::<Vec<_>>())
        });
        div()
            .w_full()
            .child(header)
            .children(lines)
            .into_any_element()
    }

    pub(super) fn tool(
        &self,
        entry: &Entry,
        call: &ToolCall,
        palette: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if let Some(card) = self.master_card(call, palette, cx) {
            return card;
        }
        // An engine tool is one plain line of what was looked at: its answer is data for the agent, not the person.
        let has_body = !call.content.is_empty() && copy::tool_words(&call.title).is_none();
        // The master's tool calls are one quiet line each, opened by the person alone.
        let by_default = self.master.is_none() && opens_by_default(call);
        let is_open = has_body && self.is_open(entry.id, by_default);
        let header = self.tool_header(entry.id, call, has_body.then_some(is_open), palette, cx);
        let body = is_open.then(|| {
            let blocks = call.content.iter().map(|content| {
                self.tool_content(entry.id, call.kind, content, palette, window, cx)
            });
            div()
                .w_full()
                .pl(tokens::LINE_ROW_INDENT)
                .pb(theme::GAP_TIGHT)
                .flex()
                .flex_col()
                .gap(tokens::LINE_ROW_GAP)
                .children(blocks.collect::<Vec<_>>())
        });
        div()
            .w_full()
            .child(header)
            .children(body)
            .into_any_element()
    }

    /// The call's row. `is_open` is `None` for a call with nothing under it: its row does not open.
    fn tool_header(
        &self,
        id: u64,
        call: &ToolCall,
        is_open: Option<bool>,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let first_line = call.title.lines().next().unwrap_or_default();
        let words = match call.title.lines().next() {
            _ if copy::tool_words(first_line).is_some() => {
                tr(copy::tool_words(first_line).unwrap_or(copy::kind_word(call.kind))).to_string()
            }
            Some(title) if !title.trim().is_empty() => copy::shown(title),
            _ => tr(copy::kind_word(call.kind)).to_string(),
        };
        // A command is literal text, so it is set in the monospaced face; anything else is the agent's prose.
        let words = match call.kind {
            ToolKind::Execute => mono().text_size(theme::TEXT_LABEL),
            _ => div().text_size(theme::TEXT_SMALL),
        }
        .text_color(palette.text_secondary)
        .line_height(tokens::QUIET_LINE_HEIGHT)
        .min_w_0()
        .child(words);
        let state = div()
            .h(tokens::QUIET_LINE_HEIGHT)
            .flex()
            .flex_none()
            .items_center()
            .gap(theme::GAP_TIGHT)
            .child(glyphs::status(call.status, palette))
            .child(
                text(
                    theme::TEXT_LABEL,
                    glyphs::status_colour(call.status, palette),
                )
                .child(tr(copy::status_word(call.status))),
            );
        // A title too long for the pane wraps under itself; the glyphs stay on its first line.
        let first_line = |mark: AnyElement| {
            div()
                .h(tokens::QUIET_LINE_HEIGHT)
                .flex()
                .flex_none()
                .items_center()
                .child(mark)
        };
        // The master's finished calls are one quiet line: only a call that did not go through says where it stands.
        let shows_state = self.master.is_none() || call.status != ToolStatus::Completed;
        let row = div()
            .id(SharedString::from(format!("thread-tool-{id}")))
            .debug_selector(move || format!("thread-tool-{id}"))
            .w_full()
            .min_h(tokens::LINE_ROW_HEIGHT)
            .py(theme::GAP_TIGHT)
            .flex()
            .items_start()
            .gap(tokens::LINE_ROW_GAP)
            .child(first_line(glyphs::kind(call.kind, palette.text_tertiary)))
            .child(words)
            .children(
                is_open.map(|is_open| first_line(glyphs::chevron(is_open, palette.text_tertiary))),
            )
            .child(div().flex_1())
            .children(shows_state.then_some(state));
        match is_open {
            Some(is_open) => row
                .on_click(cx.listener(move |view, _, _, cx| view.toggle(id, is_open, cx)))
                .into_any_element(),
            None => row.into_any_element(),
        }
    }

    fn tool_content(
        &self,
        id: u64,
        kind: ToolKind,
        content: &ToolContent,
        palette: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let is_whole = matches!(
            self.rows.get(&id),
            Some(RowState::Foldable { is_whole: true, .. })
        );
        match content {
            ToolContent::Text { text, is_truncated } => {
                self.text_block(id, kind, text, *is_truncated, is_whole, palette, cx)
            }
            ToolContent::Diff(content) => {
                let workspace = (self.host.workspace)(cx);
                let block = diff::Block {
                    id,
                    content,
                    workspace: &workspace,
                    is_whole,
                };
                let unfold =
                    cx.listener(move |view, _: &gpui::ClickEvent, _, cx| view.unfold(id, cx));
                block.render(unfold, palette, window)
            }
            ToolContent::Terminal { id } => {
                let id = copy::shown(id);
                let note = tr_with(Key("thread.terminal"), &[("id", &id)]);
                quiet_note(note.to_string(), palette)
            }
        }
    }

    /// Text as a monospaced block; a long one shows its last or first lines and a row that shows it whole.
    #[allow(clippy::too_many_arguments)]
    fn text_block(
        &self,
        id: u64,
        kind: ToolKind,
        content: &str,
        is_truncated: bool,
        is_whole: bool,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let kept = without_injected(unfenced(content, is_truncated));
        let (content, is_error) = unwrapped(&kept);
        let (lines, total) = match is_whole {
            true => (content.lines().collect(), content.lines().count()),
            false => folded_lines(content, kind),
        };
        let is_folded = lines.len() < total;
        let block = mono()
            .w_full()
            .px(tokens::BLOCK_PADDING_X)
            .py(tokens::BLOCK_PADDING_Y)
            .rounded(theme::RADIUS_CONTROL)
            .bg(palette.inset)
            .text_size(theme::TEXT_LABEL)
            .line_height(gpui::px(theme::LOG_LINE_HEIGHT))
            .text_color(match is_error {
                true => palette.danger,
                false => palette.text_secondary,
            })
            .child(copy::shown(&lines.join("\n")));
        let more = is_folded.then(|| {
            unfold_row(id, total, palette)
                .on_click(cx.listener(move |view, _, _, cx| view.unfold(id, cx)))
        });
        let cut = is_truncated.then(|| quiet_note(tr(copy::TRUNCATED).to_string(), palette));
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(theme::GAP_TIGHT)
            .child(block)
            .children(more)
            .children(cut)
            .into_any_element()
    }
}

/// The row under a folded block: "Show all N lines".
pub(super) fn unfold_row(id: u64, lines: usize, palette: &Palette) -> gpui::Stateful<gpui::Div> {
    text(theme::TEXT_LABEL, palette.text_tertiary)
        .id(SharedString::from(format!("thread-unfold-{id}")))
        .font_features(crate::ui::chrome::tabular())
        .hover({
            let colour = palette.text_secondary;
            move |style| style.text_color(colour)
        })
        .child(copy::show_all_lines(lines))
}

pub(super) fn quiet_note(sentence: String, palette: &Palette) -> AnyElement {
    text(theme::TEXT_LABEL, palette.text_tertiary)
        .child(sentence)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn numbered(lines: usize) -> String {
        (1..=lines).map(|n| format!("line {n}\n")).collect()
    }

    fn edit(status: ToolStatus) -> ToolCall {
        ToolCall {
            id: "e".into(),
            title: String::new(),
            kind: ToolKind::Edit,
            status,
            content: Vec::new(),
            locations: Vec::new(),
            raw_input: None,
        }
    }

    #[test]
    fn a_write_that_was_blocked_is_said_blocked_and_never_changed() {
        crate::l10n::set_for_test(crate::l10n::Language::English);
        let blocked = edit(ToolStatus::Failed);
        assert_eq!(group_words(&[&blocked]), "Blocked 1 change");
        let done = edit(ToolStatus::Completed);
        assert_eq!(group_words(&[&done]), "Changed 1 file");
        assert_eq!(
            group_words(&[&done, &edit(ToolStatus::Rejected)]),
            "Changed 1 file, blocked 1 change"
        );
    }

    #[test]
    fn unfenced_output_wrapped_in_a_fence_is_the_output_alone() {
        assert_eq!(
            unfenced("```\nhello.txt\nREADME.md\n```", false),
            "hello.txt\nREADME.md"
        );
        assert_eq!(unfenced("```console\n$ ls\na\n```\n", false), "$ ls\na");
        assert_eq!(unfenced("~~~~\none line\n~~~~", false), "one line");
        assert_eq!(unfenced("```\n```", false), "");
    }

    #[test]
    fn output_wrapped_in_a_tag_of_the_protocol_is_its_text_and_an_error_tag_says_so() {
        let blocked = "<tool_use_error>Blocked: standalone sleep 60.</tool_use_error>";
        assert_eq!(unwrapped(blocked), ("Blocked: standalone sleep 60.", true));
        assert_eq!(
            unwrapped("\n<tool_use_error>\none\ntwo\n</tool_use_error>\n"),
            ("one\ntwo", true)
        );
        assert_eq!(
            unwrapped("<local-command-stderr>no such file</local-command-stderr>"),
            ("no such file", true)
        );
        assert_eq!(
            unwrapped("<local-command-stdout>hello.txt</local-command-stdout>"),
            ("hello.txt", false)
        );
    }

    #[test]
    fn a_note_the_harness_adds_for_the_model_is_never_shown() {
        let read = "1\tfn main() {}\n\n<system-reminder>\nWhenever you read a file...\n</system-reminder>\n";
        assert_eq!(without_injected(read), "1\tfn main() {}");
        let both = "fatto<task-notification>id 4 ended</task-notification> e poi";
        assert_eq!(without_injected(both), "fatto e poi");
        assert_eq!(
            without_injected("niente da togliere\n"),
            "niente da togliere\n"
        );
        assert_eq!(
            without_injected("<system-reminder>senza chiusura"),
            "<system-reminder>senza chiusura"
        );
    }

    #[test]
    fn output_that_only_holds_a_tag_somewhere_is_left_as_it_is() {
        for text in [
            "<title>Example Domain</title>",
            "prima <tool_use_error>x</tool_use_error>",
            "<tool_use_error>senza chiusura",
            "text semplice",
        ] {
            assert_eq!(unwrapped(text), (text, false));
        }
    }

    #[test]
    fn unfenced_text_that_only_holds_a_fence_somewhere_is_left_as_it_is() {
        for text in [
            "line\n```\ncode\n```",
            "```\ncode\n```\ndopo",
            "```\nsenza chiusura",
            "``` a single line ```",
            "text semplice",
        ] {
            assert_eq!(unfenced(text, false), text);
        }
    }

    #[test]
    fn unfenced_output_cut_short_loses_the_fence_that_opened_it() {
        assert_eq!(unfenced("```\none\ntwo", true), "one\ntwo");
        assert_eq!(unfenced("one\ntwo", true), "one\ntwo");
    }

    #[test]
    fn folded_lines_short_text_is_shown_whole() {
        let (lines, total) = folded_lines("a\nb\nc", ToolKind::Execute);
        assert_eq!((lines, total), (vec!["a", "b", "c"], 3));
    }

    #[test]
    fn folded_lines_long_output_of_a_command_shows_its_end() {
        let text = numbered(25);
        let (lines, total) = folded_lines(&text, ToolKind::Execute);
        assert_eq!(total, 25);
        assert_eq!(lines.len(), tokens::OUTPUT_FOLDED_LINES);
        assert_eq!((lines[0], lines[9]), ("line 16", "line 25"));
    }

    #[test]
    fn folded_lines_long_text_of_anything_else_shows_its_start() {
        let text = numbered(25);
        let (lines, _) = folded_lines(&text, ToolKind::Read);
        assert_eq!((lines[0], lines[9]), ("line 1", "line 10"));
    }
}
