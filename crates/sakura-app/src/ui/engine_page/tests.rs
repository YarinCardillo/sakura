//! The Engine page as a person meets it: the pipeline drawn from the engine's rules, the preset, the pace and
//! the background services, what each control sends to the engine, and what an older engine's page says.

use super::schema::Node;
use super::EnginePage;
use crate::ui::engine_desk::Desk;
use crate::ui::engine_fixture_tests::{
    edited, full, one_account, rules_edited, rules_for_one_account,
};
use crate::ui::engine_rig_tests::{wait_until, Rig, SNAPSHOT};
use crate::ui::lane_tests::{bounds, click, open_answering};
use gpui::{Entity, TestAppContext, VisualTestContext};
use gpui_component::slider::SliderEvent;
use sakura_core::tabs::Page;

const DONE: &str = r#"{"result": "done", "exit_code": 0, "output": "ok\n"}"#;
const ENGINE: &str = r#"["host","engine"]"#;
const AUTOMATIONS: &str = r#"["host","automations"]"#;
const SERVICES: &str = r#"["host","services"]"#;

/// The desk over a fake engine with `snapshot`, on the Engine page, its rules read and drawn.
fn open<'a>(
    cx: &'a mut TestAppContext,
    snapshot: &str,
) -> (Rig, Entity<Desk>, &'a mut VisualTestContext) {
    let (rig, desk, cx) = open_answering(cx, snapshot, DONE, DONE);
    // Tall enough to hold the whole page: what is below the fold takes no click.
    cx.simulate_resize(gpui::size(gpui::px(1280.), gpui::px(2400.)));
    desk.update(cx, |desk, cx| desk.navigate(Page::Engine, false, cx));
    wait_until(cx, "the Engine page", |cx| {
        cx.debug_bounds("engine-page").is_some()
    });
    (rig, desk, cx)
}

/// The same, once the preset bar (which needs the rules) is there.
pub(super) fn open_with_rules<'a>(
    cx: &'a mut TestAppContext,
    snapshot: &str,
) -> (Rig, Entity<Desk>, &'a mut VisualTestContext) {
    let (rig, desk, cx) = open(cx, snapshot);
    wait_until(cx, "the rules", |cx| {
        cx.debug_bounds("engine-preset-0").is_some()
    });
    (rig, desk, cx)
}

/// The rules the fake engine prints are now `text`, and the page reads them again.
fn say_rules(rig: &Rig, desk: &Entity<Desk>, text: &str, cx: &mut VisualTestContext) {
    rig.say_rules(text);
    let page = page(desk, cx);
    page.update(cx, |page, cx| page.read_rules(cx));
}

fn page(desk: &Entity<Desk>, cx: &mut VisualTestContext) -> Entity<EnginePage> {
    desk.read_with(cx, |desk, cx| desk.engine_page(cx))
}

fn sent(rig: &Rig, cx: &mut VisualTestContext, line: &str) {
    wait_until(cx, "the call", |_| rig.acts().iter().any(|act| act == line));
}

fn redraw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

#[gpui::test]
fn clicking_the_review_above_plan_shows_its_rules_and_only_its_rules(cx: &mut TestAppContext) {
    let (_rig, _desk, cx) = open_with_rules(cx, &full());
    assert!(
        bounds(cx, "engine-row-steps.build.model").is_some(),
        "Build is open at first"
    );
    click(cx, "engine-node-plan_review");
    redraw(cx);
    assert!(bounds(cx, "engine-row-reviews.plan_review.on").is_some());
    assert!(bounds(cx, "engine-row-reviews.plan_review.rounds").is_some());
    assert!(
        bounds(cx, "engine-row-steps.build.model").is_none(),
        "Build's rules are gone"
    );
}

#[gpui::test]
fn switching_a_review_off_sends_k_and_the_node_is_drawn_off_once_the_engine_says_so(
    cx: &mut TestAppContext,
) {
    let (rig, _desk, cx) = open_with_rules(cx, &full());
    click(cx, "engine-node-plan_review");
    redraw(cx);
    assert!(
        bounds(cx, "engine-node-plan_review-off").is_none(),
        "on at first"
    );
    // The engine applies the change: what it prints from now on has the review off.
    rig.say_rules(&rules_edited(|rules| {
        for node in rules["nodes"].as_array_mut().expect("nodes") {
            for rule in node["rules"].as_array_mut().expect("rules") {
                if rule["id"] == "reviews.plan_review.on" {
                    rule["value"] = false.into();
                }
            }
        }
    }));
    click(cx, "engine-rule-reviews.plan_review.on");
    let line = format!(
        "cockpit act K {ENGINE} --rule reviews.plan_review.on --value false --source app --json"
    );
    sent(&rig, cx, &line);
    wait_until(cx, "the node drawn off", |cx| {
        cx.debug_bounds("engine-node-plan_review-off").is_some()
    });
}

#[gpui::test]
fn a_rule_that_is_a_whole_number_is_changed_with_the_stepper_inside_its_range(
    cx: &mut TestAppContext,
) {
    let (rig, _desk, cx) = open_with_rules(cx, &full());
    click(cx, "engine-node-task_review");
    redraw(cx);
    click(cx, "engine-rule-reviews.task_review.rounds-more");
    let_the_pause_end(cx);
    sent(&rig, cx, &format!("cockpit act K {ENGINE} --rule reviews.task_review.rounds --value 3 --source app --json"));
}

#[gpui::test]
fn picking_lean_sends_u_with_the_preset(cx: &mut TestAppContext) {
    let (rig, _desk, cx) = open_with_rules(cx, &full());
    click(cx, "engine-preset-3");
    sent(
        &rig,
        cx,
        &format!("cockpit act U {ENGINE} --preset lean --source app --json"),
    );
}

#[gpui::test]
fn once_a_rule_differs_the_page_says_edited_and_offers_to_save_it_as_a_preset(
    cx: &mut TestAppContext,
) {
    let snapshot = edited(|snapshot| snapshot["engine"]["edited"] = true.into());
    let (rig, desk, cx) = open(cx, &snapshot);
    say_rules(
        &rig,
        &desk,
        &rules_edited(|rules| rules["edited"] = true.into()),
        cx,
    );
    wait_until(cx, "the edited note", |cx| {
        cx.debug_bounds("engine-edited").is_some()
    });
    click(cx, "engine-save-preset");
    wait_until(cx, "the name field", |cx| {
        cx.debug_bounds("engine-save-preset-send").is_some()
    });
}

/// The rules with the person's change on the rule `id`: the engine says the preset is edited and the rule
/// comes from the person's layer.
fn rules_changed(id: &'static str) -> String {
    rules_edited(|rules| {
        rules["edited"] = true.into();
        for node in rules["nodes"].as_array_mut().expect("nodes") {
            for rule in node["rules"].as_array_mut().expect("rules") {
                if rule["id"] == id {
                    rule["origin"] = "rules".into();
                }
            }
        }
    })
}

#[gpui::test]
fn an_edited_standard_keeps_standard_marked_and_marks_only_the_step_that_changed(
    cx: &mut TestAppContext,
) {
    let snapshot = edited(|snapshot| snapshot["engine"]["edited"] = true.into());
    let (rig, desk, cx) = open(cx, &snapshot);
    say_rules(&rig, &desk, &rules_changed("steps.build.model"), cx);
    wait_until(cx, "the edited note", |cx| {
        cx.debug_bounds("engine-edited").is_some()
    });
    let chosen = page(&desk, cx).read_with(cx, |page, cx| page.marked_presets(cx));
    assert_eq!(chosen.iter().filter(|marked| **marked).count(), 1);
    assert!(chosen[1], "Standard, the second preset, stays marked");
    assert!(bounds(cx, "engine-node-build-edited").is_some());
    for node in ["plan", "task_review", "checks", "deliver"] {
        assert!(
            bounds(cx, &format!("engine-node-{node}-edited")).is_none(),
            "{node} is as the preset has it"
        );
    }
}

