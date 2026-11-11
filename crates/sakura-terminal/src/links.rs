//! Addresses in the terminal's text. A line the program printed may take several rows of the grid: it is
//! read as one line, so an address that the terminal wrapped is whole. Which addresses may be opened is
//! `sakura-core`'s rule (`http` and `https` only); here they are found under a cell, with the cells they span.

use crate::core::{TermCore, ViewportPoint};
use alacritty_terminal::term::cell::Flags;
use std::ops::Range;

/// An address on screen: what it opens and the cells it spans, as visible row and columns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    pub url: String,
    pub spans: Vec<(usize, Range<usize>)>,
}

impl TermCore {
    /// The lines on screen as the program printed them: rows the terminal wrapped are joined, trailing
    /// blanks trimmed.
    pub fn logical_lines(&self) -> Vec<String> {
        let rows = usize::from(self.size().rows);
        let mut lines: Vec<String> = Vec::new();
        let mut is_continuing = false;
        for row in 0..rows {
            let cells = self.row_cells(row);
            match lines.last_mut().filter(|_| is_continuing) {
                Some(line) => line.push_str(&cells),
                None => lines.push(cells),
            }
            is_continuing = self.is_wrapped(row);
        }
        lines
            .iter()
            .map(|line| line.trim_end().to_string())
            .collect()
    }

    /// The `http` or `https` address under `point`, if there is one.
    pub fn link_at(&self, point: ViewportPoint) -> Option<Link> {
        let columns = usize::from(self.size().columns);
        let (first_row, line) = self.logical_line(point.line);
        let column = (point.line - first_row) * columns + point.column;
        let (span, url) = sakura_core::links::web_url_at(&line, column)?;
        let rows = span.start / columns..=(span.end - 1) / columns;
        let spans = rows.map(|row| {
            let from = span.start.max(row * columns) - row * columns;
            let to = span.end.min((row + 1) * columns) - row * columns;
            (first_row + row, from..to)
        });
        Some(Link {
            url,
            spans: spans.collect(),
        })
    }

    /// The line through visible row `row` as the program printed it, with the visible row it starts on.
    fn logical_line(&self, row: usize) -> (usize, String) {
        let rows = usize::from(self.size().rows);
        let mut first = row;
        while first > 0 && self.is_wrapped(first - 1) {
            first -= 1;
        }
        let mut line = String::new();
        for current in first..rows {
            line.push_str(&self.row_cells(current));
            if !self.is_wrapped(current) {
                break;
            }
        }
        (first, line)
    }

    /// The characters of visible row `row`, one per cell: a blank for an empty cell and for the second
    /// half of a wide character, so a character's place in the text is its column.
    fn row_cells(&self, row: usize) -> String {
        let spacer = Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER;
        let cell = |column| {
            let cell = self.cell(row, column);
            if cell.flags.intersects(spacer) || cell.c == '\0' {
                ' '
            } else {
                cell.c
            }
        };
        (0..usize::from(self.size().columns)).map(cell).collect()
    }

    /// Whether the program's line on visible row `row` goes on onto the next row.
    fn is_wrapped(&self, row: usize) -> bool {
        let last = usize::from(self.size().columns).saturating_sub(1);
        self.cell(row, last).flags.contains(Flags::WRAPLINE)
    }
}
