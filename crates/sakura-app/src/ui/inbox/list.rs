//! The virtual Inbox list keeps asks grouped and Done ordered by answer time.

use super::face::State;
use super::model::{self, Filter, Section};
use super::{InboxPage, Shown};
use crate::l10n::{tr, Key};
use crate::theme::{self, inbox as sizes, Palette};
use crate::ui::chrome::tabular;
pub(super) use crate::ui::unix_now as now;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    div, AnyElement, Context, ElementId, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _,
};
use sakura_core::engine::snapshot::{Decided, Snapshot, Wait};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

#[derive(Clone)]
enum ListRow {
    Section(Section),
    Band(usize, SharedString, Option<String>, Option<SharedString>),
    Ask(usize),
    Done(usize),
}

impl ListRow {
    fn selectable_id(&self, snapshot: &Snapshot) -> Option<String> {
        match self {
            Self::Ask(index) => Some(snapshot.waits[*index].id.clone()),
            Self::Done(index) => snapshot
                .decided
                .as_ref()
                .map(|done| model::decided_key(&done[*index])),
            Self::Section(_) | Self::Band(..) => None,
        }
    }
}

#[derive(PartialEq, Eq)]
struct ListInput {
    shown: Shown,
    filter: Filter,
    hidden: HashSet<String>,
    held: HashSet<String>,
    snoozed: HashSet<String>,
    language: crate::l10n::Language,
}

pub(super) struct Rows {
    snapshot: Rc<Snapshot>,
    input: ListInput,
    items: Vec<ListRow>,
    ids: Rc<[String]>,
}

impl InboxPage {
    pub(super) fn ids(&mut self, snapshot: &Rc<Snapshot>, cx: &gpui::App) -> Rc<[String]> {
        self.refresh_rows(snapshot, cx);
        self.listed.as_ref().expect("refreshed rows").ids.clone()
    }

    fn refresh_rows(&mut self, snapshot: &Rc<Snapshot>, cx: &gpui::App) {
        let now = now();
        let input = ListInput {
            shown: self.shown,
            filter: self.filter.clone(),
            hidden: self
                .sending
                .keys()
                .filter(|id| self.is_hidden(id, cx))
                .cloned()
                .collect(),
            held: self.held.asks().into_iter().collect(),
            snoozed: snapshot
                .waits
                .iter()
                .filter(|wait| wait.is_snoozed_at(now))
                .map(|wait| wait.id.clone())
                .collect(),
            language: crate::l10n::language(),
        };
        if self.listed.as_ref().is_none_or(|rows| {
            input != rows.input
                || (!Rc::ptr_eq(snapshot, &rows.snapshot) && snapshot != &rows.snapshot)
        }) {
            let items = match self.shown {
                Shown::Open => open_rows(snapshot, &input),
                Shown::Done => done_rows(snapshot, &input.filter),
            };
            let anchor = self.list_scroll.logical_scroll_top();
            self.list_scroll
                .splice(0..self.list_scroll.item_count(), items.len());
            self.list_scroll.scroll_to(anchor);
            let ids = items
                .iter()
                .filter_map(|row| row.selectable_id(snapshot))
                .collect();
            self.listed = Some(Rows {
                snapshot: snapshot.clone(),
                input,
                items,
                ids,
            });
        }
    }

    pub(super) fn list(&mut self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let page = cx.weak_entity();
        let palette = *palette;
        let now = now();
        let rows = gpui::list(self.list_scroll.clone(), move |index, _, cx| {
            page.update(cx, |page, cx| page.draw_list_row(index, now, &palette, cx))
                .unwrap_or_else(|_| gpui::Empty.into_any_element())
        })
        .size_full();
        div()
            .id("inbox-list")
            .debug_selector(|| "inbox-list".into())
            .flex_none()
            .when(self.is_compact, |list| {
                list.w_full().h(gpui::relative(
                    crate::theme::page_split::NARROW_PANE_SECONDARY_FRACTION,
                ))
            })
            .when(!self.is_compact, |list| {
                list.w(sizes::LIST_WIDTH - crate::theme::PANEL_RING_WIDTH)
            })
            .px(sizes::LIST_PADDING)
            .pt(sizes::LIST_PADDING)
            .pb(sizes::LIST_PADDING_BOTTOM)
            .child(rows)
            .into_any_element()
    }

