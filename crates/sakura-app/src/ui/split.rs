//! The Agente view's horizontal split, on the kit's resizable: the agent left, its changes right at the
//! remembered share (38 percent at first launch, between 25 and 60). The divider is a hairline with the
//! col-resize cursor; nothing on it grows or animates. The share is saved when a drag ends. While the
//! changes pane hides or comes back, it keeps its width and is uncovered from the right edge.

#[cfg(feature = "boxes")]
use crate::prefs::{self, Preferences};
use crate::theme::{self, Theme};
#[cfg(not(feature = "boxes"))]
use gpui::prelude::FluentBuilder as _;
use gpui::{div, AnyElement, App, Entity, IntoElement, ParentElement as _, Pixels, Styled, Window};
use gpui_base::{ResizableState, ResizeHandleContext, ResizeHandleRenderer};
use gpui_component::{h_resizable, resizable_panel};
use std::rc::Rc;

/// How the split is laid out now.
#[cfg(feature = "boxes")]
pub struct Split<'a> {
    pub state: &'a Entity<ResizableState>,
    /// The changes pane's share of the width at first layout.
    pub share: f32,
    /// How much of the changes pane shows, from 0 to 1: under 1 while it hides or comes back.
    pub shown: f32,
    /// The width of the task page.
    pub width: Pixels,
}

/// The split with `agent` left and `changes` right.
#[cfg(feature = "boxes")]
pub fn view(agent: AnyElement, changes: AnyElement, split: Split, cx: &App) -> AnyElement {
    if split.shown < 1.0 {
        return sliding(agent, changes, &split, cx);
    }
    let (width, share) = (split.width, split.share);
    let (min, max) = (theme::CHANGES_SHARE_MIN, theme::CHANGES_SHARE_MAX);
    h_resizable("agente-split")
        .with_state(split.state)
        .with_handle_appearance(hairline())
        .child(
            resizable_panel()
                .size(width * (1. - share))
                .size_range(width * (1. - max)..width * (1. - min))
                .child(agent),
        )
        .child(
            resizable_panel()
                .size(width * share)
                .size_range(width * min..width * max)
                .child(changes),
        )
        .on_resize(|state, _, cx| save_share(state, cx))
        .into_any_element()
}

/// The tab pair laid out at its saved ratio before the resizable has measured either pane.
#[cfg(not(feature = "boxes"))]
pub fn page_tabs_view(
    left: AnyElement,
    right: AnyElement,
    state: &Entity<ResizableState>,
    width: Pixels,
    left_share_bps: u16,
    on_resize: impl Fn(&Entity<ResizableState>, &mut Window, &mut App) + 'static,
) -> AnyElement {
    use sakura_core::tabs::{MAX_SPLIT_SHARE, MIN_SPLIT_SHARE};
    let left_width = width * share_fraction(left_share_bps);
    let panel = |size, body| {
        resizable_panel()
            .size(size)
            .size_range(
                width * share_fraction(MIN_SPLIT_SHARE)..width * share_fraction(MAX_SPLIT_SHARE),
            )
            .child(body)
    };
    h_resizable("page-tab-split")
        .with_state(state)
        .with_handle_appearance(hairline())
        .child(panel(left_width, left))
        .child(panel(width - left_width, right))
        .on_resize(on_resize)
        .into_any_element()
}

#[cfg(not(feature = "boxes"))]
pub fn share_fraction(share_bps: u16) -> f32 {
    f32::from(share_bps) / 10_000.
}

#[cfg(not(feature = "boxes"))]
pub fn measured_share(state: &ResizableState) -> Option<u16> {
    let [left, right] = state.sizes().as_slice() else {
        return None;
    };
    let total = f32::from(*left + *right);
    (total > 0.).then(|| (f32::from(*left) / total * 10_000.).round() as u16)
}

/// The split while the changes pane moves: the pane at its full width, as much of it in view as `shown`
/// says, the agent taking the rest.
#[cfg(feature = "boxes")]
fn sliding(agent: AnyElement, changes: AnyElement, split: &Split, cx: &App) -> AnyElement {
    let remembered = split.state.read(cx).sizes().get(1).copied();
    let full = remembered
        .filter(|width| *width > Pixels::ZERO)
        .unwrap_or(split.width * split.share);
    div()
        .size_full()
        .flex()
        .child(div().flex_1().min_w_0().h_full().child(agent))
        .child(
            div()
                .w(full * split.shown)
                .h_full()
                .flex_none()
                .overflow_hidden()
                .border_l(theme::PANEL_RING_WIDTH)
                .border_color(Theme::palette(cx).border)
                .child(div().w(full).h_full().child(changes)),
        )
        .into_any_element()
}

/// The width of the task page in a window `viewport` wide: the window less what shows of the sidebar
/// (`sidebar_share`, from 0 to 1), the panel's left inset (there only as the sidebar goes) and its ring.
pub fn body_width(viewport: Pixels, sidebar_share: f32) -> Pixels {
    viewport
        - theme::SIDEBAR_WIDTH * sidebar_share
        - theme::PANEL_INSET * (1. - sidebar_share)
        - theme::PANEL_RING_WIDTH * 2.
}

/// The changes pane's share after a drag, kept for the next launch.
#[cfg(feature = "boxes")]
fn save_share(state: &Entity<ResizableState>, cx: &mut App) {
    let sizes = state.read(cx).sizes();
    let (Some(agent), Some(changes)) = (sizes.first(), sizes.get(1)) else {
        return;
    };
    let total = *agent + *changes;
    if total <= Pixels::ZERO {
        return;
    }
    let share = prefs::clamp_share(f32::from(*changes) / f32::from(total));
    Preferences::change(cx, |preferences| preferences.changes_share = share);
}

/// Where two fixed page panes meet, the divider stretches with their parent.
#[cfg(not(feature = "boxes"))]
pub fn columns(
    leading: AnyElement,
    trailing: AnyElement,
    is_stacked: bool,
    palette: &theme::Palette,
) -> gpui::Div {
    div()
        .size_full()
        .min_h_0()
        .min_w_0()
        .flex()
        .items_stretch()
        .when(is_stacked, |body| body.flex_col())
        .child(leading)
        .child(line(palette).when_else(
            is_stacked,
            |line| line.h(theme::PANEL_RING_WIDTH),
            |line| line.w(theme::PANEL_RING_WIDTH),
        ))
        .child(trailing)
}

fn line(palette: &theme::Palette) -> gpui::Div {
    div().flex_none().bg(palette.border)
}

#[cfg(not(feature = "boxes"))]
pub fn footer_line(palette: &theme::Palette) -> gpui::Div {
    line(palette)
        .ml(theme::PANEL_RING_WIDTH)
        .h(theme::PANEL_RING_WIDTH)
}

/// The divider: one hairline in the border colour, whatever the pointer does.
fn hairline() -> ResizeHandleRenderer {
    Rc::new(|_: &ResizeHandleContext, _: &mut Window, cx: &mut App| {
        let palette = Theme::palette(cx);
        Some(
            line(&palette)
                .w(theme::PANEL_RING_WIDTH)
                .h_full()
                .into_any_element(),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::px;

    #[test]
    fn the_page_is_wider_by_the_sidebar_less_one_inset_when_the_sidebar_is_hidden() {
        let shown = body_width(px(1440.), 1.0);
        let hidden = body_width(px(1440.), 0.0);
        assert_eq!(shown, px(1440. - 236. - 2.), "to the window's right edge");
        assert_eq!(hidden - shown, theme::SIDEBAR_WIDTH - theme::PANEL_INSET);
    }
}
