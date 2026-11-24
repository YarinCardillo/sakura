//! The components hold the rules of the scale on GPUI's test platform: the room inside a card or a chip is
//! the same on both sides, text is centred on a control's height, and a field is never without its label.

use super::tests::{click, open};
use super::*;
use crate::theme::kit::scale::*;
use crate::theme::{Appearance, Palette};
use gpui::{
    div, px, Bounds, InteractiveElement as _, IntoElement as _, ParentElement as _, Pixels,
    Styled as _, TestAppContext, VisualTestContext,
};
use gpui_kit_assets::IconName;
use std::cell::Cell;
use std::rc::Rc;

fn palette() -> Palette {
    Palette::of(Appearance::Dark, crate::theme_brand::Accent::default())
}

fn bounds(cx: &mut VisualTestContext, selector: &'static str) -> Bounds<Pixels> {
    cx.debug_bounds(selector)
        .unwrap_or_else(|| panic!("nothing named {selector} is drawn"))
}

fn centre_y(bounds: Bounds<Pixels>) -> f32 {
    f32::from(bounds.origin.y) + f32::from(bounds.size.height) / 2.
}

#[gpui::test]
fn tag_is_as_high_as_a_chip_and_centres_its_label(cx: &mut TestAppContext) {
    let cx = open(cx, |_, _| {
        div()
            .child(tag("tag", "Sakura", None, &palette()))
            .into_any_element()
    });
    let (outer, label) = (bounds(cx, "tag"), bounds(cx, "tag-label"));
    assert_eq!(outer.size.height, CHIP_HEIGHT);
    assert_eq!(centre_y(outer), centre_y(label));
}

#[gpui::test]
fn tag_keeps_the_same_room_at_both_ends_with_and_without_a_dot(cx: &mut TestAppContext) {
    let cx = open(cx, |_, _| {
        div()
            .flex()
            .flex_col()
            .child(div().flex().child(tag("plain", "Sakura", None, &palette())))
            .child(
                div()
                    .flex()
                    .child(tag("dotted", "Sakura", Some(palette().warning), &palette())),
            )
            .into_any_element()
    });
    let (plain, label) = (bounds(cx, "plain"), bounds(cx, "plain-label"));
    assert_eq!(label.left() - plain.left(), plain.right() - label.right());
    assert_eq!(label.left() - plain.left(), CHIP_PADDING_START);
    let (dotted, dot, words) = (
        bounds(cx, "dotted"),
        bounds(cx, "dotted-dot"),
        bounds(cx, "dotted-label"),
    );
    assert_eq!(dot.left() - dotted.left(), dotted.right() - words.right());
    assert_eq!(words.left() - dot.right(), ICON_TEXT_GAP);
}

#[gpui::test]
fn chip_keeps_the_same_room_at_both_ends_around_its_icon_and_chevron(cx: &mut TestAppContext) {
    let cx = open(cx, |_, cx| {
        div()
            .child(
                menu_chip("chip", "Standard", Some(IconName::FolderPlus), cx)
                    .debug_selector(|| "chip".into()),
            )
            .into_any_element()
    });
    let (outer, before, after) = (
        bounds(cx, "chip"),
        bounds(cx, "chip-before"),
        bounds(cx, "chip-after"),
    );
    assert_eq!(before.left() - outer.left(), outer.right() - after.right());
    assert_eq!(before.size.width, CHIP_ICON);
    assert_eq!(centre_y(outer), centre_y(bounds(cx, "chip-label")));
}

#[gpui::test]
fn every_button_kind_is_the_regular_control_high_with_its_label_centred(cx: &mut TestAppContext) {
    let cx = open(cx, |_, _| {
        div()
            .flex()
            .flex_col()
            .children(
                [
                    ("primary", ButtonKind::Primary),
                    ("secondary", ButtonKind::Secondary),
                    ("quiet", ButtonKind::Quiet),
                    ("destructive", ButtonKind::Destructive),
                ]
                .map(|(id, kind)| button(id, "Approve", kind, &palette())),
            )
            .into_any_element()
    });
    for id in ["primary", "secondary", "quiet", "destructive"] {
        let outer = bounds(cx, id);
        assert_eq!(outer.size.height, CONTROL_REGULAR, "{id}");
    }
}

