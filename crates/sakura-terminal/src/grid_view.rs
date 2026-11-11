//! The grid: backgrounds, selection and text, in a view of its own so GPUI can reuse its last frame when only
//! the cursor blinks. Rows are laid out again only where the emulator reports damage, and shaped text is kept
//! per row until that row changes.
//!
//! Cell geometry, pixel snapping and painting text with a forced cell advance follow Zed's
//! `crates/terminal_view/src/terminal_element.rs` (zed@d2a4fb6, GPL-3.0-or-later, Copyright Zed Industries).
//!
//! Modified for Sakura in November 2026: this file is a changed adaptation, not Zed's original.

use crate::blocks::block_rects;
use crate::core::{selection_spans, CursorState, GridFrame, SelectionSpan};
use crate::palette::FAINT_OPACITY;
use crate::rows::{BlockCell, CellStyle, TextSegment, Underline};
use crate::size::{GridSize, MIN_COLUMNS, MIN_ROWS};
use crate::stats::StatsRecorder;
use crate::style::TerminalStyle;
use crate::terminal::Terminal;
use gpui::{
    fill, point, px, relative, rgb, size, App, Bounds, ContentMask, Context, Element, ElementId,
    Entity, GlobalElementId, Hsla, InspectorElementId, IntoElement, LayoutId, LineLayout, Pixels,
    Point, Render, ShapedGlyph, ShapedLine, ShapedRun, StrikethroughStyle, Style, Subscription,
    TextAlign, TextRun, UnderlineStyle, Window,
};
use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

/// Thickness of underlines and strikeouts.
const DECORATION_THICKNESS: f32 = 1.0;

/// Where the grid is on screen and how big its cells are. Window coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct GridGeometry {
    pub origin: Point<Pixels>,
    pub cell_width: Pixels,
    pub line_height: Pixels,
    pub columns: usize,
    pub rows: usize,
}

impl GridGeometry {
    /// Bounds of `cells` cells from `column` on visible row `line`.
    pub fn cell_bounds(&self, line: usize, column: usize, cells: usize) -> Bounds<Pixels> {
        Bounds::new(
            point(
                self.origin.x + self.cell_width * column as f32,
                self.origin.y + self.line_height * line as f32,
            ),
            size(self.cell_width * cells as f32, self.line_height),
        )
    }
}

/// What the last grid layout saw, shared with the view that draws the cursor on top.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct FrameInfo {
    pub geometry: Option<GridGeometry>,
    pub cursor: Option<CursorState>,
}

/// Font metrics for one style at one scale factor.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Metrics {
    font_size: Pixels,
    cell_width: Pixels,
    line_height: Pixels,
    scale_factor: f32,
}

/// Text shaped for one row, valid while the row's layout has not been rebuilt.
#[derive(Default)]
struct ShapedRow {
    built_at: u64,
    lines: Vec<(usize, ShapedLine)>,
}

/// The view that owns the grid's caches.
pub(crate) struct GridView {
    _terminal_changed: Subscription,
    terminal: Entity<Terminal>,
    style: Rc<TerminalStyle>,
    frame: GridFrame,
    shaped: Vec<ShapedRow>,
    metrics: Option<Metrics>,
    info: Rc<Cell<FrameInfo>>,
    stats: Rc<StatsRecorder>,
}

impl GridView {
    pub fn new(
        terminal: Entity<Terminal>,
        style: Rc<TerminalStyle>,
        info: Rc<Cell<FrameInfo>>,
        stats: Rc<StatsRecorder>,
        cx: &mut Context<Self>,
    ) -> Self {
        // GPUI reuses a cached view's last frame until that view itself is notified, so every change of the
        // terminal (output, selection, scrolling) has to reach the grid explicitly.
        let terminal_changed = cx.observe(&terminal, |_, _, cx| cx.notify());
        Self {
            _terminal_changed: terminal_changed,
            terminal,
            style,
            frame: GridFrame::default(),
            shaped: Vec::new(),
            metrics: None,
            info,
            stats,
        }
    }

    /// Takes a new style: every row is laid out and shaped again.
    pub fn set_style(&mut self, style: Rc<TerminalStyle>, cx: &mut Context<Self>) {
        self.style = style;
        self.metrics = None;
        self.invalidate();
        cx.notify();
    }

    fn invalidate(&mut self) {
        self.frame.invalidate();
        self.shaped.clear();
    }

