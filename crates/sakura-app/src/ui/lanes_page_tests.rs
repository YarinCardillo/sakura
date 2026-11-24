//! The Lanes page: every lane under the state it is in, its step and progress, the project and preset
//! filters, and the way into a lane and into a new one.

use super::engine_desk::Desk;
use super::engine_rig_tests::{accent_quads, SNAPSHOT};
use super::lane_tests::{bounds, click, open_answering};
use super::lanes::{self, LaneFilter, LaneGroup};
use gpui::{Entity, Modifiers, TestAppContext, VisualTestContext};
use sakura_core::engine::snapshot::Snapshot;
use sakura_core::tabs::Page;

const ANSWER: &str = r#"{"result": "done", "exit_code": 0, "output": ""}"#;

#[test]
fn the_list_counts_and_names_the_engines_current_pipeline_step() {
    let mut fixture: serde_json::Value = serde_json::from_str(SNAPSHOT).unwrap();
    let lane = &mut fixture["items"][0];
    lane["pipeline"] = serde_json::json!([
        {"id":"plan","state":"running","on":true},
        {"id":"plan_review","state":"next","on":true},
        {"id":"deliver","state":"waiting","on":true},
        {"id":"reporter","state":"next","on":true}
    ]);
    lane["current_step"] = serde_json::json!({"id":"plan_review"});
    let snapshot = parsed(&fixture.to_string());
    let words = lanes::step_words(&snapshot.items[0]);
    assert_eq!(words.name.as_ref(), "Plan review");
    assert_eq!(words.place, Some((2, 4)));
    assert_eq!(words.counts, Some((1, 4)));

    fixture["items"][0]["current_step"] = serde_json::json!({"id":"deliver","waiting":true});
    let snapshot = parsed(&fixture.to_string());
    let words = lanes::step_words(&snapshot.items[0]);
    assert_eq!(words.name.as_ref(), "Deliver");
    assert_eq!(words.place, Some((3, 4)));
}

#[gpui::test]
fn two_thousand_lanes_draw_the_viewport_and_can_scroll_to_the_end(cx: &mut TestAppContext) {
    let mut fixture: serde_json::Value = serde_json::from_str(SNAPSHOT).unwrap();
    let template = fixture["items"][0].clone();
    let total = 2_000;
    fixture["items"] = (0..total)
        .map(|index| {
            let title = format!("lane-{index:04}");
            let mut item = template.clone();
            let key = serde_json::json!(["flow", "/work/repo", title]);
            item["key"] = key.clone();
            item["id"] = key.to_string().into();
            item["task"] = title.clone().into();
            item["title"] = title.into();
            item["last_activity_at"] = serde_json::Value::Null;
            item["agent_state"] = "idle".into();
            item
        })
        .collect::<Vec<_>>()
        .into();
    fixture["waits"] = serde_json::json!([]);
    let (_rig, desk, cx) = open_answering(cx, &fixture.to_string(), ANSWER, ANSWER);
    desk.update(cx, |desk, cx| desk.navigate(Page::Lanes, false, cx));
    super::engine_rig_tests::wait_until(cx, "the first lane", |cx| {
        bounds(cx, "lanes-row-lane-0000").is_some()
    });
    let drawn = (0..total)
        .filter(|index| bounds(cx, &format!("lanes-row-lane-{index:04}")).is_some())
        .count();
    eprintln!("Lanes viewport built {drawn} of {total} rows");
    assert!(drawn < 100, "drawn: {drawn}");
    cx.simulate_keystrokes("end");
    super::engine_rig_tests::wait_until(cx, "the last lane", |cx| {
        bounds(cx, "lanes-row-lane-1999").is_some()
    });
}

