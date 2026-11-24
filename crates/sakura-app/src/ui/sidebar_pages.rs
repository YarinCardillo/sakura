//! The rows of the sidebar that lead to a page: Inbox, Master, Lanes and Engine, each with its icon, its
//! name and, at the right, what it counts or says. What the rows share with the lane rows is here too: the
//! frame that is filled for the open page and dimmer for the others.

use super::chrome::tabular;
use super::engine_desk::Desk;
use super::lane_sidebar::{Counts, LaneSidebar};
use crate::icons;
use crate::l10n::{tr, Key};
use crate::theme::{self, sidebar as tokens, Appearance, Palette};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    div, AnyElement, App, ClickEvent, Context, Div, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, Stateful, StatefulInteractiveElement as _, Styled as _,
    WeakEntity, Window,
};
use gpui_component::{Icon, Sizable as _};
use gpui_kit_assets::IconName;
use sakura_core::tabs::Page;

/// What a page row says at its right end.
enum Trailing {
    /// Nothing.
    None,
    /// A number that needs the person, in the accent: the Inbox's.
    Hot(usize),
    /// A number that only informs.
    Count(usize),
    /// A word, in the quiet tone: the engine's pace.
    Word(SharedString),
}

/// One row of the pages: where a press leads, how the row is named in the tests and what it shows.
struct PageRow {
    page: Page,
    name: &'static str,
    icon: IconName,
    label: Key,
    trailing: Trailing,
}

impl LaneSidebar {
    /// The rows of the pages. A count that is zero is not drawn, and neither is the pace of an engine
    /// that reports none.
    pub(super) fn pages(&self, counted: &Counts, palette: &Palette) -> AnyElement {
        let count = |number: usize, make: fn(usize) -> Trailing| match number {
            0 => Trailing::None,
            number => make(number),
        };
        let pace = counted.pace.clone().map_or(Trailing::None, Trailing::Word);
        let rows = [
            (
                "inbox",
                Page::Inbox,
                icons::WAITING_PAGE,
                count(counted.asks, Trailing::Hot),
            ),
            ("master", Page::Master, icons::MASTER_PAGE, Trailing::None),
            (
                "lanes",
                Page::Lanes,
                icons::LANES_PAGE,
                count(counted.lanes, Trailing::Count),
            ),
            (
                "scheduled",
                Page::Scheduled,
                IconName::Calendar,
                Trailing::None,
            ),
            ("engine", Page::Engine, icons::ENGINE_PAGE, pace),
        ];
        let rows = rows.into_iter().map(|(name, page, icon, trailing)| {
            let label = Key(match page {
                Page::Inbox => "page.inbox",
                Page::Master => "page.master",
                Page::Lanes => "page.lanes",
                Page::Scheduled => "page.scheduled",
                _ => "page.engine",
            });
            self.page_row(
                PageRow {
                    page,
                    name,
                    icon,
                    label,
                    trailing,
                },
                palette,
            )
        });
        div()
            .flex_none()
            .flex()
            .flex_col()
            .gap(tokens::ROW_PITCH_GAP)
            .children(rows)
            .into_any_element()
    }

    fn page_row(&self, row: PageRow, palette: &Palette) -> AnyElement {
        let is_current = self.current == row.page;
        let selector = format!("sidebar-{}", row.name);
        let icon_selector = format!("sidebar-{}-icon", row.name);
        let label_selector = format!("sidebar-{}-label", row.name);
        let icon = div()
            .debug_selector(move || icon_selector)
            .size(theme::ICON_SIZE)
            .flex_none()
            .child(Icon::new(row.icon).with_size(theme::ICON_SIZE));
        let frame = frame(&selector, tokens::ROW_HEIGHT, is_current, palette)
            .px(tokens::PADDING_X)
            .gap(tokens::ROW_GAP)
            .text_size(tokens::ROW_TEXT)
            .on_click(go(self.desk.clone(), row.page.clone()))
            .on_aux_click(go_aux(self.desk.clone(), row.page))
            .child(icon)
            .child(
                div()
                    .debug_selector(move || label_selector)
                    .flex_none()
                    .child(tr(row.label)),
            )
            .child(div().flex_1())
            .children(trailing(row.trailing, row.name, palette));
        let row = marked(frame, is_current);
        let step = match selector.as_str() {
            "sidebar-engine" => super::tour::Step::Engine,
            _ => return row,
        };
        super::tour::track(step, theme::RADIUS_CONTROL, row)
    }
}

