//! The "Al lavoro" breath, drawn so that its tick repaints only what breathes.
//!
//! GPUI redraws a view together with every view above it, so a breathing glyph that asked its own view to
//! draw again redrew the sidebar and the page header on every tick (about 3 ms a frame). Now a breathing
//! glyph draws only its still ring where it stands, and leaves a mark (where its breathing half goes, the
//! clip it is drawn in, its colour) in [`BreathMarks`]. One [`BreathLayer`], drawn last in the desk, paints
//! every breathing half at the opacity of the moment, and only the layer ticks: a tick draws the desk's own
//! frame again, while the sidebar and the header are reused as they were. Each owner clears its marks when it
//! draws again, so a glyph that went is never drawn.
//!
//! The breath is slow (2.4 s, opacity only): the layer ticks at the shortest motion duration, and only while
//! something breathes (an agent at work, a verification in progress) and its glyph is on screen: a mark
//! left by a glyph starts the tick, and the tick ends itself when no mark is left. With reduced motion
//! nothing ticks and the glyph draws its half in place, full and still.

use super::glyph;
use crate::data::facts::Facts;
use crate::theme;
use gpui::{
    canvas, App, Bounds, ContentMask, Context, Entity, Global, Hsla, IntoElement, Pixels, Render,
    Styled, Task, Window,
};
use sakura_core::model::{self, StateId};
use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Instant;

/// The view that drew a breathing glyph. Its marks are replaced each time it draws again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Owner {
    Sidebar,
    Header,
    /// The page below the header (the Verifiche rows).
    Page,
}

/// Where one breathing half is painted, in window coordinates, inside the clip its glyph was drawn in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mark {
    pub bounds: Bounds<Pixels>,
    pub mask: ContentMask<Pixels>,
    pub colour: Hsla,
}

/// The breathing halves on screen, by the view that drew them.
#[derive(Default)]
pub struct BreathMarks {
    marks: HashMap<Owner, Vec<Mark>>,
    /// The layer that paints them: told when a first mark is left, so its tick starts only then.
    layer: Option<gpui::WeakEntity<BreathLayer>>,
}

impl Global for BreathMarks {}

impl BreathMarks {
    /// Forgets `owner`'s marks: called when it draws again, before its glyphs leave new ones.
    pub fn clear(owner: Owner, cx: &mut App) {
        cx.default_global::<Self>().forget(owner);
    }

    /// Leaves a mark for `owner`'s glyph painted now. Once this frame is drawn, a layer that does not
    /// tick is told there is something to breathe.
    pub fn add(owner: Owner, mark: Mark, cx: &mut App) {
        let marks = cx.default_global::<Self>();
        marks.insert(owner, mark);
        let Some(layer) = marks.layer.clone() else {
            return;
        };
        cx.defer(move |cx| drop(layer.update(cx, |layer, cx| layer.wake(cx))));
    }

    /// How many breathing halves the layer paints.
    pub fn count(cx: &App) -> usize {
        cx.try_global::<Self>()
            .map_or(0, |marks| marks.all().count())
    }

    fn forget(&mut self, owner: Owner) {
        self.marks.remove(&owner);
    }

    fn insert(&mut self, owner: Owner, mark: Mark) {
        self.marks.entry(owner).or_default().push(mark);
    }

    fn all(&self) -> impl Iterator<Item = &Mark> {
        self.marks.values().flatten()
    }
}

/// How a breathing glyph is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Breath {
    /// Reduced motion: the half drawn in place, full.
    Still,
    /// The half left to the layer, under this owner.
    Layered(Owner),
}

impl Breath {
    /// How `owner` draws breathing glyphs now.
    pub fn of(owner: Owner, cx: &App) -> Self {
        if cx.reduce_motion() {
            Breath::Still
        } else {
            Breath::Layered(owner)
        }
    }
}

/// Paints every breathing half on a tick of its own, while something breathes on screen.
pub struct BreathLayer {
    facts: Entity<Facts>,
    /// The tick: it runs only while a lavoro works AND a breathing mark is on screen, so a page that
    /// shows no working lavoro (the first page, an empty desk, a hidden sidebar) draws nothing again.
    tick: Option<Task<()>>,
}

impl BreathLayer {
    /// The layer of a desk over `facts`.
    pub fn new(facts: Entity<Facts>, cx: &mut Context<Self>) -> Self {
        cx.default_global::<BreathMarks>().layer = Some(cx.weak_entity());
        Self { facts, tick: None }
    }

