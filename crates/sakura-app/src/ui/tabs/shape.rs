//! The fused shape: the active tab and the top of the panel as one outline. The tab's top corners are round;
//! at its base two concave fillets run into the panel's top edge; the panel's top-left corner and the tab's
//! left fillet share one radius, the corner morph's (0 when the tab sits flush on the corner: the tab's left
//! side then runs straight down into the panel's). With no tab open the shape is the panel's top alone
//! ([`PanelTop`]). Pure geometry: steps for a path, drawn by `paint`.

use super::metrics::RADIUS;

/// One step of the outline, clockwise from the panel's left side.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Step {
    MoveTo(f32, f32),
    LineTo(f32, f32),
    /// A quarter circle of `radius` to the point; convex corners turn clockwise, fillets the other way.
    Arc {
        x: f32,
        y: f32,
        radius: f32,
        is_clockwise: bool,
    },
}

/// The most steps an outline has: its start, four for the left corner, four for the tab, five for the right.
pub const OUTLINE_STEPS: usize = 14;

/// The steps of an outline, in storage of a fixed size: tracing the shape asks the heap for nothing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Outline {
    steps: [Step; OUTLINE_STEPS],
    len: usize,
}

impl Outline {
    fn starting_at(x: f32, y: f32) -> Self {
        let mut steps = [Step::MoveTo(0., 0.); OUTLINE_STEPS];
        steps[0] = Step::MoveTo(x, y);
        Self { steps, len: 1 }
    }

    fn push(&mut self, step: Step) {
        self.steps[self.len] = step;
        self.len += 1;
    }
}

impl std::ops::Deref for Outline {
    type Target = [Step];

    fn deref(&self) -> &[Step] {
        &self.steps[..self.len]
    }
}

/// Where the active tab and the panel stand, in window px.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fused {
    /// The panel's left and right edges and its top edge (the strip's bottom).
    pub panel_left: f32,
    pub panel_right: f32,
    pub panel_top: f32,
    /// The active tab as drawn (in a drag: under the pointer).
    pub tab_left: f32,
    pub tab_right: f32,
    pub tab_top: f32,
    /// The corner morph's progress, 0 to 1.
    pub morph: f32,
}

impl Fused {
    /// The y the outline starts and ends on: one radius under the panel's top, where the panel's straight
    /// sides begin.
    pub fn base(&self) -> f32 {
        self.panel_top + RADIUS
    }

    /// The radius the panel's top-left corner and the tab's left fillet share.
    pub fn left_radius(&self) -> f32 {
        let gap = (self.tab_left - self.panel_left).max(0.);
        (RADIUS * self.morph).min(gap / 2.)
    }

    /// The right fillet: whole, unless the tab stands closer to the panel's own corner than a radius.
    pub fn right_radius(&self) -> f32 {
        (self.panel_right - RADIUS - self.tab_right).clamp(0., RADIUS)
    }

    /// The outline from the panel's left side at [`Fused::base`], over the tab, to its right side at the
    /// base, `inset` px inside the shape (0 for the fill, half the ring's width for its stroke).
    pub fn outline(&self, inset: f32) -> Outline {
        let mut steps = Outline::starting_at(self.panel_left + inset, self.base());
        self.left_corner(inset, &mut steps);
        self.tab(inset, &mut steps);
        self.right_corner(inset, &mut steps);
        steps
    }

    /// The panel's top-left corner and the tab's left fillet, up to the foot of the tab's left side.
    fn left_corner(&self, inset: f32, steps: &mut Outline) {
        let (left, top) = (self.panel_left + inset, self.panel_top + inset);
        let radius = self.left_radius();
        if self.tab_left - self.panel_left <= f32::EPSILON {
            return;
        }
        if radius > inset {
            steps.push(Step::LineTo(left, self.panel_top + radius));
            steps.push(arc(self.panel_left + radius, top, radius - inset, true));
            steps.push(Step::LineTo(self.tab_left - radius, top));
            let foot = self.panel_top - radius;
            steps.push(arc(self.tab_left + inset, foot, radius + inset, false));
        } else {
            steps.push(Step::LineTo(left, top));
            steps.push(Step::LineTo(self.tab_left + inset, top));
        }
    }

