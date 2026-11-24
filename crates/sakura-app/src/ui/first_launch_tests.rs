use super::engine_desk::Desk;
use super::engine_desk_tests;
use super::engine_rig_tests::{wait_until, Rig};
use super::first_launch::offer;
use super::settings_modal::Section;
use crate::prefs::Preferences;
use gpui::{Entity, Modifiers, TestAppContext, VisualTestContext};

const MIXED: &str = include_str!("../../tests/fixtures/setup-detect.json");
const EMPTY: &str = include_str!("../../tests/fixtures/setup-detect-empty.json");
const NO_PROJECTS: &str = r#"{"api":1,"waits":[],"items":[],"projects":[]}"#;

fn say_detection(rig: &Rig, fixture: &str) {
    let home = std::env::var("HOME").unwrap();
    let fixture = fixture
        .replace("/fixture/.codex\"", &format!("{home}/.codex\""))
        .replace("/fixture/.claude\"", &format!("{home}/.claude\""))
        .replace(
            "/fixture/.config/goose\"",
            &format!("{home}/.config/goose\""),
        );
    std::fs::write(rig.dir.path().join("setup-detect.json"), fixture).unwrap();
}

fn click(cx: &mut VisualTestContext, selector: &'static str) {
    let at = cx.debug_bounds(selector).expect(selector).center();
    cx.simulate_click(at, Modifiers::none());
}

fn show_review<'a>(cx: &'a mut TestAppContext, fixture: &str) -> (Rig, &'a mut VisualTestContext) {
    let (rig, desk, cx) = engine_desk_tests::open_unread(cx, NO_PROJECTS);
    say_detection(&rig, fixture);
    look_again(&rig, &desk, cx);
    (rig, cx)
}

#[gpui::test]
fn review_groups_programs_accounts_and_defaults(cx: &mut TestAppContext) {
    let (_rig, cx) = show_review(cx, MIXED);
    let bounds = |cx: &mut VisualTestContext, selector| cx.debug_bounds(selector).expect(selector);
    assert!(bounds(cx, "setup-title").top() < bounds(cx, "setup-intro").top());
    let programs = bounds(cx, "setup-programs-title");
    let accounts = bounds(cx, "setup-accounts-title");
    let defaults = bounds(cx, "setup-defaults-title");
    let program = bounds(cx, "setup-program-codex");
    let check = bounds(cx, "setup-login-0-checked");
    let name = bounds(cx, "setup-login-0-name");
    let reveal = bounds(cx, "setup-login-0-location-icon");
    assert!(programs.top() < program.top() && program.bottom() < accounts.top());
    assert!(accounts.top() < check.top() && check.bottom() < defaults.top());
    assert!(program.size.height < bounds(cx, "setup-login-0").size.height);
    assert!(check.right() <= name.left() && name.right() <= reveal.left());
    assert!(cx.debug_bounds("setup-login-0-static-name").is_none());
    assert!(cx.debug_bounds("setup-login-1-registered").is_some());
    assert!(cx.debug_bounds("setup-login-1-location-reveal").is_none());
    click(cx, "setup-login-0-checked");
    assert_eq!(cx.debug_bounds("setup-login-0-name"), Some(name));
    click(cx, "setup-login-0-location-reveal");
    assert!(cx.debug_bounds("setup-login-0-location-value").is_some());
}

fn look_again(rig: &Rig, desk: &Entity<Desk>, cx: &mut VisualTestContext) {
    cx.simulate_resize(gpui::size(gpui::px(1280.), gpui::px(800.)));
    desk.update_in(cx, |desk, window, cx| {
        desk.open_settings_at(Section::Accounts, window, cx);
    });
    // The first snapshot can add account rows above Look again between mouse down and mouse up.
    wait_until(cx, "Accounts settings with the first snapshot", |cx| {
        rig.facts
            .read_with(cx, |facts, _| facts.snapshot().is_some())
            && cx.debug_bounds("setup-look-again").is_some()
    });
    let before = cx.debug_bounds("setup-look-again");
    click(cx, "setup-look-again");
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("first-launch").is_some(),
        "Look again did not open setup: button was {before:?}, now {:?}",
        cx.debug_bounds("setup-look-again")
    );
    wait_until(cx, "setup search to finish", |cx| {
        cx.debug_bounds("setup-confirm").is_some() || cx.debug_bounds("setup-error").is_some()
    });
}

