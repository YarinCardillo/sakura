//! The layout grid, checked on the bounds GPUI lays out in a test window: the left edges that must coincide,
//! the right edges, the centre lines, the row and control heights, the glyph and icon sizes. Each check runs
//! at the default and at the minimum window size, with each pane open and closed. A view that drifts from
//! the tokens of `theme.rs` ("Layout grid") fails here.

use super::conversation_tests::{ask_a_question, QUESTION};
use super::interaction_tests::{click, draw, open_with, wait_for};
use super::real_text;
use crate::theme;
use gpui::{px, size, Bounds, Pixels, TestAppContext, VisualTestContext};

/// How far two edges that must coincide may be apart.
const HALF_A_POINT: f32 = 0.5;
const TASK: &str = "row-sample-accent";
/// The right pane's row of views, in the order it shows them.
const PANE_VIEWS: [&str; 4] = [
    "pane-view-files",
    "pane-view-changes",
    "pane-view-checks",
    "pane-view-git",
];
const SIZES: [(f32, f32); 2] = [(1440., 900.), (1100., 700.)];

/// Which panes are open while a check runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Panes {
    Both,
    NoSidebar,
    NoChanges,
}

const PANES: [Panes; 3] = [Panes::Both, Panes::NoSidebar, Panes::NoChanges];

/// The bounds of `selector`, once it is on screen (what a view reads on another thread arrives a frame
/// or two after the click that asks for it).
pub(super) fn bounds(cx: &mut VisualTestContext, selector: &'static str) -> Bounds<Pixels> {
    wait_for(cx, selector, |cx| cx.debug_bounds(selector).is_some());
    cx.debug_bounds(selector).expect(selector)
}

#[track_caller]
fn assert_near(found: Pixels, wanted: Pixels, what: &str) {
    let apart = f32::from(found - wanted).abs();
    assert!(
        apart <= HALF_A_POINT,
        "{what}: {found:?}, wanted {wanted:?} ({apart} apart)"
    );
}

fn centre_y(bounds: Bounds<Pixels>) -> Pixels {
    bounds.top() + bounds.size.height / 2.
}

/// The panel's outer edges: its ring is around the header.
fn panel_edges(cx: &mut VisualTestContext) -> (Pixels, Pixels) {
    let header = bounds(cx, "page-header");
    (
        header.left() - theme::PANEL_RING_WIDTH,
        header.right() + theme::PANEL_RING_WIDTH,
    )
}

/// The desk on the fixture with the task's page drawn, its panes moving at once.
fn open(cx: &mut TestAppContext) -> &mut VisualTestContext {
    let (cx, _, _) = open_with(cx, None);
    cx.update(|_, cx| cx.set_reduce_motion(true));
    wait_for(cx, "the rows", |cx| cx.debug_bounds(TASK).is_some());
    click(cx, TASK);
    wait_for(cx, "the changes", |cx| {
        cx.debug_bounds("changes-header-1").is_some()
    });
    cx
}

fn key(cx: &mut VisualTestContext, keys: &str) {
    cx.simulate_keystrokes(keys);
    draw(cx);
}

/// Runs `check` at each window size with each pane open and closed, on the Agente view.
fn at_each_size_and_pane(
    cx: &mut VisualTestContext,
    check: impl Fn(&mut VisualTestContext, Panes),
) {
    for (width, height) in SIZES {
        cx.simulate_resize(size(px(width), px(height)));
        draw(cx);
        for panes in PANES {
            let toggle = match panes {
                Panes::Both => None,
                Panes::NoSidebar => Some("cmd-b"),
                Panes::NoChanges => Some("alt-cmd-b"),
            };
            toggle.into_iter().for_each(|keys| key(cx, keys));
            check(cx, panes);
            toggle.into_iter().for_each(|keys| key(cx, keys));
        }
    }
}

#[test]
fn the_traffic_lights_sit_on_the_sidebar_inset_and_on_the_tabs_centre_line() {
    assert_eq!(theme::TRAFFIC_LIGHTS_X, theme::SIDEBAR_INSET);
    assert_eq!(theme::TRAFFIC_LIGHT_SIZE, px(14.));
    let tabs_centre = px((theme::tabs::TOP_INSET + theme::tabs::STRIP_HEIGHT) / 2.);
    assert_eq!(
        theme::TRAFFIC_LIGHTS_Y + theme::TRAFFIC_LIGHT_SIZE / 2.,
        tabs_centre,
        "a traffic light, centred on the line the tabs are centred on"
    );
    assert_eq!(
        theme::HEADER_CENTRE_Y,
        theme::WINDOW_STRIP_HEIGHT + theme::HEADER_HEIGHT / 2.
    );
}

