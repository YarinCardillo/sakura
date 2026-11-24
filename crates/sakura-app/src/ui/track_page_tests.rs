use super::*;
use crate::ui::engine_desk_tests;
use crate::ui::engine_rig_tests::wait_until;
use gpui::{Modifiers, TestAppContext};

const TRACKS: &str = include_str!("../../../sakura-core/tests/fixtures/engine-real-tracks.json");
const UNAVAILABLE: &str =
    include_str!("../../../sakura-core/tests/fixtures/engine-real-tracks-unavailable.json");

#[gpui::test]
fn polish_x_track_page_uses_one_title_and_describes_closing(cx: &mut TestAppContext) {
    let snapshot = Snapshot::parse(TRACKS.as_bytes()).unwrap();
    let track = &snapshot.tracks.rows()[0];
    let (_rig, desk, cx) = engine_desk_tests::open_unread(cx, TRACKS);
    desk.update(cx, |desk, cx| {
        desk.navigate(Page::Track(track.id.clone()), false, cx)
    });
    wait_until(cx, "Close Track", |cx| {
        cx.debug_bounds("track.close").is_some()
    });
    let has_second_title = cx.debug_bounds("track-body-title").is_some();
    crate::ui::lane_tests::click(cx, "track.close");
    wait_until(cx, "the confirmation", |cx| {
        cx.debug_bounds("track-dialog-title").is_some()
    });
    assert_eq!(
        (
            has_second_title,
            cx.debug_bounds("track-dialog-description").is_some()
        ),
        (false, true)
    );
}

#[gpui::test]
fn polish_x_track_membership_respects_cleaned_lanes(cx: &mut TestAppContext) {
    let mut fixture: serde_json::Value = serde_json::from_str(TRACKS).unwrap();
    fixture["items"][0]["agent_state"] = "cleaned".into();
    fixture["items"][0]["actions"] = serde_json::json!([]);
    fixture["tracks"][0]["lanes"][0]["state"] = "finished".into();
    fixture["items"][1]["agent_state"] = "cleaned".into();
    fixture["items"][1]["actions"] = serde_json::json!([]);
    let fixture = fixture.to_string();
    let snapshot = Snapshot::parse(fixture.as_bytes()).unwrap();
    let (_rig, desk, cx) = engine_desk_tests::open_unread(cx, &fixture);
    desk.update(cx, |desk, cx| {
        desk.navigate(Page::Track(snapshot.tracks.rows()[0].id.clone()), false, cx)
    });
    wait_until(cx, "the Track lane", |cx| {
        cx.debug_bounds("track-lane-0").is_some()
    });
    let shows_closed = cx.debug_bounds("track-lane-0-status-Closed").is_some();
    desk.update(cx, |desk, cx| {
        desk.open_lane(&snapshot.items[1].lane_key(), cx)
    });
    wait_until(cx, "the closed lane", |cx| {
        cx.debug_bounds("lane-page").is_some()
    });
    assert_eq!(
        (shows_closed, cx.debug_bounds("lane-track").is_some()),
        (true, false)
    );
}

#[gpui::test]
fn a_track_row_supports_opening_in_a_new_tab_and_survives_a_read_failure(cx: &mut TestAppContext) {
    let snapshot = Snapshot::parse(TRACKS.as_bytes()).unwrap();
    let release = snapshot
        .tracks
        .rows()
        .iter()
        .find(|track| track.status == TrackStatus::Open)
        .unwrap();
    let page = Page::Track(release.id.clone());
    let selector: &'static str =
        Box::leak(format!("sidebar-track-{}", release.id).into_boxed_str());
    let (rig, desk, cx) = engine_desk_tests::open_unread(cx, TRACKS);
    wait_until(cx, "the Track row", |cx| {
        cx.debug_bounds(selector).is_some()
    });
    let at = cx.debug_bounds(selector).unwrap().center();
    cx.simulate_click(at, Modifiers::command());
    wait_until(cx, "the Track page", |cx| {
        cx.debug_bounds("track-page").is_some()
    });
    rig.say_snapshot(UNAVAILABLE);
    rig.facts.update(cx, |facts, cx| facts.read(cx));
    wait_until(cx, "the unavailable Track snapshot", |cx| {
        desk.read_with(cx, |desk, cx| {
            matches!(
                desk.facts
                    .read(cx)
                    .snapshot()
                    .map(|snapshot| &snapshot.tracks),
                Some(Tracks::Unavailable(_))
            )
        })
    });
    desk.read_with(cx, |desk, cx| {
        let tabs = desk.bar.read(cx).tabs();
        assert_eq!(tabs.tabs().len(), 2);
        assert_eq!(tabs.active().page(), &page);
    });
}

