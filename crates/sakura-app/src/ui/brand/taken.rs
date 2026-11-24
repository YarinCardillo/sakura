//! The approved filled flower, used only for the Taken state at the state mark's size.

use super::cells::{runs, Rows};
use super::mark::{device_corner_of, quads};
use crate::theme;
use crate::theme_brand::GRID;
use gpui::{
    canvas, fill, point, Bounds, Hsla, IntoElement, Path, PathBuilder, Pixels, Styled, Window,
};

const SOURCE: &str = include_str!("taken.txt");
const DRAWING: Rows = drawing();

const fn drawing() -> Rows {
    let bytes = SOURCE.as_bytes();
    let mut rows = [0; GRID as usize];
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'#' {
            rows[index / (GRID as usize + 1)] |= 1 << (index % (GRID as usize + 1));
        }
        index += 1;
    }
    rows
}

/// The Taken flower at the shared state-mark size, in the surrounding text's ink.
pub fn taken_mark(colour: Hsla) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, (), window, _| paint(bounds, colour, window),
    )
    .size(theme::MARK_SIZE)
    .flex_none()
}

fn paint(bounds: Bounds<Pixels>, colour: Hsla, window: &mut Window) {
    let scale = window.scale_factor();
    if let Some(cell) = device_cell(scale) {
        let side = cell * GRID;
        let corner = device_corner_of(bounds, (side, side), scale);
        for quad in quads(&DRAWING, corner, cell) {
            window.paint_quad(fill(quad.in_points(scale), colour));
        }
    } else {
        match path(bounds) {
            Ok(path) => window.paint_path(path, colour),
            Err(error) => eprintln!("sakura: Taken flower: {error}"),
        }
    }
}

/// Quads are sharp only when each cell is a whole device pixel. Fractional cells use a path.
fn device_cell(scale: f32) -> Option<u32> {
    let cell = f32::from(theme::MARK_SIZE) * scale / GRID as f32;
    (cell >= 1. && cell.fract() == 0.).then_some(cell as u32)
}

fn path(bounds: Bounds<Pixels>) -> gpui::Result<Path<Pixels>> {
    let cell = theme::MARK_SIZE / GRID as f32;
    let mut builder = PathBuilder::fill();
    for (row, lit) in DRAWING.iter().enumerate() {
        for (start, length) in runs(*lit) {
            let left = bounds.left() + cell * start as f32;
            let top = bounds.top() + cell * row as f32;
            let right = left + cell * length as f32;
            let bottom = top + cell;
            builder.add_polygon(
                &[
                    point(left, top),
                    point(right, top),
                    point(right, bottom),
                    point(left, bottom),
                ],
                true,
            );
        }
    }
    builder.build()
}

#[cfg(test)]
mod tests {
    use super::super::cells::Phase;
    use super::*;
    use gpui::px;

    #[test]
    fn the_approved_grid_is_symmetric_and_has_426_lit_cells() {
        let lines: Vec<_> = SOURCE.lines().collect();
        assert_eq!(lines.len(), GRID as usize);
        assert!(lines.iter().all(|line| line.len() == GRID as usize));
        assert!(SOURCE
            .bytes()
            .all(|byte| matches!(byte, b'.' | b'#' | b'\n')));
        assert_eq!(DRAWING.iter().map(|row| row.count_ones()).sum::<u32>(), 426);
        for row in DRAWING {
            assert_eq!(row, row.reverse_bits());
        }
        assert_eq!(DRAWING[3], 0b111 << 12 | 0b111 << 17);
        assert_eq!(DRAWING[17].count_ones(), 18, "the dotted row is filled");
    }

    #[test]
    fn every_flower_cell_is_lit_or_in_the_brand_s_dither() {
        let brand = Phase::Whole.rows();
        let is_lit = |x: i32, y: i32| {
            (0..GRID as i32).contains(&x)
                && (0..GRID as i32).contains(&y)
                && brand[y as usize] & (1 << x) != 0
        };
        let mut filled = 0;
        for (row, lit) in DRAWING.iter().enumerate() {
            for column in 0..GRID {
                if lit & (1 << column) == 0 || is_lit(column as i32, row as i32) {
                    continue;
                }
                assert!(
                    [17, 19, 21, 23, 25, 26, 27].contains(&row),
                    "solid brand cell changed"
                );
                let (x, y) = (column as i32, row as i32);
                let is_gap = (is_lit(x - 1, y) && is_lit(x + 1, y))
                    || (is_lit(x, y - 1) && is_lit(x, y + 1));
                let inward = if x < GRID as i32 / 2 { 1 } else { -1 };
                let is_dither_edge = y == 21
                    && [6, 25].contains(&x)
                    && is_lit(x + inward, y - 1)
                    && is_lit(x, y + 1);
                assert!(
                    is_gap || is_dither_edge,
                    "cell {x},{y} is outside the brand dither"
                );
                filled += 1;
            }
        }
        assert_eq!(
            filled, 54,
            "only the brand's dotted and dithered gaps were filled"
        );
    }

    #[test]
    fn retina_uses_pixels_and_fractional_cells_use_an_antialiased_path() {
        assert_eq!(device_cell(2.), Some(1));
        assert_eq!(device_cell(4.), Some(2));
        for scale in [1., 1.25, 1.5, 3.] {
            assert_eq!(device_cell(scale), None);
        }
        let bounds = Bounds::new(
            point(px(0.25), px(0.25)),
            gpui::size(theme::MARK_SIZE, theme::MARK_SIZE),
        );
        let shape = path(bounds).expect("the flower builds a path");
        assert!(
            shape.vertices.iter().any(|vertex| {
                f32::from(vertex.xy_position.x).fract() == 0.75
                    || f32::from(vertex.xy_position.y).fract() == 0.75
            }),
            "half-point cells retain their fractional edges"
        );
    }
}
