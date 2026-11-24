//! "New lane" as a person meets it: Cmd-N and the controls that open the sheet, its title and description, the
//! engine starting the lane, and the lane's tab opening.

use super::engine_desk::Desk;
use super::engine_rig_tests::{self, wait_until, Rig, SNAPSHOT};
use super::lane_tests::{bounds, click, open_answering};
use super::new_lane::{action_inputs, lane_text, NewLaneEvent, NewLaneSheet};
use super::new_lane_view::{pace_in_force, preset_menu};
use gpui::{AppContext as _, Entity, TestAppContext, VisualTestContext};
use gpui_base::actions::{Confirm, SelectDown};
use sakura_core::engine::snapshot::Snapshot;
use sakura_core::tabs::Page;
use std::cell::RefCell;
use std::rc::Rc;

const STARTED: &str = r#"{"result": "done", "exit_code": 0, "output": "brand-new"}"#;
const STARTED_WITHOUT_ID: &str = r#"{"result": "done", "exit_code": 0, "output": ""}"#;
const REFUSED: &str = r#"{"result": "refused", "reason": "the ceiling is 1 open lane: repo/demo"}"#;
const NEW_TASK: &str = "brand-new";

/// The fixture with one more lane in `/work/repo`, the one the engine starts.
fn with_started_lane() -> String {
    let mut snapshot: serde_json::Value = serde_json::from_str(SNAPSHOT).expect("the fixture");
    let items = snapshot["items"].as_array_mut().expect("the items");
    let mut started = items
        .iter()
        .find(|item| item["key"][2] == "demo")
        .expect("a flow lane of the repo")
        .clone();
    started["key"] = serde_json::json!(["flow", "/work/repo", NEW_TASK]);
    started["id"] = serde_json::json!(r#"["flow","/work/repo","brand-new"]"#);
    items.push(started);
    snapshot.to_string()
}

fn started_key() -> String {
    serde_json::json!(["flow", "/work/repo", NEW_TASK]).to_string()
}

fn pages(desk: &Entity<Desk>, cx: &mut VisualTestContext) -> Vec<Page> {
    desk.read_with(cx, |desk, cx| {
        let tabs = desk.bar.read(cx).tabs();
        tabs.tabs().iter().map(|tab| tab.page().clone()).collect()
    })
}

fn open_sheet<'a>(
    rig_answer: &str,
    cx: &'a mut TestAppContext,
) -> (Rig, Entity<Desk>, &'a mut VisualTestContext) {
    open_sheet_over(SNAPSHOT, rig_answer, cx)
}

/// The sheet opened from the repo project's row, over a fake engine that says `snapshot`.
fn open_sheet_over<'a>(
    snapshot: &str,
    rig_answer: &str,
    cx: &'a mut TestAppContext,
) -> (Rig, Entity<Desk>, &'a mut VisualTestContext) {
    let (rig, desk, cx) = open_answering(cx, snapshot, rig_answer, rig_answer);
    click(cx, "new-lane-repo");
    wait_until(cx, "the sheet", |cx| {
        cx.debug_bounds("new-lane-sheet").is_some()
    });
    (rig, desk, cx)
}

/// A press on the field, then the words: the caret is never there before the press.
fn type_into(cx: &mut VisualTestContext, field: &str, words: &str) {
    click(cx, field);
    cx.simulate_input(words);
}

fn type_into_field(cx: &mut VisualTestContext, words: &str) {
    type_into(cx, "new-lane-title", words);
}

/// Every call of the engine's `act`, as one text.
fn acts_text(rig: &Rig) -> String {
    rig.acts().join("\n")
}

#[test]
fn the_text_is_the_title_and_then_a_blank_line_and_the_description() {
    assert_eq!(
        lane_text("Add a parser", "Retry on 5xx."),
        "Add a parser\n\nRetry on 5xx."
    );
    assert_eq!(
        lane_text("  Add a parser ", "  \n"),
        "Add a parser",
        "no description, no blank line"
    );
    assert_eq!(lane_text("", "Retry on 5xx."), "", "a lane needs its title");
}

