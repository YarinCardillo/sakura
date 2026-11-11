//! One tab: its icon (filled on the selected tab, outlined on the others), its title, and its close button
//! when it can close. An inactive tab sits on the canvas and takes a quiet fill under the pointer; the
//! active one is part of the fused shape, which is drawn behind it. The pointer stays the arrow.

use super::frame::TabFace;
use super::metrics::{
    CLOSE_ICON_SIZE, CLOSE_MARGIN, CLOSE_SIZE, HOVER_INSET_Y, HOVER_RADIUS, ICON_GAP, PADDING_END,
    PADDING_START, TAB_HEIGHT, TITLE_SIZE,
};
use super::strip::Strip;
use crate::l10n::{tr, Key};
use crate::theme::{self, Palette};
use crate::ui::glyph::tab_mark;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    div, px, AnyElement, Bounds, Entity, InteractiveElement as _, IntoElement, MouseButton,
    ParentElement as _, Pixels, SharedString, StatefulInteractiveElement as _, Styled as _,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::menu::{ContextMenuExt as _, PopupMenuItem};
use gpui_component::{IconName, Sizable as _};
use sakura_core::model::{self, StateId};
use sakura_core::tabs::TabId;

/// What closes a tab, in its menu.
pub const CLOSE_TAB: Key = Key("tabs.close");
/// What keeps one tab only, in its menu.
pub const CLOSE_OTHERS: Key = Key("tabs.close_others");

/// What a tab needs besides its own face.
pub(super) struct Around<'a> {
    pub strip: &'a Entity<Strip>,
    pub palette: &'a Palette,
    /// Where the tab is drawn in the window: where its tooltip points.
    pub bounds: Bounds<Pixels>,
    /// The strip's width, for the span a drag may cover.
    pub strip_width: f32,
}

/// The tab `face`.
pub(super) fn tab(face: &TabFace, around: &Around) -> AnyElement {
    let palette = around.palette;
    let is_lit = face.is_active || face.is_held;
    let group: SharedString = format!("tab-{}", face.id.as_u64()).into();
    let ink = palette.text;
    let body = div()
        .id(("tab", face.id.as_u64()))
        .debug_selector({
            let id = face.id.as_u64();
            move || format!("tab-{id}")
        })
        .role(gpui::Role::Tab)
        .aria_label(face.title.clone())
        .aria_selected(face.is_active)
        .group(group.clone())
        // A tab drawn over another keeps the pointer for itself: the one beneath shows no hover.
        .occlude()
        .relative()
        .w(px(face.drawn.width))
        .h(px(TAB_HEIGHT))
        .flex()
        .items_center()
        .pl(px(PADDING_START))
        .pr(px(if face.has_close {
            CLOSE_MARGIN
        } else {
            PADDING_END
        }))
        .font_family(theme::FONT_UI)
        .text_size(px(TITLE_SIZE))
        .font_weight(theme::WEIGHT_MEDIUM)
        .text_color(if is_lit { ink } else { palette.text_secondary })
        .when(!is_lit, |tab| tab.hover(move |style| style.text_color(ink)))
        .child(hover_fill(face, palette, group.clone()))
        .child(div().mr(px(ICON_GAP)).child(mark(face, around)))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .child(face.title.clone()),
        )
        .children(close_button(face, around, group));
    listen(body, face, around).into_any_element()
}

/// The fill an inactive tab takes under the pointer, and keeps while it is in the hand.
fn hover_fill(face: &TabFace, palette: &Palette, group: SharedString) -> impl IntoElement {
    let fill = palette.hover;
    div()
        .absolute()
        .left_0()
        .right_0()
        .top(px(HOVER_INSET_Y))
        .bottom(px(HOVER_INSET_Y))
        .rounded(px(HOVER_RADIUS))
        .when(face.is_held && !face.is_active, |this| this.bg(fill))
        .when(!face.is_held && !face.is_active, |this| {
            this.group_hover(group, move |style| style.bg(fill))
        })
}