    /// Lays out the grid for `bounds`: resizes the terminal to fit, rebuilds damaged rows and returns what
    /// to paint.
    fn prepare(
        &mut self,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> GridPaint {
        let started = Instant::now();
        let metrics = self.metrics(window, cx);
        let geometry = self.fit_grid(bounds, metrics, window.scale_factor(), cx);
        let (rebuilt, spans, cursor) = self.refresh_frame(&geometry, cx);
        let segments = self.shape_rows(metrics, window);
        self.info.set(FrameInfo {
            geometry: Some(geometry),
            cursor,
        });
        let paint = self.paint_list(&geometry, &spans);
        self.stats
            .grid_laid_out(started.elapsed(), rebuilt, segments);
        paint
    }

    /// The grid that fits `bounds`, resizing the terminal to it. A layout too small for a grid (a window's
    /// first frame, a collapsed panel) keeps the size the terminal has: shrinking to nothing would reflow
    /// every line into the scrollback.
    fn fit_grid(
        &mut self,
        bounds: Bounds<Pixels>,
        metrics: Metrics,
        scale_factor: f32,
        cx: &mut Context<Self>,
    ) -> GridGeometry {
        let mut geometry = fit(bounds, metrics, scale_factor);
        let current = self.terminal.read(cx).shared().lock().size();
        if geometry.columns < usize::from(MIN_COLUMNS) || geometry.rows < usize::from(MIN_ROWS) {
            geometry.columns = usize::from(current.columns);
            geometry.rows = usize::from(current.rows);
        } else {
            self.fit_terminal(&geometry, current, metrics, cx);
        }
        geometry
    }

    /// Rebuilds damaged rows under one short lock and reads what the overlay and selection need.
    fn refresh_frame(
        &mut self,
        geometry: &GridGeometry,
        cx: &mut Context<Self>,
    ) -> (usize, Vec<SelectionSpan>, Option<CursorState>) {
        let shared = self.terminal.read(cx).shared().clone();
        let mut core = shared.lock();
        let rebuilt = core.refresh(&mut self.frame);
        let spans = core
            .selection_range()
            .map(|range| {
                selection_spans(
                    &range,
                    core.display_offset(),
                    geometry.rows,
                    geometry.columns,
                )
            })
            .unwrap_or_default();
        (rebuilt, spans, core.cursor())
    }

    fn metrics(&mut self, window: &mut Window, cx: &mut App) -> Metrics {
        let scale_factor = window.scale_factor();
        if let Some(metrics) = self
            .metrics
            .filter(|metrics| metrics.scale_factor == scale_factor)
        {
            return metrics;
        }
        let metrics = measure(&self.style, scale_factor, cx);
        if self.metrics.is_some() {
            self.invalidate();
        }
        self.metrics = Some(metrics);
        metrics
    }

    fn fit_terminal(
        &mut self,
        geometry: &GridGeometry,
        current: GridSize,
        metrics: Metrics,
        cx: &mut Context<Self>,
    ) {
        let wanted = GridSize::new(geometry.columns as u16, geometry.rows as u16).with_cell_size(
            f32::from(metrics.cell_width).round() as u16,
            f32::from(metrics.line_height).round() as u16,
        );
        if current != wanted {
            self.terminal
                .update(cx, |terminal, cx| terminal.resize(wanted, cx));
        }
    }

    /// Shapes the text of rows rebuilt since they were last shaped; returns how many segments it shaped.
    fn shape_rows(&mut self, metrics: Metrics, window: &mut Window) -> usize {
        self.shaped
            .resize_with(self.frame.rows.len(), ShapedRow::default);
        let mut shaped_segments = 0;
        for (index, row) in self.frame.rows.iter().enumerate() {
            let built_at = self.frame.built_at[index];
            if self.shaped[index].built_at == built_at && built_at != 0 {
                continue;
            }
            let lines = row
                .segments
                .iter()
                .map(|segment| {
                    (
                        segment.column,
                        shape_segment(segment, &self.style, metrics, window),
                    )
                })
                .collect::<Vec<_>>();
            shaped_segments += lines.len();
            self.shaped[index] = ShapedRow { built_at, lines };
        }
        shaped_segments
    }

    fn paint_list(&self, geometry: &GridGeometry, spans: &[SelectionSpan]) -> GridPaint {
        let mut quads = self.background_quads(geometry);
        let palette = &self.style.palette;
        let tint = color(palette.selection, palette.selection_alpha);
        quads.extend(spans.iter().map(|span| {
            let bounds = geometry.cell_bounds(span.line, span.start, span.end - span.start);
            (bounds, tint)
        }));
        let lines = self
            .shaped
            .iter()
            .enumerate()
            .flat_map(|(line, row)| {
                row.lines.iter().map(move |(column, shaped)| {
                    (
                        geometry.cell_bounds(line, *column, 1).origin,
                        shaped.clone(),
                    )
                })
            })
            .collect();
        GridPaint {
            quads,
            lines,
            line_height: geometry.line_height,
        }
    }

    /// Cell backgrounds, then block elements, row by row.
    fn background_quads(&self, geometry: &GridGeometry) -> Vec<(Bounds<Pixels>, Hsla)> {
        let mut quads = Vec::new();
        for (line, row) in self.frame.rows.iter().enumerate() {
            for span in &row.backgrounds {
                let bounds = geometry.cell_bounds(line, span.column, span.width);
                quads.push((bounds, color(span.color, 1.0)));
            }
        }
        for (line, row) in self.frame.rows.iter().enumerate() {
            for block in &row.blocks {
                quads.extend(block_quads(geometry, line, block));
            }
        }
        quads
    }
}

impl Render for GridView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        GridElement { view: cx.entity() }
    }
}

