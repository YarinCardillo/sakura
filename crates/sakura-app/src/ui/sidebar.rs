//! The sidebar on the canvas, between the window strip and the status bar: the three groups of tasks, each
//! row a glyph, the name and the changed lines. No border: depth is canvas against panel.
//!
//! On the layout grid (`theme.rs`, "Layout grid"): labels and glyphs start on the sidebar's
//! inset, where the traffic lights start, and the trailing figures end on it at the right. The first line
//! (the header: the first group's label, and "New task" as a plus at its right) is as tall as the page
//! header and centred on its line; the rows start where the header ends and keep one pitch, group labels
//! included, so they share their lines with the rows of the changes pane.

use super::breath::{Breath, BreathMarks, Owner};
use super::chrome::tabular;
use super::desk::Desk;
use super::glyph::{glyph, glyph_column, placed_state_glyph, Shape};
use super::pane_toggle;
use crate::data::facts::{Facts, Phase, Row};
use crate::l10n::{say, tr, Key};
use crate::menus::{self, NewTask};
use crate::theme::{self, Palette, Theme};
use gpui::{
    div, prelude::FluentBuilder as _, px, AnyElement, Context, Div, Entity, FontWeight,
    InteractiveElement as _, IntoElement, ParentElement, Render, SharedString, Stateful,
    StatefulInteractiveElement as _, Styled, Subscription, WeakEntity, Window,
};
use gpui_component::IconName;
use sakura_core::model::{self, Group, Tone};
use std::collections::HashSet;

/// The label over the rows whose facts have not been read yet.
const READING_LABEL: Key = Key("state.reading");

/// The sidebar as its own view, drawn again only when facts or the selection change: scrolling the panel and
/// the breath's tick do not draw it again.
pub struct SidebarView {
    desk: WeakEntity<Desk>,
    facts: Entity<Facts>,
    selected: Option<String>,
    #[cfg(test)]
    renders: usize,
    _observer: Subscription,
}

impl SidebarView {
    /// The sidebar of `desk` over `facts`, with `selected` highlighted.
    pub fn new(
        desk: WeakEntity<Desk>,
        facts: Entity<Facts>,
        selected: Option<String>,
        cx: &mut Context<Self>,
    ) -> Self {
        let observer = cx.observe(&facts, |_, _, cx| cx.notify());
        Self {
            desk,
            facts,
            selected,
            #[cfg(test)]
            renders: 0,
            _observer: observer,
        }
    }

    /// How many times the sidebar was drawn.
    #[cfg(test)]
    pub fn renders(&self) -> usize {
        self.renders
    }

    /// Highlights `selected` from the next frame.
    pub fn set_selected(&mut self, selected: Option<String>, cx: &mut Context<Self>) {
        if self.selected != selected {
            self.selected = selected;
            cx.notify();
        }
    }
}

impl Render for SidebarView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        {
            self.renders += 1;
        }
        crate::snapshot::count_render("sidebar");
        BreathMarks::clear(Owner::Sidebar, cx);
        let palette = Theme::palette(cx);
        let facts = self.facts.read(cx);
        let at = Place {
            desk: &self.desk,
            selected: self.selected.as_deref(),
            waiting: facts.waiting(),
            facts,
        };
        view(facts, at, &palette, cx)
    }
}

/// Whose sidebar it is, which task it highlights and which ones have an agent that waits for the person.
#[derive(Clone, Copy)]
struct Place<'a> {
    desk: &'a WeakEntity<Desk>,
    selected: Option<&'a str>,
    waiting: &'a HashSet<String>,
    facts: &'a Facts,
}

/// The whole sidebar: its header and the rows, down to the status bar.
fn view(
    facts: &Facts,
    at: Place,
    palette: &Palette,
    cx: &Context<SidebarView>,
) -> impl IntoElement {
    let listed = match facts.phase() {
        Phase::Starting => None,
        _ => Some(listed(facts)),
    };
    let first = listed.as_ref().and_then(|groups| groups.first());
    let first = first.map(|(label, figure, _)| (*label, *figure));
    let is_empty = listed.as_ref().is_some_and(Vec::is_empty);
    div()
        .w(theme::SIDEBAR_WIDTH)
        .h_full()
        .flex_none()
        .flex()
        .flex_col()
        .child(header(first, is_empty, palette, cx))
        .child(list(listed, at, palette, cx))
}

