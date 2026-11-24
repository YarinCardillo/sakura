//! Ages belong to tasks and update without rebuilding the sidebar or page header.

use super::interaction_tests::{draw, motion_ended, wait_for};
use super::launch_tests::{fake_agentbox, open_live, settled_calls, Names};
use crate::l10n::{self, Language};
use gpui::{TestAppContext, WindowVisibility};
use sakura_core::models::BoxStatus;
use std::time::Duration;

const STATUS: &str = include_str!("../../../sakura-core/tests/fixtures/status.json");
const FIRST: &str = "sample-accent";
/// A name short enough to keep its age beside the longest word of a state, in either language.
const SECOND: &str = "t-26";
const SECOND_IN_THE_FIXTURE: &str = "headings-26";

/// Whether the element with `selector` is on screen (the selector lives as long as the test).
fn is_shown(cx: &mut gpui::VisualTestContext, selector: String) -> bool {
    cx.debug_bounds(Box::leak(selector.into_boxed_str()))
        .is_some()
}

fn aged_tasks() -> (tempfile::TempDir, std::path::PathBuf) {
    aged_tasks_with(true)
}

/// The tasks with their ages, with their changed lines or without them.
fn aged_tasks_with(has_lines: bool) -> (tempfile::TempDir, std::path::PathBuf) {
    let (dir, script) = fake_agentbox(Names::Listed, "0", "");
    let mut status: BoxStatus = serde_json::from_str(STATUS).unwrap();
    for task in &mut status.tasks {
        if task.name == SECOND_IN_THE_FIXTURE {
            task.name = SECOND.into();
        }
        if !has_lines {
            (task.insertions, task.deletions) = (None, None);
        }
        task.age_at = Some("1970-01-01T00:00:00Z".into());
        if task.name == FIRST {
            task.age_at = Some("1970-01-01T01:00:00Z".into());
        }
        std::fs::write(
            dir.path().join(format!("facts-{}.json", task.name)),
            serde_json::to_string(task).unwrap(),
        )
        .unwrap();
    }
    std::fs::write(
        dir.path().join("status.json"),
        serde_json::to_string(&status).unwrap(),
    )
    .unwrap();
    let names = dir.path().join("tasks.json");
    let renamed = std::fs::read_to_string(&names)
        .unwrap()
        .replace(SECOND_IN_THE_FIXTURE, SECOND);
    std::fs::write(names, renamed).unwrap();
    (dir, script)
}

#[gpui::test]
fn row_age_slots_use_task_times_and_hide_while_working(cx: &mut TestAppContext) {
    let (dir, script) = aged_tasks();
    let (cx, _desk, _) = open_live(cx, script, Some(FIRST), false);
    settled_calls(cx, &dir);
    wait_for(cx, "the age of the short name", |cx| {
        is_shown(cx, format!("row-age-{SECOND}"))
    });
    assert!(
        !is_shown(cx, format!("row-age-{FIRST}")),
        "its name is too long to keep its room beside an age"
    );
    assert!(cx.debug_bounds("row-age-sample-folders").is_none());
}

#[gpui::test]
fn a_row_keeps_its_name_whole_before_its_age(cx: &mut TestAppContext) {
    let (dir, script) = aged_tasks_with(false);
    let (cx, _desk, _) = open_live(cx, script, Some(FIRST), false);
    settled_calls(cx, &dir);
    wait_for(cx, "the rows and the age of the short name", |cx| {
        is_shown(cx, format!("name-{FIRST}"))
            && is_shown(cx, format!("name-{SECOND}"))
            && is_shown(cx, format!("row-age-{SECOND}"))
    });
    let mut cut = Vec::new();
    for task in [FIRST, SECOND] {
        let name = cx.debug_bounds(Box::leak(format!("name-{task}").into_boxed_str()));
        let name = name.expect("the name is on screen").size.width;
        let whole = cx.update(|window, _| {
            let size = crate::theme::TEXT_BODY;
            crate::ui::chrome::text_width(window, task, size, gpui::FontWeight::NORMAL)
        });
        let has_age = is_shown(cx, format!("row-age-{task}"));
        if has_age && name < gpui::px(whole.ceil()) {
            cut.push((task, name, whole));
        }
    }
    assert!(
        cut.is_empty(),
        "a name is cut to make room for its age: {cut:?}"
    );
    assert!(
        is_shown(cx, format!("row-age-{SECOND}")),
        "a short name keeps its age"
    );
}

