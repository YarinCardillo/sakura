//! Play in the desk, on the real chain of reads with a fake service script: the header's control
//! starts and stops it, the port is a link, the right pane's view shows the log, the status bar says
//! it runs, and at rest nothing is asked. A project without a run command and a lavoro in a box say
//! why they cannot play; the sample data has no Play at all.

use super::desk::Desk;
use super::interaction_tests::{click, draw, open_with, wait_for};
use super::launch_tests::{calls, fake_agentbox, open_live, Names};
use super::right_pane::PaneView;
use gpui::{Entity, TestAppContext, VisualTestContext};
use std::path::PathBuf;
use std::time::Duration;

/// The lavoro these tests put on the Mac, and one that stays in a box.
const ON_THE_MAC: &str = "headings-26";
const IN_A_BOX: &str = "sample-accent";

/// What the fake service answers for the projects and for Play, in the shapes the service prints.
const CASES: &str = r#"  projects) cat "$d/projects.json" ;;
  play) case "$3" in
    log) printf 'ready on http://localhost:56000\n' ;;
    start) touch "$d/playing"
      printf '{"task":"%s","running":true,"pid":4711,"ports":[56000,56001],"since":null}' "$2" ;;
    stop) rm -f "$d/playing"
      printf '{"task":"%s","running":false,"pid":null,"ports":[56000,56001],"since":null}' "$2" ;;
    status) printf '{"task":"%s","running":false,"pid":null,"ports":[],"since":null}' "$2" ;;
  esac ;;"#;

/// A service whose one project runs `run`, with [`ON_THE_MAC`] living on the Mac.
fn service(run: Option<&str>) -> (tempfile::TempDir, PathBuf) {
    let (dir, script) = fake_agentbox(Names::Listed, "0", CASES);
    let on_the_mac = |task: &mut serde_json::Value| {
        if task["name"] == ON_THE_MAC {
            task["where"] = "mac".into();
        }
    };
    let rewrite = |name: &str, change: &dyn Fn(&mut serde_json::Value)| {
        let path = dir.path().join(name);
        let text = std::fs::read_to_string(&path).expect("an answer");
        let mut answer: serde_json::Value = serde_json::from_str(&text).expect("json");
        change(&mut answer);
        std::fs::write(path, answer.to_string()).expect("an answer");
    };
    rewrite("status.json", &|status| {
        let tasks = status["tasks"].as_array_mut().expect("the tasks");
        tasks.iter_mut().for_each(on_the_mac);
    });
    rewrite(&format!("facts-{ON_THE_MAC}.json"), &on_the_mac);
    let projects = serde_json::json!([
        { "name": "example-app", "base": "main", "where": "mac", "run": run }
    ]);
    std::fs::write(dir.path().join("projects.json"), projects.to_string()).expect("the projects");
    (dir, script)
}

fn is_on_screen(cx: &mut VisualTestContext, selector: &'static str) -> bool {
    cx.debug_bounds(selector).is_some()
}

/// Waits until the Play of [`ON_THE_MAC`] is on screen and the service said where it stands.
fn wait_for_play(cx: &mut VisualTestContext, desk: &Entity<Desk>) {
    wait_for(cx, "where Play stands", |cx| {
        let is_at_rest = desk.read_with(cx, |desk, cx| {
            let play = desk.plays.of(ON_THE_MAC);
            play.is_some_and(|play| play.read(cx).is_at_rest())
        });
        is_at_rest && is_on_screen(cx, "play-control")
    });
}

