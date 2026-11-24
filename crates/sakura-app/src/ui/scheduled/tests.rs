use super::super::engine_rig_tests::wait_until;
use super::super::lane_tests::{bounds, click, open_answering};
use crate::ui::inbox::InboxEvent;
use gpui::TestAppContext;
use sakura_core::tabs::Page;

const SCHEDULED: &str = include_str!("../../../../sakura-core/tests/fixtures/scheduled.json");
const STALE: &str = r#"{"result":"refused","reason":"stale_task"}"#;

#[test]
fn property_menus_resolve_known_defaults() {
    let snapshot = sakura_core::engine::snapshot::Snapshot::parse(
        br#"{
        "api":1,
        "engine":{"preset":"standard","preset_label":"Standard"},
        "projects":[{"id":"measure","name":"measure","folder":"/fixture/measure"}],
        "accounts":[{"id":"personal","label":"Personal","provider":"claude"}]
    }"#,
    )
    .unwrap();
    let [project, preset, account] = super::words::defaults(&snapshot);
    assert_eq!(project.as_deref(), Some("measure"));
    assert_eq!(preset.as_deref(), Some("Standard"));
    assert_eq!(account.as_deref(), Some("Personal"));
}

#[gpui::test]
fn create_uses_the_shared_frame_and_keeps_permissions_in_the_body(cx: &mut TestAppContext) {
    let (_, _, window) = open_answering(cx, SCHEDULED, STALE, STALE);
    window.simulate_resize(gpui::size(gpui::px(1280.), gpui::px(800.)));
    click(window, "sidebar-scheduled");
    wait_until(window, "Scheduled controls", |cx| {
        bounds(cx, "scheduled-create").is_some()
    });
    click(window, "scheduled-create");
    wait_until(window, "task editor", |cx| {
        bounds(cx, "scheduled-save").is_some()
    });
    let surface = bounds(window, "scheduled-dialog").unwrap();
    let footer = bounds(window, "dialog-footer").unwrap();
    let title = bounds(window, "scheduled-title").unwrap();
    let toggle = bounds(window, "scheduled-may-start").unwrap();
    let inset = crate::theme::CONTENT_PADDING + crate::theme::PANEL_RING_WIDTH;
    assert_eq!(surface.size.width, crate::theme::DIALOG_WIDTH_MEDIUM);
    assert_eq!(title.left() - surface.left(), inset);
    assert_eq!(surface.right() - footer.right(), inset);
    assert_eq!(surface.bottom() - footer.bottom(), inset);
    assert!(toggle.bottom() <= footer.top());
    assert!(bounds(window, "scheduled-properties").is_some());
    let frequency = bounds(window, "scheduled-when").unwrap();
    let interval = bounds(window, "scheduled-minutes").unwrap();
    assert_eq!(frequency.size.height, crate::theme::CONTROL_HEIGHT);
    assert_eq!(interval.size.height, frequency.size.height);
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
        window.simulate_resize(viewport);
        window.update(|window, cx| window.draw(cx).clear(cx));
        let [project, preset, account] =
            ["scheduled.project", "scheduled.preset", "scheduled.account"]
                .map(|id| bounds(window, id).expect(id));
        for control in [preset, account] {
            assert_eq!(control.size.width, project.size.width);
            assert_eq!(control.right(), project.right());
        }
        let body = bounds(window, "scheduled-form-fields").unwrap();
        let last = bounds(window, "scheduled.account-row").unwrap();
        assert!(last.bottom() <= body.bottom());
    }
}

