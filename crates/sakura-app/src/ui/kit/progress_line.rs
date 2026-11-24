//! The thin progress line of a lane: how many of its steps are done, in the room it is given.

use crate::theme::kit::PROGRESS_LINE_HEIGHT;
use crate::theme::{Palette, Theme};
use gpui::{div, App, IntoElement, ParentElement as _, Pixels, RenderOnce, Styled as _, Window};

/// A line `width` wide filled to `done` of `total` steps (empty when there are no steps, full when
/// `done` is past `total`).
pub fn progress_line(done: u32, total: u32, width: Pixels) -> ProgressLine {
    ProgressLine { done, total, width }
}

/// The element [`progress_line`] builds.
#[derive(IntoElement)]
pub struct ProgressLine {
    done: u32,
    total: u32,
    width: Pixels,
}

impl ProgressLine {
    /// The share of the line that is filled, from 0 to 1.
    fn share(&self) -> f32 {
        match self.total {
            0 => 0.,
            total => (self.done as f32 / total as f32).min(1.),
        }
    }
}

impl RenderOnce for ProgressLine {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let palette: Palette = Theme::palette(cx);
        let filled = self.width * self.share();
        div()
            .w(self.width)
            .h(PROGRESS_LINE_HEIGHT)
            .flex_none()
            .overflow_hidden()
            .rounded(PROGRESS_LINE_HEIGHT / 2.)
            .bg(palette.border)
            .child(div().w(filled).h_full().bg(palette.text_tertiary))
    }
}
