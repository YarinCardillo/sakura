//! The state of a lane as a small mark, all in neutral tones: a filled disc for a lane that needs the person, a ring with its
//! right half filled for one that runs, a disc with a check for one that finished, a ring with a dash for
//! one that stopped, a dashed ring for one not started. Each state has its own shape as well as its
//! tone, so no state is told by colour alone. The marks are drawn on a 14 unit grid and scale to any size.

use crate::theme::kit::{
    GLYPH_CHECK_SIZE as CHECK_SIZE, GLYPH_DASH_WIDTH as DASH_WIDTH,
    GLYPH_FINISHED_DIAMETER as FINISHED_DIAMETER, GLYPH_GRID as GRID,
    GLYPH_HALF_RADIUS as HALF_RADIUS, GLYPH_NEEDS_YOU_DIAMETER as NEEDS_YOU_DIAMETER,
    GLYPH_RING_DIAMETER as RING_DIAMETER, GLYPH_STROKE as STROKE,
};
use crate::theme::Palette;
use gpui::{div, AnyElement, Div, Hsla, IntoElement, ParentElement as _, Pixels, Styled as _};
use gpui_component::{Icon, Sizable as _};
use gpui_kit_assets::IconName;

/// The states a lane's mark tells apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateGlyph {
    NeedsYou,
    Running,
    Finished,
    Stopped,
    NotStarted,
}

impl StateGlyph {
    /// The tone of the mark's main part. Never the accent: that colour says "this needs you" once, on the
    /// Inbox row and its count, so a lane that needs the person is told by its filled disc and its words.
    pub fn colour(self, palette: &Palette) -> Hsla {
        match self {
            StateGlyph::NeedsYou | StateGlyph::Running => palette.text_secondary,
            _ => palette.text_tertiary,
        }
    }

    /// The mark at `size`, in the tones of `palette`.
    pub fn paint(self, size: Pixels, palette: &Palette) -> AnyElement {
        let scale = Scale(size / GRID);
        let base = div().size(size).flex_none().relative();
        match self {
            StateGlyph::NeedsYou => base
                .child(scale.disc(NEEDS_YOU_DIAMETER, self.colour(palette)))
                .into_any_element(),
            StateGlyph::Running => base
                .child(scale.ring(palette.text_tertiary))
                .child(scale.half_disc(palette.text_secondary))
                .into_any_element(),
            StateGlyph::Finished => base
                .child(scale.disc(FINISHED_DIAMETER, palette.text_tertiary))
                .child(scale.check(palette.panel))
                .into_any_element(),
            StateGlyph::Stopped => base
                .child(scale.ring(palette.text_tertiary))
                .child(scale.dash(palette.text_tertiary))
                .into_any_element(),
            StateGlyph::NotStarted => base
                .child(scale.ring(palette.text_tertiary).border_dashed())
                .into_any_element(),
        }
    }
}

/// The size of one grid unit, and the parts a mark is made of, each placed on the grid.
struct Scale(Pixels);

impl Scale {
    /// A box of `side` grid units centred on the grid.
    fn centred(&self, side: f32) -> Div {
        let inset = self.0 * ((GRID - side) / 2.);
        div().absolute().top(inset).left(inset).size(self.0 * side)
    }

    fn disc(&self, diameter: f32, colour: Hsla) -> Div {
        self.centred(diameter).rounded_full().bg(colour)
    }

    fn ring(&self, colour: Hsla) -> Div {
        self.centred(RING_DIAMETER)
            .rounded_full()
            .border(self.0 * STROKE)
            .border_color(colour)
    }

    /// The right half of a disc at the grid's centre: what is done of a running lane.
    fn half_disc(&self, colour: Hsla) -> Div {
        div()
            .absolute()
            .top(self.0 * (GRID / 2. - HALF_RADIUS))
            .left(self.0 * (GRID / 2.))
            .w(self.0 * HALF_RADIUS)
            .h(self.0 * (HALF_RADIUS * 2.))
            .rounded_tr(self.0 * HALF_RADIUS)
            .rounded_br(self.0 * HALF_RADIUS)
            .bg(colour)
    }

    fn dash(&self, colour: Hsla) -> Div {
        div()
            .absolute()
            .top(self.0 * ((GRID - STROKE) / 2.))
            .left(self.0 * ((GRID - DASH_WIDTH) / 2.))
            .w(self.0 * DASH_WIDTH)
            .h(self.0 * STROKE)
            .rounded_full()
            .bg(colour)
    }

    fn check(&self, colour: Hsla) -> Div {
        let inset = self.0 * ((GRID - CHECK_SIZE) / 2.);
        div().absolute().top(inset).left(inset).child(
            Icon::new(IconName::Check)
                .with_size(self.0 * CHECK_SIZE)
                .text_color(colour),
        )
    }
}