/// The fixture changed so that every group has a lane: `fresh` delivered, `stuck` stopped with its ask
/// answered, and `busy` a working lane of `/work/repo`. `demo` and `gated` keep their open asks.
fn with_every_group() -> String {
    let mut document: serde_json::Value = serde_json::from_str(SNAPSHOT).expect("the fixture");
    let items = document["items"].as_array_mut().expect("the items");
    let mut busy = items[0].clone();
    busy["key"] = serde_json::json!(["flow", "/work/repo", "busy"]);
    busy["id"] = serde_json::json!(r#"["flow","/work/repo","busy"]"#);
    busy["agent_state"] = "working".into();
    for item in items.iter_mut() {
        match item["key"][2].as_str() {
            Some("fresh") => item["agent_state"] = "delivered".into(),
            Some("stuck") => item["agent_state"] = "stopped".into(),
            _ => {}
        }
    }
    items.push(busy);
    let waits = document["waits"].as_array_mut().expect("the waits");
    waits.retain(|wait| wait["task"] != "stuck");
    document.to_string()
}

fn parsed(text: &str) -> Snapshot {
    Snapshot::parse(text.as_bytes()).expect("the snapshot")
}

fn tasks(snapshot: &Snapshot, filter: &LaneFilter, group: LaneGroup) -> Vec<String> {
    let found = lanes::grouped(snapshot, filter);
    found
        .of(group)
        .iter()
        .map(|item| item.task().to_string())
        .collect()
}

fn active_page(desk: &Entity<Desk>, cx: &mut VisualTestContext) -> Page {
    desk.read_with(cx, |desk, cx| {
        desk.bar.read(cx).tabs().active().page().clone()
    })
}

fn open_lanes<'a>(
    snapshot: &str,
    cx: &'a mut TestAppContext,
) -> (Entity<Desk>, &'a mut VisualTestContext) {
    let (_rig, desk, cx) = open_answering(cx, snapshot, ANSWER, ANSWER);
    desk.update(cx, |desk, cx| desk.navigate(Page::Lanes, false, cx));
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    (desk, cx)
}

#[gpui::test]
fn lane_group_folds_survive_relaunch_independently(cx: &mut TestAppContext) {
    use crate::prefs::Preferences;

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("preferences.json");
    let snapshot = with_every_group();
    {
        let (_, window) = open_lanes(&snapshot, cx);
        window.update(|_, cx| Preferences::default().install(Some(path.clone()), cx));
        click(window, "lanes-group-finished");
        click(window, "lanes-group-stopped");
        click(window, "lanes-group-finished");
        window.run_until_parked();
        assert!(bounds(window, "lanes-row-stuck").is_some());
        assert!(bounds(window, "lanes-row-fresh").is_none());
    }

    let kept = Preferences::load_from(&path);
    let (_, desk, window) =
        super::lane_tests::open_answering_with(cx, &snapshot, (ANSWER, ANSWER), kept);
    desk.update(window, |desk, cx| desk.navigate(Page::Lanes, false, cx));
    super::engine_rig_tests::wait_until(window, "the stopped header", |cx| {
        bounds(cx, "lanes-group-stopped").is_some()
    });
    assert!(bounds(window, "lanes-row-stuck").is_some());
    assert!(bounds(window, "lanes-row-fresh").is_none());
}

#[test]
fn the_fixture_gives_the_four_groups_with_their_lanes() {
    let snapshot = parsed(&with_every_group());
    let none = LaneFilter::default();
    let mut needs = tasks(&snapshot, &none, LaneGroup::NeedsYou);
    needs.sort();
    assert_eq!(needs, ["demo", "gated", "measure-help"]);
    assert_eq!(tasks(&snapshot, &none, LaneGroup::Running), ["busy"]);
    assert_eq!(tasks(&snapshot, &none, LaneGroup::Finished), ["fresh"]);
    assert_eq!(tasks(&snapshot, &none, LaneGroup::Stopped), ["stuck"]);
}

#[test]
fn a_lane_with_an_open_ask_is_under_needs_you_only() {
    let mut document: serde_json::Value = serde_json::from_str(&with_every_group()).unwrap();
    document["items"][0]["agent_state"] = "working".into();
    let snapshot = parsed(&document.to_string());
    let none = LaneFilter::default();
    assert!(tasks(&snapshot, &none, LaneGroup::NeedsYou).contains(&"demo".to_string()));
    for group in [LaneGroup::Running, LaneGroup::Finished, LaneGroup::Stopped] {
        assert!(!tasks(&snapshot, &none, group).contains(&"demo".to_string()));
    }
    let listed: usize = LaneGroup::ALL
        .iter()
        .map(|group| tasks(&snapshot, &none, *group).len())
        .sum();
    assert_eq!(
        listed,
        snapshot.items.len(),
        "every lane is in exactly one group"
    );
}

