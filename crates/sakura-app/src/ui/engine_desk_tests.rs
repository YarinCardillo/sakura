//! The engine's window as a person meets it: the Inbox first, the lanes that open from the sidebar, the tabs
//! kept for the next launch.

use super::engine_desk::{Desk, DeskData};
use super::engine_rig_tests::{self, wait_until, Rig, SNAPSHOT};
use crate::prefs::Preferences;
use gpui::{AppContext as _, Entity, Modifiers, TestAppContext, VisualTestContext};
use sakura_core::tabs::{Page, Tabs};
use std::cell::RefCell;
use std::rc::Rc;

#[cfg(feature = "snapshot")]
const NOTHING: &str = r#"{"api": 1, "waits": [], "items": [], "events": [], "problems": []}"#;
const APPROVAL: &str = r#"["wait","/work/repo","gated","approval","spec"]"#;
const CONFIRM: &str = r#"{"result": "confirm", "token": "17.ab", "expires": "2026-11-16T08:02:00Z", "summary": "Approve on gated"}"#;
const ANSWER: &str = r#"{"result": "done", "exit_code": 0, "output": "ok\n"}"#;

#[gpui::test]
fn the_lane_and_scheduled_split_dividers_reach_the_panel_ring(cx: &mut TestAppContext) {
    let fixture = super::engine_fixture_tests::edited(|snapshot| {
        let scheduled: serde_json::Value = serde_json::from_str(include_str!(
            "../../../sakura-core/tests/fixtures/scheduled.json"
        ))
        .unwrap();
        snapshot["scheduled"] = scheduled["scheduled"].clone();
    });
    let (_rig, desk, cx) = open(cx, &fixture);
    cx.simulate_resize(gpui::size(gpui::px(1280.), gpui::px(800.)));
    for (page, column, is_leading) in [
        (
            Page::Lane(engine_rig_tests::lane_key("demo")),
            "lane-side",
            false,
        ),
        (Page::Scheduled, "scheduled-list", true),
    ] {
        desk.update(cx, |desk, cx| desk.navigate(page, false, cx));
        wait_until(cx, column, |cx| cx.debug_bounds(column).is_some());
        let column = cx.debug_bounds(column).unwrap();
        let panel = cx.debug_bounds("page-panel").unwrap();
        let ring = crate::theme::PANEL_RING_WIDTH;
        assert_eq!(column.bottom(), panel.bottom() - ring);
        let x = if is_leading {
            column.right()
        } else {
            column.left() - ring
        };
        let expected = gpui::Bounds::new(
            gpui::point(x, column.top()),
            gpui::size(ring, column.size.height),
        );
        cx.update(|window, cx| {
            let border = crate::theme::Theme::palette(cx).border;
            assert!(window.painted_quads().iter().any(|quad| {
                quad.bounds == expected.scale(window.scale_factor())
                    && quad.background == border.into()
            }));
        });
    }
}

/// The desk over a fake engine whose snapshot is `snapshot`, the first one read and drawn.
pub(super) fn open<'a>(
    cx: &'a mut TestAppContext,
    snapshot: &str,
) -> (Rig, Entity<Desk>, &'a mut VisualTestContext) {
    let (rig, desk, cx) = open_unread(cx, snapshot);
    wait_until(cx, "the first snapshot", |cx| {
        cx.debug_bounds("inbox").is_some() && cx.debug_bounds("group-0").is_some()
    });
    (rig, desk, cx)
}

/// The desk over a fake engine whose snapshot is `snapshot`, nothing waited for: for an engine that says
/// nothing that can be read.
pub(super) fn open_unread<'a>(
    cx: &'a mut TestAppContext,
    snapshot: &str,
) -> (Rig, Entity<Desk>, &'a mut VisualTestContext) {
    let rig = engine_rig_tests::open(cx, snapshot, ANSWER, ANSWER);
    open_rig(cx, rig)
}

pub(super) fn open_rig(
    cx: &mut TestAppContext,
    rig: Rig,
) -> (Rig, Entity<Desk>, &mut VisualTestContext) {
    cx.update(|cx| Preferences::default().install(None, cx));
    let (facts, source, sessions) = (rig.facts.clone(), rig.source.clone(), rig.sessions.clone());
    let slot = Rc::new(RefCell::new(None));
    let kept = slot.clone();
    let (_, cx) = cx.add_window_view(move |window, cx| {
        let data = DeskData {
            facts,
            source,
            sessions,
        };
        let desk = cx.new(|cx| Desk::new(data, &Preferences::default(), window, cx));
        *kept.borrow_mut() = Some(desk.clone());
        Desk::root(desk, window, cx)
    });
    let desk = slot.borrow_mut().take().expect("the desk was built");
    cx.update(|window, _| window.activate_window());
    (rig, desk, cx)
}

fn pages(desk: &Entity<Desk>, cx: &mut VisualTestContext) -> Vec<Page> {
    desk.read_with(cx, |desk, cx| {
        let tabs = desk.bar.read(cx).tabs();
        tabs.tabs().iter().map(|tab| tab.page().clone()).collect()
    })
}

#[gpui::test]
fn the_window_opens_on_the_inbox_and_a_lane_row_shows_the_lane_in_the_same_tab(
    cx: &mut TestAppContext,
) {
    let (_rig, desk, cx) = open(cx, SNAPSHOT);
    assert_eq!(pages(&desk, cx), [Page::Inbox]);
    let at = cx
        .debug_bounds("row-demo")
        .expect("the lane's row in the sidebar")
        .center();
    cx.simulate_click(at, Modifiers::none());
    let lane = Page::Lane(engine_rig_tests::lane_key("demo"));
    wait_until(cx, "the lane's page", |cx| {
        pages(&desk, cx) == [lane.clone()]
    });
}

