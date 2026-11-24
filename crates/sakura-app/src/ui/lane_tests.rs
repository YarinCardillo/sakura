//! The lanes in the engine's window: the sidebar's rows under their projects with the engine's progress,
//! the lane's own page with its roles, its models and the controls the engine gives it.

use super::engine_desk::{Desk, DeskData};
use super::engine_rig_tests::{self, wait_until, Rig, SNAPSHOT};
use super::kit::StateGlyph;
use super::lanes;
use crate::prefs::Preferences;
use gpui::{AppContext as _, Entity, Modifiers, TestAppContext, VisualTestContext};
use sakura_core::engine::snapshot::Snapshot;
use sakura_core::model::StateId;
use sakura_core::tabs::Page;
use std::cell::RefCell;
use std::rc::Rc;

const CONFIRM: &str = r#"{"result": "confirm", "token": "17.ab", "expires": "2026-11-16T08:02:00Z", "summary": "Stop demo: run in /work/repo: sakura flow stop demo"}"#;
const DONE: &str = r#"{"result": "done", "exit_code": 0, "output": "stopped demo\n"}"#;

/// Where the element named `selector` is drawn; the name is made here, so it is leaked for the test.
pub(super) fn bounds(
    cx: &mut VisualTestContext,
    selector: &str,
) -> Option<gpui::Bounds<gpui::Pixels>> {
    let selector: &'static str = Box::leak(selector.to_string().into_boxed_str());
    cx.debug_bounds(selector)
}

pub(super) fn click(cx: &mut VisualTestContext, selector: &str) {
    let at = bounds(cx, selector)
        .unwrap_or_else(|| panic!("{selector}"))
        .center();
    cx.simulate_click(at, Modifiers::none());
}

/// The desk over a fake engine, its first snapshot read and drawn.
fn open<'a>(
    cx: &'a mut TestAppContext,
    snapshot: &str,
) -> (Rig, Entity<Desk>, &'a mut VisualTestContext) {
    open_answering(cx, snapshot, CONFIRM, DONE)
}

/// The same over a fake engine that answers an action with `first`, and with `second` once a
/// confirmation token is sent.
pub(super) fn open_answering<'a>(
    cx: &'a mut TestAppContext,
    snapshot: &str,
    first: &str,
    second: &str,
) -> (Rig, Entity<Desk>, &'a mut VisualTestContext) {
    open_answering_with(cx, snapshot, (first, second), Preferences::default())
}

/// The same with the window's preferences as the last launch left them.
pub(super) fn open_answering_with<'a>(
    cx: &'a mut TestAppContext,
    snapshot: &str,
    (first, second): (&str, &str),
    preferences: Preferences,
) -> (Rig, Entity<Desk>, &'a mut VisualTestContext) {
    let rig = engine_rig_tests::open(cx, snapshot, first, second);
    let kept = preferences.clone();
    cx.update(|cx| kept.install(None, cx));
    let (facts, source, sessions) = (rig.facts.clone(), rig.source.clone(), rig.sessions.clone());
    let slot = Rc::new(RefCell::new(None));
    let kept = slot.clone();
    let (_, cx) = cx.add_window_view(move |window, cx| {
        let data = DeskData {
            facts,
            source,
            sessions,
        };
        let desk = cx.new(|cx| Desk::new(data, &preferences, window, cx));
        *kept.borrow_mut() = Some(desk.clone());
        Desk::root(desk, window, cx)
    });
    let desk = slot.borrow_mut().take().expect("the desk was built");
    cx.update(|window, _| window.activate_window());
    wait_until(cx, "the lanes", |cx| {
        rig.facts
            .read_with(cx, |facts, _| facts.snapshot().is_some())
    });
    (rig, desk, cx)
}

fn snapshot() -> Snapshot {
    Snapshot::parse(SNAPSHOT.as_bytes()).expect("the fixture")
}

#[gpui::test]
fn lane_row_bar_matches_engine_progress(cx: &mut TestAppContext) {
    let demo = snapshot()
        .items
        .into_iter()
        .find(|item| item.task() == "demo")
        .expect("the lane");
    let progress = demo.progress.as_ref().expect("its progress");
    assert_eq!(lanes::done_words(progress).as_ref(), "1 of 4 done");
    let (_rig, _desk, cx) = open(cx, SNAPSHOT);
    assert!(
        bounds(cx, "row-demo-bar").is_some(),
        "a lane with a plan has a line beside its title"
    );
    let row = bounds(cx, "row-demo").expect("the row");
    let bar = bounds(cx, "row-demo-bar").expect("the bar");
    let title = bounds(cx, "name-demo").expect("the title");
    assert!(
        title.right() <= bar.left() && bar.right() <= row.right(),
        "the line stands after the title, inside the row"
    );
}

#[gpui::test]
fn a_lane_without_a_plan_has_no_line_in_the_sidebar(cx: &mut TestAppContext) {
    let (_rig, _desk, cx) = open(cx, SNAPSHOT);
    assert!(bounds(cx, "row-fresh").is_some());
    assert!(bounds(cx, "row-fresh-bar").is_none());
    assert!(
        bounds(cx, "row-fresh-phase").is_none(),
        "the sidebar's row has no room for the phase word"
    );
    assert_eq!(lanes::phase_word("intake").as_ref(), "Intake");
    assert_eq!(
        lanes::phase_word("an-unknown-phase").as_ref(),
        "Unknown phase"
    );
}

#[test]
fn the_lanes_of_a_project_keep_their_order_whatever_state_they_change_to() {
    let order = |change: &dyn Fn(&mut serde_json::Value)| {
        let mut document: serde_json::Value = serde_json::from_str(SNAPSHOT).expect("the fixture");
        for item in document["items"].as_array_mut().expect("the items") {
            change(item);
        }
        let read = Snapshot::parse(document.to_string().as_bytes()).expect("a snapshot");
        lanes::by_project(&read)
            .iter()
            .map(|group| {
                group
                    .lanes
                    .iter()
                    .map(|item| item.task().to_string())
                    .collect()
            })
            .collect::<Vec<Vec<String>>>()
    };
    let before = order(&|_| {});
    // Every lane enters a new state at another time, the way lanes do as they work: no row moves.
    let reversed = order(&|item| {
        let at = item["since"]
            .as_str()
            .unwrap_or("2026-11-18T00:00:00+00:00")
            .to_string();
        let flipped = at.replace("2026", "2025").replace("11-1", "11-2");
        item["since"] = flipped.into();
        item["agent_state"] = "working".into();
    });
    assert_eq!(before, reversed, "a state change never moves a row");
}

