//! The graph on screen: a virtualised list of rows on the grid's one row pitch, each the lanes, the
//! commit's subject (cut when the row is short, never wrapped), the lanes that end at the commit, and
//! its age in tabular figures. A task's lane ends in the task's state mark, which stands on the lane in
//! the place of its last commit's dot, with the task's name at the row's end; the base's end is the
//! branch's name, in the same face. A task that cannot be read is said in a
//! quiet line over the list.
//!
//! A row too narrow for all of it keeps the lanes, with the states on them, and the subject, and lets
//! go of the age first, then of the names: the row wraps, and what wraps under it is not shown.

use super::lanes::{self, LaneColours};
use super::{Drawn, GitGraph, Shown};
use crate::l10n::{tr, tr_with, Key};
use crate::theme::{self, Palette, Theme};
use crate::ui::chrome::tabular;
use gpui::{
    canvas, div, prelude::FluentBuilder as _, uniform_list, AnyElement, App, Context, Div,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window,
};
use gpui_component::scroll::ScrollableElement as _;
use sakura_core::graph::{Age, Lane};
use std::ops::Range;
use std::rc::Rc;

/// The words of an age: `now`, `5 min`, `3 h`, `2 d`.
fn age_words(age: Age) -> SharedString {
    let counted = |key: &'static str, count: u64| tr_with(Key(key), &[("n", &count.to_string())]);
    match age {
        Age::Now => tr(Key("graph.age.now")).into(),
        Age::Minutes(count) => counted("graph.age.minutes", count),
        Age::Hours(count) => counted("graph.age.hours", count),
        Age::Days(count) => counted("graph.age.days", count),
    }
}

/// The subject keeps at least the width of this many lanes before the row lets go of what follows it.
const SUBJECT_MIN_LANES: usize = 4;

/// One quiet sentence on the line the rows start on.
fn sentence(words: impl Into<SharedString>, palette: &Palette) -> Div {
    div()
        .px(theme::SIDE_PANE_INSET)
        .h(theme::ROW_HEIGHT)
        .flex_none()
        .flex()
        .items_center()
        .gap(theme::GAP)
        .text_size(theme::TEXT_SMALL)
        .text_color(palette.text_tertiary)
        .child(div().min_w_0().truncate().child(words.into()))
}

/// The end of a lane on its last commit: a task's name (with its state mark, when the state is not
/// on the lane itself), or the base branch's name.
fn lane_end(lane: &Lane, mark: Option<AnyElement>, palette: &Palette) -> Div {
    // One face for every lane's end, the interface's: a task's name and the base branch's alike.
    let name = lane.lavoro.as_deref().unwrap_or(&lane.branch);
    let name = div().child(sakura_core::display::label(name));
    let named = lane.lavoro.as_deref().unwrap_or(&lane.branch);
    let (selector, chip_selector) = (format!("graph-end-{named}"), format!("graph-chip-{named}"));
    // The chip gives way with the room its end has, so a long name is cut inside it and never runs
    // under the age beside it.
    let chip = div()
        .debug_selector(move || chip_selector)
        .min_w_0()
        .h(theme::STATUS_CONTROL_HEIGHT)
        .px(theme::GAP_SMALL)
        .flex()
        .items_center()
        .gap(theme::GAP_TIGHT)
        .rounded(theme::RADIUS_CHIP)
        .bg(palette.fill)
        .text_size(theme::TEXT_LABEL)
        .text_color(palette.text_secondary)
        .children(mark)
        .child(name.min_w_0().truncate());
    // As tall as the row, so the row's one line is: the chip is centred in it.
    div()
        .debug_selector(move || selector)
        .h(theme::ROW_HEIGHT)
        .flex_none()
        .max_w(gpui::relative(theme::CHANGES_SHARE_DEFAULT))
        .flex()
        .items_center()
        .child(chip)
}

