//! The bar's row: the group at the left (the traffic lights' room and the arrows), the tabs, the new-tab
//! button.

use super::buttons::icon_button;
use super::cursor::arrow_over;
use super::glide::Gliding;
use super::keys::{GoBack, GoForward, NewTab};
use super::PageBar;
use crate::l10n::{tr, Key};
use crate::theme::{self, page_bar as bar, Palette, Theme};
use crate::ui::chrome::drag_region;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    canvas, deferred, div, px, AnyElement, Context, DispatchPhase, Entity, InteractiveElement as _,
    IntoElement, MouseButton, MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels, Render,
    StatefulInteractiveElement as _, Styled as _, Window,
};
use gpui_kit_assets::IconName;
use sakura_core::tabs::{Tab, Tabs};

impl Render for PageBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Theme::palette(cx);
        self.show_open_tab(window);
        self.watch_window(window, cx);
        self.strip_width = window.viewport_size().width;
        let tabs = self.tabs.clone();
        let bar = cx.entity();
        let pills = self.strip_pills(&tabs, &palette, cx);
        let floor = self.row_floor();
        let row = div()
            .id("page-tabs")
            .debug_selector(|| "page-tabs".into())
            .flex()
            .items_center()
            .gap(theme::TAB_GAP)
            .min_w_0()
            .when_some(floor, |row, width| row.min_w(px(width)))
            .flex_shrink(1.)
            .overflow_x_scroll()
            .track_scroll(&self.scroll)
            .children(pills)
            .child(follow_the_pointer(bar.clone()));
        let active = tabs.active();
        let arrows = div()
            .debug_selector(|| "bar-arrows".into())
            .w(self.group_width())
            .flex_none()
            .flex()
            .items_center()
            .justify_end()
            .gap(theme::TAB_GAP)
            .pr(bar::GROUP_PADDING_END)
            .child(icon_button(
                "bar-back",
                IconName::ArrowLeft,
                tr(Key("bar.back")),
                &GoBack,
                !active.can_go_back(),
                cx,
            ))
            .child(icon_button(
                "bar-forward",
                IconName::ArrowRight,
                tr(Key("bar.forward")),
                &GoForward,
                !active.can_go_forward(),
                cx,
            ));
        let new_tab = icon_button(
            "bar-new-tab",
            IconName::Plus,
            tr(Key("bar.new_tab")),
            &NewTab,
            false,
            cx,
        );
        drag_region("page-bar")
            .relative()
            .debug_selector(|| "page-bar".into())
            .h(bar::BAR_HEIGHT)
            .w_full()
            .flex_none()
            .flex()
            .items_center()
            .pr(bar::BAR_PADDING_END)
            .child(arrows)
            .child(row)
            .child(div().ml(theme::TAB_GAP).child(new_tab))
            .child(div().flex_1())
            .child(arrow_over(bar.clone()))
    }
}

impl PageBar {
    fn strip_pills(
        &self,
        tabs: &Tabs,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut pills = Vec::with_capacity(tabs.len());
        let mut tabs_in_order = tabs.tabs().iter().enumerate();
        while let Some((index, tab)) = tabs_in_order.next() {
            let pill = self.gliding_pill(index, tab, palette, cx);
            let is_left = tabs.split().is_some_and(|pair| pair.left == tab.id());
            if !is_left {
                pills.push(pill);
                continue;
            }
            let (index, right) = tabs_in_order.next().expect("the pair is adjacent");
            let right = self.gliding_pill(index, right, palette, cx);
            pills.push(
                div()
                    .debug_selector(|| "joined-tab-pills".into())
                    .flex_none()
                    .flex()
                    .gap(bar::JOINED_PILL_GAP)
                    .rounded(bar::PILL_RADIUS)
                    .bg(palette.hover)
                    .child(pill)
                    .child(right)
                    .into_any_element(),
            );
        }
        pills
    }

    fn gliding_pill(
        &self,
        index: usize,
        tab: &Tab,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let pill = self.pill(index + 1, tab, palette, cx);
        let pill = if self.held() == Some(tab.id()) {
            deferred(pill).with_priority(1).into_any_element()
        } else {
            pill
        };
        Gliding::new(tab.id(), cx.entity(), pill).into_any_element()
    }

    /// The width of the group at the bar's left: as wide as the sidebar in view, and at least what the
    /// traffic lights and the arrows need.
    fn group_width(&self) -> Pixels {
        let arrows = theme::CONTROL_HEIGHT * 2. + theme::TAB_GAP;
        let least = theme::TRAFFIC_LIGHTS_ROOM + arrows + bar::GROUP_PADDING_END;
        (theme::SIDEBAR_WIDTH * self.sidebar_share).max(least)
    }

    /// Brings the tab shown into view once, when it becomes the one shown: the row may hold more pills than
    /// it has room for, and the person may scroll away from it afterwards.
    fn show_open_tab(&mut self, window: &mut Window) {
        let open = self.tabs.active().id();
        let peer = self.tabs.other_in_visible_split();
        if self.scrolled_to == Some((open, peer)) {
            return;
        }
        if let Some(mut place) = self.tabs.index_of(open) {
            if let Some(pair) = self.tabs.split() {
                if let Some(left) = self.tabs.index_of(pair.left) {
                    place -= usize::from(place > left);
                }
            }
            self.scroll.scroll_to_item(place);
        }
        self.scrolled_to = Some((open, peer));
        // The scroll takes effect when the row is laid out, so the bar is drawn once more.
        window.request_animation_frame();
    }
}

/// While the button is down the bar follows the pointer wherever it goes in the window, and the release
/// lands the tab. A move with no button held means the release was lost: the tab goes back.
fn follow_the_pointer(bar: Entity<PageBar>) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |_, (), window, _| {
            let (moved, released) = (bar.clone(), bar);
            window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                if phase != DispatchPhase::Capture || !moved.read(cx).is_pressing() {
                    return;
                }
                let is_down = event.pressed_button == Some(MouseButton::Left);
                moved.update(cx, |bar, cx| match is_down {
                    true => bar.move_pointer(event.position, cx),
                    false => bar.cancel_drag(cx),
                });
            });
            window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                if phase == DispatchPhase::Capture
                    && event.button == MouseButton::Left
                    && released.read(cx).is_pressing()
                {
                    released.update(cx, |bar, cx| bar.release(event.position, cx));
                }
            });
        },
    )
    .absolute()
    .size_0()
}