#[gpui::test]
fn the_lanes_list_uses_its_shared_new_lane_button_and_plain_project_text(cx: &mut TestAppContext) {
    let (_rig, desk, cx) = open(cx, SNAPSHOT);
    desk.update(cx, |desk, cx| {
        desk.navigate(sakura_core::tabs::Page::Lanes, false, cx)
    });
    wait_until(cx, "lane metadata", |cx| {
        bounds(cx, "lanes-row-demo-tag").is_some()
    });
    let project = bounds(cx, "lanes-row-demo-tag").unwrap();
    assert!(
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            !window
                .painted_quads()
                .iter()
                .any(|quad| quad.bounds == project.scale(window.scale_factor()))
        }),
        "the project label has no capsule"
    );
    assert!(bounds(cx, "lanes-new-lane-label").is_some());
}

#[gpui::test]
fn lanes_grouped_by_project(cx: &mut TestAppContext) {
    let read = snapshot();
    let grouped = lanes::by_project(&read);
    let names: Vec<(String, Vec<&str>)> = grouped
        .iter()
        .map(|group| {
            (
                group.name.clone(),
                group.lanes.iter().map(|item| item.task()).collect(),
            )
        })
        .collect();
    // In name order, which no state change moves.
    assert_eq!(
        names,
        [
            ("other".to_string(), vec!["fresh", "stuck"]),
            ("repo".to_string(), vec!["measure-help", "demo", "gated"]),
        ]
    );
    let (_rig, _desk, cx) = open(cx, SNAPSHOT);
    assert!(bounds(cx, "group-other").is_some() && bounds(cx, "group-repo").is_some());
    let (other, fresh) = (
        bounds(cx, "group-other").expect("first"),
        bounds(cx, "row-fresh").expect("row"),
    );
    let (repo, stuck) = (
        bounds(cx, "group-repo").expect("second"),
        bounds(cx, "row-stuck").expect("row"),
    );
    assert!(
        other.top() < fresh.top() && stuck.top() < repo.top(),
        "each label stands over its lanes"
    );
}

#[gpui::test]
fn a_press_on_a_row_opens_the_lane_and_selects_its_row(cx: &mut TestAppContext) {
    let (_rig, desk, cx) = open(cx, SNAPSHOT);
    click(cx, "row-demo");
    wait_until(cx, "the lane page", |cx| {
        cx.debug_bounds("lane-page").is_some()
    });
    let shown = desk.read_with(cx, |desk, cx| {
        desk.bar.read(cx).tabs().active().page().clone()
    });
    assert_eq!(shown, Page::Lane(engine_rig_tests::lane_key("demo")));
}

#[gpui::test]
fn lane_page_lists_its_steps_and_the_reviews_that_are_on(cx: &mut TestAppContext) {
    let (_rig, desk, cx) = open(cx, SNAPSHOT);
    desk.update(cx, |desk, cx| {
        desk.open_lane(&engine_rig_tests::lane_key("demo"), cx)
    });
    wait_until(cx, "the lane page", |cx| {
        bounds(cx, "step-row-build").is_some()
    });
    for step in ["plan", "build", "checks", "deliver"] {
        assert!(bounds(cx, &format!("step-row-{step}")).is_some(), "{step}");
    }
    assert!(bounds(cx, "review-row-task_review").is_some());
    assert!(
        cx.debug_bounds("lane-header").is_some(),
        "the header holds the name, the state and the steps"
    );
    assert!(cx.debug_bounds("lane-state").is_some());
    assert!(cx.debug_bounds("step-track").is_some());
}

#[gpui::test]
fn the_header_draws_only_the_steps_the_lane_runs_sharing_the_width_with_a_steps_menu_at_the_end(
    cx: &mut TestAppContext,
) {
    let (_rig, desk, cx) = open(cx, SNAPSHOT);
    desk.update(cx, |desk, cx| {
        desk.open_lane(&engine_rig_tests::lane_key("demo"), cx)
    });
    wait_until(cx, "the lane page", |cx| bounds(cx, "step-track").is_some());
    let running = [
        "plan",
        "plan_review",
        "build",
        "task_review",
        "checks",
        "evaluator",
        "deliver",
    ];
    let widths: Vec<f32> = running
        .iter()
        .map(|step| {
            let bar = bounds(cx, &format!("step-seg-{step}")).unwrap_or_else(|| panic!("{step}"));
            f32::from(bar.size.width)
        })
        .collect();
    assert!(
        widths
            .windows(2)
            .all(|pair| (pair[0] - pair[1]).abs() < 1.5),
        "the bars share the width: {widths:?}"
    );
    for off in ["final_review", "reporter"] {
        assert!(
            bounds(cx, &format!("step-seg-{off}")).is_none(),
            "{off} is off and takes no room"
        );
    }
    let (track, menu) = (
        bounds(cx, "step-track").expect("the track"),
        bounds(cx, "lane-steps").expect("the Steps menu"),
    );
    assert!(track.right() <= menu.left(), "the menu ends the bar");
}

