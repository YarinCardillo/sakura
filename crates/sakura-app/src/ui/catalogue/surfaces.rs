//! The surfaces: rows in each state, cards, the empty state, the section header, the tooltip and the toast.

use super::{group, specimen};
use crate::l10n::{tr, Key};
use crate::theme::kit::catalogue as room;
use crate::theme::kit::scale::*;
use crate::theme::kit::DISABLED_OPACITY;
use crate::theme::Palette;
use crate::ui::kit::{
    self, agent_row, card, empty_state, row, section_header, settings_row, AgentRowFacts,
    DoingLine, RowKind, StateGlyph,
};
use crate::ui::tip::{tip, Words};
use gpui::{div, AnyElement, App, ParentElement as _, SharedString, Styled as _};

fn words(label: &str, palette: &Palette) -> gpui::Div {
    div()
        .min_w_0()
        .truncate()
        .text_style(CONTROL_LABEL)
        .text_color(palette.text)
        .child(label.to_string())
}

/// List, inbox and settings rows; the list row at rest, hovered, selected, disabled and with a long label.
pub(super) fn rows(palette: &Palette) -> AnyElement {
    let list = |id: &str, selected: bool| row(RowKind::List, id.to_string(), selected, palette);
    let boxed = |element: gpui::Stateful<gpui::Div>| div().w(room::ROW_WIDTH).child(element);
    let all = vec![
        specimen(
            tr(Key("catalogue.resting_row")),
            boxed(list("cat-row", false).child(words(tr(Key("catalogue.plan_release")), palette))),
            palette,
        ),
        specimen(
            tr(Key("catalogue.selected_row")),
            boxed(
                list("cat-row-sel", true).child(words(tr(Key("catalogue.plan_release")), palette)),
            ),
            palette,
        ),
        specimen(
            tr(Key("catalogue.hover_fill")),
            boxed(
                list("cat-row-hover", false)
                    .bg(palette.fill)
                    .child(words(tr(Key("catalogue.plan_release")), palette)),
            ),
            palette,
        ),
        specimen(
            tr(Key("catalogue.disabled")),
            boxed(
                list("cat-row-off", false)
                    .opacity(DISABLED_OPACITY)
                    .child(words(tr(Key("catalogue.plan_release")), palette)),
            ),
            palette,
        ),
        specimen(
            tr(Key("catalogue.truncated_label")),
            boxed(
                list("cat-row-long", false).child(words(tr(Key("catalogue.long_lane")), palette)),
            ),
            palette,
        ),
        specimen(
            tr(Key("catalogue.inbox_row")),
            boxed(
                row(RowKind::Inbox, "cat-inbox", false, palette)
                    .flex_col()
                    .items_start()
                    .gap(SPACE_XS)
                    .child(words(tr(Key("catalogue.approve_plan")), palette))
                    .child(
                        div()
                            .text_style(TEXT_FOOTNOTE)
                            .text_color(palette.text_tertiary)
                            .child(tr(Key("catalogue.inbox_age"))),
                    ),
            ),
            palette,
        ),
        specimen(
            tr(Key("catalogue.settings_row")),
            div().w(room::SETTINGS_ROW_WIDTH).child(settings_row(
                "cat-setting",
                tr(Key("catalogue.notify")),
                Some(SharedString::from(tr(Key("catalogue.notification_hint")))),
                kit::toggle("cat-setting-toggle", true, |_, _, _| {}),
                palette,
            )),
            palette,
        ),
    ];
    group("catalogue-rows", tr(Key("catalogue.rows")), all, palette)
}

/// A card with its blocks, and the empty state inside one, and the section header.
pub(super) fn cards(palette: &Palette) -> AnyElement {
    let content = card("cat-card", palette)
        .w(room::CARD_WIDTH)
        .child(
            div()
                .text_style(TEXT_HEADLINE)
                .text_color(palette.text)
                .child(tr(Key("lane_side.pace"))),
        )
        .child(
            div()
                .text_style(TEXT_BODY)
                .text_color(palette.text_tertiary)
                .child(tr(Key("catalogue.pace_note"))),
        );
    let empty = card("cat-empty-card", palette)
        .w(room::CARD_WIDTH)
        .h(room::EMPTY_CARD_HEIGHT)
        .child(empty_state(
            "cat-empty-state",
            tr(Key("inbox.nothing")),
            Some(tr(Key("catalogue.empty_hint")).into()),
            None,
            palette,
        ));
    let header = div().w(room::CARD_WIDTH).child(section_header(
        "cat-header",
        tr(Key("lanes_page.group.running")),
        None,
        palette,
    ));
    let all = vec![
        specimen(tr(Key("catalogue.card")), content, palette),
        specimen(tr(Key("catalogue.empty_state")), empty, palette),
        specimen(tr(Key("catalogue.section_header")), header, palette),
    ];
    group("catalogue-cards", tr(Key("catalogue.cards")), all, palette)
}