#[gpui::test]
fn a_cmd_click_on_a_sidebar_lane_row_opens_the_lane_in_a_new_tab(cx: &mut TestAppContext) {
    let (_rig, desk, cx) = open(cx, SNAPSHOT);
    let at = cx
        .debug_bounds("row-demo")
        .expect("the sidebar's row")
        .center();
    cx.simulate_click(at, Modifiers::command());
    let lane = Page::Lane(engine_rig_tests::lane_key("demo"));
    wait_until(cx, "the new tab", |cx| {
        pages(&desk, cx) == [Page::Inbox, lane.clone()]
    });
}

#[gpui::test]
fn the_tabs_are_kept_for_the_next_launch(cx: &mut TestAppContext) {
    let (_rig, desk, cx) = open(cx, SNAPSHOT);
    desk.update(cx, |desk, cx| {
        desk.navigate(Page::Lane(engine_rig_tests::lane_key("demo")), true, cx)
    });
    let stored = desk.read_with(cx, |desk, cx| desk.bar.read(cx).tabs().to_json());
    assert_eq!(stored["version"], 3);
    let restored = Tabs::restore(Some(&stored));
    let pages: Vec<Page> = restored
        .tabs()
        .iter()
        .map(|tab| tab.page().clone())
        .collect();
    let lane = Page::Lane(engine_rig_tests::lane_key("demo"));
    assert_eq!(pages, [Page::Inbox, lane.clone()]);
    assert_eq!(restored.active().page(), &lane);
}

#[gpui::test]
fn the_scripted_steps_approve_a_wait_and_confirm_it_without_a_click(cx: &mut TestAppContext) {
    let (rig, desk, cx) = super::lane_tests::open_answering(cx, SNAPSHOT, CONFIRM, ANSWER);
    desk.update(cx, |desk, cx| desk.drive_press("a", "gated", cx));
    wait_until(cx, "the first call", |_| rig.acts().len() == 1);
    assert!(
        rig.acts()[0].starts_with("cockpit act a "),
        "{}",
        rig.acts()[0]
    );
    assert!(!rig.acts()[0].contains("--confirm"));
    // The engine requested a confirmation (the fake engine's first answer): the second step gives it.
    wait_until(cx, "the summary", |cx| {
        desk.read_with(cx, |desk, cx| {
            desk.answers.read(cx).summary(APPROVAL).is_some()
        })
    });
    desk.update(cx, |desk, cx| desk.drive_confirm("gated", cx));
    wait_until(cx, "the second call", |_| rig.acts().len() == 2);
    assert!(
        rig.acts()[1].contains("--confirm 17.ab"),
        "{}",
        rig.acts()[1]
    );
}

/// The fixture with a proposal of the master waiting: a wait that offers Accept.
fn with_proposal() -> String {
    let mut snapshot: serde_json::Value = serde_json::from_str(SNAPSHOT).expect("the fixture");
    snapshot["waits"].as_array_mut().expect("the waits").push(serde_json::json!({
        "id": r#"["wait","/work/repo","master","proposal","note-1"]"#,
        "key": ["wait", "/work/repo", "master", "proposal", "note-1"],
        "kind": "proposal", "repo": "/work/repo", "project": "repo", "task": "master",
        "title": "Add a note to the knowledge folder", "detail": "", "since": null,
        "actions": [{"key": "y", "label": "Accept", "needs": [], "optional": [], "confirm": true},
                    {"key": "N", "label": "Reject", "needs": [], "optional": [], "confirm": true}]
    }));
    snapshot.to_string()
}

#[gpui::test]
fn the_scripted_reject_presses_reject_on_the_proposal(cx: &mut TestAppContext) {
    let (rig, desk, cx) = super::lane_tests::open_answering(cx, &with_proposal(), CONFIRM, ANSWER);
    desk.update(cx, |desk, cx| desk.drive_press("N", "note-1", cx));
    wait_until(cx, "the call", |_| rig.acts().len() == 1);
    assert!(
        rig.acts()[0].starts_with("cockpit act N "),
        "{}",
        rig.acts()[0]
    );
}

#[gpui::test]
fn the_scripted_accept_presses_accept_on_the_proposal_named_by_its_title(cx: &mut TestAppContext) {
    let (rig, desk, cx) = super::lane_tests::open_answering(cx, &with_proposal(), CONFIRM, ANSWER);
    desk.update(cx, |desk, cx| desk.drive_press("y", "knowledge folder", cx));
    wait_until(cx, "the call", |_| rig.acts().len() == 1);
    assert!(
        rig.acts()[0].starts_with("cockpit act y "),
        "{}",
        rig.acts()[0]
    );
    let id = r#"["wait","/work/repo","master","proposal","note-1"]"#;
    wait_until(cx, "the summary", |cx| {
        desk.read_with(cx, |desk, cx| desk.answers.read(cx).summary(id).is_some())
    });
    desk.update(cx, |desk, cx| desk.drive_confirm("note-1", cx));
    wait_until(cx, "the confirmed call", |_| rig.acts().len() == 2);
    assert!(
        rig.acts()[1].contains("--confirm 17.ab"),
        "{}",
        rig.acts()[1]
    );
}

