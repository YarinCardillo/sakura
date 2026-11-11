//! Draws the fused shape: one filled path for the active tab and the top of the panel, and the ring along
//! the same outline, half its width inside so it lies within the fill like a border. The two paths are
//! built when the shape's geometry changes and kept ([`Prepared`]); a frame that draws the same shape
//! again submits what was prepared.

use super::metrics::RING_WIDTH;
use super::shape::{Step, Top};
use gpui::{point, px, Hsla, Path, PathBuilder, Pixels, Window};

/// The colours of the fused shape.
#[derive(Debug, Clone, Copy)]
pub struct FusedColours {
    pub fill: Hsla,
    pub ring: Hsla,
}

/// The strip's shape of one geometry (the fused tab, or the panel's top alone), ready to submit: its fill
/// and its ring.
pub struct Prepared {
    shape: Top,
    fill: Option<Path<Pixels>>,
    ring: Option<Path<Pixels>>,
}

impl Prepared {
    /// Builds the two paths of `shape`.
    pub fn of(shape: &Top) -> Self {
        let mut fill = PathBuilder::fill();
        trace(&mut fill, &shape.outline(0.));
        fill.close();
        let mut ring = PathBuilder::stroke(px(RING_WIDTH));
        trace(&mut ring, &shape.outline(RING_WIDTH / 2.));
        Self {
            shape: *shape,
            fill: built(fill),
            ring: built(ring),
        }
    }

    /// Whether these are the paths of `shape`.
    pub fn is_for(&self, shape: &Top) -> bool {
        self.shape == *shape
    }

    /// Submits the tab and the panel's top as one shape, then its ring. The window takes a path by
    /// value, so each is handed a copy of what was prepared; nothing is traced or tessellated here.
    pub fn paint(&self, colours: FusedColours, window: &mut Window) {
        if let Some(fill) = &self.fill {
            window.paint_path(fill.clone(), colours.fill);
        }
        if let Some(ring) = &self.ring {
            window.paint_path(ring.clone(), colours.ring);
        }
    }
}

fn trace(builder: &mut PathBuilder, steps: &[Step]) {
    for step in steps {
        match *step {
            Step::MoveTo(x, y) => builder.move_to(point(px(x), px(y))),
            Step::LineTo(x, y) => builder.line_to(point(px(x), px(y))),
            Step::Arc {
                x,
                y,
                radius,
                is_clockwise,
            } => builder.arc_to(
                point(px(radius), px(radius)),
                px(0.),
                false,
                is_clockwise,
                point(px(x), px(y)),
            ),
        }
    }
}

fn built(builder: PathBuilder) -> Option<Path<Pixels>> {
    match builder.build() {
        Ok(path) => Some(path),
        Err(error) => {
            eprintln!("sakura: tab strip path: {error}");
            None
        }
    }
}