#[gpui::test]
fn master_prefers_default_claude_then_first_selected_claude_then_codex(cx: &mut TestAppContext) {
    let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap());
    let mut fixture: serde_json::Value = serde_json::from_str(MIXED).unwrap();
    let logins = fixture["logins"].as_array_mut().unwrap();
    logins[1]["folder"] = serde_json::json!(home.join(".claude"));
    logins[1]["in_book_as"] = serde_json::Value::Null;
    logins.swap(1, 2);
    let (_rig, cx) = show_review(cx, &fixture.to_string());
    assert!(cx.debug_bounds("setup-master-2").is_some());
    click(cx, "setup-login-1-unchecked");
    click(cx, "setup-login-2-checked");
    assert!(cx.debug_bounds("setup-master-1").is_some());
    click(cx, "setup-login-1-checked");
    assert!(cx.debug_bounds("setup-master-0").is_some());
    click(cx, "setup-login-0-checked");
    assert!(cx.debug_bounds("setup-master-unavailable").is_some());
    click(cx, "setup-login-1-unchecked");
    assert!(cx.debug_bounds("setup-master-1").is_some());
    click(cx, "setup-login-2-unchecked");
    assert!(cx.debug_bounds("setup-master-2").is_some());
}

#[gpui::test]
fn a_logged_out_or_unverified_login_starts_unchecked(cx: &mut TestAppContext) {
    let mut fixture: serde_json::Value = serde_json::from_str(MIXED).unwrap();
    let logins = fixture["logins"].as_array_mut().unwrap();
    let mut unknown = logins[3].clone();
    unknown["folder"] = serde_json::json!("/fixture/.claude-unverified");
    unknown["logged_in"] = serde_json::Value::Null;
    logins.insert(4, unknown.clone());
    unknown["folder"] = serde_json::json!("/fixture/.claude-not-checked");
    unknown.as_object_mut().unwrap().remove("logged_in");
    logins.insert(5, unknown);
    let (_rig, cx) = show_review(cx, &fixture.to_string());
    for (sign_in, checked) in [
        ("setup-login-3-sign-in", "setup-login-3-checked"),
        ("setup-login-4-sign-in", "setup-login-4-checked"),
        ("setup-login-5-sign-in", "setup-login-5-checked"),
    ] {
        assert!(cx.debug_bounds(sign_in).is_some(), "{sign_in}");
        assert!(cx.debug_bounds(checked).is_none(), "{checked}");
    }
}

#[gpui::test]
fn a_failed_login_probe_shows_its_status_and_reveals_its_location_on_request(
    cx: &mut TestAppContext,
) {
    let (_rig, cx) = show_review(cx, MIXED);
    assert!(cx.debug_bounds("setup-login-4-probe-error").is_some());
    assert!(cx.debug_bounds("setup-login-4-note").is_some());
    assert!(cx.debug_bounds("setup-login-4-sign-in").is_none());
    assert!(cx.debug_bounds("setup-login-4-location-value").is_none());
    click(cx, "setup-login-4-location-reveal");
    assert!(cx.debug_bounds("setup-login-4-location-value").is_some());
    click(cx, "setup-login-4-location-reveal");
    click(cx, "setup-login-4-unchecked");
    assert!(cx.debug_bounds("setup-login-4-unchecked").is_some());
    assert!(cx.debug_bounds("setup-login-4-checked").is_none());
}

