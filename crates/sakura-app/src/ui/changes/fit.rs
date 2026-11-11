//! An element whose content is built once its own width is known, in the same frame: the review list wraps
//! its lines to the pane's width, so its row heights can only be told from the width it is given.

use gpui::{
    relative, size, AnyElement, App, AvailableSpace, Bounds, Element, ElementId, GlobalElementId,
    InspectorElementId, IntoElement, LayoutId, Pixels, Style, Window,
};

type Build = Box<dyn FnOnce(Pixels, &mut Window, &mut App) -> AnyElement>;

/// Fills its parent and shows what `build` makes for its width.
pub fn fitted(build: impl FnOnce(Pixels, &mut Window, &mut App) -> AnyElement + 'static) -> Fitted {
    Fitted {
        build: Some(Box::new(build)),
    }
}

/// See [`fitted`].
pub struct Fitted {
    build: Option<Build>,
}

impl IntoElement for Fitted {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for Fitted {
    type RequestLayoutState = ();
    type PrepaintState = Option<AnyElement>;

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
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        let build = self.build.take()?;
        let mut content = build(bounds.size.width, window, cx);
        let space = size(
            AvailableSpace::Definite(bounds.size.width),
            AvailableSpace::Definite(bounds.size.height),
        );
        content.prepaint_as_root(bounds.origin, space, window, cx);
        Some(content)
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        content: &mut Option<AnyElement>,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(content) = content {
            content.paint(window, cx);
        }
    }
}
