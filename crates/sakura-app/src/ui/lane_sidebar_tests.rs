//! The sidebar of the engine's window: the pages it lists with their counts, the projects with their open
//! lanes, and what a press on each does.

use super::engine_desk::{Desk, DeskData};
use super::engine_rig_tests::{self, wait_until, Rig, SNAPSHOT};
use super::lane_sidebar::counts;
use super::lane_tests::{bounds, click};
use crate::prefs::Preferences;
use gpui::{px, AppContext as _, Entity, Modifiers, TestAppContext, VisualTestContext};
use sakura_core::engine::snapshot::Snapshot;
use sakura_core::tabs::Page;
use std::cell::RefCell;
use std::rc::Rc;

const ANSWER: &str = r#"{"result": "done", "exit_code": 0, "output": "ok\n"}"#;
const NOTHING: &str = r#"{"api": 1, "waits": [], "items": [], "events": [], "problems": []}"#;

/// The fixture changed by `change`, as the engine's text.
fn changed(change: impl FnOnce(&mut serde_json::Value)) -> String {
    let mut document: serde_json::Value = serde_json::from_str(SNAPSHOT).expect("the fixture");
    change(&mut document);
    document.to_string()
}

/// The fixture of an engine that reports its pace.
fn with_pace() -> String {
    changed(|document| document["engine"] = serde_json::json!({"pace": "normal"}))
}

/// The desk over the fake engine's `snapshot` and the kept `preferences`, the sidebar drawn.
fn open<'a>(
    cx: &'a mut TestAppContext,
    snapshot: &str,
    preferences: Preferences,
) -> (Rig, Entity<Desk>, &'a mut VisualTestContext) {
    let rig = engine_rig_tests::open(cx, snapshot, ANSWER, ANSWER);
    cx.update(|cx| preferences.clone().install(None, cx));
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
    wait_until(cx, "the first snapshot", |cx| {
        rig.facts
            .read_with(cx, |facts, _| facts.snapshot().is_some())
    });
    cx.run_until_parked();
    (rig, desk, cx)
}

fn pages(desk: &Entity<Desk>, cx: &mut VisualTestContext) -> Vec<Page> {
    desk.read_with(cx, |desk, cx| {
        let tabs = desk.bar.read(cx).tabs();
        tabs.tabs().iter().map(|tab| tab.page().clone()).collect()
    })
}

fn parsed(text: &str) -> Snapshot {
    Snapshot::parse(text.as_bytes()).expect("the snapshot")
}

#[test]
fn the_counts_are_the_open_asks_the_open_lanes_and_the_pace_in_words() {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let counted = counts(Some(&parsed(&with_pace())));
    assert_eq!(counted.asks, 5, "one per wait of the fixture");
    assert_eq!(counted.lanes, 5);
    assert_eq!(counted.pace.as_deref(), Some("Normal"));
    let ended = changed(|document| document["items"][0]["agent_state"] = "delivered".into());
    assert_eq!(counts(Some(&parsed(&ended))).lanes, 4, "a lane that ended");
    let before_any = counts(None);
    assert_eq!((before_any.asks, before_any.lanes), (0, 0));
    assert_eq!(before_any.pace, None);
}

#[gpui::test]
fn the_engine_row_says_its_pace_and_says_nothing_when_the_engine_reports_none(
    cx: &mut TestAppContext,
) {
    let (_rig, _desk, cx) = open(cx, &with_pace(), Preferences::default());
    assert!(bounds(cx, "sidebar-engine-word").is_some());
    let (_rig, _desk, cx) = open(cx, SNAPSHOT, Preferences::default());
    assert!(bounds(cx, "sidebar-engine").is_some(), "the row stays");
    assert!(
        bounds(cx, "sidebar-engine-word").is_none(),
        "an engine with no `engine` part says no pace"
    );
}

#[gpui::test]
fn the_inbox_and_lanes_rows_show_a_count_only_when_there_is_something_to_count(
    cx: &mut TestAppContext,
) {
    let (_rig, _desk, cx) = open(cx, SNAPSHOT, Preferences::default());
    assert!(bounds(cx, "sidebar-inbox-count").is_some());
    assert!(bounds(cx, "sidebar-lanes-count").is_some());
}

/// The fixture with one ask only: the person's row of the lane `demo`.
fn one_ask() -> String {
    changed(|document| {
        let waits = document["waits"].as_array_mut().expect("the waits");
        waits.retain(|wait| {
            wait["id"]
                .as_str()
                .is_some_and(|id| id.contains("owner_row\",\"R1"))
        });
    })
}

