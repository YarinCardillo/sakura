//! The emulator without a window: an alacritty `Term` with its parser, the replies it owes the program, its
//! selection and scrollback, and the damage-driven row cache the element draws from.

use crate::palette::{unpack, TerminalPalette};
use crate::rows::{layout_row, RowLayout};
use crate::size::GridSize;
use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionRange, SelectionType};
use alacritty_terminal::term::{Config, Osc52, Term, TermDamage, TermMode};
use alacritty_terminal::vte::ansi::{CursorShape, Processor};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Instant;

/// Lines of scrollback kept above the screen.
pub const SCROLLBACK_LINES: usize = 10_000;

/// Forwards the emulator's events to the core, which handles them after each parse.
pub(crate) struct Listener(Sender<Event>);

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        // The receiver lives in the same `TermCore`, so a send only fails while the core is dropped.
        let _ = self.0.send(event);
    }
}

/// What parsing a chunk of output produced besides grid changes.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Advance {
    /// Bytes the program requested (device status, colour queries): they go back to the source.
    pub reply: Vec<u8>,
    /// `Some` when the title changed; `Some(None)` when it was reset.
    pub title: Option<Option<String>>,
    /// The program rang the bell.
    pub bell: bool,
}

/// How a mouse press starts a selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SelectionKind {
    /// Drag: character by character.
    Simple,
    /// Double click: whole words.
    Word,
    /// Triple click: whole lines.
    Line,
}

/// A cell in viewport coordinates: line 0 is the top of what is on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ViewportPoint {
    pub line: usize,
    pub column: usize,
}

/// Where and how to draw the cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CursorState {
    pub point: ViewportPoint,
    pub shape: CursorShape,
    /// The character under the cursor, drawn in the background colour inside a block cursor.
    pub character: char,
    pub is_wide: bool,
}

/// Cells `start..end` of visible row `line` covered by the selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SelectionSpan {
    pub line: usize,
    pub start: usize,
    pub end: usize,
}

/// The visible part of a selection, one span per row, for a viewport of `rows` by `columns`.
pub(crate) fn selection_spans(
    range: &SelectionRange,
    display_offset: usize,
    rows: usize,
    columns: usize,
) -> Vec<SelectionSpan> {
    let to_viewport = |line: Line| line.0 + display_offset as i32;
    let (first, last) = (to_viewport(range.start.line), to_viewport(range.end.line));
    let visible = first.max(0)..=last.min(rows as i32 - 1);
    visible
        .map(|line| {
            let start = if range.is_block || line == first {
                range.start.column.0
            } else {
                0
            };
            let end = if range.is_block || line == last {
                range.end.column.0 + 1
            } else {
                columns
            };
            SelectionSpan {
                line: line as usize,
                start,
                end: end.min(columns),
            }
        })
        .filter(|span| span.start < span.end)
        .collect()
}

/// Rows laid out for drawing, rebuilt only where the emulator reports damage.
#[derive(Default)]
pub(crate) struct GridFrame {
    pub rows: Vec<RowLayout>,
    /// For each row, the refresh in which it was last rebuilt.
    pub built_at: Vec<u64>,
    pub generation: u64,
    columns: usize,
}

impl GridFrame {
    /// Forgets every row, so the next refresh rebuilds all of them (after a palette change).
    pub fn invalidate(&mut self) {
        self.rows.clear();
        self.built_at.clear();
    }
}

/// The emulator state of one terminal.
pub(crate) struct TermCore {
    term: Term<Listener>,
    parser: Processor,
    events: Receiver<Event>,
    palette: TerminalPalette,
    size: GridSize,
}

impl TermCore {
    /// A terminal of `size` drawing with `palette`, with 10000 lines of scrollback and OSC 52 refused.
    pub fn new(size: GridSize, palette: TerminalPalette) -> Self {
        let (sender, events) = channel();
        let config = Config {
            scrolling_history: SCROLLBACK_LINES,
            osc52: Osc52::Disabled,
            ..Config::default()
        };
        let term = Term::new(config, &size, Listener(sender));
        Self {
            term,
            parser: Processor::new(),
            events,
            palette,
            size,
        }
    }