#[gpui::test]
fn scheduled_keeps_its_sidebar_position_on_an_old_engine(cx: &mut TestAppContext) {
    let (_, _, window) = open_answering(cx, r#"{"api":1}"#, STALE, STALE);
    assert!(bounds(window, "sidebar-scheduled").is_some());
}

#[gpui::test]
fn a_scheduled_run_opens_in_the_inbox_pane_alongside_another_scheduled_page(
    cx: &mut TestAppContext,
) {
    let (rig, desk, window) = open_answering(cx, SCHEDULED, STALE, STALE);
    std::fs::write(
        rig.dir.path().join("conversation.json"),
        r#"{"entries":[]}"#,
    )
    .unwrap();
    window.simulate_resize(gpui::size(gpui::px(1280.), gpui::px(800.)));
    let (bar, inbox) = desk.read_with(window, |desk, _| (desk.bar.clone(), desk.inbox.clone()));
    let (left, right) = bar.update(window, |bar, cx| {
        let left = bar.tabs().active().id();
        let right = bar.open_new(Page::Scheduled, cx);
        assert!(bar.create_split(left, right, cx));
        (left, right)
    });
    inbox.update(window, |_, cx| {
        cx.emit(InboxEvent::OpenScheduledRun("run-1".into()))
    });
    wait_until(window, "the conversation in the originating pane", |cx| {
        bounds(cx, "scheduled-run-back").is_some()
    });
    bar.read_with(window, |bar, _| {
        assert_eq!(bar.tabs().tab(left).unwrap().page(), &Page::Scheduled);
        assert_eq!(bar.tabs().tab(right).unwrap().page(), &Page::Scheduled);
        assert_eq!(bar.tabs().active().id(), right);
    });
    assert!(bounds(window, "scheduled-create").is_some());
}

#[gpui::test]
fn scheduled_editor_keeps_its_geometry_and_draft_on_refusal(cx: &mut TestAppContext) {
    let (rig, _, window) = open_answering(cx, SCHEDULED, STALE, STALE);
    window.simulate_resize(gpui::size(gpui::px(1280.), gpui::px(800.)));
    click(window, "sidebar-scheduled");
    wait_until(window, "Scheduled controls", |cx| {
        bounds(cx, "scheduled-create").is_some()
    });
    click(window, "scheduled-create");
    wait_until(window, "task editor", |cx| {
        bounds(cx, "scheduled-save").is_some()
    });
    let field = bounds(window, "scheduled-name-label").unwrap();
    click(window, "scheduled-save");
    wait_until(window, "stale refusal", |cx| {
        bounds(cx, "scheduled-save-told").is_some()
    });
    let after = bounds(window, "scheduled-name-label").unwrap();
    assert_eq!(after.size, field.size);
    assert_eq!(after.left(), field.left());
    assert!((after.top() - field.top()).abs() <= gpui::px(1.));
    let actions = rig.acts();
    assert_eq!(actions.len(), 1);
    assert!(actions[0].contains(r#"scheduledcreate ["host","scheduled"] --definition"#));
    assert!(actions[0].contains(" --op "));
}

#[gpui::test]
fn history_and_conversation_are_read_only_when_opened(cx: &mut TestAppContext) {
    let (rig, _, window) = open_answering(cx, SCHEDULED, STALE, STALE);
    let reads = rig.dir.path().join("scheduled.log");
    std::fs::write(
        rig.dir.path().join("history.json"),
        r#"{"runs":[{"id":"run-1","state":"failed","reason":"all_sources_failed","conversation":"run-1"},{"id":"run-2","state":"skipped","reason":"missed_occurrence"}]}"#,
    ).unwrap();
    let transcript = rig.dir.path().join("conversation.json");
    std::fs::write(
        &transcript,
        r#"{"entries":[{"sequence":1,"kind":"prompt","text":"Summarize new items."}]}"#,
    )
    .unwrap();
    window.simulate_resize(gpui::size(gpui::px(1280.), gpui::px(800.)));
    click(window, "sidebar-scheduled");
    wait_until(window, "the task", |cx| {
        bounds(cx, "scheduled-task-morning-summary").is_some()
    });
    assert!(!reads.exists());
    click(window, "scheduled-task-morning-summary");
    wait_until(window, "history", |cx| {
        bounds(cx, "scheduled-run-run-2").is_some()
    });
    assert_eq!(
        std::fs::read_to_string(&reads).unwrap(),
        "scheduled history --json -- morning-summary\n"
    );
    click(window, "scheduled-run-run-1");
    wait_until(window, "saved prompt", |cx| {
        bounds(cx, "thread-prompt").is_some()
    });
    assert!(bounds(window, "thread-composer-box").is_none());
    assert!(rig.acts().is_empty());
    assert_eq!(
        std::fs::read_to_string(&reads).unwrap().lines().last(),
        Some("scheduled conversation --json -- run-1")
    );

    std::fs::write(transcript, r#"{"problem":"run_unavailable"}"#).unwrap();
    click(window, "scheduled-run-back");
    click(window, "scheduled-run-run-1");
    wait_until(window, "expired conversation", |cx| {
        bounds(cx, "scheduled-read-problem").is_some()
    });
    assert!(bounds(window, "thread-prompt").is_none());
    assert!(bounds(window, "scheduled-run-back").is_some());
    assert!(rig.acts().is_empty());
}

#[test]
fn paused_tasks_explain_the_operator_pause() {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    assert_eq!(
        super::words::task_reason("operator_paused", None).as_ref(),
        "You paused this task."
    );
}

#[test]
fn clock_times_name_their_time_zones() {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let iso = "2026-11-23T09:15:00Z";
    let local = chrono::DateTime::parse_from_rfc3339(iso)
        .unwrap()
        .with_timezone(&chrono::Local);
    assert_eq!(
        super::words::time(iso).to_string(),
        local
            .format("%Y-%m-%d %H:%M (local time, UTC%:z)")
            .to_string(),
    );
    let daily = sakura_core::engine::scheduled::Trigger::Daily {
        time: "09:15".into(),
        timezone: "UTC".into(),
    };
    assert_eq!(super::words::trigger(&daily, None), "Daily at 09:15 (UTC)");
}

#[test]
fn daily_next_run_uses_the_schedules_named_zone() {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let snapshot: serde_json::Value = serde_json::from_str(SCHEDULED).unwrap();
    for (instant, expected) in [
        ("2026-11-24T06:30:00Z", "2026-11-24 07:30 (Europe/Rome)"),
        ("2026-07-24T05:30:00Z", "2026-07-24 07:30 (Europe/Rome)"),
    ] {
        let mut fixture = snapshot.clone();
        fixture["scheduled"]["tasks"][0]["next_at"] = instant.into();
        let snapshot =
            sakura_core::engine::snapshot::Snapshot::parse(fixture.to_string().as_bytes()).unwrap();
        let task = &snapshot.scheduled.as_ref().unwrap().tasks[0];
        assert_eq!(
            super::words::trigger(&task.definition.when, Some(&snapshot)),
            "Daily at 07:30 (Europe/Rome)"
        );
        let next = super::words::properties(task, &snapshot)
            .into_iter()
            .find(|(key, _)| *key == "scheduled.next")
            .unwrap()
            .1;
        assert_eq!(next, expected);
    }
}

#[test]
fn deleting_a_task_uses_the_inbox_name() {
    for language in [
        crate::l10n::Language::English,
        crate::l10n::Language::Italian,
    ] {
        crate::l10n::set_for_test(language);
        let name = crate::l10n::tr(crate::l10n::Key("page.inbox"));
        let question = crate::l10n::tr_with(
            crate::l10n::Key("scheduled.delete_question"),
            &[("task", "Morning summary"), ("inbox", name)],
        );
        assert!(question.contains(name), "{question}");
    }
}

#[test]
fn daily_fields_default_to_the_macs_time_zone_and_preserve_a_saved_zone() {
    let interval = sakura_core::engine::scheduled::Trigger::Interval { minutes: 60 };
    let daily = sakura_core::engine::scheduled::Trigger::Daily {
        time: "09:15".into(),
        timezone: "America/New_York".into(),
    };
    assert_eq!(
        super::form::trigger_fields(&interval).2,
        iana_time_zone::get_timezone().unwrap_or_else(|_| "UTC".into())
    );
    assert_eq!(super::form::trigger_fields(&daily).2, "America/New_York");
}

#[gpui::test]
fn daily_task_editor_keeps_its_footer_inside_a_short_window(cx: &mut TestAppContext) {
    let mut snapshot: serde_json::Value = serde_json::from_str(SCHEDULED).unwrap();
    snapshot["scheduled"]["tasks"][0]["actions"] = serde_json::json!([
        {"key": "scheduledupdate", "label": "Edit task", "needs": ["definition", "op"]}
    ]);
    let (_, _, window) = open_answering(cx, &snapshot.to_string(), STALE, STALE);
    let height = gpui::px(280.);
    window.simulate_resize(gpui::size(gpui::px(1000.), gpui::px(800.)));
    click(window, "sidebar-scheduled");
    wait_until(window, "Scheduled task", |cx| {
        bounds(cx, "scheduled-task-morning-summary").is_some()
    });
    click(window, "scheduled-task-morning-summary");
    wait_until(window, "Edit task action", |cx| {
        bounds(cx, "scheduledupdate-morning-summary").is_some()
    });
    click(window, "scheduledupdate-morning-summary");
    wait_until(window, "task editor", |cx| {
        bounds(cx, "scheduled-save").is_some()
    });
    assert!(bounds(window, "scheduled-zone").is_some());
    window.simulate_resize(gpui::size(gpui::px(1000.), height));
    window.update(|window, cx| window.draw(cx).clear(cx));
    let fields = bounds(window, "scheduled-form-fields").unwrap();
    let save = bounds(window, "scheduled-save").unwrap();
    let before = bounds(window, "scheduled-zone").unwrap();
    let position = fields.center();
    window.simulate_mouse_move(position, None, gpui::Modifiers::none());
    window.simulate_event(gpui::ScrollEvent {
        position,
        delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(-2000.))),
        modifiers: gpui::Modifiers::none(),
        touch_phase: gpui::TouchPhase::Moved,
        source: gpui::ScrollSource::Pointer,
    });
    window.run_until_parked();
    window.update(|window, cx| window.draw(cx).clear(cx));
    let zone = bounds(window, "scheduled-zone").unwrap();
    let footer = bounds(window, "dialog-footer").unwrap();
    assert!(save.bottom() <= height, "{save:?} exceeds the viewport");
    assert!(zone.top() < before.top() && zone.bottom() <= fields.bottom());
    let toggle = bounds(window, "scheduled-may-start").unwrap();
    assert!(
        toggle.top() >= fields.top() && toggle.bottom() <= fields.bottom(),
        "{toggle:?} exceeds {fields:?}"
    );
    assert!(toggle.bottom() <= footer.top());
}

#[gpui::test]
fn overflowing_daily_creation_fields_scroll_to_the_time(cx: &mut TestAppContext) {
    let (_, _, window) = open_answering(cx, SCHEDULED, STALE, STALE);
    window.simulate_resize(gpui::size(gpui::px(1280.), gpui::px(800.)));
    click(window, "sidebar-scheduled");
    wait_until(window, "Scheduled controls", |cx| {
        bounds(cx, "scheduled-create").is_some()
    });
    click(window, "scheduled-create");
    wait_until(window, "task editor", |cx| {
        bounds(cx, "scheduled-save").is_some()
    });
    click(window, "scheduled-when-0");
    wait_until(window, "Daily fields", |cx| {
        bounds(cx, "scheduled-zone-label").is_some()
    });

    window.simulate_resize(gpui::size(gpui::px(1280.), gpui::px(300.)));
    let fields = bounds(window, "scheduled-form-fields").unwrap();
    let time = bounds(window, "scheduled-time").unwrap();
    let save = bounds(window, "scheduled-save").unwrap();
    assert!(
        time.bottom() > fields.bottom(),
        "{time:?} fits in {fields:?}"
    );

    let at = gpui::point(
        fields.right() - gpui::px(4.),
        fields.bottom() - gpui::px(4.),
    );
    window.simulate_event(gpui::ScrollEvent {
        position: at,
        delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(-2000.))),
        modifiers: gpui::Modifiers::none(),
        touch_phase: gpui::TouchPhase::Moved,
        source: gpui::ScrollSource::Pointer,
    });
    window.run_until_parked();
    window.update(|window, cx| window.draw(cx).clear(cx));
    let time = bounds(window, "scheduled-time").unwrap();
    assert!(
        time.bottom() <= fields.bottom(),
        "{time:?} is clipped by {fields:?}"
    );
    assert_eq!(bounds(window, "scheduled-save"), Some(save));
}