#[gpui::test]
fn the_scripted_lane_steps_press_what_the_snapshot_lists_and_confirm_it(cx: &mut TestAppContext) {
    let offered = |key: &str, label: &str| serde_json::json!({"key": key, "label": label, "needs": [], "optional": [], "confirm": true});
    let listed = serde_json::json!([
        offered("R", "Resume"),
        offered("s", "Stop"),
        offered("C", "Clean up")
    ]);
    let snapshot = super::lane_tests::with_demo_actions(listed);
    let (rig, desk, cx) = super::lane_tests::open_answering(cx, &snapshot, CONFIRM, ANSWER);
    for (at, action) in [("resume", "R"), ("stop", "s"), ("clean", "C")] {
        let before = rig.acts().len();
        match action {
            "R" => desk.update(cx, |desk, cx| desk.drive_lane_action("R", "demo", cx)),
            "s" => desk.update(cx, |desk, cx| desk.drive_lane_action("s", "demo", cx)),
            _ => desk.update(cx, |desk, cx| desk.drive_lane_action("C", "demo", cx)),
        }
        wait_until(cx, at, |_| rig.acts().len() == before + 1);
        let sent = &rig.acts()[before];
        assert!(
            sent.starts_with(&format!("cockpit act {action} ")),
            "{sent}"
        );
        assert!(!sent.contains("--confirm"));
        wait_until(cx, "the summary", |cx| {
            desk.read_with(cx, |desk, cx| !desk.lane_keys_named("demo", cx).is_empty())
        });
        desk.update(cx, |desk, cx| desk.drive_confirm("demo", cx));
        wait_until(cx, "the confirmed call", |_| rig.acts().len() == before + 2);
        assert!(rig.acts()[before + 1].contains("--confirm 17.ab"));
    }
    // An action the snapshot does not list is never sent.
    let before = rig.acts().len();
    desk.update(cx, |desk, cx| desk.drive_lane_action("T", "demo", cx));
    cx.run_until_parked();
    assert_eq!(rig.acts().len(), before);
}

#[gpui::test]
fn the_scripted_overlap_approval_presses_approve_on_the_overlap_gate_only(cx: &mut TestAppContext) {
    let mut snapshot: serde_json::Value = serde_json::from_str(SNAPSHOT).expect("the fixture");
    let approve = serde_json::json!([{"key": "a", "label": "Approve", "needs": [], "optional": [], "confirm": true}]);
    let wait = |gate: &str, task: &str| {
        let key = serde_json::json!(["wait", "/work/repo", task, "approval", gate]);
        serde_json::json!({"id": key.to_string(), "key": key, "kind": "approval", "repo": "/work/repo",
            "project": "repo", "task": task, "title": "x", "detail": "", "since": null, "actions": approve})
    };
    let waits = snapshot["waits"].as_array_mut().expect("the waits");
    waits.push(wait("plan", "ov"));
    waits.push(wait("overlap@a9102f70c7c7", "ov"));
    let (rig, desk, cx) =
        super::lane_tests::open_answering(cx, &snapshot.to_string(), CONFIRM, ANSWER);
    desk.update(cx, |desk, cx| {
        desk.drive_press_gate("a", Some("overlap"), "ov", cx)
    });
    wait_until(cx, "the call", |_| rig.acts().len() == 1);
    assert!(
        rig.acts()[0].contains("overlap"),
        "the overlap gate's wait: {}",
        rig.acts()[0]
    );
}

#[gpui::test]
fn settings_button_opens_the_settings_modal_in_the_window_itself(cx: &mut TestAppContext) {
    let (_rig, _desk, cx) = open(cx, SNAPSHOT);
    cx.update(|_, cx| crate::menus::install(cx));
    let windows = |cx: &mut VisualTestContext| cx.update(|_, cx| cx.windows().len());
    let (toggle, settings) = (
        cx.debug_bounds("toggle-sidebar")
            .expect("the sidebar toggle"),
        cx.debug_bounds("status-settings")
            .expect("the Settings button"),
    );
    assert_eq!(
        (toggle.size, toggle.top()),
        (settings.size, settings.top()),
        "the same size, on the same line"
    );
    assert!(
        toggle.right() <= settings.left(),
        "beside the toggle, after it"
    );
    let before = windows(cx);
    cx.simulate_click(settings.center(), Modifiers::none());
    wait_until(cx, "the Settings modal", |cx| {
        cx.debug_bounds("settings-modal").is_some()
    });
    assert_eq!(windows(cx), before, "no window of its own");
}

#[cfg(feature = "snapshot")]
#[gpui::test]
fn drive_hover_leaves_the_pointer_at_the_point_and_presses_nothing(cx: &mut TestAppContext) {
    let (_rig, _desk, cx) = open(cx, SNAPSHOT);
    let over = cx
        .debug_bounds("sidebar-lanes")
        .expect("the Lanes row")
        .center();
    let place = format!("{},{}", f32::from(over.x), f32::from(over.y));
    cx.update(|window, cx| crate::drive_input::hover(&place, window, cx));
    cx.run_until_parked();
    assert_eq!(cx.update(|window, _| window.mouse_position()), over);
    assert!(!cx.did_prompt_for_paths());
}

#[cfg(feature = "snapshot")]
#[gpui::test]
fn drive_scroll_turns_the_wheel_over_the_page_under_the_point(cx: &mut TestAppContext) {
    let (_rig, desk, cx) = open(cx, SNAPSHOT);
    desk.update(cx, |desk, cx| desk.navigate(Page::Inbox, false, cx));
    wait_until(cx, "the Inbox", |cx| {
        cx.debug_bounds("inbox-list").is_some()
    });
    cx.simulate_resize(gpui::size(gpui::px(720.), gpui::px(300.)));
    cx.run_until_parked();
    let selector = "group-0";
    let before = cx.debug_bounds(selector).expect("the first group").top();
    let page = cx.debug_bounds("inbox-list").expect("the list").center();
    let place = format!("{},{},-120", f32::from(page.x), f32::from(page.y));
    cx.update(|window, cx| crate::drive_input::scroll(&place, window, cx));
    cx.run_until_parked();
    let after = cx.debug_bounds(selector).map(|bounds| bounds.top());
    assert!(cx.debug_bounds("inbox-list").is_some());
    assert!(
        after.is_none_or(|top| top < before),
        "{after:?} after the wheel, {before:?} before"
    );
}

