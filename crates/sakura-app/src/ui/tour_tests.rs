use super::engine_desk_tests;
use super::engine_rig_tests::{wait_until, SNAPSHOT};
use gpui::{Modifiers, TestAppContext, VisualTestContext};

fn press(cx: &mut VisualTestContext, selector: &'static str) {
    let at = cx.debug_bounds(selector).expect(selector).center();
    cx.simulate_click(at, Modifiers::none());
}

fn step(cx: &mut VisualTestContext, name: &'static str, targets: &[&'static str]) {
    wait_until(cx, name, |cx| {
        cx.debug_bounds(name).is_some() && targets.iter().all(|id| cx.debug_bounds(id).is_some())
    });
    let callout = cx.debug_bounds(name).unwrap();
    for id in targets {
        let target = cx.debug_bounds(id).unwrap();
        assert!(!callout.intersects(&target), "{name} covers {id}");
    }
    assert!(cx.debug_bounds("tour-skip").is_some());
}

#[gpui::test]
fn the_spotlight_stays_on_new_lane_when_the_window_resizes(cx: &mut TestAppContext) {
    let (_rig, _desk, cx) = engine_desk_tests::open(cx, SNAPSHOT);
    press(cx, "status-tour");
    press(cx, "tour-next");
    step(cx, "tour-lanes", &["lanes-new-lane"]);
    let initial = cx.debug_bounds("tour-lanes").unwrap();
    cx.simulate_resize(gpui::size(gpui::px(1160.), gpui::px(720.)));
    let target = cx.debug_bounds("lanes-new-lane").unwrap();
    let spotlight = cx.debug_bounds("tour-spotlight").expect("spotlight");
    let callout = cx.debug_bounds("tour-lanes").unwrap();
    assert_eq!(spotlight, target);
    assert!(!callout.intersects(&spotlight));
    assert_ne!(initial, callout);
    assert!(callout.right() < gpui::px(1160.));
}

#[gpui::test]
fn the_tour_starts_at_the_composer_with_the_sidebar_hidden(cx: &mut TestAppContext) {
    let (_rig, desk, cx) = engine_desk_tests::open(cx, SNAPSHOT);
    desk.update(cx, |desk, cx| desk.toggle_sidebar(cx));
    wait_until(cx, "sidebar hidden", |cx| {
        cx.debug_bounds("sidebar-master").is_none()
    });
    cx.update(|window, cx| {
        window.dispatch_action(gpui::Action::boxed_clone(&crate::menus::ShowTour), cx);
    });
    step(cx, "tour-master", &["master-agent-chip"]);
    press(cx, "tour-next");
    step(cx, "tour-lanes", &["lanes-new-lane"]);
    let background = cx.debug_bounds("lanes-new-lane").unwrap().center();
    cx.simulate_click(background, Modifiers::none());
    assert!(cx.debug_bounds("new-lane-sheet").is_none());
    assert!(cx.debug_bounds("tour-lanes").is_some());
}

#[gpui::test]
fn the_info_button_opens_five_steps_beside_the_controls_and_escape_skips(cx: &mut TestAppContext) {
    let (_rig, _desk, cx) = engine_desk_tests::open(cx, SNAPSHOT);
    let settings = cx.debug_bounds("status-settings").unwrap();
    let info = cx
        .debug_bounds("status-tour")
        .expect("status bar tour button");
    assert_eq!(settings.size, info.size);
    assert_eq!(
        info.right(),
        cx.debug_bounds("status-bar").unwrap().right() - crate::theme::engine_bar::BAR_PADDING_X
    );
    press(cx, "status-tour");
    step(cx, "tour-master", &["master-agent-chip"]);
    assert_eq!(
        cx.debug_bounds("tour-spotlight"),
        cx.debug_bounds("master-agent-chip")
    );
    press(cx, "tour-next");
    step(cx, "tour-lanes", &["lanes-new-lane"]);
    press(cx, "tour-next");
    step(cx, "tour-inbox", &["inbox-view-0", "inbox-view-1"]);
    press(cx, "tour-next");
    step(cx, "tour-engine", &["sidebar-engine"]);
    press(cx, "tour-next");
    step(cx, "tour-connectors", &["settings-section-connectors"]);
    cx.simulate_keystrokes("escape");
    wait_until(cx, "tour skipped", |cx| {
        cx.debug_bounds("tour-connectors").is_none()
    });
}

#[gpui::test]
fn the_tour_can_be_skipped_and_reopened_at_master(cx: &mut TestAppContext) {
    let (_rig, _desk, cx) = engine_desk_tests::open(cx, SNAPSHOT);
    press(cx, "status-tour");
    step(cx, "tour-master", &["master-agent-chip"]);
    press(cx, "tour-skip");
    wait_until(cx, "tour skipped", |cx| {
        cx.debug_bounds("tour-master").is_none()
    });
    press(cx, "status-tour");
    step(cx, "tour-master", &["master-agent-chip"]);
}

#[gpui::test]
fn the_help_menu_command_reopens_the_tour(cx: &mut TestAppContext) {
    let (_rig, _desk, cx) = engine_desk_tests::open(cx, SNAPSHOT);
    cx.update(|window, cx| {
        crate::menus::rebuild(cx);
        let menus = cx.get_menus().expect("menu bar");
        let help = menus.iter().find(|menu| menu.name == "Help").expect("Help");
        let action = help
            .items
            .iter()
            .find_map(|item| match item {
                gpui::OwnedMenuItem::Action { name, action, .. } if name == "Show the tour" => {
                    Some(action.boxed_clone())
                }
                _ => None,
            })
            .expect("Show the tour");
        window.dispatch_action(action, cx);
    });
    step(cx, "tour-master", &["master-agent-chip"]);
}

#[gpui::test]
fn approving_setup_opens_the_tour_once(cx: &mut TestAppContext) {
    let (rig, desk, cx) = engine_desk_tests::open_unread(cx, SNAPSHOT);
    std::fs::write(
        rig.dir.path().join("setup-detect.json"),
        r#"{"harnesses":[{"id":"codex","found":true,"program_path":null,"version":null}],"logins":[]}"#,
    ).unwrap();
    rig.say_first(r#"{"result":"done","exit_code":0,"output":"{\"accounts\":[]}"}"#);
    desk.update_in(cx, |desk, window, cx| desk.offer_first_launch(window, cx));
    wait_until(cx, "ask", |cx| cx.debug_bounds("setup-look").is_some());
    press(cx, "setup-look");
    wait_until(cx, "review", |cx| {
        cx.debug_bounds("setup-confirm").is_some()
    });
    press(cx, "setup-confirm");
    step(cx, "tour-master", &["master-agent-chip"]);
    press(cx, "tour-skip");
    wait_until(cx, "skipped", |cx| cx.debug_bounds("tour-master").is_none());
    desk.update_in(cx, |desk, window, cx| desk.offer_first_launch(window, cx));
    assert!(cx.debug_bounds("setup-look").is_none());
    assert!(cx.debug_bounds("tour-master").is_none());
}
