//! The Settings window is drawn at the rem the kit's settings component is made for. The kit sizes
//! its rows, its sidebar and its text in rems; the app's windows are drawn at a smaller one (the
//! kit's root sets it to the interface's body size), at which the kit's settings text would be
//! smaller than the rest of the app's. The scope holds its own rem for everything inside it, in
//! every phase of the frame, and leaves the rest of the window as it is.

use gpui::{
    AnyElement, App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement,
    LayoutId, Pixels, Window,
};

/// `child`, laid out and painted at `rem`.
pub fn at_rem(rem: Pixels, child: impl IntoElement) -> RemScope {
    RemScope {
        rem,
        child: child.into_any_element(),
    }
}

pub struct RemScope {
    rem: Pixels,
    child: AnyElement,
}

impl IntoElement for RemScope {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for RemScope {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let child = &mut self.child;
        let layout =
            window.with_rem_size(Some(self.rem), |window| child.request_layout(window, cx));
        (layout, ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let child = &mut self.child;
        window.with_rem_size(Some(self.rem), |window| drop(child.prepaint(window, cx)));
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let child = &mut self.child;
        window.with_rem_size(Some(self.rem), |window| child.paint(window, cx));
    }
}