/// The lanes of row `index`, painted when the row is. `mark` is the state of the task whose lane ends
/// here: it stands on the lane in the place of the commit's dot.
fn lanes_of(
    drawn: &Rc<Drawn>,
    index: usize,
    colours: LaneColours,
    mark: Option<AnyElement>,
) -> Div {
    let (painted, has_mark) = (drawn.clone(), mark.is_some());
    let paint = move |bounds, (), window: &mut Window, _: &mut App| {
        lanes::paint(
            bounds,
            &painted.layout.rows[index],
            colours,
            has_mark,
            window,
        );
    };
    let column = drawn.layout.rows[index].column;
    let mark = mark.map(|mark| {
        div()
            .absolute()
            .left(theme::ICON_SIZE * column as f32)
            .top((theme::ROW_HEIGHT - theme::MARK_SIZE) / 2.)
            .child(mark)
    });
    div()
        .relative()
        .w(lanes::width(drawn.layout.columns))
        .h(theme::ROW_HEIGHT)
        .flex_none()
        .child(canvas(|_, _, _| (), paint).size_full())
        .children(mark)
}

/// How long ago the commit `sha` was written, at the row's end.
fn age_of(age: Age, sha: &str, palette: &Palette) -> Div {
    let selector = format!("graph-age-{sha}");
    div()
        .debug_selector(move || selector)
        .h(theme::ROW_HEIGHT)
        .flex_none()
        .flex()
        .items_center()
        .text_size(theme::TEXT_LABEL)
        .text_color(palette.text_tertiary)
        .font_features(tabular())
        .child(age_words(age))
}

impl GitGraph {
    /// The lane of the commit's own task when it ends at row `index`, with the task's state mark: the
    /// mark goes on the lane.
    fn own_end(&self, drawn: &Drawn, index: usize, cx: &App) -> Option<(usize, AnyElement)> {
        let row = &drawn.layout.rows[index];
        let task = drawn.graph.commits[row.commit].lavoro.as_deref()?;
        let is_own = |lane: &&usize| {
            let lane = drawn.graph.lanes.get(**lane);
            lane.is_some_and(|lane| lane.lavoro.as_deref() == Some(task))
        };
        let lane = *row.heads.iter().find(is_own)?;
        Some((lane, (self.mark)(task, cx)?))
    }

    /// The ends of the lanes whose last commit is row `index`. The lane `on_lane` has its state on
    /// the lane itself; a task that ends on a commit of the base carries its state here.
    fn ends(
        &self,
        drawn: &Drawn,
        (index, on_lane): (usize, Option<usize>),
        palette: &Palette,
        cx: &App,
    ) -> Vec<Div> {
        let heads = drawn.layout.rows[index].heads.iter();
        let ends = heads.filter_map(|&head| {
            let lane = drawn.graph.lanes.get(head)?;
            let task = lane.lavoro.as_deref().filter(|_| Some(head) != on_lane);
            let mark = task.and_then(|task| (self.mark)(task, cx));
            Some(lane_end(lane, mark, palette))
        });
        ends.collect()
    }

    /// Row `index` of `drawn`: a press picks its commit, when it is a task's.
    fn row(&self, drawn: &Rc<Drawn>, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let palette = Theme::palette(cx);
        let row = &drawn.layout.rows[index];
        let commit = &drawn.graph.commits[row.commit];
        let is_picked = self.picked.as_deref() == Some(commit.sha.as_str());
        let colours = LaneColours::of(commit.lavoro.is_some(), &palette);
        let (on_lane, mark) = self.own_end(drawn, index, cx).unzip();
        let (sha, hover) = (commit.sha.clone(), palette.hover);
        let selector = format!("graph-row-{}", commit.sha);
        div()
            .id(index)
            .debug_selector(move || selector)
            .w_full()
            .h(theme::ROW_HEIGHT)
            .px(theme::ROW_PADDING_X)
            .flex()
            .flex_wrap()
            .content_start()
            .overflow_hidden()
            .gap_x(theme::ROW_GAP)
            .rounded(theme::RADIUS_CONTROL)
            .text_size(theme::TEXT_BODY)
            .text_color(palette.text)
            .when(is_picked, |row| row.bg(palette.selected_row))
            // `box` gives the diff of a task's own commits only: a commit of the base branch is read,
            // not pressed, so it neither answers the pointer nor takes a press.
            .when(commit.lavoro.is_some(), |row| {
                row.when(!is_picked, |row| row.hover(move |style| style.bg(hover)))
                    .on_click(cx.listener(move |graph, _, _, cx| graph.pick(sha.clone(), cx)))
            })
            .child(lanes_of(drawn, index, colours, mark))
            .child(subject(&commit.subject, &commit.sha))
            .children(self.ends(drawn, (index, on_lane), &palette, cx))
            .children(drawn.ages[row.commit].map(|age| age_of(age, &commit.sha, &palette)))
            .into_any_element()
    }

