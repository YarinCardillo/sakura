//! A hold-to-confirm button, after Sakura's HoldButton: a calm press of 1.2 s fills it with a flat muted red
//! from the left; letting go early takes the fill back in 200 ms; a completed hold emits [`HoldConfirmed`]
//! and keeps the fill. No scale, no spring, no glow. A bare click never confirms. The fill is painted, not
//! laid out, so nothing moves while it runs.

use super::chrome::{steady_label, text};
use crate::theme::{self, Theme};
use gpui::{
    canvas, div, fill, size, Bounds, Context, EventEmitter, Hsla, InteractiveElement as _,
    IntoElement, MouseButton, MouseDownEvent, MouseUpEvent, ParentElement, Render, SharedString,
    StatefulInteractiveElement as _, Styled, Window,
};
use std::time::Instant;

/// Emitted once when a hold completes.
pub struct HoldConfirmed;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Press {
    Idle,
    Holding { since: Instant, token: u64 },
    Releasing { since: Instant, from: f32 },
    Done,
}

/// The button's state; its words are set by the person on every render.
pub struct HoldButton {
    id: SharedString,
    /// The word shown, and every word the button can show: it keeps the room of the widest.
    label: &'static str,
    words: [&'static str; 2],
    press: Press,
    token: u64,
}

impl EventEmitter<HoldConfirmed> for HoldButton {}

impl HoldButton {
    /// A button at rest.
    pub fn new(id: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: "",
            words: [""; 2],
            press: Press::Idle,
            token: 0,
        }
    }

    /// Sets the word shown on the next render, among `words` (at rest, and while `box` works): the button
    /// is as wide as the widest, so the word changing after a hold moves nothing.
    pub fn set_label(&mut self, label: &'static str, words: [&'static str; 2]) {
        self.label = label;
        self.words = words;
    }

    /// Whether a hold completed and the button keeps its fill.
    pub fn is_done(&self) -> bool {
        self.press == Press::Done
    }

    /// Back to rest, for another hold.
    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.press = Press::Idle;
        cx.notify();
    }

    fn start(&mut self, cx: &mut Context<Self>) {
        if self.press == Press::Done {
            return;
        }
        self.token += 1;
        let token = self.token;
        self.press = Press::Holding {
            since: Instant::now(),
            token,
        };
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(theme::HOLD).await;
            drop(this.update(cx, |button, cx| button.complete(token, cx)));
        })
        .detach();
        cx.notify();
    }

    fn complete(&mut self, token: u64, cx: &mut Context<Self>) {
        if matches!(self.press, Press::Holding { token: held, .. } if held == token) {
            self.press = Press::Done;
            cx.emit(HoldConfirmed);
            cx.notify();
        }
    }

    fn release(&mut self, cx: &mut Context<Self>) {
        if let Press::Holding { since, .. } = self.press {
            let from = progress(self.press, since.elapsed().as_secs_f32());
            self.press = Press::Releasing {
                since: Instant::now(),
                from,
            };
            cx.notify();
        }
    }
}

/// How much of the button is filled, from 0 to 1, `elapsed` seconds into the current press phase.
fn progress(press: Press, elapsed: f32) -> f32 {
    match press {
        Press::Idle => 0.0,
        Press::Done => 1.0,
        Press::Holding { .. } => (elapsed / theme::HOLD.as_secs_f32()).min(1.0),
        Press::Releasing { from, .. } => {
            (from * (1.0 - elapsed / theme::HOLD_RELEASE.as_secs_f32())).max(0.0)
        }
    }
}

fn phase_elapsed(press: Press) -> f32 {
    match press {
        Press::Holding { since, .. } | Press::Releasing { since, .. } => {
            since.elapsed().as_secs_f32()
        }
        Press::Idle | Press::Done => 0.0,
    }
}

impl HoldButton {
    /// Advances the press for this frame and returns how much is filled.
    fn advance(&mut self, window: &mut Window) -> f32 {
        let filled = progress(self.press, phase_elapsed(self.press));
        if matches!(self.press, Press::Releasing { .. }) && filled <= 0.0 {
            self.press = Press::Idle;
        }
        if matches!(self.press, Press::Holding { .. } | Press::Releasing { .. }) {
            window.request_animation_frame();
        }
        filled
    }
}

impl Render for HoldButton {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Theme::palette(cx);
        let filled = self.advance(window);
        let ink = if filled >= 1.0 {
            palette.hold_ink
        } else {
            palette.text
        };
        let label = text(theme::TEXT_SMALL, ink)
            .relative()
            .font_weight(theme::WEIGHT_MEDIUM)
            .child(steady_label(self.label, &self.words));
        let selector = self.id.to_string();
        div()
            .id(self.id.clone())
            .debug_selector(move || selector)
            .relative()
            .h(theme::CONTROL_HEIGHT)
            .px(theme::CONTROL_PADDING_X)
            .flex()
            .items_center()
            .rounded(theme::RADIUS_CONTROL)
            .overflow_hidden()
            .bg(palette.fill)
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_press))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_release))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_release))
            .on_hover(cx.listener(Self::on_hover_change))
            .child(fill_layer(filled, palette.hold_fill))
            .child(label)
    }
}

impl HoldButton {
    fn on_press(&mut self, _: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        self.start(cx);
    }

    fn on_release(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.release(cx);
    }

    fn on_hover_change(&mut self, is_hovered: &bool, _: &mut Window, cx: &mut Context<Self>) {
        if !is_hovered {
            self.release(cx);
        }
    }
}

/// The fill, painted from the left over `filled` of the width; nothing is laid out again while it runs.
fn fill_layer(filled: f32, colour: Hsla) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, (), window, _| {
            let width = bounds.size.width * filled;
            let filled_bounds = Bounds::new(bounds.origin, size(width, bounds.size.height));
            window.paint_quad(fill(filled_bounds, colour));
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hold_fills_over_the_hold_time_and_stops_full() {
        let holding = Press::Holding {
            since: Instant::now(),
            token: 1,
        };
        assert_eq!(progress(holding, 0.0), 0.0);
        assert!((progress(holding, 0.6) - 0.5).abs() < 1e-6);
        assert_eq!(progress(holding, 5.0), 1.0);
    }

    #[test]
    fn early_release_empties_within_the_release_time() {
        let releasing = Press::Releasing {
            since: Instant::now(),
            from: 0.5,
        };
        assert_eq!(progress(releasing, 0.0), 0.5);
        assert!((progress(releasing, 0.1) - 0.25).abs() < 1e-6);
        assert_eq!(progress(releasing, 1.0), 0.0);
    }

    #[test]
    fn rest_is_empty_and_done_is_full() {
        assert_eq!(progress(Press::Idle, 3.0), 0.0);
        assert_eq!(progress(Press::Done, 0.0), 1.0);
    }
}