#[gpui::test]
fn back_to_standard_uses_the_preset_again_and_the_marks_go_when_the_engine_has_dropped_the_changes(
    cx: &mut TestAppContext,
) {
    let snapshot = edited(|snapshot| snapshot["engine"]["edited"] = true.into());
    let (rig, desk, cx) = open(cx, &snapshot);
    say_rules(&rig, &desk, &rules_changed("steps.build.model"), cx);
    wait_until(cx, "the way back", |cx| {
        cx.debug_bounds("engine-back-to-preset").is_some()
    });
    click(cx, "engine-back-to-preset");
    sent(
        &rig,
        cx,
        &format!("cockpit act U {ENGINE} --preset standard --source app --json"),
    );
    say_rules(&rig, &desk, &rules_edited(|_| {}), cx);
    wait_until(cx, "the marks go", |cx| {
        cx.debug_bounds("engine-edited").is_none()
            && cx.debug_bounds("engine-node-build-edited").is_none()
    });
}

/// The rules of the preset `name` with the plan switched off, as a preset that has no plan reads.
fn rules_of(name: &'static str) -> String {
    rules_edited(|rules| {
        rules["preset"] = name.into();
        for node in rules["nodes"].as_array_mut().expect("nodes") {
            for rule in node["rules"].as_array_mut().expect("rules") {
                if rule["id"] == "steps.plan.on" {
                    rule["value"] = false.into();
                }
            }
        }
    })
}

#[gpui::test]
fn pressing_a_preset_keeps_the_confirmed_diagram_while_the_engine_answers(cx: &mut TestAppContext) {
    let (rig, desk, cx) = open_with_rules(cx, &full());
    // The page has met Lean: the plan is off in it.
    say_rules(&rig, &desk, &rules_of("lean"), cx);
    wait_until(cx, "Lean read", |cx| {
        cx.debug_bounds("engine-node-plan-off").is_some()
    });
    say_rules(&rig, &desk, &rules_edited(|_| {}), cx);
    wait_until(cx, "Standard read", |cx| {
        cx.debug_bounds("engine-node-plan-off").is_none()
    });
    // The engine is slow to answer.
    rig.say_delay("3");
    let at = bounds(cx, "engine-preset-3").expect("Lean").center();
    cx.simulate_click(at, gpui::Modifiers::none());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let marked = page(&desk, cx).read_with(cx, |page, cx| page.marked_presets(cx));
    assert_eq!(
        marked,
        [false, true, false, false],
        "the engine still lists Standard"
    );
    assert!(
        bounds(cx, "engine-node-plan-off").is_none(),
        "the diagram still shows Standard's steps"
    );
    assert!(bounds(cx, "engine-preset-pending").is_some());
}

#[gpui::test]
fn a_preset_the_engine_refuses_goes_back_with_its_words(cx: &mut TestAppContext) {
    let (rig, desk, cx) = open_answering(cx, &full(), REFUSED, REFUSED);
    cx.simulate_resize(gpui::size(gpui::px(1280.), gpui::px(2400.)));
    desk.update(cx, |desk, cx| desk.navigate(Page::Engine, false, cx));
    wait_until(cx, "the rules", |cx| {
        cx.debug_bounds("engine-preset-0").is_some()
    });
    say_rules(&rig, &desk, &rules_of("lean"), cx);
    say_rules(&rig, &desk, &rules_edited(|_| {}), cx);
    click(cx, "engine-preset-3");
    wait_until(cx, "the refusal", |cx| {
        cx.debug_bounds("engine-preset-told").is_some()
    });
    let marked = page(&desk, cx).read_with(cx, |page, cx| page.marked_presets(cx));
    assert_eq!(
        marked,
        [false, true, false, false],
        "Standard is marked again"
    );
}

#[gpui::test]
fn with_one_account_no_account_rule_and_no_node_line_names_an_account(cx: &mut TestAppContext) {
    let (rig, desk, cx) = open(cx, &one_account());
    say_rules(&rig, &desk, &rules_for_one_account(), cx);
    wait_until(cx, "the rules of one account", |cx| {
        cx.debug_bounds("engine-preset-0").is_some()
            && cx.debug_bounds("engine-row-steps.build.account").is_none()
    });
    assert!(bounds(cx, "engine-row-steps.build.model").is_some());
    assert!(
        bounds(cx, "engine-row-steps.build.account").is_none(),
        "no account rule"
    );
    assert!(
        bounds(cx, "engine-node-build-account").is_none(),
        "no account on the node"
    );
    assert!(
        bounds(cx, "engine-account-personal").is_some(),
        "the one account is listed in Pace"
    );
    assert!(bounds(cx, "engine-account-work").is_none());
}

#[gpui::test]
fn with_several_accounts_the_nodes_and_the_rules_name_theirs(cx: &mut TestAppContext) {
    let (_rig, _desk, cx) = open_with_rules(cx, &full());
    assert!(bounds(cx, "engine-node-build-account").is_some());
    assert!(bounds(cx, "engine-row-steps.build.account").is_some());
}

#[gpui::test]
fn releasing_the_slider_at_fast_sends_q_with_the_level(cx: &mut TestAppContext) {
    let (rig, desk, cx) = open_with_rules(cx, &full());
    let slider = page(&desk, cx).read_with(cx, |page, _| page.pace.clone());
    slider.update(cx, |_, cx| cx.emit(SliderEvent::Release(3.0.into())));
    sent(
        &rig,
        cx,
        &format!("cockpit act Q {ENGINE} --level fast --source app --json"),
    );
}

/// The slider's value now.
fn pace_value(desk: &Entity<Desk>, cx: &mut VisualTestContext) -> f32 {
    let slider = page(desk, cx).read_with(cx, |page, _| page.pace.clone());
    slider.read_with(cx, |slider, _| slider.value().start())
}

/// The point of the slider's track at `level` (from 0 to 4).
fn pace_point(cx: &mut VisualTestContext, level: f32) -> gpui::Point<gpui::Pixels> {
    let track = bounds(cx, "engine-pace-slider").expect("the slider");
    gpui::point(
        track.left() + track.size.width * (level / 4.),
        track.center().y,
    )
}

#[gpui::test]
fn dragging_the_pace_slider_follows_the_pointer_between_the_levels_and_names_the_nearest(
    cx: &mut TestAppContext,
) {
    let (rig, desk, cx) = open_with_rules(cx, &full());
    let middle = pace_point(cx, 2.);
    cx.simulate_mouse_down(middle, gpui::MouseButton::Left, gpui::Modifiers::none());
    let drag_to = pace_point(cx, 1.4);
    for level in [1.9, 1.7, 1.5] {
        let at = pace_point(cx, level);
        cx.simulate_mouse_move(at, Some(gpui::MouseButton::Left), gpui::Modifiers::none());
    }
    cx.simulate_mouse_move(
        drag_to,
        Some(gpui::MouseButton::Left),
        gpui::Modifiers::none(),
    );
    redraw(cx);
    let held = pace_value(&desk, cx);
    assert!(
        (held - 1.4).abs() < 0.2 && (held - held.round()).abs() > 0.05,
        "the thumb is at {held}, between the levels"
    );
    let named = page(&desk, cx).read_with(cx, |page, _| page.dragging);
    assert_eq!(named, Some(1), "the words name the nearest level");
    cx.simulate_mouse_up(drag_to, gpui::MouseButton::Left, gpui::Modifiers::none());
    sent(
        &rig,
        cx,
        &format!("cockpit act Q {ENGINE} --level slow --source app --json"),
    );
    for _ in 0..20 {
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(20));
        redraw(cx);
    }
    assert_eq!(pace_value(&desk, cx), 1.0, "the thumb glided to the level");
}