#[test]
fn the_preset_and_pace_chosen_are_sent_only_to_an_engine_that_takes_them() {
    let read = |optional: serde_json::Value| {
        let mut document: serde_json::Value = serde_json::from_str(SNAPSHOT).expect("the fixture");
        document["projects"][0]["actions"][0]["optional"] = optional;
        let snapshot =
            sakura_core::engine::snapshot::Snapshot::parse(document.to_string().as_bytes());
        snapshot.expect("the snapshot").projects[0]
            .start_lane()
            .cloned()
            .expect("L")
    };
    let chosen = (Some("careful"), Some("fast"));
    let both = action_inputs(
        &read(serde_json::json!(["preset", "level"])),
        "t".into(),
        chosen,
    );
    assert_eq!(both.get("preset").map(String::as_str), Some("careful"));
    assert_eq!(both.get("level").map(String::as_str), Some("fast"));
    let only_text = action_inputs(&read(serde_json::json!([])), "t".into(), chosen);
    assert_eq!(only_text.len(), 1, "{only_text:?}");
    let none = action_inputs(
        &read(serde_json::json!(["preset", "level"])),
        "t".into(),
        (None, None),
    );
    assert_eq!(none.len(), 1, "nothing chosen, nothing sent");
}

#[gpui::test]
fn every_project_row_has_a_new_lane_control(cx: &mut TestAppContext) {
    let (_rig, _desk, cx) = open_answering(cx, SNAPSHOT, STARTED, STARTED);
    assert!(bounds(cx, "new-lane-repo").is_some());
    assert!(bounds(cx, "new-lane-other").is_some());
}

#[gpui::test]
fn cmd_n_opens_the_sheet_on_any_page(cx: &mut TestAppContext) {
    let (_rig, desk, cx) = open_answering(cx, SNAPSHOT, STARTED, STARTED);
    cx.update(|_, cx| crate::menus::install(cx));
    desk.update(cx, |desk, cx| desk.navigate(Page::Master, false, cx));
    cx.run_until_parked();
    cx.simulate_keystrokes("cmd-n");
    wait_until(cx, "the sheet", |cx| {
        cx.debug_bounds("new-lane-sheet").is_some()
    });
}

#[gpui::test]
fn opened_from_a_project_row_it_starts_the_lane_in_that_project(cx: &mut TestAppContext) {
    let (rig, _desk, cx) = open_sheet(STARTED, cx);
    type_into_field(cx, "add a parser");
    click(cx, "new-lane-start");
    wait_until(cx, "the call", |_| !rig.acts().is_empty());
    assert!(
        rig.acts()[0].starts_with(r#"cockpit act L ["project","/work/repo"]"#),
        "{}",
        rig.acts()[0]
    );
}

#[gpui::test]
fn start_without_a_title_keeps_its_face_on_hover(cx: &mut TestAppContext) {
    let (_rig, _desk, cx) = open_sheet(STARTED, cx);
    let start = bounds(cx, "new-lane-start").unwrap();
    let painted = |cx: &mut VisualTestContext| {
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            let start = start.scale(window.scale_factor());
            window
                .painted_quads()
                .into_iter()
                .find(|quad| quad.bounds == start)
                .expect("Start's background quad")
                .background
        })
    };
    let before = painted(cx);
    cx.simulate_mouse_move(start.center(), None, gpui::Modifiers::none());
    assert_eq!(painted(cx), before);
}

#[gpui::test]
fn new_lane_sheet_opens_without_the_caret_in_its_fields(cx: &mut TestAppContext) {
    let (rig, _desk, cx) = open_sheet(STARTED, cx);
    // Typing with no press on a field reaches nothing of it: there is no title to start a lane with.
    cx.simulate_input("add a parser");
    click(cx, "new-lane-start");
    cx.run_until_parked();
    assert!(rig.acts().is_empty(), "the fields had no caret");
}

#[gpui::test]
fn start_sends_the_title_a_blank_line_and_the_description(cx: &mut TestAppContext) {
    let (rig, _desk, cx) = open_sheet(STARTED, cx);
    type_into_field(cx, "add a parser");
    type_into(cx, "new-lane-description", "retry on 5xx");
    click(cx, "new-lane-start");
    wait_until(cx, "the call", |_| !rig.acts().is_empty());
    assert!(
        acts_text(&rig).contains("--text add a parser\n\nretry on 5xx"),
        "{}",
        acts_text(&rig)
    );
}