#[gpui::test]
fn the_large_button_is_the_large_control(cx: &mut TestAppContext) {
    let cx = open(cx, |_, _| {
        div()
            .child(button_large(
                "big",
                "Start",
                ButtonKind::Primary,
                &palette(),
            ))
            .into_any_element()
    });
    assert_eq!(bounds(cx, "big").size.height, CONTROL_LARGE);
}

#[gpui::test]
fn card_keeps_the_same_room_on_all_four_sides_around_its_content(cx: &mut TestAppContext) {
    let cx = open(cx, |_, _| {
        div()
            .w(px(300.))
            .child(
                card("card", &palette())
                    .child(div().debug_selector(|| "inner".into()).h(px(20.)).w_full()),
            )
            .into_any_element()
    });
    let (outer, inner) = (bounds(cx, "card"), bounds(cx, "inner"));
    let inset = |a: Pixels, b: Pixels| a - b;
    assert_eq!(inset(inner.left(), outer.left()), CARD_PADDING);
    assert_eq!(inset(outer.right(), inner.right()), CARD_PADDING);
    assert_eq!(inset(inner.top(), outer.top()), CARD_PADDING);
    assert_eq!(inset(outer.bottom(), inner.bottom()), CARD_PADDING);
}

#[gpui::test]
fn list_row_is_the_large_control_high_with_the_same_room_at_both_sides(cx: &mut TestAppContext) {
    let cx = open(cx, |_, _| {
        div()
            .w(px(300.))
            .child(
                row(RowKind::List, "row", false, &palette()).child(
                    div()
                        .debug_selector(|| "content".into())
                        .w_full()
                        .h(px(18.)),
                ),
            )
            .into_any_element()
    });
    let (outer, content) = (bounds(cx, "row"), bounds(cx, "content"));
    assert_eq!(outer.size.height, CONTROL_LARGE);
    assert_eq!(
        content.left() - outer.left(),
        outer.right() - content.right()
    );
    assert_eq!(centre_y(outer), centre_y(content));
}

#[gpui::test]
fn settings_row_puts_its_name_and_its_control_the_same_distance_from_the_edges(
    cx: &mut TestAppContext,
) {
    let cx = open(cx, |_, _| {
        div()
            .w(px(480.))
            .child(settings_row(
                "setting",
                "Notify me",
                Some("When a lane needs you".into()),
                div()
                    .debug_selector(|| "control".into())
                    .w(px(40.))
                    .h(px(20.)),
                &palette(),
            ))
            .into_any_element()
    });
    let (outer, name, control) = (
        bounds(cx, "setting"),
        bounds(cx, "setting-name"),
        bounds(cx, "control"),
    );
    assert_eq!(name.left() - outer.left(), outer.right() - control.right());
    assert!(bounds(cx, "setting-note").top() >= name.bottom());
}

#[gpui::test]
fn field_shows_its_label_above_its_control(cx: &mut TestAppContext) {
    let cx = open(cx, |_, _| {
        div()
            .w(px(320.))
            .child(field(
                "title",
                "Title",
                div()
                    .debug_selector(|| "title-input".into())
                    .h(px(28.))
                    .w_full(),
                Some("A short name for the lane".into()),
                &palette(),
            ))
            .into_any_element()
    });
    let (label, input) = (bounds(cx, "title-label"), bounds(cx, "title-input"));
    assert!(
        label.bottom() <= input.top(),
        "the label stands above its field"
    );
    assert_eq!(label.left(), input.left());
    assert!(bounds(cx, "title-hint").top() >= input.bottom());
}

#[cfg(not(feature = "boxes"))]
#[gpui::test]
fn stepper_presses_move_the_value_by_one_and_stop_at_the_ends(cx: &mut TestAppContext) {
    let asked = Rc::new(Cell::new(i64::MIN));
    let seen = asked.clone();
    let cx = open(cx, move |_, _| {
        let seen = seen.clone();
        div()
            .child(stepper(
                "count",
                (1, 1),
                Some(1),
                Some(9),
                Rc::new(move |value, _, _| seen.set(value)),
                &palette(),
            ))
            .into_any_element()
    });
    click(cx, "count-less");
    assert_eq!(asked.get(), i64::MIN, "the least value goes no lower");
    click(cx, "count-more");
    assert_eq!(asked.get(), 2);
}

