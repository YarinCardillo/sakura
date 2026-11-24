//! Autoscroll of a chat while a drag-selection is held at its top or bottom edge: the view scrolls toward the
//! edge, faster the farther past it the pointer is, and the selection goes on to the text that comes under the
//! pointer. It is used by the master's conversation and by the conversation of a lane's agent.

use crate::theme;
use gpui::{
    canvas, point, px, App, Bounds, DispatchPhase, IntoElement, Modifiers, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, PlatformInput, Point, ScrollDelta,
    ScrollEvent, Styled as _, Task, TouchPhase, Window,
};
use gpui_base::AutoScroll;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// A drag that began in the chat: where the pointer is, and the viewport it is held against. Cheap to clone; the
/// clones are one drag.
#[derive(Clone, Default)]
pub struct DragScroll(Rc<Held>);

#[derive(Default)]
struct Held {
    pointer: Cell<Option<Point<Pixels>>>,
    bounds: Cell<Bounds<Pixels>>,
    /// The autoscroll's own events are on their way through the window: they are not the person's.
    is_dispatching: Cell<bool>,
    task: RefCell<Option<Task<()>>>,
}

impl DragScroll {
    /// The element that watches the pointer over the viewport it covers, without taking a press.
    pub fn watcher(&self) -> impl IntoElement {
        let (measured, listening) = (self.clone(), self.clone());
        canvas(
            move |bounds, _, _| measured.0.bounds.set(bounds),
            move |bounds, _, window, _| listening.listen(bounds, window),
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
    }

    fn listen(&self, bounds: Bounds<Pixels>, window: &mut Window) {
        let pressed = self.clone();
        window.on_mouse_event(move |event: &MouseDownEvent, phase, _, _| {
            let is_ours = phase == DispatchPhase::Capture
                && event.button == MouseButton::Left
                && bounds.contains(&event.position);
            if is_ours {
                pressed.0.pointer.set(Some(event.position));
            }
        });
        let moved = self.clone();
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
            if phase == DispatchPhase::Capture {
                moved.moved(event, window, cx);
            }
        });
        let released = self.clone();
        window.on_mouse_event(move |_: &MouseUpEvent, phase, _, _| {
            if phase == DispatchPhase::Capture {
                released.release();
            }
        });
    }

    fn moved(&self, event: &MouseMoveEvent, window: &mut Window, cx: &mut App) {
        if self.0.is_dispatching.get() || self.0.pointer.get().is_none() {
            return;
        }
        if event.pressed_button != Some(MouseButton::Left) {
            return self.release();
        }
        self.0.pointer.set(Some(event.position));
        let is_at_edge = AutoScroll::compute_delta(event.position.y, self.0.bounds.get()).is_some();
        if is_at_edge && self.0.task.borrow().is_none() {
            let drag = self.clone();
            let task = window.spawn(cx, async move |cx| loop {
                cx.background_executor()
                    .timer(theme::DRAG_SCROLL_TICK)
                    .await;
                let went_on = cx
                    .update(|window, cx| drag.tick(window, cx))
                    .unwrap_or(false);
                if !went_on {
                    break;
                }
            });
            *self.0.task.borrow_mut() = Some(task);
        }
    }

    fn release(&self) {
        self.0.pointer.set(None);
        self.0.task.borrow_mut().take();
    }

    /// One frame of the autoscroll; says whether the drag is still held at an edge.
    fn tick(&self, window: &mut Window, cx: &mut App) -> bool {
        let bounds = self.0.bounds.get();
        let delta =
            self.0.pointer.get().and_then(|pointer| {
                AutoScroll::compute_delta(pointer.y, bounds).map(|d| (pointer, d))
            });
        let Some((pointer, delta)) = delta else {
            return false;
        };
        let inside = point(
            pointer.x.clamp(
                bounds.left() + theme::DRAG_SCROLL_INSET,
                bounds.right() - theme::DRAG_SCROLL_INSET,
            ),
            pointer.y.clamp(
                bounds.top() + theme::DRAG_SCROLL_INSET,
                bounds.bottom() - theme::DRAG_SCROLL_INSET,
            ),
        );
        self.0.is_dispatching.set(true);
        let wheel = ScrollEvent {
            position: inside,
            delta: ScrollDelta::Pixels(point(px(0.), -delta)),
            modifiers: Modifiers::none(),
            touch_phase: TouchPhase::Moved,
            ..ScrollEvent::default()
        };
        window.dispatch_event(PlatformInput::Scroll(wheel), cx);
        // The text now under the pointer is what the selection goes on to.
        let moved = MouseMoveEvent {
            position: inside,
            pressed_button: Some(MouseButton::Left),
            modifiers: Modifiers::none(),
        };
        window.dispatch_event(PlatformInput::MouseMove(moved), cx);
        self.0.is_dispatching.set(false);
        true
    }
}