#[gpui::test]
fn a_snoozed_wait_hides_the_inbox_count_until_it_wakes(cx: &mut TestAppContext) {
    let active = one_ask();
    let mut document: serde_json::Value = serde_json::from_str(&active).unwrap();
    document["waits"][0]["is_snoozed"] = true.into();
    document["waits"][0]["snoozed_until"] = "2099-11-22T18:00:00Z".into();
    let (rig, _, cx) = open(cx, &document.to_string(), Preferences::default());
    assert!(bounds(cx, "sidebar-inbox-count").is_none());
    assert!(bounds(cx, "inbox-nothing").is_some());
    rig.say_snapshot(&active);
    rig.facts.update(cx, |facts, cx| facts.read(cx));
    wait_until(cx, "the count returns", |cx| {
        bounds(cx, "sidebar-inbox-count").is_some()
    });
    assert!(bounds(cx, "inbox-nothing").is_none());
}

#[gpui::test]
fn the_inbox_count_drops_when_the_answer_is_held_and_stays_down_once_it_was_sent(
    cx: &mut TestAppContext,
) {
    let (rig, _desk, cx) = open(cx, &one_ask(), Preferences::default());
    cx.update(|_, cx| super::inbox::install_keys(cx));
    assert!(bounds(cx, "sidebar-inbox-count").is_some(), "one ask waits");
    click(cx, r#"ask-["wait","/work/repo","demo","owner_row","R1"]"#);
    cx.simulate_keystrokes("a");
    assert!(bounds(cx, "toast-undo").is_some(), "the answer is held");
    assert!(
        bounds(cx, "sidebar-inbox-count").is_none(),
        "held for Undo: the ask is no longer one that waits"
    );
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(7));
    wait_until(cx, "the answer", |_| rig.acts().len() == 1);
    wait_until(cx, "the engine's word", |cx| {
        bounds(cx, "toast-undo").is_none()
    });
    assert!(
        bounds(cx, "sidebar-inbox-count").is_none(),
        "sent, though the engine still lists it until its next word"
    );
}