/// The tooltip (hover it) and the toast the page shows on opening.
pub(super) fn feedback(palette: &Palette, cx: &App) -> AnyElement {
    let target = kit::button(
        "cat-tip-target",
        tr(Key("catalogue.show_tooltip")),
        kit::ButtonKind::Secondary,
        palette,
    );
    let tipped = tip(
        target,
        "cat-tip",
        Words::with_keys(tr(Key("catalogue.approve_plan")), "A"),
        cx,
    );
    let toast = kit::toast_surface(
        "cat-toast",
        tr(super::TOAST_WORDS).into(),
        None,
        true,
        palette,
    );
    let all = vec![
        specimen(tr(Key("catalogue.tooltip")), tipped, palette),
        specimen(tr(Key("catalogue.toast_caption")), toast, palette),
    ];
    group(
        "catalogue-feedback",
        tr(Key("catalogue.feedback")),
        all,
        palette,
    )
}

/// One agent of a lane as the lane page lists it: at work on a tool, writing, finished, stopped, resumed, the
/// newest of several attempts and a long line, and under the pointer.
pub(super) fn agent_rows(palette: &Palette) -> AnyElement {
    let facts = |role: &str, detail: &str, state: &str, glyph: StateGlyph| AgentRowFacts {
        role: role.to_string().into(),
        phase: None,
        detail: detail.to_string().into(),
        task: None,
        rounds: None,
        state: state.to_string().into(),
        glyph,
        doing: None,
    };
    let tool = DoingLine::Tool {
        label: tr(Key("catalogue.ran_command")).into(),
        detail: super::SAMPLE_COMMAND.into(),
        elapsed: tr(Key("catalogue.tool_time")).into(),
    };
    let working = |doing: Option<DoingLine>| AgentRowFacts {
        doing,
        ..facts(
            tr(Key("lane.role.builder")),
            tr(Key("catalogue.personal_sonnet")),
            tr(Key("agents.state.running")),
            StateGlyph::Running,
        )
    };
    let specimens = [
        (tr(Key("catalogue.agent_tool")), working(Some(tool.clone()))),
        (
            tr(Key("catalogue.agent_writing")),
            working(Some(DoingLine::Text(
                tr(Key("catalogue.agent_progress")).into(),
            ))),
        ),
        (
            tr(Key("catalogue.finished")),
            AgentRowFacts {
                doing: Some(DoingLine::Text(tr(Key("catalogue.agent_revision")).into())),
                ..facts(
                    tr(Key("catalogue.reviewer")),
                    tr(Key("catalogue.work_opus")),
                    tr(Key("agents.state.finished")),
                    StateGlyph::Finished,
                )
            },
        ),
        (
            tr(Key("catalogue.agent_stopped")),
            facts(
                tr(Key("lane.step.checks")),
                tr(Key("catalogue.personal_haiku")),
                tr(Key("agents.state.stopped")),
                StateGlyph::Stopped,
            ),
        ),
        (
            tr(Key("catalogue.agent_resumed")),
            AgentRowFacts {
                phase: Some(tr(Key("lane.phase.contract")).into()),
                task: Some(tr(Key("catalogue.count_characters")).into()),
                rounds: Some(tr(Key("catalogue.agent_round")).into()),
                ..working(Some(tool))
            },
        ),
        (
            tr(Key("catalogue.truncated_words")),
            working(Some(DoingLine::Text(tr(Key("catalogue.long_lane")).into()))),
        ),
    ];
    let mut all: Vec<AnyElement> = specimens
        .iter()
        .enumerate()
        .map(|(at, (name, facts))| {
            let row = agent_row(format!("cat-agent-{at}"), facts, palette);
            specimen(name, div().w(room::AGENT_ROW_WIDTH).child(row), palette)
        })
        .collect();
    let hovered = agent_row("cat-agent-hover", &working(None), palette).bg(palette.fill);
    all.push(specimen(
        tr(Key("catalogue.hover_fill")),
        div().w(room::AGENT_ROW_WIDTH).child(hovered),
        palette,
    ));
    group(
        "catalogue-agent-rows",
        tr(Key("catalogue.agents")),
        all,
        palette,
    )
}