    pub(super) fn reveal_selected_row(&self) {
        let Some(rows) = &self.listed else { return };
        let Some(selected) = self.selected.as_deref() else {
            return;
        };
        let index = rows
            .items
            .iter()
            .position(|row| row.selectable_id(&rows.snapshot).as_deref() == Some(selected));
        if let Some(index) = index {
            self.list_scroll.scroll_to_reveal_item(index);
        }
    }

    fn draw_list_row(
        &mut self,
        index: usize,
        now: i64,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let rows = self.listed.as_ref().expect("the list has rows");
        let snapshot = rows.snapshot.clone();
        match rows.items[index].clone() {
            ListRow::Section(section) => section_heading(section, palette),
            ListRow::Band(index, title, date, trailing) => {
                band(index, title, date, trailing, palette)
            }
            ListRow::Ask(index) => {
                self.draw_ask(&snapshot, &snapshot.waits[index], now, palette, cx)
            }
            ListRow::Done(index) => {
                let decided = &snapshot.decided.as_ref().expect("the list has answers")[index];
                self.draw_answer(&snapshot, decided, now, palette, cx)
            }
        }
    }

    fn draw_ask(
        &mut self,
        snapshot: &Snapshot,
        wait: &Wait,
        now: i64,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let title = super::detail::ask_title(wait, snapshot);
        let look = Row {
            id: &wait.id,
            title: &title,
            sub: model::row_context(wait, snapshot),
            state: State::of_wait(wait),
            age: if self.filter.snoozed {
                model::wake_words(wait)
            } else {
                model::relative_age(wait.since.as_deref(), now)
            },
            is_selected: self.selected.as_deref() == Some(wait.id.as_str()),
            is_unread: !self.is_read(wait),
            answered: self.held_words(&wait.id),
        };
        self.row(look, "ask", palette, cx)
    }

    fn draw_answer(
        &mut self,
        snapshot: &Snapshot,
        decided: &Decided,
        now: i64,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = model::decided_key(decided);
        let title = decided_title(decided, snapshot);
        let look = Row {
            id: &id,
            title: &title,
            sub: decided_context(decided, snapshot),
            state: State::Done,
            age: model::relative_age(decided.at.as_deref(), now),
            is_selected: self.selected.as_deref() == Some(id.as_str()),
            is_unread: false,
            answered: None,
        };
        self.row(look, "done", palette, cx)
    }

    fn row(
        &mut self,
        look: Row,
        prefix: &str,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selector = format!("{prefix}-{}", look.id);
        let id = look.id.to_string();
        let missing_age = format!("missing-age-{id}");
        if look.answered.is_some() {
            return self.answered_row(&look, &selector, palette, cx);
        }
        let dot_selector = format!("dot-{prefix}-{}", look.id);
        let dot = div().size(sizes::UNREAD_DOT).rounded_full().when_some(
            look.is_unread.then_some(palette.attention),
            |dot, colour| dot.debug_selector(move || dot_selector).bg(colour),
        );
        let (title_selector, age_selector) = (
            format!("row-title-{prefix}-{}", look.id),
            format!("row-age-{prefix}-{}", look.id),
        );
        let first = div()
            .flex()
            .items_center()
            .gap(theme::GAP)
            .child(div().w(sizes::UNREAD_COLUMN).flex_none().child(dot))
            .child(
                div()
                    .debug_selector(move || title_selector)
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(theme::TEXT_SMALL)
                    .text_color(if look.is_unread {
                        palette.text
                    } else {
                        palette.text_secondary
                    })
                    .child(look.title.to_string()),
            )
            .child(
                div()
                    .debug_selector({
                        let id = format!("state-{prefix}-{}", look.id);
                        move || id
                    })
                    .child(look.state.glyph(palette)),
            )
            .child(
                div()
                    .debug_selector(move || age_selector)
                    .w(sizes::AGE_WIDTH)
                    .flex_none()
                    .text_align(gpui::TextAlign::Right)
                    .whitespace_nowrap()
                    .truncate()
                    .text_size(theme::TEXT_LABEL)
                    .text_color(palette.text_tertiary)
                    .font_features(tabular())
                    .child(match look.age {
                        Some(age) => age.into_any_element(),
                        None => div()
                            .debug_selector(move || missing_age)
                            .child("–")
                            .into_any_element(),
                    }),
            );
        let sub_selector = format!("sub-{prefix}-{}", look.id);
        let second = div()
            .debug_selector(move || sub_selector)
            .pl(sizes::UNREAD_COLUMN + theme::GAP)
            .min_w_0()
            .truncate()
            .text_size(theme::TEXT_LABEL)
            .text_color(palette.text_tertiary)
            .child(look.sub);
        let (hover, selected) = (palette.hover, palette.selected_row);
        div()
            .id(ElementId::Name(selector.clone().into()))
            .w_full()
            .debug_selector(move || selector)
            .mt(sizes::ROW_GAP_ABOVE)
            .px(sizes::ROW_PADDING_X)
            .pt(sizes::ROW_PADDING_TOP)
            .pb(sizes::ROW_PADDING_BOTTOM)
            .rounded(sizes::ROW_RADIUS)
            .flex()
            .flex_col()
            .gap(sizes::ROW_LINE_GAP)
            .when(look.is_selected, |row| row.bg(selected))
            .when(!look.is_selected, |row| {
                row.hover(move |style| style.bg(hover))
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                this.open_row(id.clone(), cx);
                window.focus(&this.focus, cx);
            }))
            .child(first)
            .child(second)
            .into_any_element()
    }
}

