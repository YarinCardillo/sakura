//! The marks of the conversation, drawn by hand on the layout grid's two sizes: what a tool call does and
//! the marks of the send and stop button fill the 16 px icon box; where a call stands, a plan step's state
//! and the chevron of a row that opens are the 12 px marks of the task state glyphs, centred in a 16 px
//! column so every row's words start on one line. A state is always a shape and a tone beside its word,
//! never colour alone, and nothing here moves.

use crate::theme::thread as tokens;
use crate::theme::{self, Palette};
use gpui::{
    canvas, div, point, px, AnyElement, Bounds, Hsla, IntoElement, ParentElement, PathBuilder,
    Pixels, Point, Styled, Window,
};
use sakura_acp::model::{PlanStatus, ToolKind, ToolStatus};

/// Strokes in the glyph box.
type Strokes = &'static [&'static [(f32, f32)]];

/// The mark of what a tool call does.
pub fn kind(kind: ToolKind, colour: Hsla) -> AnyElement {
    icon(move |bounds, window| match kind {
        ToolKind::Read => strokes(bounds, window, colour, &tokens::KIND_READ),
        ToolKind::Edit => strokes(bounds, window, colour, &tokens::KIND_EDIT),
        ToolKind::Delete => strokes(bounds, window, colour, &tokens::KIND_DELETE),
        ToolKind::Move => strokes(bounds, window, colour, &tokens::KIND_MOVE),
        ToolKind::Search => {
            let (centre, radius) = tokens::KIND_SEARCH_LENS;
            ring_at(bounds, window, colour, centre, radius, false);
            strokes(bounds, window, colour, &tokens::KIND_SEARCH);
        }
        ToolKind::Execute => strokes(bounds, window, colour, &tokens::KIND_EXECUTE),
        ToolKind::Fetch => strokes(bounds, window, colour, &tokens::KIND_FETCH),
        ToolKind::SwitchMode => strokes(bounds, window, colour, &tokens::KIND_SWITCH),
        ToolKind::Think | ToolKind::Other => {
            let (centre, radius) = tokens::KIND_RING;
            ring_at(bounds, window, colour, centre, radius, false);
        }
    })
}

/// The colour of a tool call's state. A call that waits for the person is quiet here: the card under
/// it carries the one mark in the accent of the conversation.
pub fn status_colour(status: ToolStatus, palette: &Palette) -> Hsla {
    match status {
        ToolStatus::InProgress => palette.primary,
        ToolStatus::Failed => palette.danger,
        _ => palette.text_tertiary,
    }
}

/// The mark of where a tool call stands.
pub fn status(status: ToolStatus, palette: &Palette) -> AnyElement {
    let colour = status_colour(status, palette);
    mark(move |bounds, window| match status {
        ToolStatus::Pending => ring(bounds, window, colour, true),
        ToolStatus::InProgress => running(bounds, window, colour),
        ToolStatus::Completed => marked(bounds, window, colour, &[&theme::GLYPH_CHECK]),
        ToolStatus::Failed => marked(bounds, window, colour, &CROSS),
        ToolStatus::WaitingForConfirmation => disc(bounds, window, colour, theme::GLYPH_DOT_RADIUS),
        ToolStatus::Rejected => marked(bounds, window, colour, &[&tokens::STATE_SLASH]),
        ToolStatus::Canceled => marked(bounds, window, colour, &[&tokens::STATE_BAR]),
    })
}

/// The mark of a plan step: not started, in progress, done.
pub fn plan(status: PlanStatus, palette: &Palette) -> AnyElement {
    let (muted, running_colour, ink) = (palette.text_tertiary, palette.primary, palette.panel);
    mark(move |bounds, window| match status {
        PlanStatus::Pending => ring(bounds, window, muted, true),
        PlanStatus::InProgress => running(bounds, window, running_colour),
        PlanStatus::Completed => {
            disc(
                bounds,
                window,
                muted,
                theme::GLYPH_RING_RADIUS + f32::from(theme::GLYPH_STROKE) / 2.,
            );
            strokes(bounds, window, ink, &[&theme::GLYPH_CHECK]);
        }
    })
}

