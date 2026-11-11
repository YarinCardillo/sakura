//! The state mark: an icon of the window's one set (Lucide, see `crate::icons`) at the one icon size, so
//! every state has its own shape as well as its tone (shape plus tone plus word, never colour alone). "Al
//! lavoro" breathes: the half inside its ring fades slowly, opacity only, and stands still when the system
//! asks for reduced motion. The mark draws its still ring; the breathing half is painted by the breath
//! layer (`ui::breath`), so a breath tick never draws the mark's own view again. The chevron of a row
//! that opens and the dots of an overflow menu are icons of the same set, at the same size.

use super::breath::{Breath, BreathMarks, Mark};
use crate::icons;
use crate::theme::{self, Palette};
use gpui::{
    canvas, div, AnyElement, App, Bounds, Div, Hsla, InteractiveElement as _, IntoElement,
    ParentElement, Pixels, Styled, TransformationMatrix, Window,
};
use gpui_component::{Icon, Sizable as _};
use gpui_kit_assets::IconName;
use sakura_core::model::{ResultView, StateId, TaskState, Tone, VerifyState};

/// The colour a tone takes in a palette.
pub fn tone_colour(tone: Tone, palette: &Palette) -> Hsla {
    match tone {
        Tone::Neutral | Tone::Old => palette.text_tertiary,
        Tone::Running => palette.primary,
        Tone::Attention => palette.attention,
        Tone::Passed => palette.success,
        Tone::Failed => palette.danger,
        Tone::Warning => palette.warning,
    }
}

/// The shapes a glyph can take; every state maps to one, and the word always goes beside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// A dashed ring: not started yet.
    Dashed,
    /// An empty ring: nothing current.
    Ring,
    /// A ring with a breathing half: work in progress.
    Breathing,
    /// A ring with a dot: needs the person.
    Dot,
    /// A ring with a check.
    Check,
    /// A ring with a cross.
    Cross,
    /// A ring with an exclamation mark.
    Bang,
    /// Two checks: done and taken.
    FilledCheck,
}

impl Shape {
    /// The icon that draws the shape. Of "Al lavoro" it is the whole mark, as it stands when nothing
    /// breathes.
    fn icon(self) -> IconName {
        match self {
            Shape::Dashed => icons::DASHED,
            Shape::Ring => icons::RING,
            Shape::Breathing => icons::WORKING,
            Shape::Dot => icons::NEEDS_YOU,
            Shape::Check => icons::PASSED,
            Shape::Cross => icons::FAILED,
            Shape::Bang => icons::PROBLEM,
            Shape::FilledCheck => icons::TAKEN,
        }
    }

    /// Whether the shape is drawn in a ring, which its filled form fills.
    fn has_ring(self) -> bool {
        self != Shape::FilledCheck
    }

    /// The shape of a task state.
    pub fn of_state(state: StateId) -> Self {
        match state {
            StateId::Preparing => Shape::Dashed,
            StateId::NotStarted | StateId::Idle => Shape::Ring,
            StateId::Working => Shape::Breathing,
            StateId::ToReview | StateId::NeedsYou => Shape::Dot,
            StateId::Verified => Shape::Check,
            StateId::Failed | StateId::CreationFailed => Shape::Cross,
            StateId::Unreadable => Shape::Bang,
            StateId::Taken => Shape::FilledCheck,
        }
    }

    /// The shape of a verification row.
    pub fn of_verify(state: VerifyState) -> Self {
        match state {
            VerifyState::InCorso => Shape::Breathing,
            VerifyState::InCoda { .. } => Shape::Dashed,
            VerifyState::Result(ResultView::Passato) => Shape::Check,
            VerifyState::Result(ResultView::Fallito) => Shape::Cross,
            VerifyState::Result(
                ResultView::Vecchio | ResultView::Annullata | ResultView::VerdeConRiserva(_),
            ) => Shape::Ring,
            VerifyState::Result(ResultView::MaiEseguito) => Shape::Dashed,
        }
    }
}

/// The glyph of a task state.
pub fn state_glyph(state: StateId, tone: Tone, palette: &Palette, breath: Breath) -> AnyElement {
    glyph(Shape::of_state(state), tone, palette, breath)
}

/// The mark of a lavoro's state where it is shown at `place` (the header, a sidebar row, a tab): named
/// by place and state, so a test can hold every place to one state.
pub fn placed_state_glyph(
    place: &str,
    state: TaskState,
    palette: &Palette,
    breath: Breath,
) -> AnyElement {
    let selector = format!("state-{place}-{}", state.id.id());
    div()
        .debug_selector(move || selector)
        .flex_none()
        .child(state_glyph(state.id, state.tone, palette, breath))
        .into_any_element()
}