#[gpui::test]
fn review_scrolls_its_rows_while_both_buttons_stay_inside_the_window(cx: &mut TestAppContext) {
    let mut fixture: serde_json::Value = serde_json::from_str(MIXED).unwrap();
    let logins = fixture["logins"].as_array_mut().unwrap();
    let first = logins[3].clone();
    logins.extend((0..16).map(|at| {
        let mut login = first.clone();
        login["folder"] = serde_json::json!(format!("/fixture/.claude-{at}"));
        login
    }));
    let (_rig, cx) = show_review(cx, &fixture.to_string());
    let panel = cx.debug_bounds("first-launch").unwrap();
    let later = cx.debug_bounds("setup-later").unwrap();
    let confirm = cx.debug_bounds("setup-confirm").unwrap();
    assert!(panel.bottom() <= gpui::px(800.));
    assert!(later.bottom() <= panel.bottom());
    assert!(confirm.bottom() <= panel.bottom());
    let row = cx.debug_bounds("setup-program-codex").unwrap();
    cx.simulate_event(gpui::ScrollEvent {
        position: row.center(),
        delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(-600.))),
        ..Default::default()
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(cx.debug_bounds("setup-program-codex").unwrap().top() < row.top());
    assert_eq!(cx.debug_bounds("setup-later"), Some(later));
    assert_eq!(cx.debug_bounds("setup-confirm"), Some(confirm));
    click(cx, "setup-later");
    wait_until(cx, "review closed", |cx| {
        cx.debug_bounds("first-launch").is_none() && cx.debug_bounds("settings-modal").is_some()
    });
}

#[gpui::test]
fn accounts_settings_can_repeat_the_search_and_only_lists_supported_programs(
    cx: &mut TestAppContext,
) {
    let (rig, desk, cx) = engine_desk_tests::open_unread(cx, NO_PROJECTS);
    say_detection(&rig, MIXED);
    desk.update_in(cx, |desk, window, cx| {
        desk.open_settings_at(Section::Accounts, window, cx);
    });
    wait_until(cx, "Accounts settings", |cx| {
        cx.debug_bounds("settings-modal").is_some()
    });
    click(cx, "setup-look-again");
    wait_until(cx, "detected logins", |cx| {
        cx.debug_bounds("setup-confirm").is_some()
    });
    assert!(cx.debug_bounds("setup-program-claude").is_some());
    assert!(cx.debug_bounds("setup-program-codex").is_some());
    assert!(cx.debug_bounds("setup-program-unsupported").is_none());
    assert!(cx.debug_bounds("setup-login-2-unchecked").is_some());
    assert!(cx.debug_bounds("setup-login-3-sign-in").is_some());
    assert!(cx.debug_bounds("setup-master-1").is_some());
    assert_eq!(rig.acts(), ["setup detect --json"]);
    click(cx, "setup-login-3-sign-in");
    wait_until(cx, "login to the detected folder", |cx| {
        cx.debug_bounds("account-add-open").is_some()
            && cx.debug_bounds("account-add-plan").is_some()
    });
    assert_eq!(rig.acts(), ["setup detect --json"]);
}

#[gpui::test]
fn a_search_with_nothing_found_offers_two_login_buttons(cx: &mut TestAppContext) {
    let (rig, desk, cx) = engine_desk_tests::open_unread(cx, NO_PROJECTS);
    say_detection(&rig, EMPTY);
    desk.update_in(cx, |desk, window, cx| {
        desk.open_settings_at(Section::Accounts, window, cx);
    });
    wait_until(cx, "Accounts settings", |cx| {
        cx.debug_bounds("settings-modal").is_some()
    });
    click(cx, "setup-look-again");
    wait_until(cx, "empty results", |cx| {
        cx.debug_bounds("setup-empty").is_some()
    });
    assert!(cx.debug_bounds("setup-sign-in-claude").is_some());
    assert!(cx.debug_bounds("setup-sign-in-codex").is_some());
    assert!(cx.debug_bounds("setup-confirm").is_none());
    assert_eq!(rig.acts(), ["setup detect --json"]);
}

#[gpui::test]
fn a_fresh_desk_with_no_projects_offers_to_add_an_account_and_a_project(cx: &mut TestAppContext) {
    let (_rig, _desk, cx) = engine_desk_tests::open_unread(cx, NO_PROJECTS);
    wait_until(cx, "empty Inbox", |cx| {
        cx.debug_bounds("waiting-add-project").is_some()
    });
    assert!(cx.debug_bounds("waiting-add-account").is_some());
}

