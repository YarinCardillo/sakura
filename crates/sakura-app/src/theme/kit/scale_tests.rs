//! The design scale holds its own rules: the grid, the counts, and the one chip rule.

use super::scale::*;
use gpui::Pixels;

fn points(value: Pixels) -> f32 {
    f32::from(value)
}

#[test]
fn spacing_steps_all_sit_on_the_four_point_grid() {
    for step in SPACING_STEPS {
        assert_eq!(
            points(step) % 4.,
            0.,
            "{} is off the 4 pt grid",
            points(step)
        );
    }
}

#[test]
fn spacing_steps_grow_one_after_the_other() {
    let steps = SPACING_STEPS.map(points);
    assert!(steps.windows(2).all(|pair| pair[0] < pair[1]), "{steps:?}");
}

#[test]
fn type_scale_has_body_at_thirteen_points() {
    assert_eq!(points(TEXT_BODY.size), 13.);
}

#[test]
fn type_scale_sizes_grow_from_caption_to_the_large_title() {
    let sizes = TYPE_SCALE.map(|(_, style)| points(style.size));
    assert!(sizes.windows(2).all(|pair| pair[0] < pair[1]), "{sizes:?}");
}

#[test]
fn type_scale_lines_are_taller_than_their_text_and_even() {
    for (name, style) in TYPE_SCALE {
        let (size, line) = (points(style.size), points(style.line_height));
        assert!(
            line > size,
            "{name}: a line of {line} cannot hold text of {size}"
        );
        assert_eq!(line % 2., 0., "{name}: line height {line} is odd");
    }
}

#[test]
fn type_scale_names_are_unique() {
    let mut names: Vec<_> = TYPE_SCALE.iter().map(|(name, _)| *name).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), TYPE_SCALE.len());
}

#[test]
fn every_control_label_is_the_body_size_in_the_medium_weight() {
    assert_eq!(CONTROL_LABEL.size, TEXT_BODY.size);
    assert_eq!(CONTROL_LABEL.line_height, TEXT_BODY.line_height);
    assert_eq!(CONTROL_LABEL.weight, super::super::WEIGHT_MEDIUM);
}

#[test]
fn there_are_three_icon_sizes_in_growing_order() {
    let sizes = ICON_SIZES.map(points);
    assert_eq!(sizes, [12., 16., 20.]);
}

#[test]
fn there_are_three_control_heights_and_the_regular_one_is_the_window_control() {
    let heights = CONTROL_HEIGHTS.map(points);
    assert!(
        heights.windows(2).all(|pair| pair[0] < pair[1]),
        "{heights:?}"
    );
    assert_eq!(CONTROL_REGULAR, super::super::CONTROL_HEIGHT);
}

#[test]
fn a_control_holds_its_label_with_room_over_and_under() {
    for height in CONTROL_HEIGHTS {
        assert!(points(height) >= points(CONTROL_LABEL.line_height) + 4.);
    }
}

#[test]
fn there_are_at_most_four_distinct_radii() {
    let mut radii = RADII.map(points).to_vec();
    radii.sort_by(f32::total_cmp);
    radii.dedup();
    assert!(radii.len() <= 4, "{radii:?}");
}

#[test]
fn the_pill_radius_is_half_the_chip_height() {
    assert_eq!(points(RADIUS_PILL) * 2., points(CHIP_HEIGHT));
}

#[test]
fn a_chip_is_the_small_control_with_the_same_room_on_both_sides() {
    assert_eq!(CHIP_HEIGHT, CONTROL_SMALL);
    assert_eq!(CHIP_PADDING_START, CHIP_PADDING_END);
}

#[test]
fn the_gap_between_an_icon_and_its_text_is_a_spacing_step() {
    assert!(SPACING_STEPS.contains(&ICON_TEXT_GAP));
}

#[test]
fn a_chip_icon_is_the_small_icon_and_fits_the_chip() {
    assert_eq!(CHIP_ICON, ICON_SMALL);
    assert!(points(CHIP_ICON) < points(CHIP_HEIGHT));
}

#[test]
fn a_chip_centres_its_label_the_same_distance_from_top_and_bottom() {
    let room = points(CHIP_HEIGHT) - points(CONTROL_LABEL.line_height);
    assert_eq!(room % 2., 0., "{room} points of room cannot split evenly");
}

#[test]
fn the_older_theme_tokens_are_the_scale_and_not_a_second_copy() {
    use super::super as theme;
    assert_eq!(theme::BODY_TEXT, 13.);
    assert_eq!(theme::ICON, f32::from(ICON_REGULAR));
    assert_eq!(theme::CONTROL, f32::from(CONTROL_REGULAR));
    assert_eq!(theme::STATUS_CONTROL, f32::from(CONTROL_SMALL));
    assert_eq!(theme::SURFACE_RADIUS, f32::from(RADIUS_SURFACE));
    assert_eq!(theme::CONTROL_RADIUS, f32::from(RADIUS_CONTROL));
    assert_eq!(theme::TEXT_BODY, TEXT_BODY.size);
    assert_eq!(theme::TEXT_SMALL, CONTROL_LABEL.size);
    assert_eq!(theme::TEXT_LABEL, TEXT_FOOTNOTE.size);
    assert_eq!(theme::GAP_TIGHT, SPACE_XS);
    assert_eq!(theme::GAP, SPACE_S);
    assert_eq!(theme::GAP_WIDE, SPACE_M);
}
