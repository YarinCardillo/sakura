//! The size of a terminal grid in cells, with the cell size in pixels that programs may ask for.

use alacritty_terminal::grid::Dimensions;

/// Fewest columns a terminal is laid out with; narrower grids confuse programs that draw wide characters.
pub const MIN_COLUMNS: u16 = 2;
/// Fewest rows a terminal is laid out with.
pub const MIN_ROWS: u16 = 1;

/// A grid size in cells, plus the size of one cell in whole pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridSize {
    /// Columns, at least [`MIN_COLUMNS`].
    pub columns: u16,
    /// Rows, at least [`MIN_ROWS`].
    pub rows: u16,
    /// Width of one cell in pixels (reported to programs that ask, never used for layout).
    pub cell_width: u16,
    /// Height of one cell in pixels.
    pub cell_height: u16,
}

impl GridSize {
    /// A size of `columns` by `rows`, clamped to the minimum, with an unknown cell size.
    pub fn new(columns: u16, rows: u16) -> Self {
        Self {
            columns: columns.max(MIN_COLUMNS),
            rows: rows.max(MIN_ROWS),
            cell_width: 0,
            cell_height: 0,
        }
    }

    /// Same grid with the cell size in pixels.
    pub fn with_cell_size(self, cell_width: u16, cell_height: u16) -> Self {
        Self {
            cell_width,
            cell_height,
            ..self
        }
    }

    /// Whether the columns and rows match, ignoring the pixel size of a cell.
    pub fn same_cells(&self, other: &GridSize) -> bool {
        self.columns == other.columns && self.rows == other.rows
    }
}

impl Default for GridSize {
    fn default() -> Self {
        Self::new(80, 24)
    }
}

impl Dimensions for GridSize {
    fn total_lines(&self) -> usize {
        self.screen_lines()
    }

    fn screen_lines(&self) -> usize {
        usize::from(self.rows)
    }

    fn columns(&self) -> usize {
        usize::from(self.columns)
    }
}