#[gpui::test]
fn the_accounts_are_listed_in_order_with_their_line_until_the_pace_has_none(
    cx: &mut TestAppContext,
) {
    let (_rig, desk, cx) = open_with_rules(cx, &full());
    let top = |cx: &mut VisualTestContext, id: &str| {
        f32::from(bounds(cx, &format!("engine-account-{id}")).expect(id).top())
    };
    let tops = [top(cx, "personal"), top(cx, "work"), top(cx, "codex")];
    assert!(tops[0] < tops[1] && tops[1] < tops[2], "{tops:?}");
    assert!(
        bounds(cx, "engine-account-work-line").is_some(),
        "a line at normal"
    );
    let slider = page(&desk, cx).read_with(cx, |page, _| page.pace.clone());
    slider.update(cx, |_, cx| cx.emit(SliderEvent::Change(4.0.into())));
    redraw(cx);
    assert!(
        bounds(cx, "engine-account-work-line").is_none(),
        "no line at fastest"
    );
}

#[gpui::test]
fn the_priority_switch_sends_y(cx: &mut TestAppContext) {
    let (rig, _desk, cx) = open_with_rules(cx, &full());
    click(cx, "engine-priority");
    sent(
        &rig,
        cx,
        &format!("cockpit act Y {ENGINE} --on no --source app --json"),
    );
}

#[gpui::test]
fn the_lanes_at_once_steppers_send_k_for_the_fewest_and_the_most(cx: &mut TestAppContext) {
    let (rig, _desk, cx) = open_with_rules(cx, &full());
    click(cx, "engine-lanes-max-more");
    let_the_pause_end(cx);
    sent(
        &rig,
        cx,
        &format!("cockpit act K {ENGINE} --rule lanes --value 5 --source app --json"),
    );
    redraw(cx);
    click(cx, "engine-lanes-min-more");
    let_the_pause_end(cx);
    sent(
        &rig,
        cx,
        &format!("cockpit act K {ENGINE} --rule lanes_min --value 2 --source app --json"),
    );
}

#[gpui::test]
fn an_automation_switch_sends_a_to_the_automations_and_a_service_switch_sends_b_to_the_services(
    cx: &mut TestAppContext,
) {
    let (rig, _desk, cx) = open_with_rules(cx, &full());
    click(cx, "engine-automation-silent-lane-watch");
    sent(
        &rig,
        cx,
        &format!(
            "cockpit act A {AUTOMATIONS} --name silent-lane-watch --on yes --source app --json"
        ),
    );
    click(cx, "engine-service-keeper");
    sent(
        &rig,
        cx,
        &format!("cockpit act B {SERVICES} --name keeper --on no --source app --json"),
    );
}

#[gpui::test]
fn a_running_lane_has_its_own_pace_in_the_table(cx: &mut TestAppContext) {
    let (rig, _desk, cx) = open_with_rules(cx, &full());
    click(cx, "engine-lane-pace-demo");
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    // The same five paces as the new lane's sheet: the fourth is fast.
    for _ in 0..4 {
        cx.dispatch_action(gpui_base::actions::SelectDown);
    }
    cx.dispatch_action(gpui_base::actions::Confirm { secondary: false });
    let line = r#"cockpit act q ["flow","/work/repo","demo"] --level fast --source app --json"#;
    sent(&rig, cx, line);
}

#[test]
fn the_table_is_headed_by_what_it_lists_the_open_lanes() {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    assert_eq!(
        crate::l10n::tr(crate::l10n::Key("engine_page.table.lanes")),
        "Open lanes"
    );
    crate::l10n::set_for_test(crate::l10n::Language::Italian);
    assert_eq!(
        crate::l10n::tr(crate::l10n::Key("engine_page.table.lanes")),
        "Corsie aperte"
    );
}

#[gpui::test]
fn the_table_says_a_lane_s_pace_held_for_its_next_step_as_details_does(cx: &mut TestAppContext) {
    let held = crate::ui::engine_fixture_tests::edited(|snapshot| {
        let items = snapshot["items"].as_array_mut().expect("the items");
        let demo = items.iter_mut().find(|item| item["key"][2] == "demo");
        let demo = demo.expect("demo");
        demo["pace"] = "slow".into();
        demo["pace_pending"] = true.into();
    });
    let (_rig, _desk, cx) = open_with_rules(cx, &held);
    assert!(
        bounds(cx, "engine-lane-pace-pending-demo").is_some(),
        "the same note, from the same field"
    );
    let (_rig, _desk, cx) = open_with_rules(cx, &full());
    assert!(bounds(cx, "engine-lane-pace-pending-demo").is_none());
}

#[gpui::test]
fn the_table_has_no_today_column_when_the_engine_reports_no_share(cx: &mut TestAppContext) {
    let (_rig, _desk, cx) = open_with_rules(cx, &full());
    assert!(bounds(cx, "engine-lanes-table").is_some());
    assert!(
        bounds(cx, "engine-today-header").is_none(),
        "no empty column"
    );
    assert!(bounds(cx, "engine-today-demo").is_none());
}

#[gpui::test]
fn the_morning_digest_row_opens_its_dialog_and_run_now_sends_g_on_the_digest(
    cx: &mut TestAppContext,
) {
    let (rig, _desk, cx) = open_with_rules(cx, &full());
    click(cx, "engine-digest-row");
    wait_until(cx, "the dialog", |cx| {
        cx.debug_bounds("engine-digest-dialog").is_some()
    });
    assert!(
        bounds(cx, "engine-digest-source-github").is_some(),
        "its sources"
    );
    assert!(
        bounds(cx, "engine-digest-row-account").is_some(),
        "an account choice with several accounts"
    );
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
        let [at, lookback, language, account] = [
            "engine-digest-at",
            "engine-digest-lookback_hours",
            "engine-digest-language",
            "engine-digest-account",
        ]
        .map(|id| bounds(cx, id).expect(id));
        for control in [lookback, language, account] {
            assert_eq!(control.size.width, at.size.width);
            assert_eq!(control.right(), at.right());
        }
        let body = bounds(cx, "engine-digest-body").unwrap();
        assert!(bounds(cx, "engine-digest-row-account").unwrap().bottom() <= body.bottom());
    }
    click(cx, "engine-digest-run");
    sent(
        &rig,
        cx,
        r#"cockpit act G ["host","digest"] --source app --json"#,
    );
}

#[gpui::test]
fn a_digest_source_says_what_it_reads_and_one_that_says_nothing_says_its_kind(
    cx: &mut TestAppContext,
) {
    let (_rig, _desk, cx) = open_with_rules(cx, &full());
    click(cx, "engine-digest-row");
    wait_until(cx, "the dialog", |cx| {
        cx.debug_bounds("engine-digest-dialog").is_some()
    });
    assert!(
        bounds(cx, "engine-digest-source-github-reads").is_some(),
        "what the engine says it reads"
    );
    assert!(
        bounds(cx, "engine-digest-source-notion-reads").is_some(),
        "the kind stands where no phrase comes"
    );
}

#[gpui::test]
fn each_source_of_the_digest_has_its_connector_s_mark(cx: &mut TestAppContext) {
    let (_rig, _desk, cx) = open_with_rules(cx, &full());
    click(cx, "engine-digest-row");
    wait_until(cx, "the dialog", |cx| {
        cx.debug_bounds("engine-digest-dialog").is_some()
    });
    for source in ["github", "notion"] {
        let id = Box::leak(format!("engine-digest-source-{source}-mark").into_boxed_str());
        assert!(cx.debug_bounds(id).is_some(), "{source}");
    }
}

