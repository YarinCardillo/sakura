//! A toast sits in the bottom corner of the window, with a sentence and, when the act
//! can be taken back, one text button, Undo. It has no close button: it goes by itself (4 seconds, 6 with
//! Undo, 8 for an error), at Escape or by a swipe across it, and the pointer on it holds the time. One
//! toast shows at a time: a new one replaces the old. It rises in and sinks out over the layout duration
//! with the ease out of the other movements, with no bounce.

use super::toast_phase::Phase;
use crate::l10n::{tr, Key};
use crate::theme::kit::{
    TOAST_BOTTOM, TOAST_BUTTON_PADDING_X, TOAST_BUTTON_PADDING_Y, TOAST_ERROR, TOAST_GAP,
    TOAST_PADDING_END, TOAST_PADDING_START, TOAST_PADDING_Y, TOAST_RADIUS, TOAST_RING_WIDTH,
    TOAST_RISE, TOAST_SHADOW_BLUR, TOAST_SHADOW_DARKNESS, TOAST_SHADOW_OFFSET_Y,
    TOAST_SWIPE_DISTANCE, TOAST_TICK, TOAST_WITHOUT_UNDO, TOAST_WITH_UNDO,
};
use crate::theme::{self, Palette, Theme};
use gpui::{
    div, hsla, point, px, App, AppContext as _, BoxShadow, Context, Div, DragMoveEvent, Entity,
    FontWeight, Global, InteractiveElement as _, IntoElement, MouseButton, ParentElement as _,
    Pixels, Point, Render, SharedString, Stateful, StatefulInteractiveElement as _, Styled as _,
    Subscription, TouchPhase, Window, WindowId,
};
use std::collections::HashMap;
use std::time::{Duration, Instant};

const ESCAPE: &str = "escape";

/// What Undo does: called once, or never when the toast goes first.
pub type Undo = Box<dyn FnOnce(&mut Window, &mut App)>;

pub struct Toast;

impl Toast {
    /// Shows `text` in `window`. With `undo`, the toast has an Undo button and stays longer; pressing it
    /// closes the toast and then calls `undo` once. It replaces the toast on screen, if there is one.
    pub fn show(
        window: &mut Window,
        cx: &mut App,
        text: impl Into<SharedString>,
        undo: Option<Undo>,
    ) -> u64 {
        Self::show_with_detail(window, cx, text, None, undo)
    }

    /// The same, with a second line under the sentence, in quieter ink, when there is one.
    pub fn show_with_detail(
        window: &mut Window,
        cx: &mut App,
        text: impl Into<SharedString>,
        detail: Option<SharedString>,
        undo: Option<Undo>,
    ) -> u64 {
        let stay = if undo.is_some() {
            TOAST_WITH_UNDO
        } else {
            TOAST_WITHOUT_UNDO
        };
        let mut shown = Shown::new(text.into(), undo, stay);
        shown.detail = detail;
        present(window, cx, shown)
    }

    /// Takes the Undo button off toast `id` while it is the one on screen: what it could take back has been
    /// sent. The words stay until the toast goes by itself. Another toast's id changes nothing.
    pub fn drop_undo(window: &Window, cx: &mut App, id: u64) {
        let view = view_of(window, cx);
        view.update(cx, |view, cx| {
            if let Some(shown) = view.current.as_mut().filter(|shown| shown.id == id) {
                shown.undo = None;
                cx.notify();
            }
        });
    }

    #[cfg(any(test, not(feature = "boxes")))]
    /// Whether the pointer is on the toast on screen.
    pub fn is_hovered(window: &Window, cx: &mut App) -> bool {
        let view = view_of(window, cx);
        let view = view.read(cx);
        view.current.as_ref().is_some_and(|shown| shown.is_hovered)
    }

    pub fn add_undo_detail(window: &Window, cx: &mut App, detail: SharedString) -> bool {
        view_of(window, cx).update(cx, |view, cx| {
            let Some(shown) = view.current.as_mut().filter(|shown| shown.undo.is_some()) else {
                return false;
            };
            shown.detail = Some(detail);
            cx.notify();
            true
        })
    }

