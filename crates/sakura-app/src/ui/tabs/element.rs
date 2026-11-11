//! The strip as one element. It lays the tabs out for the width it is given, in the same frame, and paints
//! in the order the look needs: the inactive tabs first, the fused shape over them (an active tab dragged
//! across another covers it), the active tab's content on the shape, and last an inactive tab in the hand,
//! which stays above the active one.

use super::frame::{Frame, TabFace};
use super::metrics::{EDGE_FADE, FADE_TO_RIGHT, STRIP_HEIGHT, TAB_HEIGHT, TOP_INSET};
use super::paint::{FusedColours, Prepared};
use super::strip::Strip;
use super::tab::{tab, Around};
use crate::theme::{Palette, Theme};
use gpui::{
    point, px, relative, size, AnyElement, App, AvailableSpace, Bounds, ContentMask, DispatchPhase,
    Element, ElementId, Entity, GlobalElementId, InspectorElementId, IntoElement, LayoutId,
    MouseButton, MouseMoveEvent, MouseUpEvent, Pixels, ScrollEvent, Style, Window,
};
use std::rc::Rc;

/// The strip of `strip`, filling the row it is put in.
pub(super) struct StripElement {
    pub strip: Entity<Strip>,
}

/// What a frame paints, in paint order.
pub(super) struct Painted {
    inactive: Vec<AnyElement>,
    shape: (Option<Rc<Prepared>>, FusedColours),
    active: Option<AnyElement>,
    held: Option<AnyElement>,
    tips: AnyElement,
    /// The span the inactive tabs show in.
    view: Bounds<Pixels>,
    /// Whether tabs are cut at the span's start and at its end.
    is_cut: (bool, bool),
    canvas: gpui::Hsla,
}

impl IntoElement for StripElement {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for StripElement {
    type RequestLayoutState = ();
    type PrepaintState = Painted;

    fn id(&self) -> Option<ElementId> {
        Some("tab-strip-tabs".into())
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = px(STRIP_HEIGHT).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        (): &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Painted {
        let frame = self
            .strip
            .update(cx, |strip, cx| strip.frame(bounds, window, cx));
        let palette = Theme::palette(cx);
        let view = Bounds::from_corners(
            point(bounds.left() + px(frame.view.0), bounds.top()),
            point(bounds.left() + px(frame.view.1), bounds.bottom()),
        );
        let mut painted = Painted {
            inactive: Vec::new(),
            shape: (self.strip.read(cx).prepared.clone(), colours(&palette)),
            active: None,
            held: None,
            tips: self.strip.read(cx).tips.clone().into_any_element(),
            view,
            is_cut: frame.is_cut,
            canvas: palette.canvas,
        };
        self.place_tabs(&frame, bounds, &palette, &mut painted, window, cx);
        painted
            .tips
            .layout_as_root(AvailableSpace::min_size(), window, cx);
        painted.tips.prepaint_at(bounds.origin, window, cx);
        painted
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        (): &mut (),
        painted: &mut Painted,
        window: &mut Window,
        cx: &mut App,
    ) {
        let mask = ContentMask {
            bounds: painted.view,
        };
        window.with_content_mask(Some(mask), |window| {
            for tab in &mut painted.inactive {
                tab.paint(window, cx);
            }
        });
        fade_cut_edges(painted, window);
        if let Some(shape) = &painted.shape.0 {
            shape.paint(painted.shape.1, window);
        }
        for tab in painted.active.iter_mut().chain(painted.held.iter_mut()) {
            tab.paint(window, cx);
        }
        painted.tips.paint(window, cx);
        self.follow_the_pointer(window);
        self.follow_the_wheel(painted.view, window);
    }
}

impl StripElement {
    /// Builds every tab, lays it out at its width and puts it where it is drawn.
    fn place_tabs(
        &self,
        frame: &Frame,
        bounds: Bounds<Pixels>,
        palette: &Palette,
        painted: &mut Painted,
        window: &mut Window,
        cx: &mut App,
    ) {
        // In paint order, so that a tab drawn over another also takes the pointer from it.
        let layer = |face: &&TabFace| (face.is_held && !face.is_active, face.is_active);
        let mut faces: Vec<&TabFace> = frame.faces.iter().collect();
        faces.sort_by_key(layer);
        for face in faces {
            let origin = bounds.origin + point(px(face.drawn.left), px(TOP_INSET));
            let extent = size(px(face.drawn.width), px(TAB_HEIGHT));
            let around = Around {
                strip: &self.strip,
                palette,
                bounds: Bounds::new(origin, extent),
                strip_width: f32::from(bounds.size.width),
            };
            let mut element = tab(face, &around);
            element.layout_as_root(extent.map(AvailableSpace::Definite), window, cx);
            let is_cut = !face.is_active && !face.is_held;
            let mask = is_cut.then_some(ContentMask {
                bounds: painted.view,
            });
            window.with_content_mask(mask, |window| element.prepaint_at(origin, window, cx));
            place(painted, face, element);
        }
    }

