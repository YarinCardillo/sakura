//! The retino behind the sidebar: the prototype's algorithm in numbers, where its cells may and may not
//! be, and when its timer runs.

use super::engine_desk_tests;
use super::engine_rig_tests::{self, wait_until};
use super::retino::{
    hash2, lit_cells, noise2, reach, strip_shift, Cell, Geometry, Retino, STRIP_LENGTH,
};
use gpui::{
    div, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _,
    TestAppContext, VisualTestContext, Window, WindowVisibility,
};
use std::collections::HashSet;
use std::time::{Duration, Instant};

const WINDOW_WIDTH: f32 = 1440.;
const WINDOW_HEIGHT: f32 = 900.;
const SIDEBAR: f32 = 244.;
/// How far the bottom row's dots travel to leave with the sidebar.
const STRIP_END: f32 = STRIP_LENGTH as f32;
const GUTTER: f32 = 8.;
const STRIP: f32 = 24.;
const PANEL_TOP: f32 = 46.;
/// The radius the panel's corners are measured with; the clearance holds for any rounder corner too.
const PANEL_RADIUS: f32 = 10.;
const PANEL_GAP: f32 = 8.;

fn window(page_left: f32) -> Geometry {
    sized(WINDOW_WIDTH, WINDOW_HEIGHT, page_left)
}

/// The windows the corner is checked in, as (width, height) in points: a full-screen Retina laptop at 2560 by
/// 1600 pixels (scale 2), the same pixels counted as points, and three sizes that are not a multiple of the
/// dither's grid in either direction.
const CORNER_WINDOWS: [(f32, f32); 5] = [
    (1280., 800.),
    (2560., 1600.),
    (1440., 900.),
    (1512., 982.),
    (1111., 733.),
];

fn sized(width: f32, height: f32, page_left: f32) -> Geometry {
    Geometry {
        width,
        height,
        column: SIDEBAR,
        strip: STRIP,
        page_left,
        panel_top: PANEL_TOP,
        panel_inset: GUTTER,
        strip_shift: 0.,
    }
}

/// Use the prototype's counts and checksum, adjusted for the parallel edge.
fn fingerprint(cells: &[Cell]) -> (usize, u64) {
    let sum = cells.iter().fold(0u64, |sum, cell| {
        let (column, row) = ((cell.x / 4.) as u64, (cell.y / 4.) as u64);
        let weight = column * 7919 + row * 104_729;
        (sum + weight) % 1_000_000_007
    });
    (cells.len(), sum)
}

fn strip_cells(cells: &[Cell]) -> impl Iterator<Item = &Cell> {
    cells.iter().filter(|cell| cell.y >= WINDOW_HEIGHT - STRIP)
}

#[test]
fn hash_and_noise_agree_with_the_prototype() {
    let cases = [
        ((0, 0), 0.0),
        ((3, 5), 0.094906086822717),
        ((-2, 7), 0.42716584760396875),
        ((100, 200), 0.915023849093128),
    ];
    for ((x, y), expected) in cases {
        assert!((hash2(x, y) - expected).abs() < 1e-12, "hash2({x}, {y})");
    }
    let cases = [
        ((1.25, 2.5), 0.6429185684979412),
        ((3.7, 0.1), 0.12002137609129146),
        ((10.3, 12.9), 0.5248072664537897),
    ];
    for ((x, y), expected) in cases {
        assert!((noise2(x, y) - expected).abs() < 1e-12, "noise2({x}, {y})");
    }
}

#[test]
fn cells_agree_with_the_prototype_with_the_sidebar_open() {
    let cells = lit_cells(&window(SIDEBAR), 3.5);
    assert_eq!(fingerprint(&cells), (1346, 251_070_578));
}

#[test]
fn cells_agree_with_the_prototype_with_the_sidebar_hidden() {
    let cells = lit_cells(&window(GUTTER), 3.5);
    assert_eq!(fingerprint(&cells), (133, 160_829_575));
}