/// The filled dot of "needs attention".
pub fn needs_you(palette: &Palette) -> AnyElement {
    let colour = palette.attention;
    mark(move |bounds, window| disc(bounds, window, colour, theme::GLYPH_DOT_RADIUS))
}

/// The mark of a problem: a ring with an exclamation mark.
pub fn problem(colour: Hsla) -> AnyElement {
    mark(move |bounds, window| {
        marked(bounds, window, colour, &[&theme::GLYPH_BANG]);
        disc_at(
            bounds,
            window,
            colour,
            theme::GLYPH_BANG_DOT,
            theme::GLYPH_BANG_DOT_RADIUS,
        );
    })
}

/// The chevron of a row that opens: right when closed, down when open.
pub fn chevron(is_open: bool, colour: Hsla) -> AnyElement {
    mark(move |bounds, window| {
        let points: &[(f32, f32)] = match is_open {
            true => &theme::CHEVRON_OPEN,
            false => &theme::CHEVRON_CLOSED,
        };
        polyline(bounds, window, colour, points);
    })
}

/// The arrow of the send button.
pub fn send(colour: Hsla) -> AnyElement {
    icon(move |bounds, window| {
        polyline(bounds, window, colour, &tokens::SEND_SHAFT);
        polyline(bounds, window, colour, &tokens::SEND_HEAD);
    })
}

/// The square of the stop button.
pub fn stop(colour: Hsla) -> AnyElement {
    icon(move |bounds, window| {
        let mut builder = PathBuilder::fill();
        trace(&mut builder, bounds, &tokens::STOP_MARK);
        builder.close();
        paint(builder, window, colour);
    })
}

/// The two strokes of a cross, from the theme's four points.
const CROSS: [&[(f32, f32)]; 2] = [
    &[theme::GLYPH_CROSS[0], theme::GLYPH_CROSS[1]],
    &[theme::GLYPH_CROSS[2], theme::GLYPH_CROSS[3]],
];

/// A drawing that fills the 16 px icon box.
fn icon(paint: impl FnOnce(Bounds<Pixels>, &mut Window) + 'static) -> AnyElement {
    div()
        .size(theme::ICON_SIZE)
        .flex_none()
        .relative()
        .child(layer(paint))
        .into_any_element()
}

/// A drawing in the 12 px mark box, centred in a 16 px column.
fn mark(paint: impl FnOnce(Bounds<Pixels>, &mut Window) + 'static) -> AnyElement {
    let ink = div()
        .size(theme::HAND_MARK_SIZE)
        .flex_none()
        .relative()
        .child(layer(paint));
    div()
        .size(theme::ICON_SIZE)
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .child(ink)
        .into_any_element()
}

fn layer(paint: impl FnOnce(Bounds<Pixels>, &mut Window) + 'static) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, (), window, _| paint(bounds, window),
    )
    .absolute()
    .size_full()
}

/// A ring with a mark inside it.
fn marked(bounds: Bounds<Pixels>, window: &mut Window, colour: Hsla, mark: Strokes) {
    ring(bounds, window, colour, false);
    strokes(bounds, window, colour, mark);
}

/// A ring with its right half filled: work in progress, standing still.
fn running(bounds: Bounds<Pixels>, window: &mut Window, colour: Hsla) {
    ring(bounds, window, colour, false);
    let (centre, radius) = (theme::GLYPH_CENTRE, theme::GLYPH_INNER_RADIUS);
    let radii = point(px(radius), px(radius));
    let mut builder = PathBuilder::fill();
    builder.move_to(at(bounds, (centre, centre - radius)));
    builder.arc_to(
        radii,
        px(0.),
        false,
        true,
        at(bounds, (centre, centre + radius)),
    );
    builder.close();
    paint(builder, window, colour);
}