#[test]
fn a_gate_with_only_snoozed_asks_leaves_the_needs_you_group() {
    let mut document: serde_json::Value = serde_json::from_str(&with_every_group()).unwrap();
    let wait = document["waits"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|wait| wait["task"] == "gated")
        .unwrap();
    wait["is_snoozed"] = true.into();
    wait["snoozed_until"] = "2099-11-22T18:00:00Z".into();
    let snapshot = parsed(&document.to_string());
    let needs_you = tasks(&snapshot, &LaneFilter::default(), LaneGroup::NeedsYou);
    assert!(!needs_you.iter().any(|task| task == "gated"));
    assert!(needs_you.iter().any(|task| task == "demo"));
}

#[test]
fn the_project_filter_keeps_the_lanes_of_that_project() {
    let snapshot = parsed(&with_every_group());
    let filter = LaneFilter {
        project: Some("/work/other".into()),
        preset: None,
    };
    let kept: Vec<String> = LaneGroup::ALL
        .iter()
        .flat_map(|group| tasks(&snapshot, &filter, *group))
        .collect();
    let mut kept = kept;
    kept.sort();
    assert_eq!(kept, ["fresh", "stuck"]);
}

#[test]
fn the_lanes_page_and_the_lane_header_count_the_same_steps_and_say_which_one_the_lane_is_at() {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let mut document: serde_json::Value = serde_json::from_str(SNAPSHOT).unwrap();
    let demo = document["items"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|item| item["key"][2] == "demo")
        .unwrap();
    let step = |id: &str, state: &str| serde_json::json!({"id": id, "name": id, "state": state});
    demo["pipeline"] = serde_json::json!([]);
    demo["steps"] = serde_json::json!([
        step("plan", "done"),
        step("build", "done"),
        step("checks", "skipped"),
        step("deliver", "now")
    ]);
    let snapshot = parsed(&document.to_string());
    let item = snapshot
        .items
        .iter()
        .find(|item| item.task() == "demo")
        .unwrap();
    let drawn = super::step_track::running_track(&item.pipeline, &item.steps, false).len();
    assert_eq!(
        drawn, 3,
        "the header draws the steps the lane runs, not the one turned off"
    );
    assert_eq!(
        lanes::step_line(&lanes::step_words(item)),
        "Deliver, step 3 of 3",
        "the page counts the steps the header draws"
    );
}

#[test]
fn a_snapshot_without_steps_shows_the_phase_of_the_progress() {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let snapshot = parsed(SNAPSHOT);
    let words = |task: &str| {
        let item = snapshot
            .items
            .iter()
            .find(|item| item.task() == task)
            .unwrap();
        lanes::step_line(&lanes::step_words(item)).to_string()
    };
    assert_eq!(
        words("demo"),
        "Build, step 3 of 7",
        "among the steps the lane runs"
    );
    assert_eq!(words("fresh"), "Intake", "no plan yet: the phase alone");
}

#[test]
fn a_preset_is_said_by_the_label_the_engine_gave_and_never_by_its_id() {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let mut document: serde_json::Value = serde_json::from_str(SNAPSHOT).unwrap();
    document["items"][0]["preset"] = "night-shift".into();
    document["items"][0]["preset_label"] = "Night shift".into();
    let snapshot = parsed(&document.to_string());
    assert_eq!(lanes::preset_words(&snapshot, "night-shift"), "Night shift");
    assert_eq!(lanes::preset_words(&snapshot, "lean"), "Lean");
    let choices = lanes::preset_choices_of(&snapshot);
    assert_eq!(choices[0].words(), "Night shift");
    assert_eq!(choices[0].name, "night-shift");
}

