//! Relative luminance and the WCAG 2 contrast ratio.

use gpui::{Hsla, Rgba};

/// The least contrast of text (WCAG AA).
#[cfg(test)]
pub const TEXT: f32 = 4.5;
/// The least contrast of a mark, an icon or a large figure.
#[cfg(test)]
pub const GRAPHIC: f32 = 3.0;

/// The relative luminance of a colour.
fn luminance(colour: Hsla) -> f32 {
    let Rgba { r, g, b, .. } = colour.into();
    let linear = |channel: f32| match channel <= 0.04045 {
        true => channel / 12.92,
        false => ((channel + 0.055) / 1.055).powf(2.4),
    };
    0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
}

/// The contrast ratio of two opaque colours, from 1 to 21.
pub fn contrast(one: Hsla, other: Hsla) -> f32 {
    let (one, other) = (luminance(one), luminance(other));
    (one.max(other) + 0.05) / (one.min(other) + 0.05)
}