impl InboxPage {
    fn answered_row(
        &mut self,
        look: &Row,
        selector: &str,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = look.id.to_string();
        let title_selector = format!("row-title-{selector}");
        let selected = palette.selected_row;
        let line = div()
            .flex()
            .items_center()
            .gap(theme::GAP)
            .child(div().w(sizes::UNREAD_COLUMN).flex_none())
            .child(
                div()
                    .debug_selector(move || title_selector)
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(theme::TEXT_SMALL)
                    .text_color(palette.text_tertiary)
                    .child(SharedString::from(look.title.to_string())),
            );
        div()
            .id(ElementId::Name(selector.to_string().into()))
            .w_full()
            .debug_selector({
                let selector = selector.to_string();
                move || selector.clone()
            })
            .mt(sizes::ROW_GAP_ABOVE)
            .px(sizes::ROW_PADDING_X)
            .pt(sizes::ROW_PADDING_TOP)
            .pb(sizes::ROW_PADDING_BOTTOM)
            .rounded(sizes::ROW_RADIUS)
            .when(look.is_selected, |row| row.bg(selected))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.select(id.clone(), cx);
                window.focus(&this.focus, cx);
            }))
            .child(line)
            .into_any_element()
    }
}

/// What a row says of its ask.
struct Row<'a> {
    id: &'a str,
    title: &'a str,
    sub: SharedString,
    state: State,
    age: Option<SharedString>,
    is_selected: bool,
    is_unread: bool,
    /// The held answer, when the row is waiting for the undo period to end.
    answered: Option<SharedString>,
}

/// The recorded title, or the lane's display name when the engine kept no title.
pub(super) fn decided_title(decided: &Decided, snapshot: &Snapshot) -> String {
    let lane = decided_lane_name(decided, snapshot);
    if decided.kind == "approval" && model::decided_gate(decided).as_deref() == Some("spec") {
        return super::detail::spec_ask_title(
            lane.as_deref().unwrap_or_else(|| tr(Key("lane.untitled"))),
        );
    }
    if decided.title.is_empty() {
        lane.unwrap_or_else(|| model::inbox_kind_words(model::history_kind(decided)).to_string())
    } else {
        decided.title.clone()
    }
}

pub(super) fn decided_context(decided: &Decided, snapshot: &Snapshot) -> SharedString {
    let answer = model::answer_words(decided);
    let lane = decided_lane_name(decided, snapshot);
    lane.map_or_else(|| answer.clone(), |lane| format!("{answer}, {lane}").into())
}

fn decided_lane_name(decided: &Decided, snapshot: &Snapshot) -> Option<String> {
    model::decided_lane(decided, snapshot).map(|lane| crate::ui::lanes::lane_words(lane, false))
}

