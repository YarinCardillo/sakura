//! The Lanes page: every lane of every project, grouped by the state it is in (Needs you, Running, Finished,
//! Stopped; the last two folded until opened). A row says the lane's title, its project, the step it is at
//! with how many are done and a thin line of it, the preset it runs as and its age. A press opens the lane in
//! the tab on screen, Cmd in a tab of its own. The header's Filter keeps one project or one preset, and "New
//! lane" starts a lane from here.

use super::chrome::{tabular, text};
use super::engine_desk::Desk;
use super::kit::progress_line;
use super::lanes::{self, LaneFilter, LaneGroup, StepWords};
use super::sidebar_pages::go_from;
use super::unix_now;
use crate::data::engine_facts::EngineFacts;
use crate::l10n::{tr, tr_with, Key};
use crate::prefs::Preferences;
use crate::theme::{self, lanes_page as tokens, Palette, Theme};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    div, AnyElement, Context, Entity, InteractiveElement as _, IntoElement, ListAlignment,
    ListState, ParentElement, Render, SharedString, StatefulInteractiveElement as _, Styled,
    Subscription, WeakEntity, Window,
};
use gpui_component::{Icon, Sizable as _};
use gpui_kit_assets::IconName;
use sakura_core::engine::snapshot::{Item, Snapshot};
use sakura_core::tabs::{Page, TabId};
use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;

/// What a group of the list is called, by the group.
const NEEDS_YOU: Key = Key("lanes_page.group.needs_you");
const RUNNING: Key = Key("lanes_page.group.running");
const FINISHED: Key = Key("lanes_page.group.finished");
const STOPPED: Key = Key("lanes_page.group.stopped");
/// What the page says while there is nothing to list.
const EMPTY: Key = Key("lanes_page.empty");
const EMPTY_FILTERED: Key = Key("lanes_page.empty_filtered");
/// What a lane an outside agent has left to decide.
const TO_DECIDE: Key = Key("lanes_page.to_decide");

/// The Lanes page of the tab shown.
pub struct LanesPage {
    pub(super) desk: WeakEntity<Desk>,
    tab: TabId,
    is_compact: bool,
    pub(super) facts: Entity<EngineFacts>,
    pub(super) filter: LaneFilter,
    open_groups: BTreeSet<String>,
    /// How far the list is scrolled, and the keys that scroll it.
    scroll: super::page_scroll::PageScroll,
    list: ListState,
    rows: Vec<LaneRow>,
    listed: Option<Rc<Snapshot>>,
    _observers: [Subscription; 2],
}

#[derive(Clone, Copy)]
enum LaneRow {
    Columns,
    Empty,
    Header(LaneGroup, usize, bool),
    Lane(usize),
}

impl LanesPage {
    /// The page over the engine's `facts`, listing every lane.
    pub fn new(
        desk: WeakEntity<Desk>,
        tab: TabId,
        facts: Entity<EngineFacts>,
        open_groups: BTreeSet<String>,
        cx: &mut Context<Self>,
    ) -> Self {
        let observers = [
            cx.observe(&facts, |_, _, cx| cx.notify()),
            cx.subscribe(
                &facts,
                |page, _, _: &crate::data::engine_facts::WaitsChanged, cx| {
                    page.listed = None;
                    cx.notify();
                },
            ),
        ];
        Self {
            desk,
            tab,
            is_compact: false,
            facts,
            filter: LaneFilter::default(),
            open_groups,
            scroll: super::page_scroll::PageScroll::new(cx),
            list: ListState::new(0, ListAlignment::Top, theme::GAP_WIDE),
            rows: Vec::new(),
            listed: None,
            _observers: observers,
        }
    }

    pub(super) fn scroll_for(&self, key: &gpui::Keystroke) -> bool {
        super::page_scroll::scroll_list_for(&self.list, key)
    }

    pub(super) fn is_compact(&self) -> bool {
        self.is_compact
    }