#[gpui::test]
fn cmd_return_starts_the_lane_from_either_field(cx: &mut TestAppContext) {
    let (rig, _desk, cx) = open_sheet(STARTED, cx);
    type_into_field(cx, "add a parser");
    cx.simulate_keystrokes("cmd-enter");
    wait_until(cx, "the call", |_| !rig.acts().is_empty());
    assert!(
        acts_text(&rig).contains("--text add a parser"),
        "{}",
        acts_text(&rig)
    );
}

#[gpui::test]
fn escape_closes_the_sheet_and_sends_nothing(cx: &mut TestAppContext) {
    let (rig, _desk, cx) = open_sheet(STARTED, cx);
    type_into_field(cx, "add a parser");
    cx.simulate_keystrokes("escape");
    wait_until(cx, "the sheet goes away", |cx| {
        cx.debug_bounds("new-lane-sheet").is_none()
    });
    assert!(rig.acts().is_empty());
}

/// The fixture whose lane-starting action also takes a preset and a pace, and in which no lane runs as a preset.
fn accepting_preset_and_pace() -> String {
    let mut document: serde_json::Value = serde_json::from_str(SNAPSHOT).expect("the fixture");
    for project in document["projects"].as_array_mut().expect("the projects") {
        project["actions"][0]["optional"] = serde_json::json!(["preset", "level"]);
    }
    for item in document["items"].as_array_mut().expect("the items") {
        item["preset"] = serde_json::Value::Null;
    }
    let real: serde_json::Value = serde_json::from_str(REAL_SNAPSHOT).expect("the real fixture");
    document["engine"] =
        serde_json::json!({"pace": "normal", "presets": real["engine"]["presets"]});
    document.to_string()
}

#[gpui::test]
fn the_chips_are_not_shown_when_the_engine_takes_only_text(cx: &mut TestAppContext) {
    let (_rig, _desk, cx) = open_sheet(STARTED, cx);
    assert!(bounds(cx, "new-lane-pace").is_none());
    assert!(bounds(cx, "new-lane-preset").is_none());
}

#[gpui::test]
fn the_preset_and_pace_chips_show_when_the_engine_takes_those_inputs(cx: &mut TestAppContext) {
    let (_rig, _desk, cx) = open_sheet_over(&accepting_preset_and_pace(), STARTED, cx);
    assert!(bounds(cx, "new-lane-pace").is_some());
    assert!(bounds(cx, "new-lane-preset").is_some());
}

/// What the real engine printed for its snapshot, with its own action table for starting a lane.
const REAL_SNAPSHOT: &str =
    include_str!("../../../sakura-core/tests/fixtures/engine-real-snapshot.json");

#[gpui::test]
fn the_real_engine_s_start_action_gives_the_new_lane_sheet_its_preset_and_pace_chips(
    cx: &mut TestAppContext,
) {
    let (_rig, _desk, cx) = open_answering(cx, REAL_SNAPSHOT, STARTED, STARTED);
    click(cx, "new-lane-orchard");
    wait_until(cx, "the sheet", |cx| {
        cx.debug_bounds("new-lane-sheet").is_some()
    });
    assert!(bounds(cx, "new-lane-preset").is_some(), "the preset chip");
    assert!(bounds(cx, "new-lane-pace").is_some(), "the pace chip");
}

/// A sheet over the fixture's projects, in the first one, with `title` typed and its events recorded.
fn sheet_over<'a>(
    cx: &'a mut TestAppContext,
    title: &str,
) -> (
    Rig,
    Entity<NewLaneSheet>,
    Rc<RefCell<Vec<NewLaneEvent>>>,
    &'a mut VisualTestContext,
) {
    let rig = engine_rig_tests::open(cx, SNAPSHOT, STARTED, STARTED);
    let snapshot = Snapshot::parse(SNAPSHOT.as_bytes()).expect("the fixture");
    let (projects, source) = (snapshot.projects.clone(), rig.source.clone());
    let heard = Rc::new(RefCell::new(Vec::new()));
    let (listening, slot) = (heard.clone(), Rc::new(RefCell::new(None)));
    let (kept, words) = (slot.clone(), title.to_string());
    let (_, cx) = cx.add_window_view(move |window, cx| {
        let sheet =
            cx.new(|cx| NewLaneSheet::new((projects, Some(0)), &snapshot, source, window, cx));
        sheet.update(cx, |sheet, cx| sheet.set_title(&words, window, cx));
        let events = listening.clone();
        cx.subscribe(&sheet, move |_, _, event: &NewLaneEvent, _| {
            events.borrow_mut().push(event.clone())
        })
        .detach();
        *kept.borrow_mut() = Some(sheet.clone());
        gpui_component::Root::new(sheet, window, cx)
    });
    let sheet = slot.borrow_mut().take().expect("the sheet was built");
    (rig, sheet, heard, cx)
}