#[test]
fn a_flow_lane_s_step_progress_is_said_in_the_app_s_words_from_the_engine_s_own_phase() {
    crate::l10n::set_for_test(crate::l10n::Language::Italian);
    let mut document: serde_json::Value = serde_json::from_str(SNAPSHOT).unwrap();
    document["items"][0]["progress"] =
        serde_json::json!({"done": 1, "total": 4, "phase": "Build", "engine_phase": "build"});
    let snapshot = parsed(&document.to_string());
    let progress = snapshot.items[0].progress.as_ref().unwrap();
    assert_eq!(lanes::phase_word(progress.phase_id()), "Costruzione");
}

#[test]
fn steps_from_the_engine_name_the_step_and_count_the_done_ones() {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let mut document: serde_json::Value = serde_json::from_str(SNAPSHOT).unwrap();
    document["items"][0]["pipeline"] = serde_json::json!([]);
    document["items"][0]["steps"] = serde_json::json!([
        {"id": "plan", "name": "Plan", "state": "done"},
        {"id": "build", "name": "Build", "state": "done"},
        {"id": "checks", "name": "Checks", "state": "now"},
        {"id": "deliver", "name": "Deliver", "state": "next"}
    ]);
    document["items"][0]["preset"] = "careful".into();
    let snapshot = parsed(&document.to_string());
    let item = &snapshot.items[0];
    assert_eq!(
        lanes::step_line(&lanes::step_words(item)),
        "Checks, step 3 of 4"
    );
    assert_eq!(lanes::presets_of(&snapshot), ["careful"]);
    let only = LaneFilter {
        project: None,
        preset: Some("careful".into()),
    };
    let kept: usize = LaneGroup::ALL
        .iter()
        .map(|g| tasks(&snapshot, &only, *g).len())
        .sum();
    assert_eq!(kept, 1, "the preset filter keeps the lanes of that preset");
}

#[gpui::test]
fn the_step_column_keeps_its_place_from_the_row_s_right_edge(cx: &mut TestAppContext) {
    let (_desk, cx) = open_lanes(&with_every_group(), cx);
    let row = bounds(cx, "lanes-row-busy").expect("a row");
    let step = bounds(cx, "lanes-row-busy-step").expect("its step");
    // Age, preset and step, two gaps, then the row's own padding.
    let tokens = crate::theme::lanes_page::ROW_GAP;
    let (gap, padding) = (
        f32::from(tokens),
        f32::from(crate::theme::lanes_page::ROW_PADDING_X),
    );
    assert_eq!(
        f32::from(row.right() - step.left()),
        f32::from(crate::theme::lanes_page::AGE_WIDTH) + 72. + 156. + gap * 2. + padding
    );
}

#[gpui::test]
fn the_new_lane_button_is_painted_with_the_sheet_fill_and_not_left_nearly_bare(
    cx: &mut TestAppContext,
) {
    let (_desk, cx) = open_lanes(&with_every_group(), cx);
    let button = bounds(cx, "lanes-new-lane").expect("the button");
    let fill = cx.update(|_, cx| crate::theme::Theme::palette(cx).fill);
    let is_filled = cx.update(|window, _| {
        let at = button.center().scale(window.scale_factor());
        window
            .painted_quads()
            .iter()
            .filter(|quad| quad.bounds.contains(&at))
            .any(|quad| quad.background.as_solid() == Some(fill))
    });
    assert!(
        is_filled,
        "a box of the fill colour under the button's middle"
    );
}

#[gpui::test]
fn the_page_lists_the_open_groups_with_their_rows_and_folds_finished_and_stopped(
    cx: &mut TestAppContext,
) {
    let (_desk, cx) = open_lanes(&with_every_group(), cx);
    for group in ["needs_you", "running", "finished", "stopped"] {
        assert!(
            bounds(cx, &format!("lanes-group-{group}")).is_some(),
            "{group}"
        );
    }
    for task in ["demo", "gated", "measure-help", "busy"] {
        assert!(bounds(cx, &format!("lanes-row-{task}")).is_some(), "{task}");
    }
    assert!(
        bounds(cx, "lanes-row-fresh").is_none(),
        "finished is folded"
    );
    assert!(bounds(cx, "lanes-row-stuck").is_none(), "stopped is folded");
    click(cx, "lanes-group-finished");
    assert!(
        bounds(cx, "lanes-row-fresh").is_some(),
        "a press opens the group"
    );
    click(cx, "lanes-group-finished");
    assert!(
        bounds(cx, "lanes-row-fresh").is_none(),
        "and folds it again"
    );
}