#[gpui::test]
fn polish_x_dialog_create_task_indents_connector_rows_inside_the_group(cx: &mut TestAppContext) {
    let mut snapshot: serde_json::Value = serde_json::from_str(SCHEDULED).unwrap();
    snapshot["connectors"] = serde_json::json!([
        {"id":"work", "kind":"github", "label":"Work", "connected": true}
    ]);
    let (_, _, window) = open_answering(cx, &snapshot.to_string(), STALE, STALE);
    window.simulate_resize(gpui::size(gpui::px(1280.), gpui::px(800.)));
    click(window, "sidebar-scheduled");
    wait_until(window, "Scheduled controls", |cx| {
        bounds(cx, "scheduled-create").is_some()
    });
    click(window, "scheduled-create");
    wait_until(window, "the connector row", |cx| {
        bounds(cx, "scheduled-connector-row-work").is_some()
    });
    let group = bounds(window, "scheduled-connectors-group").unwrap();
    let name = bounds(window, "scheduled-connector-row-work-name");
    assert_eq!(
        name.map(|name| name.left() - group.left()),
        Some(crate::theme::kit::scale::SETTINGS_ROW_PADDING)
    );
}

#[gpui::test]
fn compact_schedule_fields_align_with_the_footer(cx: &mut TestAppContext) {
    let mut snapshot: serde_json::Value = serde_json::from_str(SCHEDULED).unwrap();
    snapshot["connectors"] = serde_json::json!([]);
    let (_, _, window) = open_answering(cx, &snapshot.to_string(), STALE, STALE);
    window.simulate_resize(gpui::size(gpui::px(1280.), gpui::px(800.)));
    click(window, "sidebar-scheduled");
    wait_until(window, "Scheduled controls", |cx| {
        bounds(cx, "scheduled-create").is_some()
    });
    click(window, "scheduled-create");
    wait_until(window, "task editor", |cx| {
        bounds(cx, "scheduled-save").is_some()
    });
    click(window, "scheduled-when-0");
    wait_until(window, "daily fields", |cx| {
        bounds(cx, "scheduled-zone").is_some()
    });

    let fields = bounds(window, "scheduled-form-fields").unwrap();
    let prompt = bounds(window, "scheduled-prompt").unwrap();
    let title = bounds(window, "scheduled-name-label").unwrap();
    let time = bounds(window, "scheduled-time").unwrap();
    let zone = bounds(window, "scheduled-zone").unwrap();
    let clock = bounds(window, "scheduled-clock").unwrap();
    assert_eq!(clock.size.height, crate::theme::CONTROL_HEIGHT);
    assert_eq!(title.size.width, prompt.size.width);
    assert!(time.right() < zone.left());
    let footer = bounds(window, "dialog-footer").unwrap();
    assert_eq!(fields.right(), footer.right());
    assert_eq!(prompt.right(), footer.right());
    assert!(zone.right() < footer.right());
    assert!(bounds(window, "scheduled-connectors-group").is_none());
}