/// The sidebar's first line, as tall as the page header and centred on its line: the first group's label
/// at the left, on the sidebar's inset, and "New task" at the right, its plus ending on the inset where
/// the rows' figures end. The button is there whatever is listed: with no lavoro, and while they are read.
/// With no lavoro at all (`is_empty`) the line says what the list is, since no group's label does.
fn header(
    first: Option<(GroupName, (usize, bool))>,
    is_empty: bool,
    palette: &Palette,
    cx: &Context<SidebarView>,
) -> impl IntoElement {
    let new_task = pane_toggle::button(
        ("new-task", "new-task-icon"),
        (IconName::Plus, theme::CONTROL_HEIGHT),
        tr(menus::NEW_TASK),
        &NewTask,
        cx,
    );
    div()
        .h(theme::HEADER_HEIGHT)
        .flex_none()
        .pl(theme::SIDEBAR_INSET)
        .pr(theme::SIDEBAR_INSET - theme::CONTROL_BLEED)
        .flex()
        .items_center()
        .justify_between()
        .gap(theme::GAP)
        .child(match first {
            Some((label, figure)) => group_words(label, figure, palette),
            None if is_empty => quiet_words("sidebar-tasks", theme::TEXT_LABEL, TASKS, palette),
            None => div(),
        })
        .child(new_task)
}

/// The rows, scrolling on their own under the header; the skeleton until the first `box status` answers.
fn list(
    listed: Option<Vec<Listed<'_>>>,
    at: Place,
    palette: &Palette,
    cx: &Context<SidebarView>,
) -> impl IntoElement {
    let list = div()
        .id("sidebar-list")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .px(theme::SIDEBAR_INSET - theme::ROW_PADDING_X)
        .flex()
        .flex_col();
    match listed {
        None => list.child(skeleton(palette)),
        // A list with no lavoro stays where it is and says so in one quiet line on its first row.
        Some(listed) if listed.is_empty() => {
            let line = quiet_words("sidebar-empty", theme::TEXT_SMALL, NO_TASKS, palette);
            list.child(
                line.h(theme::ROW_HEIGHT)
                    .px(theme::ROW_PADDING_X)
                    .items_center(),
            )
        }
        Some(listed) => list.children(groups(listed, at, palette, cx)),
    }
}

/// What the list is, said in the header when there is no group to name; and the line of an empty list.
const TASKS: Key = Key("sidebar.tasks");
const NO_TASKS: Key = Key("sidebar.empty");

/// Quiet words of the sidebar when it lists no lavoro, found by `selector`.
fn quiet_words(selector: &'static str, size: gpui::Pixels, words: Key, palette: &Palette) -> Div {
    div()
        .debug_selector(move || selector.into())
        .flex_none()
        .flex()
        .text_size(size)
        .text_color(palette.text_tertiary)
        .child(tr(words))
}

/// What a group is called: the key of its label, and the name its element is found by in any language.
type GroupName = (Key, &'static str);

/// The name of the rows whose facts have not been read yet.
const READING: GroupName = (READING_LABEL, "reading");

/// A group as the sidebar lists it: its name, its figure, its rows.
type Listed<'a> = (GroupName, (usize, bool), Vec<&'a Row>);

/// The groups of the tasks whose facts were read, then the tasks whose state is still being read:
/// selectable, quiet, with no state claimed.
fn listed(facts: &Facts) -> Vec<Listed<'_>> {
    let read = facts.rows().iter().filter(|row| !row.is_pending);
    let tasks: Vec<_> = read.map(|row| row.task.clone()).collect();
    let seen =
        |task: &sakura_core::models::Task| (facts.is_waiting(&task.name), facts.live(&task.name));
    let needs = |task: &sakura_core::models::Task| {
        let (waits, live) = seen(task);
        model::is_attention(task, waits, live)
    };
    let grouped = model::group_tasks(&tasks, &seen)
        .into_iter()
        .filter(|(_, members)| !members.is_empty())
        .map(|(group, members)| {
            let rows = members.iter().filter_map(|task| facts.row(&task.name));
            let attention = members.iter().filter(|task| needs(task)).count();
            let figure = group_figure(group, members.len(), attention);
            ((group.label(), group.id()), figure, rows.collect())
        });
    let pending: Vec<&Row> = facts.rows().iter().filter(|row| row.is_pending).collect();
    let reading = (!pending.is_empty()).then_some((READING, (pending.len(), false), pending));
    grouped.chain(reading).collect()
}