    /// The rows of `drawn`, only those on screen, with the tasks that cannot be read said over them.
    fn list(&self, drawn: Rc<Drawn>, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let unreadable = drawn.graph.lanes.iter().filter(|lane| !lane.readable);
        let notes = unreadable.filter_map(|lane| {
            let task = sakura_core::display::label(lane.lavoro.as_deref()?);
            let words = tr_with(Key("graph.lane.unreadable"), &[("task", &task)]);
            let selector = format!("graph-unreadable-{task}");
            Some(sentence(words, palette).debug_selector(move || selector))
        });
        let notes: Vec<Div> = notes.collect();
        let count = drawn.layout.rows.len();
        let rows = cx.processor(move |graph: &mut Self, range: Range<usize>, _, cx| {
            let row = |index| graph.row(&drawn, index, cx);
            range.map(row).collect::<Vec<_>>()
        });
        let list = uniform_list("graph-rows", count, rows)
            .track_scroll(&self.scroll)
            .size_full();
        let scrolled = div()
            .id("graph-list")
            .flex_1()
            .min_h_0()
            .px(theme::SIDE_PANE_INSET - theme::ROW_PADDING_X)
            .child(list)
            .vertical_scrollbar(&self.scroll);
        div()
            .size_full()
            .flex()
            .flex_col()
            .children(notes)
            .child(scrolled)
            .into_any_element()
    }

    /// Why there is no graph to show, with a way to ask again when reading failed.
    fn unreadable(&self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let (fill, hover) = (palette.fill, palette.fill_hover);
        let retry = div()
            .id("graph-retry")
            .debug_selector(|| "graph-retry".into())
            .h(theme::STATUS_CONTROL_HEIGHT)
            .px(theme::CONTROL_PADDING_X)
            .flex_none()
            .flex()
            .items_center()
            .rounded(theme::RADIUS_CONTROL)
            .bg(fill)
            .hover(move |style| style.bg(hover))
            .font_weight(theme::WEIGHT_MEDIUM)
            .text_color(palette.text)
            .on_click(cx.listener(|graph, _, _, cx| graph.refresh(cx)))
            .child(tr(Key("common.retry")));
        sentence(tr(Key("graph.unreadable")), palette)
            .child(retry)
            .into_any_element()
    }
}

/// A commit's subject on one line, cut with an ellipsis where the row ends.
fn subject(subject: &str, sha: &str) -> Div {
    let selector = format!("graph-subject-{sha}");
    let words = div()
        .min_w_0()
        .truncate()
        .child(sakura_core::display::label(subject));
    div()
        .debug_selector(move || selector)
        .h(theme::ROW_HEIGHT)
        .flex_1()
        .min_w(lanes::width(SUBJECT_MIN_LANES))
        .flex()
        .items_center()
        .child(words)
}

impl Render for GitGraph {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Theme::palette(cx);
        match &self.shown {
            Shown::Reading => sentence(tr(Key("graph.reading")), &palette).into_any_element(),
            Shown::Failed => self.unreadable(&palette, cx),
            Shown::Graph(drawn) if drawn.layout.rows.is_empty() => {
                sentence(tr(Key("graph.none")), &palette).into_any_element()
            }
            Shown::Graph(drawn) => self.list(drawn.clone(), &palette, cx),
        }
    }
}
