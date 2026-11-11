//! The strip and the page under it as one panel. The strip draws the tab and the panel's top, one radius
//! deep, as one shape; the rest of the panel is a plain surface below it with the same fill and ring, so the
//! two read as one piece. The page lies over both, from the panel's top edge.

use super::metrics::{PANEL_OVERHANG, RADIUS, RING_WIDTH, STRIP_HEIGHT};
use super::strip::Strip;
use crate::theme::{self, Palette};
use gpui::{
    div, point, px, BoxShadow, Div, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, Styled as _,
};

impl Render for Strip {
    fn render(
        &mut self,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement {
        self.watch_window(window, cx);
        // The strip moves the window like a title bar; its tabs keep their own presses.
        crate::ui::chrome::drag_region("tab-strip")
            .debug_selector(|| "tab-strip".into())
            .w_full()
            .h(px(STRIP_HEIGHT))
            .flex_none()
            .child(super::element::StripElement { strip: cx.entity() })
    }
}

/// `strip` over `page`, filling the place the panel had: the strip's 36 px on the canvas, then the panel.
/// The caller keeps the panel's left inset beside this, puts its right edge on the window's right edge
/// (the panel has no inset, ring or corner there: its box overhangs the window) and draws no fill, ring
/// or radius of its own.
pub fn framed(strip: &Entity<Strip>, page: impl IntoElement, palette: &Palette) -> Div {
    let surface = div()
        .absolute()
        .top(px(RADIUS))
        .left_0()
        .right(px(-PANEL_OVERHANG))
        .bottom_0()
        .bg(palette.panel)
        .rounded_b(px(RADIUS))
        .border_color(palette.panel_ring)
        .border_l(px(RING_WIDTH))
        .border_r(px(RING_WIDTH))
        .border_b(px(RING_WIDTH));
    let body = div()
        .flex_1()
        .min_h_0()
        .relative()
        .child(surface)
        .child(
            // The page lies inside the panel's ring, as it did inside the panel's own border. At the right,
            // where the ring is outside the window, it keeps the ring's width from the window's edge, so
            // what ends on the content inset is as far from that edge as from the ring at the left.
            div()
                .absolute()
                .inset(px(RING_WIDTH))
                .overflow_hidden()
                .child(page),
        )
        // Whatever page is shown, its own fill is a rectangle: the panel's rounded bottom-left corner is
        // drawn again over it, so no view can square it off.
        .child(corner_over_page(palette));
    div()
        .size_full()
        .relative()
        .flex()
        .flex_col()
        .children(soft_edge(palette))
        .child(strip.clone())
        .child(body)
}

/// The panel's bottom-left corner, over the page: a square of the corner's size showing the canvas
/// outside the arc and the ring on it. Each piece is a circle far larger than the square, cut by it.
fn corner_over_page(palette: &Palette) -> Div {
    let hole = |border: f32, colour: gpui::Hsla| {
        // A ring whose inner edge is the corner's arc: centre on the square's top-right corner.
        let outer = RADIUS + border;
        div()
            .absolute()
            .left(px(RADIUS - outer))
            .top(px(-outer))
            .size(px(outer * 2.))
            .rounded(px(outer))
            .border(px(border))
            .border_color(colour)
    };
    div()
        .debug_selector(|| "panel-corner".into())
        .absolute()
        .left_0()
        .bottom_0()
        .size(px(RADIUS))
        .overflow_hidden()
        .child(hole(RADIUS, palette.canvas))
        .child(
            hole(RING_WIDTH, ring_on_panel(palette))
                .left(px(0.))
                .top(px(-RADIUS))
                .size(px(RADIUS * 2.))
                .rounded(px(RADIUS)),
        )
}

/// The ring's colour as it looks on the panel, opaque: the ring itself is translucent, and this arc
/// lies over the ends of the ring already drawn under the page, where two translucent strokes would
/// add up to a brighter one.
fn ring_on_panel(palette: &Palette) -> gpui::Hsla {
    let panel: gpui::Rgba = palette.panel.into();
    panel.blend(palette.panel_ring.into()).into()
}

/// The light panel's soft edge, behind the panel's whole box; dark has none.
fn soft_edge(palette: &Palette) -> Option<Div> {
    let colour = palette.panel_edge?;
    let shadow = BoxShadow {
        color: colour,
        offset: point(px(0.), theme::PANEL_EDGE_OFFSET_Y),
        blur_radius: theme::PANEL_EDGE_BLUR,
        spread_radius: px(0.),
        inset: false,
    };
    let edge = div()
        .absolute()
        .top(px(STRIP_HEIGHT))
        .left_0()
        .right(px(-PANEL_OVERHANG))
        .bottom_0()
        .rounded(px(RADIUS))
        .shadow(vec![shadow]);
    Some(edge)
}