/// The groups one under the other. The first group's label is the sidebar's header; the others take one
/// row each.
fn groups(
    listed: Vec<Listed<'_>>,
    at: Place,
    palette: &Palette,
    cx: &Context<SidebarView>,
) -> Vec<AnyElement> {
    listed
        .into_iter()
        .enumerate()
        .map(|(index, (label, figure, rows))| {
            let rows = rows.into_iter().map(|row| task_row(row, at, palette, cx));
            let label = (index > 0).then(|| group_label(label, figure, palette));
            div()
                .flex()
                .flex_col()
                .children(label)
                .children(rows)
                .into_any_element()
        })
        .collect()
}

/// A group's label on a row of its own, its words on the sidebar's inset.
fn group_label(label: GroupName, figure: (usize, bool), palette: &Palette) -> impl IntoElement {
    div()
        .h(theme::ROW_HEIGHT)
        .flex_none()
        .px(theme::ROW_PADDING_X)
        .flex()
        .items_center()
        .child(group_words(label, figure, palette))
}

/// The label and its figure: the number of rows in the group, a step stronger when one of them needs the
/// person. The accent is carried by the state mark of that row, never by small text (its contrast as
/// text on the light theme is under 4).
fn group_words(
    (label, id): GroupName,
    (count, is_attention): (usize, bool),
    palette: &Palette,
) -> Div {
    let count_colour = figure_colour(is_attention, palette);
    let selector = format!("group-{id}");
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap(theme::GAP_SMALL)
        .text_size(theme::TEXT_LABEL)
        .text_color(palette.text_tertiary)
        .child(div().debug_selector(move || selector).child(tr(label)))
        .child(
            div()
                .font_features(tabular())
                .text_color(count_colour)
                .child(count.to_string()),
        )
}

/// The colour of a group's figure: the secondary text colour when a row of the group needs the person,
/// the quiet one otherwise; never the accent.
fn figure_colour(is_attention: bool, palette: &Palette) -> gpui::Hsla {
    if is_attention {
        palette.text_secondary
    } else {
        palette.text_tertiary
    }
}

/// The figure beside a group label and whether it is marked: the figure is always the number of rows in
/// the group; only the group of what needs the person is marked, when it has a row.
fn group_figure(group: Group, total: usize, attention: usize) -> (usize, bool) {
    (total, group == Group::ServeTe && attention > 0)
}

fn task_row(
    row: &Row,
    at: Place,
    palette: &Palette,
    cx: &Context<SidebarView>,
) -> impl IntoElement {
    let is_selected = at.selected == Some(row.task.name.as_str());
    let name = SharedString::from(row.task.name.clone());
    let selected_name = row.task.name.clone();
    let desk = at.desk.clone();
    // A click shows the lavoro in the tab on screen; with Cmd it opens in a tab of its own, behind.
    let on_click = move |event: &gpui::ClickEvent, _: &mut gpui::Window, cx: &mut gpui::App| {
        let is_behind = event.modifiers().platform;
        drop(desk.update(cx, |desk, cx| match is_behind {
            true => desk.open_behind(selected_name.clone(), cx),
            false => desk.select(selected_name.clone(), cx),
        }));
    };
    let selector = format!("row-{name}");
    let glyph_selector = format!("glyph-{name}");
    div()
        .id(SharedString::from(format!("row-{name}")))
        .debug_selector(move || selector)
        .h(theme::ROW_HEIGHT)
        .flex_none()
        .px(theme::ROW_PADDING_X)
        .flex()
        .items_center()
        .gap(theme::ROW_GAP)
        .rounded(theme::RADIUS_CONTROL)
        .text_size(theme::TEXT_BODY)
        .map(|row| row_look(row, is_selected, palette))
        .on_click(on_click)
        .child(glyph_column(
            row_glyph(
                row,
                at.waiting.contains(&row.task.name),
                at.facts.live(&row.task.name),
                palette,
                cx,
            ),
            glyph_selector,
        ))
        .child(row_label(name))
        .children((!row.is_pending).then(|| row_trailing(row, palette)))
}