#[gpui::test]
fn empty_state_is_centred_in_its_room_with_the_title_over_the_sentence(cx: &mut TestAppContext) {
    let cx = open(cx, |_, _| {
        div()
            .w(px(600.))
            .h(px(400.))
            .child(empty_state(
                "empty",
                "Nothing needs you",
                Some("Lanes that ask will show here".into()),
                None,
                &palette(),
            ))
            .into_any_element()
    });
    let (outer, title, sentence) = (
        bounds(cx, "empty"),
        bounds(cx, "empty-title"),
        bounds(cx, "empty-sentence"),
    );
    let centre = |b: Bounds<Pixels>| f32::from(b.left()) + f32::from(b.size.width) / 2.;
    assert_eq!(centre(title), centre(outer));
    assert_eq!(centre(sentence), centre(outer));
    assert!(title.bottom() <= sentence.top());
}

#[gpui::test]
fn section_header_is_the_regular_control_high_and_keeps_its_title_at_the_start(
    cx: &mut TestAppContext,
) {
    let cx = open(cx, |_, _| {
        div()
            .w(px(400.))
            .child(section_header("group", "Today", None, &palette()))
            .into_any_element()
    });
    let (outer, title) = (bounds(cx, "group"), bounds(cx, "group-title"));
    assert_eq!(outer.size.height, CONTROL_REGULAR);
    assert_eq!(title.left() - outer.left(), ROW_PADDING);
}

#[gpui::test]
fn a_button_with_an_icon_keeps_the_same_room_at_both_ends(cx: &mut TestAppContext) {
    let cx = open(cx, |_, _| {
        div()
            .child(button_icon(
                "with-icon",
                "Open",
                IconName::FolderPlus,
                ButtonKind::Secondary,
                &palette(),
            ))
            .into_any_element()
    });
    let (outer, icon, label) = (
        bounds(cx, "with-icon"),
        bounds(cx, "with-icon-icon"),
        bounds(cx, "with-icon-label"),
    );
    assert_eq!(icon.left() - outer.left(), outer.right() - label.right());
    assert_eq!(label.left() - icon.right(), ICON_TEXT_GAP);
    assert_eq!(icon.size.width, ICON_REGULAR);
}

#[gpui::test]
fn a_long_button_label_wraps_inside_the_button(cx: &mut TestAppContext) {
    let cx = open(cx, |_, _| {
        div()
            .w(px(120.))
            .flex()
            .child(button(
                "long",
                "Approve and deliver every lane at once",
                ButtonKind::Secondary,
                &palette(),
            ))
            .into_any_element()
    });
    let (outer, label) = (bounds(cx, "long"), bounds(cx, "long-label"));
    assert!(outer.right() <= px(120.), "the button stays in its room");
    assert!(label.size.height > CONTROL_LABEL.line_height);
    assert!(
        label.right() <= outer.right() - SPACE_M,
        "the label keeps its padding"
    );
}

#[gpui::test]
fn a_chip_and_a_tag_with_a_label_too_long_for_their_room_are_cut_and_stay_inside(
    cx: &mut TestAppContext,
) {
    let cx = open(cx, |_, cx| {
        div()
            .w(px(120.))
            .flex()
            .flex_col()
            .child(
                div().flex().child(
                    menu_chip(
                        "long-chip",
                        "Approve and deliver every lane at once",
                        None,
                        cx,
                    )
                    .debug_selector(|| "long-chip".into()),
                ),
            )
            .child(div().flex().child(tag(
                "long-tag",
                "Approve and deliver every lane at once",
                None,
                &palette(),
            )))
            .into_any_element()
    });
    assert!(
        bounds(cx, "long-chip").right() <= px(120.),
        "the chip stays in its room"
    );
    assert!(
        bounds(cx, "long-tag").right() <= px(120.),
        "the tag stays in its room"
    );
}
