//! The lanes of one row of the graph, painted by hand: the lines of the edges that cross the row and
//! the commit's dot. A lane is one icon wide; a line is the stroke of the window's marks; the dot's
//! radius is the tightest gap of the grid. An edge between two lanes leaves the dot level and turns
//! into the other lane with a corner of half a lane, so a branch leaving the base and a merge coming
//! back read as GitKraken's do.

use crate::theme::{self, Palette};
use gpui::{fill, point, size, Bounds, Hsla, PathBuilder, Pixels, Point, Window};
use sakura_core::graph::{Row, Stroke};

/// The colours of a row's lanes.
#[derive(Debug, Clone, Copy)]
pub struct LaneColours {
    pub line: Hsla,
    pub dot: Hsla,
}

impl LaneColours {
    /// Lines are quiet; the dot of a task's commit is in the action colour, the base's stays quiet.
    pub fn of(is_task_commit: bool, palette: &Palette) -> Self {
        Self {
            line: palette.text_tertiary,
            dot: match is_task_commit {
                true => palette.primary,
                false => palette.text_tertiary,
            },
        }
    }
}

/// How wide the lanes of a graph with `columns` columns are.
pub fn width(columns: usize) -> Pixels {
    theme::ICON_SIZE * columns as f32
}

/// The centre line of `column`, from the lanes' left edge.
fn centre(bounds: Bounds<Pixels>, column: usize) -> Pixels {
    bounds.left() + theme::ICON_SIZE * column as f32 + theme::ICON_SIZE / 2.
}

/// Paints `row` in `bounds`, whose height is the row's. With `has_mark` a state mark stands in the
/// dot's place: no dot is painted and the lines end at the mark's edge.
pub fn paint(
    bounds: Bounds<Pixels>,
    row: &Row,
    colours: LaneColours,
    has_mark: bool,
    window: &mut Window,
) {
    let dot = point(centre(bounds, row.column), bounds.center().y);
    let clear = match has_mark {
        true => theme::MARK_SIZE / 2.,
        false => Pixels::ZERO,
    };
    for stroke in &row.strokes {
        match *stroke {
            Stroke::Through(column) => {
                let x = centre(bounds, column);
                line(x, bounds.top(), bounds.bottom(), colours.line, window);
            }
            Stroke::FromAbove => line(dot.x, bounds.top(), dot.y - clear, colours.line, window),
            Stroke::ToBelow => line(dot.x, dot.y + clear, bounds.bottom(), colours.line, window),
            Stroke::BendUp(column) => {
                let end = point(centre(bounds, column), bounds.top());
                bend(dot, clear, end, colours.line, window);
            }
            Stroke::BendDown(column) => {
                let end = point(centre(bounds, column), bounds.bottom());
                bend(dot, clear, end, colours.line, window);
            }
        }
    }
    if !has_mark {
        let radius = theme::GAP_TIGHT;
        let disc = Bounds::new(
            point(dot.x - radius, dot.y - radius),
            size(radius * 2., radius * 2.),
        );
        window.paint_quad(fill(disc, colours.dot).corner_radii(radius));
    }
}

/// A vertical line at `x` from `top` to `bottom`.
fn line(x: Pixels, top: Pixels, bottom: Pixels, colour: Hsla, window: &mut Window) {
    let width = theme::GLYPH_STROKE;
    let bounds = Bounds::new(point(x - width / 2., top), size(width, bottom - top));
    window.paint_quad(fill(bounds, colour));
}

/// From the dot to `end`, on the edge of the row in another lane: level out of the dot (from `clear`
/// off its centre), a corner of half a lane, then upright into the lane.
fn bend(dot: Point<Pixels>, clear: Pixels, end: Point<Pixels>, colour: Hsla, window: &mut Window) {
    let corner = theme::ICON_SIZE / 2.;
    let towards = |from: Pixels, to: Pixels, by: Pixels| if to > from { by } else { -by };
    let start = point(dot.x + towards(dot.x, end.x, clear), dot.y);
    let turn = point(end.x - towards(dot.x, end.x, corner), dot.y);
    let upright = point(end.x, dot.y + towards(dot.y, end.y, corner));
    let mut path = PathBuilder::stroke(theme::GLYPH_STROKE);
    path.move_to(start);
    path.line_to(turn);
    path.curve_to(upright, point(end.x, dot.y));
    path.line_to(end);
    match path.build() {
        Ok(path) => window.paint_path(path, colour),
        Err(error) => eprintln!("sakura: cannot draw an edge of the graph: {error}"),
    }
}