#[gpui::test]
fn scheduled_permission_places_the_switch_on_the_right(cx: &mut TestAppContext) {
    let (_, _, window) = open_answering(cx, SCHEDULED, STALE, STALE);
    window.simulate_resize(gpui::size(
        crate::theme::WINDOW_MIN_WIDTH,
        crate::theme::WINDOW_MIN_HEIGHT,
    ));
    click(window, "sidebar-scheduled");
    wait_until(window, "Scheduled controls", |cx| {
        bounds(cx, "scheduled-create").is_some()
    });
    click(window, "scheduled-create");
    wait_until(window, "task editor", |cx| {
        bounds(cx, "scheduled-may-start").is_some()
    });
    let permission = bounds(window, "scheduled-may-start-row").unwrap();
    let toggle = bounds(window, "scheduled-may-start").unwrap();
    let label = bounds(window, "scheduled-permission-row-label").unwrap();
    assert!(label.right() <= toggle.left());
    assert!(permission.right() - toggle.right() == crate::theme::kit::scale::SETTINGS_ROW_PADDING);
}

#[gpui::test]
fn reaudit_scheduled_detail_actions_share_a_row(cx: &mut TestAppContext) {
    let mut fixture: serde_json::Value = serde_json::from_str(SCHEDULED).unwrap();
    fixture["scheduled"]["tasks"][0]["actions"].as_array_mut().unwrap()
        .push(serde_json::json!({"key":"scheduledupdate","label":"Edit task","needs":["definition","op"]}));
    let (_, _, window) = open_answering(cx, &fixture.to_string(), STALE, STALE);
    click(window, "sidebar-scheduled");
    wait_until(window, "scheduled task", |window| {
        bounds(window, "scheduled-task-morning-summary").is_some()
    });
    click(window, "scheduled-task-morning-summary");
    let edit = "scheduledupdate-morning-summary";
    let run = "scheduledrun-morning-summary";
    wait_until(window, "scheduled actions", |window| {
        bounds(window, edit).is_some()
    });
    let edit = bounds(window, edit).unwrap();
    let run = bounds(window, run).unwrap();
    assert_eq!(edit.center().y, run.center().y);
    assert!(edit.right() < run.left());
}