/// The name, in the room the glyph and the trailing figures leave it: its box is the same whatever its
/// weight.
fn row_label(name: SharedString) -> impl IntoElement {
    let (selector, text_selector) = (format!("label-{name}"), format!("name-{name}"));
    // The name itself is as wide as its text (or the room, with an ellipsis): its width tells its weight.
    let text = div()
        .debug_selector(move || text_selector)
        .min_w_0()
        .truncate()
        // The name is shown as a label; the row's identity above keeps it as `box` wrote it.
        .child(sakura_core::display::label(&name));
    div()
        .debug_selector(move || selector)
        .flex_1()
        .min_w_0()
        .flex()
        .child(text)
}

/// The state's glyph; an empty ring in the neutral tone while the state is being read.
fn row_glyph(
    row: &Row,
    is_waiting: bool,
    live: model::Live,
    palette: &Palette,
    cx: &Context<SidebarView>,
) -> AnyElement {
    let breath = Breath::of(Owner::Sidebar, cx);
    if row.is_pending {
        return glyph(Shape::Ring, Tone::Neutral, palette, breath);
    }
    let state = model::shown_state_live(&row.task, is_waiting, live);
    let place = format!("row-{}", row.task.name);
    placed_state_glyph(&place, state, palette, breath)
}

/// A selected row is its fill and the full ink, at the weight of every other row: selection never makes a
/// word heavier or wider. The others brighten on hover only (never animated).
fn row_look(row: Stateful<Div>, is_selected: bool, palette: &Palette) -> Stateful<Div> {
    if is_selected {
        row.bg(palette.selected_row).text_color(palette.text)
    } else {
        let hover = palette.hover;
        row.text_color(palette.text_secondary)
            .hover(move |style| style.bg(hover))
    }
}

/// The right side of a row: the age of old facts, then the changed lines (a part that is zero is left out), or
/// the state word when the task has no readable figures.
fn row_trailing(row: &Row, palette: &Palette) -> impl IntoElement {
    let task = &row.task;
    let age = model::facts_age(row.fetched_at.elapsed().as_secs());
    let selector = format!("trailing-{}", task.name);
    let trailing = div()
        .debug_selector(move || selector)
        .flex_none()
        .flex()
        .items_center()
        .gap(theme::GAP_SMALL)
        .text_size(theme::TEXT_LABEL)
        .font_weight(FontWeight::NORMAL)
        .font_features(tabular())
        .children(age.map(|age| div().text_color(palette.text_tertiary).child(say(&age))));
    match (task.readable, task.insertions, task.deletions) {
        (true, Some(added), Some(removed)) if added + removed > 0 => {
            let (added_text, removed_text) = crate::l10n::signed_lines(added, removed);
            trailing
                .when(added > 0, |row| {
                    row.child(div().text_color(palette.success).child(added_text))
                })
                .when(removed > 0, |row| {
                    row.child(div().text_color(palette.danger).child(removed_text))
                })
        }
        (false, _, _) => trailing
            .text_color(palette.text_tertiary)
            .child(tr(model::task_state(task).word)),
        _ => trailing,
    }
}

fn skeleton(palette: &Palette) -> impl IntoElement {
    let bars = theme::SKELETON_BAR_WIDTHS.iter().map(|width| {
        div()
            .h(theme::ROW_HEIGHT)
            .px(theme::ROW_PADDING_X)
            .flex()
            .items_center()
            .child(
                div()
                    .w(px(*width))
                    .h(px(theme::SKELETON_BAR_HEIGHT))
                    .rounded(theme::RADIUS_CHIP)
                    .bg(palette.hover),
            )
    });
    div().flex().flex_col().children(bars)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_group_of_what_needs_the_person_marks_its_figure() {
        assert_eq!(group_figure(Group::ServeTe, 3, 3), (3, true));
    }

    #[test]
    fn a_group_s_figure_is_never_written_in_the_accent() {
        for palette in [Palette::dark(), Palette::light()] {
            assert_eq!(figure_colour(true, &palette), palette.text_secondary);
            assert_eq!(figure_colour(false, &palette), palette.text_tertiary);
            assert_ne!(figure_colour(true, &palette), palette.attention);
        }
    }

    #[test]
    fn what_is_to_review_shows_a_muted_total_whatever_its_rows() {
        assert_eq!(group_figure(Group::DaGuardare, 3, 0), (3, false));
        assert_eq!(group_figure(Group::DaGuardare, 3, 1), (3, false));
    }

    #[test]
    fn other_groups_always_show_a_muted_total() {
        assert_eq!(group_figure(Group::AlLavoro, 2, 2), (2, false));
        assert_eq!(group_figure(Group::Presi, 1, 0), (1, false));
    }
}