#[test]
fn the_status_bar_is_one_row_at_the_window_s_foot_with_its_toggles_on_the_insets() {
    real_text::run(|cx| {
        let cx = open(cx);
        at_each_size_and_pane(cx, |cx, _| {
            let window = cx.update(|window, _| window.viewport_size());
            let bar = bounds(cx, "status-bar");
            assert_eq!(bar.size.height, theme::STATUS_BAR_HEIGHT, "one row");
            assert_near(bar.bottom(), window.height, "at the window's foot");
            assert_near(bar.left(), px(0.), "from the window's left edge");
            assert_near(bar.right(), window.width, "to its right edge");
            let line = window.height - theme::STATUS_BAR_HEIGHT / 2.;
            for selector in [
                "toggle-sidebar",
                "toggle-sidebar-icon",
                "status-terminal",
                "status-terminal-icon",
                "toggle-changes",
                "toggle-changes-icon",
                "status-line",
            ] {
                assert_near(centre_y(bounds(cx, selector)), line, selector);
            }
            let control = size(theme::STATUS_CONTROL_HEIGHT, theme::STATUS_CONTROL_HEIGHT);
            for selector in ["toggle-sidebar", "status-terminal", "toggle-changes"] {
                assert_eq!(bounds(cx, selector).size, control, "{selector}");
            }
            // The right end is a group, as in Zed's bar: its items one tight gap apart, the pane's
            // toggle last.
            let (terminal, toggle) = (bounds(cx, "status-terminal"), bounds(cx, "toggle-changes"));
            assert_near(
                toggle.left() - terminal.right(),
                theme::GAP_TIGHT,
                "between items",
            );
            let icon = bounds(cx, "status-terminal-icon");
            assert_eq!(icon.size, size(theme::ICON_SIZE, theme::ICON_SIZE));
            let sidebar = bounds(cx, "toggle-sidebar-icon");
            assert_near(sidebar.left(), theme::SIDEBAR_INSET, "the sidebar's inset");
            assert_eq!(sidebar.size, size(theme::ICON_SIZE, theme::ICON_SIZE));
            let pane = bounds(cx, "toggle-changes-icon");
            let right_line = window.width - theme::CONTENT_INSET;
            assert_near(pane.right(), right_line, "the window's right content inset");
            assert_eq!(pane.size, size(theme::ICON_SIZE, theme::ICON_SIZE));
            let sentence = bounds(cx, "status-line");
            let names = theme::SIDEBAR_INSET + theme::ICON_SIZE + theme::ROW_GAP;
            assert_near(sentence.left(), names, "where the sidebar's names start");
            assert!(
                sentence.right() <= terminal.left(),
                "the sentence ends before the items at the right"
            );
        });
    });
}

#[test]
fn the_panel_reaches_the_window_s_right_edge_and_keeps_its_inset_at_the_left() {
    real_text::run(|cx| {
        let cx = open(cx);
        at_each_size_and_pane(cx, |cx, panes| {
            let window = cx.update(|window, _| window.viewport_size());
            let (left, right) = panel_edges(cx);
            assert_near(right, window.width, "no inset at the window's right edge");
            let sidebar = match panes {
                Panes::NoSidebar => theme::PANEL_INSET,
                Panes::Both | Panes::NoChanges => theme::SIDEBAR_WIDTH,
            };
            assert_near(left, sidebar, "the panel's left edge");
            let strip = bounds(cx, "tab-strip");
            assert_near(
                strip.right(),
                window.width,
                "the title row runs to the edge",
            );
        });
    });
}

#[test]
fn the_window_strip_holds_nothing_and_the_panel_ends_where_the_status_bar_starts() {
    real_text::run(|cx| {
        let cx = open(cx);
        at_each_size_and_pane(cx, |cx, panes| {
            let window = cx.update(|window, _| window.viewport_size());
            let foot = window.height - theme::STATUS_BAR_HEIGHT;
            for selector in ["toggle-sidebar", "toggle-changes"] {
                let toggle = bounds(cx, selector);
                assert!(toggle.top() >= foot, "{selector} is in the status bar");
            }
            if panes != Panes::NoChanges {
                let list = bounds(cx, "changes-body");
                assert_near(
                    list.bottom(),
                    foot - theme::PANEL_RING_WIDTH,
                    "the panel's content ends on the bar, inside its ring",
                );
            }
        });
    });
}