    pub(super) fn set_compact(&mut self, is_compact: bool, cx: &mut Context<Self>) {
        self.is_compact = is_compact;
        self.list.remeasure();
        cx.notify();
    }

    /// Lists only the lanes of the project in `folder`, or every lane for none.
    pub fn show_project(&mut self, folder: Option<String>, cx: &mut Context<Self>) {
        self.filter.project = folder;
        self.listed = None;
        cx.notify();
    }

    /// The project the list is kept to, by its folder.
    pub fn project(&self) -> Option<&str> {
        self.filter.project.as_deref()
    }

    pub(super) fn show_preset(&mut self, preset: Option<String>, cx: &mut Context<Self>) {
        self.filter.preset = preset;
        self.listed = None;
        cx.notify();
    }

    fn flip(&mut self, group: LaneGroup, cx: &mut Context<Self>) {
        if !is_foldable(group) {
            return;
        }
        let (_, id) = group_words(group);
        if !self.open_groups.remove(id) {
            self.open_groups.insert(id.to_string());
        }
        Preferences::change(cx, |preferences| {
            preferences.open_lane_groups.clone_from(&self.open_groups);
        });
        self.listed = None;
        cx.notify();
    }

    /// Whether the rows of `group` are shown: Finished and Stopped start folded.
    fn is_open(&self, group: LaneGroup) -> bool {
        !is_foldable(group) || self.open_groups.contains(group_words(group).1)
    }
}

impl Render for LanesPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::snapshot::count_render("lanes");
        let palette = Theme::palette(cx);
        let snapshot = self.facts.read(cx).shared_snapshot();
        if let Some(snapshot) = snapshot.as_ref() {
            if self
                .listed
                .as_ref()
                .is_none_or(|listed| !Rc::ptr_eq(listed, snapshot))
            {
                self.update_rows(snapshot);
            }
        }
        let list = div()
            .id("lanes-list")
            .debug_selector(|| "lanes-page".into())
            .size_full()
            .px(theme::PANEL_INSET)
            .pt(theme::GAP_SMALL)
            .pb(tokens::LIST_PADDING_BOTTOM)
            .flex()
            .flex_col();
        let page = cx.weak_entity();
        let rows = gpui::list(self.list.clone(), move |index, _, cx| {
            page.update(cx, |page, cx| page.draw_row(index, &palette, cx))
                .unwrap_or_else(|_| gpui::Empty.into_any_element())
        })
        .size_full();
        self.scroll.wrap_list(list.child(rows), &self.list)
    }
}

impl LanesPage {
    fn update_rows(&mut self, snapshot: &Rc<Snapshot>) {
        let found = lanes::grouped(snapshot, &self.filter);
        let indices: HashMap<_, _> = snapshot
            .items
            .iter()
            .enumerate()
            .map(|(index, item)| (std::ptr::from_ref(item), index))
            .collect();
        let listed = LaneGroup::ALL
            .iter()
            .filter(|group| !found.of(**group).is_empty())
            .count();
        let mut rows = vec![LaneRow::Columns];
        for group in LaneGroup::ALL {
            let items = found.of(group);
            if items.is_empty() {
                continue;
            }
            // The one group there is shows its rows: a lone folded band would leave the page empty.
            let is_open = self.is_open(group) || listed == 1;
            rows.push(LaneRow::Header(group, items.len(), is_open));
            if is_open {
                rows.extend(
                    items
                        .iter()
                        .map(|item| LaneRow::Lane(indices[&std::ptr::from_ref(*item)])),
                );
            }
        }
        if listed == 0 {
            rows[0] = LaneRow::Empty;
        }
        let anchor = self.list.logical_scroll_top();
        self.list.splice(0..self.rows.len(), rows.len());
        self.list.scroll_to(anchor);
        self.rows = rows;
        self.listed = Some(snapshot.clone());
    }

