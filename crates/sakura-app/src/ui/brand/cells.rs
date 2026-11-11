//! The mark's drawing: which cells of its grid are lit, read from the brand's own file, and the order
//! they switch on in. Nothing here draws a shape of its own: the cells are the brand generator's output
//! (`assets/brand/cells.json`, copied as it is), and the order is the Bayer 4x4 matrix the generator
//! dithers the mark with, indexed from the nearest side as the generator does, so the motion is as
//! symmetric as the drawing.

use crate::theme_brand::{GRID, REVEAL_STEPS};
use std::sync::LazyLock;

/// The brand generator's output: the lit cells as `[column, row]` pairs under `on`.
const SOURCE: &str = include_str!("../../../assets/brand/cells.json");

/// The Bayer 4x4 matrix: the rank of each cell of a 4x4 tile.
const BAYER_4: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];

/// The lit cells of each row of the grid, as bits: bit `column` of `rows[row]`.
pub type Rows = [u32; GRID as usize];

/// The mark as the brand draws it.
static DRAWING: LazyLock<Rows> =
    LazyLock::new(|| parse(SOURCE).expect("the brand's cells are a grid of lit cells"));

/// How much of the mark is lit at a moment of its motion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Every cell of the drawing.
    Whole,
    /// No cell.
    Nothing,
    /// Switching on: the cells of the first `n` steps are lit.
    On(u8),
    /// Switching off: the cells of the first `n` steps are dark again.
    Off(u8),
}

impl Phase {
    /// The lit cells of the mark in this phase.
    pub fn rows(self) -> Rows {
        let drawing = *DRAWING;
        let mut rows = [0; GRID as usize];
        for (row, lit) in rows.iter_mut().enumerate() {
            *lit = match self {
                Phase::Whole => drawing[row],
                Phase::Nothing => 0,
                Phase::On(steps) => drawing[row] & switched(row, steps),
                Phase::Off(steps) => drawing[row] & !switched(row, steps),
            };
        }
        rows
    }
}

/// The step (0 to 15) at which the cell at `column`, `row` switches.
fn step_of(column: usize, row: usize) -> u8 {
    let grid = GRID as usize;
    let from_side = column.min(grid - 1 - column);
    BAYER_4[row % 4][from_side % 4]
}

/// The cells of `row` that the first `steps` steps have switched, as bits.
fn switched(row: usize, steps: u8) -> u32 {
    let columns = 0..GRID as usize;
    columns
        .filter(|column| step_of(*column, row) < steps.min(REVEAL_STEPS))
        .fold(0, |bits, column| bits | 1 << column)
}

/// The runs of lit cells in one row: each as its first column and its length.
pub fn runs(lit: u32) -> impl Iterator<Item = (u32, u32)> {
    let mut rest = lit;
    let mut passed = 0;
    std::iter::from_fn(move || {
        if rest == 0 {
            return None;
        }
        let gap = rest.trailing_zeros();
        let length = (rest >> gap).trailing_ones();
        let start = passed + gap;
        // A run can reach the last column: shifting by the whole width is not a shift.
        rest = rest.checked_shr(gap + length).unwrap_or(0);
        passed = start + length;
        Some((start, length))
    })
}