/// A click at `selector`'s centre through the scripted input, as the window's own event pipeline gets it.
#[cfg(feature = "snapshot")]
fn drive_click(cx: &mut VisualTestContext, selector: &'static str) {
    let at = cx.debug_bounds(selector).expect(selector).center();
    let place = format!("{},{}", f32::from(at.x), f32::from(at.y));
    cx.update(|window, cx| crate::drive_input::click(&place, window, cx));
    cx.run_until_parked();
}

#[cfg(feature = "snapshot")]
#[gpui::test]
fn drive_click_hits_the_control_under_the_point(cx: &mut TestAppContext) {
    let (_rig, _desk, cx) = super::lane_tests::open_answering(cx, NOTHING, ANSWER, ANSWER);
    wait_until(cx, "the empty page", |cx| {
        cx.debug_bounds("waiting-add-project").is_some()
    });
    // A corner of the empty page: nothing is under the point.
    let page = cx.debug_bounds("inbox").expect("the page");
    let corner = format!(
        "{},{}",
        f32::from(page.left()) + 20.,
        f32::from(page.top()) + 20.
    );
    cx.update(|window, cx| crate::drive_input::click(&corner, window, cx));
    cx.run_until_parked();
    assert!(
        !cx.did_prompt_for_paths(),
        "a click on nothing starts nothing"
    );
    drive_click(cx, "waiting-add-project");
    assert!(
        cx.did_prompt_for_paths(),
        "the row under the point ran its handler"
    );
}

#[cfg(feature = "snapshot")]
#[gpui::test]
fn drive_pick_folder_answers_the_next_folder_panel_once_without_showing_it(
    cx: &mut TestAppContext,
) {
    let (rig, _desk, cx) = super::lane_tests::open_answering(cx, NOTHING, ANSWER, ANSWER);
    wait_until(cx, "the empty page", |cx| {
        cx.debug_bounds("waiting-add-project").is_some()
    });
    let reads = rig.facts.read_with(cx, |facts, _| facts.reads_ended());
    cx.update(|_, cx| crate::ui::project_add::pick_next_folder("/work/picked".into(), cx));
    drive_click(cx, "waiting-add-project");
    assert!(!cx.did_prompt_for_paths(), "the panel was not shown");
    wait_until(cx, "the engine's call", |_| !rig.acts().is_empty());
    assert_eq!(
        rig.acts(),
        ["cockpit act add-project /work/picked --source app --json"]
    );
    wait_until(cx, "the post-add snapshot", |cx| {
        rig.facts.read_with(cx, |facts, _| facts.reads_ended()) > reads
    });
    drive_click(cx, "waiting-add-project");
    assert!(cx.did_prompt_for_paths(), "the next one is the Mac's own");
}