    fn draw_row(&self, index: usize, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        match self.rows[index] {
            LaneRow::Columns => columns(palette, self.is_compact),
            LaneRow::Empty => self.empty(palette),
            LaneRow::Header(group, count, open) => self.header(group, count, open, palette, cx),
            LaneRow::Lane(index) => {
                let snapshot = self.listed.as_ref().expect("rows have a snapshot");
                self.row(&snapshot.items[index], snapshot, unix_now(), palette)
            }
        }
    }

    fn empty(&self, palette: &Palette) -> AnyElement {
        let words = if self.filter.is_empty() {
            EMPTY
        } else {
            EMPTY_FILTERED
        };
        text(theme::TEXT_SMALL, palette.text_tertiary)
            .debug_selector(|| "lanes-empty".into())
            .px(theme::GAP_WIDE)
            .py(theme::GAP_WIDE)
            .child(tr(words))
            .into_any_element()
    }

    /// A group's band: its mark, its name, how many lanes it holds and, for a group that can be folded, a
    /// chevron. A press folds or opens it.
    fn header(
        &self,
        group: LaneGroup,
        count: usize,
        is_open: bool,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (name, selector) = group_words(group);
        let chevron = match is_open {
            true => IconName::ChevronDown,
            false => IconName::ChevronRight,
        };
        div()
            .id(SharedString::from(format!("lanes-group-{selector}")))
            .debug_selector(move || format!("lanes-group-{selector}"))
            .h(tokens::GROUP_HEIGHT)
            .w_full()
            .flex_none()
            .mt(tokens::GROUP_GAP)
            .px(tokens::ROW_PADDING_X)
            .flex()
            .items_center()
            .gap(theme::GAP)
            .text_size(theme::TEXT_LABEL)
            .font_weight(theme::WEIGHT_MEDIUM)
            .text_color(palette.text_tertiary)
            .when(is_foldable(group), |header| {
                header.on_click(cx.listener(move |page, _, _, cx| page.flip(group, cx)))
            })
            .child(tr(name))
            .child(
                div()
                    .debug_selector(move || format!("lanes-group-{selector}-count"))
                    .flex_none()
                    .font_weight(gpui::FontWeight::NORMAL)
                    .font_features(tabular())
                    .text_color(palette.text_tertiary)
                    .child(count.to_string()),
            )
            .when(is_foldable(group), |header| {
                let icon = Icon::new(chevron).with_size(theme::HAND_MARK_SIZE);
                header.child(div().text_color(palette.text_tertiary).child(icon))
            })
            .into_any_element()
    }

    /// One lane: mark, title, project, step with its line, preset and age. A press opens it.
    fn row(&self, item: &Item, snapshot: &Snapshot, now: i64, palette: &Palette) -> AnyElement {
        let (task, lane) = (item.task().to_string(), item.lane_key());
        let selector = format!("lanes-row-{task}");
        let title = div()
            .flex_1()
            .min_w_0()
            .truncate()
            .text_color(palette.text)
            .child(lanes::lane_words(item, false));
        crate::ui::kit::row(crate::ui::kit::RowKind::List, selector, false, palette)
            .h(tokens::ROW_HEIGHT)
            .w_full()
            .flex_none()
            .px(tokens::ROW_PADDING_X)
            .gap(tokens::ROW_GAP)
            .text_size(tokens::ROW_TEXT)
            .on_click(go_from(
                self.desk.clone(),
                self.tab,
                Page::Lanes,
                Page::Lane(lane.clone()),
                false,
            ))
            .on_aux_click(go_from(
                self.desk.clone(),
                self.tab,
                Page::Lanes,
                Page::Lane(lane),
                true,
            ))
            .child(
                div()
                    .w(tokens::GLYPH_COLUMN)
                    .flex_none()
                    .flex()
                    .justify_center()
                    .child(lanes::glyph_of(item).paint(tokens::GLYPH_SIZE, palette)),
            )
            .child(title)
            .child(project_label(
                item,
                &task,
                snapshot.is_project_folder_missing(&item.repo),
                palette,
            ))
            .when(!self.is_compact, |row| {
                row.child(step_cell(item, snapshot, &task, now, palette))
                    .child(preset_cell(item, &task, palette))
            })
            .child(age_cell(item, &task, now, palette))
            .into_any_element()
    }
}