#[gpui::test]
fn open_connectors_in_the_digest_dialog_shows_settings_on_the_connectors(cx: &mut TestAppContext) {
    let (_rig, _desk, cx) = open_with_rules(cx, &full());
    click(cx, "engine-digest-row");
    wait_until(cx, "the dialog", |cx| {
        cx.debug_bounds("engine-digest-dialog").is_some()
    });
    click(cx, "engine-digest-connectors");
    wait_until(cx, "the connectors", |cx| {
        cx.debug_bounds("settings-connector-github").is_some()
    });
}

#[gpui::test]
fn the_digest_dialog_has_no_account_choice_with_one_account(cx: &mut TestAppContext) {
    let (_rig, _desk, cx) = open_with_rules(cx, &one_account());
    wait_until(cx, "the digest row", |cx| {
        cx.debug_bounds("engine-digest-row").is_some()
    });
    click(cx, "engine-digest-row");
    wait_until(cx, "the dialog", |cx| {
        cx.debug_bounds("engine-digest-dialog").is_some()
    });
    assert!(bounds(cx, "engine-digest-row-account").is_none());
}

#[gpui::test]
fn asking_the_master_to_add_an_automation_opens_the_master(cx: &mut TestAppContext) {
    let (_rig, desk, cx) = open_with_rules(cx, &full());
    click(cx, "engine-ask-master");
    redraw(cx);
    let shown = desk.read_with(cx, |desk, cx| {
        desk.bar.read(cx).tabs().active().page().clone()
    });
    assert_eq!(shown, Page::Master);
}

#[gpui::test]
fn the_header_counts_the_open_lanes_the_table_lists_and_says_how_many_of_them_run(
    cx: &mut TestAppContext,
) {
    let (_rig, desk, cx) = open_with_rules(cx, &full());
    assert!(bounds(cx, "engine-header").is_some());
    let words = page(&desk, cx).read_with(cx, |page, cx| page.running_words(cx));
    let snapshot = sakura_core::engine::snapshot::Snapshot::parse(full().as_bytes());
    let snapshot = snapshot.expect("the fixture");
    let open = crate::ui::lanes::open_count(&snapshot.items);
    assert!(
        open > 1,
        "the fixture has several open lanes, none of them at work"
    );
    assert_eq!(
        words.as_deref(),
        Some(format!("{open} open lanes, none running, 1 to 4 at once").as_str()),
        "what is counted is said"
    );
    let rows = snapshot
        .items
        .iter()
        .filter(|item| crate::ui::lanes::is_open(item) && !item.is_digest())
        .filter(|item| bounds(cx, &format!("engine-lane-{}", item.task())).is_some())
        .count();
    assert_eq!(
        rows, open,
        "the table lists the same lanes the header counts"
    );
}

#[gpui::test]
fn a_lane_at_work_is_one_of_the_open_lanes_running(cx: &mut TestAppContext) {
    let mut document: serde_json::Value = serde_json::from_str(&full()).expect("the fixture");
    let items = document["items"].as_array_mut().expect("the items");
    let stuck = items
        .iter_mut()
        .find(|item| item["agent_state"] == "blocked");
    stuck.expect("a stuck lane")["agent_state"] = "working".into();
    let lanes = crate::ui::lanes::open_count(
        &sakura_core::engine::snapshot::Snapshot::parse(document.to_string().as_bytes())
            .expect("a snapshot")
            .items,
    );
    let (_rig, desk, cx) = open(cx, &document.to_string());
    let words = page(&desk, cx).read_with(cx, |page, cx| page.running_words(cx));
    let expected = format!("{lanes} open lanes, 1 running, 1 to 4 at once");
    assert_eq!(words.as_deref(), Some(expected.as_str()));
}

#[gpui::test]
fn an_old_engine_gets_the_drawing_read_only_and_a_sentence_for_each_missing_part(
    cx: &mut TestAppContext,
) {
    let (_rig, _desk, cx) = open(cx, SNAPSHOT);
    for fallback in [
        "engine-rules-unreported",
        "engine-pace-unreported",
        "engine-automations-unreported",
    ] {
        assert!(bounds(cx, fallback).is_some(), "{fallback}");
    }
    assert!(bounds(cx, "engine-schema").is_some(), "the drawing stays");
    click(cx, "engine-node-plan");
    redraw(cx);
    assert!(
        bounds(cx, "engine-row-steps.plan.on").is_none(),
        "read only: nothing opens"
    );
    let _ = Node::Plan;
}

#[gpui::test]
fn an_engine_with_no_accounts_says_usage_is_unavailable_and_the_rest_still_draws(
    cx: &mut TestAppContext,
) {
    let snapshot = edited(|snapshot| snapshot["accounts"] = serde_json::json!([]));
    let (_rig, _desk, cx) = open_with_rules(cx, &snapshot);
    assert!(bounds(cx, "engine-usage-unavailable").is_some());
    assert!(bounds(cx, "engine-priority").is_some());
}

#[gpui::test]
fn pace_by_account_says_when_the_usage_was_updated(cx: &mut TestAppContext) {
    let said = edited(|snapshot| {
        snapshot["usage"] =
            serde_json::json!({"as_of": "2026-11-18T07:55:00Z", "now": "x", "providers": []});
    });
    let (_rig, _desk, cx) = open_with_rules(cx, &said);
    assert!(bounds(cx, "engine-usage-updated").is_some());
}

#[gpui::test]
fn pace_by_account_has_no_updated_line_while_no_reading_was_taken(cx: &mut TestAppContext) {
    let unsaid = edited(|snapshot| {
        snapshot["usage"] = serde_json::json!({"as_of": null, "now": "x", "providers": []});
    });
    let (_rig, _desk, cx) = open_with_rules(cx, &unsaid);
    assert!(bounds(cx, "engine-account-personal").is_some());
    assert!(bounds(cx, "engine-usage-updated").is_none());
}

const REFUSED: &str = r#"{"result": "refused", "reason": "the engine said no"}"#;

/// The desk on the Engine page over a fake engine that refuses every action.
fn open_refusing<'a>(
    cx: &'a mut TestAppContext,
    snapshot: &str,
) -> (Rig, Entity<Desk>, &'a mut VisualTestContext) {
    let (rig, desk, cx) = open_answering(cx, snapshot, REFUSED, REFUSED);
    cx.simulate_resize(gpui::size(gpui::px(1280.), gpui::px(2400.)));
    desk.update(cx, |desk, cx| desk.navigate(Page::Engine, false, cx));
    wait_until(cx, "the rules", |cx| {
        cx.debug_bounds("engine-preset-0").is_some()
    });
    (rig, desk, cx)
}

