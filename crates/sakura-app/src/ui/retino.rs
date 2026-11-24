//! The retino behind the sidebar: the brand's 8x8 Bayer dither, drawn on the window's canvas under the
//! sidebar column and along the bottom row. The dots take a muted version of the chosen accent. A
//! noise field drifts up through them and the dots thin out to the right along the bottom row.
//! When the sidebar hides, the bottom row slides out with it.
//!
//! The cells come from [`lit_cells_into`], a pure function of the window's geometry and of time (the
//! prototype's `drawGrain`, constant for constant); [`Retino`] paints them under everything else. It redraws
//! 12 times a second while the window is visible and motion is allowed.

use crate::prefs::Preferences;
use crate::theme::{self, engine_bar, page_bar, Theme};
use gpui::{
    canvas, fill, point, px, size, Bounds, Context, Hsla, IntoElement, Pixels, Render, Styled as _,
    Task, Window,
};
use std::cell::{Cell as SharedCell, RefCell};
use std::rc::Rc;
use std::time::Instant;

/// The grid the dots stand on (a dot is [`theme::RETINO_DOT_SIZE`], one pixel of gap between two).
const CELL: f64 = 4.;
/// The 8x8 Bayer matrix of the dither.
const BAYER: [u8; 64] = [
    0, 32, 8, 40, 2, 34, 10, 42, 48, 16, 56, 24, 50, 18, 58, 26, 12, 44, 4, 36, 14, 46, 6, 38, 60,
    28, 52, 20, 62, 30, 54, 22, 3, 35, 11, 43, 1, 33, 9, 41, 51, 19, 59, 27, 49, 17, 57, 25, 15,
    47, 7, 39, 13, 45, 5, 37, 63, 31, 55, 23, 61, 29, 53, 21,
];
/// How fast the noise slides up, in pixels a second, and how many pixels one noise cell spans.
const DRIFT_SPEED: f64 = 7.;
const NOISE_SPAN: f64 = 52.;
/// Where the dots begin to rise (a share of the window's height) and how the rise bends.
const RISE_START: f64 = 0.45;
const RISE_RANGE: f64 = 0.55;
const RISE_BEND: f64 = 1.6;
const RISE_STRENGTH: f64 = 0.55;
/// The noise's strength, and the share of the height over which it comes in from 0.32.
const NOISE_STRENGTH: f64 = 0.2;
const NOISE_START: f64 = 0.32;
const NOISE_END: f64 = 0.62;
/// How far the noise can push a dot's value either way, before the ramp and the strip's thinning.
const NOISE_REACH: f64 = NOISE_STRENGTH / 2.;
/// How much the strip's thinning is multiplied by before it limits the noise.
const NOISE_ACROSS_GAIN: f64 = 1.4;
/// The sidebar column's look: its dots thin a little towards its right edge.
const COLUMN_SPAN: f64 = 244.;
const COLUMN_INK: f64 = 236.;
const COLUMN_THINNING: f64 = 0.3;
/// The bottom row, measured from the window's left edge whatever the sidebar does: from the column's own
/// look at `COLUMN_INK` the density eases over `JOIN_END - COLUMN_INK` (one Bayer tile) into the strip's
/// own, which thins out and is gone at `DOTS_END`.
const JOIN_END: f64 = 268.;
const DOTS_END: f64 = 600.;
const STRIP_STRENGTH: f64 = 0.85;
/// The keep-out around the page panel: the panel grown by this much on every side, its corners rounded by
/// the panel's own radius plus this much, so the field's edge runs parallel to the panel, round its
/// corners too.
const PANEL_GROWTH: f64 = 12.;
/// The first ring of cells outside the keep-out, straight sides and corners alike, draws the field's
/// edge: there a cell's value is lifted to this many times the field's strength...
const EDGE_GAIN: f64 = 2.;
/// Half the Bayer range selects a checkerboard, avoiding adjacent lit cells along the edge.
/// The lift holds for one corner radius past the curve and tapers over the next.
const EDGE_CEILING: f64 = 0.5;
/// Anchor the Bayer phase to the panel's bottom so the corner pattern survives window resizing.
const BOTTOM_EDGE_PATTERN_ROW: i64 = 3;
/// How many rows the Bayer matrix repeats over.
const PATTERN_ROWS: i64 = 8;
/// How far a dot's square reaches from its corner (its side is one pixel less than the grid).
const DOT_EXTENT: f64 = 3.;

