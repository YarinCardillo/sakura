//! The geometry of a tab dragged along the strip, as in browser tabs: where it would land, how far the tabs
//! it passes step aside, and where it stops at the strip's ends. Pure; the strip feeds it the tabs' boxes as
//! they stood when the press began.

use super::metrics::SWAP_HYSTERESIS;

/// A tab's box along the strip.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TabBox {
    pub left: f32,
    pub width: f32,
}

impl TabBox {
    pub fn right(&self) -> f32 {
        self.left + self.width
    }

    fn centre(&self) -> f32 {
        self.left + self.width / 2.
    }
}

/// The span a dragged tab stays inside, in the same coordinates as the boxes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StripBounds {
    pub left: f32,
    pub right: f32,
}

/// Where a dragged tab would land: a neighbour trades places as soon as the dragged tab's leading edge (its
/// right edge heading right, its left edge heading left) crosses the neighbour's midpoint, long before it is
/// covered. A trade already made (`current`, the place the tab last had) is only undone once the edge is
/// [`SWAP_HYSTERESIS`] back past that midpoint. Any place is open, the first included.
pub fn drop_index(boxes: &[TabBox], from: usize, dx: f32, current: usize) -> usize {
    let own = boxes[from];
    let (left, right) = (own.left + dx, own.right() + dx);
    let slack = |is_traded: bool| if is_traded { SWAP_HYSTERESIS } else { 0. };
    let passed_right = boxes
        .iter()
        .enumerate()
        .filter(|(index, other)| *index > from && right > other.centre() - slack(*index <= current))
        .count();
    let passed_left = boxes
        .iter()
        .enumerate()
        .filter(|(index, other)| *index < from && left < other.centre() + slack(*index >= current))
        .count();
    from + passed_right - passed_left
}

/// How far a tab other than the dragged one stands aside while `from` heads for `to`: one `step` or none.
pub fn slide_offset(index: usize, from: usize, to: usize, step: f32) -> f32 {
    if from < to && index > from && index <= to {
        -step
    } else if from > to && index < from && index >= to {
        step
    } else {
        0.
    }
}