#[test]
fn the_top_third_of_the_window_stays_dark() {
    for t in [0., 9.3, 41.] {
        let cells = lit_cells(&window(SIDEBAR), t);
        assert!(!cells.is_empty());
        assert!(cells.iter().all(|cell| cell.y >= 0.32 * WINDOW_HEIGHT - 4.));
    }
}

/// The lit cells of one band of the window, as grid positions.
fn band(cells: &[Cell], top: f32, bottom: f32) -> HashSet<(i32, i32)> {
    let (top, bottom) = (top * WINDOW_HEIGHT, bottom * WINDOW_HEIGHT);
    let in_band = cells.iter().filter(|cell| (top..bottom).contains(&cell.y));
    in_band
        .map(|cell| ((cell.x / 4.) as i32, (cell.y / 4.) as i32))
        .collect()
}

/// How many cells of `later` stand where a cell of `earlier` stood `rows` rows lower.
fn carried_up(earlier: &HashSet<(i32, i32)>, later: &HashSet<(i32, i32)>, rows: i32) -> usize {
    later
        .iter()
        .filter(|(x, y)| earlier.contains(&(*x, y + rows)))
        .count()
}

#[test]
fn the_mist_drifts_upward_as_time_goes() {
    // 32 px is four Bayer rows twice over: the dither lines up again, so what moved is the noise.
    const ROWS: i32 = 8;
    let seconds = f64::from(ROWS * 4) / 7.;
    let before = band(&lit_cells(&window(SIDEBAR), 0.), 0.5, 0.75);
    let after = band(&lit_cells(&window(SIDEBAR), seconds), 0.5, 0.75);
    let up = carried_up(&before, &after, ROWS);
    let down = carried_up(&before, &after, -ROWS);
    assert!(up > down * 11 / 10, "up {up}, down {down}");
}

#[test]
fn no_cell_stands_over_the_page_panel() {
    for page_left in [SIDEBAR, GUTTER, 120.] {
        let cells = lit_cells(&window(page_left), 5.);
        let above = cells.iter().filter(|cell| cell.y < WINDOW_HEIGHT - STRIP);
        assert!(above.clone().all(|cell| cell.x < page_left));
        assert!(above.count() > 0 || page_left == GUTTER);
    }
}

/// How far the point (`x`, `y`) is from the page panel's rounded rectangle: negative inside it.
fn panel_distance(x: f32, y: f32, page_left: f32) -> f32 {
    let (right, bottom) = (WINDOW_WIDTH - GUTTER, WINDOW_HEIGHT - STRIP);
    let (middle_x, middle_y) = ((page_left + right) / 2., (PANEL_TOP + bottom) / 2.);
    let half_x = (right - page_left) / 2. - PANEL_RADIUS;
    let half_y = (bottom - PANEL_TOP) / 2. - PANEL_RADIUS;
    let outside_x = ((x - middle_x).abs() - half_x).max(0.);
    let outside_y = ((y - middle_y).abs() - half_y).max(0.);
    outside_x.hypot(outside_y) - PANEL_RADIUS
}

#[test]
fn no_part_of_a_dot_comes_within_eight_pixels_of_the_rounded_page_panel() {
    const STEPS: i32 = 12;
    for page_left in [SIDEBAR, GUTTER] {
        for t in [0., 3.5, 17.] {
            let cells = lit_cells(&window(page_left), t);
            assert!(!cells.is_empty());
            for cell in cells {
                // Every point of the 3 px square, a quarter pixel apart.
                for (i, j) in (0..=STEPS).flat_map(|i| (0..=STEPS).map(move |j| (i, j))) {
                    let (x, y) = (cell.x + i as f32 / 4., cell.y + j as f32 / 4.);
                    let distance = panel_distance(x, y, page_left);
                    assert!(
                        distance >= PANEL_GAP - 1e-3,
                        "({x}, {y}) is {distance} from the panel"
                    );
                }
            }
        }
    }
}

/// How far the field's edge stands from the page panel along its straight sides.
const FIELD_GAP: f32 = 12.;