    /// Parses program output.
    pub fn advance(&mut self, bytes: &[u8]) -> Advance {
        self.parser.advance(&mut self.term, bytes);
        self.drain_events()
    }

    /// When a synchronized update (DEC mode 2026) is pending, the moment it must be flushed anyway.
    pub fn sync_deadline(&self) -> Option<Instant> {
        self.parser.sync_timeout().sync_timeout()
    }

    /// Ends a synchronized update whose deadline passed.
    pub fn flush_expired_sync(&mut self) -> Advance {
        if self
            .sync_deadline()
            .is_some_and(|deadline| deadline <= Instant::now())
        {
            self.parser.stop_sync(&mut self.term);
        }
        self.drain_events()
    }

    /// Resizes the grid; lines reflow when the width changes.
    pub fn resize(&mut self, size: GridSize) {
        if size != self.size {
            self.size = size;
            self.term.resize(size);
        }
    }

    pub fn size(&self) -> GridSize {
        self.size
    }

    pub fn mode(&self) -> TermMode {
        *self.term.mode()
    }

    pub fn set_palette(&mut self, palette: TerminalPalette) {
        self.palette = palette;
    }

    /// Tells the emulator whether the view has focus.
    pub fn set_focused(&mut self, is_focused: bool) {
        self.term.is_focused = is_focused;
    }

    fn drain_events(&mut self) -> Advance {
        let mut advance = Advance::default();
        while let Ok(event) = self.events.try_recv() {
            self.handle_event(event, &mut advance);
        }
        advance
    }

    fn handle_event(&self, event: Event, advance: &mut Advance) {
        match event {
            Event::PtyWrite(text) => advance.reply.extend_from_slice(text.as_bytes()),
            Event::ColorRequest(index, format) => {
                let color = self.palette.by_index(index, self.term.colors());
                advance
                    .reply
                    .extend_from_slice(format(unpack(color)).as_bytes());
            }
            Event::TextAreaSizeRequest(format) => {
                advance
                    .reply
                    .extend_from_slice(format(self.window_size()).as_bytes());
            }
            Event::Title(title) => advance.title = Some(Some(title)),
            Event::ResetTitle => advance.title = Some(None),
            Event::Bell => advance.bell = true,
            // OSC 52 is disabled in the config; a store or load that still arrives is refused here.
            Event::ClipboardStore(..) | Event::ClipboardLoad(..) => {}
            Event::MouseCursorDirty
            | Event::CursorBlinkingChange
            | Event::Wakeup
            | Event::Exit
            | Event::ChildExit(_) => {}
        }
    }

    fn window_size(&self) -> WindowSize {
        WindowSize {
            num_lines: self.size.rows,
            num_cols: self.size.columns,
            cell_width: self.size.cell_width,
            cell_height: self.size.cell_height,
        }
    }
}

/// Scrollback.
impl TermCore {
    /// Lines scrolled back into history; 0 is the live screen.
    pub fn display_offset(&self) -> usize {
        self.term.grid().display_offset()
    }

    /// Lines of history above the screen.
    #[cfg(test)]
    pub fn history_size(&self) -> usize {
        self.term.grid().history_size()
    }

    /// Scrolls the view; positive goes back into history.
    pub fn scroll(&mut self, lines: i32) {
        self.term.scroll_display(Scroll::Delta(lines));
    }

    pub fn scroll_to_bottom(&mut self) {
        if self.display_offset() != 0 {
            self.term.scroll_display(Scroll::Bottom);
        }
    }
}

/// Selection.
impl TermCore {
    pub fn start_selection(&mut self, kind: SelectionKind, at: ViewportPoint, side: Side) {
        let ty = match kind {
            SelectionKind::Simple => SelectionType::Simple,
            SelectionKind::Word => SelectionType::Semantic,
            SelectionKind::Line => SelectionType::Lines,
        };
        let point = self.grid_point(at);
        self.term.selection = Some(Selection::new(ty, point, side));
    }