/// Where the retino is drawn, in the window's points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Geometry {
    pub width: f32,
    pub height: f32,
    /// The sidebar column's width: the dots fill it down to the bottom row.
    pub column: f32,
    /// The bottom row's height: the dots run along it under the page.
    pub strip: f32,
    /// The page panel's left edge: nothing above the bottom row is drawn from here on, over the panel.
    pub page_left: f32,
    /// The panel's top edge, and its distance from the window's right edge.
    pub panel_top: f32,
    pub panel_inset: f32,
    /// How far the bottom row's dots have moved to the left with the sidebar: none while it is shown, the
    /// whole length of the row ([`STRIP_LENGTH`]) when it is hidden, when no dot is left in the row.
    pub strip_shift: f32,
}

/// The length of the bottom row's dots from the window's left edge, which is as far as they travel to
/// leave with the sidebar.
pub const STRIP_LENGTH: f64 = DOTS_END;

/// How far the bottom row's dots are from their place when `share` of the sidebar is in view (from 0, hidden,
/// to 1, shown): they leave and come back with it, on the same ease.
pub fn strip_shift(share: f32) -> f32 {
    STRIP_LENGTH as f32 * (1. - share.clamp(0., 1.))
}

/// The top-left corner of one lit dot.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cell {
    pub x: f32,
    pub y: f32,
}

/// Every lit dot of the window `geometry` at `t_seconds`.
#[cfg(test)]
pub fn lit_cells(geometry: &Geometry, t_seconds: f64) -> Vec<Cell> {
    let mut cells = Vec::new();
    lit_cells_into(geometry, t_seconds, &mut cells);
    cells
}

/// [`lit_cells`] into `cells`, which is emptied first, so a caller that draws every frame reuses one buffer.
pub fn lit_cells_into(geometry: &Geometry, t_seconds: f64, cells: &mut Vec<Cell>) {
    cells.clear();
    let drift = t_seconds * DRIFT_SPEED;
    let rows = (f64::from(geometry.height) / CELL).ceil() as usize;
    let strip_top = f64::from(geometry.height - geometry.strip);
    // The rows stand on the panel's bottom edge, and the pattern is read from the row that edge has in the
    // prototype's window.
    let top_phase = strip_top.rem_euclid(CELL);
    let bottom_row = ((strip_top - top_phase) / CELL).round() as i64;
    let pattern_shift = (BOTTOM_EDGE_PATTERN_ROW - bottom_row).rem_euclid(PATTERN_ROWS) as usize;
    // The row's dots move in whole cells, so the dither keeps its grid while they leave.
    let moved = (f64::from(geometry.strip_shift) / CELL).round() as usize;
    for row in 0..rows {
        let y = top_phase + row as f64 * CELL;
        let (first, reach) = if y >= strip_top {
            (
                moved,
                f64::from(geometry.width + geometry.strip_shift).min(DOTS_END),
            )
        } else {
            (0, f64::from(geometry.column.min(geometry.page_left)))
        };
        let line = Line::new(geometry, (row + pattern_shift, y), drift);
        for column in first..(reach / CELL).ceil() as usize {
            let beyond = if y >= strip_top { moved } else { 0 };
            cells.extend(lit_cell(geometry, &line, (column, beyond)));
        }
    }
}

/// What one row of the grid shares between its cells.
struct Line {
    /// The row of the dither's pattern, not of the window.
    row: usize,
    y: f64,
    drift: f64,
    /// The dots' own rise at this height, before the sidebar column or the strip scales it.
    rise: f64,
    /// How much noise comes in at this height.
    ramp: f64,
}

impl Line {
    fn new(geometry: &Geometry, (row, y): (usize, f64), drift: f64) -> Self {
        let share = y / f64::from(geometry.height);
        let rise = ((share - RISE_START) / RISE_RANGE).max(0.).powf(RISE_BEND) * RISE_STRENGTH;
        Self {
            row,
            y,
            drift,
            rise,
            ramp: smooth(NOISE_START, NOISE_END, share),
        }
    }
}