fn create_for_reaudit(cx: &mut TestAppContext) -> &mut gpui::VisualTestContext {
    let (_, _, window) = open_answering(cx, SCHEDULED, STALE, STALE);
    window.simulate_resize(gpui::size(
        crate::theme::WINDOW_MIN_WIDTH,
        crate::theme::WINDOW_MIN_HEIGHT,
    ));
    click(window, "sidebar-scheduled");
    wait_until(window, "Scheduled controls", |window| {
        bounds(window, "scheduled-create").is_some()
    });
    click(window, "scheduled-create");
    wait_until(window, "Scheduled editor", |window| {
        bounds(window, "scheduled-save").is_some()
    });
    window
}

#[gpui::test]
fn reaudit_task_fields_paint_the_shared_fill(cx: &mut TestAppContext) {
    let window = create_for_reaudit(cx);
    let fields =
        ["scheduled-name-label", "scheduled-prompt"].map(|id| bounds(window, id).expect(id));
    assert_eq!(fields[0].left(), fields[1].left());
    window.update(|window, cx| {
        window.draw(cx).clear(cx);
        let fill = crate::theme::Theme::palette(cx).fill;
        for field in fields {
            assert!(
                window.painted_quads().iter().any(|quad| {
                    quad.bounds == field.scale(window.scale_factor())
                        && quad.background.as_solid() == Some(fill)
                }),
                "no field fill at {field:?}"
            );
        }
    });
}

