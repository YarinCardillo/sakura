//! The window's tooltips, from one controller: a label and, for a control with a key, its shortcut. The first
//! comes after [`COLD_DELAY`] of hover; while the system is warm (a tooltip is up, or one went away less than
//! [`WARM_WINDOW`] ago) the next comes at once and without its enter animation, so moving along a row of
//! controls reads as one tooltip that follows. One at a time. A press on the control, or anywhere else in
//! the window, hides it, and it does not come back until the pointer has left the control and returned. Where it stands is worked out once
//! from the control's bounds ([`Placement::of`]): above a control in the lower half of the window, below one
//! in the upper half, its edge on the control's left edge, so a row of controls gets tooltips in a row and
//! switching between neighbours moves nothing but the words.
//!
//! A control becomes a target with [`bind`] (a control that is already an element with an id) or [`tip`]
//! (any control); the [`Tips`] entity of the window draws the one tooltip that is up ([`layer`]).

#[cfg(not(feature = "boxes"))]
mod layer;

#[cfg(test)]
mod tests;

#[cfg(not(feature = "boxes"))]
pub use layer::TipLayer;

use gpui::{
    canvas, div, App, Bounds, Context, Div, Entity, Global, InteractiveElement as _, IntoElement,
    Keystroke, ParentElement as _, Pixels, Point, SharedString, Size, Stateful,
    StatefulInteractiveElement as _, Styled as _, Task,
};
use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// How long the pointer rests on a control before its tooltip comes, with the system cold.
pub const COLD_DELAY: Duration = Duration::from_millis(400);
/// How long after a tooltip goes away the next one still comes at once.
pub const WARM_WINDOW: Duration = Duration::from_millis(800);
/// The room between a control and its tooltip.
pub const GAP: Pixels = crate::theme::kit::scale::SPACE_S;

/// What a tooltip says: a label and, when the control has a key, the key as the Mac's menus write it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Words {
    pub label: SharedString,
    pub shortcut: Option<SharedString>,
}

impl Words {
    /// A label with no key.
    pub fn plain(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            shortcut: None,
        }
    }

    /// A label and the key `keys` (as `Keystroke::parse` reads it, "cmd-1") written as the menus do.
    #[cfg(not(feature = "boxes"))]
    pub fn with_keys(label: impl Into<SharedString>, keys: &'static str) -> Self {
        Self {
            label: label.into(),
            shortcut: Keystroke::parse(keys).ok().map(|stroke| keys_of(&stroke)),
        }
    }
}

/// `label` and the key the app binds to `action`, as the menus write it; the label alone for an action with
/// no key.
pub fn words_for(label: impl Into<SharedString>, action: &dyn gpui::Action, cx: &App) -> Words {
    let keymap = cx.key_bindings();
    let keymap = keymap.borrow();
    let bound = keymap.bindings_for_action(action).next_back();
    let keys = bound.and_then(|binding| {
        binding
            .keystrokes()
            .first()
            .map(|stroke| keys_of(stroke.inner()))
    });
    Words {
        label: label.into(),
        shortcut: keys,
    }
}

/// A key as the Mac's menus write it: the modifiers in the Mac's order, then the key.
pub fn keys_of(stroke: &Keystroke) -> SharedString {
    let modifiers = [
        (stroke.modifiers.control, '⌃'),
        (stroke.modifiers.alt, '⌥'),
        (stroke.modifiers.shift, '⇧'),
        (stroke.modifiers.platform, '⌘'),
    ];
    let mut keys: String = modifiers
        .into_iter()
        .filter_map(|(is_held, sign)| is_held.then_some(sign))
        .collect();
    keys.push_str(&stroke.key.to_uppercase());
    keys.into()
}

/// Which side of its control a tooltip stands on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Above,
    Below,
    #[cfg_attr(feature = "boxes", allow(dead_code))]
    Sidebar,
    Path,
}

/// Where a tooltip stands: its side of the control and the point of the control it hangs from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    pub side: Side,
    /// The corner of the tooltip that stands at `point`: its bottom left above a control, its top left below.
    pub point: Point<Pixels>,
}

impl Placement {
    /// The place for a control at `bounds` in a window of `viewport`: above it in the lower half of the
    /// window, below it in the upper half, the tooltip's left edge on the control's left edge.
    pub fn of(bounds: Bounds<Pixels>, viewport: Size<Pixels>) -> Self {
        if bounds.center().y > viewport.height / 2. {
            let point = gpui::point(bounds.left(), bounds.top() - GAP);
            return Self {
                side: Side::Above,
                point,
            };
        }
        let point = gpui::point(bounds.left(), bounds.bottom() + GAP);
        Self {
            side: Side::Below,
            point,
        }
    }
}

/// The tooltip that is up.
#[derive(Debug, Clone, PartialEq)]
pub struct Shown {
    pub key: SharedString,
    pub words: Words,
    pub placement: Placement,
    /// Whether it comes in with a fade: only the first after a rest.
    pub is_animated: bool,
}