/// The rows of the generator's output; `None` when it is not a grid of [`GRID`] cells with its lit
/// cells inside it.
fn parse(source: &str) -> Option<Rows> {
    let file: serde_json::Value = serde_json::from_str(source).ok()?;
    if file.get("cells")?.as_u64()? != u64::from(GRID) {
        return None;
    }
    let mut rows = [0; GRID as usize];
    for cell in file.get("on")?.as_array()? {
        let column = cell.get(0)?.as_u64()?;
        let row = cell.get(1)?.as_u64()?;
        if column >= u64::from(GRID) || row >= u64::from(GRID) {
            return None;
        }
        rows[row as usize] |= 1 << column;
    }
    Some(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The lit cells the brand's file lists.
    const BRAND_CELLS: u32 = 382;

    fn lit(rows: &Rows) -> u32 {
        rows.iter().map(|row| row.count_ones()).sum()
    }

    #[test]
    fn the_drawing_is_the_brand_s_cells_and_is_symmetric() {
        let rows = Phase::Whole.rows();
        assert_eq!(lit(&rows), BRAND_CELLS);
        for row in rows {
            assert_eq!(row, row.reverse_bits(), "{row:032b}");
        }
        // The top petal's notch, as the brand's file lists its first cells: row 3, columns 12 to 14
        // and 17 to 19.
        assert_eq!(rows[3], 0b111 << 12 | 0b111 << 17);
        assert_eq!(rows[0], 0, "the first rows are empty");
    }

    #[test]
    fn a_file_that_is_not_the_brand_s_grid_is_refused() {
        assert!(parse(r#"{"cells":32,"on":[[0,0],[31,31]]}"#).is_some());
        assert!(parse(r#"{"cells":21,"on":[[0,0]]}"#).is_none());
        assert!(parse(r#"{"cells":32,"on":[[32,0]]}"#).is_none());
        assert!(parse(r#"{"cells":32}"#).is_none());
        assert!(parse("").is_none());
    }

    #[test]
    fn the_runs_of_a_row_cover_its_lit_cells_once_each() {
        assert_eq!(runs(0).count(), 0);
        assert_eq!(runs(0b0110_0111).collect::<Vec<_>>(), [(0, 3), (5, 2)]);
        assert_eq!(runs(u32::MAX).collect::<Vec<_>>(), [(0, 32)]);
        assert_eq!(runs(1 << 31).collect::<Vec<_>>(), [(31, 1)]);
        for row in Phase::Whole.rows() {
            let covered = runs(row).fold(0u32, |bits, (start, length)| {
                let run = (0..length).fold(0, |run, cell| run | 1 << (start + cell));
                assert_eq!(bits & run, 0, "no cell twice");
                bits | run
            });
            assert_eq!(covered, row);
        }
    }

    /// The brand sheet's own order, written as the generator writes it: the rank of a cell in the
    /// Bayer 8x8 matrix built from the 4x4 and the 2x2, over 64, in sixteen steps.
    fn sheet_step(column: usize, row: usize) -> u8 {
        const BAYER_2: [[u8; 2]; 2] = [[0, 2], [3, 1]];
        let mirrored = if column < 16 { column } else { 31 - column };
        let (y, x) = (row % 8, mirrored % 8);
        let rank = 4 * BAYER_4[y % 4][x % 4] + BAYER_2[y >> 2][x >> 2];
        (f32::from(rank) / 64. * 16.).floor() as u8
    }

    #[test]
    fn the_cells_switch_in_the_order_the_brand_sheet_plays() {
        for row in 0..GRID as usize {
            for column in 0..GRID as usize {
                assert_eq!(step_of(column, row), sheet_step(column, row));
            }
        }
    }

    #[test]
    fn sixteen_steps_light_the_whole_mark_and_each_step_lights_more() {
        assert_eq!(Phase::On(REVEAL_STEPS).rows(), Phase::Whole.rows());
        assert_eq!(Phase::On(0).rows(), Phase::Nothing.rows());
        assert_eq!(Phase::Off(REVEAL_STEPS).rows(), Phase::Nothing.rows());
        assert_eq!(Phase::Off(0).rows(), Phase::Whole.rows());
        let mut before = 0;
        for steps in 1..=REVEAL_STEPS {
            let now = lit(&Phase::On(steps).rows());
            assert!(now > before, "step {steps} lights no cell");
            before = now;
        }
    }

    #[test]
    fn switching_on_and_switching_off_split_the_mark_between_them() {
        let whole = Phase::Whole.rows();
        for steps in 0..=REVEAL_STEPS {
            let (on, off) = (Phase::On(steps).rows(), Phase::Off(steps).rows());
            for row in 0..GRID as usize {
                assert_eq!(on[row] & off[row], 0);
                assert_eq!(on[row] | off[row], whole[row]);
            }
        }
    }
}