/// How far the point (`x`, `y`) is from the nearest part of `cell`'s square.
fn distance_to_dot(cell: &Cell, (x, y): (f32, f32)) -> f32 {
    let side = f32::from(crate::theme::RETINO_DOT_SIZE);
    let outside = |at: f32, from: f32| (from - at).max(at - (from + side)).max(0.);
    outside(x, cell.x).hypot(outside(y, cell.y))
}

/// The dots trace the panel's rounded corner at the same clearance as its straight sides.
#[test]
fn the_field_s_edge_follows_the_panel_s_rounded_corner_without_a_break() {
    let radius = f32::from(crate::theme::RADIUS_SURFACE);
    let along = radius + FIELD_GAP + 1.5;
    for ((width, height), t) in corner_cases() {
        let bottom = height - STRIP;
        let centre = (SIDEBAR + radius, bottom - radius);
        let cells = lit_cells(&sized(width, height, SIDEBAR), t);
        for degrees in (5..=85).step_by(5) {
            let angle = (degrees as f32).to_radians();
            let point = (
                centre.0 - along * angle.cos(),
                centre.1 + along * angle.sin(),
            );
            let nearest = cells
                .iter()
                .map(|cell| distance_to_dot(cell, point))
                .fold(f32::INFINITY, f32::min);
            assert!(
                nearest <= f32::from(crate::theme::RETINO_DOT_SIZE),
                "{degrees} degrees round the corner at {t} s in {width} by {height}: the nearest dot is {nearest} away"
            );
        }
    }
}

const CORNER_TIMES: [f64; 3] = [0., 3.5, 17.];

/// Every window of [`CORNER_WINDOWS`] with every moment of [`CORNER_TIMES`].
fn corner_cases() -> impl Iterator<Item = ((f32, f32), f64)> {
    CORNER_WINDOWS
        .into_iter()
        .flat_map(|window| CORNER_TIMES.into_iter().map(move |case| (window, case)))
}
/// The grid the dots stand on.
const GRID: f32 = 4.;

/// How far the nearest part of `cell`'s square is from the page panel, its corners rounded as drawn.
fn dot_gap_to_panel(cell: &Cell, (width, height): (f32, f32)) -> f32 {
    let radius = f32::from(crate::theme::RADIUS_SURFACE);
    let side = f32::from(crate::theme::RETINO_DOT_SIZE);
    let (left, right) = (SIDEBAR + radius, width - GUTTER - radius);
    let (top, bottom) = (PANEL_TOP + radius, height - STRIP - radius);
    let beyond = |from: f32, to: f32, low: f32, high: f32| (low - to).max(from - high).max(0.);
    let x = beyond(cell.x, cell.x + side, left, right);
    let y = beyond(cell.y, cell.y + side, top, bottom);
    x.hypot(y) - radius
}

/// The curved edge keeps the dither's spacing without lighting adjacent cells.
#[test]
fn the_field_s_edge_round_the_corner_keeps_the_dither_s_spacing() {
    for ((width, height), t) in corner_cases() {
        let cells = lit_cells(&sized(width, height, SIDEBAR), t);
        let edge: HashSet<(i32, i32)> = cells
            .iter()
            .filter(|cell| dot_gap_to_panel(cell, (width, height)) < FIELD_GAP + GRID)
            .map(|cell| ((cell.x / GRID) as i32, (cell.y / GRID) as i32))
            .collect();
        assert!(!edge.is_empty());
        for &(column, row) in &edge {
            for beside in [(column + 1, row), (column, row + 1)] {
                assert!(
                    !edge.contains(&beside),
                    "at {t} s in {width} by {height} the edge's dots ({column}, {row}) and {beside:?} stand side by side"
                );
            }
        }
    }
}

