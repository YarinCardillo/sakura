//! The command palette in the engine's window: Cmd+K lists the pages, the lanes, the projects and the
//! actions of the page on screen; Return goes there or runs it, Cmd+Return opens a page in a tab of its own.

use super::engine_desk::Desk;
use super::engine_rig_tests::{wait_until, Rig, SNAPSHOT};
use super::lane_tests::open_answering;
use super::palette::Ask;
use gpui::{Entity, TestAppContext, VisualTestContext};
use sakura_core::tabs::Page;
use std::time::Duration;

const ANSWER: &str = r#"{"result": "done", "exit_code": 0, "output": ""}"#;
const ASKS: &str = r#"{"result": "confirm", "token": "17.ab", "expires": "2026-11-16T08:02:00Z", "summary": "Approve on gated: run in /work/repo: sakura flow approve gated spec"}"#;
const UPLOAD: &str = "upload-retries";

/// The fixture with its first lane called `upload-retries`.
fn with_upload_lane() -> String {
    let mut document: serde_json::Value = serde_json::from_str(SNAPSHOT).expect("the fixture");
    let item = &mut document["items"][0];
    item["key"] = serde_json::json!(["flow", "/work/repo", UPLOAD]);
    item["id"] = serde_json::json!(r#"["flow","/work/repo","upload-retries"]"#);
    document.to_string()
}

fn upload_key() -> String {
    serde_json::json!(["flow", "/work/repo", UPLOAD]).to_string()
}

/// The desk over a fake engine that says `snapshot`, with the app's keys bound.
fn open<'a>(
    snapshot: &str,
    cx: &'a mut TestAppContext,
) -> (Rig, Entity<Desk>, &'a mut VisualTestContext) {
    let (rig, desk, cx) = open_answering(cx, snapshot, ANSWER, ANSWER);
    cx.update(|_, cx| crate::menus::install(cx));
    (rig, desk, cx)
}