/// A lane's band: its name (and the day, for the digest), then what the list says of it at the right, in the
/// small grey type of the kit's section header with no fill (a filled band read as a selected row). No
/// mark: the accent is for an ask nobody answered, which is a row's dot.
fn open_rows(snapshot: &Snapshot, input: &ListInput) -> Vec<ListRow> {
    let indices: HashMap<_, _> = snapshot
        .waits
        .iter()
        .enumerate()
        .map(|(index, wait)| (std::ptr::from_ref(wait), index))
        .collect();
    let shown: Vec<_> = model::groups(snapshot)
        .into_iter()
        .filter_map(|group| {
            let asks: Vec<_> = group
                .asks
                .iter()
                .copied()
                .filter(|wait| {
                    input.filter.allows(&wait.project, model::wait_kind(wait))
                        && input.filter.snoozed == input.snoozed.contains(&wait.id)
                        && !input.hidden.contains(&wait.id)
                })
                .collect();
            (!asks.is_empty()).then_some((group, asks))
        })
        .collect();
    let has_several = shown.len() > 1;
    let mut rows = Vec::new();
    let mut section = None;
    for (index, (group, asks)) in shown.into_iter().enumerate() {
        if section != Some(group.section) {
            rows.push(ListRow::Section(group.section));
            section = Some(group.section);
        }
        let trailing = model::band_trailing(&group, &asks, snapshot, has_several);
        rows.push(ListRow::Band(index, group.title, group.date, trailing));
        rows.extend(
            asks.into_iter()
                .map(|wait| ListRow::Ask(indices[&std::ptr::from_ref(wait)])),
        );
    }
    rows
}

fn section_heading(section: Section, palette: &Palette) -> AnyElement {
    let key = section.label();
    div()
        .debug_selector(move || key.0.into())
        .px(sizes::ROW_PADDING_X + sizes::UNREAD_COLUMN + theme::GAP)
        .py(theme::GAP)
        .text_color(palette.text_secondary)
        .text_size(theme::TEXT_SMALL)
        .font_weight(theme::WEIGHT_MEDIUM)
        .child(tr(key))
        .into_any_element()
}

fn done_rows(snapshot: &Snapshot, filter: &Filter) -> Vec<ListRow> {
    let indices: HashMap<_, _> = snapshot
        .decided
        .iter()
        .flatten()
        .enumerate()
        .map(|(index, decided)| (std::ptr::from_ref(decided), index))
        .collect();
    model::decided_list(snapshot, filter)
        .into_iter()
        .map(|decided| ListRow::Done(indices[&std::ptr::from_ref(decided)]))
        .collect()
}

fn band(
    index: usize,
    title: SharedString,
    date: Option<String>,
    trailing: Option<SharedString>,
    palette: &Palette,
) -> AnyElement {
    let date = date.map(|date| {
        div()
            .text_color(palette.text_tertiary)
            .font_weight(gpui::FontWeight::NORMAL)
            .child(date)
    });
    let (title_selector, count_selector) = (
        format!("group-{index}-title"),
        format!("group-{index}-count"),
    );
    div()
        .debug_selector(move || format!("group-{index}"))
        .w_full()
        .h(sizes::BAND_HEIGHT)
        .mt(sizes::BAND_GAP_ABOVE)
        .pl(sizes::ROW_PADDING_X + sizes::UNREAD_COLUMN + theme::GAP)
        .pr(sizes::ROW_PADDING_X)
        .flex()
        .items_center()
        .gap(theme::GAP)
        .text_size(sizes::BAND_TEXT)
        .font_weight(theme::WEIGHT_MEDIUM)
        .text_color(palette.text_tertiary)
        .child(
            div()
                .debug_selector(move || title_selector)
                .min_w_0()
                .truncate()
                .child(title),
        )
        .children(trailing.map(|words| {
            div()
                .debug_selector(move || count_selector)
                .flex_none()
                .text_color(palette.text_tertiary)
                .font_weight(gpui::FontWeight::NORMAL)
                .font_features(tabular())
                .child(words)
        }))
        .children(date)
        .into_any_element()
}