/// Waits for the status the ages come from: `settled_calls` cannot see a call the app has started
/// whose process has not run yet, so the ages are waited for, not assumed.
fn ages_arrived(cx: &mut gpui::VisualTestContext, desk: &gpui::Entity<super::desk::Desk>) {
    wait_for(cx, "the second task's age", |cx| {
        desk.read_with(cx, |desk, cx| {
            let row = desk.facts.read(cx).row(SECOND);
            row.is_some_and(|row| row.task.age_at.is_some())
        })
    });
}

fn clock_at(desk: &gpui::Entity<super::desk::Desk>, now: i64, cx: &mut gpui::VisualTestContext) {
    let ages = desk.read_with(cx, |desk, _| desk.row_ages.clone());
    ages.update(cx, |ages, cx| ages.set_now(now, cx));
    draw(cx);
    cx.run_until_parked();
    draw(cx);
}

#[gpui::test]
fn row_age_ticks_reuse_sidebar_and_header_and_sleep_when_hidden(cx: &mut TestAppContext) {
    let (dir, script) = aged_tasks();
    let (cx, desk, _) = open_live(cx, script, Some(FIRST), false);
    settled_calls(cx, &dir);
    ages_arrived(cx, &desk);
    l10n::apply(Language::English);
    cx.update(|_, cx| cx.set_reduce_motion(true));
    clock_at(&desk, 120, cx);
    assert!(is_shown(cx, format!("row-age-text-{SECOND}-2 min ago")));
    motion_ended(cx);
    let before = desk.read_with(cx, |desk, cx| desk.renders(cx));
    let calls = super::launch_tests::calls(&dir);
    cx.executor().advance_clock(Duration::from_secs(60));
    cx.run_until_parked();
    draw(cx);
    assert!(is_shown(cx, format!("row-age-text-{SECOND}-3 min ago")));
    assert_eq!(desk.read_with(cx, |desk, cx| desk.renders(cx)), before);
    assert_eq!(super::launch_tests::calls(&dir), calls);
    let ages = desk.read_with(cx, |desk, _| desk.row_ages.clone());
    cx.simulate_visibility_change(WindowVisibility::Hidden);
    cx.run_until_parked();
    assert!(!ages.read_with(cx, |ages, _| ages.is_ticking()));
    cx.executor().advance_clock(Duration::from_secs(600));
    cx.run_until_parked();
    assert!(!ages.read_with(cx, |ages, _| ages.is_ticking()));
    cx.simulate_visibility_change(WindowVisibility::Visible);
    cx.run_until_parked();
    draw(cx);
    assert!(ages.read_with(cx, |ages, _| ages.is_ticking()));
    assert!(is_shown(cx, format!("row-age-text-{SECOND}-13 min ago")));
}

#[gpui::test]
fn row_age_hours_tick_hourly_and_follow_the_current_language(cx: &mut TestAppContext) {
    let (dir, script) = aged_tasks();
    let (cx, desk, _) = open_live(cx, script, Some(FIRST), false);
    settled_calls(cx, &dir);
    ages_arrived(cx, &desk);
    l10n::apply(Language::Italian);
    cx.update(|_, cx| cx.set_reduce_motion(true));
    clock_at(&desk, 7200, cx);
    assert!(is_shown(cx, format!("row-age-text-{SECOND}-2 h fa")));
    motion_ended(cx);
    let before = desk.read_with(cx, |desk, cx| desk.renders(cx));
    cx.executor().advance_clock(Duration::from_secs(3599));
    cx.run_until_parked();
    draw(cx);
    assert!(is_shown(cx, format!("row-age-text-{SECOND}-2 h fa")));
    cx.executor().advance_clock(Duration::from_secs(1));
    cx.run_until_parked();
    draw(cx);
    assert!(is_shown(cx, format!("row-age-text-{SECOND}-3 h fa")));
    assert_eq!(desk.read_with(cx, |desk, cx| desk.renders(cx)), before);
}

#[gpui::test]
fn hiding_the_sidebar_cancels_row_age_timers(cx: &mut TestAppContext) {
    let (dir, script) = aged_tasks();
    let (cx, desk, _) = open_live(cx, script, Some(FIRST), false);
    settled_calls(cx, &dir);
    ages_arrived(cx, &desk);
    clock_at(&desk, 120, cx);
    let ages = desk.read_with(cx, |desk, _| desk.row_ages.clone());
    super::interaction_tests::click(cx, "toggle-sidebar");
    super::interaction_tests::motion_ended(cx);
    cx.run_until_parked();
    draw(cx);
    assert!(!ages.read_with(cx, |ages, _| ages.is_ticking()));
}