/// The dot of `column` of the pattern, drawn `moved` cells to the left of it; none where the pattern has no
/// dot or the panel's keep-out is.
fn lit_cell(geometry: &Geometry, line: &Line, (column, moved): (usize, usize)) -> Option<Cell> {
    let x = column as f64 * CELL;
    let drawn_x = (column - moved) as f64 * CELL;
    let across = reach(x);
    let base = line.rise * across;
    let threshold = threshold(column, line.row);
    let lifted = match clearance(geometry, drawn_x, line.y) {
        Clearance::Inside => return None,
        Clearance::Edge(weight) if threshold < EDGE_CEILING => {
            (base * EDGE_GAIN).min(EDGE_CEILING) * weight
        }
        Clearance::Edge(_) | Clearance::Clear => 0.,
    };
    let noise_reach = NOISE_REACH * line.ramp * (across * NOISE_ACROSS_GAIN).min(1.);
    if (base + noise_reach).max(lifted) <= threshold {
        return None;
    }
    let mist = noise2(x / NOISE_SPAN, (line.y + line.drift) / NOISE_SPAN) - 0.5;
    let value = (base + mist * NOISE_STRENGTH * line.ramp * (across * NOISE_ACROSS_GAIN).min(1.))
        .max(lifted);
    if value <= threshold {
        return None;
    }
    Some(Cell {
        x: drawn_x as f32,
        y: line.y as f32,
    })
}

/// Where a dot stands against the field's keep-out shape round the page panel.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Clearance {
    /// Some part of it lies inside: it is not drawn.
    Inside,
    /// It is in the first ring of cells outside the keep-out, along a side or round a corner, with how
    /// much of the edge's lift it takes: all of it round the corner and near it, none far along a side.
    Edge(f64),
    /// Anywhere else outside.
    Clear,
}

/// Where the dot whose top-left corner is (`x`, `y`) stands against the field's keep-out shape: the page
/// panel grown by [`PANEL_GROWTH`] on every side, its corners rounded by the panel's radius plus that
/// growth. That keeps the same gap on straight edges and round the corners. The shape is an inner
/// rectangle plus a disc, so the nearest point of the dot's square to it is found per axis.
fn clearance(geometry: &Geometry, x: f64, y: f64) -> Clearance {
    let radius = PANEL_GROWTH + f64::from(f32::from(theme::RADIUS_SURFACE));
    let shrink = radius - PANEL_GROWTH;
    let inner_x = (
        f64::from(geometry.page_left) + shrink,
        f64::from(geometry.width - geometry.panel_inset) - shrink,
    );
    let inner_y = (
        f64::from(geometry.panel_top) + shrink,
        f64::from(geometry.height - geometry.strip) - shrink,
    );
    let beyond_x = gap_between((x, x + DOT_EXTENT), inner_x);
    let beyond_y = gap_between((y, y + DOT_EXTENT), inner_y);
    let squared = beyond_x * beyond_x + beyond_y * beyond_y;
    let edge = radius + CELL;
    if squared < radius * radius {
        Clearance::Inside
    } else if squared < edge * edge {
        let along_x = run_along_side(beyond_x, (x, x + DOT_EXTENT), inner_x);
        let along_y = run_along_side(beyond_y, (y, y + DOT_EXTENT), inner_y);
        Clearance::Edge(1. - smooth(radius, 2. * radius, along_x.max(along_y)))
    } else {
        Clearance::Clear
    }
}

/// How far the dot's `span` lies inside the keep-out's straight side `side` on one axis, from the nearer
/// end of that side, where a corner's curve starts; zero when the dot is beyond the side (`beyond` > 0).
fn run_along_side(beyond: f64, span: (f64, f64), side: (f64, f64)) -> f64 {
    if beyond > 0. {
        return 0.;
    }
    (span.0 - side.0).min(side.1 - span.1).max(0.)
}

/// The room between two spans on one axis; zero when they overlap.
fn gap_between(first: (f64, f64), second: (f64, f64)) -> f64 {
    (first.0 - second.1).max(second.0 - first.1).max(0.)
}

/// Dot density eases from the sidebar into the bottom row over one Bayer tile to hide the seam.
pub(super) fn reach(x: f64) -> f64 {
    let column = 1. - x / COLUMN_SPAN * COLUMN_THINNING;
    if x < COLUMN_INK {
        return column;
    }
    let edge = 1. - COLUMN_INK / COLUMN_SPAN * COLUMN_THINNING;
    let join = smooth(COLUMN_INK, JOIN_END, x);
    (column + (edge * STRIP_STRENGTH - column) * join) * (1. - smooth(COLUMN_INK, DOTS_END, x))
}

fn smooth(from: f64, to: f64, x: f64) -> f64 {
    let step = ((x - from) / (to - from)).clamp(0., 1.);
    step * step * (3. - 2. * step)
}