/// The dragged tab's offset for a pointer `dx` from the press: exact while the whole tab stays inside the
/// strip, and stopped flush at either end of it, so no part of the tab is drawn past the panel's edges. A
/// tab already partly outside the strip (a scrolled one) keeps its own place as the limit.
pub fn resist_drag(boxes: &[TabBox], from: usize, dx: f32, strip: StripBounds) -> f32 {
    let own = boxes[from];
    let min = (strip.left - own.left).min(0.);
    let max = (strip.right - own.right()).max(0.);
    dx.clamp(min, max)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Three tabs, each 120 px wide with the 4 px gap.
    const BOXES: [TabBox; 3] = [
        TabBox {
            left: 0.,
            width: 120.,
        },
        TabBox {
            left: 124.,
            width: 120.,
        },
        TabBox {
            left: 248.,
            width: 120.,
        },
    ];
    /// The strip the tabs sit in: free canvas after the last tab.
    const STRIP: StripBounds = StripBounds {
        left: 0.,
        right: 600.,
    };

    fn edges_after(from: usize, dx: f32, strip: StripBounds) -> (f32, f32) {
        let offset = resist_drag(&BOXES, from, dx, strip);
        (BOXES[from].left + offset, BOXES[from].right() + offset)
    }

    #[test]
    fn resist_drag_inside_the_strip_follows_the_pointer_exactly() {
        assert_eq!(resist_drag(&BOXES, 2, -100., STRIP), -100.);
        assert_eq!(resist_drag(&BOXES, 1, 124., STRIP), 124.);
        assert_eq!(resist_drag(&BOXES, 0, 60., STRIP), 60.);
    }

    #[test]
    fn resist_drag_far_left_stops_the_left_edge_at_the_strip_start() {
        for dx in [-400., -5000.] {
            assert_eq!(edges_after(2, dx, STRIP).0, STRIP.left);
            assert_eq!(edges_after(1, dx, STRIP).0, STRIP.left);
        }
    }

    #[test]
    fn resist_drag_far_right_stops_the_right_edge_at_the_strip_end() {
        for dx in [400., 5000.] {
            assert_eq!(edges_after(2, dx, STRIP).1, STRIP.right);
        }
        assert_eq!(edges_after(0, 5000., STRIP).1, STRIP.right);
    }

    #[test]
    fn resist_drag_of_the_only_tab_stops_at_the_start_and_follows_inside() {
        let one = &BOXES[..1];
        assert_eq!(resist_drag(one, 0, 50., STRIP), 50.);
        assert_eq!(resist_drag(one, 0, -50., STRIP), 0.);
        assert_eq!(resist_drag(one, 0, -5000., STRIP), 0.);
    }

    #[test]
    fn resist_drag_in_a_scrolled_strip_clamps_to_the_visible_bounds() {
        let scrolled = StripBounds {
            left: 80.,
            right: 400.,
        };
        assert_eq!(edges_after(1, -500., scrolled).0, scrolled.left);
        assert_eq!(edges_after(1, 5000., scrolled).1, scrolled.right);
    }

    #[test]
    fn drop_index_any_tab_can_land_first() {
        assert_eq!(drop_index(&BOXES, 2, -400., 2), 0);
        assert_eq!(drop_index(&BOXES, 1, -400., 1), 0);
    }

    #[test]
    fn drop_index_trades_places_at_the_neighbour_midpoint_before_it_is_covered() {
        // Tab 0 ends at 120, tab 1's midpoint is 184: the edge reaches it after 64 px.
        assert_eq!(drop_index(&BOXES, 0, 63., 0), 0);
        assert_eq!(drop_index(&BOXES, 0, 65., 0), 1);
        assert_eq!(drop_index(&BOXES, 0, 65. + 124., 0), 2);
        // Tab 2 starts at 248: the same midpoint is 64 px to its left.
        assert_eq!(drop_index(&BOXES, 2, -63., 2), 2);
        assert_eq!(drop_index(&BOXES, 2, -65., 2), 1);
    }

    #[test]
    fn drop_index_back_inside_the_hysteresis_keeps_the_trade() {
        let inside = 64. - SWAP_HYSTERESIS + 1.;
        assert_eq!(drop_index(&BOXES, 0, inside, 1), 1);
        assert_eq!(drop_index(&BOXES, 0, inside, 0), 0);
    }

    #[test]
    fn drop_index_back_past_the_hysteresis_trades_back() {
        assert_eq!(drop_index(&BOXES, 0, 64. - SWAP_HYSTERESIS - 1., 1), 0);
        assert_eq!(drop_index(&BOXES, 2, -(64. - SWAP_HYSTERESIS + 1.), 1), 1);
        assert_eq!(drop_index(&BOXES, 2, -(64. - SWAP_HYSTERESIS - 1.), 1), 2);
    }

    #[test]
    fn drop_index_a_wobble_around_the_midpoint_never_flickers() {
        let mut current = 0;
        let settled: Vec<usize> = [60., 66., 62., 66., 61., 67.]
            .iter()
            .map(|dx| {
                current = drop_index(&BOXES, 0, *dx, current);
                current
            })
            .collect();
        assert_eq!(settled, [0, 1, 1, 1, 1, 1]);
    }

    #[test]
    fn slide_offset_only_the_passed_tabs_step_aside_by_one_step() {
        assert_eq!(slide_offset(1, 0, 2, 124.), -124.);
        assert_eq!(slide_offset(2, 0, 2, 124.), -124.);
        assert_eq!(slide_offset(0, 2, 0, 124.), 124.);
        assert_eq!(slide_offset(1, 2, 0, 124.), 124.);
        assert_eq!(slide_offset(2, 0, 1, 124.), 0.);
        assert_eq!(slide_offset(1, 1, 1, 124.), 0.);
    }
}