/// The curved edge meets the straight sides without changing rows or columns.
#[test]
fn the_field_s_edge_meets_the_panel_s_straight_sides_without_a_step() {
    let radius = f32::from(crate::theme::RADIUS_SURFACE);
    let reach = 40.;
    for ((width, height), t) in corner_cases() {
        let bottom = height - STRIP;
        let cells = lit_cells(&sized(width, height, SIDEBAR), t);
        let under_straight = cells.iter().filter(|cell| {
            cell.y > bottom && (SIDEBAR + radius..SIDEBAR + radius + reach).contains(&cell.x)
        });
        let first_row = under_straight
            .map(|cell| cell.y)
            .fold(f32::INFINITY, f32::min);
        let beside_straight = cells.iter().filter(|cell| {
            cell.x < SIDEBAR && (bottom - radius - reach..bottom - radius).contains(&cell.y)
        });
        let first_column = beside_straight
            .map(|cell| cell.x)
            .fold(f32::NEG_INFINITY, f32::max);
        assert!(first_row.is_finite() && first_column.is_finite());
        for cell in &cells {
            let is_under_curve = cell.y > bottom && (SIDEBAR..SIDEBAR + radius).contains(&cell.x);
            assert!(
                !is_under_curve || cell.y >= first_row,
                "at {t} s in {width} by {height} a dot under the curve at ({}, {}) sits above the first row {first_row}",
                cell.x,
                cell.y
            );
            let is_beside_curve = cell.x < SIDEBAR && (bottom - radius..bottom).contains(&cell.y);
            assert!(
                !is_beside_curve || cell.x <= first_column,
                "at {t} s in {width} by {height} a dot beside the curve at ({}, {}) sits right of the first column \
                 {first_column}",
                cell.x,
                cell.y
            );
        }
    }
}

#[test]
fn the_strip_joins_the_column_without_a_seam_or_a_kink() {
    // The density and its slope agree across x = 236, where the column's look hands over to the strip's.
    let step = 0.01;
    let below = reach(STRIP_START as f64 - step);
    let above = reach(STRIP_START as f64 + step);
    assert!((above - below).abs() < 1e-3, "jump {below} to {above}");
    let slope = |x: f64| (reach(x + step) - reach(x - step)) / (2. * step);
    let (before, after) = (
        slope(STRIP_START as f64 - 0.05),
        slope(STRIP_START as f64 + 0.05),
    );
    assert!((before - after).abs() < 1e-4, "slope {before} to {after}");
}

/// Where the bottom row leaves the sidebar column's look, from the window's left edge.
const STRIP_START: f32 = 236.;
const DOTS_GONE: f32 = 364.;

#[test]
fn the_strip_thins_out_to_nothing() {
    let cells = lit_cells(&window(SIDEBAR), 1.7);
    let lit_in = |from: f32, to: f32| {
        let range = STRIP_START + from..STRIP_START + to;
        strip_cells(&cells)
            .filter(|cell| range.contains(&cell.x))
            .count()
    };
    let first = lit_in(0., 100.);
    let second = lit_in(100., 200.);
    let fourth = lit_in(300., 400.);
    assert!(
        first > second && second > fourth,
        "dots thin out: {first} {second} {fourth}"
    );
    assert_eq!(lit_in(DOTS_GONE, WINDOW_WIDTH), 0, "no dot past 600");
}

#[test]
fn the_strip_below_the_panels_free_band_does_not_depend_on_the_panels_left_edge() {
    // The free band hugs the panel, which moves with the sidebar; below it the strip does not depend on it
    // (the strip's own move with the sidebar is its shift, below).
    let below_band = |cell: &&Cell| cell.y >= WINDOW_HEIGHT - STRIP + 2. * PANEL_GAP;
    let open = lit_cells(&window(SIDEBAR), 4.2);
    let hidden = lit_cells(&window(GUTTER), 4.2);
    let open: Vec<_> = strip_cells(&open).filter(below_band).copied().collect();
    let hidden: Vec<_> = strip_cells(&hidden).filter(below_band).copied().collect();
    assert!(!open.is_empty());
    assert!(
        open == hidden,
        "{} cells open, {} hidden",
        open.len(),
        hidden.len()
    );
}

/// The window with the bottom row's dots `shift` points to the left of their place.
fn shifted(page_left: f32, shift: f32) -> Geometry {
    Geometry {
        strip_shift: shift,
        ..window(page_left)
    }
}

