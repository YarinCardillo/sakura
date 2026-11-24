//! The tooltip that is up, drawn over the window: a card on the window's raised surface, with the theme's
//! thin border, holding the label and, after it, the key.

use super::{Side, Tips};
use crate::theme::{self, Theme};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    anchored, canvas, deferred, div, Anchor, Context, DispatchPhase, Entity,
    InteractiveElement as _, IntoElement, MouseDownEvent, MouseUpEvent, ParentElement as _, Render,
    Styled as _, Subscription, Window,
};
use gpui::{Animation, AnimationExt as _};

/// The draw order of a tooltip: over everything.
const PRIORITY: usize = 200;

/// The view of the controller's tooltip.
pub struct TipLayer {
    tips: Entity<Tips>,
    _changes: Subscription,
}

impl TipLayer {
    /// The layer over `tips`.
    pub fn new(tips: Entity<Tips>, cx: &mut Context<Self>) -> Self {
        let changes = cx.observe(&tips, |_, _, cx| cx.notify());
        Self {
            tips,
            _changes: changes,
        }
    }
}

impl TipLayer {
    /// An element that hears every press in the window before the controls do and tells the controller.
    fn press_listener(&self) -> impl IntoElement {
        let tips = self.tips.clone();
        canvas(
            |_, _, _| (),
            move |_, (), window, _| {
                let tips = tips.clone();
                let held = tips.clone();
                window.on_mouse_event(move |_: &MouseDownEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture {
                        held.update(cx, |tips, cx| tips.press_anywhere(cx));
                    }
                });
                let tips = tips.clone();
                window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture {
                        tips.update(cx, |tips, _| tips.release());
                    }
                });
            },
        )
    }
}

impl Render for TipLayer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let listener = self.press_listener();
        let Some(shown) = self.tips.read(cx).shown().cloned() else {
            return div().child(listener).into_any_element();
        };
        let palette = Theme::palette(cx);
        let corner = match shown.placement.side {
            Side::Above => Anchor::BottomLeft,
            Side::Below => Anchor::TopLeft,
            Side::Sidebar | Side::Path => Anchor::BottomRight,
        };
        let label = if shown.placement.side == Side::Path {
            shown.words.label.replace('/', "/\u{200b}").into()
        } else {
            shown.words.label.clone()
        };
        let wraps = matches!(shown.placement.side, Side::Sidebar | Side::Path);
        let label = div()
            .when(wraps, |label| label.min_w_0().flex_1().whitespace_normal())
            .child(label);
        let card = div()
            .debug_selector(|| "tip-card".into())
            .px(theme::TIP_PADDING_X)
            .py(theme::TIP_PADDING_Y)
            .flex()
            .items_center()
            .gap(theme::GAP_WIDE)
            .rounded(theme::RADIUS_CONTROL)
            .bg(palette.overlay_surface)
            .border_1()
            .border_color(palette.overlay_edge)
            .text_color(palette.text)
            .text_size(theme::TEXT_LABEL)
            .when(shown.placement.side == Side::Sidebar, |card| {
                card.max_w(theme::SIDEBAR_WIDTH - theme::TIP_WINDOW_MARGIN * 2.)
            })
            .when(shown.placement.side == Side::Path, |card| {
                card.max_w(theme::TIP_PATH_WIDTH).overflow_hidden()
            })
            .when(!wraps, |card| card.whitespace_nowrap())
            .child(label)
            .children(shown.words.shortcut.clone().map(|keys| {
                div()
                    .debug_selector(|| "tip-keys".into())
                    .text_color(palette.text_secondary)
                    .child(keys)
            }));
        let card = match shown.is_animated {
            true => card
                .with_animation(
                    ("tip-fade", 0usize),
                    Animation::new(theme::TIP_FADE),
                    |card, progress| card.opacity(progress),
                )
                .into_any_element(),
            false => card.into_any_element(),
        };
        let tip = deferred(
            anchored()
                .position(shown.placement.point)
                .anchor(corner)
                .snap_to_window_with_margin(theme::TIP_WINDOW_MARGIN)
                .child(card),
        )
        .with_priority(PRIORITY);
        div().child(listener).child(tip).into_any_element()
    }
}
