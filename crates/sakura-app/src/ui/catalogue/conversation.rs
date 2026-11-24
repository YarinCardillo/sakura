//! The conversation with one agent: the rows of the master chat's own look (the person's block, the agent's
//! words, the text being written), the quiet lines a lane adds (a tool with its time, the engine's line), a
//! result card and the words under the box.

use super::{group, specimen};
use crate::l10n::{tr, Key};
use crate::theme::kit::catalogue as room;
use crate::theme::thread as tokens;
use crate::theme::{self, Palette};
use crate::ui::agent_chat::draw::{
    engine_line, not_listening, result_card, round_divider, tool_line,
};
use crate::ui::agent_chat::rows::ToolRow;
use crate::ui::chrome::text;
use crate::ui::inert_markdown::inert;
use crate::ui::thread::prompt;
use gpui::{
    div, AnyElement, App, AppContext as _, Entity, IntoElement as _, ParentElement as _,
    Styled as _,
};
use gpui_component::text::{TextView, TextViewState};

/// The answer the page shows, as markdown.
pub(super) fn answer_made(cx: &mut App) -> Entity<TextViewState> {
    cx.new(|cx| TextViewState::markdown(&inert(tr(Key("catalogue.agent_answer"))), cx))
}

fn tool(started: &str, ended: Option<&str>) -> ToolRow {
    ToolRow {
        name: "bash".into(),
        path: String::new(),
        line: None,
        command: super::SAMPLE_COMMAND.into(),
        pattern: String::new(),
        target: String::new(),
        workspace: String::new(),
        started_at: sakura_core::engine::moment::iso_seconds(started),
        ended_at: ended.and_then(sakura_core::engine::moment::iso_seconds),
    }
}

/// A read of a file the engine did not name.
fn bare_read() -> ToolRow {
    ToolRow {
        name: "read".into(),
        command: String::new(),
        ..tool(
            "2026-11-19T09:14:05+00:00",
            Some("2026-11-19T09:14:07+00:00"),
        )
    }
}

/// The rows of an agent's conversation, each in its state.
pub(super) fn conversation(answer: &Entity<TextViewState>, palette: &Palette) -> AnyElement {
    let boxed = |element: AnyElement| div().w(room::CONVERSATION_WIDTH).child(element);
    let reading = div()
        .text_size(theme::TEXT_BODY)
        .line_height(tokens::MESSAGE_LINE_HEIGHT)
        .text_color(palette.text)
        .child(TextView::new(answer).selectable(true));
    let all = vec![
        specimen(
            tr(Key("catalogue.person_message")),
            boxed(prompt(0, tr(Key("catalogue.prompt")), true, palette)),
            palette,
        ),
        specimen(
            tr(Key("catalogue.agent_message")),
            boxed(reading.into_any_element()),
            palette,
        ),
        specimen(
            tr(Key("catalogue.tool_ended")),
            boxed(tool_line(
                0,
                &tool(
                    "2026-11-19T09:14:05+00:00",
                    Some("2026-11-19T09:15:15+00:00"),
                ),
                palette,
            )),
            palette,
        ),
        specimen(
            tr(Key("catalogue.tool_running")),
            boxed(tool_line(
                1,
                &tool("2026-11-19T09:14:05+00:00", None),
                palette,
            )),
            palette,
        ),
        specimen(
            tr(Key("catalogue.tool_no_target")),
            boxed(tool_line(5, &bare_read(), palette)),
            palette,
        ),
        specimen(
            tr(Key("catalogue.engine_message")),
            boxed(engine_line(2, tr(Key("catalogue.resumed_build")), palette)),
            palette,
        ),
        specimen(
            tr(Key("catalogue.round_start")),
            boxed(round_divider(8, 2, palette)),
            palette,
        ),
        specimen(
            tr(Key("catalogue.result_passed")),
            boxed(result_card(
                3,
                "pass",
                tr(Key("catalogue.passed_note")),
                palette,
            )),
            palette,
        ),
        specimen(
            tr(Key("catalogue.result_approved")),
            boxed(result_card(
                6,
                "APPROVE",
                tr(Key("catalogue.approved_note")),
                palette,
            )),
            palette,
        ),
        specimen(
            tr(Key("catalogue.result_needs_changes")),
            boxed(result_card(
                7,
                "REVISE",
                tr(Key("catalogue.change_note")),
                palette,
            )),
            palette,
        ),
        specimen(
            tr(Key("catalogue.result_failed")),
            boxed(result_card(
                4,
                "fail",
                tr(Key("catalogue.failed_note")),
                palette,
            )),
            palette,
        ),
        specimen(
            tr(Key("catalogue.engine_reply")),
            boxed(
                text(theme::TEXT_SMALL, palette.text_secondary)
                    .child(tr(Key("catalogue.reviewer_reply")))
                    .into_any_element(),
            ),
            palette,
        ),
        specimen(
            tr(Key("catalogue.unread_note_caption")),
            boxed(not_listening(tr(Key("catalogue.unread_note")), palette)),
            palette,
        ),
    ];
    group(
        "catalogue-conversation",
        tr(Key("catalogue.conversation")),
        all,
        palette,
    )
}
