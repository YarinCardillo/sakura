//! The mark as an element: the brand's cells as filled squares, one quad per run of lit cells, every
//! edge on a whole device pixel. The element is bare: it paints no ground, no tile, no radius and no
//! shadow, and it never smooths. Its layout box is the mark with the brand's clear space around it.

use super::cells::{runs, Phase, Rows};
use crate::theme_brand::{self, CLEAR_CELLS, GRID};
use gpui::{canvas, fill, point, px, rgb, Bounds, Hsla, IntoElement, Pixels, Styled, Window};

/// The side of the mark, in points: a whole multiple of the grid, so a cell is a whole number of
/// points and, on any screen, a whole number of device pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MarkSize(u32);

impl MarkSize {
    /// The sign of an agent connecting.
    pub const SMALL: MarkSize = MarkSize(theme_brand::MARK_SMALL);
    /// The first page and the empty desk.
    pub const MEDIUM: MarkSize = MarkSize(theme_brand::MARK_MEDIUM);

    /// A mark `points` a side. Only whole multiples of the grid are sizes of the mark: another one is
    /// refused in a debug build, and drawn at the multiple below it (never under one grid) otherwise,
    /// so that no build ever shows a cell on half a pixel.
    #[cfg(test)]
    pub fn points(points: u32) -> Self {
        let is_size = Self::is_on_grid(points);
        debug_assert!(is_size, "a mark's side is a multiple of {GRID}: {points}");
        Self((points / GRID).max(1) * GRID)
    }

    /// Whether `points` is a size the mark can be shown at.
    #[cfg(test)]
    pub fn is_on_grid(points: u32) -> bool {
        points >= GRID && points.is_multiple_of(GRID)
    }

    /// The points one cell takes.
    fn cell_points(self) -> u32 {
        self.0 / GRID
    }

    /// The side of the layout box: the mark and its clear space on both sides.
    pub fn box_side(self) -> Pixels {
        px((self.cell_points() * (GRID + CLEAR_CELLS * 2)) as f32)
    }
}

/// The mark's colour. The brand has two pairs and no other: the fuchsia mark, which stands on the
/// panel, and the black mark on fuchsia. The window shows the mark only on the panel, so only the first
/// is here; the second pair is the selected tab's state mark, cut in black on its fuchsia disc.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ink {
    Fuchsia,
}

impl Ink {
    fn colour(self) -> Hsla {
        match self {
            Ink::Fuchsia => rgb(theme_brand::FUCHSIA).into(),
        }
    }
}

/// The mark at `size` in `ink`, with the cells of `phase` lit. It takes the room of the mark and its
/// clear space whatever the phase, so a mark that switches on moves nothing.
pub fn mark(size: MarkSize, ink: Ink, phase: Phase) -> impl IntoElement {
    let rows = phase.rows();
    let paint = move |bounds: Bounds<Pixels>, (), window: &mut Window, _: &mut gpui::App| {
        let scale = window.scale_factor();
        let cell = device_cell(size.cell_points(), scale);
        let corner = device_corner(bounds, cell * GRID, scale);
        for quad in quads(&rows, corner, cell) {
            window.paint_quad(fill(quad.in_points(scale), ink.colour()));
        }
    };
    canvas(|_, _, _| (), paint)
        .size(size.box_side())
        .flex_none()
}

/// The device pixels one cell takes on a screen of `scale`: the nearest whole number, never none.
/// On a screen whose scale is not whole (1.25, 1.5) the mark is a little smaller or larger than its
/// points say, and still sharp.
pub(super) fn device_cell(cell_points: u32, scale: f32) -> u32 {
    ((cell_points as f32 * scale).round() as u32).max(1)
}

/// The top-left corner, in device pixels, of a drawing `side` device pixels wide and `tall` high that
/// stands at the centre of `bounds`: on a whole pixel.
pub(super) fn device_corner_of(
    bounds: Bounds<Pixels>,
    (side, tall): (u32, u32),
    scale: f32,
) -> (i32, i32) {
    let centre = bounds.center();
    let from = |centre: Pixels, length: u32| {
        (f32::from(centre) * scale - length as f32 / 2.).round() as i32
    };
    (from(centre.x, side), from(centre.y, tall))
}

fn device_corner(bounds: Bounds<Pixels>, side: u32, scale: f32) -> (i32, i32) {
    device_corner_of(bounds, (side, side), scale)
}

/// A rectangle in whole device pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct DeviceRect {
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
}

impl DeviceRect {
    /// The rectangle in points on a screen of `scale`: GPUI rounds each edge back to the device
    /// pixel it came from.
    pub fn in_points(self, scale: f32) -> Bounds<Pixels> {
        let at = |device: i32| px(device as f32 / scale);
        Bounds::from_corners(
            point(at(self.left), at(self.top)),
            point(
                at(self.left + self.width as i32),
                at(self.top + self.height as i32),
            ),
        )
    }
}