fn press(cx: &mut VisualTestContext, keys: &str) {
    cx.simulate_keystrokes(keys);
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

fn page(desk: &Entity<Desk>, cx: &mut VisualTestContext) -> Page {
    desk.read_with(cx, |desk, cx| {
        desk.bar.read(cx).tabs().active().page().clone()
    })
}

fn tab_count(desk: &Entity<Desk>, cx: &mut VisualTestContext) -> usize {
    desk.read_with(cx, |desk, cx| desk.bar.read(cx).tabs().tabs().len())
}

fn is_open(desk: &Entity<Desk>, cx: &mut VisualTestContext) -> bool {
    desk.read_with(cx, |desk, cx| desk.palette.read(cx).is_open())
}

fn ask(desk: &Entity<Desk>, ask: &Ask, cx: &mut VisualTestContext) {
    cx.update(|window, cx| desk.update(cx, |desk, cx| desk.palette_asked(ask, window, cx)));
    cx.run_until_parked();
}

#[gpui::test]
fn polish_zd_cmd_k_accepts_input_on_open_and_after_clicking(cx: &mut TestAppContext) {
    let (_rig, desk, cx) = open(&with_upload_lane(), cx);
    assert!(!is_open(&desk, cx));
    press(cx, "cmd-k");
    assert!(is_open(&desk, cx));
    cx.simulate_input("u");
    let typed = desk.read_with(cx, |desk, cx| desk.palette.read(cx).query(cx));
    assert_eq!(typed.as_ref(), "u");
    focus_search(cx);
    press(cx, "end");
    cx.simulate_input("pl");
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let typed = desk.read_with(cx, |desk, cx| desk.palette.read(cx).query(cx));
    assert_eq!(typed.as_ref(), "upl", "typing goes to the activated field");
    press(cx, "enter");
    assert!(!is_open(&desk, cx));
    assert_eq!(page(&desk, cx), Page::Lane(upload_key()));
}

fn focus_search(cx: &mut VisualTestContext) {
    let panel = cx.debug_bounds("palette-panel").unwrap();
    let point = gpui::point(panel.center().x, panel.top() + gpui::px(15.));
    cx.simulate_click(point, gpui::Modifiers::none());
}

#[gpui::test]
fn return_on_a_page_goes_there_and_cmd_return_opens_it_in_a_tab_of_its_own(
    cx: &mut TestAppContext,
) {
    let (_rig, desk, cx) = open(SNAPSHOT, cx);
    press(cx, "cmd-k");
    focus_search(cx);
    cx.simulate_input("engi");
    press(cx, "enter");
    assert_eq!((page(&desk, cx), tab_count(&desk, cx)), (Page::Engine, 1));
    press(cx, "cmd-k");
    focus_search(cx);
    cx.simulate_input("mast");
    press(cx, "cmd-enter");
    assert_eq!((page(&desk, cx), tab_count(&desk, cx)), (Page::Master, 2));
}

#[gpui::test]
fn a_project_chosen_shows_the_lanes_of_that_project(cx: &mut TestAppContext) {
    let (_rig, desk, cx) = open(SNAPSHOT, cx);
    ask(
        &desk,
        &Ask::OpenProject {
            folder: "/work/other".into(),
            is_new_tab: false,
        },
        cx,
    );
    assert_eq!(page(&desk, cx), Page::Lanes);
    let kept = desk.read_with(cx, |desk, cx| {
        desk.active_lanes(cx)
            .unwrap()
            .read(cx)
            .project()
            .map(str::to_string)
    });
    assert_eq!(kept.as_deref(), Some("/work/other"));
}

const GATED: &str = r#"["wait","/work/repo","gated","approval","spec"]"#;

/// Selects the ask `wait` in the Inbox, as a click on its row does.
fn select_ask(desk: &Entity<Desk>, wait: &str, cx: &mut VisualTestContext) {
    let inbox = desk.read_with(cx, |desk, _| desk.inbox.clone());
    inbox.update(cx, |inbox, cx| inbox.select(wait.to_string(), cx));
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

fn offered(desk: &Entity<Desk>, cx: &mut VisualTestContext) -> Vec<String> {
    desk.read_with(cx, |desk, cx| desk.situation(cx).selected_ask)
        .map(|ask| ask.verbs.into_iter().map(|verb| verb.verb).collect())
        .unwrap_or_default()
}

#[gpui::test]
fn the_selected_ask_s_verbs_are_listed_only_while_the_inbox_is_shown(cx: &mut TestAppContext) {
    let (_rig, desk, cx) = open(SNAPSHOT, cx);
    select_ask(&desk, GATED, cx);
    assert_eq!(offered(&desk, cx), ["approve"]);
    desk.update(cx, |desk, cx| desk.navigate(Page::Lanes, false, cx));
    cx.run_until_parked();
    assert!(offered(&desk, cx).is_empty(), "another page shows no ask");
}

#[gpui::test]
fn a_verb_chosen_in_the_palette_is_held_under_undo_for_six_seconds_and_then_sent(
    cx: &mut TestAppContext,
) {
    let (rig, desk, cx) = open_answering(cx, SNAPSHOT, ASKS, ANSWER);
    cx.update(|_, cx| crate::menus::install(cx));
    select_ask(&desk, GATED, cx);
    ask(
        &desk,
        &Ask::Answer {
            wait: GATED.into(),
            verb: "approve".into(),
        },
        cx,
    );
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("toast-undo").is_some(),
        "the answer is held under the Undo toast"
    );
    wait_until(cx, "the engine question in the toast", |cx| {
        cx.debug_bounds("toast-detail").is_some()
    });
    cx.executor().advance_clock(Duration::from_secs(5));
    cx.run_until_parked();
    let confirms = || {
        rig.acts()
            .iter()
            .filter(|act| act.contains("--confirm"))
            .count()
    };
    assert_eq!(confirms(), 0, "held: {:?}", rig.acts());
    cx.executor().advance_clock(Duration::from_secs(2));
    wait_until(cx, "the confirmed call", |_| confirms() == 1);
    let sent = &rig.acts()[0];
    assert!(
        sent.starts_with(&format!("cockpit act a {GATED}")),
        "{sent}"
    );
}

#[gpui::test]
fn choosing_a_palette_command_never_answers_the_selected_ask(cx: &mut TestAppContext) {
    let (rig, desk, cx) = open_answering(cx, SNAPSHOT, ASKS, ANSWER);
    select_ask(&desk, GATED, cx);
    press(cx, "cmd-k");
    focus_search(cx);
    press(cx, "enter");
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(!is_open(&desk, cx));
    assert!(cx.debug_bounds("toast-undo").is_none(), "nothing was held");
    cx.executor().advance_clock(Duration::from_secs(7));
    cx.run_until_parked();
    assert!(rig.acts().is_empty(), "nothing was sent: {:?}", rig.acts());
    assert_ne!(page(&desk, cx), Page::Inbox, "it went to a page");
}

#[gpui::test]
fn a_verb_of_an_ask_that_is_no_longer_selected_sends_nothing(cx: &mut TestAppContext) {
    let (rig, desk, cx) = open(SNAPSHOT, cx);
    select_ask(
        &desk,
        r#"["wait","/work/repo","demo","owner_row","R1"]"#,
        cx,
    );
    ask(
        &desk,
        &Ask::Answer {
            wait: GATED.into(),
            verb: "approve".into(),
        },
        cx,
    );
    cx.executor().advance_clock(Duration::from_secs(7));
    cx.run_until_parked();
    assert!(rig.acts().is_empty(), "{:?}", rig.acts());
}

#[gpui::test]
fn stop_is_offered_on_a_lane_page_and_goes_through_the_runner(cx: &mut TestAppContext) {
    let (rig, desk, cx) = open(SNAPSHOT, cx);
    let gated = super::engine_rig_tests::lane_key("gated");
    desk.update(cx, |desk, cx| {
        desk.navigate(Page::Lane(gated.clone()), false, cx)
    });
    cx.run_until_parked();
    let stoppable = desk.read_with(cx, |desk, cx| desk.situation(cx).stoppable);
    assert_eq!(stoppable.as_deref(), Some(gated.as_str()));
    ask(&desk, &Ask::StopLane(gated), cx);
    wait_until(cx, "the act", |_| !rig.acts().is_empty());
    assert!(
        rig.acts()[0].starts_with("cockpit act s "),
        "{}",
        rig.acts()[0]
    );
}

#[gpui::test]
fn cmd_p_outside_a_lane_page_does_nothing(cx: &mut TestAppContext) {
    let (_rig, desk, cx) = open(SNAPSHOT, cx);
    let none = desk.read_with(cx, |desk, cx| desk.quick_open_lane(cx));
    assert_eq!(none, None, "the Inbox has no files");
    press(cx, "cmd-p");
    assert!(!is_open(&desk, cx), "it is not the palette");
    assert_eq!(page(&desk, cx), Page::Inbox);
    let gated = super::engine_rig_tests::lane_key("gated");
    desk.update(cx, |desk, cx| {
        desk.navigate(Page::Lane(gated.clone()), false, cx)
    });
    let lane = desk.read_with(cx, |desk, cx| desk.quick_open_lane(cx));
    assert_eq!(
        lane,
        Some(gated),
        "on a lane's page it is that lane's files"
    );
}

#[gpui::test]
fn escape_closes_the_palette_at_once_even_with_words_typed(cx: &mut TestAppContext) {
    let (_rig, desk, cx) = open(SNAPSHOT, cx);
    press(cx, "cmd-k");
    focus_search(cx);
    cx.simulate_input("dig");
    assert!(is_open(&desk, cx));
    press(cx, "escape");
    assert!(!is_open(&desk, cx), "the first Escape leaves the palette");
}

#[gpui::test]
fn a_page_change_closes_the_palette(cx: &mut TestAppContext) {
    let (_rig, desk, cx) = open(SNAPSHOT, cx);
    press(cx, "cmd-k");
    assert!(is_open(&desk, cx));
    desk.update(cx, |desk, cx| desk.navigate(Page::Master, false, cx));
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(!is_open(&desk, cx));
}

#[gpui::test]
fn opening_settings_closes_the_palette_so_one_overlay_is_open_at_a_time(cx: &mut TestAppContext) {
    let (_rig, desk, cx) = open(SNAPSHOT, cx);
    press(cx, "cmd-k");
    assert!(is_open(&desk, cx));
    cx.update(|window, cx| desk.update(cx, |desk, cx| desk.open_settings(window, cx)));
    cx.run_until_parked();
    assert!(!is_open(&desk, cx));
}

/// The top of the palette's panel, where its search field stands.
fn panel_top(cx: &mut VisualTestContext) -> gpui::Pixels {
    cx.debug_bounds("palette-panel").expect("the panel").top()
}

#[gpui::test]
fn the_search_field_stays_where_it_is_while_the_matches_come_and_go(cx: &mut TestAppContext) {
    let (_rig, _desk, cx) = open(SNAPSHOT, cx);
    press(cx, "cmd-k");
    let at_rest = panel_top(cx);
    let full = cx
        .debug_bounds("palette-panel")
        .expect("the panel")
        .size
        .height;
    focus_search(cx);
    cx.simulate_input("zzzz");
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert_eq!(panel_top(cx), at_rest, "no match: the field did not move");
    let empty = cx
        .debug_bounds("palette-panel")
        .expect("the panel")
        .size
        .height;
    assert!(empty < full, "the list did shrink");
    cx.simulate_keystrokes("backspace backspace backspace backspace");
    cx.simulate_input("lan");
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert_eq!(panel_top(cx), at_rest, "a few matches: nor did it move");
}

#[gpui::test]
fn the_palette_opened_and_closed_before_any_frame_is_closed(cx: &mut TestAppContext) {
    let (_rig, desk, cx) = open(SNAPSHOT, cx);
    cx.simulate_keystrokes("cmd-k escape");
    assert!(!is_open(&desk, cx), "Escape in the frame it opened in");
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(!is_open(&desk, cx));
}

#[gpui::test]
fn settings_opened_and_closed_before_any_frame_is_closed(cx: &mut TestAppContext) {
    let (_rig, desk, cx) = open(SNAPSHOT, cx);
    cx.update(|window, cx| desk.update(cx, |desk, cx| desk.open_settings(window, cx)));
    cx.simulate_keystrokes("escape");
    cx.update(|window, cx| {
        use gpui_component::WindowExt as _;
        assert!(
            !window.has_active_dialog(cx),
            "Escape in the frame it opened in"
        );
    });
}

#[test]
fn a_lane_has_the_same_mark_in_the_palette_as_in_the_sidebar_whatever_its_state() {
    let snapshot = sakura_core::engine::snapshot::Snapshot::parse(SNAPSHOT.as_bytes());
    let snapshot = snapshot.expect("the fixture");
    let listed = super::engine_palette::palette_lanes(&snapshot);
    assert!(
        snapshot
            .items
            .iter()
            .any(|item| item.agent_state == "blocked"),
        "the fixture has a stuck lane"
    );
    for lane in &listed {
        let item = snapshot
            .items
            .iter()
            .find(|item| item.lane_key() == lane.key);
        let item = item.expect("the lane is in the snapshot");
        assert_eq!(lane.glyph, super::lanes::glyph_of(item), "{}", lane.name);
    }
    assert!(!listed.is_empty());
}

#[gpui::test]
fn polish_zd_tracks_can_be_found_and_opened_from_the_palette(cx: &mut TestAppContext) {
    let fixture = include_str!("../../../sakura-core/tests/fixtures/engine-real-tracks.json");
    let snapshot = sakura_core::engine::snapshot::Snapshot::parse(fixture.as_bytes()).unwrap();
    let track = &snapshot.tracks.rows()[0];
    let (_rig, desk, cx) = open(fixture, cx);
    press(cx, "cmd-k");
    focus_search(cx);
    cx.simulate_input(&track.name);
    press(cx, "enter");
    assert_eq!(page(&desk, cx), Page::Track(track.id.clone()));
}
