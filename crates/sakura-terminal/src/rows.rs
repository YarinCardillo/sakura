//! Turns one grid row into what the element draws: background spans and text segments whose adjacent cells of
//! the same style are batched into one run. Plain data, so it is tested without a window.
//!
//! The batching rules (skip wide-character spacers, cut a segment at a blank or a wide character so glyphs stay
//! on the cell grid) are adapted from Zed's `terminal_element.rs` (`layout_grid`, `BatchedTextRun`), zed@d2a4fb6,
//! GPL-3.0-or-later, Copyright Zed Industries.
//!
//! Modified for Sakura in November 2026: this file is a changed adaptation, not Zed's original.

use crate::palette::TerminalPalette;
use alacritty_terminal::grid::Row;
use alacritty_terminal::index::Column;
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color, NamedColor};

/// How a run of text is underlined.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Underline {
    /// Single, double, dotted and dashed underlines, drawn straight.
    Straight,
    /// Undercurl.
    Curly,
}

/// The look of one run of text. Colours are `0xRRGGBB`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct CellStyle {
    pub fg: u32,
    pub faint: bool,
    pub bold: bool,
    pub italic: bool,
    pub underline: Option<(Underline, u32)>,
    pub strikeout: bool,
}

/// A run of `len` bytes of a segment's text sharing one style.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StyledRun {
    pub len: usize,
    pub style: CellStyle,
}

/// Text drawn from `column` on, one cell per base character.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TextSegment {
    pub column: usize,
    pub text: String,
    pub runs: Vec<StyledRun>,
    /// Cells this segment covers (a wide character covers two).
    pub cells: usize,
    /// Byte offset in `text` of each cell's base character; combining marks follow their base.
    pub bases: Vec<usize>,
}

/// A block element (U+2580 to U+259F) drawn as rectangles filling its cell instead of as a glyph, so
/// adjacent blocks join without gaps at any line height.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BlockCell {
    pub column: usize,
    pub character: char,
    pub fg: u32,
    pub faint: bool,
}

/// A background colour across `width` cells from `column`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BackgroundSpan {
    pub column: usize,
    pub width: usize,
    pub color: u32,
}

/// Everything drawn for one visible row.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct RowLayout {
    pub segments: Vec<TextSegment>,
    pub backgrounds: Vec<BackgroundSpan>,
    pub blocks: Vec<BlockCell>,
}

impl RowLayout {
    /// The row's characters with blanks as spaces and trailing blanks trimmed (used by tests and logs).
    pub fn text(&self) -> String {
        let mut line = String::new();
        let mut column = 0;
        for segment in &self.segments {
            line.extend(std::iter::repeat_n(
                ' ',
                segment.column.saturating_sub(column),
            ));
            line.push_str(&segment.text);
            column = segment.column + segment.cells;
        }
        line
    }
}

/// Builds a [`RowLayout`] from a grid row.
pub(crate) fn layout_row(
    row: &Row<Cell>,
    columns: usize,
    palette: &TerminalPalette,
    colors: &Colors,
) -> RowLayout {
    let mut builder = RowBuilder::default();
    for column in 0..columns {
        let cell = &row[Column(column)];
        if cell
            .flags
            .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
        {
            builder.end_segment();
            continue;
        }
        let (style, background) = cell_colors(cell, palette, colors);
        let width = if cell.flags.contains(Flags::WIDE_CHAR) {
            2
        } else {
            1
        };
        if let Some(color) = background {
            builder.push_background(column, width, color);
        }
        builder.push_cell(column, cell, style, width);
    }
    builder.finish()
}