#[test]
fn the_left_edges_of_the_sidebar_are_the_traffic_lights_edge() {
    real_text::run(|cx| {
        let cx = open(cx);
        at_each_size_and_pane(cx, |cx, panes| {
            if panes == Panes::NoSidebar {
                return;
            }
            let line = theme::TRAFFIC_LIGHTS_X;
            for selector in [
                "group-review",
                "glyph-sample-accent",
                "glyph-sample-folders",
                "glyph-headings-26",
                "glyph-broken-check",
            ] {
                assert_near(bounds(cx, selector).left(), line, selector);
            }
            let fill = bounds(cx, TASK);
            assert_near(
                fill.left() + theme::ROW_PADDING_X,
                line,
                "the row's content",
            );
            let names = ["label-sample-accent", "label-sample-folders"];
            let starts: Vec<_> = names.map(|name| bounds(cx, name).left()).to_vec();
            assert_near(starts[0], starts[1], "the names start on one line");
            assert_near(
                starts[0],
                line + theme::ICON_SIZE + theme::ROW_GAP,
                "one icon and a gap after the glyphs",
            );
        });
    });
}

#[test]
fn the_left_edges_of_the_panel_are_its_content_inset() {
    real_text::run(|cx| {
        let cx = open(cx);
        at_each_size_and_pane(cx, |cx, panes| {
            let (left, _) = panel_edges(cx);
            let line = left + theme::CONTENT_INSET;
            assert_near(bounds(cx, "header-glyph").left(), line, "the header");
            if panes != Panes::NoChanges {
                let pane = bounds(cx, "changes-header-0");
                let chevron = bounds(cx, "changes-chevron");
                assert_near(
                    chevron.left() - pane.left(),
                    theme::SIDE_PANE_INSET,
                    "the changes pane",
                );
            }
            if panes == Panes::NoChanges {
                return;
            }
            // The right pane's own left line: its first view, the commit picker's words, the chevrons
            // of the changed files and the rows of the checks all start on it.
            let pane = bounds(cx, "right-pane").left() + theme::SIDE_PANE_INSET;
            assert_near(bounds(cx, "pane-view-files").left(), pane, "the first view");
            let picker = bounds(cx, "commit-picker-label");
            assert_near(picker.left(), pane, "the toolbar's words");
            click(cx, "pane-view-checks");
            let row = bounds(cx, "verify-test");
            assert_near(row.left() + theme::ROW_PADDING_X, pane, "Verifiche");
            click(cx, "pane-view-changes");
        });
    });
}

#[test]
fn the_right_edges_are_one_line_from_the_header_to_the_status_bar() {
    real_text::run(|cx| {
        let cx = open(cx);
        at_each_size_and_pane(cx, |cx, panes| {
            let (_, right) = panel_edges(cx);
            let line = right - theme::CONTENT_INSET;
            let toggle = bounds(cx, "toggle-changes-icon").right();
            assert_near(toggle, line, "the status bar");
            assert_near(bounds(cx, "task-menu-icon").right(), line, "the overflow");
            if panes != Panes::NoChanges {
                assert_near(bounds(cx, "changes-trailing").right(), line, "the changes");
            }
            if panes != Panes::NoSidebar {
                let trailing = bounds(cx, "trailing-sample-accent");
                let sidebar_line = theme::SIDEBAR_WIDTH - theme::SIDEBAR_INSET;
                assert_near(trailing.right(), sidebar_line, "the sidebar's figures");
            }
            if panes == Panes::NoChanges {
                return;
            }
            click(cx, "pane-view-checks");
            assert_near(bounds(cx, "run-test").right(), line, "Verifiche");
            click(cx, "pane-view-changes");
        });
    });
}