fn threshold(column: usize, row: usize) -> f64 {
    (f64::from(BAYER[(row % 8) * 8 + column % 8]) + 0.5) / 64.
}

/// The integer hash behind the noise, as a number from 0 to 1.
pub(super) fn hash2(ix: i32, iy: i32) -> f64 {
    let mixed = (ix as u32)
        .wrapping_mul(374_761_393)
        .wrapping_add((iy as u32).wrapping_mul(668_265_263));
    let mixed = (mixed ^ (mixed >> 13)).wrapping_mul(1_274_126_177);
    f64::from(mixed ^ (mixed >> 16)) / f64::from(u32::MAX)
}

/// Smooth value noise over [`hash2`].
pub(super) fn noise2(x: f64, y: f64) -> f64 {
    let (ix, iy) = (x.floor(), y.floor());
    let (fx, fy) = (x - ix, y - iy);
    let (fx, fy) = (fx * fx * (3. - 2. * fx), fy * fy * (3. - 2. * fy));
    let (ix, iy) = (ix as i32, iy as i32);
    let (a, b) = (hash2(ix, iy), hash2(ix + 1, iy));
    let (c, d) = (hash2(ix, iy + 1), hash2(ix + 1, iy + 1));
    a + (b - a) * fx + (c - a) * fy + (a - b - c + d) * fx * fy
}

#[derive(Default)]
struct CellCache {
    frame: Option<(Geometry, f64)>,
    cells: Vec<Cell>,
}

impl CellCache {
    fn cells_for(&mut self, frame: (Geometry, f64)) -> &[Cell] {
        if self.frame != Some(frame) {
            let (geometry, seconds) = frame;
            lit_cells_into(&geometry, seconds, &mut self.cells);
            self.frame = Some(frame);
        }
        &self.cells
    }
}

/// The view that paints the retino, under every other view of the window.
pub struct Retino {
    inputs: Rc<SharedCell<(f32, f32)>>,
    sidebar_mask: SidebarMask,
    created_at: Instant,
    seconds: f64,
    cells: Rc<RefCell<CellCache>>,
    is_shown: bool,
    is_animated: bool,
    /// The timer that redraws it; none while it is still.
    tick: Option<Task<()>>,
    _observers: Vec<gpui::Subscription>,
}

impl Retino {
    /// A retino for `window`, still until its first frame says motion is allowed.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let visibility = cx.observe_window_visibility(window, |retino, visibility, _, cx| {
            retino.is_shown = visibility.is_visible();
            retino.sync_tick(cx);
        });
        let preferences = Preferences::observe(cx, |retino, cx| {
            retino.is_animated = Preferences::is_retino_animated(cx);
            retino.sync_tick(cx);
        });
        let theme = cx.observe_global::<Theme>(|_, cx| cx.notify());
        Self {
            inputs: Rc::default(),
            sidebar_mask: SidebarMask::default(),
            created_at: cx.background_executor().now(),
            seconds: 0.,
            cells: Rc::default(),
            is_shown: window.is_visible(),
            is_animated: Preferences::is_retino_animated(cx),
            tick: None,
            _observers: vec![visibility, preferences, theme],
        }
    }

    /// The caller updates the geometry during its own draw, so no extra frame is needed.
    pub fn set_inputs(&self, inputs: (f32, f32)) {
        self.inputs.set(inputs);
    }

    pub(super) fn sidebar_mask(&self) -> SidebarMask {
        self.sidebar_mask.clone()
    }

    /// How far the bottom row's dots were last told to be from their place.
    #[cfg(test)]
    pub fn strip_shift(&self) -> f32 {
        self.inputs.get().1
    }

    /// Whether its timer runs.
    #[cfg(test)]
    pub fn is_ticking(&self) -> bool {
        self.tick.is_some()
    }

    /// Where the page panel's left edge is, as it was last told.
    #[cfg(test)]
    pub fn page_left(&self) -> f32 {
        self.inputs.get().0
    }

    fn is_drifting_now(&self, cx: &gpui::App) -> bool {
        self.is_shown && self.is_animated && !cx.reduce_motion()
    }

    fn sync_tick(&mut self, cx: &mut Context<Self>) {
        if !self.is_drifting_now(cx) {
            self.tick = None;
        } else if self.tick.is_none() {
            self.tick = Some(cx.spawn(tick));
        }
    }

    /// One beat of the timer: draws again, or ends the timer when the dots should be still.
    fn beat(&mut self, cx: &mut Context<Self>) -> bool {
        let is_kept = self.is_drifting_now(cx);
        if is_kept {
            cx.notify();
        } else {
            self.tick = None;
        }
        is_kept
    }
}

