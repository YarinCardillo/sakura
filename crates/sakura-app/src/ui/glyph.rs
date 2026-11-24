//! The state mark: a Lucide icon, or the filled brand flower for Taken, at the shared mark size, so
//! every state has its own shape as well as its tone (shape plus tone plus word, never colour alone). "Al
//! task" breathes: the half inside its ring fades slowly, opacity only, and stands still when the system
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
        // The accent means one thing, an ask nobody answered: a state's mark says it with a shape, not the accent.
        Tone::Attention => palette.text_secondary,
        Tone::Passed => palette.success,
        Tone::Failed => palette.danger,
        Tone::Warning => palette.warning,
    }
}

/// The shapes a glyph can take; every state maps to one, beside its state word.
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
    /// The filled brand flower: done and taken.
    Flower,
}

impl Shape {
    /// The icon that draws the shape. Of "Working" it is the whole mark, as it stands when nothing
    /// breathes.
    fn icon(self) -> Option<IconName> {
        Some(match self {
            Shape::Dashed => icons::DASHED,
            Shape::Ring => icons::RING,
            Shape::Breathing => icons::WORKING,
            Shape::Dot => icons::NEEDS_YOU,
            Shape::Check => icons::PASSED,
            Shape::Cross => icons::FAILED,
            Shape::Bang => icons::PROBLEM,
            Shape::Flower => return None,
        })
    }

    /// Whether the shape is drawn in a ring, which its filled form fills.
    fn has_ring(self) -> bool {
        self != Shape::Flower
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
            StateId::Taken => Shape::Flower,
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
                ResultView::Vecchio
                | ResultView::Annullata
                | ResultView::Scartata
                | ResultView::VerdeConRiserva(_),
            ) => Shape::Ring,
            VerifyState::Result(ResultView::MaiEseguito) => Shape::Dashed,
        }
    }
}

pub fn state_glyph(state: StateId, tone: Tone, palette: &Palette, breath: Breath) -> AnyElement {
    glyph(Shape::of_state(state), tone, palette, breath)
}

/// The mark of a task's state where it is shown at `place` (the header, a sidebar row, a tab): named
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

/// A task's state on its tab: the mark the sidebar and the header give it. The selected tab has the
/// filled form (a disc in the mark's colour with the mark cut out of it in the panel's colour), the
/// others the outline. Taken uses the flower without a disc on every tab: the tab icon rule. A state with no tone of its own takes the tab's
/// ink, full on the selected tab and quiet on the others; a tone keeps its colour on every tab, so a tab
/// says which task needs the person. Nothing breathes in the strip.
pub fn tab_mark(state: TaskState, palette: &Palette, is_selected: bool) -> AnyElement {
    let shape = Shape::of_state(state.id);
    let colour = match (state.tone, is_selected) {
        (Tone::Neutral | Tone::Old, true) => palette.text,
        (Tone::Neutral | Tone::Old, false) => palette.text_tertiary,
        (tone, _) => tone_colour(tone, palette),
    };
    let base = div().size(theme::MARK_SIZE).flex_none().relative();
    if !(is_selected && shape.has_ring()) {
        return base.child(shape_mark(shape, colour)).into_any_element();
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
        .child(shape_mark(shape, cut_colour(colour, palette)));
    base.child(disc).child(cut).into_any_element()
}

/// The colour a mark is cut in on a disc of `disc`: the panel's, except on the accent, where it is the
/// accent's own ink (black or white, whichever reads better on it).
fn cut_colour(disc: Hsla, palette: &Palette) -> Hsla {
    if disc == palette.attention {
        palette.attention_ink
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

/// Paints the breathing part of an "Working" mark whose box is `bounds`: the whole mark in `colour`,
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
        _ => base.child(shape_mark(shape, colour)).into_any_element(),
    }
}

fn shape_mark(shape: Shape, colour: Hsla) -> AnyElement {
    match shape.icon() {
        Some(name) => icon(name, colour).into_any_element(),
        None => super::brand::taken_mark(colour).into_any_element(),
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
    fn a_mark_on_the_accent_is_cut_in_the_accent_s_ink_and_in_the_panel_s_colour_elsewhere() {
        for palette in [Palette::dark(), Palette::light()] {
            assert_eq!(
                cut_colour(palette.attention, &palette),
                palette.attention_ink
            );
            assert_eq!(cut_colour(palette.text, &palette), palette.panel);
            assert_eq!(cut_colour(palette.success, &palette), palette.panel);
        }
    }

    #[test]
    fn no_tone_takes_the_accent_because_the_accent_is_for_an_unanswered_ask() {
        for palette in [Palette::dark(), Palette::light()] {
            let tones = [
                Tone::Neutral,
                Tone::Running,
                Tone::Attention,
                Tone::Passed,
                Tone::Failed,
                Tone::Old,
                Tone::Warning,
            ];
            assert!(tones
                .iter()
                .all(|tone| tone_colour(*tone, &palette) != palette.attention));
            assert_eq!(
                tone_colour(Tone::Attention, &palette),
                palette.text_secondary
            );
        }
    }

    #[gpui::test]
    fn a_taken_mark_paints_the_flower_in_device_pixels(cx: &mut gpui::TestAppContext) {
        struct Taken;
        impl gpui::Render for Taken {
            fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
                state_glyph(StateId::Taken, Tone::Old, &Palette::dark(), Breath::Still)
            }
        }
        let (_, cx) = cx.add_window_view(|_, _| Taken);
        let quads = cx.update(|window, cx| {
            window.set_scale_factor(2.);
            window.draw(cx).clear(cx);
            window.painted_quads()
        });
        let area: f32 = quads
            .iter()
            .map(|quad| quad.bounds.size.width.0 * quad.bounds.size.height.0)
            .sum();
        assert_eq!(area, 426., "one device pixel for every approved cell");
        assert!(quads.iter().all(|quad| quad.bounds.size.height.0 == 1.));
    }

    struct TakenTab {
        is_selected: bool,
        palette: Palette,
    }

    impl gpui::Render for TakenTab {
        fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
            tab_mark(
                sakura_core::model::state_for(StateId::Taken),
                &self.palette,
                self.is_selected,
            )
        }
    }

    #[gpui::test]
    fn taken_tabs_use_their_text_ink_without_a_disc(cx: &mut gpui::TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, _| TakenTab {
            is_selected: false,
            palette: Palette::dark(),
        });
        for (palette, is_selected) in [
            (Palette::dark(), false),
            (Palette::dark(), true),
            (Palette::light(), false),
            (Palette::light(), true),
        ] {
            let ink: gpui::Background = if is_selected {
                palette.text
            } else {
                palette.text_tertiary
            }
            .into();
            view.update(cx, |view, cx| {
                view.palette = palette;
                view.is_selected = is_selected;
                cx.notify();
            });
            let quads = cx.update(|window, cx| {
                window.set_scale_factor(2.);
                window.draw(cx).clear(cx);
                window.painted_quads()
            });
            assert!(!quads.is_empty());
            assert!(quads.iter().all(|quad| quad.background == ink));
            assert_eq!(
                quads
                    .iter()
                    .map(|quad| quad.bounds.size.width.0 * quad.bounds.size.height.0)
                    .sum::<f32>(),
                426.
            );
        }
    }
}