#[gpui::test]
fn legacy_preferences_skip_the_first_launch_ask(cx: &mut TestAppContext) {
    let (rig, _desk, cx) = engine_desk_tests::open_unread(cx, NO_PROJECTS);
    let path = rig.dir.path().join("preferences.json");
    std::fs::write(
        &path,
        r#"{"lastProject":"Existing project","sidebarHidden":true}"#,
    )
    .unwrap();
    cx.update(|window, cx| {
        Preferences::load_from(&path).install(Some(path), cx);
        assert!(offer(rig.source.clone(), rig.facts.clone(), window, cx).is_none());
    });
    assert!(rig.acts().is_empty());
}

#[gpui::test]
fn quitting_before_answering_first_launch_leaves_the_ask_for_the_next_launch(
    cx: &mut TestAppContext,
) {
    let (rig, _desk, cx) = engine_desk_tests::open_unread(cx, NO_PROJECTS);
    let path = rig.dir.path().join("preferences.json");
    cx.update(|window, cx| {
        Preferences::load_from(&path).install(Some(path.clone()), cx);
        assert!(offer(rig.source.clone(), rig.facts.clone(), window, cx).is_some());
        window.draw(cx).clear(cx);
    });
    assert!(cx.debug_bounds("setup-ask").is_some());
    cx.update(|_, cx| Preferences::change(cx, |prefs| prefs.is_sidebar_hidden = true));
    wait_until(cx, "preferences saved while setup is open", |_| {
        path.exists()
    });

    let saved = Preferences::load_from(&path);
    assert!(saved.is_sidebar_hidden);
    cx.update(|window, cx| {
        saved.install(Some(path), cx);
        assert!(offer(rig.source.clone(), rig.facts.clone(), window, cx).is_some());
    });
    assert!(rig.acts().is_empty());
}

#[gpui::test]
fn first_launch_asks_before_searching_and_decline_survives_a_restart(cx: &mut TestAppContext) {
    let (rig, _desk, cx) = engine_desk_tests::open_unread(cx, NO_PROJECTS);
    let path = rig.dir.path().join("preferences.json");
    assert!(!path.exists());
    cx.update(|window, cx| {
        Preferences::load_from(&path).install(Some(path.clone()), cx);
        assert!(offer(rig.source.clone(), rig.facts.clone(), window, cx).is_some());
        window.draw(cx).clear(cx);
    });
    assert!(cx.debug_bounds("setup-ask").is_some());
    assert!(cx.debug_bounds("setup-look").is_some());
    assert!(rig.acts().is_empty());
    click(cx, "setup-not-now");
    wait_until(cx, "welcome and saved decline", |cx| {
        Preferences::load_from(&path).is_setup_done
            && cx.debug_bounds("first-launch").is_none()
            && cx.debug_bounds("waiting-add-account").is_some()
            && cx.debug_bounds("waiting-add-project").is_some()
    });
    cx.update(|window, cx| {
        Preferences::load_from(&path).install(Some(path.clone()), cx);
        assert!(offer(rig.source.clone(), rig.facts.clone(), window, cx).is_none());
    });
    assert!(rig.acts().is_empty());
}

#[gpui::test]
fn look_again_keeps_unchanged_engine_defaults(cx: &mut TestAppContext) {
    let snapshot = r#"{"api":1,"waits":[],"items":[],"projects":[],
        "engine":{"preset":"lean","pace":"fast"},"accounts":[
        {"provider":"claude","id":"claude-work-id","label":"Work"}]}"#;
    let (rig, desk, cx) = engine_desk_tests::open_unread(cx, snapshot);
    let path = rig.dir.path().join("preferences.json");
    std::fs::write(&path, r#"{"lastProject":"Existing project"}"#).unwrap();
    cx.update(|_, cx| Preferences::load_from(&path).install(Some(path), cx));
    say_detection(&rig, MIXED);
    rig.say_first(include_str!("../../tests/fixtures/setup-apply.json"));
    look_again(&rig, &desk, cx);
    click(cx, "setup-login-2-unchecked");
    click(cx, "setup-confirm");
    wait_until(cx, "completed setup", |cx| {
        assert!(cx.debug_bounds("setup-retry").is_none());
        cx.debug_bounds("first-launch").is_none()
    });
    let acts = rig.acts();
    assert_eq!(acts.len(), 2);
    let (choices, _) = setup_choices_argument(&acts[1]);
    let choices: serde_json::Value = serde_json::from_str(choices).unwrap();
    assert_eq!(choices["accounts"].as_array().unwrap().len(), 2);
    assert!(choices.get("preset").is_none(), "{choices}");
    assert!(choices.get("pace").is_none(), "{choices}");
}