#[gpui::test]
fn reaudit_tall_sheet_uses_the_minimum_window_content_height(cx: &mut TestAppContext) {
    use crate::theme;
    let window = create_for_reaudit(cx);
    let sheet = bounds(window, "scheduled-dialog").unwrap();
    let body = bounds(window, "scheduled-form-fields").unwrap();
    let footer = bounds(window, "dialog-footer").unwrap();
    assert!(
        theme::WINDOW_MIN_HEIGHT - sheet.bottom() <= theme::DIALOG_TOP,
        "{sheet:?}"
    );
    assert!(
        body.bottom() <= footer.top() && body.size.height > gpui::px(340.),
        "{body:?}"
    );
}

#[test]
fn reaudit3_scheduled_property_labels_fit_on_one_line() {
    crate::ui::real_text::run(|cx| {
        let (_, _, window) = open_answering(cx, SCHEDULED, STALE, STALE);
        for width in [
            crate::theme::WINDOW_MIN_WIDTH,
            crate::theme::WINDOW_START_WIDTH,
        ] {
            window.simulate_resize(gpui::size(width, crate::theme::WINDOW_START_HEIGHT));
            click(window, "sidebar-scheduled");
            wait_until(window, "Scheduled task", |cx| {
                bounds(cx, "scheduled-task-morning-summary").is_some()
            });
            click(window, "scheduled-task-morning-summary");
            wait_until(window, "Scheduled properties", |cx| {
                bounds(cx, "scheduled.connectors-value").is_some()
            });
            let mut row = |label| bounds(window, label).unwrap();
            let status = row("scheduled.status");
            for label in ["scheduled.connectors", "scheduled.may_start"] {
                assert_eq!(row(label).size.height, status.size.height, "{label}");
            }
            assert_eq!(
                row("scheduled.status-value").left(),
                row("scheduled.connectors-value").left()
            );
        }
    });
}