    /// Shows `text` as something that went wrong: no button, and it stays longer so it can be read.
    pub fn show_error(window: &mut Window, cx: &mut App, text: impl Into<SharedString>) {
        present(window, cx, Shown::new(text.into(), None, TOAST_ERROR));
    }

    #[cfg(test)]
    pub fn text_for_test(window: &Window, cx: &mut App) -> Option<SharedString> {
        view_of(window, cx)
            .read(cx)
            .current
            .as_ref()
            .map(|shown| shown.text.clone())
    }

    /// Draws the window's toast at the bottom centre of its positioned panel.
    #[cfg(test)]
    pub fn layer(window: &Window, cx: &mut App, controls_inset: Pixels) -> impl IntoElement {
        div()
            .absolute()
            .left_0()
            .right_0()
            .bottom(controls_inset)
            .child(gpui::deferred(view_of(window, cx)).with_priority(180))
    }

    #[cfg(not(feature = "boxes"))]
    pub fn gutter(window: &Window, cx: &mut App) -> impl IntoElement {
        div().flex_none().child(view_of(window, cx))
    }
}

/// The toast on screen and what it shares with its button, its timer and its leaving.
struct Shown {
    id: u64,
    text: SharedString,
    /// A second line under the sentence.
    detail: Option<SharedString>,
    undo: Option<Undo>,
    left: Duration,
    is_hovered: bool,
    phase: Phase,
    /// The pointer only holds the toast after moving onto it.
    pointer_at_show: Point<Pixels>,
}

impl Shown {
    fn new(text: SharedString, undo: Option<Undo>, stay: Duration) -> Self {
        Self {
            id: 0,
            text,
            detail: None,
            undo,
            left: stay,
            is_hovered: false,
            phase: Phase::Shown,
            pointer_at_show: Point::default(),
        }
    }
}

/// A drag across a toast. It carries nothing and draws nothing: the toast is what moves under the
/// pointer's travel, and it leaves when the travel is far enough.
struct Swipe;

impl Render for Swipe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// The toast of one window.
#[derive(Default)]
struct ToastView {
    current: Option<Shown>,
    next_id: u64,
    escape: Option<Subscription>,
    swipe_from: Option<Pixels>,
    wheel_travel: Pixels,
}

/// The toast views of the app's windows, one each.
#[derive(Default)]
struct Views(HashMap<WindowId, Entity<ToastView>>);

impl Global for Views {}

fn view_of(window: &Window, cx: &mut App) -> Entity<ToastView> {
    let id = window.window_handle().window_id();
    if let Some(view) = cx.try_global::<Views>().and_then(|views| views.0.get(&id)) {
        return view.clone();
    }
    let view = cx.new(|_| ToastView::default());
    cx.default_global::<Views>().0.insert(id, view.clone());
    view
}

fn present(window: &mut Window, cx: &mut App, mut shown: Shown) -> u64 {
    shown.pointer_at_show = window.mouse_position();
    let view = view_of(window, cx);
    view.update(cx, |view, cx| view.replace_with(shown, cx))
}

impl ToastView {
    /// Puts `shown` on screen in place of the toast there, and says which toast it is.
    fn replace_with(&mut self, mut shown: Shown, cx: &mut Context<Self>) -> u64 {
        self.next_id += 1;
        shown.id = self.next_id;
        shown.is_hovered = self.current.as_ref().is_some_and(|old| old.is_hovered);
        if !cx.reduce_motion() {
            shown.phase = Phase::rising(Instant::now());
        }
        self.current = Some(shown);
        self.swipe_from = None;
        self.wheel_travel = Pixels::ZERO;
        self.listen_for_escape(cx);
        self.keep_time(self.next_id, cx);
        cx.notify();
        self.next_id
    }