#[cfg(feature = "snapshot")]
#[gpui::test]
fn drive_type_and_key_start_a_lane_by_input_alone(cx: &mut TestAppContext) {
    let (rig, _desk, cx) = open(cx, SNAPSHOT);
    drive_click(cx, "new-lane-repo");
    wait_until(cx, "the sheet", |cx| {
        cx.debug_bounds("new-lane-title").is_some()
    });
    drive_click(cx, "new-lane-title");
    cx.update(|window, cx| {
        crate::drive_input::press_each("add a parser", window, cx);
        crate::drive_input::press("cmd-enter", window, cx);
    });
    wait_until(cx, "the engine's call", |_| !rig.acts().is_empty());
    assert!(
        rig.acts()[0].contains(r#"cockpit act L ["project","/work/repo"] --text add a parser"#),
        "{}",
        rig.acts()[0]
    );
}

#[gpui::test]
fn an_engine_that_cannot_be_read_is_said_on_the_inbox_with_a_button_that_opens_settings(
    cx: &mut TestAppContext,
) {
    let (_rig, _desk, cx) = open_unread(cx, "this is not a snapshot");
    wait_until(cx, "the notice", |cx| {
        cx.debug_bounds("waiting-engine-problem").is_some()
    });
    let asked = Rc::new(std::cell::Cell::new(0));
    let counted = asked.clone();
    cx.update(|_, cx| {
        cx.on_action(move |_: &crate::menus::ShowSettings, _| counted.set(counted.get() + 1));
    });
    let at = cx
        .debug_bounds("waiting-engine-settings")
        .expect("the button")
        .center();
    cx.simulate_click(at, Modifiers::none());
    cx.run_until_parked();
    assert_eq!(asked.get(), 1, "the button asks for Settings");
}

#[gpui::test]
fn an_engine_that_can_be_read_has_no_notice_on_the_inbox(cx: &mut TestAppContext) {
    let (_rig, _desk, cx) = open(cx, SNAPSHOT);
    assert!(cx.debug_bounds("waiting-engine-problem").is_none());
}

#[gpui::test]
fn fix36_a_missing_project_stays_visible_and_does_not_offer_folder_actions(
    cx: &mut TestAppContext,
) {
    let mut snapshot: serde_json::Value = serde_json::from_str(SNAPSHOT).unwrap();
    let other = snapshot["projects"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|project| project["folder"] == "/work/other")
        .unwrap();
    other["folder_missing"] = true.into();
    let (rig, desk, cx) = open(cx, &snapshot.to_string());
    assert!(cx.debug_bounds("group-other").is_some());
    assert!(cx.debug_bounds("sidebar-folder-missing-other").is_some());
    let wait = r#"["wait","/work/other","stuck","blocked","blocked"]"#;
    let row = format!("ask-{wait}");
    let at = super::lane_tests::bounds(cx, &row).expect("the blocked ask is still in Inbox");
    cx.simulate_click(at.center(), Modifiers::none());
    let resume = cx
        .debug_bounds("verb-approve")
        .expect("the disabled Resume");
    cx.simulate_click(resume.center(), Modifiers::none());
    assert!(cx.debug_bounds("inbox-folder-missing").is_some());
    assert!(rig.acts().is_empty());
    desk.update(cx, |desk, cx| desk.open_lanes_of("/work/other", false, cx));
    wait_until(cx, "the lane of the missing project", |cx| {
        cx.debug_bounds("lanes-row-stuck").is_some()
    });
    assert!(cx.debug_bounds("lanes-folder-missing-stuck").is_some());
}

#[gpui::test]
fn fix37_a_missing_project_shows_disabled_resume_and_stop(cx: &mut TestAppContext) {
    let mut snapshot: serde_json::Value = serde_json::from_str(SNAPSHOT).unwrap();
    let other = snapshot["projects"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|project| project["folder"] == "/work/other")
        .unwrap();
    other["folder_missing"] = true.into();
    let lane = snapshot["items"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|item| item["key"][2] == "stuck")
        .unwrap();
    lane["actions"] = serde_json::json!([]);

    let (rig, desk, cx) = open(cx, &snapshot.to_string());
    let ask = r#"["wait","/work/other","stuck","blocked","blocked"]"#;
    super::lane_tests::click(cx, &format!("ask-{ask}"));
    super::lane_tests::click(cx, "verb-approve");
    super::lane_tests::click(cx, "verb-stop");
    cx.simulate_keystrokes("a");
    assert!(cx.debug_bounds("inbox-folder-missing").is_some());
    let key = super::engine_rig_tests::lane_key("stuck");
    desk.update(cx, |desk, cx| desk.open_lane(&key, cx));
    wait_until(cx, "the blocked card", |cx| {
        cx.debug_bounds("agents-problem").is_some()
    });
    super::lane_tests::click(cx, &format!("ask-{ask}-0"));
    super::lane_tests::click(cx, &format!("ask-{ask}-1"));
    assert!(rig.acts().is_empty());
}

#[gpui::test]
fn committed_split_changes_reach_preferences_through_the_page_bar(cx: &mut TestAppContext) {
    let (_rig, desk, cx) = open(cx, SNAPSHOT);
    let bar = desk.read_with(cx, |desk, _| desk.bar.clone());
    bar.update(cx, |bar, cx| {
        let left = bar.tabs().active().id();
        let right = bar.open_new(Page::Lanes, cx);
        assert!(bar.create_split(left, right, cx));
        assert!(bar.set_split_share(left, right, 5700, cx));
        assert!(bar.swap_split(left, cx));
    });
    let stored = cx.update(|_, cx| Preferences::current(cx).tabs.unwrap());
    assert_eq!(stored["split"]["left_share_bps"], 5700);
    let restored = Tabs::restore(Some(&stored));
    assert_eq!(restored.tabs()[0].page(), &Page::Lanes);
    assert_eq!(restored.tabs()[1].page(), &Page::Inbox);
}

#[gpui::test]
fn late_page_event_routes_by_source_tab_or_is_ignored_if_hidden(cx: &mut TestAppContext) {
    let (_rig, desk, cx) = open(cx, SNAPSHOT);
    let bar = desk.read_with(cx, |desk, _| desk.bar.clone());
    let (left, right) = bar.update(cx, |bar, cx| {
        let left = bar.tabs().active().id();
        let right = bar.open_new(Page::Inbox, cx);
        assert!(bar.create_split(left, right, cx));
        (left, right)
    });
    desk.update(cx, |desk, cx| {
        assert!(desk.follow_page_link(left, &Page::Inbox, Page::Lanes, false, cx));
        assert!(!desk.follow_page_link(left, &Page::Inbox, Page::Engine, false, cx));
    });
    assert_eq!(pages(&desk, cx), [Page::Lanes, Page::Inbox]);
    bar.update(cx, |bar, cx| {
        assert_eq!(bar.tabs().active().id(), right);
        bar.open_new(Page::Master, cx);
    });
    desk.update(cx, |desk, cx| {
        assert!(!desk.follow_page_link(right, &Page::Inbox, Page::Engine, true, cx));
    });
    assert_eq!(pages(&desk, cx), [Page::Lanes, Page::Inbox, Page::Master]);
}

#[gpui::test]
fn two_projects_resolve_their_own_folder(cx: &mut TestAppContext) {
    let (_rig, desk, cx) = open(cx, SNAPSHOT);
    let bar = desk.read_with(cx, |desk, _| desk.bar.clone());
    let (left, right) = bar.update(cx, |bar, cx| {
        bar.navigate(Page::Project("/work/repo".into()), cx);
        let left = bar.tabs().active().id();
        let right = bar.open_new(Page::Project("/work/other".into()), cx);
        assert!(bar.create_split(left, right, cx));
        (left, right)
    });
    desk.read_with(cx, |desk, cx| {
        let first = desk.project_for_test(left, "/work/repo").unwrap();
        let second = desk.project_for_test(right, "/work/other").unwrap();
        assert_ne!(first, second);
        assert_eq!(first.read(cx).folder_for_test(), "/work/repo");
        assert_eq!(second.read(cx).folder_for_test(), "/work/other");
    });
}

#[gpui::test]
fn two_views_of_the_same_page_have_distinct_scroll_and_selection(cx: &mut TestAppContext) {
    let (_rig, desk, cx) = open(cx, SNAPSHOT);
    let bar = desk.read_with(cx, |desk, _| desk.bar.clone());
    let (left, right) = bar.update(cx, |bar, cx| {
        let left = bar.tabs().active().id();
        let right = bar.open_new(Page::Inbox, cx);
        assert!(bar.create_split(left, right, cx));
        (left, right)
    });
    let (first, second) = desk.read_with(cx, |desk, _| {
        (
            desk.inbox_for_test(left).unwrap().clone(),
            desk.inbox_for_test(right).unwrap().clone(),
        )
    });
    let offset = gpui::point(gpui::px(0.), gpui::px(-23.));
    cx.update(|_, cx| {
        first.update(cx, |page, cx| {
            page.select("only-left".to_string(), cx);
            page.set_detail_scroll_for_test(offset);
        });
        assert_eq!(
            first.read(cx).selection_and_scroll_for_test(),
            (Some("only-left"), offset)
        );
        let (selection, scroll) = second.read(cx).selection_and_scroll_for_test();
        assert_ne!(selection, Some("only-left"));
        assert_ne!(scroll, offset);
    });
}

#[gpui::test]
fn both_visible_lane_pages_are_shown_and_background_readers_stop(cx: &mut TestAppContext) {
    let (_rig, desk, cx) = open(cx, SNAPSHOT);
    let bar = desk.read_with(cx, |desk, _| desk.bar.clone());
    let lane = Page::Lane(engine_rig_tests::lane_key("demo"));
    let (left, right) = bar.update(cx, |bar, cx| {
        bar.navigate(lane.clone(), cx);
        let left = bar.tabs().active().id();
        let right = bar.open_new(lane.clone(), cx);
        assert!(bar.create_split(left, right, cx));
        (left, right)
    });
    let (first, second) = desk.read_with(cx, |desk, _| {
        (
            desk.lane_view(left, &lane).unwrap().clone(),
            desk.lane_view(right, &lane).unwrap().clone(),
        )
    });
    assert_ne!(first, second);
    cx.update(|_, cx| {
        assert!(first.read(cx).is_shown_for_test());
        assert!(second.read(cx).is_shown_for_test());
    });
    bar.update(cx, |bar, cx| {
        bar.open_new(Page::Lanes, cx);
    });
    cx.update(|_, cx| {
        assert!(!first.read(cx).is_shown_for_test());
        assert!(!second.read(cx).is_shown_for_test());
    });
}

#[gpui::test]
fn missing_lane_in_one_pane_leaves_the_other_usable(cx: &mut TestAppContext) {
    let (_rig, desk, cx) = open(cx, SNAPSHOT);
    let bar = desk.read_with(cx, |desk, _| desk.bar.clone());
    let missing = Page::Lane("missing".into());
    let (left, right) = bar.update(cx, |bar, cx| {
        let left = bar.tabs().active().id();
        let right = bar.open_new(missing.clone(), cx);
        assert!(bar.create_split(left, right, cx));
        (left, right)
    });
    desk.update(cx, |desk, cx| {
        assert!(desk.follow_page_link(left, &Page::Inbox, Page::Lanes, false, cx));
    });
    bar.read_with(cx, |bar, _| {
        assert_eq!(bar.tabs().tab(left).unwrap().page(), &Page::Lanes);
        assert_eq!(bar.tabs().tab(right).unwrap().page(), &missing);
    });
}

pub(super) fn make_split(
    desk: &Entity<Desk>,
    left_page: Page,
    right_page: Page,
    share: u16,
    cx: &mut VisualTestContext,
) -> (sakura_core::tabs::TabId, sakura_core::tabs::TabId) {
    let bar = desk.read_with(cx, |desk, _| desk.bar.clone());
    bar.update(cx, |bar, cx| {
        bar.navigate(left_page, cx);
        let left = bar.tabs().active().id();
        let right = bar.open_new(right_page, cx);
        assert!(bar.create_split(left, right, cx));
        bar.set_split_share(left, right, share, cx);
        (left, right)
    })
}

pub(super) fn pane_bounds(
    id: sakura_core::tabs::TabId,
    cx: &mut VisualTestContext,
) -> gpui::Bounds<gpui::Pixels> {
    super::lane_tests::bounds(cx, &format!("page-pane-{}", id.as_u64())).expect("pane drawn")
}

#[gpui::test]
fn restored_asymmetric_split_first_frame_uses_saved_ratio(cx: &mut TestAppContext) {
    let mut tabs = Tabs::new();
    let left = tabs.active().id();
    let right = tabs.open_new(Page::Lanes);
    tabs.create_split(left, right);
    tabs.set_split_share(left, right, 5800);
    let preferences = Preferences {
        tabs: Some(tabs.to_json()),
        ..Preferences::default()
    };
    let (_rig, desk, cx) =
        super::lane_tests::open_answering_with(cx, SNAPSHOT, (ANSWER, ANSWER), preferences);
    let (left, right) = desk.read_with(cx, |desk, cx| {
        let (left, right) = desk.bar.read(cx).tabs().visible();
        (left, right.unwrap())
    });
    let left = pane_bounds(left, cx);
    let right = pane_bounds(right, cx);
    let total = f32::from(left.size.width + right.size.width);
    assert!((f32::from(left.size.width) / total - 0.58).abs() < 0.005);
}

#[gpui::test]
fn each_pane_renders_its_own_header_and_controls(cx: &mut TestAppContext) {
    let (_rig, desk, cx) = open(cx, SNAPSHOT);
    let (left, right) = make_split(&desk, Page::Inbox, Page::Lanes, 5000, cx);
    cx.run_until_parked();
    assert!(pane_bounds(left, cx).right() <= pane_bounds(right, cx).left());
    assert!(super::lane_tests::bounds(cx, &format!("tab-header-{}", left.as_u64())).is_some());
    assert!(super::lane_tests::bounds(cx, &format!("tab-header-{}", right.as_u64())).is_some());
}

#[gpui::test]
fn swap_keeps_divider_x_and_moves_both_page_entities(cx: &mut TestAppContext) {
    let (_rig, desk, cx) = open(cx, SNAPSHOT);
    let (left, right) = make_split(&desk, Page::Inbox, Page::Inbox, 5900, cx);
    cx.run_until_parked();
    let before = pane_bounds(left, cx).right();
    let old_left = desk.read_with(cx, |desk, _| desk.inbox_for_test(left).unwrap().clone());
    let bar = desk.read_with(cx, |desk, _| desk.bar.clone());
    bar.update(cx, |bar, cx| assert!(bar.swap_split(left, cx)));
    cx.run_until_parked();
    let after = pane_bounds(right, cx).right();
    assert!((f32::from(after) - f32::from(before)).abs() <= 1.);
    desk.read_with(cx, |desk, _| {
        assert_eq!(&old_left, desk.inbox_for_test(left).unwrap())
    });
}

#[gpui::test]
fn create_resize_collapse_keeps_the_shell_bounds(cx: &mut TestAppContext) {
    let (_rig, desk, cx) = open(cx, SNAPSHOT);
    let shell = |cx: &mut VisualTestContext| {
        ["page-bar", "sidebar-lanes", "status-bar"].map(|id| cx.debug_bounds(id).unwrap())
    };
    let before = shell(cx);
    let (left, right) = make_split(&desk, Page::Inbox, Page::Lanes, 6000, cx);
    cx.run_until_parked();
    assert!(pane_bounds(left, cx).right() <= pane_bounds(right, cx).left());
    assert_eq!(before, shell(cx));
    let bar = desk.read_with(cx, |desk, _| desk.bar.clone());
    bar.update(cx, |bar, cx| assert!(bar.close(right, cx)));
    cx.run_until_parked();
    assert_eq!(before, shell(cx));
}

#[gpui::test]
fn minimum_window_with_sidebar_has_two_usable_content_regions(cx: &mut TestAppContext) {
    let (_rig, desk, cx) = open(cx, SNAPSHOT);
    cx.simulate_resize(gpui::size(gpui::px(1100.), gpui::px(700.)));
    let (left, right) = make_split(&desk, Page::Inbox, Page::Engine, 4000, cx);
    let bar = desk.read_with(cx, |desk, _| desk.bar.clone());
    let long_title = "project name ".repeat(20);
    for page in [
        Page::Inbox,
        Page::Master,
        Page::Lanes,
        Page::Engine,
        Page::Lane(engine_rig_tests::lane_key("demo")),
        Page::Project(long_title),
    ] {
        bar.update(cx, |bar, cx| {
            bar.navigate_from(left, page, cx);
        });
        cx.run_until_parked();
        let left = pane_bounds(left, cx);
        let right = pane_bounds(right, cx);
        let panel = cx.debug_bounds("page-panel").unwrap();
        assert!(left.size.width >= gpui::px(300.));
        assert!(right.size.width >= gpui::px(300.));
        assert!(left.right() <= right.left());
        assert!(left.left() >= panel.left() && right.right() <= panel.right());
    }
}

#[gpui::test]
fn resize_clamps_and_repaints_without_rebuilding_pages(cx: &mut TestAppContext) {
    use gpui::{point, px, MouseButton};
    let (_rig, desk, cx) = open(cx, SNAPSHOT);
    let (left, right) = make_split(&desk, Page::Inbox, Page::Inbox, 5000, cx);
    cx.run_until_parked();
    let old_left = desk.read_with(cx, |desk, _| desk.inbox_for_test(left).unwrap().clone());
    let pane = pane_bounds(left, cx);
    let start = point(pane.right(), pane.center().y);
    let end = point(start.x + px(280.), start.y);
    cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(
        point(start.x + px(20.), start.y),
        MouseButton::Left,
        Modifiers::none(),
    );
    cx.simulate_mouse_move(end, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_up(end, MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
    let bar = desk.read_with(cx, |desk, _| desk.bar.clone());
    bar.read_with(cx, |bar, _| {
        assert_eq!(bar.tabs().split().unwrap().left_share_bps, 6000)
    });
    assert!(pane_bounds(left, cx).size.width > pane.size.width);
    assert!(pane_bounds(left, cx).right() <= pane_bounds(right, cx).left());
    desk.read_with(cx, |desk, _| {
        assert_eq!(&old_left, desk.inbox_for_test(left).unwrap())
    });
}

#[gpui::test]
fn escape_after_a_pointer_open_does_not_focus_the_settings_gear(cx: &mut TestAppContext) {
    let (_rig, _desk, cx) = open(cx, SNAPSHOT);
    cx.update(|_, cx| crate::menus::install(cx));
    let settings = cx.debug_bounds("status-settings").unwrap();
    let before = cx.update(|window, cx| window.focused(cx));
    cx.simulate_mouse_move(settings.center(), None, Modifiers::none());
    cx.simulate_event(gpui::MouseDownEvent {
        button: gpui::MouseButton::Left,
        position: settings.center(),
        modifiers: Modifiers::none(),
        click_count: 1,
        first_mouse: false,
    });
    cx.simulate_event(gpui::MouseUpEvent {
        button: gpui::MouseButton::Left,
        position: settings.center(),
        modifiers: Modifiers::none(),
        click_count: 1,
    });
    wait_until(cx, "Settings", |cx| {
        cx.debug_bounds("settings-modal").is_some()
    });
    cx.simulate_keystrokes("escape");
    wait_until(cx, "Settings closed", |cx| {
        cx.debug_bounds("settings-modal").is_none()
    });
    assert_eq!(cx.update(|window, cx| window.focused(cx)), before);
}

#[gpui::test]
fn a_lane_tab_switches_from_stopped_to_closed_when_cleaned(cx: &mut TestAppContext) {
    use crate::ui::kit::StateGlyph;

    let mut snapshot: serde_json::Value = serde_json::from_str(SNAPSHOT).unwrap();
    let item = snapshot["items"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|item| item["key"][2] == "demo")
        .unwrap();
    item["agent_state"] = "stopped".into();
    let (rig, desk, cx) = open(cx, &snapshot.to_string());
    let key = engine_rig_tests::lane_key("demo");
    desk.update(cx, |desk, cx| {
        desk.navigate(Page::Lane(key.clone()), true, cx)
    });
    let glyph = |cx: &mut VisualTestContext| {
        desk.read_with(cx, |desk, cx| {
            desk.bar.read(cx).glyphs_of(std::slice::from_ref(&key))[0]
        })
    };
    assert_eq!(glyph(cx), Some(StateGlyph::Stopped));

    snapshot["items"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|item| item["key"][2] == "demo")
        .unwrap()["agent_state"] = "cleaned".into();
    rig.say_snapshot(&snapshot.to_string());
    rig.facts.update(cx, |facts, cx| facts.read(cx));
    wait_until(cx, "the cleaned lane", |cx| {
        rig.facts.read_with(cx, |facts, _| {
            facts
                .snapshot()
                .unwrap()
                .items
                .iter()
                .any(|item| item.agent_state == "cleaned")
        })
    });
    assert_eq!(glyph(cx), Some(StateGlyph::NotStarted));
}

#[test]
fn polish_zd_settings_dialogs_and_new_lane_replay_text_at_the_minimum_size() {
    super::real_text::run(settings_and_new_lane);
}

fn settings_and_new_lane(cx: &mut TestAppContext) {
    use super::settings_modal::Section;
    let fixture = super::engine_fixture_tests::edited(|snapshot| {
        for lane in snapshot["items"].as_array_mut().unwrap().iter_mut().take(2) {
            lane["agent_state"] = "working".into();
        }
    });
    let (_rig, desk, cx) = open(cx, &fixture);
    cx.simulate_resize(gpui::size(gpui::px(1100.), gpui::px(700.)));
    desk.update(cx, |desk, cx| desk.navigate(Page::Lanes, false, cx));
    desk.update_in(cx, |desk, window, cx| {
        desk.open_settings_at(Section::Accounts, window, cx);
    });
    super::lane_tests::click(cx, "settings-accounts-add");
    wait_until(cx, "Add account", |cx| {
        cx.debug_bounds("account-add").is_some()
    });
    cx.simulate_keystrokes("escape");
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(1));
    cx.run_until_parked();
    assert!(cx.debug_bounds("account-add").is_none());
    super::lane_tests::click(cx, "settings-section-connectors");
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(cx.debug_bounds("settings-connectors-intro").is_some());
    cx.simulate_keystrokes("escape");
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(1));
    cx.run_until_parked();
    cx.update(|_, cx| crate::menus::install(cx));
    cx.simulate_keystrokes("cmd-n");
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(cx.debug_bounds("new-lane-title").is_some());
    for _ in 0..4 {
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }
}

#[gpui::test]
fn polish_zd_toasts_are_outside_the_inbox_detail_and_keep_lane_divider_height(
    cx: &mut TestAppContext,
) {
    let (_rig, desk, cx) = open(cx, SNAPSHOT);
    cx.simulate_resize(gpui::size(gpui::px(1100.), gpui::px(700.)));
    let inbox = desk.read_with(cx, |desk, _| desk.inbox.clone());
    inbox.update(cx, |inbox, cx| inbox.select(APPROVAL.to_string(), cx));
    cx.update(|window, cx| {
        cx.set_reduce_motion(true);
        super::kit::Toast::show(window, cx, "Approving the spec", Some(Box::new(|_, _| {})));
        window.draw(cx).clear(cx);
    });
    let detail = cx.debug_bounds("inbox-detail").unwrap();
    let toast = cx.debug_bounds("toast").unwrap();
    assert!(toast.right() <= detail.left() || toast.top() >= detail.bottom());
    desk.update(cx, |desk, cx| {
        desk.open_lane(&engine_rig_tests::lane_key("demo"), cx)
    });
    wait_until(cx, "the lane", |cx| cx.debug_bounds("lane-side").is_some());
    let panel = cx.debug_bounds("page-panel").unwrap();
    let divider = cx.debug_bounds("lane-side").unwrap();
    assert_eq!(
        divider.bottom(),
        panel.bottom() - crate::theme::PANEL_RING_WIDTH
    );
}