#[test]
fn changing_an_operation_target_or_draft_allocates_a_new_id() {
    let snapshot = Snapshot::parse(TRACKS.as_bytes()).unwrap();
    let action = snapshot.tracks.action("tracknew").unwrap();
    let mut operation = Operation::default();
    let draft =
        |name: &str, target: &str| request(action, target.into(), [("name".into(), name.into())]);
    let original = operation.prepare(draft("Beta", COLLECTION));
    assert_eq!(original, operation.prepare(draft("Beta", COLLECTION)));
    let changed = operation.prepare(draft("Release", COLLECTION));
    assert_ne!(original.inputs["op"], changed.inputs["op"]);
    let moved = operation.prepare(draft("Release", r#"["track","t-01234567"]"#));
    assert_ne!(changed.inputs["op"], moved.inputs["op"]);
}

#[gpui::test]
fn creation_is_offered_with_zero_projects_only_when_advertised(cx: &mut TestAppContext) {
    let empty = r#"{"api":1,"tracks":[],"track_actions":[{"key":"tracknew","label":"New Track","needs":["name","goal","op"],"optional":["project"]}]}"#;
    let (rig, desk, cx) = engine_desk_tests::open_unread(cx, empty);
    wait_until(cx, "New Track", |cx| cx.debug_bounds("new-track").is_some());
    assert!(desk.read_with(cx, |desk, cx| desk.situation(cx).can_create_track));
    let at = cx.debug_bounds("new-track").unwrap().center();
    cx.simulate_click(at, Modifiers::none());
    wait_until(cx, "the creation dialog", |cx| {
        cx.debug_bounds("track-dialog").is_some()
    });

    cx.simulate_input("Ignored before focus");
    crate::ui::lane_tests::click(cx, "track-field-name");
    cx.update(|window, _| assert!(!window.last_input_was_keyboard()));
    cx.simulate_input("Release planning");
    cx.simulate_keystrokes("tab");
    cx.update(|window, _| assert!(window.last_input_was_keyboard()));
    cx.simulate_input("Ship the beta");
    crate::ui::lane_tests::click(cx, "track-save");
    wait_until(cx, "the creation request", |_| !rig.acts().is_empty());
    let call = rig.acts().join("\n");
    assert!(call.contains("--name=Release planning"), "{call}");
    assert!(call.contains("--goal=Ship the beta"), "{call}");
    assert!(
        !call.contains("Ignored before focus") && !call.contains("--project"),
        "{call}"
    );
}

#[test]
fn track_refusal_and_labels_are_localized() {
    use crate::l10n::{say, set_for_test, Language};
    let refusal = sakura_core::engine::tracks::refusal(Some("name_in_use"), None, "");
    for (language, title, problem) in [
        (
            Language::English,
            "New track",
            "An open or paused Track already has this name.",
        ),
        (
            Language::Italian,
            "Nuovo percorso",
            "Un percorso aperto o in pausa ha già questo nome.",
        ),
    ] {
        set_for_test(language);
        assert_eq!(tr(Key("track.new")), title);
        assert_eq!(say(&refusal).as_ref(), problem);
    }
}

#[gpui::test]
fn a_lane_started_from_a_track_keeps_the_track_and_op_in_argv(cx: &mut TestAppContext) {
    let snapshot = Snapshot::parse(TRACKS.as_bytes()).unwrap();
    let track = snapshot
        .tracks
        .rows()
        .iter()
        .find(|track| track.status == TrackStatus::Open)
        .unwrap();
    let (rig, desk, cx) = engine_desk_tests::open_unread(cx, TRACKS);
    rig.say_first(r#"{"result":"done","exit_code":0,"output":"add-search"}"#);
    let page = Page::Track(track.id.clone());
    desk.update(cx, |desk, cx| desk.navigate(page, false, cx));
    wait_until(cx, "a Track with an available project", |cx| {
        cx.debug_bounds("track-new-lane").is_some()
    });
    crate::ui::lane_tests::click(cx, "track-new-lane");
    wait_until(cx, "the Track picker", |cx| {
        cx.debug_bounds("new-lane-track").is_some()
    });
    crate::ui::lane_tests::click(cx, "new-lane-title");
    cx.simulate_input("Verify beta");
    crate::ui::lane_tests::click(cx, "new-lane-start");
    wait_until(cx, "the exact created lane", |cx| {
        desk.read_with(cx, |desk, cx| {
            desk.bar.read(cx).tabs().active().page()
                == &Page::Lane(r#"["flow","/work/orchard","add-search"]"#.into())
        })
    });
    let call = rig.acts().join("\n");
    assert!(call.contains(&format!("--track={}", track.id)), "{call}");
    assert!(call.contains("--op="), "{call}");
}

#[gpui::test]
fn reservation_retry_uses_the_saved_start_and_opens_the_returned_lane(cx: &mut TestAppContext) {
    let snapshot = Snapshot::parse(TRACKS.as_bytes()).unwrap();
    let track = snapshot
        .tracks
        .rows()
        .iter()
        .find(|track| track.status == TrackStatus::Open)
        .unwrap();
    let reservation = track.lanes.iter().find(|lane| lane.did_not_start).unwrap();
    let (rig, desk, cx) = engine_desk_tests::open_unread(cx, TRACKS);
    rig.say_first(r#"{"result":"done","exit_code":0,"output":"add-search"}"#);
    let page = Page::Track(track.id.clone());
    desk.update(cx, |desk, cx| desk.navigate(page, false, cx));
    wait_until(cx, "the reservation actions", |cx| {
        cx.debug_bounds("track-lane-2-action-0").is_some()
    });
    crate::ui::lane_tests::click(cx, "track-lane-2-action-0");
    wait_until(cx, "the Retry dialog", |cx| {
        cx.debug_bounds("track-save").is_some()
    });
    crate::ui::lane_tests::click(cx, "track-save");
    wait_until(cx, "the lane named by Retry", |cx| {
        desk.read_with(cx, |desk, cx| {
            desk.bar.read(cx).tabs().active().page()
                == &Page::Lane(r#"["flow","/work/orchard","add-search"]"#.into())
        })
    });
    let call = rig.acts().join("\n");
    assert!(call.contains("cockpit act trackretry"), "{call}");
    assert!(
        call.contains(&format!(
            "--startop={}",
            reservation.startop.as_deref().unwrap()
        )),
        "{call}"
    );
    assert!(call.contains("--op="), "{call}");
}

#[gpui::test]
fn conversation_links_resolve_only_the_exact_persisted_identity(cx: &mut TestAppContext) {
    let snapshot = Snapshot::parse(TRACKS.as_bytes()).unwrap();
    let track = snapshot
        .tracks
        .rows()
        .iter()
        .find(|track| !track.conversations.is_empty())
        .unwrap();
    let key = &track.conversations[0].key;
    let mut document: serde_json::Value = serde_json::from_str(TRACKS).unwrap();
    document["accounts"] =
        serde_json::json!([{"id": key.account, "provider": key.provider, "label": "Studio"}]);
    let (rig, desk, cx) = engine_desk_tests::open_unread(cx, &document.to_string());
    wait_until(cx, "the registered account", |cx| {
        desk.read_with(cx, |desk, cx| desk.facts.read(cx).snapshot().is_some())
    });
    let slot = desk.read_with(cx, |desk, cx| {
        desk.bar.read(cx).tabs().active().conversation().to_string()
    });
    let account = super::super::master_accounts::session_key(&key.account, &slot);
    rig.sessions.store("@master", &account, &key.session);
    desk.read_with(cx, |desk, cx| {
        assert!(desk.can_open_track_conversation(key, cx));
        let other_account = sakura_core::engine::tracks::ConversationKey {
            account: "other".into(),
            ..key.clone()
        };
        let other_provider = sakura_core::engine::tracks::ConversationKey {
            provider: "codex".into(),
            ..key.clone()
        };
        assert!(!desk.can_open_track_conversation(&other_account, cx));
        assert!(!desk.can_open_track_conversation(&other_provider, cx));
    });
}

#[gpui::test]
fn long_track_content_and_refusal_stay_inside_the_dialog_at_1280(cx: &mut TestAppContext) {
    let fixture = include_str!("../../fixtures/all-states.json");
    let snapshot = Snapshot::parse(fixture.as_bytes()).unwrap();
    let track = snapshot
        .tracks
        .rows()
        .iter()
        .find(|track| track.status == TrackStatus::Open)
        .unwrap();
    let (rig, desk, cx) = engine_desk_tests::open_unread(cx, fixture);
    let reason = serde_json::json!({
        "result": "refused", "reason": "The lane is open", "reason_code": "open_lanes",
        "reason_args": {"labels": [track.name.repeat(8)]}
    });
    rig.say_first(&reason.to_string());
    cx.simulate_resize(gpui::size(gpui::px(1280.), gpui::px(800.)));
    let page = Page::Track(track.id.clone());
    desk.update(cx, |desk, cx| desk.navigate(page, false, cx));
    wait_until(cx, "the Track controls", |cx| {
        cx.debug_bounds("track.close").is_some()
    });
    crate::ui::lane_tests::click(cx, "track.close");
    wait_until(cx, "the Close dialog", |cx| {
        cx.debug_bounds("track-form-feedback").is_some()
    });
    let before = cx.debug_bounds("track-dialog").unwrap();
    crate::ui::lane_tests::click(cx, "track-save");
    wait_until(cx, "the refusal", |cx| {
        !rig.acts().is_empty() && cx.debug_bounds("track-form-feedback").is_some()
    });
    let shown = cx.debug_bounds("track-dialog").unwrap();
    let feedback = cx.debug_bounds("track-form-feedback").unwrap();
    assert_eq!(before, shown);
    assert!(feedback.right() <= shown.right());
    assert!(shown.right() <= gpui::px(1280.));
    assert!(shown.bottom() <= gpui::px(800.));
}

#[gpui::test]
fn an_ordinary_start_recovers_when_its_project_returns(cx: &mut TestAppContext) {
    let mut document: serde_json::Value = serde_json::from_str(TRACKS).unwrap();
    let project = Snapshot::parse(TRACKS.as_bytes())
        .unwrap()
        .projects
        .into_iter()
        .find(|project| !project.track_choices.is_empty())
        .unwrap();
    let (rig, desk, cx) = engine_desk_tests::open_unread(cx, TRACKS);
    rig.say_first(r#"{"result":"done","exit_code":0,"output":"add-search"}"#);
    wait_until(cx, "the project", |cx| {
        cx.debug_bounds("new-lane-orchard").is_some()
    });
    cx.update(|window, cx| desk.update(cx, |desk, cx| desk.open_new_lane(project, window, cx)));
    wait_until(cx, "the Track picker", |cx| {
        cx.debug_bounds("new-lane-track").is_some()
    });
    crate::ui::lane_tests::click(cx, "new-lane-title");
    cx.simulate_input("Verify beta");

    let project = document["projects"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|project| project["folder"] == "/work/orchard")
        .unwrap();
    project["folder_missing"] = true.into();
    project["actions"] = serde_json::json!([]);
    rig.say_snapshot(&document.to_string());
    rig.facts.update(cx, |facts, cx| facts.read(cx));
    wait_until(cx, "the missing folder message", |cx| {
        cx.debug_bounds("new-lane-note").is_some()
    });
    crate::ui::lane_tests::click(cx, "new-lane-start");
    assert!(rig.acts().is_empty());

    rig.say_snapshot(TRACKS);
    rig.facts.update(cx, |facts, cx| facts.read(cx));
    wait_until(cx, "the same project returns", |cx| {
        cx.debug_bounds("new-lane-track").is_some()
    });
    crate::ui::lane_tests::click(cx, "new-lane-start");
    wait_until(cx, "the ordinary lane start", |_| !rig.acts().is_empty());
    let call = rig.acts().join("\n");
    assert!(call.contains("--text Verify beta"), "{call}");
    assert!(
        !call.contains("--track") && !call.contains("--op"),
        "{call}"
    );
}

#[gpui::test]
fn polish_zd_stopped_track_lanes_say_stopped_even_if_the_membership_is_live(
    cx: &mut TestAppContext,
) {
    let mut fixture: serde_json::Value = serde_json::from_str(TRACKS).unwrap();
    fixture["items"][0]["agent_state"] = "stopped".into();
    fixture["tracks"][0]["lanes"][0]["state"] = "live".into();
    let fixture = fixture.to_string();
    let snapshot = Snapshot::parse(fixture.as_bytes()).unwrap();
    let (_rig, desk, cx) = engine_desk_tests::open_unread(cx, &fixture);
    desk.update(cx, |desk, cx| {
        desk.navigate(Page::Track(snapshot.tracks.rows()[0].id.clone()), false, cx)
    });
    wait_until(cx, "the Track lane", |cx| {
        cx.debug_bounds("track-lane-0").is_some()
    });
    assert!(cx.debug_bounds("track-lane-0-status-Stopped").is_some());
}

#[gpui::test]
fn polish_zd_the_closed_linked_lane_header_has_no_track_control(cx: &mut TestAppContext) {
    let mut fixture: serde_json::Value = serde_json::from_str(TRACKS).unwrap();
    fixture["items"][0]["agent_state"] = "cleaned".into();
    let fixture = fixture.to_string();
    let snapshot = Snapshot::parse(fixture.as_bytes()).unwrap();
    let (_rig, desk, cx) = engine_desk_tests::open_unread(cx, &fixture);
    desk.update(cx, |desk, cx| {
        desk.open_lane(&snapshot.items[0].lane_key(), cx)
    });
    wait_until(cx, "the closed lane", |cx| {
        cx.debug_bounds("lane-page").is_some()
    });
    assert!(cx.debug_bounds("lane-track").is_none());
}