#[test]
fn the_new_lavoro_button_sits_in_the_sidebar_s_header_on_its_right_inset() {
    real_text::run(|cx| {
        let cx = open(cx);
        at_each_size_and_pane(cx, |cx, panes| {
            if panes == Panes::NoSidebar {
                return;
            }
            let line = theme::SIDEBAR_WIDTH - theme::SIDEBAR_INSET;
            let (button, icon) = (bounds(cx, "new-task"), bounds(cx, "new-task-icon"));
            assert_near(
                icon.right(),
                line,
                "the plus ends on the sidebar's right inset",
            );
            assert_near(
                centre_y(icon),
                theme::HEADER_CENTRE_Y,
                "on the header's line",
            );
            assert_eq!(icon.size, size(theme::ICON_SIZE, theme::ICON_SIZE));
            let control = size(theme::CONTROL_HEIGHT, theme::CONTROL_HEIGHT);
            assert_eq!(button.size, control, "one control high and wide");
            assert_near(centre_y(button), theme::HEADER_CENTRE_Y, "its fill too");
            let figures = bounds(cx, "trailing-sample-accent");
            assert_near(
                figures.right(),
                line,
                "the rows' figures end on the same line",
            );
            let fill = bounds(cx, TASK);
            assert_near(
                fill.right() - theme::ROW_PADDING_X,
                line,
                "and so does a row's content",
            );
            let label = bounds(cx, "group-review");
            assert!(
                label.right() < button.left(),
                "the first label ends before it"
            );
        });
    });
}

#[test]
fn the_header_and_the_sidebars_first_line_share_one_centre_line() {
    real_text::run(|cx| {
        let cx = open(cx);
        at_each_size_and_pane(cx, |cx, panes| {
            let line = theme::HEADER_CENTRE_Y;
            for selector in [
                "header-glyph",
                "header-state",
                "centre-view-agent",
                "header-branch",
                "take",
                "task-menu",
                "task-menu-icon",
            ] {
                assert_near(centre_y(bounds(cx, selector)), line, selector);
            }
            if panes != Panes::NoSidebar {
                // What is at work is listed before what is to review: its label is the first line.
                let label = bounds(cx, "group-working");
                assert_near(centre_y(label), line, "the sidebar's first line");
            }
        });
    });
}

#[test]
fn rows_start_where_the_header_ends_and_keep_one_pitch_across_the_window() {
    real_text::run(|cx| {
        let cx = open(cx);
        at_each_size_and_pane(cx, |cx, panes| {
            let start = theme::WINDOW_STRIP_HEIGHT + theme::HEADER_HEIGHT;
            let header = bounds(cx, "page-header");
            assert_near(header.bottom(), start, "the header's hairline ends there");
            if panes != Panes::NoChanges {
                let files = ["changes-header-0", "changes-header-1"].map(|file| bounds(cx, file));
                // Under the pane's row of views and its toolbar: two rows down, on the sidebar's lines.
                let under = start + theme::PANE_VIEWS_HEIGHT + theme::TOOLBAR_HEIGHT;
                assert_near(files[0].top(), under, "the first changed file");
                assert_near(files[1].top() - files[0].top(), theme::ROW_HEIGHT, "pitch");
                assert_eq!(files[0].size.height, theme::ROW_HEIGHT);
            }
            if panes != Panes::NoSidebar {
                let first = bounds(cx, "row-sample-folders");
                let rows = [
                    "row-sample-folders",
                    "row-sample-accent",
                    "row-headings-26",
                    "row-broken-check",
                ]
                .map(|row| bounds(cx, row));
                let top = rows
                    .iter()
                    .map(|row| row.top())
                    .fold(first.top(), Pixels::min);
                assert_near(top, start, "the sidebar's first row");
                for row in rows {
                    assert_eq!(row.size.height, theme::ROW_HEIGHT);
                    let rows_down = f32::from(row.top() - start) / f32::from(theme::ROW_HEIGHT);
                    assert!(
                        (rows_down - rows_down.round()).abs() < 0.02,
                        "a sidebar row off the pitch: {rows_down} rows down"
                    );
                }
            }
            if panes == Panes::NoChanges {
                return;
            }
            // The right pane: its row of views and its toolbar are a row each, so its files start two
            // rows under the header, on the lines of the sidebar's rows.
            let views = bounds(cx, "pane-views");
            assert_near(
                views.top(),
                start,
                "the row of views starts where the header ends",
            );
            assert_eq!(views.size.height, theme::PANE_VIEWS_HEIGHT);
            for view in PANE_VIEWS {
                assert_near(centre_y(bounds(cx, view)), centre_y(views), view);
            }
            // A count is smaller text in the same pill: its line box is centred to the device pixel.
            let count = centre_y(bounds(cx, "pane-count-changes")) - centre_y(views);
            assert!(
                count.abs() <= gpui::px(1.),
                "the count is {count:?} off the row's centre"
            );
            let file = bounds(cx, "changes-header-0");
            assert_eq!(file.size.height, theme::ROW_HEIGHT);
            let under = start + theme::PANE_VIEWS_HEIGHT + theme::TOOLBAR_HEIGHT;
            assert_near(file.top(), under, "under the views and the toolbar");
        });
    });
}