#[test]
fn with_the_sidebar_hidden_no_dot_is_left_along_the_bottom() {
    let hidden = lit_cells(&shifted(GUTTER, strip_shift(0.)), 4.2);
    assert_eq!(
        strip_cells(&hidden).count(),
        0,
        "the bottom row left with the sidebar"
    );
    let shown = lit_cells(&shifted(SIDEBAR, strip_shift(1.)), 4.2);
    assert!(strip_cells(&shown).count() > 0, "and is there with it");
}

#[test]
fn the_bottom_rows_dots_go_left_with_the_sidebar_and_its_end_follows_the_sidebar_s_offset() {
    let mut last_end = f32::MAX;
    for share in [1., 0.8, 0.6, 0.4, 0.2, 0.05] {
        let shift = strip_shift(share);
        let cells = lit_cells(&shifted(GUTTER, shift), 4.2);
        let end = strip_cells(&cells).map(|cell| cell.x).fold(0., f32::max);
        assert!(
            end <= STRIP_END - shift + 4.,
            "at {share}: a dot at {end} is past the row's moving end {}",
            STRIP_END - shift
        );
        assert!(
            end <= last_end,
            "the row only goes left as the sidebar does"
        );
        assert!(
            strip_cells(&cells).all(|cell| cell.x >= 0.),
            "none leaves the window"
        );
        last_end = end;
    }
}

#[test]
fn the_dots_keep_their_grid_while_they_move() {
    for shift in [0., 37., 111., 250.5, 599.] {
        let cells = lit_cells(&shifted(GUTTER, shift), 1.3);
        assert!(
            cells
                .iter()
                .all(|cell| cell.x % 4. == 0. && cell.y % 4. == 0.),
            "shift {shift}"
        );
    }
}

#[test]
fn the_shift_is_none_with_the_sidebar_shown_the_row_s_length_with_it_hidden_and_follows_it_between()
{
    assert_eq!(strip_shift(1.), 0.);
    assert_eq!(strip_shift(0.), STRIP_END);
    assert_eq!(strip_shift(0.5), STRIP_END / 2.);
    assert_eq!(strip_shift(-1.), STRIP_END, "never past hidden");
    assert_eq!(strip_shift(2.), 0., "never past shown");
}

/// A test window holding only the retino.
struct Host {
    retino: Option<Entity<Retino>>,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .relative()
            .children(self.retino.clone())
            .child(
                div()
                    .absolute()
                    .top(gpui::px(510.))
                    .left(gpui::px(1.))
                    .size(gpui::px(1.))
                    .bg(gpui::red()),
            )
    }
}

fn open_retino(cx: &mut TestAppContext) -> (Entity<Retino>, &mut VisualTestContext) {
    engine_rig_tests::prepare(cx);
    let slot = std::rc::Rc::new(std::cell::RefCell::new(None));
    let kept = slot.clone();
    let (_, cx) = cx.add_window_view(move |window, cx| {
        let retino = cx.new(|cx| Retino::new(window, cx));
        *kept.borrow_mut() = Some(retino.clone());
        Host {
            retino: Some(retino),
        }
    });
    let retino = slot.borrow_mut().take().expect("the retino");
    (retino, cx)
}

fn draw(cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

fn painted_dots(
    cx: &mut VisualTestContext,
) -> Vec<(gpui::Bounds<gpui::ScaledPixels>, gpui::Background)> {
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
        let scale = window.scale_factor();
        let side = crate::theme::RETINO_DOT_SIZE.scale(scale);
        let sidebar = crate::theme::SIDEBAR_WIDTH.scale(scale);
        window
            .painted_quads()
            .into_iter()
            .filter(|quad| {
                quad.bounds.size == gpui::size(side, side) && quad.bounds.left() < sidebar
            })
            .map(|quad| (quad.bounds, quad.background))
            .collect()
    })
}