/// A lavoro's state on its tab: the mark the sidebar and the header give it. The selected tab has the
/// filled form (a disc in the mark's colour with the mark cut out of it in the panel's colour), the
/// others the outline: the tab icon rule. A state with no tone of its own takes the tab's
/// ink, full on the selected tab and quiet on the others; a tone keeps its colour on every tab, so a tab
/// says which lavoro needs the person. Nothing breathes in the strip.
pub fn tab_mark(state: TaskState, palette: &Palette, is_selected: bool) -> AnyElement {
    let shape = Shape::of_state(state.id);
    let colour = match (state.tone, is_selected) {
        (Tone::Neutral | Tone::Old, true) => palette.text,
        (Tone::Neutral | Tone::Old, false) => palette.text_tertiary,
        (tone, _) => tone_colour(tone, palette),
    };
    let base = div().size(theme::MARK_SIZE).flex_none().relative();
    if !(is_selected && shape.has_ring()) {
        return base.child(icon(shape.icon(), colour)).into_any_element();
    }
    let disc = div()
        .absolute()
        .inset(theme::MARK_DISC_INSET)
        .rounded_full()
        .bg(colour);
    let cut = div()
        .absolute()
        .top_0()
        .left_0()
        .child(icon(shape.icon(), cut_colour(colour, palette)));
    base.child(disc).child(cut).into_any_element()
}

/// The colour a mark is cut in on a disc of `disc`: the panel's, except on the accent, where the brand's
/// pair is black on fuchsia in both themes.
fn cut_colour(disc: Hsla, palette: &Palette) -> Hsla {
    if disc == palette.attention {
        gpui::rgb(crate::theme_brand::BLACK).into()
    } else {
        palette.panel
    }
}

/// The column a row keeps for its glyph: one icon wide, the glyph on its left edge, so the labels of rows
/// with a glyph and of rows with an icon start on one line. `selector` names the glyph's own box.
pub fn glyph_column(glyph: AnyElement, selector: String) -> Div {
    let mark = div()
        .debug_selector(move || selector)
        .flex_none()
        .child(glyph);
    div()
        .w(theme::ICON_SIZE)
        .flex_none()
        .flex()
        .items_center()
        .child(mark)
}

/// Paints the breathing part of an "Al lavoro" mark whose box is `bounds`: the whole mark in `colour`,
/// over its still ring, so the ring stays as it is and the half inside it fades in and out.
pub fn paint_half(bounds: Bounds<Pixels>, window: &mut Window, colour: Hsla, cx: &App) {
    let path = icons::WORKING.path();
    let painted = window.paint_svg(bounds, path, None, TransformationMatrix::unit(), colour, cx);
    if let Err(error) = painted {
        eprintln!("sakura: breathing mark: {error}");
    }
}

/// A glyph of `shape` in the colour of `tone`.
pub fn glyph(shape: Shape, tone: Tone, palette: &Palette, breath: Breath) -> AnyElement {
    let colour = tone_colour(tone, palette);
    let base = div().size(theme::MARK_SIZE).flex_none().relative();
    match (shape, breath) {
        (Shape::Breathing, Breath::Layered(owner)) => base
            .child(icon(icons::RING, colour))
            .child(breath_mark(owner, colour))
            .into_any_element(),
        _ => base.child(icon(shape.icon(), colour)).into_any_element(),
    }
}

/// The chevron of a row that opens: pointing right when closed, down when open.
pub fn chevron(is_open: bool, colour: Hsla) -> impl IntoElement {
    let name = match is_open {
        true => icons::CHEVRON_OPEN,
        false => icons::CHEVRON_CLOSED,
    };
    div()
        .size(theme::MARK_SIZE)
        .flex_none()
        .child(icon(name, colour))
}

/// The dots of an overflow menu's trigger.
pub fn overflow(colour: Hsla) -> impl IntoElement {
    icon(icons::OVERFLOW, colour)
}

/// An icon of the set at the one size, in `colour`.
fn icon(name: IconName, colour: Hsla) -> Icon {
    Icon::new(name)
        .with_size(theme::MARK_SIZE)
        .text_color(colour)
}

/// Tells the breath layer where the mark is: it paints the breathing half there, on its own tick.
fn breath_mark(owner: super::breath::Owner, colour: Hsla) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, (), window, cx| {
            let mask = window.content_mask();
            BreathMarks::add(
                owner,
                Mark {
                    bounds,
                    mask,
                    colour,
                },
                cx,
            );
        },
    )
    // Over the ring, not after it: an absolute child with no inset would stand where the flow puts it.
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mark_on_the_accent_is_cut_in_black_in_both_themes_and_in_the_panel_s_colour_elsewhere() {
        let black: Hsla = gpui::rgb(crate::theme_brand::BLACK).into();
        for palette in [Palette::dark(), Palette::light()] {
            assert_eq!(cut_colour(palette.attention, &palette), black);
            assert_eq!(cut_colour(palette.text, &palette), palette.panel);
            assert_eq!(cut_colour(palette.success, &palette), palette.panel);
        }
    }

    #[test]
    fn every_tone_has_a_colour_and_attention_is_only_for_attention() {
        let palette = Palette::dark();
        let tones = [
            Tone::Neutral,
            Tone::Running,
            Tone::Passed,
            Tone::Failed,
            Tone::Old,
            Tone::Warning,
        ];
        assert!(tones
            .iter()
            .all(|tone| tone_colour(*tone, &palette) != palette.attention));
        assert_eq!(tone_colour(Tone::Attention, &palette), palette.attention);
    }
}