/// Finished and Stopped can be folded, and start folded; the groups that need or hold work stay open.
fn is_foldable(group: LaneGroup) -> bool {
    matches!(
        group,
        LaneGroup::Finished | LaneGroup::Stopped | LaneGroup::Closed
    )
}

/// The group's words, and what names its header in the window.
fn group_words(group: LaneGroup) -> (Key, &'static str) {
    match group {
        LaneGroup::NeedsYou => (NEEDS_YOU, "needs_you"),
        LaneGroup::Running => (RUNNING, "running"),
        LaneGroup::Finished => (FINISHED, "finished"),
        LaneGroup::Stopped => (STOPPED, "stopped"),
        LaneGroup::Closed => (Key("lane.state.cleaned"), "closed"),
    }
}

fn project_label(item: &Item, task: &str, is_missing: bool, palette: &Palette) -> AnyElement {
    let selector = format!("lanes-row-{task}-project");
    let cell = div()
        .debug_selector(move || selector)
        .w(tokens::TAG_WIDTH)
        .flex_none()
        .flex()
        .flex_col()
        .items_start();
    if !is_missing && item.is_outside() && item.agent() == item.project() {
        return cell.into_any_element();
    }
    let label = format!("lanes-row-{task}-tag");
    cell.child(
        text(theme::TEXT_LABEL, palette.text_secondary)
            .debug_selector(move || label)
            .max_w_full()
            .truncate()
            .child(item.project().to_string()),
    )
    .children(is_missing.then(|| {
        let selector = format!("lanes-folder-missing-{task}");
        text(theme::TEXT_LABEL, palette.text_secondary)
            .debug_selector(move || selector)
            .child(tr(lanes::FOLDER_MISSING))
    }))
    .into_any_element()
}

/// The step the lane is at with its count, and a thin line of how far it is.
fn step_cell(
    item: &Item,
    snapshot: &Snapshot,
    task: &str,
    now: i64,
    palette: &Palette,
) -> AnyElement {
    let words: StepWords = lanes::step_words(item);
    let line = words
        .counts
        .filter(|(done, total)| *total > 0 && *done > 0)
        .map(|(done, total)| progress_line(done, total, tokens::LINE_WIDTH));
    let (selector, cell) = (
        format!("lanes-row-{task}-step-words"),
        format!("lanes-row-{task}-step"),
    );
    div()
        .debug_selector(move || cell)
        .w(tokens::STEP_WIDTH)
        .flex_none()
        .flex()
        .items_center()
        .gap(theme::GAP)
        .text_size(theme::TEXT_LABEL)
        .font_features(tabular())
        .text_color(palette.text_tertiary)
        .child(
            div()
                .debug_selector(move || selector)
                .min_w_0()
                .truncate()
                .child(progress_words(item, snapshot, now)),
        )
        .children(line)
        .into_any_element()
}

/// The preset the lane runs as; for a lane an outside agent opened, the agent's name ("Digest").
pub(super) fn preset_words(item: &Item) -> String {
    match (&item.preset, item.is_outside()) {
        (Some(_), _) => item.preset_name().unwrap_or_default(),
        (None, true) => lanes::capitalised(item.agent()),
        (None, false) => String::new(),
    }
}