#[gpui::test]
fn reaudit3_daily_editor_shows_a_whole_connector_row_at_minimum(cx: &mut TestAppContext) {
    use crate::theme;
    let mut snapshot: serde_json::Value = serde_json::from_str(SCHEDULED).unwrap();
    snapshot["scheduled"]["tasks"][0]["actions"] = serde_json::json!([
        {"key": "scheduledupdate", "label": "Edit task", "needs": ["definition", "op"]}
    ]);
    snapshot["connectors"] = serde_json::json!([
        {"id":"work", "kind":"github", "label":"Work", "connected": true}
    ]);
    let (_, _, window) = open_answering(cx, &snapshot.to_string(), STALE, STALE);
    window.simulate_resize(gpui::size(
        theme::WINDOW_MIN_WIDTH,
        theme::WINDOW_MIN_HEIGHT,
    ));
    click(window, "sidebar-scheduled");
    wait_until(window, "Scheduled task", |cx| {
        bounds(cx, "scheduled-task-morning-summary").is_some()
    });
    click(window, "scheduled-task-morning-summary");
    wait_until(window, "Edit task action", |cx| {
        bounds(cx, "scheduledupdate-morning-summary").is_some()
    });
    click(window, "scheduledupdate-morning-summary");
    wait_until(window, "task editor", |cx| {
        bounds(cx, "scheduled-connector-row-work").is_some()
    });
    let fields = bounds(window, "scheduled-form-fields").unwrap();
    let connector = bounds(window, "scheduled-connector-row-work").unwrap();
    assert!(
        connector.bottom() + theme::GAP <= fields.bottom(),
        "{connector:?} in {fields:?}"
    );
}