enum Phase {
    Idle,
    Waiting(SharedString),
    Shown(Shown),
}

/// The window's tooltip controller.
pub struct Tips {
    phase: Phase,
    /// Until when the next tooltip comes at once, after one went away.
    warm_until: Option<Instant>,
    /// The control whose tooltip a press hid: it stays quiet until the pointer has left it.
    pressed: Option<SharedString>,
    /// The timer of the tooltip waiting to come.
    waiting: Option<Task<()>>,
    /// Whether a button is down: a control that is carried under the pointer (a tab being dragged) is left and
    /// entered again by it, and none of that is a hover that earns a tooltip.
    is_held: bool,
}

/// The controller of the app's window, for the controls that have no handle to it.
#[derive(Clone)]
pub struct TipsHandle(pub Entity<Tips>);

impl Global for TipsHandle {}

impl Tips {
    /// A controller with nothing up and nothing warm.
    pub fn new() -> Self {
        Self {
            phase: Phase::Idle,
            warm_until: None,
            pressed: None,
            waiting: None,
            is_held: false,
        }
    }

    /// The tooltip that is up.
    pub fn shown(&self) -> Option<&Shown> {
        match &self.phase {
            Phase::Shown(shown) => Some(shown),
            _ => None,
        }
    }

    /// The pointer came onto the control `key`, which says `words`, at `bounds` in a window of `viewport`.
    pub fn enter(
        &mut self,
        key: SharedString,
        words: Words,
        (bounds, viewport): (Bounds<Pixels>, Size<Pixels>),
        cx: &mut Context<Self>,
    ) {
        self.enter_at(key, words, Placement::of(bounds, viewport), cx);
    }

    fn enter_at(
        &mut self,
        key: SharedString,
        words: Words,
        placement: Placement,
        cx: &mut Context<Self>,
    ) {
        if self.is_held || self.pressed.as_ref() == Some(&key) || self.is_for(&key) {
            return;
        }
        self.pressed = None;
        let now = cx.background_executor().now();
        let is_warm = self.shown().is_some() || self.warm_until.is_some_and(|until| now < until);
        if is_warm {
            self.waiting = None;
            let shown = Shown {
                key,
                words,
                placement,
                is_animated: false,
            };
            return self.set(Phase::Shown(shown), cx);
        }
        self.phase = Phase::Waiting(key.clone());
        self.waiting = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(COLD_DELAY).await;
            drop(this.update(cx, |tips, cx| tips.come(key, words, placement, cx)));
        }));
    }

    /// The pointer left the control `key`. A control that is not the one the tooltip is for or waits for
    /// is none of its business, in whatever order the two controls hear of the pointer.
    pub fn leave(&mut self, key: &SharedString, cx: &mut Context<Self>) {
        if self.pressed.as_ref() == Some(key) {
            self.pressed = None;
        }
        if self.is_for(key) {
            self.go_away(cx);
        }
    }

    /// The button went down on the control `key`: its tooltip goes and stays away until the pointer has
    /// left it and come back.
    pub fn press(&mut self, key: &SharedString, cx: &mut Context<Self>) {
        self.is_held = true;
        if self.is_for(key) {
            self.go_away(cx);
        }
        self.pressed = Some(key.clone());
    }

    /// The button came up: hovers earn tooltips again.
    #[cfg(not(feature = "boxes"))]
    pub fn release(&mut self) {
        self.is_held = false;
    }

    /// The tabs changed order or one was picked up: what is up or waiting is for a control that moved, so it
    /// goes, and the next hover says the moved control's own words.
    #[cfg(not(feature = "boxes"))]
    pub fn close(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.phase, Phase::Idle) {
            self.go_away(cx);
        }
    }

    /// A button went down somewhere in the window, on a control with a tooltip or on one without: whatever is up or
    /// waiting goes, as a press on its own control does. A control can lose its pointer without hearing it
    /// (a tab bar that redraws under it), so a press is the one sure sign the person has moved on.
    #[cfg(not(feature = "boxes"))]
    pub fn press_anywhere(&mut self, cx: &mut Context<Self>) {
        self.is_held = true;
        let key = match &self.phase {
            Phase::Idle => return,
            Phase::Waiting(key) => key.clone(),
            Phase::Shown(shown) => shown.key.clone(),
        };
        self.press(&key, cx);
    }

    fn is_for(&self, key: &SharedString) -> bool {
        match &self.phase {
            Phase::Idle => false,
            Phase::Waiting(waiting) => waiting == key,
            Phase::Shown(shown) => &shown.key == key,
        }
    }

    fn come(
        &mut self,
        key: SharedString,
        words: Words,
        placement: Placement,
        cx: &mut Context<Self>,
    ) {
        if !matches!(&self.phase, Phase::Waiting(waiting) if *waiting == key) {
            return;
        }
        let shown = Shown {
            key,
            words,
            placement,
            is_animated: !cx.reduce_motion(),
        };
        self.set(Phase::Shown(shown), cx);
    }

    fn go_away(&mut self, cx: &mut Context<Self>) {
        let now = cx.background_executor().now();
        self.warm_until = Some(now + WARM_WINDOW);
        self.waiting = None;
        self.set(Phase::Idle, cx);
    }

    fn set(&mut self, phase: Phase, cx: &mut Context<Self>) {
        self.phase = phase;
        cx.notify();
    }
}

