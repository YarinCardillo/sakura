//! The page of a lavoro inside the panel: one header row over the centre and the right pane. The page never
//! scrolls; what is in it does.
//!
//! The header does not say the lavoro's name: the tab over it does (Zed's toolbar under its tabs and
//! Linear's issue header keep the row for where one is and what one can do, not for the title again). At
//! the left it says the lavoro's state (the mark in its tone, the word) and carries the switch between
//! the centre's views; at the right the changed lines, the branch (a press copies it) and the actions,
//! the overflow last. No sentence in it: the reasons are in "Checks" and under it, and the right pane's
//! views belong to the right pane (`right_pane`). When the window is narrow it gives parts up in a fixed
//! order (see [`super::header_fit`]) and nothing overlaps.
//!
//! It sits on the layout grid (`theme.rs`, "Layout grid"): its mark on the panel's content inset, its last
//! action ending on the same inset at the right, every control one height, everything centred on the line
//! the sidebar's first line is centred on.

use super::breath::{Breath, Owner};
use super::chrome::{drag_region, tabular, text};
use super::desk::Desk;
use super::glyph::{glyph, glyph_column, placed_state_glyph, Shape};
use super::header_fit::Fit;
use crate::l10n::{tr, tr_with, Key};
use crate::theme::{self, Palette};
use gpui::{div, AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement, Styled};
use sakura_core::display;
use sakura_core::model::{self, Tone};
use sakura_core::models::Task;

/// The page: `header` when a task is shown, a `notice` under it when there is one to give, over `body`.
pub fn view(
    header: Option<AnyElement>,
    notice: Option<AnyElement>,
    body: AnyElement,
) -> AnyElement {
    div()
        .size_full()
        .flex()
        .flex_col()
        .children(header)
        .children(notice)
        .child(div().flex_1().min_h_0().child(body))
        .into_any_element()
}

/// What a page says while the facts of its task are being read.
pub const READING: Key = Key("state.reading");
/// What the panes that need the facts say meanwhile.
pub const READING_SENTENCE: Key = Key("page.reading");

/// What the header shows of a task.
pub struct HeaderFacts<'a> {
    pub task: &'a Task,
    /// Only the task's name is known: its facts are being read.
    pub is_pending: bool,
    /// Its agent asked the person something and waits: the state shown before any other.
    pub is_waiting: bool,
    /// What the lavoro's conversation says of its agent.
    pub live: model::Live,
    /// What the header's width has room for.
    pub fit: Fit,
}

/// The header row: the state and the `switch` between the centre's views at the left, the changed lines
/// and `actions` at the right.
pub fn header(
    facts: &HeaderFacts,
    switch: AnyElement,
    actions: Vec<AnyElement>,
    palette: &Palette,
    cx: &Context<Desk>,
) -> impl IntoElement {
    let breath = Breath::of(Owner::Header, cx);
    let figures = (facts.fit.has_figures && !facts.is_pending)
        .then(|| figures(facts.task, palette))
        .flatten();
    drag_region("page-header")
        .debug_selector(|| "page-header".into())
        .h(theme::HEADER_BOX_HEIGHT)
        .flex_none()
        .px(theme::CONTENT_PADDING)
        .flex()
        .items_center()
        .gap(theme::GAP_WIDE)
        .border_b_1()
        .border_color(palette.border)
        .child(state(facts, palette, breath))
        .child(switch)
        .child(div().flex_1())
        .children(figures)
        .children(actions)
}

/// The lavoro's state: its mark, and its word while the header has room for it.
fn state(facts: &HeaderFacts, palette: &Palette, breath: Breath) -> impl IntoElement {
    let mark = if facts.is_pending {
        glyph(Shape::Ring, Tone::Neutral, palette, breath)
    } else {
        let state = model::shown_state_live(facts.task, facts.is_waiting, facts.live);
        placed_state_glyph("header", state, palette, breath)
    };
    let word = facts.fit.has_state_word.then(|| state_word(facts, palette));
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap(theme::ROW_GAP)
        .child(glyph_column(mark, "header-glyph".into()))
        .children(word)
}

/// "box/m1 da lab/example-app": the task's branch and the base it starts from; the base alone for a task
/// whose branch is not known. Both are shown as labels: what they hide is marked.
pub fn where_from(task: &Task) -> Option<String> {
    let (branch, base) = (
        task.branch.as_deref().map(display::label),
        display::label(&task.base),
    );
    match (branch, base.is_empty()) {
        (Some(branch), false) => {
            let values = [("branch", branch.as_str()), ("base", base.as_str())];
            Some(tr_with(Key("page.branch_from"), &values).to_string())
        }
        (Some(branch), true) => Some(branch),
        (None, false) => Some(base),
        (None, true) => None,
    }
}

/// The state's word, quiet: the glyph before it carries the tone, once. While the facts are being
/// read, that.
fn state_word(facts: &HeaderFacts, palette: &Palette) -> impl IntoElement {
    let (word, colour) = if facts.is_pending {
        (READING, palette.text_tertiary)
    } else {
        let state = model::shown_state_live(facts.task, facts.is_waiting, facts.live);
        (state.word, palette.text_secondary)
    };
    text(theme::TEXT_SMALL, colour)
        .debug_selector(|| "header-state".into())
        .flex_none()
        .whitespace_nowrap()
        .child(tr(word))
}

/// The changed lines, "+14 −0", when there are any: the interface font, tabular figures.
fn figures(task: &Task, palette: &Palette) -> Option<AnyElement> {
    let (added, removed) = (task.insertions?, task.deletions?);
    if added + removed == 0 {
        return None;
    }
    let (added_text, removed_text) = crate::l10n::signed_lines(added, removed);
    let figures = div()
        .debug_selector(|| "header-figures".into())
        .flex_none()
        .flex()
        .items_center()
        .gap(theme::GAP_SMALL)
        .text_size(theme::TEXT_LABEL)
        .font_features(tabular())
        .child(div().text_color(palette.success).child(added_text))
        .child(div().text_color(palette.danger).child(removed_text));
    Some(figures.into_any_element())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sakura_core::models::BoxStatus;

    fn task() -> Task {
        let status: BoxStatus = serde_json::from_str(include_str!(
            "../../../sakura-core/tests/fixtures/status.json"
        ))
        .unwrap();
        status.tasks.into_iter().next().unwrap()
    }

    #[test]
    fn the_branch_says_where_the_task_comes_from_under_the_pointer() {
        crate::l10n::set_for_test(crate::l10n::Language::Italian);
        let mut task = task();
        task.branch = Some("box/m1".into());
        task.base = "lab/example-app".into();
        assert_eq!(
            where_from(&task).as_deref(),
            Some("box/m1 da lab/example-app")
        );
        task.branch = None;
        assert_eq!(where_from(&task).as_deref(), Some("lab/example-app"));
        task.base = String::new();
        assert_eq!(where_from(&task), None);
    }

    #[test]
    fn a_branch_or_a_base_that_hides_part_of_itself_is_shown_with_marks() {
        crate::l10n::set_for_test(crate::l10n::Language::Italian);
        let mut task = task();
        task.branch = Some("box/m1\u{202e}".into());
        task.base = "lab\nwork".into();
        assert_eq!(
            where_from(&task).as_deref(),
            Some("box/m1\u{fffd} da lab\u{fffd}work")
        );
    }
}