/// The frame every row of the sidebar shares: `height` high, its fill ending at the sidebar's padding, the
/// open page's row filled and in the full tone, the others dimmer and filled only under the pointer. Every
/// row has the weight of the others: the open one is its fill and its ink, never a heavier word.
pub(super) fn frame(
    selector: &str,
    height: gpui::Pixels,
    is_current: bool,
    palette: &Palette,
) -> Stateful<Div> {
    let (hover, selector) = (palette.hover, selector.to_string());
    div()
        .id(SharedString::from(selector.clone()))
        .debug_selector(move || selector)
        .h(height)
        .w_full()
        .flex_none()
        .flex()
        .items_center()
        .rounded(theme::RADIUS_CONTROL)
        .when(is_current, |row| {
            row.bg(selected_fill(palette)).text_color(palette.text)
        })
        .when(!is_current, |row| {
            row.text_color(palette.text_secondary)
                .hover(move |style| style.bg(hover))
        })
}

/// The fill of the open page's row: the ink at a few percent, so it is a step above the fill under the
/// pointer in both appearances.
fn selected_fill(palette: &Palette) -> gpui::Hsla {
    let share = match palette.appearance {
        Appearance::Dark => tokens::SELECTED_SHARE_DARK,
        Appearance::Light => tokens::SELECTED_SHARE_LIGHT,
    };
    palette.text.opacity(share)
}

/// `row`, named "sidebar-current" when it is the open page's, so that the open page's row can be found.
pub(super) fn marked(row: Stateful<Div>, is_current: bool) -> AnyElement {
    match is_current {
        true => div()
            .debug_selector(|| "sidebar-current".into())
            .w_full()
            .flex_none()
            .child(row)
            .into_any_element(),
        false => row.into_any_element(),
    }
}

/// The handler of a press on a row that leads to `page`: the tab on screen shows it, or, with Cmd, a tab
/// of its own.
pub(super) fn go(
    desk: WeakEntity<Desk>,
    page: Page,
) -> impl Fn(&ClickEvent, &mut Window, &mut App) {
    move |event, _, cx| {
        let is_new_tab = event.modifiers().platform;
        let page = page.clone();
        drop(desk.update(cx, |desk, cx| desk.navigate(page, is_new_tab, cx)));
    }
}

/// The handler of a middle press on a row that leads to `page`: a tab of its own, as Cmd-click gives.
pub(super) fn go_aux(
    desk: WeakEntity<Desk>,
    page: Page,
) -> impl Fn(&ClickEvent, &mut Window, &mut App) {
    move |event, _, cx| {
        let is_middle = matches!(event, ClickEvent::Mouse(click) if click.up.button == gpui::MouseButton::Middle);
        if is_middle {
            let page = page.clone();
            drop(desk.update(cx, |desk, cx| desk.navigate(page, true, cx)));
        }
    }
}

pub(super) fn go_from(
    desk: WeakEntity<Desk>,
    id: sakura_core::tabs::TabId,
    source: Page,
    destination: Page,
    is_auxiliary: bool,
) -> impl Fn(&ClickEvent, &mut Window, &mut App) {
    move |event, _, cx| {
        let is_middle = matches!(event, ClickEvent::Mouse(click) if click.up.button == gpui::MouseButton::Middle);
        if is_auxiliary && !is_middle {
            return;
        }
        let is_new_tab = is_middle || event.modifiers().platform;
        drop(desk.update(cx, |desk, cx| {
            desk.follow_page_link(id, &source, destination.clone(), is_new_tab, cx);
        }));
    }
}

/// What stands at a page row's right end.
fn trailing(trailing: Trailing, name: &str, palette: &Palette) -> Option<AnyElement> {
    let (text, colour, selector) = match trailing {
        Trailing::None => return None,
        Trailing::Hot(number) => (number.to_string(), palette.attention, "count"),
        Trailing::Count(number) => (number.to_string(), palette.text_tertiary, "count"),
        Trailing::Word(word) => (word.to_string(), palette.text_tertiary, "word"),
    };
    let selector = format!("sidebar-{name}-{selector}");
    let is_hot = colour == palette.attention;
    let element = div()
        .debug_selector(move || selector)
        .flex_none()
        .text_size(tokens::COUNT_TEXT)
        .text_color(colour)
        .font_features(tabular())
        .when(is_hot, |count| {
            count.font_weight(gpui::FontWeight::SEMIBOLD)
        })
        .child(text);
    Some(element.into_any_element())
}