/// Measures the cell for `style`: the advance of `m` and the line height, snapped to device pixels so cells
/// tile without seams.
fn measure(style: &TerminalStyle, scale_factor: f32, cx: &App) -> Metrics {
    let text_system = cx.text_system();
    let font_size = style.font_size_px();
    let font_id = text_system.resolve_font(&style.font());
    let advance = match text_system.advance(font_id, font_size, 'm') {
        Ok(advance) => advance.width,
        Err(error) => {
            log::warn!("terminal font {} has no 'm': {error}", style.font_family);
            font_size * 0.6
        }
    };
    let snap =
        |value: Pixels| px((f32::from(value) * scale_factor).round().max(1.0) / scale_factor);
    Metrics {
        font_size,
        cell_width: snap(advance),
        line_height: snap(font_size * style.line_height),
        scale_factor,
    }
}

/// The largest grid that fits `bounds`, anchored at its top left corner snapped to a device pixel. The counts
/// may be below the minimum grid; the caller decides what to do with a layout that small.
fn fit(bounds: Bounds<Pixels>, metrics: Metrics, scale_factor: f32) -> GridGeometry {
    let snap = |value: Pixels| px((f32::from(value) * scale_factor).floor() / scale_factor);
    let columns = (f32::from(bounds.size.width) / f32::from(metrics.cell_width)).floor() as usize;
    let rows = (f32::from(bounds.size.height) / f32::from(metrics.line_height)).floor() as usize;
    GridGeometry {
        origin: point(snap(bounds.origin.x), snap(bounds.origin.y)),
        cell_width: metrics.cell_width,
        line_height: metrics.line_height,
        columns: columns.min(usize::from(u16::MAX)),
        rows: rows.min(usize::from(u16::MAX)),
    }
}

fn shape_segment(
    segment: &TextSegment,
    style: &TerminalStyle,
    metrics: Metrics,
    window: &mut Window,
) -> ShapedLine {
    let runs: Vec<TextRun> = segment
        .runs
        .iter()
        .map(|run| text_run(run.len, &run.style, style))
        .collect();
    let mut line = window.text_system().shape_line(
        segment.text.clone().into(),
        metrics.font_size,
        &runs,
        None,
    );
    let placed = place_on_cells(&line, &segment.bases, metrics.cell_width, segment.cells);
    *line = Arc::new(placed);
    line
}

/// Moves every glyph onto its cell: a base character exactly at its cell's left edge, a combining mark at its
/// shaped offset from its base. GPUI's own forced width lets glyphs drift up to a pixel before snapping them,
/// which breaks box drawing when the cell is rounded to whole device pixels.
fn place_on_cells(
    layout: &LineLayout,
    bases: &[usize],
    cell_width: Pixels,
    cells: usize,
) -> LineLayout {
    let cell_of = |index: usize| {
        bases
            .partition_point(|&start| start <= index)
            .saturating_sub(1)
    };
    let base_x = base_positions(layout, bases.len(), cell_of);
    let place = |glyph: &ShapedGlyph| {
        let cell = cell_of(glyph.index);
        let offset = glyph.position.x - base_x[cell].unwrap_or(glyph.position.x);
        ShapedGlyph {
            position: point(cell_width * cell as f32 + offset, glyph.position.y),
            ..glyph.clone()
        }
    };
    let runs = layout
        .runs
        .iter()
        .map(|run| ShapedRun {
            font_id: run.font_id,
            glyphs: run.glyphs.iter().map(place).collect(),
        })
        .collect();
    LineLayout {
        width: cell_width * cells as f32,
        runs,
        ..*layout
    }
}

/// Where the shaper put the first glyph of each cell.
fn base_positions(
    layout: &LineLayout,
    cells: usize,
    cell_of: impl Fn(usize) -> usize,
) -> Vec<Option<Pixels>> {
    let mut base_x = vec![None; cells.max(1)];
    for glyph in layout.runs.iter().flat_map(|run| &run.glyphs) {
        base_x[cell_of(glyph.index)].get_or_insert(glyph.position.x);
    }
    base_x
}