/// One rectangle per run of lit cells, for a drawing whose corner is at `corner` and whose cells are
/// `cell` device pixels a side.
fn quads(rows: &Rows, (left, top): (i32, i32), cell: u32) -> Vec<DeviceRect> {
    let of_row = |(row, lit): (usize, &u32)| {
        let top = top + (row as u32 * cell) as i32;
        runs(*lit).map(move |(start, length)| DeviceRect {
            left: left + (start * cell) as i32,
            top,
            width: length * cell,
            height: cell,
        })
    };
    rows.iter().enumerate().flat_map(of_row).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::size;

    #[test]
    fn only_whole_multiples_of_the_grid_are_sizes_of_the_mark() {
        for points in [32, 64, 96, 128, 256] {
            assert!(MarkSize::is_on_grid(points), "{points}");
            assert_eq!(MarkSize::points(points).0, points);
        }
        for points in [0, 16, 24, 48, 100] {
            assert!(!MarkSize::is_on_grid(points), "{points}");
        }
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "a multiple of 32: 48")]
    fn a_size_off_the_grid_is_refused_in_a_debug_build() {
        let _ = MarkSize::points(48);
    }

    #[test]
    fn the_layout_box_holds_four_cells_of_clear_space_on_every_side() {
        assert_eq!(MarkSize::SMALL.box_side(), px(40.));
        assert_eq!(MarkSize::MEDIUM.box_side(), px(80.));
    }

    #[test]
    fn a_cell_is_a_whole_number_of_device_pixels_on_every_screen() {
        let screens = [
            (1, 1.0, 1),
            (1, 2.0, 2),
            (2, 2.0, 4),
            (4, 2.0, 8),
            (1, 3.0, 3),
        ];
        for (cell_points, scale, expected) in screens {
            assert_eq!(device_cell(cell_points, scale), expected);
        }
        // Scales that are not whole: the nearest whole pixel, never none.
        assert_eq!(device_cell(1, 1.25), 1);
        assert_eq!(device_cell(1, 1.5), 2);
        assert_eq!(device_cell(2, 1.25), 3);
        assert_eq!(device_cell(1, 0.4), 1);
    }

    #[test]
    fn the_drawing_is_centred_in_its_box_on_a_whole_device_pixel() {
        let at = |x: f32, y: f32| Bounds::new(point(px(x), px(y)), size(px(40.), px(40.)));
        assert_eq!(device_corner(at(0., 0.), 64, 2.0), (8, 8));
        // A box that the layout left on a third of a point: the drawing still starts on a pixel.
        assert_eq!(device_corner(at(10.33, 7.66), 64, 2.0), (29, 23));
        assert_eq!(device_corner(at(10.33, 7.66), 32, 1.0), (14, 12));
    }

    #[test]
    fn the_whole_mark_is_one_quad_per_run_and_covers_each_lit_cell_once() {
        let rows = Phase::Whole.rows();
        let cell = 4;
        let painted = quads(&rows, (10, 20), cell);
        let runs_in_all: usize = rows.iter().map(|row| runs(*row).count()).sum();
        assert_eq!(painted.len(), runs_in_all);
        let lit: u32 = rows.iter().map(|row| row.count_ones()).sum();
        let area: u32 = painted.iter().map(|quad| quad.width * quad.height).sum();
        assert_eq!(area, lit * cell * cell);
        for quad in &painted {
            assert_eq!((quad.left - 10) % cell as i32, 0);
            assert_eq!((quad.top - 20) % cell as i32, 0);
            assert_eq!(quad.height, cell);
            assert!(quad.left >= 10 && quad.left + quad.width as i32 <= 10 + (GRID * cell) as i32);
        }
    }

    #[test]
    fn a_quad_goes_back_to_the_device_pixels_it_came_from() {
        let quad = DeviceRect {
            left: 29,
            top: 23,
            width: 6,
            height: 2,
        };
        for scale in [1.0, 1.25, 1.5, 2.0, 3.0] {
            let bounds = quad.in_points(scale);
            let device = |points: Pixels| (f32::from(points) * scale).round() as i32;
            assert_eq!(device(bounds.left()), 29, "{scale}");
            assert_eq!(device(bounds.top()), 23, "{scale}");
            assert_eq!(device(bounds.right()), 35, "{scale}");
            assert_eq!(device(bounds.bottom()), 25, "{scale}");
        }
    }

    #[test]
    fn a_phase_with_nothing_lit_paints_nothing() {
        assert!(quads(&Phase::Nothing.rows(), (0, 0), 2).is_empty());
    }
}