#[gpui::test]
fn confirm_applies_once_and_saves_the_master_account_id(cx: &mut TestAppContext) {
    let snapshot = r#"{"api":1,"waits":[],"items":[],"accounts":[
        {"provider":"claude","id":"claude-work-id","label":"Work"}]}"#;
    let (rig, _desk, cx) = engine_desk_tests::open_unread(cx, snapshot);
    say_detection(&rig, MIXED);
    rig.say_first(include_str!("../../tests/fixtures/setup-apply.json"));
    let path = rig.dir.path().join("preferences.json");
    cx.update(|window, cx| {
        Preferences::default().install(Some(path.clone()), cx);
        offer(rig.source.clone(), rig.facts.clone(), window, cx);
        window.draw(cx).clear(cx);
    });
    click(cx, "setup-look");
    wait_until(cx, "confirm", |cx| {
        cx.debug_bounds("setup-confirm").is_some()
    });
    click(cx, "setup-login-2-unchecked");
    click(cx, "setup-confirm");
    wait_until(cx, "submitted setup", |_| rig.acts().len() == 2);
    let acts = rig.acts();
    assert_eq!(acts.len(), 2);
    assert_eq!(acts[0], "setup detect --json");
    let (choices, rest) = setup_choices_argument(&acts[1]);
    assert_eq!(rest.split_whitespace().count(), 4);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(choices).unwrap(),
        serde_json::json!({
            "accounts": [
                {"harness":"codex","folder":std::path::PathBuf::from(std::env::var_os("HOME").unwrap()).join(".codex"),"name":"Codex"},
                {"harness":"claude","folder":"/fixture/.claude-alt","name":"Claude Alt"}
            ],
            "preset":"standard",
            "pace":"normal"
        })
    );
    wait_until(cx, "saved setup", |cx| {
        assert!(cx.debug_bounds("setup-retry").is_none());
        cx.debug_bounds("first-launch").is_none() && Preferences::load_from(&path).is_setup_done
    });
    let preferences = Preferences::load_from(&path);
    assert_eq!(
        preferences.new_master_account.as_deref(),
        Some("claude-work-id")
    );
    assert!(preferences.pending_setup.is_none());
}

fn setup_choices_argument(command: &str) -> (&str, &str) {
    command
        .strip_prefix(r#"cockpit act setupapply ["host","engine"] --choices "#)
        .unwrap_or_else(|| panic!("unexpected setup command: {command}"))
        .split_once(" --op ")
        .expect("operation id")
}

#[gpui::test]
fn resumed_setup_uses_the_book_label_for_the_saved_master_folder(cx: &mut TestAppContext) {
    let snapshot = r#"{"api":1,"waits":[],"items":[],"accounts":[
        {"id":"codex-id","provider":"codex","label":"Codex"},
        {"id":"claude-team-id","provider":"claude","label":"Team Claude"}]}"#;
    let (rig, _desk, cx) = engine_desk_tests::open_unread(cx, snapshot);
    rig.say_first(include_str!("../../tests/fixtures/setup-apply.json"));
    let path = rig.dir.path().join("preferences.json");
    cx.update(|window, cx| {
        assert!(Preferences {
            pending_setup: Some(serde_json::json!({
                "op": "already-approved",
                "choices": {
                    "accounts": [
                        {"harness":"codex","folder":"/fixture/.codex","name":"Codex"},
                        {"harness":"claude","folder":"/fixture/.claude-alt","name":"Claude Alt"}
                    ],
                    "preset":"standard",
                    "pace":"normal"
                },
                "master": "/fixture/.claude-alt"
            })),
            ..Preferences::default()
        }
        .save_to(&path));
        Preferences::load_from(&path).install(Some(path.clone()), cx);
        offer(rig.source.clone(), rig.facts.clone(), window, cx);
        window.draw(cx).clear(cx);
    });
    wait_until(cx, "resumed setup", |cx| {
        assert!(cx.debug_bounds("setup-retry").is_none());
        Preferences::load_from(&path).pending_setup.is_none()
    });
    cx.update(|_, cx| {
        assert_eq!(
            Preferences::current(cx).new_master_account.as_deref(),
            Some("claude-team-id")
        );
    });
    assert_eq!(rig.acts().len(), 1);
    assert!(rig.acts()[0].contains("--op already-approved "));
}