    /// Starts the toast's leaving, or ends it at once with reduced motion.
    fn begin_leave(&mut self, cx: &mut Context<Self>) {
        let Some(shown) = self.current.as_mut() else {
            return;
        };
        if shown.phase.is_leaving() {
            return;
        }
        if cx.reduce_motion() {
            self.clear();
        } else {
            shown.phase = shown.phase.leaving(Instant::now());
        }
        cx.notify();
    }

    fn clear(&mut self) {
        self.current = None;
        self.escape = None;
        self.swipe_from = None;
    }

    /// Presses Undo: the toast leaves, then the undo is called, outside this view's update so it may
    /// show a toast of its own.
    fn press_undo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(undo) = self.current.as_mut().and_then(|shown| shown.undo.take()) else {
            return;
        };
        self.begin_leave(cx);
        window.defer(cx, move |window, cx| undo(window, cx));
    }

    fn listen_for_escape(&mut self, cx: &mut Context<Self>) {
        let subscription = cx.observe_keystrokes(|this, event, _, cx| {
            if event.keystroke.key == ESCAPE {
                this.begin_leave(cx);
            }
        });
        self.escape = Some(subscription);
    }

    /// Counts toast `id`'s time down while the pointer is away, then sends it off.
    fn keep_time(&self, id: u64, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(TOAST_TICK).await;
            if !matches!(this.update(cx, |view, cx| view.tick(id, cx)), Ok(true)) {
                break;
            }
        })
        .detach();
    }

    /// One look at the clock: whether toast `id` is still there to be looked at again.
    fn tick(&mut self, id: u64, cx: &mut Context<Self>) -> bool {
        let Some(shown) = self.current.as_mut().filter(|shown| shown.id == id) else {
            return false;
        };
        if shown.phase.is_over(Instant::now()) {
            self.clear();
            cx.notify();
            return false;
        }
        if !shown.is_hovered {
            shown.left = shown.left.saturating_sub(TOAST_TICK);
        }
        if shown.left.is_zero() {
            self.begin_leave(cx);
        }
        self.current.is_some()
    }
}

// The swipe: a drag across the toast, or a sideways scroll over it.
impl ToastView {
    fn drag_to(&mut self, x: Pixels, cx: &mut Context<Self>) {
        let Some(from) = self.swipe_from else {
            return;
        };
        if (x - from).abs() >= TOAST_SWIPE_DISTANCE {
            self.swipe_from = None;
            self.begin_leave(cx);
        }
    }

    fn scroll_by(&mut self, across: Pixels, phase: TouchPhase, cx: &mut Context<Self>) {
        self.wheel_travel += across.abs();
        if self.wheel_travel >= TOAST_SWIPE_DISTANCE {
            self.wheel_travel = Pixels::ZERO;
            self.begin_leave(cx);
        } else if phase == TouchPhase::Ended {
            self.wheel_travel = Pixels::ZERO;
        }
    }
}

impl Render for ToastView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let base = div().relative();
        let Some(shown) = self.current.as_ref() else {
            return base;
        };
        let now = Instant::now();
        let visibility = shown.phase.visibility(now);
        if shown.phase.is_moving(now) {
            window.request_animation_frame();
        }
        let palette = Theme::palette(cx);
        let sheet = self.sheet(visibility, &palette, cx);
        base.pt(crate::theme::GAP_SMALL).pb(TOAST_BOTTOM).child(
            div()
                .relative()
                .top(TOAST_RISE * (1. - visibility))
                .px(TOAST_BOTTOM)
                .flex()
                .justify_center()
                .child(sheet),
        )
    }
}

fn pointer_moved_onto_toast(is_hovered: bool, at_show: Point<Pixels>, now: Point<Pixels>) -> bool {
    is_hovered && now != at_show
}