#[test]
fn reaudit3_scheduled_properties_distinguish_resolved_and_missing_defaults() {
    use sakura_core::engine::snapshot::Snapshot;
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let snapshot = Snapshot::parse(SCHEDULED.as_bytes()).unwrap();
    let task = &snapshot.scheduled.as_ref().unwrap().tasks[0];
    let values = |snapshot: &Snapshot| {
        super::words::properties(task, snapshot)
            .into_iter()
            .collect::<std::collections::HashMap<_, _>>()
    };
    let missing = values(&snapshot);
    assert_eq!(missing["scheduled.project"], "No project selected");
    assert_eq!(missing["scheduled.account"], "No account selected");
    let resolved = Snapshot::parse(br#"{"api":1,"projects":[{"id":"repo","name":"repo","folder":"/fixture/repo"}],"accounts":[{"id":"personal","label":"Personal","provider":"claude"}]}"#).unwrap();
    let shown = values(&resolved);
    assert_eq!(shown["scheduled.project"], "repo");
    assert_eq!(shown["scheduled.account"], "Personal");
}

#[gpui::test]
fn reaudit3_scheduled_detail_separates_actions_from_properties(cx: &mut TestAppContext) {
    let (_, _, window) = open_answering(cx, SCHEDULED, STALE, STALE);
    click(window, "sidebar-scheduled");
    wait_until(window, "Scheduled task", |cx| {
        bounds(cx, "scheduled-task-morning-summary").is_some()
    });
    click(window, "scheduled-task-morning-summary");
    wait_until(window, "Scheduled properties", |cx| {
        bounds(cx, "scheduled.project").is_some()
    });
    let run = bounds(window, "scheduledrun-morning-summary").unwrap();
    let status = bounds(window, "scheduled.status").unwrap();
    let project = bounds(window, "scheduled.project").unwrap();
    let cadence = project.top() - status.top();
    assert!(
        status.top() - run.bottom() > cadence * 2.,
        "actions {run:?}, properties {status:?}, cadence {cadence:?}"
    );
}

#[gpui::test]
fn reaudit3_scheduled_full_width_controls_end_at_the_footer(cx: &mut TestAppContext) {
    let mut snapshot: serde_json::Value = serde_json::from_str(SCHEDULED).unwrap();
    snapshot["connectors"] = serde_json::json!([
        {"id":"work", "kind":"github", "label":"Work", "connected": true}
    ]);
    let (_, _, window) = open_answering(cx, &snapshot.to_string(), STALE, STALE);
    window.simulate_resize(gpui::size(
        crate::theme::WINDOW_START_WIDTH,
        crate::theme::WINDOW_START_HEIGHT,
    ));
    click(window, "sidebar-scheduled");
    wait_until(window, "Scheduled controls", |cx| {
        bounds(cx, "scheduled-create").is_some()
    });
    click(window, "scheduled-create");
    wait_until(window, "scheduled connectors", |cx| {
        bounds(cx, "scheduled-connectors-group").is_some()
    });
    let footer = bounds(window, "dialog-footer").unwrap();
    let group = bounds(window, "scheduled-connectors-group").unwrap();
    assert_eq!(group.right(), footer.right());
}

#[gpui::test]
fn reaudit3_daily_time_and_zone_fields_have_the_same_height(cx: &mut TestAppContext) {
    let window = create_for_reaudit(cx);
    click(window, "scheduled-when-0");
    wait_until(window, "daily fields", |cx| {
        bounds(cx, "scheduled-zone").is_some()
    });
    let clock = bounds(window, "scheduled-clock").unwrap();
    let zone = bounds(window, "scheduled-zone").unwrap();
    assert_eq!(clock.top(), zone.top());
    assert_eq!(clock.size.height, zone.size.height);
}

#[gpui::test]
fn reaudit3_scrolling_task_thumb_stays_clear_of_the_controls(cx: &mut TestAppContext) {
    use crate::theme;
    let mut data: serde_json::Value = serde_json::from_str(SCHEDULED).unwrap();
    data["connectors"] = (0..12).map(|id| serde_json::json!({
        "id":format!("work-{id}"), "kind":"github", "label":format!("Work {id}"), "connected":true,
    })).collect::<Vec<_>>().into();
    let (_, _, window) = open_answering(cx, &data.to_string(), STALE, STALE);
    window.simulate_resize(gpui::size(
        theme::WINDOW_MIN_WIDTH,
        theme::WINDOW_MIN_HEIGHT,
    ));
    click(window, "sidebar-scheduled");
    wait_until(window, "Scheduled controls", |cx| {
        bounds(cx, "scheduled-create").is_some()
    });
    click(window, "scheduled-create");
    wait_until(window, "task editor", |cx| {
        bounds(cx, "scheduled-form-fields").is_some()
    });
    let fields = bounds(window, "scheduled-form-fields").unwrap();
    window.simulate_event(gpui::ScrollEvent {
        position: fields.center(),
        delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::Pixels::ZERO, gpui::px(-200.))),
        touch_phase: gpui::TouchPhase::Moved,
        source: gpui::ScrollSource::Pointer,
        ..Default::default()
    });
    window.run_until_parked();
    window.update(|window, cx| {
        window.draw(cx).clear(cx);
        let scale = window.scale_factor();
        let thumb = window
            .painted_quads()
            .into_iter()
            .find(|quad| {
                quad.bounds.size.width <= gpui::px(12.).scale(scale)
                    && quad.bounds.size.height >= gpui::px(30.).scale(scale)
                    && quad.bounds.left() >= (fields.right() - theme::GAP_WIDE).scale(scale)
                    && quad.bounds.left() < (fields.right() + theme::CONTENT_PADDING).scale(scale)
            })
            .expect("visible scrollbar thumb");
        assert!(
            thumb.bounds.left() >= fields.right().scale(window.scale_factor()),
            "thumb {:?} overlaps controls {fields:?}",
            thumb.bounds
        );
    });
}