#[gpui::test]
fn undo_brings_the_inbox_count_back(cx: &mut TestAppContext) {
    let (_rig, _desk, cx) = open(cx, &one_ask(), Preferences::default());
    cx.update(|_, cx| super::inbox::install_keys(cx));
    click(cx, r#"ask-["wait","/work/repo","demo","owner_row","R1"]"#);
    cx.simulate_keystrokes("a");
    assert!(bounds(cx, "sidebar-inbox-count").is_none());
    click(cx, "toast-undo");
    assert!(bounds(cx, "sidebar-inbox-count").is_some());
}

#[gpui::test]
fn with_nothing_to_count_no_count_is_drawn(cx: &mut TestAppContext) {
    let (_rig, _desk, cx) = open(cx, NOTHING, Preferences::default());
    assert!(bounds(cx, "sidebar-lanes").is_some());
    assert!(bounds(cx, "sidebar-inbox-count").is_none());
    assert!(bounds(cx, "sidebar-lanes-count").is_none());
    assert!(
        bounds(cx, "sidebar-new-lane").is_none(),
        "no project to start a lane in"
    );
}

#[gpui::test]
fn the_projects_heading_has_one_control_a_plain_plus_that_adds_a_project_and_no_project_row_loses_its_own(
    cx: &mut TestAppContext,
) {
    let (_rig, _desk, cx) = open(cx, SNAPSHOT, Preferences::default());
    let heading = bounds(cx, "sidebar-projects-heading").expect("the heading");
    let add = bounds(cx, "sidebar-add-project").expect("the one control");
    assert!(heading.contains(&add.center()));
    assert!(
        bounds(cx, "sidebar-new-lane").is_none(),
        "a new lane is never started from the heading"
    );
    assert!(
        bounds(cx, "new-lane-repo").is_some(),
        "the project row keeps its own plus"
    );
}

#[gpui::test]
fn a_press_on_a_page_row_shows_that_page_and_with_cmd_opens_it_in_a_tab_of_its_own(
    cx: &mut TestAppContext,
) {
    let (_rig, desk, cx) = open(cx, SNAPSHOT, Preferences::default());
    click(cx, "sidebar-master");
    assert_eq!(pages(&desk, cx), [Page::Master]);
    let at = bounds(cx, "sidebar-engine").expect("the row").center();
    cx.simulate_click(at, Modifiers::command());
    assert_eq!(pages(&desk, cx), [Page::Master, Page::Engine]);
    click(cx, "sidebar-lanes");
    assert_eq!(pages(&desk, cx), [Page::Master, Page::Lanes]);
    click(cx, "sidebar-inbox");
    assert_eq!(pages(&desk, cx), [Page::Master, Page::Inbox]);
}

#[gpui::test]
fn the_row_of_the_open_page_is_the_one_marked_current(cx: &mut TestAppContext) {
    let (_rig, desk, cx) = open(cx, SNAPSHOT, Preferences::default());
    let current = |cx: &mut VisualTestContext| bounds(cx, "sidebar-current");
    assert_eq!(current(cx), bounds(cx, "sidebar-inbox"));
    desk.update(cx, |desk, cx| desk.navigate(Page::Engine, false, cx));
    cx.run_until_parked();
    assert_eq!(current(cx), bounds(cx, "sidebar-engine"));
    desk.update(cx, |desk, cx| {
        desk.open_lane(&engine_rig_tests::lane_key("demo"), cx)
    });
    cx.run_until_parked();
    assert_eq!(current(cx), bounds(cx, "row-demo"));
}

#[gpui::test]
fn only_open_lanes_are_listed_under_their_project(cx: &mut TestAppContext) {
    let ended = changed(|document| document["items"][1]["agent_state"] = "delivered".into());
    let (_rig, _desk, cx) = open(cx, &ended, Preferences::default());
    assert!(bounds(cx, "row-gated").is_none(), "it was delivered");
    assert!(bounds(cx, "row-demo").is_some());
    assert!(bounds(cx, "project-repo").is_some());
}

#[gpui::test]
fn a_project_chevron_follows_its_last_open_lane_and_keeps_the_name_aligned(
    cx: &mut TestAppContext,
) {
    let snapshot = |state: &str| {
        changed(|document| {
            for item in document["items"].as_array_mut().unwrap() {
                if item["repo"] == "/work/repo" {
                    item["agent_state"] = if item["key"][2] == "demo" {
                        state
                    } else {
                        "delivered"
                    }
                    .into();
                }
            }
        })
    };
    let empty = snapshot("delivered");
    let (rig, _, cx) = open(cx, &empty, Preferences::default());
    let chevron = "project-chevron-repo";
    let name_left = bounds(cx, "group-other").unwrap().left();
    assert!(bounds(cx, "project-chevron-other").is_some());
    assert!(bounds(cx, "row-demo").is_none());
    assert!(bounds(cx, chevron).is_none());
    assert_eq!(bounds(cx, "group-repo").unwrap().left(), name_left);
    press_the_chevron(cx, "project-repo");
    assert!(cx.update(|_, cx| Preferences::current(cx).collapsed_projects.is_empty()));

    for (state, has_lane) in [("running", true), ("delivered", false)] {
        rig.say_snapshot(&snapshot(state));
        rig.facts.update(cx, |facts, cx| facts.read(cx));
        wait_until(cx, state, |cx| bounds(cx, "row-demo").is_some() == has_lane);
        assert_eq!(bounds(cx, chevron).is_some(), has_lane);
        assert_eq!(bounds(cx, "group-repo").unwrap().left(), name_left);
    }
}

/// Presses the left end of a project's row, where its chevron is: the name opens the project's page, the rest of
/// the row folds its lanes.
fn press_the_chevron(cx: &mut gpui::VisualTestContext, row: &str) {
    let row = bounds(cx, row).expect("the project's row");
    cx.simulate_click(
        gpui::point(row.left() + gpui::px(14.), row.center().y),
        gpui::Modifiers::none(),
    );
}

#[gpui::test]
fn a_press_on_a_project_closes_its_lanes_and_opens_them_again_and_the_choice_is_kept(
    cx: &mut TestAppContext,
) {
    let (_rig, _desk, cx) = open(cx, SNAPSHOT, Preferences::default());
    press_the_chevron(cx, "project-repo");
    cx.run_until_parked();
    assert!(bounds(cx, "row-demo").is_none());
    assert!(bounds(cx, "project-repo").is_some(), "the project stays");
    assert!(
        bounds(cx, "row-stuck").is_some(),
        "the other project's lane"
    );
    let kept = cx.update(|_, cx| Preferences::current(cx).collapsed_projects);
    assert_eq!(kept, ["/work/repo".to_string()].into());
    press_the_chevron(cx, "project-repo");
    cx.run_until_parked();
    assert!(bounds(cx, "row-demo").is_some());
    assert!(cx.update(|_, cx| Preferences::current(cx).collapsed_projects.is_empty()));
}

#[gpui::test]
fn a_project_kept_closed_by_the_last_launch_opens_closed(cx: &mut TestAppContext) {
    let kept = Preferences {
        collapsed_projects: ["/work/repo".to_string()].into(),
        ..Preferences::default()
    };
    let (_rig, _desk, cx) = open(cx, SNAPSHOT, kept);
    assert!(bounds(cx, "project-repo").is_some());
    assert!(bounds(cx, "row-stuck").is_some());
    assert!(bounds(cx, "row-demo").is_none(), "it was closed");
}

#[gpui::test]
fn the_plus_of_a_project_row_opens_the_new_lane_sheet(cx: &mut TestAppContext) {
    let (_rig, _desk, cx) = open(cx, SNAPSHOT, Preferences::default());
    click(cx, "new-lane-repo");
    wait_until(cx, "the sheet", |cx| {
        cx.debug_bounds("new-lane-sheet").is_some()
    });
}

#[gpui::test]
fn a_lane_row_has_its_mark_its_title_and_a_thin_progress_line_when_it_has_a_plan(
    cx: &mut TestAppContext,
) {
    let (_rig, _desk, cx) = open(cx, SNAPSHOT, Preferences::default());
    let row = bounds(cx, "row-demo").expect("the row");
    let (mark, title) = (
        bounds(cx, "glyph-demo").expect("the mark"),
        bounds(cx, "name-demo").expect("the title"),
    );
    let line = bounds(cx, "row-demo-bar").expect("the line");
    assert!(mark.right() <= title.left() && title.right() <= line.left());
    assert!(line.right() <= row.right() && line.size.height <= px(4.));
    assert_eq!(row.size.height, px(28.));
    assert!(
        bounds(cx, "row-fresh-bar").is_none(),
        "a lane with no plan has no line"
    );
}

#[gpui::test]
fn the_rows_are_thirty_high_start_under_the_title_bar_and_end_where_the_arrows_end(
    cx: &mut TestAppContext,
) {
    let (_rig, _desk, cx) = open(cx, SNAPSHOT, Preferences::default());
    let inbox = bounds(cx, "sidebar-inbox").expect("the first row");
    let title = bounds(cx, "page-bar").expect("the title bar");
    assert_eq!(inbox.top(), title.bottom(), "right under the title bar");
    assert_eq!(inbox.size.height, px(30.));
    assert_eq!(inbox.left(), px(8.));
    assert_eq!(inbox.right(), crate::theme::SIDEBAR_WIDTH - px(8.));
    let icon = bounds(cx, "sidebar-inbox-icon").expect("the icon");
    assert_eq!(icon.size, gpui::size(px(16.), px(16.)));
}

/// The weight a row's frame sets for its text, if it sets one.
fn frame_weight(is_current: bool) -> Option<gpui::FontWeight> {
    let palette = crate::theme::Appearance::Light.palette();
    let mut row = super::sidebar_pages::frame("row", px(30.), is_current, &palette);
    gpui::Styled::style(&mut row).text.font_weight
}

#[test]
fn a_sidebar_row_has_the_same_weight_open_and_not() {
    assert_eq!(
        frame_weight(true),
        frame_weight(false),
        "the open page's row is its fill and its ink, never a heavier word"
    );
}

#[test]
fn a_page_row_s_name_is_as_wide_open_as_not() {
    super::real_text::run(|cx| {
        let (_rig, desk, cx) = open(cx, SNAPSHOT, Preferences::default());
        let widths = |cx: &mut VisualTestContext| {
            ["inbox", "master", "lanes", "engine"]
                .map(|name| bounds(cx, &format!("sidebar-{name}-label")).map(|b| b.size.width))
        };
        let before = widths(cx);
        for page in [Page::Master, Page::Lanes, Page::Engine, Page::Inbox] {
            desk.update(cx, |desk, cx| desk.navigate(page, false, cx));
            cx.run_until_parked();
            assert_eq!(before, widths(cx), "a name is as wide open as not");
        }
        assert!(before.iter().all(Option::is_some), "every row is named");
    });
}

#[gpui::test]
fn a_lane_of_an_outside_agent_is_listed_under_from_outside_and_is_no_project(
    cx: &mut TestAppContext,
) {
    let digest = changed(|document| document["items"][4]["repo"] = "/work/measure".into());
    let (_rig, _desk, cx) = open(cx, &digest, Preferences::default());
    assert!(bounds(cx, "sidebar-outside").is_some(), "its own heading");
    assert!(
        bounds(cx, "row-measure-help").is_some(),
        "its lane is listed"
    );
    assert!(
        bounds(cx, "project-measure").is_none(),
        "a folder only an outside agent works in is not a project"
    );
    let heading = bounds(cx, "sidebar-outside").expect("the heading");
    let lane = bounds(cx, "row-measure-help").expect("the row");
    assert!(heading.bottom() <= lane.top(), "the heading is above it");
    assert!(
        bounds(cx, "project-repo").expect("a project").bottom() <= heading.top(),
        "after the projects"
    );
}

#[gpui::test]
fn the_sidebar_says_nothing_of_outside_agents_when_all_lanes_are_in_projects(
    cx: &mut TestAppContext,
) {
    let (_rig, _desk, cx) = open(cx, SNAPSHOT, Preferences::default());
    assert!(bounds(cx, "sidebar-outside").is_none());
}

#[gpui::test]
fn a_lane_with_nothing_done_has_no_progress_line(cx: &mut TestAppContext) {
    let (_rig, _desk, cx) = open(cx, SNAPSHOT, Preferences::default());
    assert!(bounds(cx, "row-demo-bar").is_some(), "one of four is done");
    assert!(
        bounds(cx, "row-measure-help-bar").is_none(),
        "none of one is done: an empty line reads as a divider"
    );
}

#[test]
fn a_project_shows_an_open_chevron_only_when_it_has_lanes_to_show() {
    use super::sidebar_projects::project_chevron;
    use gpui_kit_assets::IconName;
    assert_eq!(project_chevron(true, true), Some(IconName::ChevronDown));
    assert_eq!(project_chevron(false, true), Some(IconName::ChevronRight));
    assert_eq!(project_chevron(true, false), None);
    assert_eq!(project_chevron(false, false), None);
}

#[gpui::test]
fn cmd_click_or_middle_click_on_a_lane_opens_a_second_tab_on_it_and_keeps_the_first(
    cx: &mut TestAppContext,
) {
    let (_rig, desk, cx) = open(cx, SNAPSHOT, Preferences::default());
    let lane = Page::Lane(engine_rig_tests::lane_key("demo"));
    let at = bounds(cx, "row-demo").expect("the lane").center();
    cx.simulate_click(at, Modifiers::command());
    assert_eq!(pages(&desk, cx), [Page::Inbox, lane.clone()]);
    // A middle click does the same on another lane.
    let other = Page::Lane(engine_rig_tests::lane_key("stuck"));
    let at = bounds(cx, "row-stuck").expect("the lane").center();
    cx.simulate_mouse_down(at, gpui::MouseButton::Middle, Modifiers::none());
    cx.simulate_mouse_up(at, gpui::MouseButton::Middle, Modifiers::none());
    assert_eq!(pages(&desk, cx), [Page::Inbox, lane, other]);
}

#[gpui::test]
fn the_plus_is_beside_the_tabs_and_closing_the_last_tab_leaves_the_inbox(cx: &mut TestAppContext) {
    let (_rig, desk, cx) = open(cx, SNAPSHOT, Preferences::default());
    assert!(
        bounds(cx, "bar-new-tab").is_some(),
        "a tab is made by the + as well as by Cmd-click"
    );
    desk.update(cx, |desk, cx| desk.navigate(Page::Engine, false, cx));
    assert_eq!(pages(&desk, cx), [Page::Engine]);
    desk.update(cx, |desk, cx| {
        desk.bar.update(cx, |bar, cx| {
            let only = bar.tabs().active().id();
            bar.close(only, cx);
        })
    });
    assert_eq!(
        pages(&desk, cx),
        [Page::Inbox],
        "the last tab goes back to the Inbox"
    );
}

#[gpui::test]
fn reaudit_sidebar_short_titles_do_not_show_tooltips(cx: &mut TestAppContext) {
    let (_, _, window) = open(cx, SNAPSHOT, Preferences::default());
    let row = bounds(window, "name-demo").unwrap();
    window.simulate_mouse_move(row.center(), None, Modifiers::none());
    window
        .executor()
        .advance_clock(std::time::Duration::from_millis(600));
    window.run_until_parked();
    window
        .executor()
        .advance_clock(std::time::Duration::from_millis(300));
    window.update(|window, cx| {
        window.draw(cx).clear(cx);
        let fill = crate::theme::Theme::palette(cx).overlay_surface;
        assert!(
            !window.painted_quads().iter().any(|quad| {
                quad.background.as_solid() == Some(fill)
                    && quad.bounds.top() >= row.top().scale(window.scale_factor())
            }),
            "an untruncated name opened a tooltip"
        );
    });
}