fn text_run(len: usize, cell: &CellStyle, style: &TerminalStyle) -> TextRun {
    let alpha = if cell.faint { FAINT_OPACITY } else { 1.0 };
    let foreground = color(cell.fg, alpha);
    let underline = cell
        .underline
        .map(|(kind, underline_color)| UnderlineStyle {
            thickness: px(DECORATION_THICKNESS),
            color: Some(color(underline_color, alpha)),
            wavy: kind == Underline::Curly,
        });
    let strikethrough = cell.strikeout.then(|| StrikethroughStyle {
        thickness: px(DECORATION_THICKNESS),
        color: Some(foreground),
    });
    TextRun {
        len,
        font: style.font_for(cell.bold, cell.italic),
        color: foreground,
        background_color: None,
        underline,
        strikethrough,
    }
}

/// The rectangles of a block element cell, in window coordinates.
fn block_quads(
    geometry: &GridGeometry,
    line: usize,
    block: &BlockCell,
) -> Vec<(Bounds<Pixels>, Hsla)> {
    let cell = geometry.cell_bounds(line, block.column, 1);
    let faint = if block.faint { FAINT_OPACITY } else { 1.0 };
    block_rects(block.character)
        .into_iter()
        .map(|rect| {
            let origin = point(
                cell.origin.x + cell.size.width * rect.left,
                cell.origin.y + cell.size.height * rect.top,
            );
            let extent = size(
                cell.size.width * (rect.right - rect.left),
                cell.size.height * (rect.bottom - rect.top),
            );
            (
                Bounds::new(origin, extent),
                color(block.fg, rect.alpha * faint),
            )
        })
        .collect()
}

/// `0xRRGGBB` at `alpha` as a GPUI colour.
pub(crate) fn color(hex: u32, alpha: f32) -> Hsla {
    let mut color: Hsla = rgb(hex).into();
    color.a = alpha;
    color
}

/// Quads and text the grid paints this frame.
pub(crate) struct GridPaint {
    quads: Vec<(Bounds<Pixels>, Hsla)>,
    lines: Vec<(Point<Pixels>, ShapedLine)>,
    line_height: Pixels,
}

/// The element that lays out and paints the grid of a [`GridView`].
struct GridElement {
    view: Entity<GridView>,
}

impl IntoElement for GridElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for GridElement {
    type RequestLayoutState = ();
    type PrepaintState = GridPaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.view
            .update(cx, |view, cx| view.prepare(bounds, window, cx))
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        paint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            for (quad_bounds, quad_color) in &paint.quads {
                window.paint_quad(fill(*quad_bounds, *quad_color));
            }
            for (origin, line) in &paint.lines {
                if let Err(error) = line.paint(
                    *origin,
                    paint.line_height,
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                ) {
                    log::warn!("terminal text did not paint: {error}");
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{px, FontId, GlyphId};

    fn glyph(index: usize, x: f32) -> ShapedGlyph {
        ShapedGlyph {
            id: GlyphId(0),
            position: point(px(x), px(0.)),
            index,
            is_emoji: false,
        }
    }

    fn layout(glyphs: Vec<ShapedGlyph>, len: usize) -> LineLayout {
        LineLayout {
            font_size: px(13.),
            width: px(0.),
            ascent: px(10.),
            descent: px(3.),
            runs: vec![ShapedRun {
                font_id: FontId(0),
                glyphs,
            }],
            len,
        }
    }

    fn xs(layout: &LineLayout) -> Vec<f32> {
        layout.runs[0]
            .glyphs
            .iter()
            .map(|glyph| f32::from(glyph.position.x))
            .collect()
    }

    #[test]
    fn glyphs_land_exactly_on_rounded_cells_without_drift() {
        // Seven box-drawing glyphs shaped 7.8 px apart, cells rounded to 8 px (GPUI would keep the first six
        // where they are and jump at the seventh).
        let shaped = layout((0..7).map(|i| glyph(i * 3, i as f32 * 7.8)).collect(), 21);
        let bases: Vec<usize> = (0..7).map(|i| i * 3).collect();
        let placed = place_on_cells(&shaped, &bases, px(8.), 7);
        assert_eq!(xs(&placed), vec![0., 8., 16., 24., 32., 40., 48.]);
        assert_eq!(placed.width, px(56.));
    }

    #[test]
    fn combining_marks_keep_their_offset_from_the_base() {
        // "e" + U+0301 + "x": the mark shaped 1.5 px right of its base stays with it.
        let shaped = layout(vec![glyph(0, 0.), glyph(1, 1.5), glyph(3, 7.8)], 4);
        let placed = place_on_cells(&shaped, &[0, 3], px(8.), 2);
        assert_eq!(xs(&placed), vec![0., 1.5, 8.]);
    }
}
