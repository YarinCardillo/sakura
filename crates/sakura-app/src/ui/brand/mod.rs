//! Sakura's brand in the window: the mark, its one motion, the lockup and the About window.
//!
//! The brand's rules are kept here by construction, so a view cannot break them by accident:
//! - one drawing at every size: the cells of the brand's own file (`cells`), never redrawn;
//! - cells on whole pixels: the mark takes only sizes that are whole multiples of 32 points
//!   ([`MarkSize`]) and paints every edge on a whole device pixel;
//! - never smoothed: squares only, no radius, no shadow, no blur, no rotation;
//! - two pairs of colours and no other: the fuchsia mark on the panel, the black mark on fuchsia
//!   ([`Ink`]);
//! - the lockup is the brand's file, never rebuilt from live text (`lockup`);
//! - four cells of clear space are part of the layout box of the mark and of the lockup;
//! - the mark is bare: the element paints no ground, so it cannot sit in a tile or a badge of its
//!   own making. The app icon is the one place where it stands on a tile.
//!
//! What a view takes from here:
//! - [`mark`]: the mark, still, at a size and in a phase;
//! - [`BrandMark`]: the mark as a view that moves: [`BrandMark::revealing`] plays the reveal once,
//!   the first time it is drawn, and [`BrandMark::connecting`] is the sign of an agent connecting,
//!   which its calling component starts and stops;
//! - [`about::content`] and [`about::open`]: the About column and its window;
//! - [`install`], once at launch.

pub mod about;
mod cells;
mod lockup;
mod mark;
mod motion;
#[cfg(test)]
mod tests;

pub use cells::Phase;
pub use mark::{mark, Ink, MarkSize};
pub use motion::{BrandMark, Rest};

/// The About page of the Settings window: the same column as the About window.
pub fn about_page(palette: &crate::theme::Palette) -> gpui::Div {
    about::content(palette)
}

/// Installs what the brand adds to the app: the action that opens the About window.
pub fn install(cx: &mut gpui::App) {
    about::install(cx);
}