#[gpui::test]
fn pressing_a_control_again_while_it_is_pending_sends_nothing_more(cx: &mut TestAppContext) {
    let (rig, desk, cx) = open_with_rules(cx, &full());
    let acts = page(&desk, cx).read_with(cx, |page, _| page.acts.clone());
    acts.update(cx, |acts, cx| {
        let act = crate::ui::engine_acts::SET_PRIORITY.act(&[("on", "no")]);
        acts.press("engine-priority", act.clone(), "", cx);
        acts.press("engine-priority", act, "", cx);
    });
    wait_until(cx, "the call", |_| !rig.acts().is_empty());
    for _ in 0..10 {
        redraw(cx);
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
    assert_eq!(rig.acts().len(), 1, "{:?}", rig.acts());
}

#[gpui::test]
fn a_refused_pace_puts_the_slider_back_on_the_engine_s_level(cx: &mut TestAppContext) {
    let (_rig, desk, cx) = open_refusing(cx, &full());
    let slider = page(&desk, cx).read_with(cx, |page, _| page.pace.clone());
    slider.update(cx, |slider, cx| {
        cx.emit(SliderEvent::Release(3.0.into()));
        let _ = slider;
    });
    wait_until(cx, "the refusal", |cx| {
        cx.debug_bounds("engine-pace-told").is_some()
    });
    let thumb = slider.read_with(cx, |slider, _| slider.value().start());
    assert_eq!(thumb, 2.0, "the engine's pace is Normal");
}

#[gpui::test]
fn a_refused_rule_says_why_under_that_rule(cx: &mut TestAppContext) {
    let (_rig, _desk, cx) = open_refusing(cx, &full());
    click(cx, "engine-node-plan_review");
    redraw(cx);
    click(cx, "engine-rule-reviews.plan_review.on");
    wait_until(cx, "the refusal", |cx| {
        cx.debug_bounds("engine-rule-reviews.plan_review.on-told")
            .is_some()
    });
}

#[gpui::test]
fn the_preset_form_names_its_field_shows_an_example_and_has_one_primary_button_and_a_cancel(
    cx: &mut TestAppContext,
) {
    let snapshot = edited(|snapshot| snapshot["engine"]["edited"] = true.into());
    let (rig, desk, cx) = open_refusing(cx, &snapshot);
    say_rules(
        &rig,
        &desk,
        &rules_edited(|rules| rules["edited"] = true.into()),
        cx,
    );
    wait_until(cx, "the edited note", |cx| {
        cx.debug_bounds("engine-edited").is_some()
    });
    click(cx, "engine-save-preset");
    wait_until(cx, "the name field", |cx| {
        cx.debug_bounds("engine-save-preset-send").is_some()
    });
    assert!(cx.debug_bounds("engine-save-preset-name-label").is_some());
    assert!(cx.debug_bounds("engine-save-preset-cancel").is_some());
    assert!(
        cx.debug_bounds("engine-save-preset").is_none(),
        "the opening button is not drawn beside the form's own"
    );
    let field = page(&desk, cx).read_with(cx, |page, _| page.preset_name.clone());
    let has_caret = cx
        .update(|window, cx| gpui::Focusable::focus_handle(field.read(cx), cx).is_focused(window));
    assert!(!has_caret, "opening the form gives the field no caret");
    click(cx, "engine-save-preset-cancel");
    wait_until(cx, "the form closes", |cx| {
        cx.debug_bounds("engine-save-preset-send").is_none()
    });
}

#[gpui::test]
fn a_refused_preset_name_is_said_and_the_form_stays_with_what_was_written(cx: &mut TestAppContext) {
    let snapshot = edited(|snapshot| snapshot["engine"]["edited"] = true.into());
    let (rig, desk, cx) = open_refusing(cx, &snapshot);
    say_rules(
        &rig,
        &desk,
        &rules_edited(|rules| rules["edited"] = true.into()),
        cx,
    );
    wait_until(cx, "the edited note", |cx| {
        cx.debug_bounds("engine-edited").is_some()
    });
    click(cx, "engine-save-preset");
    wait_until(cx, "the name field", |cx| {
        cx.debug_bounds("engine-save-preset-send").is_some()
    });
    let field = page(&desk, cx).read_with(cx, |page, _| page.preset_name.clone());
    cx.update(|window, cx| field.update(cx, |field, cx| field.set_value("standard", window, cx)));
    click(cx, "engine-save-preset-send");
    wait_until(cx, "the refusal", |cx| {
        cx.debug_bounds("engine-preset-save-told").is_some()
    });
    assert!(
        bounds(cx, "engine-save-preset-send").is_some(),
        "the form stays"
    );
    assert_eq!(
        field.read_with(cx, |field, _| field.value().to_string()),
        "standard"
    );
}

/// The rules where the plan review is off.
fn rules_without_plan_review() -> String {
    rules_edited(|rules| {
        rules["edited"] = true.into();
        for node in rules["nodes"].as_array_mut().expect("nodes") {
            for rule in node["rules"].as_array_mut().expect("rules") {
                if rule["id"] == "reviews.plan_review.on" {
                    rule["value"] = false.into();
                }
            }
        }
    })
}

#[gpui::test]
fn a_rule_changed_from_a_terminal_is_read_even_while_the_rules_stay_edited(
    cx: &mut TestAppContext,
) {
    let snapshot = edited(|snapshot| snapshot["engine"]["edited"] = true.into());
    let (rig, desk, cx) = open(cx, &snapshot);
    say_rules(
        &rig,
        &desk,
        &rules_edited(|rules| rules["edited"] = true.into()),
        cx,
    );
    let home = tempfile::tempdir().expect("a folder");
    page(&desk, cx).update(cx, |page, _| {
        page.use_engine_home(home.path().to_path_buf())
    });
    rig.say_rules(&rules_without_plan_review());
    std::fs::write(home.path().join("rules.toml"), "[reviews]\n").expect("a file");
    rig.facts.update(cx, |facts, cx| facts.read(cx));
    wait_until(cx, "the node drawn off", |cx| {
        cx.debug_bounds("engine-node-plan_review-off").is_some()
    });
}

#[gpui::test]
fn a_failed_read_of_the_rules_is_tried_again_at_the_next_look(cx: &mut TestAppContext) {
    let (rig, desk, cx) = open_with_rules(cx, &full());
    rig.say_rules("not json");
    page(&desk, cx).update(cx, |page, cx| page.read_rules(cx));
    redraw(cx);
    assert!(
        bounds(cx, "engine-preset-0").is_some(),
        "what was shown stays"
    );
    rig.say_rules(&rules_without_plan_review());
    wait_until(cx, "the retry", |cx| {
        rig.facts.update(cx, |facts, cx| facts.read(cx));
        cx.debug_bounds("engine-node-plan_review-off").is_some()
    });
}

#[gpui::test]
fn the_digest_dialog_reads_the_top_level_settings_and_its_switches_use_yes_and_no(
    cx: &mut TestAppContext,
) {
    let (rig, _desk, cx) = open_with_rules(cx, &full());
    click(cx, "engine-digest-row");
    wait_until(cx, "the dialog", |cx| {
        cx.debug_bounds("engine-digest-dialog").is_some()
    });
    assert!(bounds(cx, "engine-digest-row-at").is_some(), "its time");
    click(cx, "engine-digest-source-github");
    sent(
        &rig,
        cx,
        r#"cockpit act J ["host","digest-settings"] --name github --on no --source app --json"#,
    );
    click(cx, "engine-digest-on");
    sent(
        &rig,
        cx,
        &format!("cockpit act A {AUTOMATIONS} --name morning-digest --on no --source app --json"),
    );
}

#[gpui::test]
fn the_presets_offered_are_the_ones_the_snapshot_lists_and_the_chosen_one_is_marked(
    cx: &mut TestAppContext,
) {
    let (rig, _desk, cx) = open_with_rules(cx, &full());
    assert!(bounds(cx, "engine-preset-3").is_some(), "four presets");
    assert!(bounds(cx, "engine-preset-4").is_none(), "no legacy name");
    rig.say_snapshot(&edited(|snapshot| {
        let presets = serde_json::json!([
            {"name": "solo", "label": "Solo", "description": "One agent.", "chosen": false},
            {"name": "standard", "label": "Standard", "description": "The team.", "chosen": true}]);
        snapshot["engine"]["presets"] = presets;
    }));
    rig.facts.update(cx, |facts, cx| facts.read(cx));
    wait_until(cx, "two presets", |cx| {
        cx.debug_bounds("engine-preset-3").is_none()
    });
    assert!(bounds(cx, "engine-preset-1").is_some());
}

/// What the engine printed for its rules, with its own ids (`claude:sonnet`) and Build's fixed switch.
const REAL_RULES: &str =
    include_str!("../../../../sakura-core/tests/fixtures/engine-real-rules.json");

#[gpui::test]
fn the_engine_s_fixed_build_switch_is_no_row_and_the_models_are_drawn_by_name(
    cx: &mut TestAppContext,
) {
    let (rig, desk, cx) = open_with_rules(cx, &full());
    say_rules(&rig, &desk, REAL_RULES, cx);
    wait_until(cx, "the real rules", |cx| {
        cx.debug_bounds("engine-row-steps.build.agent").is_some()
    });
    assert!(bounds(cx, "engine-node-build-model").is_some());
    assert!(
        bounds(cx, "engine-row-steps.build.on").is_none(),
        "no row that says Build and true"
    );
    let agent = page(&desk, cx).read_with(cx, |page, _| page.agent_line_for_test(Node::Build));
    assert_eq!(agent.as_deref(), Some("Claude, Sonnet medium"));
    assert!(
        bounds(cx, "engine-row-builders").is_some(),
        "its other rules stay"
    );
}

#[gpui::test]
fn a_model_menu_shows_names_and_sends_the_engine_s_id(cx: &mut TestAppContext) {
    let (rig, desk, cx) = open_with_rules(cx, &full());
    let mut ready: serde_json::Value = serde_json::from_str(REAL_RULES).expect("the real rules");
    ready["agents"][0]["models"] = serde_json::json!([{"id": "haiku", "label": "Haiku"}]);
    say_rules(&rig, &desk, &ready.to_string(), cx);
    wait_until(cx, "the model catalogue", |cx| {
        cx.debug_bounds("engine-rule-steps.build.agent").is_some()
    });
    click(cx, "engine-rule-steps.build.agent");
    wait_until(cx, "the Haiku menu entry", |cx| {
        cx.debug_bounds("chip-entry-2-0").is_some()
    });
    click(cx, "chip-entry-2-0");
    sent(
        &rig,
        cx,
        &format!("cockpit act K {ENGINE} --rule steps.build.model --value claude:haiku --source app --json"),
    );
}

fn acts_of(
    desk: &Entity<Desk>,
    cx: &mut VisualTestContext,
) -> Entity<crate::ui::engine_acts::EngineActs> {
    page(desk, cx).read_with(cx, |page, _| page.acts.clone())
}

fn shown(
    acts: &Entity<crate::ui::engine_acts::EngineActs>,
    key: &str,
    cx: &mut VisualTestContext,
) -> Option<String> {
    acts.read_with(cx, |acts, _| acts.shown(key).map(str::to_string))
}

/// Lets a stepper's pause end: what was pressed so far is sent.
fn let_the_pause_end(cx: &mut VisualTestContext) {
    cx.executor()
        .advance_clock(crate::ui::engine_acts::COALESCE_PAUSE * 2);
    cx.run_until_parked();
}

#[gpui::test]
fn ten_quick_presses_on_a_stepper_send_one_act_with_the_final_value(cx: &mut TestAppContext) {
    let (rig, desk, cx) = open_with_rules(cx, &full());
    for _ in 0..10 {
        click(cx, "engine-lanes-max-more");
    }
    let acts = acts_of(&desk, cx);
    assert_eq!(shown(&acts, "engine-lanes-max", cx).as_deref(), Some("14"));
    assert!(
        rig.acts().is_empty(),
        "nothing is sent while the presses go on"
    );
    let_the_pause_end(cx);
    sent(
        &rig,
        cx,
        &format!("cockpit act K {ENGINE} --rule lanes --value 14 --source app --json"),
    );
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert_eq!(rig.acts().len(), 1, "{:?}", rig.acts());
}

#[gpui::test]
fn a_stepper_keeps_its_confirmed_value_while_the_engine_answers(cx: &mut TestAppContext) {
    let (rig, desk, cx) = open_with_rules(cx, &full());
    let hold = rig.dir.path().join("hold-actions");
    std::fs::File::create(&hold).expect("hold the engine reply");
    click(cx, "engine-lanes-max-more");
    let_the_pause_end(cx);
    redraw(cx);
    let acts = acts_of(&desk, cx);
    assert_eq!(shown(&acts, "engine-lanes-max", cx).as_deref(), Some("5"));
    assert!(bounds(cx, "engine-lanes-max-value-4").is_some());
    assert!(bounds(cx, "engine-lanes-max-pending").is_some());
    assert!(rig.acts().is_empty(), "the engine is still answering");
    std::fs::remove_file(hold).expect("allow the engine reply");
    wait_until(cx, "the engine's answer", |cx| {
        acts.read_with(cx, |acts, _| acts.step("engine-lanes-max").is_none())
    });
    wait_until(cx, "the engine read again", |cx| {
        shown(&acts, "engine-lanes-max", cx).is_none()
    });
}

#[gpui::test]
fn a_refused_stepper_goes_back_to_the_engine_s_value_and_says_why(cx: &mut TestAppContext) {
    let (_rig, desk, cx) = open_refusing(cx, &full());
    click(cx, "engine-lanes-max-more");
    let acts = acts_of(&desk, cx);
    assert_eq!(shown(&acts, "engine-lanes-max", cx).as_deref(), Some("5"));
    let_the_pause_end(cx);
    wait_until(cx, "the refusal", |cx| {
        cx.debug_bounds("engine-lanes-max-told").is_some()
    });
    assert_eq!(shown(&acts, "engine-lanes-max", cx), None);
}

#[gpui::test]
fn the_slider_let_go_twice_sends_both_levels_in_order_and_keeps_the_last_shown(
    cx: &mut TestAppContext,
) {
    let (rig, desk, cx) = open_with_rules(cx, &full());
    rig.say_delay("1");
    let slider = page(&desk, cx).read_with(cx, |page, _| page.pace.clone());
    slider.update(cx, |_, cx| cx.emit(SliderEvent::Release(3.0.into())));
    slider.update(cx, |_, cx| cx.emit(SliderEvent::Release(4.0.into())));
    let acts = acts_of(&desk, cx);
    assert_eq!(shown(&acts, "engine-pace", cx).as_deref(), Some("fastest"));
    wait_until(cx, "both calls", |_| rig.acts().len() == 2);
    assert_eq!(
        rig.acts(),
        [
            format!("cockpit act Q {ENGINE} --level fast --source app --json"),
            format!("cockpit act Q {ENGINE} --level fastest --source app --json"),
        ]
    );
}

#[gpui::test]
fn the_phone_page_is_a_service_next_to_the_keeper_and_its_switch_sends_b(cx: &mut TestAppContext) {
    let (rig, _desk, cx) = open_with_rules(cx, &full());
    let keeper = bounds(cx, "engine-service-keeper").expect("the keeper");
    let phone = bounds(cx, "engine-service-phone-page").expect("the phone page");
    assert!(
        keeper.top() < phone.top(),
        "the phone page follows the keeper"
    );
    click(cx, "engine-service-phone-page");
    sent(
        &rig,
        cx,
        &format!("cockpit act B {SERVICES} --name phone-page --on no --source app --json"),
    );
}

#[gpui::test]
fn the_engine_page_scrolls_with_the_keys_when_nothing_has_the_keyboard(cx: &mut TestAppContext) {
    let (_rig, desk, cx) = open_with_rules(cx, &full());
    // A window lower than the page, so that there is something to scroll to.
    cx.simulate_resize(gpui::size(gpui::px(1280.), gpui::px(500.)));
    redraw(cx);
    let scrolled = |cx: &mut VisualTestContext| {
        page(&desk, cx).read_with(cx, |page, _| f32::from(page.scroll.scrolled()))
    };
    assert_eq!(scrolled(cx), 0.0);
    cx.simulate_keystrokes("pagedown");
    let after_a_page = scrolled(cx);
    assert!(after_a_page > 0.0, "Page Down moved the page");
    cx.simulate_keystrokes("end");
    let at_the_end = scrolled(cx);
    assert!(at_the_end > after_a_page, "End went to the end");
    cx.simulate_keystrokes("pageup");
    assert!(scrolled(cx) < at_the_end, "Page Up moved it back");
    cx.simulate_keystrokes("home");
    assert_eq!(scrolled(cx), 0.0, "Home went to the top");
    cx.simulate_keystrokes("space");
    assert!(scrolled(cx) > 0.0, "Space moved the page down");
    cx.simulate_keystrokes("shift-space");
    assert_eq!(scrolled(cx), 0.0, "Shift-Space moved it up");
    cx.simulate_keystrokes("down");
    assert!(scrolled(cx) > 0.0, "an arrow moved it");
}

#[gpui::test]
fn the_phone_page_says_its_port_only_while_it_is_on(cx: &mut TestAppContext) {
    let (_rig, _desk, cx) = open_phone(cx);
    assert!(bounds(cx, "engine-service-phone-page-address").is_some());
    assert!(
        bounds(cx, "engine-service-keeper-address").is_none(),
        "only the phone page has one"
    );
}

#[gpui::test]
fn a_phone_page_that_is_off_or_has_no_port_says_no_address(cx: &mut TestAppContext) {
    let off = edited(|snapshot| {
        let items = snapshot["automations"]["items"]
            .as_array_mut()
            .expect("items");
        for item in items.iter_mut().filter(|item| item["name"] == "phone-page") {
            item["enabled"] = false.into();
        }
    });
    let (_rig, _desk, cx) = open_with_rules(cx, &off);
    wait_until(cx, "phone setup", |cx| {
        bounds(cx, "engine-phone-link").is_some()
    });
    assert!(bounds(cx, "engine-service-phone-page").is_some());
    assert!(
        bounds(cx, "engine-service-phone-page-address").is_none(),
        "off"
    );
}

#[gpui::test]
fn a_phone_page_with_no_reported_port_says_no_address(cx: &mut TestAppContext) {
    let (rig, desk, cx) = open_phone(cx);
    rig.say_phone(r#"{"state":"ready","enabled":true}"#);
    page(&desk, cx).update(cx, |page, cx| page.read_phone(cx));
    wait_until(cx, "the unreported address", |cx| {
        bounds(cx, "engine-service-phone-page-address").is_none()
    });
}

/// The Engine page over the fixture engine, scrolled by its tall window to the phone page's row.
fn open_phone(cx: &mut TestAppContext) -> (Rig, Entity<Desk>, &mut VisualTestContext) {
    let (rig, desk, cx) = open(cx, &full());
    wait_until(cx, "the phone setup", |cx| {
        cx.debug_bounds("engine-phone-link").is_some()
    });
    (rig, desk, cx)
}

#[gpui::test]
fn the_phone_port_comes_from_the_engine_response(cx: &mut TestAppContext) {
    let (_rig, _desk, cx) = open_phone(cx);
    assert!(bounds(cx, "engine-service-phone-page-address").is_some());
}

#[gpui::test]
fn the_phone_page_shows_the_full_link_the_engine_knows_and_who_may_open_it(
    cx: &mut TestAppContext,
) {
    let (_rig, _desk, cx) = open_phone(cx);
    assert!(
        bounds(cx, "engine-phone-copy").is_some(),
        "the link is copyable"
    );
    assert!(bounds(cx, "engine-phone-open").is_some(), "and openable");
    assert!(
        bounds(cx, "engine-phone-opens-from").is_some(),
        "one line says who it opens for"
    );
    assert!(
        bounds(cx, "engine-phone-device-n2").is_some(),
        "the allowed device is listed by name"
    );
    assert!(
        bounds(cx, "engine-phone-device-n3").is_none(),
        "one not allowed is not"
    );
}

#[gpui::test]
fn removing_an_allowed_device_sends_z_with_its_id_and_allowing_one_sends_f(
    cx: &mut TestAppContext,
) {
    let (rig, _desk, cx) = open_phone(cx);
    click(cx, "engine-phone-remove-n2");
    sent(
        &rig,
        cx,
        &format!("cockpit act Z {ENGINE} --device n2 --source app --json"),
    );
    click(cx, "engine-phone-allow");
    cx.run_until_parked();
    // The menu lists the devices that may not open it yet: this Mac, then the tablet.
    redraw(cx);
    cx.dispatch_action(gpui_base::actions::SelectDown);
    cx.dispatch_action(gpui_base::actions::SelectDown);
    cx.dispatch_action(gpui_base::actions::Confirm { secondary: false });
    sent(
        &rig,
        cx,
        &format!("cockpit act F {ENGINE} --device n3 --source app --json"),
    );
}

#[gpui::test]
fn a_setup_that_is_not_ready_says_its_state_in_the_engines_words(cx: &mut TestAppContext) {
    let (rig, desk, cx) = open(cx, &full());
    rig.say_phone(
        r#"{"state": "logged_out", "words": "Tailscale is installed but signed out. Sign in, then come back here.", "devices": []}"#,
    );
    page(&desk, cx).update(cx, |page, cx| page.read_phone(cx));
    wait_until(cx, "the engine's signed-out state", |cx| {
        bounds(cx, "engine-phone-words").is_some() && cx.debug_bounds("engine-phone-link").is_none()
    });
    assert!(
        bounds(cx, "engine-phone-words").is_some(),
        "the engine's words stay"
    );
}

/// The fixture's rules where the build step's agent is the account "a" ("Claude"), whose program lists two models.
fn rules_with_the_agent_of_build() -> String {
    rules_edited(|rules| {
        rules["agents"] = serde_json::json!([
            {"id": "a", "label": "Claude", "provider": "claude", "models": [
                {"id": "opus", "label": "Opus 5.5", "efforts": ["low", "high"], "default_effort": "high"},
                {"id": "sonnet", "label": "Sonnet 5.5", "efforts": ["low", "high"], "default_effort": "high"}]}
        ]);
        for node in rules["nodes"].as_array_mut().expect("nodes") {
            for rule in node["rules"].as_array_mut().expect("rules") {
                if rule["id"] == "steps.build.model" {
                    rule["value"] = "claude:sonnet".into();
                    rule["agent"] = serde_json::json!({"account": "a", "provider": "claude",
                        "model": "sonnet", "effort": "high"});
                }
            }
        }
    })
}

#[gpui::test]
fn a_step_with_an_agent_has_one_menu_of_account_model_and_effort_and_a_pick_sends_the_rule(
    cx: &mut TestAppContext,
) {
    let (rig, desk, cx) = open_with_rules(cx, &full());
    say_rules(&rig, &desk, &rules_with_the_agent_of_build(), cx);
    wait_until(cx, "the agent row", |cx| {
        cx.debug_bounds("engine-row-steps.build.agent").is_some()
    });
    for separate in ["model", "account", "effort"] {
        assert!(
            bounds(cx, &format!("engine-row-steps.build.{separate}")).is_none(),
            "{separate} is in the one menu"
        );
    }
    click(cx, "engine-rule-steps.build.agent");
    redraw(cx);
    // The account is the first entry, then the models: the second down is Opus.
    cx.dispatch_action(gpui_base::actions::SelectDown);
    cx.dispatch_action(gpui_base::actions::SelectDown);
    cx.dispatch_action(gpui_base::actions::Confirm { secondary: false });
    sent(
        &rig,
        cx,
        &format!("cockpit act K {ENGINE} --rule steps.build.model --value claude:opus --source app --json"),
    );
}

/// The same rules as [`rules_with_the_agent_of_build`], before the engine has asked the programs for their models.
fn rules_with_the_agent_of_build_and_no_models() -> String {
    let mut rules: serde_json::Value =
        serde_json::from_str(&rules_with_the_agent_of_build()).expect("rules");
    rules["agents"][0]["models"] = serde_json::json!([]);
    rules.to_string()
}

#[gpui::test]
fn empty_model_lists_are_asked_for_once_on_the_page_and_the_menu_shows_them_when_they_arrive(
    cx: &mut TestAppContext,
) {
    let (rig, desk, cx) = open_with_rules(cx, &full());
    rig.say_delay("1");
    say_rules(
        &rig,
        &desk,
        &rules_with_the_agent_of_build_and_no_models(),
        cx,
    );
    wait_until(cx, "the agent row", |cx| {
        cx.debug_bounds("engine-row-steps.build.agent").is_some()
    });
    rig.say_rules(&rules_with_the_agent_of_build());
    sent(
        &rig,
        cx,
        &format!("cockpit act M {ENGINE} --source app --json"),
    );
    wait_until(cx, "the models", |cx| {
        redraw(cx);
        page(&desk, cx).read_with(cx, |page, _| page.has_models_for_test())
    });
    assert_eq!(
        rig.acts()
            .iter()
            .filter(|act| act.contains(" act M "))
            .count(),
        1,
        "the models are requested once"
    );
}

#[gpui::test]
fn the_label_of_a_step_s_agent_row_is_never_squeezed_by_the_chip_beside_it(
    cx: &mut TestAppContext,
) {
    let (_rig, desk, cx) = open_with_rules(cx, &full());
    let mut rules: serde_json::Value =
        serde_json::from_str(&rules_with_the_agent_of_build()).expect("rules");
    rules["agents"][0]["label"] = "Claude with a long account name".into();
    rules["agents"][0]["models"][1]["label"] = "Sonnet with a very long model name".into();
    say_rules(&_rig, &desk, &rules.to_string(), cx);
    wait_until(cx, "the agent row", |cx| {
        cx.debug_bounds("engine-row-steps.build.agent-name")
            .is_some()
    });
    let name = bounds(cx, "engine-row-steps.build.agent-name").expect("the label");
    assert!(
        name.size.width >= gpui::px(40.),
        "the label {name:?} keeps the room of its words"
    );
}

const FALLBACK_NOTE: &str =
    "No Claude account exists, so the builder runs on the Codex account (Codex).";

/// The rules with the engine's note on the agent of `rule`: its provider has no account.
fn rules_with_note(rule_id: &'static str) -> String {
    rules_edited(|rules| {
        for node in rules["nodes"].as_array_mut().expect("nodes") {
            for rule in node["rules"].as_array_mut().expect("rules") {
                if rule["id"] == rule_id {
                    rule["agent"] = serde_json::json!({"account": "project", "provider": "codex",
                        "model": "gpt", "effort": "high", "note": FALLBACK_NOTE});
                }
            }
        }
    })
}

#[gpui::test]
fn a_step_whose_provider_has_no_account_says_so_in_the_engine_s_words_inside_its_box(
    cx: &mut TestAppContext,
) {
    let (rig, desk, cx) = open_with_rules(cx, &full());
    for (rule, node) in [
        ("steps.build.model", "build"),
        ("reviews.task_review.model", "task_review"),
    ] {
        say_rules(&rig, &desk, &rules_with_note(rule), cx);
        let selector: &'static str = Box::leak(format!("engine-node-{node}-note").into_boxed_str());
        wait_until(cx, "the note", |cx| cx.debug_bounds(selector).is_some());
        let note = bounds(cx, &format!("engine-node-{node}-note")).expect("the note");
        let node_box = bounds(cx, &format!("engine-node-{node}")).expect("the box");
        assert!(
            note.bottom() <= node_box.bottom() && note.right() <= node_box.right(),
            "the note of {node} lies inside its box: {note:?} in {node_box:?}"
        );
    }
}

#[gpui::test]
fn a_step_that_runs_on_its_own_provider_has_no_note_line(cx: &mut TestAppContext) {
    let (_rig, _desk, cx) = open_with_rules(cx, &full());
    for node in ["build", "task_review", "plan"] {
        assert!(
            bounds(cx, &format!("engine-node-{node}-note")).is_none(),
            "{node}"
        );
    }
}

#[gpui::test]
fn with_one_account_added_by_path_the_step_boxes_say_its_leeway_name_and_never_its_id(
    cx: &mut TestAppContext,
) {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let snapshot = edited(|snapshot| {
        snapshot["accounts"] = serde_json::json!([{
            "id": "claude-aa0b79cab9b0ea15", "key": "claude-aa0b79cab9b0ea15", "label": "Claude",
            "provider": "claude", "enabled": true, "hidden": false
        }]);
    });
    let (rig, desk, cx) = open_with_rules(cx, &snapshot);
    let rules =
        include_str!("../../../../sakura-core/tests/fixtures/engine-rules-one-account.json");
    say_rules(&rig, &desk, rules, cx);
    wait_until(cx, "the rules", |cx| {
        page(&desk, cx).read_with(cx, |page, _| {
            page.agent_line_for_test(super::schema::Node::Build)
                .is_some()
        })
    });
    let line = page(&desk, cx).read_with(cx, |page, _| {
        page.agent_line_for_test(super::schema::Node::Build)
    });
    assert_eq!(line.as_deref(), Some("Claude, Haiku medium"));
}

#[gpui::test]
fn once_the_engine_drops_an_edit_that_went_back_to_the_preset_s_value_the_page_shows_no_edited_and_no_dot(
    cx: &mut TestAppContext,
) {
    let snapshot = edited(|snapshot| snapshot["engine"]["edited"] = true.into());
    let (rig, desk, cx) = open(cx, &snapshot);
    say_rules(&rig, &desk, &rules_changed("steps.build.model"), cx);
    wait_until(cx, "the edited note", |cx| {
        cx.debug_bounds("engine-edited").is_some()
            && cx.debug_bounds("engine-node-build-edited").is_some()
    });
    // The engine (its commit 3c0c3db) drops the key of a rule set back to what the preset gives: the snapshot and the
    // rules say it is no longer edited.
    rig.say_snapshot(&full());
    rig.facts.update(cx, |facts, cx| facts.read(cx));
    say_rules(&rig, &desk, &rules_edited(|_| {}), cx);
    wait_until(cx, "no edited note", |cx| {
        redraw(cx);
        cx.debug_bounds("engine-edited").is_none()
            && cx.debug_bounds("engine-node-build-edited").is_none()
    });
}

#[gpui::test]
fn polish_zd_pipeline_node_allocates_its_account_and_model_separate_lines(cx: &mut TestAppContext) {
    let (rig, desk, cx) = open_with_rules(cx, &full());
    say_rules(&rig, &desk, &rules_with_the_agent_of_build(), cx);
    wait_until(cx, "the agent rule", |cx| {
        cx.debug_bounds("engine-row-steps.build.agent").is_some()
    });
    let account = bounds(cx, "engine-node-build-account").expect("account line");
    let model = bounds(cx, "engine-node-build-model").expect("model line");
    assert!(model.top() >= account.bottom());
}