    /// Moves the free end of the selection; returns whether there is a selection to extend.
    pub fn extend_selection(&mut self, to: ViewportPoint, side: Side) -> bool {
        let point = self.grid_point(to);
        match self.term.selection.as_mut() {
            Some(selection) => {
                selection.update(point, side);
                true
            }
            None => false,
        }
    }

    #[cfg(test)]
    pub fn clear_selection(&mut self) {
        self.term.selection = None;
    }

    pub fn has_selection(&self) -> bool {
        self.term
            .selection
            .as_ref()
            .is_some_and(|selection| !selection.is_empty())
    }

    pub fn selection_text(&self) -> Option<String> {
        self.term
            .selection_to_string()
            .filter(|text| !text.is_empty())
    }

    pub fn selection_range(&self) -> Option<SelectionRange> {
        self.term
            .selection
            .as_ref()
            .and_then(|selection| selection.to_range(&self.term))
    }

    fn grid_point(&self, at: ViewportPoint) -> Point {
        let line = Line(at.line as i32 - self.display_offset() as i32);
        let column = Column(at.column.min(self.size.columns.saturating_sub(1) as usize));
        Point::new(line, column)
    }
}

/// Drawing.
impl TermCore {
    /// Rebuilds the rows of `frame` that changed since the last refresh; returns how many were rebuilt.
    pub fn refresh(&mut self, frame: &mut GridFrame) -> usize {
        let (rows, columns) = (self.term.screen_lines(), self.term.columns());
        frame.generation += 1;
        let is_full = frame.rows.len() != rows || frame.columns != columns;
        let damaged: Vec<usize> = match self.term.damage() {
            TermDamage::Full => (0..rows).collect(),
            _ if is_full => (0..rows).collect(),
            TermDamage::Partial(lines) => lines.map(|damage| damage.line).collect(),
        };
        self.term.reset_damage();
        if is_full {
            frame.rows = vec![RowLayout::default(); rows];
            frame.built_at = vec![0; rows];
            frame.columns = columns;
        }
        for &line in &damaged {
            frame.rows[line] = self.layout_viewport_row(line);
            frame.built_at[line] = frame.generation;
        }
        damaged.len()
    }

    /// Lays out one visible row from scratch.
    pub fn layout_viewport_row(&self, line: usize) -> RowLayout {
        let grid_line = Line(line as i32 - self.display_offset() as i32);
        let row = &self.term.grid()[grid_line];
        layout_row(row, self.term.columns(), &self.palette, self.term.colors())
    }

    /// The last visible row that holds text, trailing blanks trimmed: what a program printed last before it
    /// exited (its error, for a command that failed).
    pub fn last_line(&self) -> Option<String> {
        (0..self.term.screen_lines())
            .rev()
            .map(|line| self.layout_viewport_row(line).text())
            .find(|text| !text.trim().is_empty())
            .map(|text| text.trim().to_string())
    }

    /// The cursor when it is on screen and not hidden by the program.
    pub fn cursor(&self) -> Option<CursorState> {
        let content = self.term.renderable_content();
        let cursor = content.cursor;
        if cursor.shape == CursorShape::Hidden {
            return None;
        }
        let line = usize::try_from(cursor.point.line.0 + content.display_offset as i32).ok()?;
        if line >= self.term.screen_lines() {
            return None;
        }
        let cell = &self.term.grid()[cursor.point];
        Some(CursorState {
            point: ViewportPoint {
                line,
                column: cursor.point.column.0,
            },
            shape: cursor.shape,
            character: cell.c,
            is_wide: cell
                .flags
                .contains(alacritty_terminal::term::cell::Flags::WIDE_CHAR),
        })
    }
}

impl TermCore {
    /// The cell at visible row `line` and `column`.
    pub fn cell(&self, line: usize, column: usize) -> &alacritty_terminal::term::cell::Cell {
        let grid_line = Line(line as i32 - self.display_offset() as i32);
        &self.term.grid()[grid_line][Column(column)]
    }
}

#[cfg(test)]
impl TermCore {
    /// The text of visible line `line`, trailing blanks trimmed.
    pub fn line_text(&self, line: usize) -> String {
        self.layout_viewport_row(line).text()
    }
}