    /// The tab's two sides and its top, with its round top corners.
    fn tab(&self, inset: f32, steps: &mut Outline) {
        let (left, right) = (self.tab_left + inset, self.tab_right - inset);
        let (top, radius) = (self.tab_top + inset, RADIUS - inset);
        steps.push(Step::LineTo(left, top + radius));
        steps.push(arc(left + radius, top, radius, true));
        steps.push(Step::LineTo(right - radius, top));
        steps.push(arc(right, top + radius, radius, true));
    }

    /// The tab's right fillet, the panel's top edge and its top-right corner.
    fn right_corner(&self, inset: f32, steps: &mut Outline) {
        let (right, top) = (self.panel_right - inset, self.panel_top + inset);
        let fillet = self.right_radius();
        if fillet > 0. {
            steps.push(Step::LineTo(
                self.tab_right - inset,
                self.panel_top - fillet,
            ));
            steps.push(arc(self.tab_right + fillet, top, fillet + inset, false));
        } else {
            steps.push(Step::LineTo(self.tab_right - inset, top));
        }
        steps.push(Step::LineTo(self.panel_right - RADIUS, top));
        steps.push(arc(right, self.panel_top + RADIUS, RADIUS - inset, true));
        steps.push(Step::LineTo(right, self.base()));
    }
}

/// The panel's top with no tab on it, in window px: its two round corners and the edge between them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PanelTop {
    pub left: f32,
    pub right: f32,
    pub top: f32,
}

impl PanelTop {
    /// The outline from the panel's left side, one radius under its top, over its top edge to its right
    /// side at the same height, `inset` px inside the shape.
    pub fn outline(&self, inset: f32) -> Outline {
        let (left, right, top) = (self.left + inset, self.right - inset, self.top + inset);
        let base = self.top + RADIUS;
        let mut steps = Outline::starting_at(left, base);
        steps.push(arc(self.left + RADIUS, top, RADIUS - inset, true));
        steps.push(Step::LineTo(self.right - RADIUS, top));
        steps.push(arc(right, base, RADIUS - inset, true));
        steps
    }
}

/// What the strip draws over the panel: the tab shown fused into the panel's top, or the top alone when no
/// tab is open.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Top {
    Fused(Fused),
    Plain(PanelTop),
}

impl Top {
    /// The shape's outline, `inset` px inside it.
    pub fn outline(&self, inset: f32) -> Outline {
        match self {
            Top::Fused(fused) => fused.outline(inset),
            Top::Plain(panel) => panel.outline(inset),
        }
    }
}