#[gpui::test]
fn a_wait_does_not_change_the_painted_dots(cx: &mut TestAppContext) {
    let with_waits = engine_rig_tests::SNAPSHOT;
    let mut without_waits: serde_json::Value = serde_json::from_str(with_waits).unwrap();
    without_waits["waits"] = serde_json::json!([]);
    let (rig, _, cx) = engine_desk_tests::open_unread(cx, &without_waits.to_string());
    wait_until(cx, "the sidebar without waits", |cx| {
        cx.debug_bounds("row-demo").is_some() && cx.debug_bounds("sidebar-inbox-count").is_none()
    });
    let idle = painted_dots(cx);
    assert!(!idle.is_empty());

    rig.say_snapshot(with_waits);
    rig.facts.update(cx, |facts, cx| facts.read(cx));
    wait_until(cx, "the Inbox count", |cx| {
        cx.debug_bounds("sidebar-inbox-count").is_some()
    });
    assert!(painted_dots(cx) == idle, "waiting asks changed the dots");
}

#[gpui::test]
fn the_still_retino_updates_immediately_when_the_accent_or_theme_changes(cx: &mut TestAppContext) {
    use crate::theme::{Appearance, Theme};
    use crate::theme_brand::Accent;
    use std::cell::Cell;
    use std::rc::Rc;

    let (retino, cx) = open_retino(cx);
    draw(cx);
    assert!(!retino.read_with(cx, |retino, _| retino.is_ticking()));
    let updates = Rc::new(Cell::new(0));
    let seen = updates.clone();
    let _subscription =
        cx.update(|_, cx| cx.observe(&retino, move |_, _| seen.set(seen.get() + 1)));
    let dot = |cx: &mut VisualTestContext| {
        let brush = cx.update(|_, cx| Theme::palette(cx).retino_dot.into());
        let dots = painted_dots(cx);
        assert!(!dots.is_empty());
        assert!(dots.iter().all(|(_, painted)| *painted == brush));
        brush
    };
    let pink = dot(cx);

    cx.update(|_, cx| Theme::set_accent(Accent::InstrumentCyan, cx));
    assert_eq!(updates.get(), 1);
    let dark_cyan = dot(cx);
    assert_ne!(dark_cyan, pink);

    cx.update(|_, cx| Theme::set(Appearance::Light, cx));
    assert_eq!(updates.get(), 2);
    assert_ne!(dot(cx), dark_cyan);
}

#[gpui::test]
fn the_retino_stops_when_hidden_and_resumes_when_visible(cx: &mut TestAppContext) {
    let (retino, cx) = open_retino(cx);
    cx.update(|_, cx| cx.set_reduce_motion(false));
    draw(cx);
    assert!(retino.read_with(cx, |retino, _| retino.is_ticking()));

    cx.simulate_visibility_change(WindowVisibility::Hidden);
    assert!(!retino.read_with(cx, |retino, _| retino.is_ticking()));

    cx.simulate_visibility_change(WindowVisibility::Visible);
    assert!(retino.read_with(cx, |retino, _| retino.is_ticking()));
}

#[gpui::test]
fn the_retino_requests_frames_while_no_agent_is_working(cx: &mut TestAppContext) {
    let mut snapshot: serde_json::Value = serde_json::from_str(engine_rig_tests::SNAPSHOT).unwrap();
    snapshot["items"] = serde_json::json!([]);
    snapshot["waits"] = serde_json::json!([]);
    let (_, desk, cx) = engine_desk_tests::open_unread(cx, &snapshot.to_string());
    let retino = desk.read_with(cx, |desk, _| desk.retino.clone());
    let notifications = std::rc::Rc::new(std::cell::Cell::new(0));
    let seen = notifications.clone();
    let _observer = cx.update(|_, cx| {
        cx.set_reduce_motion(false);
        cx.observe(&retino, move |_, _| seen.set(seen.get() + 1))
    });
    draw(cx);
    let step = crate::theme::RETINO_FRAME;
    for _ in 0..=Duration::from_secs(3).as_millis() / step.as_millis() {
        cx.executor().advance_clock(step);
        cx.run_until_parked();
        draw(cx);
    }
    assert!(retino.read_with(cx, |retino, _| retino.is_ticking()));
    notifications.set(0);
    cx.executor().advance_clock(step);
    cx.run_until_parked();
    assert!(notifications.get() > 0);
}

