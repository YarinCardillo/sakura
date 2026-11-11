//! The tokens of Sakura's brand in the window: its two colours, the grid the mark is drawn on, the sizes
//! the mark is shown at, the timing of its one motion and the About window. The drawing itself is not
//! here: it is the brand's own file (`assets/brand/cells.json`), read by `ui::brand`.

use gpui::{px, Pixels};
use std::time::Duration;

/// The brand's fuchsia. It is also the interface's one accent (`Palette::attention`): what needs the
/// person. Text and marks that stand on it are [`BLACK`].
pub const FUCHSIA: u32 = 0xf400a1;
/// The brand's black: the ground the fuchsia mark stands on, and the ink of whatever stands on fuchsia.
pub const BLACK: u32 = 0x000000;

/// The mark is drawn on a grid of this many cells a side.
pub const GRID: u32 = 32;
/// The clear space kept on every side of the mark and of the lockup, in cells. It is part of their
/// layout box, so nothing can be set closer.
pub const CLEAR_CELLS: u32 = 4;
/// The lockup (mark, gap, word) is this many cells wide and [`GRID`] cells tall.
pub const LOCKUP_CELLS_WIDE: u32 = 123;

/// The sizes the mark is shown at, in points: whole multiples of [`GRID`], so that a cell is a whole
/// number of points.
pub const MARK_SMALL: u32 = 32;
pub const MARK_MEDIUM: u32 = 64;

/// The mark's one motion: its cells switch on in sixteen steps, in the order of the Bayer matrix the
/// mark is dithered with, one step every [`REVEAL_STEP`], 480 ms in all.
pub const REVEAL_STEPS: u8 = 16;
pub const REVEAL_STEP: Duration = Duration::from_millis(30);

/// The About window: its size, the points one cell of its lockup takes, and the gaps of its column.
pub const ABOUT_WIDTH: Pixels = px(360.);
pub const ABOUT_HEIGHT: Pixels = px(240.);
pub const ABOUT_LOCKUP_CELL: u32 = 2;
pub const ABOUT_GAP: Pixels = px(8.);
pub const ABOUT_LINE_GAP: Pixels = px(4.);

/// Where Sakura's source is.
pub const REPOSITORY: &str = "https://github.com/YarinCardillo/sakura";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Palette;
    use gpui::{rgb, Hsla, Rgba};

    /// The relative luminance of a colour (WCAG 2).
    fn luminance(colour: Hsla) -> f32 {
        let Rgba { r, g, b, .. } = colour.into();
        let linear = |channel: f32| match channel <= 0.04045 {
            true => channel / 12.92,
            false => ((channel + 0.055) / 1.055).powf(2.4),
        };
        0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
    }

    /// The contrast ratio of two colours (WCAG 2).
    fn contrast(one: Hsla, other: Hsla) -> f32 {
        let (one, other) = (luminance(one), luminance(other));
        (one.max(other) + 0.05) / (one.min(other) + 0.05)
    }

    /// The least contrast of text (WCAG AA) and of a mark or a large figure.
    const TEXT: f32 = 4.5;
    const GRAPHIC: f32 = 3.0;

    #[test]
    fn the_accent_is_the_brand_s_fuchsia_in_both_appearances() {
        let fuchsia = Hsla::from(rgb(FUCHSIA));
        assert_eq!(Palette::dark().attention, fuchsia);
        assert_eq!(Palette::light().attention, fuchsia);
    }

    #[test]
    fn black_on_fuchsia_and_fuchsia_on_the_dark_panel_read_as_text() {
        let (fuchsia, black) = (Hsla::from(rgb(FUCHSIA)), Hsla::from(rgb(BLACK)));
        assert!(contrast(black, fuchsia) >= TEXT);
        let dark = Palette::dark();
        for ground in [dark.panel, dark.canvas, dark.raised, dark.hover] {
            assert!(contrast(fuchsia, ground) >= TEXT, "{ground:?}");
        }
    }

    #[test]
    fn fuchsia_on_the_light_surfaces_reads_as_a_mark_and_not_as_small_text() {
        let fuchsia = Hsla::from(rgb(FUCHSIA));
        let light = Palette::light();
        for ground in [light.panel, light.canvas, light.raised, light.hover] {
            let ratio = contrast(fuchsia, ground);
            assert!(ratio >= GRAPHIC, "{ground:?}: {ratio}");
            assert!(ratio < TEXT, "{ground:?}: {ratio}");
        }
    }

    #[test]
    fn the_mark_s_sizes_are_whole_multiples_of_the_grid() {
        for points in [MARK_SMALL, MARK_MEDIUM] {
            assert_eq!(points % GRID, 0, "{points}");
        }
        assert_eq!(
            REVEAL_STEP * u32::from(REVEAL_STEPS),
            Duration::from_millis(480)
        );
    }
}
