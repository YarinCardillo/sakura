//! The smart corner morph's one calculation: the distance in px between the active
//! tab's left edge and the panel's left edge, as a progress from 0 (the tab sits flush on the corner, which
//! is square, and has no left fillet) to 1 (both fully round). The corner is always what the active tab's
//! drawn distance gives: it follows a drag on every frame, and choosing another tab places it at once
//! (nothing animates on a tab switch).

use super::metrics::MORPH_DISTANCE;

/// The morph's progress for a tab `distance` px from the panel's left edge: 0 flush, 1 from 28 px on.
pub fn morph_progress(distance: f32) -> f32 {
    (distance / MORPH_DISTANCE).clamp(0.0, 1.0)
}

/// The radius of the panel's top-left corner and of the tab's left fillet at that distance.
#[cfg(test)]
pub fn corner_radius(distance: f32) -> f32 {
    super::metrics::RADIUS * morph_progress(distance)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn morph_progress_at_0_14_28_and_200_px_is_square_half_round_round() {
        assert_eq!(morph_progress(0.), 0.0);
        assert_eq!(morph_progress(14.), 0.5);
        assert_eq!(morph_progress(28.), 1.0);
        assert_eq!(morph_progress(200.), 1.0);
    }

    #[test]
    fn corner_radius_at_0_14_28_and_200_px_is_0_6_12_12() {
        assert_eq!(corner_radius(0.), 0.0);
        assert_eq!(corner_radius(14.), 6.0);
        assert_eq!(corner_radius(28.), 12.0);
        assert_eq!(corner_radius(200.), 12.0);
    }

    #[test]
    fn morph_progress_left_of_the_edge_stays_square() {
        assert_eq!(morph_progress(-30.), 0.0);
    }

    #[test]
    fn corner_and_fillet_always_fit_in_the_gap_they_share() {
        for distance in 0..=60 {
            let distance = distance as f32;
            assert!(corner_radius(distance) * 2. <= distance + f32::EPSILON);
        }
    }
}