async fn tick(retino: gpui::WeakEntity<Retino>, cx: &mut gpui::AsyncApp) {
    loop {
        cx.background_executor().timer(theme::RETINO_FRAME).await;
        if !matches!(retino.update(cx, Retino::beat), Ok(true)) {
            return;
        }
    }
}

impl Render for Retino {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::snapshot::count_render("retino");
        self.sync_tick(cx);
        if self.tick.is_some() {
            self.seconds = (cx.background_executor().now() - self.created_at).as_secs_f64();
        }
        let seconds = if cx.reduce_motion() { 0. } else { self.seconds };
        let inputs = self.inputs.clone();
        let colour = Theme::palette(cx).retino_dot;
        let cells = self.cells.clone();
        let sidebar_mask = self.sidebar_mask.clone();
        canvas(
            |_, _, _| (),
            move |bounds, (), window, _| {
                // The foreground can update the panel geometry during prepaint.
                paint(
                    bounds,
                    (inputs.get(), seconds),
                    &cells,
                    colour,
                    &sidebar_mask,
                    window,
                );
            },
        )
        .absolute()
        .size_full()
    }
}

#[derive(Clone, Default)]
pub(super) struct SidebarMask(Rc<SharedCell<Bounds<Pixels>>>);

impl SidebarMask {
    pub(super) fn measure(&self) -> impl IntoElement {
        let mask = self.clone();
        canvas(
            move |bounds, window, _| {
                let visible = bounds.intersect(&window.content_mask().bounds);
                mask.0.set(Bounds::from_corners(
                    point(Pixels::ZERO, visible.top()),
                    visible.bottom_right(),
                ));
            },
            |_, (), _, _| (),
        )
        .absolute()
        .inset_0()
    }
}

fn is_behind_status_icon(dot: Bounds<Pixels>, window: Bounds<Pixels>) -> bool {
    let half_button = engine_bar::ICON_BUTTON / 2.;
    let first_x = window.left() + engine_bar::BAR_PADDING_X + half_button;
    let stride = engine_bar::ICON_BUTTON + engine_bar::BAR_GAP;
    let y = window.bottom() - engine_bar::BAR_HEIGHT / 2.;
    let radius = f32::from(engine_bar::ICON_SIZE) * std::f32::consts::FRAC_1_SQRT_2 + 3.;
    [first_x, first_x + stride].into_iter().any(|x| {
        let dx = f32::from((dot.left() - x).max(x - dot.right()).max(Pixels::ZERO));
        let dy = f32::from((dot.top() - y).max(y - dot.bottom()).max(Pixels::ZERO));
        dx * dx + dy * dy <= radius * radius
    })
}

/// Paints the lit dots of the window `bounds` as squares.
fn paint(
    bounds: Bounds<Pixels>,
    ((page_left, strip_shift), seconds): ((f32, f32), f64),
    cells: &RefCell<CellCache>,
    colour: Hsla,
    sidebar_mask: &SidebarMask,
    window: &mut Window,
) {
    let geometry = Geometry {
        width: f32::from(bounds.size.width),
        height: f32::from(bounds.size.height),
        column: f32::from(theme::SIDEBAR_WIDTH),
        strip: f32::from(engine_bar::BAR_HEIGHT),
        page_left,
        panel_top: f32::from(page_bar::BAR_HEIGHT),
        panel_inset: f32::from(theme::PANEL_INSET),
        strip_shift,
    };
    let mut cells = cells.borrow_mut();
    // The grid leaves a gap between dots, so one draw order covers the entire layer.
    window.paint_layer(bounds, |window| {
        let sidebar = sidebar_mask.0.get();
        for cell in cells.cells_for((geometry, seconds)) {
            let origin = bounds.origin + point(px(cell.x), px(cell.y));
            let dot = Bounds::new(origin, size(theme::RETINO_DOT_SIZE, theme::RETINO_DOT_SIZE));
            if cell.x >= CELL as f32
                && dot.bottom() <= bounds.bottom()
                && !dot.intersects(&sidebar)
                && !is_behind_status_icon(dot, bounds)
            {
                window.paint_quad(fill(dot, colour));
            }
        }
    });
}
