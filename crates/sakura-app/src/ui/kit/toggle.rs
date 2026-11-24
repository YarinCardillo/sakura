//! An on or off switch: the kit's own, named in the window by `id`. It is a tab stop: Space or Return asks
//! for the other state, and a ring shows around it only while the keyboard moved the focus there; a press
//! of the pointer never gives it a ring.

use super::LabelTarget;
use crate::theme::Theme;
use gpui::{
    div, transparent_black, AnyElement, App, InteractiveElement as _, IntoElement,
    ParentElement as _, RenderOnce, SharedString, StatefulInteractiveElement as _, Styled as _,
    Window,
};
use gpui_component::switch::Switch;
use gpui_component::FocusableExt as _;
use std::rc::Rc;

/// The keys that ask a focused switch for the other state.
const FLIP_KEYS: [&str; 2] = ["space", "enter"];

type OnChange = Rc<dyn Fn(bool, &mut Window, &mut App)>;

/// A switch showing `is_on`; a press calls `on_change` with the state it asks for.
pub fn toggle(
    id: impl Into<SharedString>,
    is_on: bool,
    on_change: impl Fn(bool, &mut Window, &mut App) + 'static,
) -> AnyElement {
    toggle_with_label(id, is_on, on_change).0
}

/// A [`toggle`] and what the click on the name of its row does: the same press as the switch's.
pub fn toggle_with_label(
    id: impl Into<SharedString>,
    is_on: bool,
    on_change: impl Fn(bool, &mut Window, &mut App) + 'static,
) -> (AnyElement, LabelTarget) {
    let on_change: OnChange = Rc::new(on_change);
    let pressed = on_change.clone();
    let target = LabelTarget::Press(Rc::new(move |window, cx| pressed(!is_on, window, cx)));
    let switch = Toggle {
        id: id.into(),
        is_on,
        on_change,
    };
    (switch.into_any_element(), target)
}

#[derive(IntoElement)]
struct Toggle {
    id: SharedString,
    is_on: bool,
    on_change: OnChange,
}

impl RenderOnce for Toggle {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let palette = Theme::palette(cx);
        let ring = palette.ring;
        let (selector, is_on) = (self.id.clone(), self.is_on);
        let (on_click, on_key) = (self.on_change.clone(), self.on_change);
        let switch = Switch::new(self.id.clone())
            .checked(is_on)
            .color(match palette.appearance {
                crate::theme::Appearance::Light => palette.attention,
                crate::theme::Appearance::Dark => palette.text_tertiary,
            })
            .focus_ring(false)
            .tab_stop(false)
            .on_click(move |asked, window, cx| on_click(*asked, window, cx));
        div()
            .id(self.id)
            .debug_selector(move || selector.to_string())
            .rounded_full()
            .border_2()
            .border_color(transparent_black())
            .focusable()
            .tab_stop(true)
            .focus_visible(move |style| style.border_color(ring))
            .capture_any_mouse_down(|_, window, _| window.prevent_default())
            .on_key_down(move |event, window, cx| {
                if FLIP_KEYS.contains(&event.keystroke.key.as_str()) {
                    on_key(!is_on, window, cx);
                    cx.stop_propagation();
                }
            })
            .child(switch)
    }
}