    /// While the button is down the strip follows the pointer wherever it goes in the window, and the
    /// release lands the tab. A move with no button held means the release was lost: the tab goes back.
    fn follow_the_pointer(&self, window: &mut Window) {
        let (moved, released) = (self.strip.clone(), self.strip.clone());
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
            if phase != DispatchPhase::Bubble {
                return;
            }
            let x = f32::from(event.position.x);
            let is_down = event.pressed_button == Some(MouseButton::Left);
            moved.update(cx, |strip, cx| {
                if is_down {
                    strip.move_pointer(x, cx);
                } else {
                    strip.cancel_drag(cx);
                }
            });
        });
        window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
            if phase == DispatchPhase::Bubble && event.button == MouseButton::Left {
                released.update(cx, |strip, cx| strip.release(cx));
            }
        });
    }

    /// A wheel turned over the tabs scrolls the ones that do not fit: sideways as it is, and up or down
    /// too, since a strip has nothing else a plain wheel could move.
    fn follow_the_wheel(&self, view: Bounds<Pixels>, window: &mut Window) {
        let strip = self.strip.clone();
        window.on_mouse_event(move |event: &ScrollEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble || !view.contains(&event.position) {
                return;
            }
            let delta = event.delta.pixel_delta(window.line_height());
            let along = if delta.x.abs() >= delta.y.abs() {
                delta.x
            } else {
                delta.y
            };
            strip.update(cx, |strip, cx| strip.scroll_by(f32::from(along), cx));
        });
    }
}

/// Where tabs are cut by the strip's ends they fade into the canvas, so a cut title reads as more tabs
/// that way and not as a broken one. Painted over the tabs on the canvas and under the tab shown.
fn fade_cut_edges(painted: &Painted, window: &mut Window) {
    let (view, clear) = (painted.view, painted.canvas.opacity(0.));
    let width = px(EDGE_FADE).min(view.size.width / 2.);
    let band = |left: Pixels| Bounds::new(point(left, view.top()), size(width, view.size.height));
    let stop = gpui::linear_color_stop;
    if painted.is_cut.0 {
        let fade = gpui::linear_gradient(FADE_TO_RIGHT, stop(painted.canvas, 0.), stop(clear, 1.));
        window.paint_quad(gpui::fill(band(view.left()), fade));
    }
    if painted.is_cut.1 {
        let fade = gpui::linear_gradient(FADE_TO_RIGHT, stop(clear, 0.), stop(painted.canvas, 1.));
        window.paint_quad(gpui::fill(band(view.right() - width), fade));
    }
}

/// Files `element` under the layer `face` paints in.
fn place(painted: &mut Painted, face: &TabFace, element: AnyElement) {
    if face.is_active {
        painted.active = Some(element);
    } else if face.is_held {
        painted.held = Some(element);
    } else {
        painted.inactive.push(element);
    }
}

fn colours(palette: &Palette) -> FusedColours {
    FusedColours {
        fill: palette.panel,
        ring: palette.panel_ring,
    }
}