/// The toast's own look, with nothing live in it: the sheet, its words, the second line and, when asked, the
/// Undo label. The window's toast and the catalogue's specimen both draw this, so they cannot differ.
pub fn toast_surface(
    id: &'static str,
    text: SharedString,
    detail: Option<SharedString>,
    has_undo: bool,
    palette: &Palette,
) -> Stateful<Div> {
    let text = div()
        .id("toast-text")
        .debug_selector(|| "toast-text".to_string())
        .whitespace_normal()
        .child(text);
    let detail = detail.map(|detail| {
        div()
            .id("toast-detail")
            .debug_selector(|| "toast-detail".to_string())
            .text_color(palette.text_tertiary)
            .whitespace_normal()
            .child(detail)
    });
    let words = div()
        .flex()
        .flex_col()
        .min_w_0()
        .child(text)
        .children(detail);
    div()
        .id(id)
        .debug_selector(move || id.to_string())
        .flex()
        .min_w_0()
        .items_center()
        .gap(TOAST_GAP)
        .py(TOAST_PADDING_Y)
        .pl(TOAST_PADDING_START)
        .pr(TOAST_PADDING_END)
        .rounded(TOAST_RADIUS)
        .bg(palette.overlay_surface)
        .border(TOAST_RING_WIDTH)
        .border_color(palette.overlay_edge)
        .text_color(palette.text)
        .text_size(theme::TEXT_SMALL)
        .shadow(vec![shadow()])
        .child(words)
        .children(has_undo.then(|| undo_face(palette)))
}

/// The Undo label of a toast, at rest.
fn undo_face(palette: &Palette) -> Stateful<Div> {
    let hover = palette.fill_hover;
    div()
        .id("toast-undo")
        .debug_selector(|| "toast-undo".to_string())
        .px(TOAST_BUTTON_PADDING_X)
        .py(TOAST_BUTTON_PADDING_Y)
        .rounded(theme::RADIUS_CONTROL)
        .font_weight(FontWeight::SEMIBOLD)
        .hover(move |style| style.bg(hover))
        .child(tr(Key("toast.undo")))
}

impl ToastView {
    fn sheet(&self, visibility: f32, palette: &Palette, cx: &mut Context<Self>) -> Stateful<Div> {
        let shown = self.current.as_ref().expect("a toast to draw");
        toast_surface(
            "toast",
            shown.text.clone(),
            shown.detail.clone(),
            false,
            palette,
        )
        .opacity(visibility)
        .children(shown.undo.is_some().then(|| undo_button(palette, cx)))
        .on_hover(cx.listener(|this, is_hovered, window, _| {
            if let Some(shown) = this.current.as_mut() {
                let (at_show, now) = (shown.pointer_at_show, window.mouse_position());
                shown.is_hovered = pointer_moved_onto_toast(*is_hovered, at_show, now);
            }
        }))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, event: &gpui::MouseDownEvent, _, _| {
                this.swipe_from = Some(event.position.x)
            }),
        )
        .on_drag(Swipe, |_, _, _, cx| cx.new(|_| Swipe))
        .on_drag_move(cx.listener(|this, event: &DragMoveEvent<Swipe>, _, cx| {
            this.drag_to(event.event.position.x, cx);
        }))
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(|this, _, _, _| this.swipe_from = None),
        )
        .on_scroll(cx.listener(|this, event: &gpui::ScrollEvent, window, cx| {
            let across = event.delta.pixel_delta(window.line_height()).x;
            this.scroll_by(across, event.touch_phase, cx);
        }))
    }
}

fn undo_button(palette: &Palette, cx: &mut Context<ToastView>) -> Stateful<Div> {
    undo_face(palette).on_click(cx.listener(|this, _, window, cx| this.press_undo(window, cx)))
}

fn shadow() -> BoxShadow {
    BoxShadow {
        color: hsla(0., 0., 0., TOAST_SHADOW_DARKNESS),
        offset: point(px(0.), TOAST_SHADOW_OFFSET_Y),
        blur_radius: TOAST_SHADOW_BLUR,
        spread_radius: px(0.),
        inset: false,
    }
}
