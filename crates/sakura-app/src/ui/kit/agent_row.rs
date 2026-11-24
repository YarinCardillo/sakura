//! The agent row: one agent of a lane in a list. The role and, under it, the model and the account stand at
//! the start; the state in words with its mark stands at the end, and under it what the agent is doing now.
//! Only words come in: the page decides what they say. The row is the kit's inbox row, so its fill under the
//! pointer and its selected fill are the list's own.

use super::row::{row, RowKind};
use super::state_glyph::StateGlyph;
use crate::theme::kit::scale::{
    Typeset as _, ICON_REGULAR, SPACE_S, SPACE_XS, TEXT_BODY, TEXT_FOOTNOTE, WEIGHT_MEDIUM,
};
use crate::theme::kit::{AGENT_ROW_DOING_MAX_WIDTH, AGENT_ROW_ROLE_WIDTH};
use crate::theme::Palette;
use gpui::{
    div, AnyElement, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    Stateful, Styled as _,
};

/// What an agent is doing now, in words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DoingLine {
    /// A tool that has not finished: what it is, what it works on and how long it has run.
    Tool {
        label: SharedString,
        detail: SharedString,
        elapsed: SharedString,
    },
    /// The first line of what the agent wrote last.
    Text(SharedString),
}

/// Everything an agent row says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRowFacts {
    /// The role: "Builder".
    pub role: SharedString,
    /// Which step of the plan a planner's run is; none for any other role.
    pub phase: Option<SharedString>,
    /// The model and the account, one muted line.
    pub detail: SharedString,
    /// The task the agent works on, when another agent has its role: the builders of two tasks.
    pub task: Option<SharedString>,
    /// The agent's history, for an agent with rounds or resumes: "Round 3, continued once".
    pub rounds: Option<SharedString>,
    pub state: SharedString,
    pub glyph: StateGlyph,
    pub doing: Option<DoingLine>,
}

/// The row named `id` for `facts`; the caller adds the press. `{id}-role`, `{id}-detail`, `{id}-state` and
/// `{id}-doing` name its parts in the window.
pub fn agent_row(
    id: impl Into<SharedString>,
    facts: &AgentRowFacts,
    palette: &Palette,
) -> Stateful<gpui::Div> {
    let id: SharedString = id.into();
    row(RowKind::Inbox, id.clone(), false, palette)
        .flex_wrap()
        .child(start(&id, facts, palette))
        .child(end(&id, facts, palette))
}

fn start(id: &SharedString, facts: &AgentRowFacts, palette: &Palette) -> AnyElement {
    let (role, detail) = (format!("{id}-role"), format!("{id}-detail"));
    let second = [
        facts.phase.clone(),
        Some(facts.detail.clone()),
        facts.task.clone(),
        facts.rounds.clone(),
    ];
    let second: Vec<SharedString> = second.into_iter().flatten().collect();
    div()
        .flex_grow_1()
        .flex_basis(AGENT_ROW_ROLE_WIDTH)
        .min_w_0()
        .flex()
        .flex_col()
        .gap(SPACE_XS)
        .child(
            div().flex().items_center().gap(SPACE_S).child(
                div()
                    .debug_selector(move || role.clone())
                    .text_style(TEXT_BODY)
                    .font_weight(WEIGHT_MEDIUM)
                    .text_color(palette.text)
                    .child(facts.role.clone()),
            ),
        )
        .child(
            div()
                .debug_selector(move || detail.clone())
                .min_w_0()
                .truncate()
                .text_style(TEXT_FOOTNOTE)
                .text_color(palette.text_tertiary)
                .child(second.join(", ")),
        )
        .into_any_element()
}

fn end(id: &SharedString, facts: &AgentRowFacts, palette: &Palette) -> AnyElement {
    let (state, doing) = (format!("{id}-state"), format!("{id}-doing"));
    let doing_line = facts.doing.as_ref().map(|line| {
        div()
            .debug_selector(move || doing.clone())
            .max_w(AGENT_ROW_DOING_MAX_WIDTH)
            .min_w_0()
            .flex()
            .gap(SPACE_S)
            .text_style(TEXT_FOOTNOTE)
            .text_color(palette.text_tertiary)
            .child(doing_words(line))
    });
    div()
        .flex_none()
        .min_w_0()
        .max_w_full()
        .flex()
        .flex_col()
        .items_end()
        .gap(SPACE_XS)
        .child(
            div()
                .debug_selector(move || state.clone())
                .flex()
                .items_center()
                .gap(SPACE_S)
                .text_style(TEXT_FOOTNOTE)
                .text_color(palette.text_secondary)
                .child(facts.glyph.paint(ICON_REGULAR, palette))
                .child(facts.state.clone()),
        )
        .children(doing_line)
        .into_any_element()
}

fn doing_words(line: &DoingLine) -> AnyElement {
    match line {
        DoingLine::Text(words) => div()
            .min_w_0()
            .truncate()
            .child(words.clone())
            .into_any_element(),
        DoingLine::Tool {
            label,
            detail,
            elapsed,
        } => div()
            .min_w_0()
            .flex()
            .gap(SPACE_S)
            .child(div().flex_none().child(label.clone()))
            .child(div().min_w_0().truncate().child(detail.clone()))
            .child(div().flex_none().child(elapsed.clone()))
            .into_any_element(),
    }
}