#[test]
fn every_control_of_the_header_has_the_one_control_height() {
    real_text::run(|cx| {
        let cx = open(cx);
        at_each_size_and_pane(cx, |cx, panes| {
            for selector in ["take", "task-menu"] {
                let height = bounds(cx, selector).size.height;
                assert_eq!(height, theme::CONTROL_HEIGHT, "{selector}");
            }
            if panes != Panes::NoChanges {
                for view in PANE_VIEWS {
                    let height = bounds(cx, view).size.height;
                    assert_eq!(
                        height,
                        theme::STATUS_CONTROL_HEIGHT,
                        "{view}: a bar's control"
                    );
                }
            }
            if panes == Panes::NoSidebar {
                return;
            }
            click(cx, "row-sample-folders");
            for selector in ["header-stop", "hold-sample-folders"] {
                let height = bounds(cx, selector).size.height;
                assert_eq!(height, theme::CONTROL_HEIGHT, "{selector}");
            }
            click(cx, TASK);
        });
    });
}

#[test]
fn every_state_glyph_and_every_icon_has_its_one_size() {
    real_text::run(|cx| {
        let cx = open(cx);
        let mark = size(theme::MARK_SIZE, theme::MARK_SIZE);
        let icon = size(theme::ICON_SIZE, theme::ICON_SIZE);
        for selector in [
            "header-glyph",
            "glyph-sample-accent",
            "glyph-sample-folders",
            "glyph-headings-26",
            "glyph-broken-check",
            "changes-chevron",
        ] {
            assert_eq!(bounds(cx, selector).size, mark, "{selector}");
        }
        for selector in ["toggle-sidebar-icon", "toggle-changes-icon"] {
            assert_eq!(bounds(cx, selector).size, icon, "{selector}");
        }
        assert_eq!(
            bounds(cx, "task-menu-icon").size.width,
            theme::ICON_SIZE,
            "the overflow's dots are one icon wide"
        );
        click(cx, "pane-view-checks");
        assert_eq!(bounds(cx, "verify-glyph").size, mark, "a check's glyph");
    });
}

/// The width of `text` at the interface's body size in `family`, with `features`.
#[test]
fn the_conversation_is_one_column_on_the_content_inset_and_its_controls_are_the_grid_s() {
    real_text::run(|cx| {
        let (cx, desk, _) = open_with(cx, None);
        cx.update(|_, cx| cx.set_reduce_motion(true));
        wait_for(cx, "the rows", |cx| cx.debug_bounds(TASK).is_some());
        click(cx, TASK);
        wait_for(cx, "the conversation", |cx| {
            cx.debug_bounds("thread-composer-box").is_some()
        });
        ask_a_question(cx, &desk, "sample-accent");
        let tokens = theme::thread::COLUMN_PADDING_X;
        at_each_size_and_pane(cx, |cx, _| {
            let centre = bounds(cx, "centre");
            // One column, centred and no wider than the reading width; its content is inset by the
            // content inset, so in a pane no wider than the column it starts on the header's line.
            let column = centre.size.width.min(theme::thread::COLUMN_MAX_WIDTH);
            let left = centre.left() + (centre.size.width - column) / 2. + tokens;
            let right = left + column - tokens * 2.;
            for selector in ["thread-prompt", QUESTION[0], "thread-composer-box"] {
                let part = bounds(cx, selector);
                assert_near(part.left(), left, &format!("{selector}'s left edge"));
                assert_near(part.right(), right, &format!("{selector}'s right edge"));
            }
            if centre.size.width <= theme::thread::COLUMN_MAX_WIDTH {
                let header = bounds(cx, "header-glyph");
                assert_near(left, header.left(), "the header's content line");
            }
            let composer = bounds(cx, "thread-composer-box");
            assert_near(
                composer.bottom(),
                centre.bottom() - tokens,
                "the composer's foot",
            );
            let send = bounds(cx, "thread-send");
            assert_eq!(
                send.size,
                size(theme::CONTROL_HEIGHT, theme::CONTROL_HEIGHT)
            );
            assert_near(
                send.right(),
                composer.right() - theme::thread::COMPOSER_PADDING,
                "the send button, inside the composer",
            );
            // The card's choices: the grid's control height, in the order of their cost from the left.
            let card = bounds(cx, QUESTION[0]);
            let choices: Vec<_> = QUESTION[1..]
                .iter()
                .map(|choice| bounds(cx, choice))
                .collect();
            assert_near(
                choices[0].left(),
                card.left() + theme::thread::CARD_PADDING,
                "the first choice, on the card's inset",
            );
            for (choice, name) in choices.iter().zip(&QUESTION[1..]) {
                assert_eq!(choice.size.height, theme::CONTROL_HEIGHT, "{name}");
                assert!(choice.right() <= card.right() - theme::thread::CARD_PADDING);
            }
            for pair in choices.windows(2) {
                assert_near(
                    pair[1].left() - pair[0].right(),
                    theme::thread::BUTTON_GAP,
                    "one gap between two choices",
                );
            }
        });
    });
}