#[gpui::test]
fn fix36_look_again_keeps_its_position_when_accounts_first_arrive(cx: &mut TestAppContext) {
    let snapshot = crate::ui::engine_fixture_tests::full();
    let rig = super::engine_rig_tests::open_with_script(
        cx,
        (&snapshot, "{}", "{}"),
        r#"d="$(dirname "$0")"
if [ "$2" = snapshot ]; then
    for i in $(seq 1 100); do
        if [ -f "$d/release" ]; then cat "$d/snapshot.json"; exit; fi
        sleep 0.05
    done
fi
exit 1"#,
    );
    let (rig, desk, cx) = engine_desk_tests::open_rig(cx, rig);
    cx.simulate_resize(gpui::size(gpui::px(1280.), gpui::px(800.)));
    desk.update_in(cx, |desk, window, cx| {
        desk.open_settings_at(Section::Accounts, window, cx);
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let before = cx
        .debug_bounds("setup-look-again")
        .expect("Look again while loading");
    assert!(rig
        .facts
        .read_with(cx, |facts, _| facts.snapshot().is_none()));
    std::fs::write(rig.dir.path().join("release"), "").unwrap();
    wait_until(cx, "Accounts from the first snapshot", |cx| {
        rig.facts
            .read_with(cx, |facts, _| facts.snapshot().is_some())
    });
    assert_eq!(cx.debug_bounds("setup-look-again"), Some(before));
}

#[gpui::test]
fn polish_x_dialog_goose_configures_only_on_the_account_row(cx: &mut TestAppContext) {
    let mut fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../sakura-core/tests/fixtures/engine-real-goose-setup.json"
    ))
    .unwrap();
    let rows = fixture["harnesses"].as_array_mut().unwrap();
    rows.push(serde_json::json!({"id":"unsupported", "found":true, "readiness":"unknown"}));
    fixture["logins"][0]["folder"] = serde_json::json!(std::path::PathBuf::from(
        std::env::var_os("HOME").unwrap()
    )
    .join(".config/goose"));
    fixture["logins"][0]["readiness"] = "unknown".into();
    fixture["logins"][0]["reason"] = "connection_required".into();
    let (_rig, cx) = show_review(cx, &fixture.to_string());
    for selector in [
        "setup-program-claude",
        "setup-program-codex",
        "setup-program-goose",
    ] {
        assert!(cx.debug_bounds(selector).is_some());
    }
    assert!(cx.debug_bounds("setup-program-unsupported").is_none());
    assert!(cx.debug_bounds("setup-login-0-checked").is_some());
    let name = cx.debug_bounds("setup-login-0-name").unwrap();
    let note = cx.debug_bounds("setup-login-0-note").unwrap();
    let provider = cx.debug_bounds("setup-login-0-provider").unwrap();
    assert_eq!(
        (
            cx.debug_bounds("setup-program-goose-sign-in").is_some(),
            cx.debug_bounds("setup-login-0-configure").is_some(),
            provider.top() >= note.bottom(),
            provider.left() >= name.left(),
            provider.size.width >= crate::theme::DIALOG_WIDTH_SMALL / 2.,
        ),
        (false, true, true, true, true),
    );
}