/// Waits for the port's link after a press on the control; says what was asked when it does not come.
fn wait_for_port(cx: &mut VisualTestContext, desk: &Entity<Desk>, dir: &tempfile::TempDir) {
    // Waits on the link itself; the deadline only bounds a hang on a busy machine.
    let deadline = std::time::Instant::now() + super::interaction_tests::HANG;
    while std::time::Instant::now() < deadline {
        cx.run_until_parked();
        draw(cx);
        if is_on_screen(cx, "play-port") {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let (is_at_rest, is_running) = desk.read_with(cx, |desk, cx| {
        let play = desk.plays.of(ON_THE_MAC).expect("the Play").read(cx);
        (play.is_at_rest(), play.is_running())
    });
    panic!(
        "no port: at rest {is_at_rest}, running {is_running}, asked {:?}",
        calls(dir)
    );
}

/// How many times the service was asked something that starts with `start`.
fn asked(dir: &tempfile::TempDir, start: &str) -> usize {
    let calls = calls(dir);
    calls.iter().filter(|call| call.starts_with(start)).count()
}

#[gpui::test]
fn play_starts_and_stops_from_the_header_and_nothing_is_asked_at_rest(cx: &mut TestAppContext) {
    let (dir, script) = service(Some("npm run dev"));
    let (cx, desk, _) = open_live(cx, script, Some(ON_THE_MAC), false);
    wait_for_play(cx, &desk);
    assert_eq!(asked(&dir, "play"), 1, "{:?}", calls(&dir));
    assert_eq!(asked(&dir, "play headings-26 status --json"), 1);
    assert!(!is_on_screen(cx, "play-port"), "not running: no port");
    assert!(!is_on_screen(cx, "status-playing"));

    click(cx, "play-control");
    wait_for_port(cx, &desk, &dir);
    assert_eq!(asked(&dir, "play headings-26 start --json"), 1);
    assert!(is_on_screen(cx, "status-playing"), "the bar says it runs");
    click(cx, "play-port");
    assert_eq!(cx.opened_url().as_deref(), Some("http://localhost:56000/"));

    click(cx, "pane-view-play");
    wait_for(cx, "the log", |cx| is_on_screen(cx, "play-log"));
    assert_eq!(asked(&dir, "play headings-26 log"), 1);

    // At rest: ten minutes pass and the service is asked nothing more about Play.
    let before = asked(&dir, "play");
    for _ in 0..10 {
        cx.executor().advance_clock(Duration::from_secs(60));
        cx.run_until_parked();
        draw(cx);
    }
    std::thread::sleep(Duration::from_millis(200));
    cx.run_until_parked();
    assert_eq!(asked(&dir, "play"), before, "{:?}", calls(&dir));

    click(cx, "play-control");
    wait_for(cx, "the stop", |cx| !is_on_screen(cx, "play-port"));
    assert_eq!(asked(&dir, "play headings-26 stop --json"), 1);
    assert!(!is_on_screen(cx, "status-playing"));
}

#[gpui::test]
fn a_project_without_a_run_command_says_so_with_a_link_to_its_settings(cx: &mut TestAppContext) {
    let (dir, script) = service(None);
    let (cx, _, _) = open_live(cx, script, Some(ON_THE_MAC), false);
    wait_for(cx, "the control", |cx| is_on_screen(cx, "play-control"));
    click(cx, "play-control");
    click(cx, "pane-view-play");
    assert!(is_on_screen(cx, "play-unavailable"), "the view says why");
    assert!(is_on_screen(cx, "play-project-settings"), "and where to go");
    assert!(!is_on_screen(cx, "play-log"));
    assert_eq!(asked(&dir, "play"), 0, "nothing to ask: {:?}", calls(&dir));
}

#[gpui::test]
fn a_lavoro_in_a_box_cannot_play_yet_and_says_so(cx: &mut TestAppContext) {
    let (dir, script) = service(Some("npm run dev"));
    let (cx, _, _) = open_live(cx, script, Some(IN_A_BOX), false);
    wait_for(cx, "the control", |cx| is_on_screen(cx, "play-control"));
    click(cx, "play-control");
    click(cx, "pane-view-play");
    assert!(is_on_screen(cx, "play-unavailable"), "the view says why");
    assert!(
        !is_on_screen(cx, "play-project-settings"),
        "no setting of the project changes it"
    );
    assert_eq!(asked(&dir, "play"), 0, "nothing to ask: {:?}", calls(&dir));
}

#[gpui::test]
fn the_sample_data_has_no_play(cx: &mut TestAppContext) {
    let (cx, _, _) = open_with(cx, None);
    wait_for(cx, "the rows", |cx| is_on_screen(cx, "row-headings-26"));
    click(cx, "row-headings-26");
    wait_for(cx, "the header", |cx| is_on_screen(cx, "page-header"));
    assert!(!is_on_screen(cx, "play-control"));
    assert!(!is_on_screen(cx, "pane-view-play"));
    assert!(
        is_on_screen(cx, "pane-view-git"),
        "the other views are there"
    );
}

in_every_language! {
fn starting_play_moves_nothing_in_the_header_but_the_port_that_comes(cx: &mut TestAppContext) {
    let (dir, script) = service(Some("npm run dev"));
    let (cx, desk, _) = open_live(cx, script, Some(ON_THE_MAC), false);
    wait_for_play(cx, &desk);
    let place = |cx: &mut VisualTestContext, selector: &'static str| {
        cx.debug_bounds(selector).unwrap_or_else(|| panic!("{selector} is not on screen"))
    };
    let places = |cx: &mut VisualTestContext| {
        ["play-control", "task-menu", "header-glyph", "header-figures"].map(|part| place(cx, part))
    };
    let before = places(cx);
    assert!(before[0].right() < before[1].left(), "Play stands before the page's actions");
    click(cx, "play-control");
    wait_for_port(cx, &desk, &dir);
    assert_eq!(places(cx), before, "a start moves nothing: the port comes in room kept for it");
    let port = place(cx, "play-port");
    assert!(port.right() <= before[0].left(), "the port stands before the control");
    assert!(port.left() >= before[3].right(), "and after the changed lines");
}
}

in_every_language! {
fn at_the_smallest_window_the_five_views_are_whole_in_the_row_at_once(cx: &mut TestAppContext) {
    let (_dir, script) = service(Some("npm run dev"));
    let (cx, desk, _) = open_live(cx, script, Some(ON_THE_MAC), false);
    wait_for_play(cx, &desk);
    let place = |cx: &mut VisualTestContext, selector: &'static str| {
        cx.debug_bounds(selector).unwrap_or_else(|| panic!("{selector} is not on screen"))
    };
    // With room, a tab keeps the whole room at its sides: only a narrow row takes some of it.
    let label = cx.update(|window, _| {
        let (size, weight) = (crate::theme::TEXT_SMALL, crate::theme::WEIGHT_MEDIUM);
        crate::ui::chrome::text_width(window, PaneView::Play.label(), size, weight)
    });
    let roomy = gpui::px(label) + crate::theme::CONTROL_PADDING_X * 2.;
    let play = place(cx, "pane-view-play").size.width;
    assert!((play - roomy).abs() <= gpui::px(1.), "Play is {play:?} wide, not {roomy:?}");
    cx.simulate_resize(gpui::size(gpui::px(1100.), gpui::px(700.)));
    draw(cx);
    draw(cx);
    let row = place(cx, "pane-views");
    let inset = crate::theme::CONTENT_PADDING;
    let views = ["files", "changes", "checks", "git", "play"];
    let tabs = views.map(|view| -> &'static str { format!("pane-view-{view}").leak() });
    let mut end = row.left();
    for tab in tabs {
        let tab = place(cx, tab);
        assert!(tab.left() >= end, "a view stands on the one before it: {tab:?}");
        assert!(
            tab.right() <= row.right() - inset,
            "a view is cut with none chosen or scrolled to: {tab:?} in {row:?}"
        );
        end = tab.right();
    }
}
}

in_every_language! {
fn at_the_smallest_window_the_view_chosen_is_whole_in_the_row_of_five(cx: &mut TestAppContext) {
    let (_dir, script) = service(Some("npm run dev"));
    let (cx, desk, _) = open_live(cx, script, Some(ON_THE_MAC), false);
    wait_for_play(cx, &desk);
    cx.simulate_resize(gpui::size(gpui::px(1100.), gpui::px(700.)));
    draw(cx);
    let place = |cx: &mut VisualTestContext, selector: &'static str| {
        cx.debug_bounds(selector).unwrap_or_else(|| panic!("{selector} is not on screen"))
    };
    let views = [
        (PaneView::Files, "pane-view-files"),
        (PaneView::Changes, "pane-view-changes"),
        (PaneView::Checks, "pane-view-checks"),
        (PaneView::Git, "pane-view-git"),
        (PaneView::Play, "pane-view-play"),
    ];
    for (view, selector) in views {
        desk.update(cx, |desk, cx| desk.show_pane_view(ON_THE_MAC, view, cx));
        // The row is scrolled when it is laid out: the frame after shows the view where it went.
        draw(cx);
        draw(cx);
        let (row, tab) = (place(cx, "pane-views"), place(cx, selector));
        assert!(row.right() <= gpui::px(1100.), "the row ends in the window: {row:?}");
        let inset = crate::theme::CONTENT_PADDING;
        assert!(
            tab.left() >= row.left() && tab.right() <= row.right() - inset,
            "{selector} is whole between the row's insets: {tab:?} in {row:?}"
        );
    }
}
}
