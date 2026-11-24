//! A tab drawn where its spring or the pointer puts it, not where the row laid it out. The row lays the
//! pill out as it always does; this element reads the slot it got, asks the bar how far to draw it from
//! there in the same frame, and prepaints the pill shifted by that much, so a tab never shows at its new
//! slot for a frame before it glides from the old one. It asks for another frame while a spring moves.

use super::PageBar;
use crate::ui::tab_motion::drag::TabBox;
use gpui::{
    point, px, AnyElement, App, Bounds, Element, ElementId, Entity, GlobalElementId,
    InspectorElementId, IntoElement, LayoutId, Pixels, Window,
};
use sakura_core::tabs::TabId;
use std::time::Instant;

/// The pill of the tab `id`, drawn where the bar says.
pub(super) struct Gliding {
    id: TabId,
    bar: Entity<PageBar>,
    pill: AnyElement,
}

impl Gliding {
    pub(super) fn new(id: TabId, bar: Entity<PageBar>, pill: AnyElement) -> Self {
        Self { id, bar, pill }
    }
}

impl IntoElement for Gliding {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for Gliding {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
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
        (self.pill.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        (): &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let slot = TabBox {
            left: f32::from(bounds.left()),
            width: f32::from(bounds.size.width),
        };
        let is_still = cx.reduce_motion();
        let id = self.id;
        let shift = self
            .bar
            .update(cx, |bar, _| bar.place(id, slot, Instant::now(), is_still));
        window.with_element_offset(point(px(shift), px(0.)), |window| {
            self.pill.prepaint(window, cx)
        });
        if self.bar.read(cx).is_moving() {
            window.request_animation_frame();
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        (): &mut (),
        (): &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.pill.paint(window, cx);
    }
}