impl Default for Tips {
    fn default() -> Self {
        Self::new()
    }
}

/// `control` as a target of a tooltip saying `words`: its own element `id` names it to the controller. A
/// window with no controller (a test of a single view) leaves the control as it is.
pub fn tip(
    control: impl IntoElement,
    id: impl Into<SharedString>,
    words: Words,
    cx: &App,
) -> gpui::AnyElement {
    let id: SharedString = id.into();
    let target = div().id(id.clone()).relative().flex_none().child(control);
    bind(target, id, words, cx).into_any_element()
}

/// `element`, which has an id, as a target of a tooltip saying `words`: hover asks for the tooltip, a press
/// takes it away. `key` names the control to the controller.
pub fn bind(
    element: Stateful<Div>,
    key: impl Into<SharedString>,
    words: Words,
    cx: &App,
) -> Stateful<Div> {
    bind_placed(element, key.into(), words, None, cx)
}

#[cfg(not(feature = "boxes"))]
pub(super) fn bind_path(
    element: Stateful<Div>,
    key: impl Into<SharedString>,
    words: Words,
    cx: &App,
) -> Stateful<Div> {
    bind_placed(element, key.into(), words, Some(Side::Path), cx)
}

fn bind_placed(
    element: Stateful<Div>,
    key: SharedString,
    words: Words,
    side: Option<Side>,
    cx: &App,
) -> Stateful<Div> {
    let Some(TipsHandle(tips)) = cx.try_global::<TipsHandle>().cloned() else {
        return element;
    };
    let bounds = Rc::new(Cell::new(Bounds::<Pixels>::default()));
    let (hovered, pressed) = (tips.clone(), tips);
    let (hover_key, press_key, measured) = (key.clone(), key, bounds.clone());
    element
        .on_hover(move |is_over, window, cx| {
            let bounds = measured.get();
            hovered.update(cx, |tips, cx| match (is_over, side) {
                (true, Some(Side::Path)) => tips.enter_at(
                    hover_key.clone(),
                    words.clone(),
                    path_placement(bounds, super::dialog_frame::top(window)),
                    cx,
                ),
                (true, _) => tips.enter(
                    hover_key.clone(),
                    words.clone(),
                    (bounds, window.viewport_size()),
                    cx,
                ),
                (false, _) => tips.leave(&hover_key, cx),
            });
        })
        .capture_any_mouse_down(move |_, _, cx| {
            pressed.update(cx, |tips, cx| tips.press(&press_key, cx));
        })
        .child(measure(bounds))
}

#[cfg(not(feature = "boxes"))]
pub(super) fn sidebar_label(
    element: Stateful<Div>,
    key: SharedString,
    label: SharedString,
    cx: &App,
) -> Stateful<Div> {
    let Some(TipsHandle(tips)) = cx.try_global::<TipsHandle>().cloned() else {
        return element;
    };
    let at = Rc::new(Cell::new(None));
    let measured = at.clone();
    let text = label.clone();
    let measure = canvas(
        move |bounds, window, _| {
            let style = window.text_style();
            let width = window
                .text_system()
                .shape_line(
                    text.clone(),
                    style.font_size.to_pixels(window.rem_size()),
                    &[style.to_run(text.len())],
                    None,
                )
                .width;
            measured.set(truncated_sidebar_placement(bounds, width));
        },
        |_, (), _, _| (),
    )
    .absolute()
    .inset_0();
    element
        .on_hover(move |is_over, _, cx| {
            tips.update(cx, |tips, cx| match (is_over, at.get()) {
                (true, Some(place)) => {
                    tips.enter_at(key.clone(), Words::plain(label.clone()), place, cx)
                }
                _ => tips.leave(&key, cx),
            });
        })
        .child(measure)
}

fn path_placement(bounds: Bounds<Pixels>, sheet_top: Pixels) -> Placement {
    Placement {
        side: Side::Path,
        point: gpui::point(bounds.right(), sheet_top - GAP),
    }
}

#[cfg(not(feature = "boxes"))]
fn truncated_sidebar_placement(bounds: Bounds<Pixels>, width: Pixels) -> Option<Placement> {
    (width > bounds.size.width).then_some(Placement {
        side: Side::Sidebar,
        point: gpui::point(crate::theme::SIDEBAR_WIDTH - GAP, bounds.top() - GAP),
    })
}

/// An element that records the bounds of the element it stands in, at every frame.
fn measure(bounds: Rc<Cell<Bounds<Pixels>>>) -> impl IntoElement {
    canvas(move |at, _, _| bounds.set(at), |_, (), _, _| ())
        .absolute()
        .top_0()
        .left_0()
        .size_full()
}