#[gpui::test]
fn a_step_turned_off_leaves_the_bar_over_the_base_duration_and_the_others_take_its_share(
    cx: &mut TestAppContext,
) {
    let (rig, desk, cx) = open(cx, SNAPSHOT);
    desk.update(cx, |desk, cx| {
        desk.open_lane(&engine_rig_tests::lane_key("demo"), cx)
    });
    wait_until(cx, "the lane page", |cx| {
        bounds(cx, "step-seg-checks").is_some()
    });
    // The rig reduces motion; this test is about the motion.
    cx.update(|_, cx| cx.set_reduce_motion(false));
    let before = f32::from(bounds(cx, "step-seg-plan").expect("a bar").size.width);
    let mut document: serde_json::Value = serde_json::from_str(SNAPSHOT).expect("the fixture");
    for step in document["items"][0]["pipeline"]
        .as_array_mut()
        .expect("the pipeline")
    {
        if step["id"] == "checks" {
            step["on"] = false.into();
            step["state"] = "off".into();
        }
    }
    rig.say_snapshot(&document.to_string());
    rig.facts.update(cx, |facts, cx| facts.read(cx));
    // The engine's word has arrived once the bar starts to give the step's room back.
    let mut half_way = false;
    for _ in 0..200 {
        cx.run_until_parked();
        cx.executor().advance_clock(crate::theme::DURATION_BASE / 4);
        cx.update(|window, cx| window.simulate_next_frame(cx));
        let narrower = bounds(cx, "step-seg-checks").is_some_and(|bar| {
            f32::from(bar.size.width)
                < f32::from(bounds(cx, "step-seg-plan").map_or(gpui::px(0.), |p| p.size.width)) - 1.
        });
        if narrower {
            half_way = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(
        half_way,
        "it is still there, narrower than the others, part of the way"
    );
    for _ in 0..20 {
        cx.executor().advance_clock(crate::theme::DURATION_BASE);
        rig.facts.update(cx, |facts, cx| facts.read(cx));
        cx.run_until_parked();
        cx.update(|window, cx| window.simulate_next_frame(cx));
        if bounds(cx, "step-seg-checks").is_none() {
            break;
        }
    }
    assert!(bounds(cx, "step-seg-checks").is_none(), "then it is gone");
    let after = f32::from(bounds(cx, "step-seg-plan").expect("a bar").size.width);
    assert!(after > before, "the others share the room it left");
}

#[gpui::test]
fn the_steps_menu_turns_a_step_on_with_the_engines_action_t_and_a_step_it_may_not_toggle_does_nothing(
    cx: &mut TestAppContext,
) {
    let (rig, desk, cx) = open(cx, SNAPSHOT);
    desk.update(cx, |desk, cx| {
        desk.open_lane(&engine_rig_tests::lane_key("demo"), cx)
    });
    wait_until(cx, "the lane page", |cx| bounds(cx, "lane-steps").is_some());
    click(cx, "lane-steps");
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    for stays in ["plan", "build", "task_review"] {
        assert!(
            bounds(cx, &format!("steps-row-{stays}")).is_some(),
            "{stays}"
        );
    }
    // Build may not be toggled: a press on it sends nothing.
    click(cx, "steps-row-build");
    cx.run_until_parked();
    assert!(rig.acts().is_empty(), "build is always on");
    cx.update(|window, cx| window.draw(cx).clear(cx));
    if bounds(cx, "steps-row-final_review").is_none() {
        click(cx, "lane-steps");
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }
    click(cx, "steps-row-final_review");
    wait_until(cx, "the call", |_| !rig.acts().is_empty());
    assert_eq!(
        rig.acts()[0],
        r#"cockpit act t ["flow","/work/repo","demo"] --state on --step final_review --source app --json"#
    );
}

#[gpui::test]
fn the_steps_menu_scrolls_to_steps_below_the_viewport(cx: &mut TestAppContext) {
    let mut document: serde_json::Value = serde_json::from_str(SNAPSHOT).unwrap();
    let item = document["items"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|item| item["key"][2] == "demo")
        .unwrap();
    let steps = item["pipeline"].as_array_mut().unwrap();
    let sample = steps[0].clone();
    for index in 0..35 {
        let mut step = sample.clone();
        step["id"] = format!("extra-{index}").into();
        steps.push(step);
    }
    let (_rig, desk, cx) = open(cx, &document.to_string());
    cx.simulate_resize(gpui::size(gpui::px(1280.), gpui::px(800.)));
    desk.update(cx, |desk, cx| {
        desk.open_lane(&engine_rig_tests::lane_key("demo"), cx)
    });
    wait_until(cx, "Steps", |cx| bounds(cx, "lane-steps").is_some());
    click(cx, "lane-steps");
    let first = bounds(cx, "steps-row-extra-0").unwrap();
    let position = bounds(cx, "steps-row-plan").unwrap().center();
    cx.simulate_event(gpui::ScrollEvent {
        position,
        delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(-240.))),
        ..Default::default()
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(bounds(cx, "steps-row-extra-0").unwrap().top() < first.top());
}

#[gpui::test]
fn lane_page_actions_are_the_items_actions(cx: &mut TestAppContext) {
    let demo = snapshot()
        .items
        .into_iter()
        .find(|item| item.task() == "demo")
        .expect("the lane");
    let keys: Vec<&str> = demo
        .actions
        .iter()
        .map(|action| action.key.as_str())
        .collect();
    assert_eq!(keys, ["m", "D", "a", "R", "s", "w", "t"]);
    let (rig, desk, cx) = open(cx, SNAPSHOT);
    let key = engine_rig_tests::lane_key("demo");
    desk.update(cx, |desk, cx| desk.open_lane(&key, cx));
    wait_until(cx, "the lane page", |cx| {
        bounds(cx, "lane-agents").is_some()
    });
    assert!(
        bounds(cx, "lane-more").is_some(),
        "Resume and Stop are in the menu"
    );
    let page = desk.read_with(cx, |desk, cx| {
        desk.lane_for_test(&key, cx).expect("the page").clone()
    });
    page.update(cx, |page, cx| page.press_lane_action("s", cx));
    wait_until(cx, "the call", |_| rig.acts().len() == 1);
    let sent = &rig.acts()[0];
    assert!(
        sent.starts_with(r#"cockpit act s ["flow","/work/repo","demo"]"#),
        "{sent}"
    );
    wait_until(cx, "the summary", |cx| {
        bounds(cx, "summary-[\"flow\",\"/work/repo\",\"demo\"]#s").is_some()
    });
}

#[gpui::test]
fn a_lane_that_is_gone_says_so_on_its_page(cx: &mut TestAppContext) {
    let (rig, desk, cx) = open(cx, SNAPSHOT);
    desk.update(cx, |desk, cx| {
        desk.open_lane(&engine_rig_tests::lane_key("demo"), cx)
    });
    wait_until(cx, "the lane page", |cx| {
        cx.debug_bounds("lane-page").is_some()
    });
    rig.say_snapshot(r#"{"api": 1, "waits": [], "items": [], "events": [], "problems": []}"#);
    rig.facts.update(cx, |facts, cx| facts.read(cx));
    // The tab stays, and the page says the lane is gone.
    cx.run_until_parked();
    assert_eq!(
        desk.read_with(cx, |desk, cx| desk.bar.read(cx).tabs().len()),
        1
    );
    assert!(
        cx.debug_bounds("lane-page").is_some(),
        "the lane's page is still shown, in its gone state"
    );
}

/// The fixture with its outside lane changed by `change`.
fn with_outside_lane(change: impl FnOnce(&mut serde_json::Value)) -> String {
    let mut snapshot: serde_json::Value = serde_json::from_str(SNAPSHOT).expect("the fixture");
    let items = snapshot["items"].as_array_mut().expect("the items");
    let lane = items.iter_mut().find(|item| item["kind"] == "outside");
    change(lane.expect("the outside lane"));
    snapshot.to_string()
}

#[gpui::test]
fn outside_lane_row_named_by_title_with_engine_progress(cx: &mut TestAppContext) {
    let read = snapshot();
    let lane = read.items.iter().find(|item| item.is_outside());
    let lane = lane.expect("the outside lane");
    assert_eq!(lane.name(), "Add --help to measure.py");
    let progress = lane.progress.as_ref().expect("its progress");
    assert_eq!(lanes::done_words(progress).as_ref(), "0 of 1 done");
    assert_eq!(
        lanes::state_of(lane).id,
        StateId::NeedsYou,
        "an open wait is the one reason for the mark"
    );
    let started = with_outside_lane(|lane| {
        lane["progress"] = serde_json::json!({"done": 1, "total": 2, "phase": "draft"});
    });
    let (_rig, _desk, cx) = open(cx, &started);
    wait_until(cx, "the lane's row", |cx| {
        bounds(cx, "row-measure-help").is_some()
    });
    assert!(bounds(cx, "name-measure-help").is_some());
    assert!(
        bounds(cx, "row-measure-help-bar").is_some(),
        "the line of the engine's progress stands after the title once something is done"
    );
    assert!(bounds(cx, "row-measure-help-phase").is_none());
}

#[gpui::test]
fn outside_lane_without_rows_has_no_line_and_the_word_of_its_phase_stays_in_words(
    cx: &mut TestAppContext,
) {
    let quiet = with_outside_lane(|lane| {
        lane["agent_state"] = "unknown".into();
        lane["progress"] = serde_json::json!({"done": 0, "total": 0, "phase": "ready"});
    });
    let read = Snapshot::parse(quiet.as_bytes()).expect("the snapshot");
    let lane = read.items.iter().find(|item| item.is_outside());
    assert_eq!(
        lanes::state_of(lane.expect("the lane")).id,
        StateId::Idle,
        "no open wait, no mark"
    );
    assert_eq!(lanes::phase_word("draft").as_ref(), "Draft");
    assert_eq!(lanes::phase_word("ready").as_ref(), "Ready");
    let (_rig, _desk, cx) = open(cx, &quiet);
    wait_until(cx, "the lane's row", |cx| {
        bounds(cx, "row-measure-help").is_some()
    });
    assert!(bounds(cx, "row-measure-help-phase").is_none());
    assert!(bounds(cx, "row-measure-help-bar").is_none());
}

/// A flow lane `same` in two projects (the first with a plan, the second without) and an outside lane
/// whose id is `same` too.
fn same_id_snapshot() -> String {
    let lane = |kind: &str, key: serde_json::Value, repo: &str, state: &str, total: u32| {
        serde_json::json!({
            "id": key.to_string(), "key": key, "kind": kind, "repo": repo, "agent_state": state,
            "since": null, "last_activity_at": null, "title": "Outside same", "author": "outside:fixture",
            "progress": {"done": 0, "total": total, "phase": "plan"}, "actions": []
        })
    };
    serde_json::json!({
        "api": 1, "waits": [], "events": [], "problems": [],
        "items": [
            lane("flow", serde_json::json!(["flow", "/work/a", "same"]), "/work/a", "working", 3),
            lane("flow", serde_json::json!(["flow", "/work/b", "same"]), "/work/b", "gate", 0),
            lane("outside", serde_json::json!(["outside", "same"]), "/work/a", "waiting", 1),
        ]
    })
    .to_string()
}

fn same_id_keys() -> Vec<String> {
    let read = Snapshot::parse(same_id_snapshot().as_bytes()).expect("the snapshot");
    read.items.iter().map(|item| item.lane_key()).collect()
}

#[gpui::test]
fn lanes_with_one_id_stay_three_lanes_with_their_own_tabs_marks_and_pages(cx: &mut TestAppContext) {
    let keys = same_id_keys();
    assert_eq!(keys.len(), 3);
    let (_rig, desk, cx) = open(cx, &same_id_snapshot());
    for key in &keys {
        desk.update(cx, |desk, cx| {
            desk.navigate(Page::Lane(key.to_string()), true, cx)
        });
    }
    wait_until(cx, "the tabs", |cx| {
        desk.read_with(cx, |desk, cx| desk.bar.read(cx).tabs().len() == 4)
    });
    let marks = desk.read_with(cx, |desk, cx| desk.bar.read(cx).glyphs_of(&keys));
    assert_eq!(
        marks,
        [
            Some(StateGlyph::Running),
            Some(StateGlyph::NeedsYou),
            Some(StateGlyph::NeedsYou)
        ],
        "each lane has the mark of its own state"
    );
    for (key, is_outside) in [(&keys[0], false), (&keys[1], false), (&keys[2], true)] {
        desk.update(cx, |desk, cx| desk.open_lane(key, cx));
        wait_until(cx, "the lane page", |cx| {
            bounds(cx, "lane-header").is_some()
        });
        assert!(bounds(cx, "lane-title").is_some(), "{key}");
        assert_eq!(bounds(cx, "lane-from").is_some(), is_outside, "{key}");
    }
}

#[gpui::test]
fn the_sidebar_lists_both_lanes_with_one_id_apart(cx: &mut TestAppContext) {
    let read = Snapshot::parse(same_id_snapshot().as_bytes()).expect("the snapshot");
    let grouped = lanes::by_project(&read);
    let counts: Vec<(String, usize)> = grouped
        .iter()
        .map(|group| (group.name.clone(), group.lanes.len()))
        .collect();
    assert_eq!(
        counts,
        [("a".to_string(), 1), ("b".to_string(), 1)],
        "the outside lane in /work/a is no project's"
    );
    assert_eq!(lanes::outside_lanes(&read).len(), 1);
    let (_rig, desk, cx) = open(cx, &same_id_snapshot());
    let keys = same_id_keys();
    desk.update(cx, |desk, cx| desk.open_lane(&keys[1], cx));
    let shown = desk.read_with(cx, |desk, cx| {
        desk.bar.read(cx).tabs().active().page().clone()
    });
    assert_eq!(
        shown,
        Page::Lane(keys[1].clone()),
        "the second lane's page, not the first one's"
    );
}

/// The fixture's flow lane `demo` with its actions replaced by `actions`.
pub(super) fn with_demo_actions(actions: serde_json::Value) -> String {
    let mut snapshot: serde_json::Value = serde_json::from_str(SNAPSHOT).expect("the fixture");
    let items = snapshot["items"].as_array_mut().expect("the items");
    let lane = items
        .iter_mut()
        .find(|item| item["kind"] == "flow" && item["key"][2] == "demo");
    lane.expect("the demo lane")["actions"] = actions;
    snapshot.to_string()
}

#[gpui::test]
fn the_lane_menu_offers_only_the_actions_the_snapshot_lists(cx: &mut TestAppContext) {
    let listed = serde_json::json!([
        {"key": "R", "label": "Resume", "needs": [], "optional": [], "confirm": true},
        {"key": "C", "label": "Recontract", "needs": [], "optional": [], "confirm": true},
        {"key": "X", "label": "Clean", "needs": [], "optional": [], "confirm": true}
    ]);
    let (_rig, desk, cx) = open(cx, &with_demo_actions(listed));
    desk.update(cx, |desk, cx| {
        desk.open_lane(&engine_rig_tests::lane_key("demo"), cx)
    });
    wait_until(cx, "the lane page", |cx| bounds(cx, "lane-more").is_some());
    let key = engine_rig_tests::lane_key("demo");
    let page = desk.read_with(cx, |desk, cx| {
        desk.lane_for_test(&key, cx).expect("the page").clone()
    });
    page.update(cx, |page, cx| page.press_lane_action("s", cx));
    cx.run_until_parked();
    assert!(
        _rig.acts().is_empty(),
        "Stop is not offered when the engine does not list it"
    );
}

#[test]
fn a_lane_that_cannot_continue_says_blocked() {
    let read = snapshot();
    let lane = read.items.iter().find(|item| item.agent_state == "blocked");
    let lane = lane.expect("the fixture has a blocked lane");
    let state = lanes::state_of(lane);
    assert_eq!(state.word, sakura_core::said::Key("lane.state.blocked"));
    assert_eq!(
        state.id,
        StateId::Failed,
        "the tone of a lane that cannot go on is unchanged"
    );
    crate::l10n::set_for_test(crate::l10n::Language::English);
    assert_eq!(crate::l10n::tr(state.word), "Blocked");
    crate::l10n::set_for_test(crate::l10n::Language::Italian);
    assert_eq!(crate::l10n::tr(state.word), "Bloccato");
}

#[gpui::test]
fn the_lane_menu_shows_only_while_the_snapshot_offers_resume_or_stop(cx: &mut TestAppContext) {
    let offered = |key: &str, label: &str| serde_json::json!({"key": key, "label": label, "needs": [], "optional": [], "confirm": true});
    let with = with_demo_actions(serde_json::json!([
        offered("a", "Approve"),
        offered("R", "Resume")
    ]));
    let (_rig, desk, cx) = open(cx, &with);
    desk.update(cx, |desk, cx| {
        desk.open_lane(&engine_rig_tests::lane_key("demo"), cx)
    });
    wait_until(cx, "the lane page", |cx| bounds(cx, "lane-more").is_some());
    let without = with_demo_actions(serde_json::json!([offered("a", "Approve")]));
    _rig.say_snapshot(&without);
    _rig.facts.update(cx, |facts, cx| facts.read(cx));
    wait_until(cx, "the menu to go", |cx| bounds(cx, "lane-more").is_none());
}

/// The fixture's flow lane `demo` in the state `agent_state`, with `delivery` when given.
fn with_demo_ended(agent_state: &str, delivery: Option<serde_json::Value>) -> String {
    let mut snapshot: serde_json::Value = serde_json::from_str(SNAPSHOT).expect("the fixture");
    let items = snapshot["items"].as_array_mut().expect("the items");
    let lane = items
        .iter_mut()
        .find(|item| item["kind"] == "flow" && item["key"][2] == "demo");
    let lane = lane.expect("the demo lane");
    lane["agent_state"] = agent_state.into();
    lane["delivery"] = delivery.unwrap_or(serde_json::Value::Null);
    snapshot.to_string()
}

#[test]
fn a_lane_that_delivered_was_cleaned_or_was_stopped_is_never_idle() {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let words = [
        ("delivered", StateId::Verified, "Delivered", "Consegnato"),
        ("cleaned", StateId::NotStarted, "Closed", "Chiusa"),
        ("stopped", StateId::NotStarted, "Stopped", "Fermato"),
        ("blocked", StateId::Failed, "Blocked", "Bloccato"),
    ];
    for (agent_state, id, english, italian) in words {
        let read =
            Snapshot::parse(with_demo_ended(agent_state, None).as_bytes()).expect("a snapshot");
        let lane = read
            .items
            .iter()
            .find(|item| item.task() == "demo")
            .expect("the lane");
        let state = lanes::state_of(lane);
        assert_eq!(state.id, id, "{agent_state}");
        assert_ne!(state.id, StateId::Idle);
        crate::l10n::set_for_test(crate::l10n::Language::English);
        assert_eq!(crate::l10n::tr(state.word), english);
        crate::l10n::set_for_test(crate::l10n::Language::Italian);
        assert_eq!(crate::l10n::tr(state.word), italian);
    }
    let read = Snapshot::parse(with_demo_ended("unknown", None).as_bytes()).expect("a snapshot");
    let lane = read
        .items
        .iter()
        .find(|item| item.task() == "demo")
        .expect("the lane");
    assert_eq!(
        lanes::state_of(lane).id,
        StateId::Idle,
        "only a lane that says nothing is idle"
    );
}

#[gpui::test]
fn the_sidebar_drops_a_delivered_lane_and_its_page_shows_its_branch_and_head(
    cx: &mut TestAppContext,
) {
    let delivery = serde_json::json!({"kind": "push", "head": "a9102f70c7c7deadbeef", "branch": "lane/demo", "url": ""});
    let (_rig, desk, cx) = open(cx, &with_demo_ended("delivered", Some(delivery)));
    wait_until(cx, "the sidebar", |cx| bounds(cx, "row-gated").is_some());
    assert!(
        bounds(cx, "row-demo").is_none(),
        "the sidebar lists open lanes only"
    );
    desk.update(cx, |desk, cx| {
        desk.open_lane(&engine_rig_tests::lane_key("demo"), cx)
    });
    wait_until(cx, "the delivery", |cx| {
        bounds(cx, "lane-delivery").is_some()
    });
    assert!(bounds(cx, "lane-delivery-head").is_some());
    assert!(bounds(cx, "lane-delivery-branch").is_some());
}

#[gpui::test]
fn a_delivered_lane_with_a_pull_request_opens_it(cx: &mut TestAppContext) {
    let delivery = serde_json::json!({"kind": "pull_request", "head": "a9102f70c7c7", "branch": "lane/demo", "url": "https://example.com/pr/12"});
    let (_rig, desk, cx) = open(cx, &with_demo_ended("delivered", Some(delivery)));
    desk.update(cx, |desk, cx| {
        desk.open_lane(&engine_rig_tests::lane_key("demo"), cx)
    });
    wait_until(cx, "the delivery", |cx| {
        bounds(cx, "lane-delivery").is_some()
    });
    click(cx, "lane-delivery");
    assert_eq!(
        cx.opened_url().as_deref(),
        Some("https://example.com/pr/12")
    );
}

#[gpui::test]
fn a_long_branch_gives_way_and_the_head_is_shown_whole_on_its_own(cx: &mut TestAppContext) {
    let branch = format!("feat/flow-{}", "a-very-long-lane-name-".repeat(6));
    let delivery = serde_json::json!({"kind": "push", "head": "a9102f70c7c7deadbeef", "branch": branch, "url": ""});
    let (_rig, desk, cx) = open(cx, &with_demo_ended("delivered", Some(delivery)));
    desk.update(cx, |desk, cx| {
        desk.open_lane(&engine_rig_tests::lane_key("demo"), cx)
    });
    wait_until(cx, "the head", |cx| {
        bounds(cx, "lane-delivery-head").is_some()
    });
    let (head, branch_box, side) = (
        bounds(cx, "lane-delivery-head").expect("the head"),
        bounds(cx, "lane-delivery-branch").expect("the branch"),
        bounds(cx, "lane-side").expect("the side column"),
    );
    assert!(
        head.right() <= side.right(),
        "the head is inside the column, not cut off"
    );
    assert!(
        branch_box.right() <= head.left() + gpui::px(1.),
        "the branch ends before the head begins"
    );
    assert!(
        branch_box.size.width < gpui::px(300.),
        "only the branch gave way"
    );
}

#[test]
fn a_push_to_a_remote_on_this_mac_is_labelled_as_a_push_like_any_other() {
    use super::lane_page::delivery_words;
    assert_eq!(delivery_words("local"), delivery_words("push"));
    assert_eq!(
        delivery_words("push"),
        crate::l10n::Key("lane.delivery.push")
    );
    assert_ne!(delivery_words("pull_request"), delivery_words("push"));
}

/// The fixture with its outside lane working in `/work/digest`, a folder no project owns.
fn with_outside_lane_in_a_folder_of_no_project() -> String {
    let mut snapshot: serde_json::Value = serde_json::from_str(SNAPSHOT).expect("the fixture");
    let items = snapshot["items"].as_array_mut().expect("the items");
    for item in items.iter_mut().filter(|item| item["kind"] == "outside") {
        item["repo"] = "/work/digest".into();
    }
    snapshot.to_string()
}

#[gpui::test]
fn a_folder_no_project_owns_is_not_listed_as_a_project_but_its_lane_is_listed_from_outside(
    cx: &mut TestAppContext,
) {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let text = with_outside_lane_in_a_folder_of_no_project();
    let read = Snapshot::parse(text.as_bytes()).expect("the snapshot");
    let projects: Vec<String> = lanes::by_project(&read)
        .iter()
        .map(|group| group.folder.clone())
        .collect();
    assert!(
        !projects.contains(&"/work/digest".to_string()),
        "{projects:?}"
    );
    let outside: Vec<&str> = lanes::outside_lanes(&read)
        .iter()
        .map(|item| item.task())
        .collect();
    assert_eq!(outside, ["measure-help"]);
    let (_rig, _desk, cx) = open(cx, &text);
    assert!(bounds(cx, "group-digest").is_none());
    assert!(bounds(cx, "sidebar-outside").is_some());
    assert!(bounds(cx, "row-measure-help").is_some());
}

#[test]
fn the_preset_a_new_lane_starts_under_is_the_engines_own_preset_in_words() {
    let with_engine = |engine: serde_json::Value| {
        let mut document: serde_json::Value = serde_json::from_str(SNAPSHOT).expect("the fixture");
        document["engine"] = engine;
        Snapshot::parse(document.to_string().as_bytes()).expect("the snapshot")
    };
    let named = with_engine(serde_json::json!({"preset": "standard"}));
    assert_eq!(
        lanes::engine_preset_words(&named).as_deref(),
        Some("Standard")
    );
    let labelled = with_engine(serde_json::json!({"preset": "lean", "preset_label": "Lean mode"}));
    assert_eq!(
        lanes::engine_preset_words(&labelled).as_deref(),
        Some("Lean mode")
    );
    let silent = Snapshot::parse(SNAPSHOT.as_bytes()).expect("the snapshot");
    assert_eq!(lanes::engine_preset_words(&silent), None);
}

#[test]
fn an_untitled_lane_uses_its_task_identifier() {
    let mut item = snapshot().items.into_iter().next().unwrap();
    item.title.clear();
    assert_eq!(lanes::lane_words(&item, false), item.task());
}

#[test]
fn lane_words_say_the_name_and_with_option_held_the_branch() {
    let mut demo = snapshot()
        .items
        .into_iter()
        .find(|item| item.task() == "demo")
        .expect("the lane");
    demo.title = "Demo lane".into();
    assert_eq!(lanes::lane_words(&demo, false), "Demo lane");
    assert_eq!(lanes::lane_words(&demo, true), "lane/demo");
    demo.branch.clear();
    assert_eq!(
        lanes::lane_words(&demo, true),
        "Demo lane",
        "a lane with no branch keeps its name"
    );
}

/// Opens the lane `demo` with its Steps menu drawn, over an engine that answers an action with `first`.
fn open_steps_menu<'a>(
    cx: &'a mut TestAppContext,
    first: &str,
) -> (Rig, Entity<Desk>, &'a mut VisualTestContext) {
    let long = "the plan review cannot run: the plan reviewer has no model; set one first in the Engine page";
    let mut snapshot: serde_json::Value = serde_json::from_str(SNAPSHOT).expect("the fixture");
    for item in snapshot["items"].as_array_mut().expect("items") {
        let steps = item
            .get_mut("pipeline")
            .and_then(|steps| steps.as_array_mut());
        for step in steps.into_iter().flatten() {
            if step["id"] == "plan" {
                step["reason"] = long.into();
            }
        }
    }
    let (rig, desk, cx) = open_answering(cx, &snapshot.to_string(), first, first);
    cx.simulate_resize(gpui::size(gpui::px(1280.), gpui::px(800.)));
    desk.update(cx, |desk, cx| {
        desk.open_lane(&engine_rig_tests::lane_key("demo"), cx)
    });
    wait_until(cx, "the lane page", |cx| bounds(cx, "lane-steps").is_some());
    click(cx, "lane-steps");
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    (rig, desk, cx)
}

#[gpui::test]
fn a_step_turned_on_waits_for_the_snapshot_before_the_header_shows_it(cx: &mut TestAppContext) {
    let (rig, _desk, cx) = open_steps_menu(cx, DONE);
    assert!(
        bounds(cx, "step-seg-final_review").is_none(),
        "off to begin with"
    );
    rig.say_delay("0.5");
    click(cx, "steps-row-final_review");
    assert!(bounds(cx, "lane-step-pending").is_some());
    assert!(bounds(cx, "step-seg-final_review").is_none());
    rig.say_snapshot(&with_final_review());
    wait_until(cx, "the call", |_| !rig.acts().is_empty());
    wait_until(cx, "the bar", |cx| {
        cx.update(|window, cx| window.draw(cx).clear(cx));
        bounds(cx, "step-seg-final_review").is_some()
    });
}

#[gpui::test]
fn a_step_the_engine_refuses_to_turn_on_goes_back_off_in_the_header(cx: &mut TestAppContext) {
    let refused =
        r#"{"result": "refused", "reason": "the final review cannot run: it has no model"}"#;
    let (rig, _desk, cx) = open_steps_menu(cx, refused);
    click(cx, "steps-row-final_review");
    wait_until(cx, "the call", |_| !rig.acts().is_empty());
    for _ in 0..40 {
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(50));
    }
    assert!(
        bounds(cx, "step-seg-final_review").is_none(),
        "refused, so still off"
    );
}

#[gpui::test]
fn the_steps_menu_stays_inside_the_window_at_1280_by_800_with_its_lines_wrapped(
    cx: &mut TestAppContext,
) {
    let (_rig, _desk, cx) = open_steps_menu(cx, DONE);
    let window = cx.update(|window, _| window.viewport_size());
    for step in [
        "plan",
        "plan_review",
        "build",
        "task_review",
        "final_review",
        "checks",
    ] {
        let Some(row) = bounds(cx, &format!("steps-row-{step}")) else {
            continue;
        };
        assert!(
            row.right() <= window.width && row.left() >= gpui::px(0.),
            "{step} lies inside the window: {row:?} in {window:?}"
        );
        assert!(
            row.size.width <= bounds(cx, "dropdown-surface").unwrap().size.width,
            "{step} is wrapped to the menu: {row:?}"
        );
    }
}

#[gpui::test]
fn a_review_turned_on_shows_in_details_only_after_the_snapshot_confirms(cx: &mut TestAppContext) {
    let (rig, _desk, cx) = open_steps_menu(cx, DONE);
    assert!(
        bounds(cx, "review-row-final_review").is_none(),
        "off to begin with"
    );
    click(cx, "steps-row-final_review");
    assert!(bounds(cx, "review-row-final_review").is_none());
    rig.say_snapshot(&with_final_review());
    wait_until(cx, "the call", |_| !rig.acts().is_empty());
    rig.facts.update(cx, |facts, cx| facts.read(cx));
    wait_until(cx, "the row", |cx| {
        cx.update(|window, cx| window.draw(cx).clear(cx));
        bounds(cx, "review-row-final_review").is_some()
    });
}

fn with_final_review() -> String {
    let mut document: serde_json::Value = serde_json::from_str(SNAPSHOT).unwrap();
    let pipeline = document["items"][0]["pipeline"].as_array_mut().unwrap();
    let step = pipeline
        .iter_mut()
        .find(|step| step["id"] == "final_review")
        .unwrap();
    step["on"] = true.into();
    step["state"] = "next".into();
    document.to_string()
}

const DELIVER_NOTE: &str = "The lane stopped at the pull request: open it, then resume.";

/// The fixture with the lane's deliver step waiting, as the engine sends it when the lane stops at the pull request.
fn deliver_waits_snapshot() -> String {
    let mut snapshot: serde_json::Value = serde_json::from_str(SNAPSHOT).expect("the fixture");
    for item in snapshot["items"].as_array_mut().expect("items") {
        let Some(steps) = item
            .get_mut("pipeline")
            .and_then(|steps| steps.as_array_mut())
        else {
            continue;
        };
        for step in steps {
            if step["id"] == "deliver" {
                step["state"] = "waiting".into();
                step["note"] = DELIVER_NOTE.into();
            }
        }
    }
    snapshot.to_string()
}

#[gpui::test]
fn details_says_a_step_the_lane_waits_in_needs_you_with_the_engine_s_note(cx: &mut TestAppContext) {
    let (_rig, desk, cx) = open_answering(cx, &deliver_waits_snapshot(), DONE, DONE);
    desk.update(cx, |desk, cx| {
        desk.open_lane(&engine_rig_tests::lane_key("demo"), cx)
    });
    wait_until(cx, "the lane page", |cx| {
        bounds(cx, "step-row-deliver").is_some()
    });
    assert!(
        bounds(cx, "step-state-deliver").is_some_and(|state| state.size.width > gpui::px(0.)),
        "the step says Needs you"
    );
    assert!(
        bounds(cx, "step-note-deliver").is_some(),
        "the engine's note is in Details under the step"
    );
}

#[gpui::test]
fn the_steps_menu_says_a_step_the_lane_waits_in_needs_you_with_the_engine_s_note(
    cx: &mut TestAppContext,
) {
    let snapshot = deliver_waits_snapshot();
    let (rig, desk, cx) = open_answering(cx, &snapshot, DONE, DONE);
    let _ = rig;
    cx.simulate_resize(gpui::size(gpui::px(1280.), gpui::px(800.)));
    desk.update(cx, |desk, cx| {
        desk.open_lane(&engine_rig_tests::lane_key("demo"), cx)
    });
    wait_until(cx, "the lane page", |cx| bounds(cx, "lane-steps").is_some());
    click(cx, "lane-steps");
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        bounds(cx, "steps-note-deliver").is_some(),
        "the engine's words are in the menu under the step"
    );
}

#[gpui::test]
fn a_stopped_lane_offers_close_and_waits_for_confirmation(cx: &mut TestAppContext) {
    let listed = serde_json::json!([
        {"key": "R", "label": "Resume", "needs": [], "optional": [], "confirm": true},
        {"key": "C", "label": "Clean up", "needs": [], "optional": [], "confirm": true}
    ]);
    let mut snapshot: serde_json::Value = serde_json::from_str(&with_demo_actions(listed)).unwrap();
    let lane = snapshot["items"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|item| item["kind"] == "flow" && item["key"][2] == "demo")
        .unwrap();
    lane["agent_state"] = "stopped".into();
    lane["title"] = "Version option".into();
    let result = r#"{"result":"done","exit_code":0,"output":"discarded demo\n"}"#;
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let (rig, desk, cx) = open_answering(cx, &snapshot.to_string(), CONFIRM, result);
    let target = engine_rig_tests::lane_key("demo");
    desk.update(cx, |desk, cx| desk.open_lane(&target, cx));
    wait_until(cx, "the lane page", |cx| bounds(cx, "lane-more").is_some());
    click(cx, "lane-more");
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.simulate_keystrokes("down down enter");
    let confirm = format!("confirm-{}", super::answers::lane_key(&target, "C"));
    wait_until(cx, "the Close confirmation", |cx| {
        bounds(cx, &confirm).is_some()
    });
    assert_eq!(rig.acts().len(), 1);
    assert!(rig.acts()[0].starts_with("cockpit act C "));
    assert!(!rig.acts()[0].contains("--confirm"));
    click(cx, &confirm);
    wait_until(cx, "the confirmed Close", |_| rig.acts().len() == 2);
    assert!(rig.acts()[1].contains("--confirm 17.ab"));
    let answers = desk.read_with(cx, |desk, _| desk.answers.clone());
    let key = super::answers::lane_key(&target, "C");
    wait_until(cx, "the Close result", |cx| {
        answers.read_with(cx, |answers, _| answers.told(&key).is_some())
    });
    answers.read_with(cx, |answers, _| {
        assert_eq!(answers.told(&key), Some("Closed Version option."));
    });
}

#[gpui::test]
fn polish_s_stop_is_quiet_until_confirmed_and_names_the_lane(cx: &mut TestAppContext) {
    let listed = serde_json::json!([
        {"key": "s", "label": "Stop", "needs": [], "optional": [], "confirm": true}
    ]);
    let mut snapshot: serde_json::Value = serde_json::from_str(&with_demo_actions(listed)).unwrap();
    snapshot["items"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|item| item["key"][2] == "demo")
        .unwrap()["title"] = "Version option".into();
    let result = r#"{"result":"done","exit_code":0,"output":"stop requested for demo"}"#;
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let (rig, desk, cx) = open_answering(cx, &snapshot.to_string(), CONFIRM, result);
    rig.say_delay("0.2");
    let target = engine_rig_tests::lane_key("demo");
    desk.update(cx, |desk, cx| desk.open_lane(&target, cx));
    wait_until(cx, "the lane page", |cx| bounds(cx, "lane-more").is_some());
    let page = desk.read_with(cx, |desk, cx| {
        desk.lane_for_test(&target, cx).unwrap().clone()
    });
    page.update(cx, |page, cx| page.press_lane_action("s", cx));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let key = super::answers::lane_key(&target, "s");
    assert!(bounds(cx, &format!("sending-{key}")).is_none());
    let confirm = format!("confirm-{key}");
    wait_until(cx, "the Stop confirmation", |cx| {
        bounds(cx, &confirm).is_some()
    });
    assert_eq!(rig.acts().len(), 1);
    click(cx, &confirm);
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(bounds(cx, &format!("sending-{key}")).is_some());
    let answers = desk.read_with(cx, |desk, _| desk.answers.clone());
    wait_until(cx, "the Stop result", |cx| {
        answers.read_with(cx, |answers, _| answers.told(&key).is_some())
    });
    assert_eq!(
        answers
            .read_with(cx, |answers, _| answers.told(&key).map(str::to_owned))
            .as_deref(),
        Some("Stop requested for Version option.")
    );
}

#[gpui::test]
fn polish_zd_steps_stay_on_one_row_at_minimum_width(cx: &mut TestAppContext) {
    let (_rig, desk, cx) = open(cx, SNAPSHOT);
    cx.simulate_resize(gpui::size(gpui::px(1100.), gpui::px(700.)));
    desk.update(cx, |desk, cx| {
        desk.open_lane(&engine_rig_tests::lane_key("demo"), cx)
    });
    wait_until(cx, "Steps", |cx| bounds(cx, "step-seg-deliver").is_some());
    let plan = bounds(cx, "step-seg-plan").unwrap();
    let deliver = bounds(cx, "step-seg-deliver").unwrap();
    assert_eq!(plan.top(), deliver.top());
}

#[gpui::test]
fn polish_zd_steps_menu_encloses_the_custom_rows(cx: &mut TestAppContext) {
    let (_rig, _desk, cx) = open_steps_menu(cx, DONE);
    let row = bounds(cx, "steps-row-plan").unwrap();
    let outer = bounds(cx, "dropdown-item-0").unwrap();
    assert!(row.right() < outer.right(), "{row:?} leaves {outer:?}");
}