fn shaped_width(
    window: &gpui::Window,
    text: &'static str,
    family: &'static str,
    (weight, features): (gpui::FontWeight, gpui::FontFeatures),
) -> f32 {
    let font = gpui::Font {
        family: family.into(),
        weight,
        features,
        ..gpui::Font::default()
    };
    let run = gpui::TextRun {
        len: text.len(),
        font,
        color: gpui::Hsla::default(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let line = window
        .text_system()
        .shape_line(text.into(), theme::TEXT_BODY, &[run], None);
    f32::from(line.width)
}

#[test]
fn the_interface_is_set_in_the_mac_s_system_font_in_three_weights_with_tabular_figures() {
    real_text::run(|cx| {
        let cx = open(cx);
        cx.update(|window, _| {
            let plain = || gpui::FontFeatures::default();
            let width =
                |family, weight| shaped_width(window, "Lavoro 2026", family, (weight, plain()));
            let regular = width(theme::FONT_UI, gpui::FontWeight::NORMAL);
            assert_eq!(
                regular,
                width(".AppleSystemUIFont", gpui::FontWeight::NORMAL),
                "the interface's family is the system's own"
            );
            assert_ne!(
                regular,
                width("Helvetica", gpui::FontWeight::NORMAL),
                "not a fallback"
            );
            assert_ne!(regular, width(theme::FONT_MONO, gpui::FontWeight::NORMAL));
            let medium = width(theme::FONT_UI, theme::WEIGHT_MEDIUM);
            let semibold = width(theme::FONT_UI, gpui::FontWeight::SEMIBOLD);
            assert!(
                regular < medium && medium < semibold,
                "three weights, three faces: {regular} {medium} {semibold}"
            );
            assert_eq!(
                width(theme::FONT_UI, gpui::FontWeight::MEDIUM),
                regular,
                "at exactly 500 GPUI's matching takes the regular face: the theme's medium is why"
            );
            let figures = |text, features| {
                shaped_width(
                    window,
                    text,
                    theme::FONT_UI,
                    (gpui::FontWeight::NORMAL, features),
                )
            };
            assert!(
                figures("1111", plain()) < figures("0000", plain()),
                "the system font's own figures are proportional"
            );
            assert_eq!(
                figures("1111", crate::ui::chrome::tabular()),
                figures("0000", crate::ui::chrome::tabular()),
                "with tabular figures every digit is as wide"
            );
        });
    });
}

/// The composer's one line is on its button's centre: the field is as high as the button beside it.
#[test]
fn the_composer_s_field_is_as_high_as_its_button_and_on_its_centre() {
    real_text::run(|cx| {
        let cx = open(cx);
        wait_for(cx, "the conversation", |cx| {
            cx.debug_bounds("thread-composer-box").is_some()
        });
        let field = bounds(cx, "thread-composer");
        let send = bounds(cx, "thread-send");
        assert_eq!(field.size.height, send.size.height, "the field's height");
        assert_near(centre_y(field), centre_y(send), "the field and the button");
    });
}