/// The text style and the background colour (`None` for the default background) of a cell.
fn cell_colors(
    cell: &Cell,
    palette: &TerminalPalette,
    colors: &Colors,
) -> (CellStyle, Option<u32>) {
    let (mut fg, mut bg) = (cell.fg, cell.bg);
    if cell.flags.contains(Flags::INVERSE) {
        std::mem::swap(&mut fg, &mut bg);
    }
    let fg_rgb = palette.resolve(fg, colors);
    let background = (!is_default_background(bg, colors)).then(|| palette.resolve(bg, colors));
    let underline = underline_kind(cell.flags).map(|kind| {
        let color = cell
            .underline_color()
            .map_or(fg_rgb, |color| palette.resolve(color, colors));
        (kind, color)
    });
    let style = CellStyle {
        fg: fg_rgb,
        faint: cell.flags.contains(Flags::DIM),
        bold: cell.flags.contains(Flags::BOLD),
        italic: cell.flags.contains(Flags::ITALIC),
        underline,
        strikeout: cell.flags.contains(Flags::STRIKEOUT),
    };
    (style, background)
}

fn is_default_background(color: Color, colors: &Colors) -> bool {
    color == Color::Named(NamedColor::Background)
        && colors[NamedColor::Background as usize].is_none()
}

fn underline_kind(flags: Flags) -> Option<Underline> {
    if flags.contains(Flags::UNDERCURL) {
        Some(Underline::Curly)
    } else if flags.intersects(Flags::ALL_UNDERLINES) {
        Some(Underline::Straight)
    } else {
        None
    }
}

/// A cell that draws nothing besides its background.
fn is_blank(cell: &Cell, style: &CellStyle) -> bool {
    let empty = matches!(cell.c, ' ' | '\t' | '\0') || cell.flags.contains(Flags::HIDDEN);
    empty && style.underline.is_none() && !style.strikeout
}

#[derive(Default)]
struct RowBuilder {
    layout: RowLayout,
    current: Option<TextSegment>,
}

impl RowBuilder {
    fn push_background(&mut self, column: usize, width: usize, color: u32) {
        if let Some(last) = self.layout.backgrounds.last_mut() {
            if last.color == color && last.column + last.width == column {
                last.width += width;
                return;
            }
        }
        self.layout.backgrounds.push(BackgroundSpan {
            column,
            width,
            color,
        });
    }

    fn push_cell(&mut self, column: usize, cell: &Cell, style: CellStyle, width: usize) {
        if is_blank(cell, &style) {
            self.end_segment();
            return;
        }
        let character = if cell.flags.contains(Flags::HIDDEN) {
            ' '
        } else {
            cell.c
        };
        if crate::blocks::is_block_element(character) {
            self.end_segment();
            self.layout.blocks.push(BlockCell {
                column,
                character,
                fg: style.fg,
                faint: style.faint,
            });
            return;
        }
        if width > 1 {
            // A wide glyph spans two cells: it gets its own segment, so every other segment has one cell
            // per base character.
            self.end_segment();
            self.append(column, character, cell, style, width);
            self.end_segment();
        } else {
            self.append(column, character, cell, style, width);
        }
    }

    /// Adds a cell's character and combining marks to the open segment, extending its last run when the
    /// style is the same.
    fn append(
        &mut self,
        column: usize,
        character: char,
        cell: &Cell,
        style: CellStyle,
        width: usize,
    ) {
        let segment = self.current.get_or_insert_with(|| TextSegment {
            column,
            text: String::new(),
            runs: Vec::new(),
            cells: 0,
            bases: Vec::new(),
        });
        let start = segment.text.len();
        segment.bases.push(start);
        segment.text.push(character);
        segment.text.extend(cell.zerowidth().unwrap_or_default());
        segment.cells += width;
        let len = segment.text.len() - start;
        match segment.runs.last_mut() {
            Some(run) if run.style == style => run.len += len,
            _ => segment.runs.push(StyledRun { len, style }),
        }
    }

    fn end_segment(&mut self) {
        if let Some(segment) = self.current.take() {
            self.layout.segments.push(segment);
        }
    }

    fn finish(mut self) -> RowLayout {
        self.end_segment();
        self.layout
    }
}