/// What the Progress column says: the step of a flow lane with its count; for an outside agent's lane how
/// many of its asks are left to decide, which is all it has.
pub(super) fn progress_words(item: &Item, snapshot: &Snapshot, now: i64) -> SharedString {
    let outside = item.progress.as_ref().filter(|_| item.is_outside());
    match outside {
        Some(progress) if progress.total > 0 => {
            let snoozed = lanes::snoozed_owner_rows(item, snapshot, now);
            let left = progress
                .total
                .saturating_sub(progress.done)
                .saturating_sub(snoozed)
                .to_string();
            tr_with(TO_DECIDE, &[("n", &left)])
        }
        _ => lanes::step_line(&lanes::step_words(item)),
    }
}

fn preset_cell(item: &Item, task: &str, palette: &Palette) -> AnyElement {
    let selector = format!("lanes-row-{task}-preset");
    text(theme::TEXT_LABEL, palette.text_tertiary)
        .debug_selector(move || selector)
        .w(tokens::PRESET_WIDTH)
        .flex_none()
        .truncate()
        .child(preset_words(item))
        .into_any_element()
}

/// How long ago the lane last did something, at the row's right end.
fn age_cell(item: &Item, task: &str, now: i64, palette: &Palette) -> AnyElement {
    let selector = format!("lanes-row-{task}-age");
    text(theme::TEXT_LABEL, palette.text_tertiary)
        .debug_selector(move || selector)
        .w(tokens::AGE_WIDTH)
        .flex_none()
        .flex()
        .justify_end()
        .font_features(tabular())
        .children(age_words(item, now))
        .into_any_element()
}

/// The names of the columns, over the rows' own fixed columns: the title's room, then project, progress,
/// preset and age, each as wide as its cells and starting where they do.
fn columns(palette: &Palette, is_compact: bool) -> AnyElement {
    let cell = |name: &'static str, key: Key, width: gpui::Pixels| {
        text(theme::TEXT_LABEL, palette.text_tertiary)
            .debug_selector(move || format!("lanes-column-{name}"))
            .w(width)
            .flex_none()
            .child(tr(key))
    };
    div()
        .debug_selector(|| "lanes-columns".into())
        .h(tokens::COLUMNS_HEIGHT)
        .w_full()
        .flex_none()
        .px(tokens::ROW_PADDING_X)
        .flex()
        .items_center()
        .gap(tokens::ROW_GAP)
        .child(div().w(tokens::GLYPH_COLUMN).flex_none())
        .child(div().flex_1().min_w_0().child(
            text(theme::TEXT_LABEL, palette.text_tertiary).child(tr(Key("lanes_page.column.lane"))),
        ))
        .child(cell(
            "project",
            Key("lanes_page.column.project"),
            tokens::TAG_WIDTH,
        ))
        .when(!is_compact, |row| {
            row.child(cell(
                "step",
                Key("lanes_page.column.progress"),
                tokens::STEP_WIDTH,
            ))
            .child(cell(
                "preset",
                Key("lanes_page.column.preset"),
                tokens::PRESET_WIDTH,
            ))
        })
        .child(
            div()
                .debug_selector(|| "lanes-column-age".into())
                .w(tokens::AGE_WIDTH)
                .flex_none()
                .flex()
                .justify_end()
                .text_size(theme::TEXT_LABEL)
                .text_color(palette.text_tertiary)
                .child(tr(Key("lanes_page.column.updated"))),
        )
        .into_any_element()
}

fn age_words(item: &Item, now: i64) -> Option<SharedString> {
    super::relative_age_words(
        item.last_activity_at.as_deref().or(item.since.as_deref()),
        now,
    )
}

impl Desk {
    /// Shows the Lanes page kept to the lanes of the project in `folder`, in the tab on screen or, with
    /// `is_new_tab`, in a tab of its own.
    pub fn open_lanes_of(&mut self, folder: &str, is_new_tab: bool, cx: &mut Context<Self>) {
        let folder = folder.to_string();
        self.navigate(Page::Lanes, is_new_tab, cx);
        let id = self.bar.read(cx).tabs().active().id();
        self.lanes_view(id, cx)
            .update(cx, |page, cx| page.show_project(Some(folder), cx));
    }
}