    /// Whether the layer's tick runs.
    #[cfg(test)]
    pub fn is_ticking(&self) -> bool {
        self.tick.is_some()
    }

    /// Starts the tick after a frame that left a mark, if it does not run and something breathes.
    fn wake(&mut self, cx: &mut Context<Self>) {
        if self.tick.is_none() && is_needed(self.facts.read(cx), cx) {
            self.tick = Some(cx.spawn(tick));
        }
    }

    /// One tick: draws the marks again, or ends the tick when nothing on screen breathes any more.
    /// The marks are counted here, between frames, where every owner has left its own.
    fn beat(&mut self, cx: &mut Context<Self>) -> bool {
        let is_kept = BreathMarks::count(cx) > 0 && is_needed(self.facts.read(cx), cx);
        if is_kept {
            cx.notify();
        } else {
            self.tick = None;
        }
        is_kept
    }
}

impl Render for BreathLayer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::snapshot::count_render("breath layer");
        if !is_needed(self.facts.read(cx), cx) {
            self.tick = None;
        }
        canvas(|_, _, _| (), |_, (), window, cx| paint_marks(window, cx))
            .absolute()
            .size_full()
    }
}

/// Paints every mark at the opacity of this moment, each inside its own clip.
fn paint_marks(window: &mut Window, cx: &mut App) {
    let opacity = opacity_now();
    let Some(marks) = cx.try_global::<BreathMarks>() else {
        return;
    };
    for mark in marks.all() {
        let colour = mark.colour.opacity(opacity);
        window.with_content_mask(Some(mark.mask), |window| {
            glyph::paint_half(mark.bounds, window, colour, cx);
        });
    }
}

/// The breath's opacity now: every glyph breathes in step, counted from the first time it is read.
fn opacity_now() -> f32 {
    let epoch = *BREATH_EPOCH.get_or_init(Instant::now);
    let into = epoch.elapsed().as_secs_f32() % theme::BREATH.as_secs_f32();
    opacity_at(into / theme::BREATH.as_secs_f32())
}

/// When breaths are counted from.
static BREATH_EPOCH: OnceLock<Instant> = OnceLock::new();

/// Opacity through one breath: full at the start and end, lowest halfway.
fn opacity_at(delta: f32) -> f32 {
    let low = theme::BREATH_LOW_OPACITY;
    low + (1.0 - low) * (0.5 + 0.5 * (std::f32::consts::TAU * delta).cos())
}

async fn tick(layer: gpui::WeakEntity<BreathLayer>, cx: &mut gpui::AsyncApp) {
    loop {
        cx.background_executor()
            .timer(theme::DURATION_INSTANT)
            .await;
        if !matches!(layer.update(cx, BreathLayer::beat), Ok(true)) {
            return;
        }
    }
}

/// Whether anything may breathe: an agent at work or a verification running, and motion allowed.
pub fn is_needed(facts: &Facts, cx: &App) -> bool {
    if cx.reduce_motion() {
        return false;
    }
    let is_working = facts
        .rows()
        .iter()
        .any(|row| model::task_state(&row.task).id == StateId::Working);
    let is_verifying = facts
        .runner()
        .is_some_and(|runner| runner.current.is_some());
    is_working || is_verifying
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{point, px, size};

    fn mark(x: f32) -> Mark {
        let bounds = Bounds::new(point(px(x), px(0.)), size(px(16.), px(16.)));
        Mark {
            bounds,
            mask: ContentMask { bounds },
            colour: gpui::black(),
        }
    }

    #[test]
    fn an_owner_drawing_again_replaces_only_its_own_marks() {
        let mut marks = BreathMarks::default();
        marks.insert(Owner::Sidebar, mark(0.));
        marks.insert(Owner::Sidebar, mark(20.));
        marks.insert(Owner::Header, mark(40.));
        marks.forget(Owner::Sidebar);
        marks.insert(Owner::Sidebar, mark(60.));
        let left: Vec<f32> = marks.all().map(|m| f32::from(m.bounds.origin.x)).collect();
        assert_eq!(left.len(), 2);
        assert!(left.contains(&40.) && left.contains(&60.));
    }

    #[test]
    fn breath_is_full_at_rest_and_lowest_halfway() {
        assert!((opacity_at(0.0) - 1.0).abs() < 1e-6);
        assert!((opacity_at(1.0) - 1.0).abs() < 1e-6);
        assert!((opacity_at(0.5) - theme::BREATH_LOW_OPACITY).abs() < 1e-6);
    }
}
