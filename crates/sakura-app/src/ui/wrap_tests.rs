//! Long diff lines in a test window: on the fixture task whose patch has a 600 character line in every
//! file, no row of code ends past the list's edge, the long line is as tall as the rows it wraps onto, and
//! the wrap follows the list's width when the sidebar is hidden.

use super::changes::list::LayoutNow;
use super::desk::Desk;
use super::interaction_tests::{click, draw, open_with, wait_for};
use crate::data::fixture_changes::LARGE_TASK;
use crate::theme;
use gpui::{Entity, TestAppContext, VisualTestContext};

fn layout(cx: &mut VisualTestContext, desk: &Entity<Desk>) -> LayoutNow {
    cx.update(|window, cx| {
        let list = desk.read(cx).changes_list(LARGE_TASK);
        let list = list.expect("the list");
        list.update(cx, |list, cx| list.layout_now(window, cx))
    })
}

/// The rows of the tallest row of the list.
fn tallest(layout: &LayoutNow) -> f32 {
    let tallest = layout.heights.iter().copied().fold(0.0, f32::max);
    tallest / theme::DIFF_LINE_HEIGHT
}

/// No row of code, as the text system shaped it, ends past the list's edge: the rows' own extents, not an
/// estimate from the column count. Half a point of rounding is allowed.
fn assert_shaped_rows_fit(layout: &LayoutNow) {
    assert!(layout.shaped_end > gpui::px(0.), "code was shaped");
    assert!(
        layout.shaped_end <= layout.width + gpui::px(0.5),
        "the widest shaped row ends at {:?} in a list {:?} wide",
        layout.shaped_end,
        layout.width
    );
}

fn open_large(cx: &mut TestAppContext) -> (&mut VisualTestContext, Entity<Desk>) {
    let (cx, desk, _) = open_with(cx, None);
    cx.update(|_, cx| cx.set_reduce_motion(true));
    click(cx, "row-sample-folders");
    wait_for(cx, "the changes", |cx| {
        cx.debug_bounds("changes-header-0").is_some()
    });
    (cx, desk)
}

#[gpui::test]
fn in_the_pane_a_long_line_wraps_inside_the_list_and_is_as_tall_as_its_rows(
    cx: &mut TestAppContext,
) {
    let (cx, desk) = open_large(cx);
    click(cx, "changes-header-0");
    let layout = layout(cx, &desk);
    assert!(layout.width > gpui::px(0.));
    assert!(
        layout.code_end + theme::DIFF_CODE_PADDING <= layout.width,
        "a full row of code ends at {:?} in a list {:?} wide",
        layout.code_end,
        layout.width
    );
    assert_shaped_rows_fit(&layout);
    let rows = tallest(&layout);
    assert!(
        rows >= 600.0 / layout.columns as f32,
        "{rows} rows of {}",
        layout.columns
    );
    assert_eq!(rows.fract(), 0.0, "whole rows");
}

#[gpui::test]
fn the_wrap_follows_the_width_when_the_sidebar_is_hidden(cx: &mut TestAppContext) {
    let (cx, desk) = open_large(cx);
    click(cx, "changes-header-0");
    let narrow = layout(cx, &desk);
    cx.simulate_keystrokes("cmd-b");
    draw(cx);
    let wide = layout(cx, &desk);
    assert!(wide.width > narrow.width);
    assert!(
        wide.columns > narrow.columns,
        "more columns in a wider list"
    );
    assert!(
        tallest(&wide) <= tallest(&narrow),
        "so the long line takes no more rows"
    );
    assert!(wide.code_end + theme::DIFF_CODE_PADDING <= wide.width);
    assert_shaped_rows_fit(&narrow);
    assert_shaped_rows_fit(&wide);
}

/// GPUI's test platform gives every glyph one width; here the Mac's own text system shapes the code, wide
/// characters and all, and the shaped rows are the ones measured.
#[test]
fn with_the_mac_s_own_text_no_shaped_row_of_code_ends_past_the_list() {
    super::real_text::run(|cx| {
        let (cx, desk) = open_large(cx);
        click(cx, "changes-header-0");
        let beside_the_sidebar = layout(cx, &desk);
        assert_shaped_rows_fit(&beside_the_sidebar);
        cx.simulate_keystrokes("cmd-b");
        draw(cx);
        let without_it = layout(cx, &desk);
        assert_shaped_rows_fit(&without_it);
    });
}