/// The tab's mark is its lavoro's state, the same mark the sidebar and the header show: filled on the
/// selected tab, outlined on the others. A lavoro not read yet has the quiet ring of nothing claimed.
fn mark(face: &TabFace, around: &Around) -> impl IntoElement {
    let state = face
        .state
        .unwrap_or_else(|| model::state_for(StateId::NotStarted));
    let (id, place) = (
        face.id.as_u64(),
        format!("state-tab-{}-{}", face.task, state.id.id()),
    );
    let mark =
        div()
            .debug_selector(move || place)
            .child(tab_mark(state, around.palette, face.is_active));
    div()
        .when(state.id == StateId::NeedsYou, |mark| {
            mark.debug_selector(move || format!("tab-needs-you-{id}"))
        })
        .child(mark)
}

/// The close button: always on the active tab and on one in the hand, under the pointer on the others.
fn close_button(face: &TabFace, around: &Around, group: SharedString) -> Option<impl IntoElement> {
    if !face.has_close {
        return None;
    }
    let (strip, id) = (around.strip.clone(), face.id);
    let selector = format!("tab-close-{}", id.as_u64());
    let button = Button::new(("tab-close", id.as_u64()))
        .debug_selector(move || selector.clone())
        .ghost()
        .xsmall()
        .icon(gpui_component::Icon::new(IconName::Close).size(px(CLOSE_ICON_SIZE)))
        .size(px(CLOSE_SIZE))
        .on_click(move |_, _, cx| strip.update(cx, |strip, cx| strip.close(id, cx)));
    let is_shown = face.is_active || face.is_held;
    let slot = div()
        .id(("tab-close-slot", id.as_u64()))
        .ml(px(CLOSE_MARGIN))
        .flex_none()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .when(!is_shown, |slot| {
            slot.invisible().group_hover(group, |style| style.visible())
        })
        .child(button);
    Some(slot)
}

/// A press picks the tab, the middle button closes it, the secondary one opens its menu, and resting on it
/// shows its tooltip.
fn listen(body: gpui::Stateful<gpui::Div>, face: &TabFace, around: &Around) -> impl IntoElement {
    let (id, width, bounds) = (face.id, around.strip_width, around.bounds);
    let tip = (face.title.clone(), face.digit);
    let (press, middle, hover) = (
        around.strip.clone(),
        around.strip.clone(),
        around.strip.clone(),
    );
    body.on_mouse_down(MouseButton::Left, move |event, _, cx| {
        cx.stop_propagation();
        let x = f32::from(event.position.x);
        press.update(cx, |strip, cx| strip.press_tab(id, x, width, cx));
    })
    .on_mouse_down(MouseButton::Middle, |_, _, cx| cx.stop_propagation())
    .on_mouse_up(MouseButton::Middle, move |_, _, cx| {
        middle.update(cx, |strip, cx| strip.close(id, cx));
    })
    .on_hover(move |is_over, window, cx| {
        let tip = tip.clone();
        hover.update(cx, |strip, cx| {
            strip.hover_tab((id, *is_over), tip, bounds, window, cx);
        });
    })
    .context_menu(menu(id, face.can_close, around.strip.clone()))
}

/// The tab's menu: close it, or close the others. Both need another tab to remain.
fn menu(
    id: TabId,
    can_close: bool,
    strip: Entity<Strip>,
) -> impl Fn(
    gpui_component::menu::PopupMenu,
    &mut gpui::Window,
    &mut gpui::Context<gpui_component::menu::PopupMenu>,
) -> gpui_component::menu::PopupMenu {
    move |menu, _, _| {
        let (close, others) = (strip.clone(), strip.clone());
        menu.item(
            PopupMenuItem::new(tr(CLOSE_TAB))
                .disabled(!can_close)
                .on_click(move |_, _, cx| close.update(cx, |strip, cx| strip.close(id, cx))),
        )
        .item(
            PopupMenuItem::new(tr(CLOSE_OTHERS))
                .disabled(!can_close)
                .on_click(move |_, _, cx| {
                    others.update(cx, |strip, cx| strip.close_others(id, cx));
                }),
        )
    }
}