#[gpui::test]
fn polish_y_setup_uses_aligned_accounts_and_an_inline_provider(cx: &mut TestAppContext) {
    let mut fixture: serde_json::Value = serde_json::from_str(MIXED).unwrap();
    let mut goose: serde_json::Value = serde_json::from_str(include_str!(
        "../../../sakura-core/tests/fixtures/engine-real-goose-setup.json"
    ))
    .unwrap();
    goose["logins"][0]["readiness"] = "unknown".into();
    goose["logins"][0]["reason"] = "connection_required".into();
    goose["logins"][0]["proposed_name"] = "Research".into();
    let logins = fixture["logins"].as_array_mut().unwrap();
    let codex = logins
        .iter()
        .find(|login| login["harness"] == "codex")
        .unwrap()
        .clone();
    *logins = vec![logins[2].clone(), codex, goose["logins"][0].clone()];
    let (_rig, cx) = show_review(cx, &fixture.to_string());
    for viewport in [
        gpui::size(
            crate::theme::WINDOW_START_WIDTH,
            crate::theme::WINDOW_START_HEIGHT,
        ),
        gpui::size(
            crate::theme::WINDOW_MIN_WIDTH,
            crate::theme::WINDOW_MIN_HEIGHT,
        ),
    ] {
        cx.simulate_resize(viewport);
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let at = |cx: &mut VisualTestContext, id: &'static str| cx.debug_bounds(id).expect(id);
        let name = at(cx, "setup-login-2-name");
        let configure = at(cx, "setup-login-2-configure");
        let note = at(cx, "setup-login-2-note");
        let provider = at(cx, "setup-login-2-provider");
        assert_eq!(name.center().y, configure.center().y);
        let label = at(cx, "setup-login-2-provider-label");
        assert_eq!(label.left(), name.left());
        assert_eq!(label.center().y, provider.center().y);
        assert!(label.right() <= provider.left() && label.top() >= note.bottom());
        assert!(provider.right() >= configure.right());
        assert!(note.left() > name.right());
        assert!((note.center().y - name.center().y).abs() <= gpui::px(1.));
        assert_eq!(at(cx, "setup-login-0-name").left(), name.left());
        let folder = at(cx, "setup-login-2-location-icon");
        assert_eq!(
            at(cx, "setup-login-0-location-icon").right(),
            folder.right()
        );
        assert_eq!(
            at(cx, "setup-login-1-location-icon").right(),
            folder.right()
        );
    }
}
#[gpui::test]
fn polish_y_setup_default_controls_align_and_scroll_inside_the_sheet(cx: &mut TestAppContext) {
    let fixture = crate::ui::engine_fixture_tests::full();
    let (rig, desk, cx) = engine_desk_tests::open_unread(cx, &fixture);
    say_detection(&rig, MIXED);
    look_again(&rig, &desk, cx);
    for viewport in [
        gpui::size(
            crate::theme::WINDOW_START_WIDTH,
            crate::theme::WINDOW_START_HEIGHT,
        ),
        gpui::size(
            crate::theme::WINDOW_MIN_WIDTH,
            crate::theme::WINDOW_MIN_HEIGHT,
        ),
    ] {
        cx.simulate_resize(viewport);
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let master = [
            "setup-master-0",
            "setup-master-1",
            "setup-master-2",
            "setup-master-3",
        ]
        .into_iter()
        .find_map(|selector| cx.debug_bounds(selector))
        .unwrap();
        let [preset, pace] = ["setup-preset", "setup-pace"]
            .map(|selector| cx.debug_bounds(selector).expect(selector));
        let selected = [master, preset, pace];
        assert!(selected
            .windows(2)
            .all(|pair| pair[0].size.width == pair[1].size.width));
        assert!(selected
            .windows(2)
            .all(|pair| pair[0].right() == pair[1].right()));
        let body = cx.debug_bounds("setup-review-list").unwrap();
        let footer = cx.debug_bounds("dialog-footer").unwrap();
        cx.simulate_event(gpui::ScrollEvent {
            position: body.center(),
            delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(-1200.))),
            ..Default::default()
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let last = cx.debug_bounds("setup-pace-row").unwrap();
        assert!(body.top() <= last.top() && last.bottom() <= body.bottom());
        assert_eq!(cx.debug_bounds("dialog-footer"), Some(footer));
    }
}