impl LaneSidebar {
    pub(super) fn track_section(
        &self,
        tracks: &sakura_core::engine::tracks::Tracks,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        use sakura_core::engine::tracks::{TrackStatus, Tracks};
        if matches!(tracks, Tracks::Unsupported) {
            return None;
        }
        let desk = self.desk.clone();
        let heading = div()
            .id("sidebar-tracks")
            .debug_selector(|| "sidebar-tracks".into())
            .mt(tokens::HEADING_MARGIN_TOP)
            .mb(tokens::HEADING_MARGIN_BOTTOM)
            .px(tokens::PADDING_X)
            .h(tokens::HEADING_HEIGHT)
            .flex()
            .items_center()
            .justify_between()
            .text_color(palette.text_tertiary)
            .text_size(theme::TEXT_LABEL)
            .child(tr(Key("track.plural")))
            .children(tracks.action("tracknew").map(|_| {
                super::pane_toggle::press_button(
                    ("new-track".into(), "new-track-icon"),
                    (IconName::Plus, theme::STATUS_CONTROL_HEIGHT),
                    tr(Key("track.new")),
                    move |window, cx| {
                        drop(desk.update(cx, |desk, cx| desk.open_new_track(window, cx)));
                    },
                    cx,
                )
            }));
        let (closed, active): (Vec<_>, Vec<_>) = tracks
            .rows()
            .iter()
            .partition(|track| track.status == TrackStatus::Closed);
        let mut rows = div().flex().flex_col().child(heading).children(
            active
                .into_iter()
                .map(|track| self.track_row(track, palette)),
        );
        if let Tracks::Unavailable(problem) = tracks {
            rows = rows.child(
                div()
                    .px(tokens::PADDING_X)
                    .min_w_0()
                    .whitespace_normal()
                    .text_size(theme::TEXT_SMALL)
                    .text_color(palette.text_secondary)
                    .child(problem.clone()),
            );
        }
        if !closed.is_empty() {
            rows = rows.child(self.closed_tracks_control(palette, cx));
            if self.has_expanded_closed_tracks {
                rows = rows.children(
                    closed
                        .into_iter()
                        .map(|track| self.track_row(track, palette)),
                );
            }
        }
        Some(rows.into_any_element())
    }

    fn closed_tracks_control(&self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let icon = if self.has_expanded_closed_tracks {
            IconName::ChevronDown
        } else {
            IconName::ChevronRight
        };
        let row = frame("closed-tracks", tokens::ROW_HEIGHT, false, palette)
            .px(tokens::PADDING_X)
            .gap(tokens::ROW_GAP)
            .text_size(tokens::ROW_TEXT)
            .child(Icon::new(icon).with_size(tokens::CHEVRON_SIZE))
            .child(tr(Key("track.closed_list")));
        super::kit::press_row(
            row,
            palette,
            cx.listener(|sidebar, _, _, cx| {
                sidebar.has_expanded_closed_tracks = !sidebar.has_expanded_closed_tracks;
                cx.notify();
            }),
        )
        .into_any_element()
    }

    fn track_row(
        &self,
        track: &sakura_core::engine::tracks::Track,
        palette: &Palette,
    ) -> AnyElement {
        let page = Page::Track(track.id.clone());
        let is_current = self.current == page;
        let row = frame(
            &format!("sidebar-track-{}", track.id),
            tokens::ROW_HEIGHT,
            is_current,
            palette,
        )
        .px(tokens::PADDING_X)
        .gap(tokens::ROW_GAP)
        .text_size(tokens::ROW_TEXT)
        .child(Icon::new(IconName::GitBranch).with_size(tokens::CHEVRON_SIZE))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .child(super::track_page::name(track)),
        );
        let row = super::kit::press_row(row, palette, go(self.desk.clone(), page.clone()))
            .on_aux_click(go_aux(self.desk.clone(), page));
        marked(row, is_current)
    }
}