#[gpui::test]
fn the_retino_keeps_drifting_in_a_visible_non_key_window(cx: &mut TestAppContext) {
    let (retino, cx) = open_retino(cx);
    cx.update(|window, cx| {
        cx.set_reduce_motion(false);
        window.activate_window();
    });
    retino.update(cx, |retino, _| retino.set_inputs((SIDEBAR, 0.)));
    draw(cx);
    assert!(retino.read_with(cx, |retino, _| retino.is_ticking()));

    cx.deactivate_window();
    cx.executor().advance_clock(Duration::from_secs(3));
    cx.run_until_parked();
    cx.update(|window, _| assert!(window.is_visible() && !window.is_window_active()));
    assert!(retino.read_with(cx, |retino, _| retino.is_ticking()));
}

#[gpui::test]
fn the_retino_draws_once_and_starts_no_timer_when_motion_is_reduced(cx: &mut TestAppContext) {
    let (retino, cx) = open_retino(cx);
    cx.update(|_, cx| assert!(cx.reduce_motion()));
    retino.update(cx, |retino, cx| {
        retino.set_inputs((SIDEBAR, 0.));
        cx.notify();
    });
    draw(cx);
    assert!(!retino.read_with(cx, |retino, _| retino.is_ticking()));
}

#[gpui::test]
fn the_retino_stops_when_reduce_motion_is_enabled(cx: &mut TestAppContext) {
    let (retino, cx) = open_retino(cx);
    cx.update(|_, cx| cx.set_reduce_motion(false));
    draw(cx);
    assert!(retino.read_with(cx, |retino, _| retino.is_ticking()));

    cx.update(|_, cx| cx.set_reduce_motion(true));
    cx.executor().advance_clock(crate::theme::RETINO_FRAME);
    cx.run_until_parked();
    assert!(!retino.read_with(cx, |retino, _| retino.is_ticking()));
}

#[gpui::test]
fn the_desk_gives_the_retino_the_panels_left_edge_with_the_sidebar_open_and_hidden(
    cx: &mut TestAppContext,
) {
    let (_rig, desk, cx) = engine_desk_tests::open(cx, engine_rig_tests::SNAPSHOT);
    let left = |cx: &mut VisualTestContext| {
        desk.read_with(cx, |desk, cx| desk.retino.read(cx).page_left())
    };
    // The sidebar is 236 wide as in the prototype (it was 244); the retino maths above keep their numbers.
    assert_eq!(left(cx), f32::from(crate::theme::SIDEBAR_WIDTH));
    desk.update(cx, |desk, cx| desk.toggle_sidebar(cx));
    wait_until(cx, "the sidebar hidden", |cx| left(cx) == GUTTER);
}

#[gpui::test]
fn the_desk_takes_the_bottom_rows_dots_away_with_the_sidebar_and_brings_them_back(
    cx: &mut TestAppContext,
) {
    let (_rig, desk, cx) = engine_desk_tests::open(cx, engine_rig_tests::SNAPSHOT);
    let shift = |cx: &mut VisualTestContext| {
        desk.read_with(cx, |desk, cx| desk.retino.read(cx).strip_shift())
    };
    assert_eq!(shift(cx), 0.);
    desk.update(cx, |desk, cx| desk.toggle_sidebar(cx));
    wait_until(cx, "the row gone", |cx| shift(cx) == STRIP_END);
    desk.update(cx, |desk, cx| desk.toggle_sidebar(cx));
    wait_until(cx, "the row back", |cx| shift(cx) == 0.);
}

#[test]
fn a_frame_of_cells_costs_little() {
    let geometry = window(SIDEBAR);
    let started = Instant::now();
    let rounds = 20;
    let mut cells = 0;
    for round in 0..rounds {
        cells = lit_cells(&geometry, f64::from(round)).len();
    }
    let each = started.elapsed() / rounds;
    eprintln!(
        "lit_cells: {each:?} for {cells} cells in a {WINDOW_WIDTH} by {WINDOW_HEIGHT} window"
    );
    assert!(each < Duration::from_millis(40), "{each:?}");
}

/// A test window with nothing in it: what a draw costs on its own.
struct Empty;

impl Render for Empty {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full()
    }
}