fn ring(bounds: Bounds<Pixels>, window: &mut Window, colour: Hsla, is_dashed: bool) {
    let centre = (theme::GLYPH_CENTRE, theme::GLYPH_CENTRE);
    ring_at(
        bounds,
        window,
        colour,
        centre,
        theme::GLYPH_RING_RADIUS,
        is_dashed,
    );
}

fn ring_at(
    bounds: Bounds<Pixels>,
    window: &mut Window,
    colour: Hsla,
    centre: (f32, f32),
    radius: f32,
    is_dashed: bool,
) {
    let mut builder = PathBuilder::stroke(theme::GLYPH_STROKE);
    if is_dashed {
        builder = builder.dash_array(&[px(theme::GLYPH_DASH), px(theme::GLYPH_DASH)]);
    }
    circle(&mut builder, bounds, centre, radius);
    paint(builder, window, colour);
}

fn disc(bounds: Bounds<Pixels>, window: &mut Window, colour: Hsla, radius: f32) {
    let centre = (theme::GLYPH_CENTRE, theme::GLYPH_CENTRE);
    disc_at(bounds, window, colour, centre, radius);
}

fn disc_at(
    bounds: Bounds<Pixels>,
    window: &mut Window,
    colour: Hsla,
    centre: (f32, f32),
    radius: f32,
) {
    let mut builder = PathBuilder::fill();
    circle(&mut builder, bounds, centre, radius);
    paint(builder, window, colour);
}

fn circle(builder: &mut PathBuilder, bounds: Bounds<Pixels>, (x, y): (f32, f32), radius: f32) {
    let radii = point(px(radius), px(radius));
    builder.move_to(at(bounds, (x, y - radius)));
    builder.arc_to(radii, px(0.), false, true, at(bounds, (x, y + radius)));
    builder.arc_to(radii, px(0.), false, true, at(bounds, (x, y - radius)));
    builder.close();
}

fn strokes(bounds: Bounds<Pixels>, window: &mut Window, colour: Hsla, all: Strokes) {
    for points in all {
        polyline(bounds, window, colour, points);
    }
}

fn polyline(bounds: Bounds<Pixels>, window: &mut Window, colour: Hsla, points: &[(f32, f32)]) {
    let mut builder = PathBuilder::stroke(theme::GLYPH_STROKE);
    trace(&mut builder, bounds, points);
    paint(builder, window, colour);
}

fn trace(builder: &mut PathBuilder, bounds: Bounds<Pixels>, points: &[(f32, f32)]) {
    let mut points = points.iter().map(|p| at(bounds, *p));
    if let Some(first) = points.next() {
        builder.move_to(first);
        points.for_each(|p| builder.line_to(p));
    }
}

fn at(bounds: Bounds<Pixels>, (x, y): (f32, f32)) -> Point<Pixels> {
    bounds.origin + point(px(x), px(y))
}

fn paint(builder: PathBuilder, window: &mut Window, colour: Hsla) {
    match builder.build() {
        Ok(path) => window.paint_path(path, colour),
        Err(error) => eprintln!("sakura: glyph path: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_tool_call_s_state_is_in_the_accent_not_even_one_that_waits_for_the_person() {
        let statuses = [
            ToolStatus::Pending,
            ToolStatus::InProgress,
            ToolStatus::Completed,
            ToolStatus::Failed,
            ToolStatus::WaitingForConfirmation,
            ToolStatus::Rejected,
            ToolStatus::Canceled,
        ];
        for palette in [Palette::dark(), Palette::light()] {
            for status in statuses {
                assert_ne!(
                    status_colour(status, &palette),
                    palette.attention,
                    "{status:?}: the card is the one place that says the person is needed"
                );
            }
        }
    }
}