#[gpui::test]
fn a_press_on_a_row_opens_the_lane_and_cmd_opens_it_in_a_new_tab(cx: &mut TestAppContext) {
    let (desk, cx) = open_lanes(&with_every_group(), cx);
    let at = bounds(cx, "lanes-row-busy").expect("the row").center();
    cx.simulate_click(at, Modifiers::command());
    cx.run_until_parked();
    let tabs = desk.read_with(cx, |desk, cx| desk.bar.read(cx).tabs().tabs().len());
    assert_eq!(tabs, 2, "Cmd opened a tab of its own");
    let busy = serde_json::json!(["flow", "/work/repo", "busy"]).to_string();
    assert_eq!(active_page(&desk, cx), Page::Lane(busy));
    desk.update(cx, |desk, cx| desk.navigate(Page::Lanes, false, cx));
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    click(cx, "lanes-row-gated");
    cx.run_until_parked();
    let expected = Page::Lane(super::engine_rig_tests::lane_key("gated"));
    assert_eq!(active_page(&desk, cx), expected);
}

#[gpui::test]
fn a_project_row_of_the_sidebar_opens_the_lanes_of_that_project_only(cx: &mut TestAppContext) {
    let (desk, cx) = open_lanes(&with_every_group(), cx);
    click(cx, "lanes-of-other");
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert_eq!(active_page(&desk, cx), Page::Lanes);
    assert!(
        bounds(cx, "lanes-row-stuck").is_none(),
        "stopped stays folded"
    );
    assert!(bounds(cx, "lanes-group-finished").is_some());
    assert!(
        bounds(cx, "lanes-row-demo").is_none(),
        "a lane of another project"
    );
    assert!(bounds(cx, "lanes-row-busy").is_none());
    assert!(
        bounds(cx, "lanes-clear-filter").is_some(),
        "the filter can be taken off"
    );
}

#[gpui::test]
fn the_new_lane_button_opens_the_sheet(cx: &mut TestAppContext) {
    let (_desk, cx) = open_lanes(&with_every_group(), cx);
    click(cx, "lanes-new-lane");
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(bounds(cx, "new-lane-sheet").is_some());
}

#[gpui::test]
fn the_lanes_page_draws_nothing_in_the_accent_whatever_needs_the_person(cx: &mut TestAppContext) {
    let (_desk, cx) = open_lanes(&with_every_group(), cx);
    assert!(bounds(cx, "lanes-group-needs_you").is_some());
    assert_eq!(accent_quads(cx), 0, "the accent is for a dot of the Inbox");
}

#[gpui::test]
fn the_list_has_a_header_row_over_its_fixed_columns(cx: &mut TestAppContext) {
    let (_desk, cx) = open_lanes(&with_every_group(), cx);
    let header = bounds(cx, "lanes-columns").expect("a header row");
    let row = bounds(cx, "lanes-row-busy").expect("a row");
    assert!(header.bottom() <= row.top());
    for column in ["project", "step", "preset", "age"] {
        let at = bounds(cx, &format!("lanes-column-{column}")).expect(column);
        let cell = bounds(cx, &format!("lanes-row-busy-{column}")).expect(column);
        assert_eq!(
            (at.left(), at.right()),
            (cell.left(), cell.right()),
            "the {column} column's header stands over its cells"
        );
    }
}

/// The fixture with every lane stopped and no ask.
fn all_stopped() -> String {
    let mut document: serde_json::Value = serde_json::from_str(SNAPSHOT).expect("the fixture");
    document["waits"] = serde_json::json!([]);
    for item in document["items"].as_array_mut().expect("the items") {
        item["agent_state"] = "stopped".into();
    }
    document.to_string()
}

#[gpui::test]
fn when_only_stopped_lanes_exist_their_rows_are_listed_instead_of_one_folded_band(
    cx: &mut TestAppContext,
) {
    let (_desk, cx) = open_lanes(&all_stopped(), cx);
    assert!(bounds(cx, "lanes-row-demo").is_some(), "the rows are there");
}