/// The time of one draw of the window, over `rounds` draws, `before` each of them.
fn time_draws(
    cx: &mut VisualTestContext,
    mut before: impl FnMut(&mut VisualTestContext),
) -> Duration {
    let rounds = 20;
    let started = Instant::now();
    for _ in 0..rounds {
        before(cx);
        draw(cx);
    }
    started.elapsed() / rounds
}

#[gpui::test]
fn a_paint_of_the_retino_costs_little(cx: &mut TestAppContext) {
    let (retino, cx) = open_retino(cx);
    retino.update(cx, |retino, cx| {
        retino.set_inputs((SIDEBAR, 0.));
        cx.notify();
    });
    draw(cx);
    let with = time_draws(cx, |cx| retino.update(cx, |_, cx| cx.notify()));
    let (empty, empty_cx) = cx.add_window_view(|_, _| Empty);
    let _ = empty;
    draw(empty_cx);
    let without = time_draws(empty_cx, |_| {});
    eprintln!("one draw of a window: {without:?} empty, {with:?} with the retino");
    assert!(
        with.saturating_sub(without) < Duration::from_millis(250),
        "{with:?} {without:?}"
    );
}

#[gpui::test]
fn retino_frames_group_the_dots_under_the_foreground(cx: &mut TestAppContext) {
    let (retino, cx) = open_retino(cx);
    cx.simulate_resize(gpui::size(gpui::px(WINDOW_WIDTH), gpui::px(WINDOW_HEIGHT)));
    retino.update(cx, |retino, cx| {
        retino.set_inputs((SIDEBAR, 0.));
        cx.notify();
    });
    draw(cx);
    let with = time_draws(cx, |cx| retino.update(cx, |_, cx| cx.notify()));
    eprintln!("retino paint with cached cells at 1440x900: {with:?}");
    cx.update(|window, _| {
        let quads = window.painted_quads();
        assert!(quads.len() > 1000);
        let overlay = quads
            .iter()
            .find(|quad| quad.background == gpui::red().into())
            .unwrap();
        assert!(
            quads
                .iter()
                .filter(|quad| quad.background != overlay.background)
                .all(|dot| dot.order < overlay.order),
            "the overlay shares the dots' draw order"
        );
    });
}

#[gpui::test]
fn a_retino_beat_reuses_the_inbox_foreground(cx: &mut TestAppContext) {
    let mut snapshot: serde_json::Value = serde_json::from_str(engine_rig_tests::SNAPSHOT).unwrap();
    snapshot["items"] = serde_json::json!([]);
    let (_, desk, cx) = engine_desk_tests::open(cx, &snapshot.to_string());
    cx.simulate_resize(gpui::size(gpui::px(WINDOW_WIDTH), gpui::px(WINDOW_HEIGHT)));
    cx.update(|_, cx| cx.set_reduce_motion(false));
    draw(cx);
    let before = painted_dots(cx);
    let renders = desk.read_with(cx, |desk, _| desk.renders);

    cx.executor().advance_clock(Duration::from_secs(1));
    cx.run_until_parked();
    assert_ne!(painted_dots(cx), before);
    assert!(cx.debug_bounds("inbox").is_some());
    assert_eq!(desk.read_with(cx, |desk, _| desk.renders), renders);
}

#[test]
fn audit_retino_has_similar_contrast_in_both_themes() {
    use crate::theme::{Appearance, Palette};
    for accent in crate::theme_brand::Accent::ALL {
        let dark = Palette::of(Appearance::Dark, accent);
        let light = Palette::of(Appearance::Light, accent);
        let contrast = crate::theme_contrast::contrast;
        let dark_ratio = contrast(dark.retino_dot, dark.canvas);
        let light_ratio = contrast(light.retino_dot, light.canvas);
        if accent == crate::theme_brand::Accent::InstrumentCyan {
            assert!((dark_ratio - 4.3).abs() < 0.1);
        } else {
            assert!(
                (dark_ratio - light_ratio).abs() < 0.45,
                "{accent:?}: {dark_ratio}, {light_ratio}"
            );
        }
    }
}