#[gpui::test]
fn the_lane_started_opens_in_the_project_it_was_asked_in_whatever_the_picker_says_later(
    cx: &mut TestAppContext,
) {
    let (_rig, sheet, heard, cx) = sheet_over(cx, "add a parser");
    sheet.update(cx, |sheet, cx| sheet.submit(cx));
    sheet.update(cx, |sheet, _| sheet.project = 1);
    wait_until(cx, "the lane started", |_| !heard.borrow().is_empty());
    let events = heard.borrow();
    assert_eq!(
        events.as_slice(),
        [NewLaneEvent::Started {
            folder: "/work/repo".into(),
            task: NEW_TASK.into()
        }]
    );
}

#[gpui::test]
fn the_project_cannot_be_changed_while_the_lane_is_starting(cx: &mut TestAppContext) {
    let (_rig, sheet, _heard, cx) = sheet_over(cx, "add a parser");
    sheet.update(cx, |sheet, cx| sheet.submit(cx));
    sheet.update(cx, |sheet, cx| sheet.choose_project(1, cx));
    assert_eq!(sheet.read_with(cx, |sheet, _| sheet.project), 0);
}

#[gpui::test]
fn new_lane_sheet_starts_lane_and_opens_its_tab(cx: &mut TestAppContext) {
    let (rig, desk, cx) = open_sheet(STARTED, cx);
    type_into_field(cx, "add a parser");
    rig.say_snapshot(&with_started_lane());
    click(cx, "new-lane-start");
    wait_until(cx, "the lane's page", |cx| {
        pages(&desk, cx) != [Page::Lanes]
    });
    assert_eq!(pages(&desk, cx), [Page::Lane(started_key())]);
    wait_until(cx, "the sheet goes away", |cx| {
        cx.debug_bounds("new-lane-sheet").is_none()
    });
}

#[gpui::test]
fn new_lane_empty_title_cannot_start(cx: &mut TestAppContext) {
    let (rig, desk, cx) = open_sheet(STARTED, cx);
    click(cx, "new-lane-start");
    cx.run_until_parked();
    assert!(rig.acts().is_empty(), "nothing was sent");
    assert!(bounds(cx, "new-lane-sheet").is_some(), "the sheet stays");
    assert_eq!(pages(&desk, cx), [Page::Lanes]);
    type_into_field(cx, "   ");
    click(cx, "new-lane-start");
    cx.run_until_parked();
    assert!(rig.acts().is_empty(), "spaces are no title");
}

#[gpui::test]
fn new_lane_refusal_is_said_in_the_sheet_and_the_sheet_stays(cx: &mut TestAppContext) {
    let (_rig, desk, cx) = open_sheet(REFUSED, cx);
    type_into_field(cx, "add a parser");
    click(cx, "new-lane-start");
    wait_until(cx, "the engine's words", |cx| {
        cx.debug_bounds("new-lane-note").is_some()
    });
    assert!(bounds(cx, "new-lane-sheet").is_some());
    assert_eq!(pages(&desk, cx), [Page::Lanes], "no lane page opened");
}