#[test]
fn a_lane_an_outside_agent_opened_names_the_agent_and_how_many_asks_are_left() {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let mut document: serde_json::Value = serde_json::from_str(SNAPSHOT).expect("the fixture");
    let outside = document["items"]
        .as_array_mut()
        .expect("the items")
        .iter_mut()
        .find(|item| item["kind"] == "outside")
        .expect("the outside lane");
    outside["author"] = "outside:digest".into();
    outside["progress"] = serde_json::json!({"done": 2, "total": 6, "phase": "ready"});
    let snapshot = parsed(&document.to_string());
    let item = snapshot
        .items
        .iter()
        .find(|item| item.is_outside())
        .expect("the lane");
    assert_eq!(super::lanes_page::preset_words(item), "Digest");
    assert_eq!(
        super::lanes_page::progress_words(item, &snapshot, super::unix_now()).as_ref(),
        "4 to decide"
    );
    let flow = snapshot.items.iter().find(|item| item.task() == "demo");
    assert_eq!(
        super::lanes_page::progress_words(flow.expect("a flow lane"), &snapshot, super::unix_now())
            .as_ref(),
        "Build, step 3 of 7"
    );
}

#[test]
fn outside_progress_excludes_the_snoozed_ask() {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let mut document: serde_json::Value = serde_json::from_str(SNAPSHOT).unwrap();
    let wait = document["waits"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|wait| wait["task"] == "measure-help")
        .unwrap();
    wait["is_snoozed"] = true.into();
    wait["snoozed_until"] = "2099-11-22T18:00:00Z".into();
    let snapshot = parsed(&document.to_string());
    let item = snapshot
        .items
        .iter()
        .find(|item| item.is_outside())
        .unwrap();
    assert_eq!(
        super::lanes_page::progress_words(item, &snapshot, super::unix_now()).as_ref(),
        "0 to decide"
    );
}

/// What the real engine prints with an outside lane and a day's morning digest.
const WITH_DIGEST: &str =
    include_str!("../../../sakura-core/tests/fixtures/engine-real-snapshot-outside.json");

#[test]
fn a_morning_digest_is_in_no_list_of_lanes_or_projects_and_is_no_open_lane() {
    let snapshot = Snapshot::parse(WITH_DIGEST.as_bytes()).expect("the real snapshot");
    assert!(snapshot.items.iter().any(|item| item.is_digest()));
    let groups = lanes::grouped(&snapshot, &LaneFilter::default());
    let listed = [
        LaneGroup::NeedsYou,
        LaneGroup::Running,
        LaneGroup::Finished,
        LaneGroup::Stopped,
    ]
    .into_iter()
    .flat_map(|group| groups.of(group).iter().copied());
    assert!(listed.clone().count() > 0, "the lanes are listed");
    assert!(listed.into_iter().all(|item| !item.is_digest()));
    let in_projects = lanes::by_project(&snapshot);
    assert!(in_projects
        .iter()
        .flat_map(|group| &group.lanes)
        .all(|item| !item.is_digest()));
    assert!(in_projects.iter().all(|group| !group.name.is_empty()));
    assert!(lanes::outside_lanes(&snapshot)
        .iter()
        .all(|item| !item.is_digest()));
    let lanes_open = snapshot
        .items
        .iter()
        .filter(|item| !item.is_digest() && lanes::is_open(item));
    assert_eq!(lanes::open_count(&snapshot.items), lanes_open.count());
}

#[test]
fn polish_zd_discarded_lanes_are_not_finished() {
    let mut document: serde_json::Value = serde_json::from_str(&with_every_group()).unwrap();
    let lanes = document["items"].as_array_mut().unwrap();
    lanes
        .iter_mut()
        .find(|lane| lane["key"][2] == "fresh")
        .unwrap()["agent_state"] = "cleaned".into();
    let snapshot = parsed(&document.to_string());
    assert!(tasks(&snapshot, &LaneFilter::default(), LaneGroup::Finished).is_empty());
}