#[gpui::test]
fn audit_setup_body_shares_the_footer_right_edge(cx: &mut TestAppContext) {
    let (_rig, cx) = show_review(cx, MIXED);
    let footer = cx.debug_bounds("dialog-footer").unwrap();
    let account = cx.debug_bounds("setup-login-0").unwrap();
    assert_eq!(account.right(), footer.right());
}

#[gpui::test]
fn audit_setup_uses_the_installed_codex_without_an_acp_adapter(cx: &mut TestAppContext) {
    let (_rig, window) = show_review(cx, MIXED);
    let installed = window
        .debug_bounds("setup-program-codex-detail")
        .unwrap()
        .size
        .width;
    let mut fixture: serde_json::Value = serde_json::from_str(MIXED).unwrap();
    fixture["harnesses"][0]["readiness"] = "unavailable".into();
    fixture["harnesses"][0]["reason"] = "install_adapter".into();
    let (_rig, window) = show_review(cx, &fixture.to_string());
    let without_adapter = window
        .debug_bounds("setup-program-codex-detail")
        .unwrap()
        .size
        .width;
    assert_eq!(installed, without_adapter);
}

#[gpui::test]
fn audit_a_select_row_has_the_same_height_in_settings_and_setup(cx: &mut TestAppContext) {
    let (_, desk, window) = engine_desk_tests::open_unread(cx, NO_PROJECTS);
    desk.update_in(window, |desk, window, cx| {
        desk.open_settings_at(Section::General, window, cx);
    });
    wait_until(window, "language settings", |window| {
        window.debug_bounds("settings-row-language").is_some()
    });
    let language = window.debug_bounds("settings-row-language").unwrap();
    let (_, window) = show_review(cx, MIXED);
    let pace = window.debug_bounds("setup-pace-row").unwrap();
    assert_eq!(language.size.height, pace.size.height);
}

#[gpui::test]
fn discovery_checks_only_the_harness_default_and_review_accepts_escape(cx: &mut TestAppContext) {
    let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap());
    let mut fixture: serde_json::Value = serde_json::from_str(MIXED).unwrap();
    let logins = fixture["logins"].as_array_mut().unwrap();
    logins[0]["folder"] = serde_json::json!(home.join(".codex"));
    logins[1]["folder"] = serde_json::json!(home.join(".claude"));
    logins[1]["in_book_as"] = serde_json::Value::Null;
    logins[2]["folder"] = serde_json::json!(home.join(".claude-alt"));
    let (rig, cx) = show_review(cx, &fixture.to_string());
    assert!(cx.debug_bounds("setup-login-0-checked").is_some());
    assert!(cx.debug_bounds("setup-login-1-checked").is_some());
    assert!(cx.debug_bounds("setup-login-2-unchecked").is_some());
    click(cx, "setup-login-2-unchecked");
    assert!(cx.debug_bounds("setup-login-2-checked").is_some());
    click(cx, "setup-login-0-name");
    cx.simulate_keystrokes("escape");
    assert!(cx.debug_bounds("first-launch").is_none());
    assert_eq!(rig.acts(), ["setup detect --json"]);
}

#[gpui::test]
fn escape_closes_first_launch_after_look_for_them_without_focus_in_a_field(
    cx: &mut TestAppContext,
) {
    let (rig, _desk, cx) = engine_desk_tests::open_unread(cx, NO_PROJECTS);
    say_detection(&rig, MIXED);
    cx.update(|window, cx| {
        offer(rig.source.clone(), rig.facts.clone(), window, cx);
        window.draw(cx).clear(cx);
    });
    click(cx, "setup-look");
    wait_until(cx, "setup review", |cx| {
        cx.debug_bounds("setup-confirm").is_some()
    });
    cx.simulate_keystrokes("escape");
    assert!(cx.debug_bounds("first-launch").is_none());
    assert_eq!(rig.acts(), ["setup detect --json"]);
}
