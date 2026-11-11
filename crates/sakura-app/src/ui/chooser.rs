//! A chooser of the kit that opens on the press. The kit's select closes an open menu on a press
//! outside it and stops that press there, so a press on another chooser only closed the first one and
//! a second press was needed. Here the press on a closed chooser is taken before any open menu hears
//! of it: the chooser pressed opens, and the one that was open closes because it lost the keyboard.
//! The kit's own press still closes a chooser that is open.

use gpui::{
    canvas, div, App, DispatchPhase, Div, FocusHandle, HitboxBehavior, IntoElement, MouseButton,
    MouseDownEvent, ParentElement as _, Styled as _,
};
use gpui_base::actions::SelectDown;
use std::rc::Rc;

/// Whether the chooser whose keyboard handle is `shown` now is open: closed, the handle is its
/// trigger's (`trigger`, read once when the chooser was made); open, it is its list's.
pub fn is_open(trigger: &FocusHandle, shown: &FocusHandle) -> bool {
    shown != trigger
}

/// `select` drawn so that one press opens it whatever else is open. `trigger` is the chooser's keyboard
/// handle while it is closed; `shown` reads the handle it has now.
pub fn opens_on_press(
    select: impl IntoElement,
    trigger: FocusHandle,
    shown: impl Fn(&App) -> FocusHandle + 'static,
) -> Div {
    let shown = Rc::new(shown);
    let press = canvas(
        |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
        move |_, hitbox, window, _| {
            window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                // A menu drawn over this chooser hides it from the pointer: the press is the menu's.
                let is_on_it = phase == DispatchPhase::Capture
                    && event.button == MouseButton::Left
                    && hitbox.is_hovered_at(event.position, window);
                if !is_on_it || is_open(&trigger, &shown(cx)) {
                    return;
                }
                cx.stop_propagation();
                let trigger = trigger.clone();
                window.defer(cx, move |window, cx| {
                    trigger.focus(window, cx);
                    window.dispatch_action(Box::new(SelectDown), cx);
                });
            });
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full();
    div().relative().child(select).child(press)
}