/// The fixture with two more lanes in `/work/repo`: the one the sheet's start made, and another one that
/// appeared meanwhile (the master's, say), listed first.
fn with_another_lane_too() -> String {
    let mut snapshot: serde_json::Value =
        serde_json::from_str(&with_started_lane()).expect("the snapshot");
    let items = snapshot["items"].as_array_mut().expect("the items");
    let mut other = items.last().expect("the started lane").clone();
    other["key"] = serde_json::json!(["flow", "/work/repo", "by-the-master"]);
    other["id"] = serde_json::json!(r#"["flow","/work/repo","by-the-master"]"#);
    let at = items.len() - 1;
    items.insert(at, other);
    snapshot.to_string()
}

#[gpui::test]
fn new_lane_opens_the_lane_this_submission_created_and_not_another_that_appeared(
    cx: &mut TestAppContext,
) {
    let (rig, desk, cx) = open_sheet(STARTED, cx);
    type_into_field(cx, "add a parser");
    rig.say_snapshot(&with_another_lane_too());
    click(cx, "new-lane-start");
    wait_until(cx, "the lane's page", |cx| {
        pages(&desk, cx) != [Page::Lanes]
    });
    assert_eq!(pages(&desk, cx), [Page::Lane(started_key())]);
}

#[gpui::test]
fn new_lane_without_an_id_from_the_engine_opens_no_tab(cx: &mut TestAppContext) {
    let (rig, desk, cx) = open_sheet(STARTED_WITHOUT_ID, cx);
    type_into_field(cx, "add a parser");
    rig.say_snapshot(&with_started_lane());
    click(cx, "new-lane-start");
    wait_until(cx, "the sheet goes away", |cx| {
        cx.debug_bounds("new-lane-sheet").is_none()
    });
    cx.run_until_parked();
    assert_eq!(pages(&desk, cx), [Page::Lanes], "no lane was guessed");
}

/// Opens the menu of the chip `chip` and takes its choice number `choice` (the first is 1), by the keys.
fn choose_in_chip(cx: &mut VisualTestContext, chip: &str, choice: usize) {
    click(cx, chip);
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    for _ in 0..choice {
        cx.dispatch_action(SelectDown);
    }
    cx.dispatch_action(Confirm { secondary: false });
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

#[gpui::test]
fn the_preset_and_pace_menus_choose_what_start_sends(cx: &mut TestAppContext) {
    let (rig, _desk, cx) = open_sheet_over(&accepting_preset_and_pace(), STARTED, cx);
    type_into_field(cx, "add a parser");
    choose_in_chip(cx, "new-lane-preset", 2);
    choose_in_chip(cx, "new-lane-pace", 4);
    click(cx, "new-lane-start");
    wait_until(cx, "the act", |_| !rig.acts().is_empty());
    let sent = acts_text(&rig);
    assert!(sent.contains("--preset solo"), "{sent}");
    assert!(sent.contains("--level fast"), "{sent}");
}

#[gpui::test]
fn the_preset_menu_lists_the_engine_s_presets_even_when_no_lane_runs_as_one(
    cx: &mut TestAppContext,
) {
    let (rig, _desk, cx) = open_sheet_over(&accepting_preset_and_pace(), STARTED, cx);
    type_into_field(cx, "add a parser");
    // Default, then solo, standard, verified and lean, as the engine's snapshot lists them: no legacy name.
    choose_in_chip(cx, "new-lane-preset", 5);
    click(cx, "new-lane-start");
    wait_until(cx, "the act", |_| !rig.acts().is_empty());
    assert!(
        acts_text(&rig).contains("--preset lean"),
        "{}",
        acts_text(&rig)
    );
}

#[gpui::test]
fn the_sheet_stands_near_the_top_of_the_window_and_not_in_its_middle(cx: &mut TestAppContext) {
    let (_rig, _desk, cx) = open_sheet(STARTED, cx);
    let sheet = bounds(cx, "new-lane-sheet").expect("the sheet");
    let window = cx.update(|window, _| window.viewport_size());
    let off_by = (sheet.origin.y - crate::theme::DIALOG_SHEET_TOP).abs();
    assert!(
        off_by < gpui::px(1.),
        "the sheet's top is {:?}",
        sheet.origin.y
    );
    assert!(sheet.center().y < window.height / 2.);
}

#[gpui::test]
fn the_sheet_is_centred_across_the_window_and_fits_its_content_over_lanes(cx: &mut TestAppContext) {
    let (_rig, desk, cx) = open_sheet(STARTED, cx);
    let sheet = bounds(cx, "new-lane-sheet").expect("the sheet");
    let window = cx.update(|window, _| window.viewport_size());
    assert!(
        (sheet.center().x - window.width / 2.).abs() < gpui::px(1.),
        "centred on the window, not on the page: {:?}",
        sheet.center().x
    );
    let footer = bounds(cx, "dialog-footer").expect("the footer");
    assert_eq!(
        sheet.bottom() - footer.bottom(),
        crate::theme::CONTENT_PADDING + crate::theme::PANEL_RING_WIDTH
    );
    assert_eq!(
        pages(&desk, cx),
        [Page::Lanes],
        "the Lanes page is what the sheet opens over"
    );
}

#[gpui::test]
fn new_lane_with_no_project_offers_add_a_project_in_place_and_becomes_the_composer_once_one_is_added(
    cx: &mut TestAppContext,
) {
    let mut empty: serde_json::Value = serde_json::from_str(SNAPSHOT).expect("the fixture");
    empty["projects"] = serde_json::json!([]);
    let (rig, desk, cx) = open_answering(cx, &empty.to_string(), STARTED, STARTED);
    cx.update(|window, cx| desk.update(cx, |desk, cx| desk.open_new_lane_default(window, cx)));
    wait_until(cx, "the sentence", |cx| {
        cx.debug_bounds("new-lane-no-project").is_some()
    });
    assert!(
        bounds(cx, "new-lane-sheet").is_none(),
        "no sheet to fill in yet"
    );
    // The engine will list the project once it is added.
    rig.say_snapshot(SNAPSHOT);
    click(cx, "new-lane-add-project");
    assert!(
        cx.did_prompt_for_paths(),
        "the folder picker opens in place"
    );
    assert!(
        bounds(cx, "settings-modal").is_none(),
        "the person is not sent to Settings"
    );
    cx.simulate_path_prompt_response(|_| Some(vec![std::path::PathBuf::from("/work/repo")]));
    wait_until(cx, "the composer", |cx| {
        cx.debug_bounds("new-lane-sheet").is_some()
    });
    assert!(bounds(cx, "new-lane-no-project").is_none());
}

#[gpui::test]
fn new_lane_from_the_lanes_page_asks_for_the_project_when_there_are_several(
    cx: &mut TestAppContext,
) {
    let (_rig, desk, cx) = open_answering(cx, SNAPSHOT, STARTED, STARTED);
    cx.update(|window, cx| desk.update(cx, |desk, cx| desk.open_new_lane_default(window, cx)));
    wait_until(cx, "the sheet", |cx| {
        cx.debug_bounds("new-lane-sheet").is_some()
    });
    assert!(
        bounds(cx, "new-lane-asking-project").is_some(),
        "the project is requested, not guessed"
    );
    // Opened from a project's own row it is already chosen.
    drop(desk);
}

fn preset(name: &str, label: &str) -> sakura_core::engine::presets::PresetChoice {
    sakura_core::engine::presets::PresetChoice::named(name, Some(label))
}

#[test]
fn the_preset_menu_names_each_preset_once_the_one_in_force_included() {
    let listed = [
        preset("haiku", "Haiku"),
        preset("solo", "Solo"),
        preset("lean", "Lean"),
    ];
    let menu = preset_menu(Some("Haiku"), &listed, "Default");
    let words: Vec<&str> = menu.iter().map(|(word, _)| word.as_str()).collect();
    assert_eq!(
        words,
        ["Haiku", "Solo", "Lean"],
        "Haiku is not listed twice"
    );
    assert_eq!(
        menu[0].1, None,
        "the preset in force takes the person's own choice off"
    );
    assert_eq!(menu[1].1.as_deref(), Some("solo"));
}

#[test]
fn a_preset_in_force_that_is_not_listed_still_heads_the_menu_and_none_says_the_engine_picks() {
    let listed = [preset("solo", "Solo")];
    let menu = preset_menu(Some("Careful"), &listed, "Careful");
    let words: Vec<&str> = menu.iter().map(|(word, _)| word.as_str()).collect();
    assert_eq!(words, ["Careful", "Solo"]);
    let without = preset_menu(None, &listed, "The engine picks");
    assert_eq!(without[0].0, "The engine picks");
}

/// The fixture whose lane-starting action takes a pace and whose engine runs at the pace `engine`.
fn at_engine_pace(engine: &str) -> String {
    let mut document: serde_json::Value =
        serde_json::from_str(&accepting_preset_and_pace()).expect("a fixture");
    document["engine"]["pace"] = engine.into();
    document.to_string()
}

#[gpui::test]
fn a_lane_starts_at_normal_whatever_the_engine_s_pace_so_the_sheet_says_normal_and_sends_nothing(
    cx: &mut TestAppContext,
) {
    let (rig, _desk, cx) = open_sheet_over(&at_engine_pace("fast"), STARTED, cx);
    type_into_field(cx, "add a parser");
    assert_eq!(
        pace_in_force(None),
        "normal",
        "what the lane starts with when nothing is chosen"
    );
    click(cx, "new-lane-start");
    wait_until(cx, "the act", |_| !rig.acts().is_empty());
    assert!(!acts_text(&rig).contains("--level"), "{}", acts_text(&rig));
}

#[gpui::test]
fn the_pace_the_engine_runs_at_is_a_choice_like_any_other_for_a_new_lane(cx: &mut TestAppContext) {
    let (rig, _desk, cx) = open_sheet_over(&at_engine_pace("fast"), STARTED, cx);
    type_into_field(cx, "add a parser");
    choose_in_chip(cx, "new-lane-pace", 4);
    click(cx, "new-lane-start");
    wait_until(cx, "the act", |_| !rig.acts().is_empty());
    assert!(
        acts_text(&rig).contains("--level fast "),
        "{}",
        acts_text(&rig)
    );
}

#[gpui::test]
fn empty_new_lane_fields_paint_filled_controls_at_minimum_width(cx: &mut TestAppContext) {
    let (_rig, _desk, cx) = open_sheet(STARTED, cx);
    cx.simulate_resize(gpui::size(
        crate::theme::WINDOW_MIN_WIDTH,
        crate::theme::WINDOW_MIN_HEIGHT,
    ));
    for id in ["new-lane-title", "new-lane-description"] {
        let field = bounds(cx, id).unwrap();
        cx.update(|window, cx| {
            let palette = crate::theme::Theme::palette(cx);
            window.draw(cx).clear(cx);
            assert!(
                field.size.height >= crate::theme::CONTROL_HEIGHT,
                "{id}: {field:?}"
            );
            assert!(
                window.painted_quads().iter().any(|quad| {
                    quad.bounds == field.scale(window.scale_factor())
                        && quad.background == palette.fill.into()
                }),
                "{id} needs a filled control"
            );
        });
    }
}

#[gpui::test]
fn reaudit3_new_lane_offers_project_in_a_full_width_row(cx: &mut TestAppContext) {
    let (_, desk, window) = open_answering(cx, SNAPSHOT, STARTED, STARTED);
    window.update(|window, cx| desk.update(cx, |desk, cx| desk.open_new_lane_default(window, cx)));
    wait_until(window, "New lane", |cx| {
        bounds(cx, "new-lane-sheet").is_some()
    });
    let row = bounds(window, "new-lane-project-row").expect("a labelled Project row");
    let footer = bounds(window, "dialog-footer").unwrap();
    assert_eq!(row.left(), footer.left());
    assert_eq!(row.right(), footer.right());
}

#[gpui::test]
fn polish_zd_new_lane_track_sits_with_the_other_choice_chips(cx: &mut TestAppContext) {
    let fixture = include_str!("../../../sakura-core/tests/fixtures/engine-real-tracks.json");
    let (_rig, desk, cx) = super::engine_desk_tests::open_unread(cx, fixture);
    let snapshot = sakura_core::engine::snapshot::Snapshot::parse(fixture.as_bytes()).unwrap();
    let track = snapshot
        .tracks
        .rows()
        .iter()
        .find(|track| track.status == sakura_core::engine::tracks::TrackStatus::Open)
        .unwrap();
    desk.update(cx, |desk, cx| {
        desk.navigate(sakura_core::tabs::Page::Track(track.id.clone()), false, cx)
    });
    wait_until(cx, "New lane in Track", |cx| {
        bounds(cx, "track-new-lane").is_some()
    });
    click(cx, "track-new-lane");
    wait_until(cx, "Track chip", |cx| {
        bounds(cx, "new-lane-track").is_some()
    });
    let track = bounds(cx, "new-lane-track").unwrap();
    let start = bounds(cx, "new-lane-start").unwrap();
    assert_eq!(track.center().y, start.center().y);
    assert!(track.right() < start.left());
}