fn arc(x: f32, y: f32, radius: f32, is_clockwise: bool) -> Step {
    Step::Arc {
        x,
        y,
        radius,
        is_clockwise,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A panel from 244 to 1432 with its top at 44, and a 120 px tab `distance` px from its left edge.
    fn fused(distance: f32, morph: f32) -> Fused {
        Fused {
            panel_left: 244.,
            panel_right: 1432.,
            panel_top: 44.,
            tab_left: 244. + distance,
            tab_right: 244. + distance + 120.,
            tab_top: 8.,
            morph,
        }
    }

    fn arcs(steps: &[Step]) -> Vec<(f32, bool)> {
        steps
            .iter()
            .filter_map(|step| match step {
                Step::Arc {
                    radius,
                    is_clockwise,
                    ..
                } => Some((*radius, *is_clockwise)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_longest_outline_fits_its_fixed_storage() {
        // A tab away from both corners has every step there is.
        assert_eq!(fused(28., 1.).outline(0.5).len(), OUTLINE_STEPS);
    }

    #[test]
    fn with_no_tab_the_panel_s_top_is_two_round_corners_and_the_edge_between() {
        let panel = PanelTop {
            left: 244.,
            right: 1432.,
            top: 44.,
        };
        let steps = panel.outline(0.);
        assert_eq!(steps[0], Step::MoveTo(244., 56.));
        assert_eq!(steps[2], Step::LineTo(1420., 44.));
        assert_eq!(steps.last(), Some(&arc(1432., 56., 12., true)));
        assert_eq!(
            arcs(&steps),
            [(12., true), (12., true)],
            "no tab, no fillet"
        );
        let ring = Top::Plain(panel).outline(0.5);
        assert_eq!(ring[0], Step::MoveTo(244.5, 56.));
        assert_eq!(arcs(&ring), [(11.5, true), (11.5, true)]);
    }

    #[test]
    fn a_flush_tab_has_a_square_corner_and_no_left_fillet() {
        let steps = fused(0., 0.).outline(0.);
        assert_eq!(steps[0], Step::MoveTo(244., 56.));
        assert_eq!(
            steps[1],
            Step::LineTo(244., 20.),
            "straight up the tab's side"
        );
        let turns = arcs(&steps);
        assert_eq!(
            turns,
            [(12., true), (12., true), (12., false), (12., true)],
            "the tab's two corners, its right fillet, the panel's right corner"
        );
    }

    #[test]
    fn a_tab_28_px_away_has_a_full_corner_and_a_full_left_fillet() {
        let steps = fused(28., 1.).outline(0.);
        let turns = arcs(&steps);
        assert_eq!(turns[0], (12., true), "the panel's corner");
        assert_eq!(turns[1], (12., false), "the tab's left fillet");
        assert_eq!(turns.len(), 6);
        assert_eq!(steps[1], Step::LineTo(244., 56.));
    }

    #[test]
    fn a_tab_14_px_away_has_half_the_radius_on_both() {
        let turns = arcs(&fused(14., 0.5).outline(0.));
        assert_eq!(turns[0], (6., true));
        assert_eq!(turns[1], (6., false));
    }

    #[test]
    fn corner_and_fillet_never_overlap_whatever_the_progress_says() {
        // A tab 10 px away with a progress of 1 takes what fits in the gap.
        let shape = fused(10., 1.);
        assert_eq!(shape.left_radius(), 5.);
    }

    #[test]
    fn the_ring_runs_half_a_px_inside_with_concentric_radii() {
        let steps = fused(28., 1.).outline(0.5);
        assert_eq!(steps[0], Step::MoveTo(244.5, 56.));
        let turns = arcs(&steps);
        assert_eq!(turns[0], (11.5, true), "a convex corner loses the inset");
        assert_eq!(turns[1], (12.5, false), "a fillet gains it");
        assert_eq!(turns[2], (11.5, true));
    }

    #[test]
    fn the_outline_ends_on_the_panel_right_side_at_the_base() {
        let steps = fused(28., 1.).outline(0.);
        assert_eq!(steps.last(), Some(&Step::LineTo(1432., 56.)));
    }

    #[test]
    fn a_tab_at_the_strip_end_gives_up_its_right_fillet_to_the_panel_corner() {
        let mut shape = fused(28., 1.);
        shape.tab_right = shape.panel_right - RADIUS;
        shape.tab_left = shape.tab_right - 120.;
        assert_eq!(shape.right_radius(), 0.);
        let turns = arcs(&shape.outline(0.));
        assert!(
            turns
                .iter()
                .filter(|(_, is_clockwise)| !is_clockwise)
                .count()
                == 1
        );
    }

    #[test]
    fn a_morph_too_small_for_the_ring_draws_square() {
        let steps = fused(1., 1. / 28.).outline(0.5);
        assert!(matches!(steps[1], Step::LineTo(x, y) if x == 244.5 && y == 44.5));
    }
}
