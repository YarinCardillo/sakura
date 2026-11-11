//! A tool call: one row with what it does, the agent's words for it and where it stands (a glyph and a
//! word), and under it what it shows: a diff in the review list's style, or text in a monospaced block
//! with a long output folded behind its line count.

use super::segments::{closes, fence_start};
use super::{copy, diff, glyphs, RowState, ThreadView};
use crate::l10n::{tr, tr_with, Key};
use crate::theme::thread as tokens;
use crate::theme::{self, Palette};
use crate::ui::chrome::{mono, text};
use gpui::{
    div, AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement as _, Styled, Window,
};
use sakura_acp::model::{ToolContent, ToolKind, ToolStatus};
use sakura_acp::thread::{Entry, ToolCall};

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

impl ThreadView {
    pub(super) fn tool(
        &self,
        entry: &Entry,
        call: &ToolCall,
        palette: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let has_body = !call.content.is_empty();
        let is_open = has_body && self.is_open(entry.id, opens_by_default(call));
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
        let words = match call.title.lines().next() {
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
        let row = div()
            .id(SharedString::from(format!("thread-tool-{id}")))
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
            .child(state);
        match is_open {
            Some(is_open) => row
                .cursor_pointer()
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
        let content = unfenced(content, is_truncated);
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
            .text_color(palette.text_secondary)
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

/// The row under a folded block: "Mostra tutte le N righe".
pub(super) fn unfold_row(id: u64, lines: usize, palette: &Palette) -> gpui::Stateful<gpui::Div> {
    text(theme::TEXT_LABEL, palette.text_tertiary)
        .id(SharedString::from(format!("thread-unfold-{id}")))
        .font_features(crate::ui::chrome::tabular())
        .cursor_pointer()
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

    #[test]
    fn unfenced_output_wrapped_in_a_fence_is_the_output_alone() {
        assert_eq!(
            unfenced("```\nhello.txt\nREADME.md\n```", false),
            "hello.txt\nREADME.md"
        );
        assert_eq!(unfenced("```console\n$ ls\na\n```\n", false), "$ ls\na");
        assert_eq!(unfenced("~~~~\nuna riga\n~~~~", false), "una riga");
        assert_eq!(unfenced("```\n```", false), "");
    }

    #[test]
    fn unfenced_text_that_only_holds_a_fence_somewhere_is_left_as_it_is() {
        for text in [
            "riga\n```\ncodice\n```",
            "```\ncodice\n```\ndopo",
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
