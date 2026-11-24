//! The Inbox: its grouping and its verbs as pure facts, then the page on GPUI's test platform over a fake
//! `sakura`.

use super::model::{self, Say};
use super::pending::{HeldAct, HeldActs};
use super::{InboxEvent, InboxPage, Shown};
use crate::ui::answers::Answers;
use crate::ui::engine_rig_tests::{self, wait_until, Rig, SNAPSHOT};
use crate::ui::kit::Toast;
use gpui::{
    div, AppContext as _, Context, Entity, IntoElement, Modifiers, ParentElement as _, Render,
    Styled as _, TestAppContext, VisualTestContext, Window,
};
use sakura_core::engine::snapshot::Snapshot;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::Duration;

fn parse(text: &str) -> Snapshot {
    Snapshot::parse(text.as_bytes()).expect("a snapshot")
}

fn wait(
    task: &str,
    kind: &str,
    since: Option<&str>,
    actions: &[(&str, &[&str], &[&str])],
) -> String {
    let actions: Vec<_> = actions
        .iter()
        .map(|(key, needs, optional)| {
            serde_json::json!({"key": key, "label": key, "needs": needs, "optional": optional, "confirm": false})
        })
        .collect();
    serde_json::json!({
        "id": format!("[\"wait\",\"/work/repo\",\"{task}\",\"{kind}\",\"{task}\"]"),
        "key": ["wait", "/work/repo", task, kind, task], "kind": kind, "repo": "/work/repo",
        "project": "repo", "task": task, "title": format!("Ask of {task}"), "detail": "",
        "since": since, "actions": actions
    })
    .to_string()
}

fn snapshot_of(waits: &[String]) -> Snapshot {
    parse(&format!(
        r#"{{"api": 1, "waits": [{}], "items": [], "events": [], "problems": []}}"#,
        waits.join(",")
    ))
}

#[test]
fn groups_are_ordered_by_their_newest_ask_and_each_lane_lists_its_asks_newest_first() {
    let snapshot = snapshot_of(&[
        wait("old", "approval", Some("2026-11-18T08:00:00Z"), &[]),
        wait("new", "approval", Some("2026-11-18T10:00:00Z"), &[]),
        wait("none", "approval", None, &[]),
    ]);
    let tasks: Vec<&str> = model::groups(&snapshot)
        .iter()
        .map(|group| group.asks[0].task.as_str())
        .collect();
    assert_eq!(tasks, ["new", "old", "none"]);
}

#[test]
fn the_fixture_gives_a_group_for_each_lane_with_an_ask() {
    let snapshot = parse(SNAPSHOT);
    let groups = model::groups(&snapshot);
    let counts: Vec<usize> = groups.iter().map(|group| group.asks.len()).collect();
    assert_eq!(counts.iter().sum::<usize>(), 5);
    assert_eq!(groups.len(), 4, "gated, stuck, demo and the outside lane");
    let outside = groups
        .iter()
        .find(|group| group.asks.len() == 2)
        .expect("the outside lane");
    assert_eq!(outside.title.as_ref(), "Add --help to measure.py");
}

#[test]
fn a_titled_lane_heads_its_group_and_an_untitled_lane_uses_the_sidebar_name() {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let mut document: serde_json::Value = serde_json::from_str(SNAPSHOT).expect("the fixture");
    for item in document["items"].as_array_mut().expect("the items") {
        if item["key"][2] == "gated" {
            item["title"] = "Gate the uploads".into();
        }
    }
    let snapshot = parse(&document.to_string());
    let titles: Vec<String> = model::groups(&snapshot)
        .iter()
        .map(|group| group.title.to_string())
        .collect();
    assert!(
        titles.contains(&"Gate the uploads".to_string()),
        "{titles:?}"
    );
    assert!(titles.contains(&"demo".to_string()), "{titles:?}");
    let spec = snapshot
        .waits
        .iter()
        .find(|wait| wait.key_name() == "spec")
        .expect("the spec ask");
    let said = super::detail::ask_title(spec, &snapshot);
    assert!(said.contains("Gate the uploads"), "{said}");
}

#[test]
fn an_ask_whose_lane_is_not_listed_is_headed_by_the_lane_title_the_ask_carries_else_its_id() {
    let mut titled: serde_json::Value =
        serde_json::from_str(&wait("add-search", "approval", None, &[])).unwrap();
    titled["lane_title"] = "Add search to the orchard".into();
    let untitled = wait("fix-login", "approval", None, &[]);
    let snapshot = snapshot_of(&[titled.to_string(), untitled]);
    let mut titles: Vec<String> = model::groups(&snapshot)
        .iter()
        .map(|group| group.title.to_string())
        .collect();
    titles.sort();
    assert_eq!(titles, ["Add search to the orchard", "fix-login"]);
}

#[test]
fn the_asks_of_the_morning_digest_form_a_group_titled_with_its_date() {
    let mut digest = serde_json::from_str::<serde_json::Value>(&wait(
        "d1",
        "owner_row",
        Some("2026-11-17T07:02:00Z"),
        &[],
    ))
    .unwrap();
    digest["source"] = "outside".into();
    digest["author"] = "outside:digest".into();
    digest["digest"] = "2026-11-16".into();
    digest["project"] = "".into();
    let snapshot = snapshot_of(&[digest.to_string()]);
    let groups = model::groups(&snapshot);
    assert_eq!(groups[0].title.as_ref(), "Morning digest");
    assert_eq!(
        groups[0].date.as_deref(),
        Some("2026-11-16"),
        "the day the engine names, not the day the ask was raised"
    );
}

#[test]
fn a_proposal_of_the_master_is_in_the_group_named_master() {
    let snapshot = snapshot_of(&[wait("master", "proposal", None, &[])]);
    assert_eq!(model::groups(&snapshot)[0].title.as_ref(), "Master");
}

fn verbs_of(kind: &str, actions: &[(&str, &[&str], &[&str])]) -> model::Verbs {
    let snapshot = snapshot_of(&[wait("t", kind, None, actions)]);
    model::verbs(&snapshot.waits[0], None)
}

#[test]
fn approve_is_the_primary_verb_of_an_approval() {
    let verbs = verbs_of("approval", &[("a", &[], &[])]);
    assert_eq!(verbs.primary.expect("a primary").say, Say::Approve);
    assert!(verbs.secondary.is_empty() && verbs.reply.is_none());
}

#[test]
fn always_approve_follows_only_on_the_checks_gate_that_may_be_remembered() {
    let checks = serde_json::json!({
        "id": "x", "key": ["wait", "/work/repo", "t", "approval", "checks"], "kind": "approval",
        "repo": "/work/repo", "project": "repo", "task": "t", "title": "T", "since": null,
        "actions": [{"key": "a", "label": "Approve", "needs": [], "optional": ["remember"], "confirm": true}]
    });
    let snapshot = snapshot_of(&[checks.to_string()]);
    let verbs = model::verbs(&snapshot.waits[0], None);
    assert_eq!(verbs.secondary.len(), 1);
    assert_eq!(verbs.secondary[0].say, Say::AlwaysApprove);
    assert_eq!(
        verbs.secondary[0]
            .inputs
            .get("remember")
            .map(String::as_str),
        Some("yes")
    );
    let spec = verbs_of("approval", &[("a", &[], &["remember"])]);
    assert!(spec.secondary.is_empty(), "the gate is not the checks");
}

#[test]
fn a_mark_is_approve_with_verdict_ok_and_decline_with_verdict_ko_that_takes_a_note() {
    let verbs = verbs_of("owner_row", &[("k", &["verdict"], &["note"])]);
    let primary = verbs.primary.expect("a primary");
    assert_eq!(primary.say, Say::Approve);
    assert_eq!(
        primary.inputs.get("verdict").map(String::as_str),
        Some("ok")
    );
    let decline = &verbs.secondary[0];
    assert_eq!(decline.say, Say::Decline);
    assert_eq!(
        decline.inputs.get("verdict").map(String::as_str),
        Some("ko")
    );
    assert!(decline.takes_note);
}

#[test]
fn accept_and_reject_of_a_proposal_are_approve_and_decline_without_a_note() {
    let verbs = verbs_of("proposal", &[("y", &[], &[]), ("N", &[], &[])]);
    assert_eq!(verbs.primary.expect("a primary").action.key, "y");
    let decline = &verbs.secondary[0];
    assert_eq!(
        (decline.say, decline.action.key.as_str()),
        (Say::Decline, "N")
    );
    assert!(!decline.takes_note, "the engine's reject takes no note");
}

#[test]
fn a_question_offers_its_choices_and_a_reply() {
    let mut question = serde_json::from_str::<serde_json::Value>(&wait(
        "q",
        "question",
        None,
        &[("r", &[], &["choice", "text"])],
    ))
    .unwrap();
    question["choices"] = serde_json::json!(["yes", "no"]);
    let snapshot = snapshot_of(&[question.to_string()]);
    let verbs = model::verbs(&snapshot.waits[0], None);
    assert!(verbs.primary.is_none());
    assert_eq!(verbs.choices, ["yes", "no"]);
    assert_eq!(verbs.reply.expect("a reply").key, "r");
}

#[test]
fn a_blocked_ask_is_resumed_or_stopped_through_its_lane_s_own_actions() {
    let snapshot = parse(SNAPSHOT);
    let blocked = snapshot
        .waits
        .iter()
        .find(|wait| wait.kind == "blocked")
        .expect("blocked");
    let lane = model::lane_of(blocked, &snapshot).expect("its lane");
    let verbs = model::verbs(blocked, Some(lane));
    let primary = verbs.primary.expect("a primary");
    assert_eq!(
        (primary.say, primary.action.key.as_str()),
        (Say::Resume, "R")
    );
    assert_eq!(primary.target, lane.id);
    assert_eq!(verbs.secondary[0].say, Say::Stop);
    assert!(
        model::verbs(blocked, None).primary.is_none(),
        "no lane, no verb"
    );
}

#[test]
fn kinds_and_answers_are_said_in_the_interface_s_words() {
    assert_eq!(model::kind_words("owner_row").as_ref(), "To look at");
    assert_eq!(model::kind_words("blocked").as_ref(), "Blocked");
    assert_eq!(model::kind_words("something_new").as_ref(), "Other");
    let decided = |answer: &str, choice: Option<&str>| {
        let text = serde_json::json!({"api": 1, "waits": [], "items": [], "events": [], "problems": [],
            "decided": [{"id": "w", "project": "p", "task": "t", "answer": answer, "choice": choice}]});
        parse(&text.to_string()).decided.expect("decided").remove(0)
    };
    assert_eq!(
        model::answer_words(&decided("mark_ok", None)).as_ref(),
        "Done"
    );
    assert_eq!(
        model::answer_words(&decided("mark_ko", None)).as_ref(),
        "Not now"
    );
    assert_eq!(
        model::answer_words(&decided("approve", None)).as_ref(),
        "Approved"
    );
    assert_eq!(
        model::answer_words(&decided("reject", None)).as_ref(),
        "Declined"
    );
    assert_eq!(
        model::answer_words(&decided("choice", Some("Keep it"))).as_ref(),
        "Answered: Keep it"
    );
    assert_eq!(
        model::answer_words(&decided("novel", None)).as_ref(),
        "Answered"
    );
}

#[test]
fn the_time_of_an_ask_is_the_clock_time_it_was_raised_at_and_none_without_one() {
    let now = sakura_core::graph::parse_time("2026-11-18T12:00:00Z").expect("a time");
    let said = model::clock_words(Some("2026-11-18T10:00:00Z"), now);
    let offset = crate::ui::engine_page::reset::local_offset(now);
    let hours = 10 + offset / 3_600;
    if (0..24).contains(&hours) && offset % 3_600 == 0 {
        assert_eq!(said.as_deref(), Some(format!("{hours:02}:00").as_str()));
    }
    assert_eq!(model::clock_words(None, now), None);
}

#[test]
fn an_ask_of_the_day_before_is_said_yesterday_and_an_older_one_by_its_weekday() {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let at = |text: &str| sakura_core::graph::parse_time(text).expect("a time");
    let now = at("2026-11-18T12:00:00Z");
    let said = |text: &str| model::clock_words_in(at(text), now, 0);
    assert_eq!(said("2026-11-17T11:57:00Z"), "Yesterday 11:57");
    assert_eq!(said("2026-11-18T09:30:00Z"), "09:30");
    assert_eq!(said("2026-11-16T11:57:00Z"), "Mon 11:57");
}

const APPROVAL: &str = r#"["wait","/work/repo","gated","approval","spec"]"#;
const OWNER_ROW: &str = r#"["wait","/work/repo","demo","owner_row","R1"]"#;
const BLOCKED: &str = r#"["wait","/work/other","stuck","blocked","blocked"]"#;
const QUESTION: &str = r#"["wait","outside","measure-help","question","flag-short@4692185e775f"]"#;
const PROPOSAL: &str = r#"["wait","/work/repo","master","proposal","note-1"]"#;

const MARKED: &str = r#"{"result": "done", "exit_code": 0, "output": "marked R1 ok\n"}"#;
const DONE: &str = r#"{"result": "done", "exit_code": 0, "output": "approved gated\n"}"#;
const CONFIRM: &str = r#"{"result": "confirm", "token": "17.ab", "expires": "2026-11-16T08:02:00Z", "summary": "Approve on gated: run in /work/repo: sakura flow approve gated spec"}"#;
const REFUSED: &str = r#"{"result": "refused", "reason": "no longer waiting: the gate moved on"}"#;

/// The window's content as the desk draws it: the page, and the layer the toasts are drawn on.
struct Frame {
    page: Entity<InboxPage>,
    sidebar: gpui::Pixels,
}

impl Render for Frame {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .pl(self.sidebar)
                    .child(self.page.clone()),
            )
            .child(
                div()
                    .h(crate::theme::kit::TOAST_DOCK_HEIGHT)
                    .flex_none()
                    .child(crate::ui::kit::Toast::gutter(window, cx)),
            )
    }
}

/// The page over a fake engine whose snapshot is `snapshot`, which answers `first` to an action and `second`
/// to its confirmation; the first snapshot read and drawn.
pub(super) fn open_on<'a>(
    cx: &'a mut TestAppContext,
    snapshot: &str,
    first: &str,
    second: &str,
) -> (Rig, Entity<InboxPage>, &'a mut VisualTestContext) {
    open_with_sidebar(cx, snapshot, first, second, gpui::Pixels::ZERO)
}

fn open_with_sidebar<'a>(
    cx: &'a mut TestAppContext,
    snapshot: &str,
    first: &str,
    second: &str,
    sidebar: gpui::Pixels,
) -> (Rig, Entity<InboxPage>, &'a mut VisualTestContext) {
    let rig = engine_rig_tests::open(cx, snapshot, first, second);
    let (facts, source) = (rig.facts.clone(), rig.source.clone());
    let slot = Rc::new(RefCell::new(None));
    let kept = slot.clone();
    let (_, cx) = cx.add_window_view(move |window, cx| {
        let answers = cx.new(|cx| Answers::new(facts.clone(), source.clone(), cx));
        let page = cx.new(|cx| InboxPage::new(facts, answers, source, cx));
        *kept.borrow_mut() = Some(page.clone());
        gpui_component::Root::new(cx.new(|_| Frame { page, sidebar }), window, cx)
    });
    let page = slot.borrow_mut().take().expect("the page was built");
    cx.update(|window, cx| {
        super::install_keys(cx);
        window.activate_window();
    });
    wait_until(cx, "the first ask", |cx| {
        cx.debug_bounds("group-0").is_some()
    });
    cx.update(|window, cx| {
        assert!(!page.read(cx).focus.is_focused(window));
        window.focus(&page.read(cx).focus.clone(), cx);
    });
    (rig, page, cx)
}

fn open<'a>(
    cx: &'a mut TestAppContext,
    first: &str,
    second: &str,
) -> (Rig, Entity<InboxPage>, &'a mut VisualTestContext) {
    open_on(cx, SNAPSHOT, first, second)
}

/// Where the element named `selector` is drawn; the name is made here, so it is leaked for the test.
fn bounds(cx: &mut VisualTestContext, selector: &str) -> Option<gpui::Bounds<gpui::Pixels>> {
    let selector: &'static str = Box::leak(selector.to_string().into_boxed_str());
    cx.debug_bounds(selector)
}

fn click(cx: &mut VisualTestContext, selector: &str) {
    let at = bounds(cx, selector)
        .unwrap_or_else(|| panic!("{selector}"))
        .center();
    cx.simulate_click(at, Modifiers::none());
}

/// Selects `ask` as a click on its row does, and waits for its page.
fn select(cx: &mut VisualTestContext, ask: &str) {
    click(cx, &format!("ask-{ask}"));
    wait_until(cx, "the ask", |cx| {
        bounds(cx, &format!("title-{ask}")).is_some()
    });
}

fn selected(page: &Entity<InboxPage>, cx: &mut VisualTestContext) -> Option<String> {
    page.read_with(cx, |page, _| page.selected().map(str::to_string))
}

/// Moves the clock `by`, as the person's seconds go by, and lets the page draw.
fn wait_for(cx: &mut VisualTestContext, by: Duration) {
    cx.executor().advance_clock(by);
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

fn told(page: &Entity<InboxPage>, cx: &mut VisualTestContext, key: &str) -> Option<String> {
    let answers = page.read_with(cx, |page, _| page.answers().clone());
    answers.read_with(cx, |answers, _| answers.told(key).map(str::to_string))
}

fn summary(page: &Entity<InboxPage>, cx: &mut VisualTestContext, key: &str) -> Option<String> {
    let answers = page.read_with(cx, |page, _| page.answers().clone());
    answers.read_with(cx, |answers, _| answers.summary(key).map(str::to_string))
}

#[gpui::test]
fn the_fixture_is_listed_by_lane_with_the_first_ask_selected(cx: &mut TestAppContext) {
    let (_rig, page, cx) = open(cx, MARKED, MARKED);
    for index in 0..4 {
        assert!(
            bounds(cx, &format!("group-{index}")).is_some(),
            "group {index}"
        );
    }
    assert!(bounds(cx, "group-4").is_none(), "four lanes have asks");
    let snapshot = parse(SNAPSHOT);
    let order = model::ordered(&snapshot, &model::Filter::default());
    for wait in &order {
        assert!(
            bounds(cx, &format!("ask-{}", wait.id)).is_some(),
            "{}",
            wait.id
        );
    }
    assert_eq!(selected(&page, cx).as_deref(), Some(order[0].id.as_str()));
}

#[gpui::test]
fn clicking_an_ask_selects_it(cx: &mut TestAppContext) {
    let (_rig, page, cx) = open(cx, MARKED, MARKED);
    select(cx, OWNER_ROW);
    assert_eq!(selected(&page, cx).as_deref(), Some(OWNER_ROW));
}

#[gpui::test]
fn j_and_k_and_the_arrows_move_the_selection_and_stay_inside_the_list(cx: &mut TestAppContext) {
    let (_rig, page, cx) = open(cx, MARKED, MARKED);
    let snapshot = parse(SNAPSHOT);
    let order: Vec<String> = model::ordered(&snapshot, &model::Filter::default())
        .iter()
        .map(|wait| wait.id.clone())
        .collect();
    cx.simulate_keystrokes("j");
    assert_eq!(selected(&page, cx).as_deref(), Some(order[1].as_str()));
    cx.simulate_keystrokes("down");
    assert_eq!(selected(&page, cx).as_deref(), Some(order[2].as_str()));
    cx.simulate_keystrokes("k up up up");
    assert_eq!(
        selected(&page, cx).as_deref(),
        Some(order[0].as_str()),
        "never past the first"
    );
    for _ in 0..order.len() + 2 {
        cx.simulate_keystrokes("j");
    }
    assert_eq!(
        selected(&page, cx).as_deref(),
        Some(order[order.len() - 1].as_str())
    );
}

#[gpui::test]
fn a_sends_nothing_for_six_seconds_and_then_the_approval_with_the_ask_s_id(
    cx: &mut TestAppContext,
) {
    let (rig, _page, cx) = open(cx, MARKED, MARKED);
    select(cx, OWNER_ROW);
    cx.simulate_keystrokes("a");
    assert!(bounds(cx, "toast-undo").is_some(), "the toast offers Undo");
    wait_for(cx, Duration::from_secs(5));
    assert!(rig.acts().is_empty(), "held: {:?}", rig.acts());
    wait_for(cx, Duration::from_secs(2));
    wait_until(cx, "the answer", |_| rig.acts().len() == 1);
    let sent = &rig.acts()[0];
    assert!(
        sent.starts_with(&format!("cockpit act k {OWNER_ROW} ")),
        "{sent}"
    );
    assert!(sent.contains("--verdict ok"), "{sent}");
}

#[gpui::test]
fn the_undo_toast_never_covers_the_button_that_was_pressed_nor_the_footer_s_buttons(
    cx: &mut TestAppContext,
) {
    let (_rig, _page, cx) = open(cx, MARKED, MARKED);
    // A window as narrow as a person's is: the centre of it is where the footer's buttons stand.
    cx.simulate_resize(gpui::size(gpui::px(900.), gpui::px(640.)));
    select(cx, OWNER_ROW);
    let pressed = bounds(cx, "verb-approve").expect("Approve").center();
    cx.simulate_mouse_move(pressed, None, Modifiers::none());
    click(cx, "verb-approve");
    wait_until(cx, "the toast", |cx| bounds(cx, "toast").is_some());
    wait_for(cx, Duration::from_millis(400));
    let toast = bounds(cx, "toast").expect("the toast");
    assert!(
        !toast.contains(&pressed),
        "the toast covers the pressed button"
    );
    for footer in ["verb-approve", "verb-decline", "verb-reply"] {
        if let Some(button) = bounds(cx, footer) {
            assert!(!toast.intersects(&button), "the toast covers {footer}");
        }
    }
}

#[gpui::test]
fn the_toast_leaves_the_inbox_detail_clickable(cx: &mut TestAppContext) {
    let (_rig, _page, cx) = open(cx, MARKED, MARKED);
    cx.simulate_resize(gpui::size(gpui::px(1280.), gpui::px(800.)));
    select(cx, APPROVAL);
    cx.update(|window, cx| {
        Toast::show(
            window,
            cx,
            "Approved the change to the version option",
            None,
        );
    });
    wait_for(cx, Duration::from_millis(400));
    let toast = bounds(cx, "toast").expect("the toast");
    let footer = bounds(cx, "inbox-actions").expect("the detail actions");
    assert!(!toast.intersects(&footer), "{toast:?} overlaps {footer:?}");
}

#[gpui::test]
fn a_button_does_not_show_its_key_the_key_is_in_its_tooltip_and_still_answers(
    cx: &mut TestAppContext,
) {
    use crate::ui::tip::{Tips, TipsHandle, COLD_DELAY};
    let tips = cx.update(|cx| {
        let tips = cx.new(|_| Tips::new());
        cx.set_global(TipsHandle(tips.clone()));
        tips
    });
    let (_rig, _page, cx) = open(cx, MARKED, MARKED);
    select(cx, OWNER_ROW);
    for (button, key) in [("verb-approve", "A"), ("verb-decline", "D")] {
        let face = bounds(cx, button).expect(button);
        let label = match button {
            "verb-approve" => "Mark done",
            _ => "Not now",
        };
        cx.simulate_mouse_move(face.center(), None, Modifiers::none());
        cx.executor().advance_clock(COLD_DELAY);
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let said = tips.read_with(cx, |tips, _| tips.shown().map(|shown| shown.words.clone()));
        let said = said.unwrap_or_else(|| panic!("{button} has a tooltip"));
        assert_eq!(said.shortcut.as_deref(), Some(key), "{button}");
        assert_eq!(said.label.as_ref(), label, "{button}");
    }
}

/// Puts the pointer on the toast and lets the page draw it hovered.
fn hover_the_toast(cx: &mut VisualTestContext) {
    let at = bounds(cx, "toast").expect("the toast").center();
    cx.simulate_mouse_move(at, None, Modifiers::none());
    wait_for(cx, Duration::from_millis(100));
}

#[gpui::test]
fn the_pointer_on_the_toast_keeps_the_answer_held_and_undo_still_works(cx: &mut TestAppContext) {
    let (rig, page, cx) = open(cx, MARKED, MARKED);
    select(cx, OWNER_ROW);
    cx.simulate_keystrokes("a");
    wait_for(cx, Duration::from_millis(100));
    hover_the_toast(cx);
    wait_for(cx, Duration::from_secs(8));
    assert!(
        rig.acts().is_empty(),
        "held under the pointer: {:?}",
        rig.acts()
    );
    click(cx, "toast-undo");
    wait_for(cx, Duration::from_secs(10));
    assert!(rig.acts().is_empty(), "{:?}", rig.acts());
    assert_eq!(selected(&page, cx).as_deref(), Some(OWNER_ROW));
}

#[gpui::test]
fn the_answer_goes_once_the_pointer_leaves_the_toast_after_the_time(cx: &mut TestAppContext) {
    let (rig, _page, cx) = open(cx, MARKED, MARKED);
    select(cx, OWNER_ROW);
    cx.simulate_keystrokes("a");
    wait_for(cx, Duration::from_millis(100));
    hover_the_toast(cx);
    wait_for(cx, Duration::from_secs(8));
    assert!(rig.acts().is_empty());
    cx.simulate_mouse_move(
        gpui::point(gpui::px(1.), gpui::px(1.)),
        None,
        Modifiers::none(),
    );
    wait_for(cx, Duration::from_secs(1));
    wait_until(cx, "the answer", |_| rig.acts().len() == 1);
}

#[gpui::test]
fn an_ask_answered_keeps_its_title_and_undo_until_the_hold_is_over(cx: &mut TestAppContext) {
    let (_rig, _page, cx) = open(cx, MARKED, MARKED);
    select(cx, OWNER_ROW);
    let at = bounds(cx, &format!("ask-{OWNER_ROW}"))
        .expect("the row")
        .origin;
    let title = format!("row-title-ask-{OWNER_ROW}");
    let title_left = bounds(cx, &title).expect("the original title").left();
    cx.simulate_keystrokes("a");
    wait_for(cx, Duration::from_millis(100));
    let held = bounds(cx, &format!("ask-{OWNER_ROW}")).expect("the answered row stays");
    assert_eq!(held.origin, at, "the row holds its slot");
    assert_eq!(
        bounds(cx, &title).expect("the held title").left(),
        title_left
    );
    assert!(
        bounds(cx, "toast-text").is_some(),
        "the toast says the action"
    );
    assert!(
        bounds(cx, &format!("undo-{OWNER_ROW}")).is_none(),
        "Undo lives in the toast"
    );
    wait_for(cx, Duration::from_secs(7));
    assert!(
        bounds(cx, &format!("ask-{OWNER_ROW}")).is_none(),
        "the row goes when the hold is over"
    );
}

#[gpui::test]
fn the_key_moves_the_selection_to_the_next_ask_to_answer_and_a_click_keeps_it_on_the_answered_row(
    cx: &mut TestAppContext,
) {
    let (_rig, page, cx) = open(cx, MARKED, MARKED);
    select(cx, OWNER_ROW);
    cx.simulate_keystrokes("a");
    assert_ne!(
        selected(&page, cx).as_deref(),
        Some(OWNER_ROW),
        "a key moves on"
    );
    assert!(selected(&page, cx).is_some());
    select(cx, OWNER_ROW);
    assert_eq!(selected(&page, cx).as_deref(), Some(OWNER_ROW));
    wait_for(cx, Duration::from_millis(100));
    assert_eq!(
        selected(&page, cx).as_deref(),
        Some(OWNER_ROW),
        "a click on the answered row leaves it selected while it is held"
    );
    wait_for(cx, Duration::from_secs(7));
    assert_ne!(
        selected(&page, cx).as_deref(),
        Some(OWNER_ROW),
        "the selection moves on when the row goes"
    );
}

#[gpui::test]
fn a_double_click_on_approve_answers_once(cx: &mut TestAppContext) {
    let (rig, _page, cx) = open(cx, MARKED, MARKED);
    select(cx, OWNER_ROW);
    let at = bounds(cx, "verb-approve").expect("Approve").center();
    cx.simulate_click(at, Modifiers::none());
    cx.simulate_click(at, Modifiers::none());
    cx.simulate_click(at, Modifiers::none());
    wait_for(cx, Duration::from_secs(7));
    wait_until(cx, "the answer", |_| !rig.acts().is_empty());
    wait_for(cx, Duration::from_secs(1));
    assert_eq!(rig.acts().len(), 1, "{:?}", rig.acts());
}

#[gpui::test]
fn a_pressed_key_the_keyboard_repeats_answers_one_ask_not_the_next(cx: &mut TestAppContext) {
    let (rig, _page, cx) = open(cx, MARKED, MARKED);
    select(cx, OWNER_ROW);
    cx.simulate_keystrokes("a");
    cx.simulate_event(gpui::KeyDownEvent {
        keystroke: gpui::Keystroke::parse("a").expect("a key"),
        is_held: true,
        prefer_character_input: false,
    });
    wait_for(cx, Duration::from_secs(7));
    wait_until(cx, "the answer", |_| !rig.acts().is_empty());
    wait_for(cx, Duration::from_secs(1));
    assert_eq!(rig.acts().len(), 1, "{:?}", rig.acts());
}

#[gpui::test]
fn answers_given_inside_one_hold_share_one_toast_that_counts_them_and_one_undo_takes_all_back(
    cx: &mut TestAppContext,
) {
    let (rig, page, cx) = open(cx, MARKED, MARKED);
    select(cx, OWNER_ROW);
    cx.simulate_keystrokes("a");
    cx.simulate_keystrokes("a");
    assert_eq!(
        toasts_on_screen(cx),
        1,
        "one toast, not one for each answer"
    );
    click(cx, "toast-undo");
    wait_for(cx, Duration::from_secs(10));
    // The gated ask's first call only asks the engine its question; nothing was run or confirmed.
    let ran = rig
        .acts()
        .into_iter()
        .filter(|act| act.contains("--verdict") || act.contains("--confirm"))
        .count();
    assert_eq!(ran, 0, "{:?}", rig.acts());
    assert!(selected(&page, cx).is_some());
}

fn toasts_on_screen(cx: &mut VisualTestContext) -> usize {
    usize::from(bounds(cx, "toast").is_some())
}

#[gpui::test]
fn undo_inside_the_hold_sends_nothing_and_brings_the_ask_back(cx: &mut TestAppContext) {
    let (rig, page, cx) = open(cx, MARKED, MARKED);
    select(cx, OWNER_ROW);
    cx.simulate_keystrokes("a");
    wait_for(cx, Duration::from_secs(2));
    click(cx, "toast-undo");
    wait_for(cx, Duration::from_secs(10));
    assert!(rig.acts().is_empty(), "{:?}", rig.acts());
    assert!(bounds(cx, &format!("ask-{OWNER_ROW}")).is_some());
    assert_eq!(selected(&page, cx).as_deref(), Some(OWNER_ROW));
}

#[gpui::test]
fn closing_the_window_inside_the_hold_sends_the_answer(cx: &mut TestAppContext) {
    let (rig, page, cx) = open(cx, MARKED, MARKED);
    select(cx, OWNER_ROW);
    cx.simulate_keystrokes("a");
    wait_for(cx, Duration::from_secs(1));
    assert!(rig.acts().is_empty());
    drop(page);
    cx.update(|window, _| window.remove_window());
    cx.run_until_parked();
    wait_until_sent(&rig, 1);
    assert!(rig.acts()[0].starts_with(&format!("cockpit act k {OWNER_ROW} ")));
}

#[gpui::test]
fn quitting_the_app_inside_the_hold_sends_the_answer_before_it_ends(cx: &mut TestAppContext) {
    let (rig, _page, cx) = open(cx, MARKED, MARKED);
    select(cx, OWNER_ROW);
    cx.simulate_keystrokes("a");
    wait_for(cx, Duration::from_secs(1));
    assert!(rig.acts().is_empty());
    rig.facts.update(cx, |_, cx| cx.shutdown());
    wait_until_sent(&rig, 1);
}

/// Waits, in real time, for the fake engine to have been called `count` times.
fn wait_until_sent(rig: &Rig, count: usize) {
    let limit = std::time::Instant::now() + Duration::from_secs(30);
    while rig.acts().len() < count {
        assert!(
            std::time::Instant::now() < limit,
            "waited for the engine's calls"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[gpui::test]
fn no_note_field_is_shown_until_decline_or_reply_and_no_field_is_focused_on_opening(
    cx: &mut TestAppContext,
) {
    let (_rig, page, cx) = open(cx, MARKED, MARKED);
    select(cx, OWNER_ROW);
    assert!(bounds(cx, "inbox-note").is_none());
    assert!(
        page.read_with(cx, |page, _| page.field.is_none()),
        "no field was made, so none has the caret"
    );
    cx.update(|window, cx| {
        assert!(
            page.read(cx).focus.is_focused(window),
            "the page itself has the keys"
        );
    });
}

#[gpui::test]
fn decline_on_an_owner_row_asks_for_a_note_and_sends_ko_with_it(cx: &mut TestAppContext) {
    let (rig, _page, cx) = open(cx, MARKED, MARKED);
    select(cx, OWNER_ROW);
    click(cx, "verb-decline");
    wait_until(cx, "the note field", |cx| {
        bounds(cx, "inbox-note").is_some()
    });
    click(cx, "inbox-note");
    cx.simulate_input("too slow");
    click(cx, "note-send");
    wait_for(cx, Duration::from_secs(7));
    wait_until_sent(&rig, 1);
    let sent = &rig.acts()[0];
    assert!(
        sent.starts_with(&format!("cockpit act k {OWNER_ROW} ")),
        "{sent}"
    );
    assert!(
        sent.contains("--verdict ko") && sent.contains("--note too slow"),
        "{sent}"
    );
}

#[gpui::test]
fn escape_leaves_the_note_without_sending(cx: &mut TestAppContext) {
    let (rig, _page, cx) = open(cx, MARKED, MARKED);
    select(cx, OWNER_ROW);
    cx.simulate_keystrokes("d");
    wait_until(cx, "the note field", |cx| {
        bounds(cx, "inbox-note").is_some()
    });
    cx.simulate_keystrokes("escape");
    wait_until(cx, "the note gone", |cx| bounds(cx, "inbox-note").is_none());
    wait_for(cx, Duration::from_secs(8));
    assert!(rig.acts().is_empty());
    assert!(
        bounds(cx, &format!("ask-{OWNER_ROW}")).is_some(),
        "the ask is still waiting"
    );
}

fn with_proposal() -> String {
    let mut snapshot: serde_json::Value = serde_json::from_str(SNAPSHOT).expect("the fixture");
    let offered = |key: &str, label: &str| serde_json::json!({"key": key, "label": label, "needs": [], "optional": [], "confirm": true});
    snapshot["waits"]
        .as_array_mut()
        .expect("the waits")
        .push(serde_json::json!({
            "id": PROPOSAL, "key": ["wait", "/work/repo", "master", "proposal", "note-1"],
            "kind": "proposal", "repo": "/work/repo", "project": "repo", "task": "master",
            "title": "Add a note to the knowledge folder", "detail": "", "since": null,
            "actions": [offered("y", "Accept"), offered("N", "Reject")]
        }));
    snapshot.to_string()
}

#[gpui::test]
fn declining_a_proposal_sends_the_engine_s_reject_at_once_with_no_note_field(
    cx: &mut TestAppContext,
) {
    let (rig, _page, cx) = open_on(cx, &with_proposal(), MARKED, MARKED);
    select(cx, PROPOSAL);
    click(cx, "verb-decline");
    assert!(
        bounds(cx, "inbox-note").is_none(),
        "the engine's reject takes no note"
    );
    wait_for(cx, Duration::from_secs(7));
    wait_until_sent(&rig, 1);
    assert!(
        rig.acts()[0].starts_with(&format!("cockpit act N {PROPOSAL}")),
        "{}",
        rig.acts()[0]
    );
}

#[gpui::test]
fn a_question_shows_a_button_for_each_choice_and_the_choice_is_sent(cx: &mut TestAppContext) {
    let (rig, _page, cx) = open(cx, MARKED, MARKED);
    select(cx, QUESTION);
    assert!(bounds(cx, "choice-0").is_some() && bounds(cx, "choice-1").is_some());
    assert!(bounds(cx, "choice-2").is_none());
    click(cx, "choice-1");
    wait_for(cx, Duration::from_secs(7));
    wait_until_sent(&rig, 1);
    let sent = &rig.acts()[0];
    assert!(
        sent.starts_with(&format!("cockpit act r {QUESTION}")) && sent.contains("--choice no"),
        "{sent}"
    );
}

#[gpui::test]
fn r_opens_the_reply_field_and_return_sends_the_words(cx: &mut TestAppContext) {
    let (rig, _page, cx) = open(cx, MARKED, MARKED);
    select(cx, QUESTION);
    cx.simulate_keystrokes("r");
    wait_until(cx, "the reply field", |cx| {
        bounds(cx, "inbox-note").is_some()
    });
    click(cx, "inbox-note");
    cx.simulate_input("only for the long flag");
    cx.simulate_keystrokes("enter");
    wait_for(cx, Duration::from_secs(7));
    wait_until_sent(&rig, 1);
    let sent = &rig.acts()[0];
    assert!(sent.contains("--text only for the long flag"), "{sent}");
}

#[gpui::test]
fn answering_a_question_in_words_is_held_then_sent_and_a_choice_it_does_not_offer_sends_nothing(
    cx: &mut TestAppContext,
) {
    let (rig, page, cx) = open(cx, MARKED, MARKED);
    let answer = |cx: &mut VisualTestContext, answer: model::Answer| {
        page.update_in(cx, |page, window, cx| {
            page.answer_ask(QUESTION, answer, window, cx)
        });
    };
    answer(cx, model::Answer::Choice("maybe".into()));
    answer(cx, model::Answer::Reply("  ".into()));
    wait_for(cx, Duration::from_secs(7));
    assert!(rig.acts().is_empty(), "nothing offered: {:?}", rig.acts());
    answer(cx, model::Answer::Reply("only for the long flag".into()));
    assert!(rig.acts().is_empty(), "held first: {:?}", rig.acts());
    wait_for(cx, Duration::from_secs(7));
    wait_until_sent(&rig, 1);
    let sent = &rig.acts()[0];
    assert!(
        sent.starts_with(&format!("cockpit act r {QUESTION}"))
            && sent.contains("--text only for the long flag"),
        "{sent}"
    );
}

/// The question the engine's confirmation of the approval of `gated` comes to in words.
const APPROVAL_QUESTION: &str = "Approve the spec of gated?";
/// What the toast says of it while it is held: a statement, not a question.
const APPROVAL_STATEMENT: &str = "Approving the spec of gated";
/// The engine asking something else about the same approval, after the person saw the first question.
const CONFIRM_CHANGED: &str = r#"{"result": "confirm", "token": "18.cd", "expires": "2026-11-16T08:02:00Z", "summary": "Approve on gated: the spec was edited"}"#;

fn held_words(page: &Entity<InboxPage>, cx: &mut VisualTestContext, ask: &str) -> Option<String> {
    page.read_with(cx, |page, _| {
        page.held_words(ask).map(|words| words.to_string())
    })
}

fn confirms(rig: &Rig) -> usize {
    rig.acts()
        .iter()
        .filter(|act| act.contains("--confirm"))
        .count()
}

/// Presses Approve on the gated ask and waits until the engine's question is in the toast.
fn approve_until_asked(rig: &Rig, page: &Entity<InboxPage>, cx: &mut VisualTestContext) {
    select(cx, APPROVAL);
    cx.simulate_keystrokes("a");
    wait_until(cx, "the question in the toast", |cx| {
        bounds(cx, "toast-detail").is_some()
    });
    assert_eq!(
        held_words(page, cx, APPROVAL).as_deref(),
        Some(APPROVAL_STATEMENT)
    );
    assert_eq!(
        rig.acts().len(),
        1,
        "only the call that asks: {:?}",
        rig.acts()
    );
}

#[gpui::test]
fn an_inbox_approval_toast_names_the_action_before_the_engine_summary(cx: &mut TestAppContext) {
    let (rig, _page, cx) = open(cx, CONFIRM, DONE);
    rig.say_delay("0.3");
    select(cx, APPROVAL);
    cx.simulate_keystrokes("a");
    let toast = cx.update(|window, cx| Toast::text_for_test(window, cx));
    assert_eq!(toast.as_deref(), Some(APPROVAL_STATEMENT));
    assert!(bounds(cx, "toast-detail").is_none());
    wait_until(cx, "the engine summary", |cx| {
        bounds(cx, "toast-detail").is_some()
    });
    assert_eq!(
        cx.update(|window, cx| Toast::text_for_test(window, cx))
            .as_deref(),
        Some(APPROVAL_STATEMENT)
    );
}

#[gpui::test]
fn a_confirm_first_approve_shows_the_engine_s_question_during_the_hold_before_any_confirm(
    cx: &mut TestAppContext,
) {
    let (rig, page, cx) = open(cx, CONFIRM, DONE);
    approve_until_asked(&rig, &page, cx);
    assert!(!rig.acts()[0].contains("--confirm"), "{}", rig.acts()[0]);
    wait_until(cx, "the toast", |cx| bounds(cx, "toast-text").is_some());
    assert!(bounds(cx, "toast-undo").is_some(), "the toast offers Undo");
    assert!(
        bounds(cx, "toast-detail").is_some(),
        "the engine's summary, without its command, is the second line"
    );
    wait_for(cx, Duration::from_secs(5));
    assert_eq!(confirms(&rig), 0, "nothing ran during the hold");
    wait_for(cx, Duration::from_secs(2));
    wait_until(cx, "the confirmed call", |_| confirms(&rig) == 1);
    let acts = rig.acts();
    assert_eq!(acts.len(), 3, "ask, ask again to check, confirm: {acts:?}");
    assert!(!acts[1].contains("--confirm"), "{}", acts[1]);
    assert!(
        acts[2].contains("--confirm 17.ab"),
        "the token that was shown: {}",
        acts[2]
    );
}

#[gpui::test]
fn undo_on_a_confirm_first_approve_sends_only_the_call_that_asks(cx: &mut TestAppContext) {
    let (rig, page, cx) = open(cx, CONFIRM, DONE);
    approve_until_asked(&rig, &page, cx);
    wait_for(cx, Duration::from_secs(2));
    click(cx, "toast-undo");
    wait_for(cx, Duration::from_secs(10));
    assert_eq!(rig.acts().len(), 1, "{:?}", rig.acts());
    assert_eq!(confirms(&rig), 0);
    assert!(
        bounds(cx, &format!("ask-{APPROVAL}")).is_some(),
        "the ask is back"
    );
}

#[gpui::test]
fn a_question_that_changed_during_the_hold_is_not_confirmed_and_is_asked_again(
    cx: &mut TestAppContext,
) {
    let (rig, page, cx) = open(cx, CONFIRM, DONE);
    approve_until_asked(&rig, &page, cx);
    rig.say_first(CONFIRM_CHANGED);
    wait_for(cx, Duration::from_secs(7));
    wait_until(cx, "the new question", |cx| {
        summary(&page, cx, APPROVAL).is_some()
    });
    assert_eq!(confirms(&rig), 0, "{:?}", rig.acts());
    wait_until(cx, "the ask back with its question", |cx| {
        bounds(cx, &format!("confirm-{APPROVAL}")).is_some()
    });
    click(cx, &format!("confirm-{APPROVAL}"));
    wait_until(cx, "the confirmed call", |_| confirms(&rig) == 1);
    assert!(
        rig.acts()
            .last()
            .is_some_and(|act| act.contains("--confirm 18.cd")),
        "the new token: {:?}",
        rig.acts()
    );
}

#[gpui::test]
fn quitting_after_the_question_was_shown_confirms_it_once_the_question_is_checked(
    cx: &mut TestAppContext,
) {
    let (rig, page, cx) = open(cx, CONFIRM, DONE);
    approve_until_asked(&rig, &page, cx);
    rig.facts.update(cx, |_, cx| cx.shutdown());
    wait_until_sent(&rig, 3);
    assert!(
        rig.acts()[2].contains("--confirm 17.ab"),
        "{:?}",
        rig.acts()
    );
}

#[gpui::test]
fn quitting_waits_for_a_slow_engine_to_take_the_confirmed_call(cx: &mut TestAppContext) {
    let (rig, page, cx) = open(cx, CONFIRM, DONE);
    approve_until_asked(&rig, &page, cx);
    rig.say_delay("0.4");
    let has_quit = Rc::new(std::cell::Cell::new(false));
    let finish = has_quit.clone();
    cx.update(|_, cx| super::quit::quit_after_answers_then(cx, move |_| finish.set(true)));
    assert!(!has_quit.get(), "the quit waits for the engine");
    wait_until(cx, "the confirmed call", |_| confirms(&rig) == 1);
    wait_until(cx, "the quit after the outcome", |_| has_quit.get());
    assert!(
        rig.acts()
            .last()
            .is_some_and(|act| act.contains("--confirm 17.ab")),
        "{:?}",
        rig.acts()
    );
}

#[gpui::test]
fn quitting_with_nothing_held_quits_at_once(cx: &mut TestAppContext) {
    let (_rig, _page, cx) = open(cx, CONFIRM, DONE);
    let has_quit = Rc::new(std::cell::Cell::new(false));
    let finish = has_quit.clone();
    cx.update(|_, cx| super::quit::quit_after_answers_then(cx, move |_| finish.set(true)));
    assert!(has_quit.get());
}

#[gpui::test]
fn closing_the_window_after_the_question_was_shown_confirms_it(cx: &mut TestAppContext) {
    let (rig, page, cx) = open(cx, CONFIRM, DONE);
    approve_until_asked(&rig, &page, cx);
    drop(page);
    cx.update(|window, _| window.remove_window());
    cx.run_until_parked();
    wait_until_sent(&rig, 3);
    assert!(
        rig.acts()[2].contains("--confirm 17.ab"),
        "{:?}",
        rig.acts()
    );
}

#[gpui::test]
fn flushing_sends_no_confirm_for_an_answer_whose_question_was_not_shown(cx: &mut TestAppContext) {
    let rig = engine_rig_tests::open(cx, SNAPSHOT, CONFIRM, DONE);
    let approve = parse(SNAPSHOT)
        .waits
        .iter()
        .flat_map(|wait| wait.actions.iter())
        .find(|action| action.key == "a")
        .expect("an approve action")
        .clone();
    assert!(approve.confirm, "the fixture's approval asks to confirm");
    let mut held = HeldActs::new(rig.source.clone());
    let act = HeldAct {
        key: APPROVAL.into(),
        action: approve,
        target: APPROVAL.into(),
        inputs: BTreeMap::new(),
    };
    held.hold(APPROVAL, act, APPROVAL_STATEMENT.into(), "Approved".into());
    assert!(held.flush().is_empty(), "nothing is sent for it");
    assert!(rig.acts().is_empty(), "{:?}", rig.acts());
}

#[gpui::test]
fn the_done_history_uses_the_record_after_a_hold_confirms(cx: &mut TestAppContext) {
    let mut snapshot: serde_json::Value = serde_json::from_str(SNAPSHOT).expect("the fixture");
    snapshot["decided"] = serde_json::json!([
        {"id": APPROVAL, "key": ["wait", "/work/repo", "gated", "approval", "spec"], "kind": "approval",
         "project": "repo", "task": "gated", "title": "Approve the spec gate", "answer": "approve",
         "choice": null, "text": null, "actor": "overlay:me:handset", "place": "phone", "at": "2026-11-16T02:00:00Z"}
    ]);
    let (rig, page, cx) = open_on(cx, &snapshot.to_string(), CONFIRM, DONE);
    approve_until_asked(&rig, &page, cx);
    wait_for(cx, Duration::from_secs(7));
    wait_until(cx, "the confirmed call", |_| confirms(&rig) == 1);
    page.update(cx, |page, cx| page.show(Shown::Done, cx));
    wait_until(cx, "the answer", |cx| {
        cx.debug_bounds("inbox-decided").is_some()
    });
    assert!(cx.debug_bounds("inbox-confirmed").is_none());
}

#[gpui::test]
fn a_refusal_is_shown_under_the_ask_that_comes_back(cx: &mut TestAppContext) {
    let (_rig, page, cx) = open(cx, REFUSED, REFUSED);
    select(cx, OWNER_ROW);
    cx.simulate_keystrokes("a");
    wait_for(cx, Duration::from_secs(7));
    wait_until(cx, "the refusal", |cx| told(&page, cx, OWNER_ROW).is_some());
    assert_eq!(
        told(&page, cx, OWNER_ROW).as_deref(),
        Some("no longer waiting: the gate moved on")
    );
    wait_until(cx, "the line", |cx| {
        bounds(cx, &format!("told-{OWNER_ROW}")).is_some()
    });
}

#[gpui::test]
fn a_confirmation_returned_after_the_hold_replaces_the_queued_toast(cx: &mut TestAppContext) {
    let mut fixture: serde_json::Value = serde_json::from_str(SNAPSHOT).unwrap();
    fixture["waits"][0]["actions"][0]["confirm"] = false.into();
    let (_rig, page, cx) = open_on(cx, &fixture.to_string(), CONFIRM, DONE);
    select(cx, APPROVAL);
    cx.simulate_keystrokes("a");
    wait_for(cx, Duration::from_secs(7));
    wait_until(cx, "the engine's confirmation", |cx| {
        page.read_with(cx, |page, cx| {
            page.answers().read(cx).summary(APPROVAL).is_some()
        })
    });
    let toast = cx.update(|window, cx| Toast::text_for_test(window, cx));
    assert_eq!(toast.as_deref(), Some(APPROVAL_QUESTION));
}

#[gpui::test]
fn a_reproposed_check_with_an_old_since_uses_its_new_arrival(cx: &mut TestAppContext) {
    let (rig, page, cx) = open_on(cx, &at_checks_gate(), CONFIRM, DONE);
    let mut fixture: serde_json::Value = serde_json::from_str(&at_checks_gate()).unwrap();
    let id = fixture["waits"][0]["id"].as_str().unwrap().to_string();
    let old = "2026-11-16T00:00:00Z";
    let started = super::list::now();
    fixture["waits"][0]["since"] = old.into();
    fixture["waits"][0]["detail"] = "Run these revised checks.".into();
    fixture["decided"] = serde_json::json!([{
        "id": id, "kind": "approval", "project": "repo", "task": "gated", "answer": "approve",
        "at": "2026-11-16T00:01:00Z"
    }]);
    rig.say_snapshot(&fixture.to_string());
    rig.facts.update(cx, |facts, cx| facts.read(cx));
    wait_until(cx, "the new proposal", |cx| {
        rig.facts.read_with(cx, |facts, _| {
            facts.snapshot().unwrap().waits[0].since.as_deref() == Some(old)
        })
    });
    select(cx, &id);
    let since = page.read_with(cx, |page, cx| {
        let snapshot = page.snapshot(cx).unwrap();
        model::raised(page.selected_ask(&snapshot).unwrap())
    });
    assert!(since >= Some(started), "{since:?} is older than {started}");
}

#[gpui::test]
fn a_failed_resume_keeps_the_item_and_reports_failure_in_the_toast(cx: &mut TestAppContext) {
    let failure = r#"{"result":"done","exit_code":1,"output":"AttributeError: missing revision"}"#;
    let (rig, page, cx) = open(cx, failure, failure);
    let toast =
        |cx: &mut VisualTestContext| cx.update(|window, cx| Toast::text_for_test(window, cx));
    select(cx, BLOCKED);
    cx.simulate_keystrokes("a");
    let before = toast(cx);
    let snapshot = parse(SNAPSHOT);
    let blocked = snapshot
        .waits
        .iter()
        .find(|wait| wait.id == BLOCKED)
        .unwrap();
    let key = model::verbs(blocked, model::lane_of(blocked, &snapshot))
        .primary
        .unwrap()
        .key;
    wait_for(cx, Duration::from_secs(7));
    wait_until_sent(&rig, 1);
    wait_until(cx, "the blocked row comes back", |cx| {
        bounds(cx, &format!("ask-{BLOCKED}")).is_some() && told(&page, cx, &key).is_some()
    });
    let after = toast(cx).expect("failure toast");
    assert_eq!(after.as_ref(), "The engine couldn't resume this lane.");
    assert!(bounds(cx, &format!("told-{key}")).is_none());
    assert!(!before.is_some_and(|words| words.contains("Resumed")));
    assert!(bounds(cx, "toast-undo").is_none());
}

#[gpui::test]
fn a_blocked_ask_is_resumed_through_its_lane_s_own_action(cx: &mut TestAppContext) {
    let (rig, _page, cx) = open(cx, CONFIRM, DONE);
    select(cx, BLOCKED);
    cx.simulate_keystrokes("a");
    wait_for(cx, Duration::from_secs(7));
    wait_until_sent(&rig, 1);
    assert!(
        rig.acts()[0].starts_with("cockpit act R "),
        "{}",
        rig.acts()[0]
    );
}

#[gpui::test]
fn resuming_a_blocked_lane_reveals_its_ask_when_raised_again(cx: &mut TestAppContext) {
    let (rig, page, cx) = open(cx, DONE, DONE);
    select(cx, BLOCKED);
    cx.simulate_keystrokes("a");
    wait_for(cx, Duration::from_secs(7));
    wait_until(cx, "Resume succeeded", |cx| {
        page.read_with(cx, |page, cx| {
            page.sending.get(BLOCKED).is_some_and(|sent| {
                matches!(
                    page.answers.read(cx).step(&sent.key),
                    Some(crate::ui::answers::Step::Told {
                        is_problem: false,
                        ..
                    })
                )
            })
        })
    });
    let row = format!("ask-{BLOCKED}");
    assert!(bounds(cx, &row).is_none(), "the unchanged ask stays hidden");

    let mut snapshot: serde_json::Value = serde_json::from_str(SNAPSHOT).unwrap();
    let wait = snapshot["waits"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|wait| wait["id"] == BLOCKED)
        .unwrap();
    wait["since"] = "2099-11-16T00:20:00Z".into();
    rig.say_snapshot(&snapshot.to_string());
    rig.facts.update(cx, |facts, cx| facts.read(cx));
    wait_until(cx, "the newer wait", |cx| {
        rig.facts.read_with(cx, |facts, _| {
            facts.snapshot().unwrap().waits.iter().any(|wait| {
                wait.id == BLOCKED && wait.since.as_deref() == Some("2099-11-16T00:20:00Z")
            })
        })
    });
    assert!(bounds(cx, &row).is_some(), "the same ID was raised again");
    select(cx, BLOCKED);
    let snapshot = parse(&snapshot.to_string());
    let blocked = snapshot
        .waits
        .iter()
        .find(|wait| wait.id == BLOCKED)
        .unwrap();
    let verb = model::verbs(blocked, model::lane_of(blocked, &snapshot))
        .primary
        .unwrap();
    assert!(
        bounds(cx, &format!("told-{}", verb.key)).is_none(),
        "the earlier result is gone"
    );
}

#[gpui::test]
fn open_lane_button_asks_the_desk_to_open_the_lane_of_the_ask(cx: &mut TestAppContext) {
    let (_rig, page, cx) = open(cx, MARKED, MARKED);
    let opened = Rc::new(RefCell::new(Vec::new()));
    let seen = opened.clone();
    cx.update(|_, cx| {
        cx.subscribe(&page, move |_, event: &InboxEvent, _| {
            seen.borrow_mut().push(event.clone())
        })
        .detach();
    });
    select(cx, OWNER_ROW);
    click(cx, "open-lane");
    assert_eq!(
        *opened.borrow(),
        [InboxEvent::OpenLane(engine_rig_tests::lane_key("demo"))]
    );
}

#[gpui::test]
fn an_outside_ask_says_whom_it_is_from(cx: &mut TestAppContext) {
    let (_rig, _page, cx) = open(cx, MARKED, MARKED);
    let outside = r#"["wait","outside","measure-help","owner_row","help-prints@786781a00abb"]"#;
    select(cx, outside);
    assert!(bounds(cx, &format!("from-{outside}")).is_some());
    select(cx, OWNER_ROW);
    assert!(
        bounds(cx, &format!("from-{OWNER_ROW}")).is_none(),
        "an engine ask has no author"
    );
}

#[gpui::test]
fn the_spec_of_a_lane_is_read_as_markdown_with_the_context_of_its_lane(cx: &mut TestAppContext) {
    let progress = serde_json::json!({"phase": "spec", "done": 0, "total": 3});
    let (_rig, _page, cx) = open_on(cx, &gated_with_progress(progress), MARKED, MARKED);
    select(cx, APPROVAL);
    assert!(bounds(cx, &format!("body-{APPROVAL}")).is_some());
    assert!(
        bounds(cx, "inbox-context").is_some(),
        "its lane has a plan to say"
    );
    assert!(bounds(cx, "inbox-properties").is_some() && bounds(cx, "inbox-actions").is_some());
}

#[gpui::test]
fn an_inline_code_identifier_fitting_the_inbox_column_stays_on_one_line(cx: &mut TestAppContext) {
    let identifier = "test_cli_version_prints_the_version_without_touching_result";
    let make =
        |text: &str| SNAPSHOT.replace("spec ready: run flow approve gated spec --continue", text);
    let (rig, _page, cx) = open_on(cx, &make("Short"), MARKED, MARKED);
    cx.simulate_resize(gpui::size(gpui::px(1280.), gpui::px(800.)));
    select(cx, APPROVAL);
    let selector = format!("body-{APPROVAL}");
    let single_line = bounds(cx, &selector).unwrap().size.height;
    rig.say_snapshot(&make(&format!("`{identifier}`")));
    rig.facts.update(cx, |facts, cx| facts.read(cx));
    wait_until(cx, "the inline code", |cx| {
        rig.facts.read_with(cx, |facts, _| {
            facts
                .snapshot()
                .unwrap()
                .waits
                .iter()
                .any(|wait| wait.detail.contains(identifier))
        })
    });
    wait_for(cx, Duration::from_millis(100));
    assert!(bounds(cx, &selector).unwrap().size.height <= single_line + gpui::px(4.));
}

#[gpui::test]
fn the_full_pytest_identifier_wraps_in_the_spec_goal(cx: &mut TestAppContext) {
    assert_pytest_identifier_wraps(cx, "## Goal\n\nThe test `{id}` covers it.");
}

#[gpui::test]
fn the_full_pytest_identifier_wraps_in_a_spec_journey(cx: &mut TestAppContext) {
    assert_pytest_identifier_wraps(
        cx,
        "## Journeys\n\n1. Run the command.\n2. Check the output.\n3. `pytest {id}` passes.",
    );
}

const PYTEST_IDENTIFIER: &str =
    "tests/test_measure.py::test_cli_version_prints_the_version_without_touching_result";

#[gpui::test]
fn the_wrapped_code_background_ends_at_the_last_identifier_fragment(cx: &mut TestAppContext) {
    let spec = "## Goal\n\n`{id}`.";
    let shorter = PYTEST_IDENTIFIER.strip_suffix("_result").unwrap();
    let short_boxes = pytest_identifier_boxes(cx, spec, shorter);
    let full_boxes = pytest_identifier_boxes(cx, spec, PYTEST_IDENTIFIER);
    assert!(short_boxes.len() >= 2 && full_boxes.len() >= 2);
    let short_last = short_boxes.last().unwrap();
    let full_last = full_boxes.last().unwrap();
    assert!(short_last.top() > short_boxes[0].top());
    assert!(full_last.top() > full_boxes[0].top());
    let total_width = |boxes: &[gpui::Bounds<gpui::ScaledPixels>]| {
        boxes.iter().map(|chip| chip.size.width.0).sum::<f32>()
    };
    assert!(
        total_width(&short_boxes) < total_width(&full_boxes),
        "the chips fit their text even when a shorter identifier wraps differently"
    );
}

fn assert_pytest_identifier_wraps(cx: &mut TestAppContext, spec: &str) {
    let boxes = pytest_identifier_boxes(cx, spec, PYTEST_IDENTIFIER);
    assert!(
        boxes.len() >= 2,
        "the oversized identifier wraps: {boxes:?}"
    );
    assert!(boxes.last().unwrap().top() > boxes[0].top());
}

fn pytest_identifier_boxes(
    cx: &mut TestAppContext,
    spec: &str,
    id: &str,
) -> Vec<gpui::Bounds<gpui::ScaledPixels>> {
    let mut fixture: serde_json::Value = serde_json::from_str(SNAPSHOT).unwrap();
    fixture["waits"][0]["detail"] = spec.replace("{id}", id).into();
    let (_rig, page, cx) = open_with_sidebar(
        cx,
        &fixture.to_string(),
        MARKED,
        MARKED,
        crate::theme::SIDEBAR_WIDTH,
    );
    cx.simulate_resize(gpui::size(gpui::px(1280.), gpui::px(800.)));
    select(cx, APPROVAL);
    wait_for(cx, Duration::from_millis(100));
    page.read_with(cx, |page, cx| {
        let (_, state) = page.bodies.get(APPROVAL).expect("the Spec reader");
        assert!(state.read(cx).rendered_text().as_str().contains(id));
    });
    let column = bounds(cx, &format!("body-{APPROVAL}")).expect("the column");
    cx.update(|window, cx| {
        let column = column.scale(window.scale_factor());
        let raised = crate::theme::Theme::palette(cx).raised;
        let mut boxes: Vec<_> = window
            .painted_quads()
            .iter()
            .filter(|quad| {
                quad.background == raised.into() && column.contains(&quad.bounds.center())
            })
            .map(|quad| {
                assert!(
                    quad.bounds.right().0 <= column.right().0 + 1.,
                    "{:?} leaves {column:?}",
                    quad.bounds
                );
                quad.bounds
            })
            .collect();
        boxes.sort_by(|a, b| a.top().0.total_cmp(&b.top().0));
        boxes
    })
}

#[gpui::test]
fn an_oversize_code_identifier_in_an_engine_ask_takes_one_line(cx: &mut TestAppContext) {
    let (rig, _page, cx) = open_on(cx, &blocked_with("Short.", None), MARKED, MARKED);
    cx.simulate_resize(gpui::size(gpui::px(1050.), gpui::px(800.)));
    select(cx, BLOCKED);
    let selector = format!("body-{BLOCKED}");
    let line = bounds(cx, &selector).expect("one line").size.height;
    let detail = format!(
        "Short.\ntests/test_file.py::`{}` --help",
        "test_usage_and_exits_zero_".repeat(12)
    );
    rig.say_snapshot(&blocked_with(&detail, None));
    rig.facts.update(cx, |facts, cx| facts.read(cx));
    wait_until(cx, "the new detail", |cx| {
        rig.facts.read_with(cx, |facts, _| {
            facts
                .snapshot()
                .unwrap()
                .waits
                .iter()
                .any(|wait| wait.detail == detail)
        })
    });
    let wrapped = bounds(cx, &selector).expect("wrapped body").size.height;
    assert!(
        wrapped <= line * 4.,
        "{wrapped:?} for four lines of {line:?}"
    );
}

#[gpui::test]
fn the_markdown_of_an_ask_the_engine_no_longer_lists_is_forgotten(cx: &mut TestAppContext) {
    let (rig, page, cx) = open(cx, MARKED, MARKED);
    select(cx, APPROVAL);
    assert!(page.read_with(cx, |page, _| page.bodies.contains_key(APPROVAL)));
    let mut snapshot: serde_json::Value = serde_json::from_str(SNAPSHOT).expect("the fixture");
    snapshot["waits"]
        .as_array_mut()
        .expect("the waits")
        .retain(|wait| wait["id"] != APPROVAL);
    rig.say_snapshot(&snapshot.to_string());
    rig.facts.update(cx, |facts, cx| facts.read(cx));
    wait_until(cx, "the body forgotten", |cx| {
        !page.read_with(cx, |page, _| page.bodies.contains_key(APPROVAL))
    });
}

/// The spec of the lane at the spec gate, as `text`, drawn, and a click on its first word.
fn click_on_the_spec(cx: &mut TestAppContext, text: &str) -> Option<String> {
    let snapshot = SNAPSHOT.replace("spec ready: run flow approve gated spec --continue", text);
    let (_rig, _page, cx) = open_on(cx, &snapshot, MARKED, MARKED);
    select(cx, APPROVAL);
    let spec = bounds(cx, &format!("body-{APPROVAL}")).expect("the spec");
    cx.simulate_click(
        spec.origin + gpui::point(gpui::px(6.), gpui::px(8.)),
        Modifiers::none(),
    );
    cx.opened_url()
}

#[gpui::test]
fn a_web_address_in_an_ask_opens_on_click(cx: &mut TestAppContext) {
    let opened = click_on_the_spec(
        cx,
        "<https://example.com/guide> and an ![pixel](https://tracker.example/p.png)",
    );
    assert_eq!(opened.as_deref(), Some("https://example.com/guide"));
}

#[gpui::test]
fn an_address_that_is_not_web_opens_nothing(cx: &mut TestAppContext) {
    let opened = click_on_the_spec(
        cx,
        "[the file](file:///etc/passwd) and <x-apple-systempreferences:com.apple.x>",
    );
    assert_eq!(opened, None, "only http and https open");
}

const NOTHING: &str = r#"{"api": 1, "waits": [], "items": [], "events": [], "problems": [], "projects": [{"id": "[\"project\",\"/work/repo\"]", "key": ["project", "/work/repo"], "name": "repo", "folder": "/work/repo", "open_lanes": 0, "actions": []}]}"#;

#[gpui::test]
fn an_empty_inbox_counts_blocked_lanes_as_blocked(cx: &mut TestAppContext) {
    let (rig, _page, cx) = open(cx, MARKED, MARKED);
    let mut document: serde_json::Value = serde_json::from_str(SNAPSHOT).unwrap();
    document["waits"] = serde_json::json!([]);
    document["items"]
        .as_array_mut()
        .unwrap()
        .retain(|item| item["agent_state"] == "blocked");
    assert_eq!(document["items"].as_array().unwrap().len(), 1);
    rig.say_snapshot(&document.to_string());
    rig.facts.update(cx, |facts, cx| facts.read(cx));
    wait_until(cx, "the empty Inbox", |cx| {
        bounds(cx, "inbox-empty").is_some()
    });
    assert!(bounds(cx, "inbox-running").is_none());
    assert!(bounds(cx, "inbox-blocked").is_some());
}

#[gpui::test]
fn with_no_ask_the_page_explains_the_inbox_and_offers_to_start_a_lane(cx: &mut TestAppContext) {
    let rig = engine_rig_tests::open(cx, NOTHING, MARKED, MARKED);
    let (facts, source) = (rig.facts.clone(), rig.source.clone());
    let (_, cx) = cx.add_window_view(move |window, cx| {
        let answers = cx.new(|cx| Answers::new(facts.clone(), source.clone(), cx));
        let page = cx.new(|cx| InboxPage::new(facts, answers, source, cx));
        gpui_component::Root::new(
            cx.new(|_| Frame {
                page,
                sidebar: gpui::Pixels::ZERO,
            }),
            window,
            cx,
        )
    });
    wait_until(cx, "the empty page", |cx| {
        cx.debug_bounds("inbox-empty").is_some()
    });
    assert!(
        cx.debug_bounds("inbox-nothing").is_some()
            && cx.debug_bounds("inbox-empty-intro").is_some()
    );
    assert!(cx.debug_bounds("inbox-list").is_none());
    assert!(cx.debug_bounds("inbox-start-lane").is_some());
}

fn with_decided() -> String {
    let mut snapshot: serde_json::Value = serde_json::from_str(SNAPSHOT).expect("the fixture");
    snapshot["decided"] = serde_json::json!([
        {"id": "w1", "key": ["wait", "/work/repo", "demo", "owner_row", "R0"], "kind": "owner_row",
         "project": "repo", "task": "demo", "title": "Check row R0", "answer": "reply",
         "choice": null, "text": "the title reads Demos", "actor": "cli:me", "at": "2026-11-16T01:00:00Z"},
        {"id": "w2", "key": ["wait", "/work/repo", "gated", "approval", "spec"], "kind": "approval",
         "project": "repo", "task": "gated", "title": "Approve the spec gate", "answer": "approve",
         "choice": null, "text": null, "actor": "overlay:me:handset", "place": "phone", "at": "2026-11-16T02:00:00Z"}
    ]);
    snapshot.to_string()
}

#[gpui::test]
fn done_without_decided_says_that_decided_asks_appear_there(cx: &mut TestAppContext) {
    let (_rig, page, cx) = open(cx, MARKED, MARKED);
    page.update(cx, |page, cx| page.show(Shown::Done, cx));
    wait_until(cx, "the sentence", |cx| {
        cx.debug_bounds("inbox-done-absent").is_some()
    });
    assert!(cx.debug_bounds("inbox-list").is_none());
}

#[gpui::test]
fn done_lists_the_decided_asks_newest_first_with_their_answers(cx: &mut TestAppContext) {
    let (_rig, page, cx) = open_on(cx, &with_decided(), MARKED, MARKED);
    page.update(cx, |page, cx| page.show(Shown::Done, cx));
    wait_until(cx, "the rows", |cx| cx.debug_bounds("inbox-list").is_some());
    let (newer, older) = (
        "done-w2@2026-11-16T02:00:00Z",
        "done-w1@2026-11-16T01:00:00Z",
    );
    let (newer_at, older_at) = (
        bounds(cx, newer).expect("newer"),
        bounds(cx, older).expect("older"),
    );
    assert!(newer_at.top() < older_at.top(), "newest first");
    assert_eq!(
        selected(&page, cx).as_deref(),
        Some("w2@2026-11-16T02:00:00Z")
    );
    assert!(bounds(cx, "inbox-resolved-by").is_some());
    let snapshot = parse(&with_decided());
    let entries = snapshot.decided.as_ref().unwrap();
    let approval = snapshot
        .waits
        .iter()
        .find(|wait| wait.id == APPROVAL)
        .unwrap();
    assert_eq!(
        super::list::decided_title(&entries[1], &snapshot),
        super::detail::ask_title(approval, &snapshot)
    );
    assert_eq!(
        model::resolved_by(&entries[1]).as_deref(),
        Some("me, Phone")
    );
    assert_eq!(
        super::list::decided_context(&entries[1], &snapshot).as_ref(),
        format!(
            "Approved, {}",
            crate::ui::lanes::lane_words(model::lane_of(approval, &snapshot).unwrap(), false)
        )
    );
    assert_eq!(model::resolved_by(&entries[0]).as_deref(), Some("me, CLI"));
    click(cx, older);
    wait_until(cx, "the answer", |cx| {
        cx.debug_bounds("inbox-decided").is_some()
    });
    assert!(
        cx.debug_bounds("inbox-note-text").is_some(),
        "the recorded reply is shown as the answer"
    );
    assert!(
        cx.debug_bounds("verb-approve").is_none(),
        "a decided ask has no verbs"
    );
}

#[test]
fn a_source_line_names_the_lane_once_when_the_agent_and_the_project_share_a_name() {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    assert_eq!(
        crate::ui::lanes::from_words("digest", "digest"),
        "From digest"
    );
    assert_eq!(
        crate::ui::lanes::from_words("claude", "work"),
        "From claude in work"
    );
    crate::l10n::set_for_test(crate::l10n::Language::Italian);
    assert_eq!(
        crate::ui::lanes::from_words("digest", "digest"),
        "Da digest"
    );
}

#[test]
fn only_an_outside_agent_s_ask_says_whom_it_is_from() {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let snapshot = parse(SNAPSHOT);
    let words = |kind: &str, task: &str| {
        let wait = snapshot
            .waits
            .iter()
            .find(|wait| wait.kind == kind && wait.task == task);
        model::from_words(wait.expect("the ask")).map(|words| words.to_string())
    };
    assert_eq!(
        words("question", "measure-help").as_deref(),
        Some("From fixture in repo")
    );
    assert_eq!(words("owner_row", "demo"), None);
}

#[test]
fn an_ask_names_the_lane_of_its_own_kind_when_a_flow_task_and_an_outside_lane_share_an_id() {
    let lane = |kind: &str, key: serde_json::Value| {
        serde_json::json!({
            "id": key.to_string(), "key": key, "kind": kind, "repo": "/work/repo",
            "agent_state": "waiting", "title": "Outside same", "author": "outside:fixture"
        })
    };
    let ask = |source: &str, id: &str| {
        serde_json::json!({
            "id": id, "key": ["wait", "/work/repo", "same", "question", "q"], "kind": "question",
            "repo": "/work/repo", "project": "repo", "task": "same", "title": "Which?", "source": source
        })
    };
    let text = serde_json::json!({
        "api": 1, "events": [], "problems": [],
        "items": [lane("flow", serde_json::json!(["flow", "/work/repo", "same"])),
                  lane("outside", serde_json::json!(["outside", "same"]))],
        "waits": [ask("flow", "wf"), ask("outside", "wo")]
    })
    .to_string();
    let snapshot = parse(&text);
    let kind = |index: usize| {
        model::lane_of(&snapshot.waits[index], &snapshot).map(|lane| lane.kind.clone())
    };
    assert_eq!(
        kind(0).as_deref(),
        Some("flow"),
        "a flow ask is about the flow task"
    );
    assert_eq!(
        kind(1).as_deref(),
        Some("outside"),
        "an outside ask is about the outside lane"
    );
}

/// The fixture with the approval waiting at the checks gate.
fn at_checks_gate() -> String {
    let mut snapshot: serde_json::Value = serde_json::from_str(SNAPSHOT).expect("the fixture");
    let approval = &mut snapshot["waits"][0];
    approval["key"][4] = "checks".into();
    approval["id"] = r#"["wait","/work/repo","gated","approval","checks"]"#.into();
    snapshot.to_string()
}

#[gpui::test]
fn always_approve_on_the_checks_gate_sends_the_approval_with_remember(cx: &mut TestAppContext) {
    let (rig, _page, cx) = open_on(cx, &at_checks_gate(), CONFIRM, DONE);
    let ask = r#"["wait","/work/repo","gated","approval","checks"]"#;
    select(cx, ask);
    click(cx, "verb-always");
    wait_for(cx, Duration::from_secs(7));
    wait_until_sent(&rig, 1);
    assert!(
        rig.acts()[0].contains("--remember yes"),
        "{}",
        rig.acts()[0]
    );
}

#[gpui::test]
fn another_gate_offers_no_always_approve(cx: &mut TestAppContext) {
    let (_rig, _page, cx) = open(cx, CONFIRM, DONE);
    select(cx, APPROVAL);
    assert!(bounds(cx, "verb-approve").is_some());
    assert!(bounds(cx, "verb-always").is_none());
}

#[gpui::test]
fn a_proposal_is_approved_after_the_hold_and_the_text_the_engine_bound_is_shown_in_the_toast(
    cx: &mut TestAppContext,
) {
    let summary_text =
        "Add the note notes/parser.md to the knowledge folder: 112 bytes, text exactly as shown";
    let confirm = format!(
        r#"{{"result": "confirm", "token": "17.ab", "expires": "2026-11-16T08:02:00Z", "summary": "{summary_text}"}}"#
    );
    let (rig, page, cx) = open_on(cx, &with_proposal(), &confirm, DONE);
    select(cx, PROPOSAL);
    cx.simulate_keystrokes("a");
    wait_until(cx, "the text in the toast", |cx| {
        held_words(&page, cx, PROPOSAL).is_some_and(|words| words.contains(summary_text))
    });
    assert!(
        rig.acts()[0].starts_with(&format!("cockpit act y {PROPOSAL}")),
        "{}",
        rig.acts()[0]
    );
    assert_eq!(confirms(&rig), 0, "nothing is written during the hold");
    wait_for(cx, Duration::from_secs(7));
    wait_until(cx, "the confirmed call", |_| confirms(&rig) == 1);
}

/// The fixture with its outside lane's two asks only, in one group.
fn one_group() -> String {
    let mut snapshot: serde_json::Value = serde_json::from_str(SNAPSHOT).expect("the fixture");
    let waits = snapshot["waits"].as_array_mut().expect("the waits");
    waits.retain(|wait| wait["source"] == "outside" && wait["kind"] == "owner_row");
    snapshot.to_string()
}

#[gpui::test]
fn the_dot_of_an_open_ask_stays_while_the_ask_is_selected_so_the_dot_and_the_count_agree(
    cx: &mut TestAppContext,
) {
    let (_rig, page, cx) = open(cx, MARKED, MARKED);
    let snapshot = parse(SNAPSHOT);
    let order = model::ordered(&snapshot, &model::Filter::default());
    assert_eq!(selected(&page, cx).as_deref(), Some(order[0].id.as_str()));
    for wait in &order {
        assert!(
            bounds(cx, &format!("dot-ask-{}", wait.id)).is_some(),
            "{} is not answered, so it has its dot",
            wait.id
        );
    }
}

#[gpui::test]
fn a_group_band_carries_no_mark_and_no_count_while_the_list_has_one_group(cx: &mut TestAppContext) {
    let (_rig, _page, cx) = open_on(cx, &one_group(), MARKED, MARKED);
    assert!(bounds(cx, "group-0").is_some());
    assert!(bounds(cx, "group-0-mark").is_none(), "no dot on a header");
    assert!(
        bounds(cx, "group-0-count").is_none(),
        "the sidebar says how many"
    );
}

#[gpui::test]
fn a_group_band_counts_its_asks_beside_other_groups_and_still_carries_no_mark(
    cx: &mut TestAppContext,
) {
    let (_rig, _page, cx) = open(cx, MARKED, MARKED);
    assert!(bounds(cx, "group-0-count").is_some());
    assert!(bounds(cx, "group-0-mark").is_none());
}

#[gpui::test]
fn grouped_rows_show_the_kind_and_details_use_properties(cx: &mut TestAppContext) {
    let document = one_group();
    let (_rig, _page, cx) = open_on(cx, &document, MARKED, MARKED);
    let snapshot = parse(&document);
    let wait = &snapshot.waits[0];
    assert_eq!(model::row_context(wait, &snapshot).as_ref(), "Approval");
    assert!(bounds(cx, &format!("sub-ask-{}", wait.id)).is_some());
    assert!(bounds(cx, &format!("chip-ask-{}", wait.id)).is_none());
    assert!(bounds(cx, &format!("state-ask-{}", wait.id)).is_some());
    assert!(bounds(cx, "inbox-properties").is_some());
    assert!(bounds(cx, "inbox-property-state").is_some());
}

#[gpui::test]
fn read_wait_has_no_unread_dot_even_while_open(cx: &mut TestAppContext) {
    let mut document: serde_json::Value = serde_json::from_str(&one_group()).unwrap();
    document["waits"][0]["is_read"] = true.into();
    let id = document["waits"][0]["id"].as_str().unwrap().to_string();
    let (_rig, _page, cx) = open_on(cx, &document.to_string(), MARKED, MARKED);
    assert!(bounds(cx, &format!("ask-{id}")).is_some());
    assert!(bounds(cx, &format!("dot-ask-{id}")).is_none());
}

#[gpui::test]
fn fifty_long_titles_fit_the_inbox_at_1280_by_800(cx: &mut TestAppContext) {
    let mut document: serde_json::Value = serde_json::from_str(&one_group()).unwrap();
    let mut template = document["waits"][0].clone();
    template["title"] = "The selected question has a long title that wraps in its detail. "
        .repeat(10)
        .into();
    document["waits"] = (0..50)
        .map(|index| {
            template["id"] = format!("long-{index}").into();
            template.clone()
        })
        .collect::<Vec<_>>()
        .into();
    let (_rig, page, cx) = open_on(cx, &document.to_string(), MARKED, MARKED);
    cx.simulate_resize(gpui::size(gpui::px(1280.), gpui::px(800.)));
    cx.run_until_parked();
    let list = bounds(cx, "inbox-list").unwrap();
    let row_title = bounds(cx, "row-title-ask-long-0").unwrap();
    let detail_title = bounds(cx, "title-long-0").unwrap();
    let properties = bounds(cx, "inbox-properties").unwrap();
    assert!(row_title.right() <= list.right());
    assert!(detail_title.size.height > row_title.size.height * 2.);
    assert!(detail_title.right() <= properties.left());
    assert!(properties.right() <= gpui::px(1280.));
    page.read_with(cx, |page, _| assert_eq!(page.list_scroll.item_count(), 52));
}

#[gpui::test]
fn polish_y_inbox_dividers_reach_the_panel_and_the_detail_edges(cx: &mut TestAppContext) {
    let (_rig, _desk, cx) = crate::ui::engine_desk_tests::open(cx, SNAPSHOT);
    cx.simulate_resize(gpui::size(gpui::px(1280.), gpui::px(800.)));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let panel = bounds(cx, "page-panel").unwrap();
    let list = bounds(cx, "inbox-list").unwrap();
    let detail = bounds(cx, "inbox-detail").unwrap();
    let footer = bounds(cx, "inbox-actions").unwrap();
    let bottom = panel.bottom() - crate::theme::PANEL_RING_WIDTH;
    assert_eq!(
        list.bottom(),
        bottom,
        "the list divider reaches the panel ring"
    );
    assert_eq!(detail.bottom(), bottom);
    assert_eq!(footer.bottom(), bottom);
    assert_eq!(
        (footer.left(), footer.right()),
        (detail.left(), detail.right())
    );
    assert_eq!(footer.left(), list.right() + crate::theme::PANEL_RING_WIDTH);
    cx.update(|window, cx| {
        let scale = window.scale_factor();
        let border = crate::theme::Theme::palette(cx).border;
        let divider = gpui::Bounds::new(
            gpui::point(list.right(), list.top()),
            gpui::size(crate::theme::PANEL_RING_WIDTH, list.size.height),
        );
        let footer_rule = gpui::Bounds::new(
            footer.origin + gpui::point(crate::theme::PANEL_RING_WIDTH, gpui::px(0.)),
            gpui::size(
                footer.size.width - crate::theme::PANEL_RING_WIDTH,
                crate::theme::PANEL_RING_WIDTH,
            ),
        );
        let quads = window.painted_quads();
        for rule in [divider, footer_rule] {
            assert!(
                quads.iter().any(|quad| {
                    quad.bounds == rule.scale(scale) && quad.background == border.into()
                }),
                "the divider fills {rule:?}"
            );
        }
    });
}

#[test]
fn an_ask_whose_text_is_a_list_of_one_item_reads_as_a_paragraph() {
    let plain = |text: &str| model::plain_text(text);
    assert_eq!(
        plain("1. Senza non vedi il repo."),
        "Senza non vedi il repo."
    );
    assert_eq!(
        plain("1. First line\n   second line"),
        "First line\nsecond line"
    );
    assert_eq!(
        plain("1. One\n2. Two"),
        "1. One\n2. Two",
        "a real list stays"
    );
    assert_eq!(plain("Just words"), "Just words");
}

#[test]
fn the_verdicts_of_a_check_are_done_and_not_now_and_those_of_an_approval_stay_approve_and_decline()
{
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let mark = verbs_of("owner_row", &[("k", &["verdict"], &["note"])]);
    let primary = mark.primary.expect("a primary");
    assert_eq!(primary.label().as_ref(), "Mark done");
    assert_eq!(mark.secondary[0].label().as_ref(), "Not now");
    assert_eq!(primary.say.done_words().as_ref(), "Approved");
    assert_eq!(primary.done_words().as_ref(), "Done");
    let approval = verbs_of("approval", &[("a", &[], &[])]);
    assert_eq!(
        approval.primary.expect("a primary").label().as_ref(),
        "Approve"
    );
}

/// The fixture with no ask and every lane stopped.
fn all_stopped() -> String {
    let mut snapshot: serde_json::Value = serde_json::from_str(SNAPSHOT).expect("the fixture");
    snapshot["waits"] = serde_json::json!([]);
    for item in snapshot["items"].as_array_mut().expect("the items") {
        item["agent_state"] = "stopped".into();
    }
    snapshot.to_string()
}

#[gpui::test]
fn with_no_ask_and_stopped_lanes_the_page_says_how_many_are_stopped_and_links_to_lanes(
    cx: &mut TestAppContext,
) {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let rig = engine_rig_tests::open(cx, &all_stopped(), MARKED, MARKED);
    let (facts, source) = (rig.facts.clone(), rig.source.clone());
    let (_, cx) = cx.add_window_view(move |window, cx| {
        let answers = cx.new(|cx| Answers::new(facts.clone(), source.clone(), cx));
        let page = cx.new(|cx| InboxPage::new(facts, answers, source, cx));
        gpui_component::Root::new(
            cx.new(|_| Frame {
                page,
                sidebar: gpui::Pixels::ZERO,
            }),
            window,
            cx,
        )
    });
    wait_until(cx, "the empty page", |cx| {
        cx.debug_bounds("inbox-empty").is_some()
    });
    assert!(cx.debug_bounds("inbox-stopped").is_some());
    assert!(cx.debug_bounds("inbox-running").is_none(), "none runs");
    assert!(cx.debug_bounds("inbox-see-lanes").is_some());
}

#[gpui::test]
fn the_band_count_is_next_to_its_title_and_outside_the_age_column(cx: &mut TestAppContext) {
    let (_rig, _page, cx) = open(cx, MARKED, MARKED);
    let snapshot = parse(SNAPSHOT);
    let first = &model::groups(&snapshot)[0].asks[0].id.clone();
    let title = bounds(cx, "group-0-title").expect("the band's title");
    let count = bounds(cx, "group-0-count").expect("the band's count");
    let row_title = bounds(cx, &format!("row-title-ask-{first}")).expect("the row's title");
    let row_age = bounds(cx, &format!("row-age-ask-{first}")).expect("the row's time");
    assert_eq!(title.left(), row_title.left(), "text starts on one line");
    assert_eq!(count.left() - title.right(), crate::theme::GAP);
    assert!(count.right() < row_age.left());
}

const RAW_ERROR: &str = "pull request lookup failed: To get started with GitHub CLI, please run:  gh auth login; see /home/someone/engine-home/measure/runs/operations/pull-request-lookup.log";

/// The fixture whose blocked lane says `detail`, and the engine's raw line behind it (`technical`) when given.
fn blocked_with(detail: &str, technical: Option<&str>) -> String {
    let mut snapshot: serde_json::Value = serde_json::from_str(SNAPSHOT).expect("the fixture");
    let waits = snapshot["waits"].as_array_mut().expect("the waits");
    let blocked = waits
        .iter_mut()
        .find(|wait| wait["kind"] == "blocked")
        .expect("a blocked ask");
    blocked["detail"] = detail.into();
    if let Some(technical) = technical {
        blocked["technical"] = technical.into();
    }
    snapshot.to_string()
}

#[gpui::test]
fn github_auth_opens_connectors_at_the_top_without_showing_a_command(cx: &mut TestAppContext) {
    let mut snapshot: serde_json::Value =
        serde_json::from_str(&blocked_with(RAW_ERROR, None)).unwrap();
    snapshot["connectors"] = serde_json::json!([]);
    let (_rig, page, cx) = open_on(cx, &snapshot.to_string(), MARKED, MARKED);
    select(cx, BLOCKED);
    assert_eq!(
        page.read_with(cx, |page, _| page.body_text(BLOCKED))
            .as_deref(),
        Some("GitHub is not connected: Sakura cannot open the pull request.")
    );
    assert!(bounds(cx, &format!("ask-details-{BLOCKED}")).is_none());
    click(cx, "inbox-connect-github");
    wait_until(cx, "the GitHub connector", |cx| {
        bounds(cx, "settings-connector-github-button").is_some()
    });
    let viewport = bounds(cx, "settings-scroll").unwrap();
    let intro = bounds(cx, "settings-connectors-intro").unwrap();
    let github = bounds(cx, "settings-connector-github").unwrap();
    assert!(
        intro.top() >= viewport.top(),
        "{intro:?} above {viewport:?}"
    );
    assert!(github.top() >= intro.bottom() && github.bottom() <= viewport.bottom());
}

#[gpui::test]
fn a_blocked_ask_keeps_a_diagnostic_path_behind_details(cx: &mut TestAppContext) {
    let raw = "pull request lookup failed: see /tmp/engine/lookup.log";
    let (_rig, page, cx) = open_on(cx, &blocked_with(raw, None), MARKED, MARKED);
    select(cx, BLOCKED);
    assert_eq!(
        page.read_with(cx, |page, _| page.body_text(BLOCKED))
            .as_deref(),
        Some("Pull request lookup failed."),
        "a plain sentence naming what failed"
    );
    let details = format!("ask-details-{BLOCKED}");
    assert!(
        bounds(cx, &details).is_some(),
        "the raw text is behind Details"
    );
    assert!(
        bounds(cx, &format!("ask-details-text-{BLOCKED}")).is_none(),
        "closed"
    );
    click(cx, &details);
    wait_until(cx, "the raw text", |cx| {
        bounds(cx, &format!("ask-details-text-{BLOCKED}")).is_some()
    });
    click(cx, &details);
    wait_until(cx, "closed again", |cx| {
        bounds(cx, &format!("ask-details-text-{BLOCKED}")).is_none()
    });
}

#[gpui::test]
fn the_engine_s_words_for_a_blocked_ask_are_shown_when_it_gives_them(cx: &mut TestAppContext) {
    let snapshot = blocked_with(
        "GitHub is not signed in, so the pull request could not be looked up.",
        Some("pull request lookup failed: see /tmp/engine/lookup.log"),
    );
    let (_rig, page, cx) = open_on(cx, &snapshot, MARKED, MARKED);
    select(cx, BLOCKED);
    assert_eq!(
        page.read_with(cx, |page, _| page.body_text(BLOCKED))
            .as_deref(),
        Some("GitHub is not signed in, so the pull request could not be looked up.")
    );
    assert!(bounds(cx, &format!("ask-details-{BLOCKED}")).is_some());
}

#[gpui::test]
fn a_blocked_ask_in_plain_words_has_no_details_to_open(cx: &mut TestAppContext) {
    let (_rig, page, cx) = open(cx, MARKED, MARKED);
    select(cx, BLOCKED);
    assert_eq!(
        page.read_with(cx, |page, _| page.body_text(BLOCKED))
            .as_deref(),
        Some("The builder ran out of budget.")
    );
    assert!(bounds(cx, &format!("ask-details-{BLOCKED}")).is_none());
}

/// The fixture whose approval of the lane `gated` is the ask to deliver although the review wants changes.
fn with_findings(findings: &[&str]) -> String {
    let mut snapshot: serde_json::Value = serde_json::from_str(SNAPSHOT).expect("the fixture");
    let waits = snapshot["waits"].as_array_mut().expect("the waits");
    let ask = waits.iter_mut().find(|wait| wait["kind"] == "approval");
    let ask = ask.expect("an approval");
    ask["key"][4] = "final-review".into();
    ask["id"] = r#"["wait","/work/repo","gated","approval","final-review"]"#.into();
    ask["title"] = "Deliver although the review still wants changes?".into();
    ask["detail"] = "The reviewer still requested changes after 1 round.".into();
    ask["findings"] = findings.into();
    snapshot.to_string()
}

const FINAL_REVIEW: &str = r#"["wait","/work/repo","gated","approval","final-review"]"#;

#[gpui::test]
fn the_ask_to_deliver_although_the_review_wants_changes_shows_the_reviewer_s_findings(
    cx: &mut TestAppContext,
) {
    let findings = [
        "The parser drops a trailing quote in tests/parse_test.py.",
        "No test covers an empty query.",
    ];
    let (_rig, _page, cx) = open_on(cx, &with_findings(&findings), MARKED, MARKED);
    select(cx, FINAL_REVIEW);
    for at in 0..2 {
        assert!(
            bounds(cx, &format!("ask-findings-line-{FINAL_REVIEW}-{at}")).is_some(),
            "finding {at}"
        );
    }
    assert!(bounds(cx, &format!("ask-findings-line-{FINAL_REVIEW}-2")).is_none());
}

#[gpui::test]
fn a_stuck_ask_that_carries_findings_shows_them_like_the_deliver_anyway_ask(
    cx: &mut TestAppContext,
) {
    let mut snapshot: serde_json::Value =
        serde_json::from_str(&blocked_with("The fix builder made no commit.", None))
            .expect("a snapshot");
    let waits = snapshot["waits"].as_array_mut().expect("the waits");
    let blocked = waits.iter_mut().find(|wait| wait["kind"] == "blocked");
    blocked.expect("a blocked ask")["findings"] =
        serde_json::json!(["No test covers an empty query."]);
    let (_rig, _page, cx) = open_on(cx, &snapshot.to_string(), MARKED, MARKED);
    select(cx, BLOCKED);
    assert!(bounds(cx, &format!("ask-findings-line-{BLOCKED}-0")).is_some());
}

#[test]
fn the_overlap_ask_names_one_lane_or_several_in_its_title_as_its_sentence_does() {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let snapshot = Snapshot::parse(SNAPSHOT.as_bytes()).expect("the fixture");
    let mut ask = snapshot.waits[0].clone();
    ask.kind = "approval".into();
    ask.key = ["wait", "/work/repo", "gated", "approval", "overlap@a91"]
        .map(String::from)
        .to_vec();
    ask.title = "Delivery overlaps another lane".into();
    let mut said = |detail: &str| {
        ask.detail = detail.into();
        model::ask_title_words(&ask)
    };
    assert_eq!(
        said("\"Lines and characters\" delivered before this lane and changed files this lane also changes: tally.py."),
        "Delivery overlaps another lane"
    );
    assert_eq!(
        said("\"Lines and characters\" and \"Reverse and contributing\" delivered before this lane and changed files this lane also changes: tally.py."),
        "Delivery overlaps other lanes"
    );
    ask.title = "Run these 3 checks?".into();
    ask.key[4] = "checks".into();
    assert_eq!(
        model::ask_title_words(&ask),
        "Run these 3 checks?",
        "another ask is as the engine says it"
    );
}

#[gpui::test]
fn a_long_finding_wraps_whole_and_is_cut_nowhere_by_the_app(cx: &mut TestAppContext) {
    let long = "The parser drops a trailing quote and the empty query returns every row, which the results page then shows as a failure of its own; nothing tests either case. ".repeat(6);
    let (_rig, _page, cx) = open_on(cx, &with_findings(&[long.trim(), "Short."]), MARKED, MARKED);
    select(cx, FINAL_REVIEW);
    let long_line =
        bounds(cx, &format!("ask-findings-line-{FINAL_REVIEW}-0")).expect("the long finding");
    let short_line =
        bounds(cx, &format!("ask-findings-line-{FINAL_REVIEW}-1")).expect("the short one");
    assert!(
        long_line.size.height > short_line.size.height * 3.,
        "it wraps onto several lines instead of being cut to one: {:?} against {:?}",
        long_line.size.height,
        short_line.size.height
    );
}

/// An overlap ask whose sentence names two lanes, in the snapshot's waits.
fn overlap_with_two_lanes() -> Snapshot {
    let mut snapshot = Snapshot::parse(SNAPSHOT.as_bytes()).expect("the fixture");
    let ask = &mut snapshot.waits[0];
    ask.kind = "approval".into();
    ask.key = ["wait", "/work/repo", "demo", "approval", "overlap@a91"]
        .map(String::from)
        .to_vec();
    ask.title = "Delivery overlaps another lane".into();
    ask.detail = "\"A\" and \"B\" delivered before this lane: tally.py.".into();
    snapshot
}

#[test]
fn a_title_the_engine_cut_ends_with_an_ellipsis_and_a_whole_one_is_left_alone() {
    let snapshot = Snapshot::parse(SNAPSHOT.as_bytes()).expect("the fixture");
    let mut ask = snapshot.waits[0].clone();
    ask.kind = "owner_row".into();
    ask.title = "Try it yourself: Trigger build-windows.yml by hand on a Windows machine or runner and confirm the".into();
    ask.detail = "Trigger build-windows.yml by hand on a Windows machine or runner and confirm the Smoke test step reports no error.".into();
    assert_eq!(
        model::ask_title_words(&ask),
        format!("{}…", ask.title),
        "the rest of the sentence is in the body: the title was cut"
    );
    ask.title = "Try it yourself: Open the page.".into();
    ask.detail = "Open the page.".into();
    assert_eq!(
        model::ask_title_words(&ask),
        "Try it yourself: Open the page.",
        "whole"
    );
    ask.title = "Check row R1".into();
    ask.detail = "Open the page and check the table.".into();
    assert_eq!(
        model::ask_title_words(&ask),
        "Check row R1",
        "a title that is not the start of its body"
    );
}

#[test]
fn the_same_overlap_ask_reads_the_same_title_in_every_place_that_shows_one() {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let snapshot = overlap_with_two_lanes();
    let ask = &snapshot.waits[0];
    let plural = "Delivery overlaps other lanes";
    assert_eq!(
        super::detail::ask_title(ask, &snapshot),
        plural,
        "the Inbox"
    );
    assert_eq!(
        model::ask_title_words(ask),
        plural,
        "the lane's header and card"
    );
    let told = crate::ui::engine_notifying::wait_title(ask);
    assert!(told.ends_with(plural), "the notification: {told}");
    // The right column's card and every other place go through the same function, not the engine's field.
    for (file, source) in [
        (
            "lane_side/details.rs",
            include_str!("../lane_side/details.rs"),
        ),
        (
            "engine_notifying.rs",
            include_str!("../engine_notifying.rs"),
        ),
    ] {
        let code = source.split("#[cfg(test)]").next().unwrap_or_default();
        assert!(
            !code.contains("wait.title"),
            "{file} shows the engine's title as it is"
        );
    }
}

#[gpui::test]
fn an_ask_with_no_findings_shows_no_findings_block(cx: &mut TestAppContext) {
    let (_rig, _page, cx) = open_on(cx, &with_findings(&[]), MARKED, MARKED);
    select(cx, FINAL_REVIEW);
    assert!(bounds(cx, &format!("ask-findings-{FINAL_REVIEW}")).is_none());
}

#[gpui::test]
fn the_engine_s_words_for_a_stuck_ask_are_never_read_as_markdown(cx: &mut TestAppContext) {
    let words = "plan reviewer changed the workspace during arbiter: __pycache__/tally.cpython-314.pyc, tests/__pycache__/test_reverse.cpython-314.pyc";
    let (_rig, page, cx) = open_on(cx, &blocked_with(words, None), MARKED, MARKED);
    select(cx, BLOCKED);
    assert_eq!(
        page.read_with(cx, |page, _| page.body_text(BLOCKED))
            .as_deref(),
        Some(words),
        "every character as the engine wrote it"
    );
    assert!(
        !page.read_with(cx, |page, _| page.bodies.contains_key(BLOCKED)),
        "no Markdown parser reads it"
    );
    assert!(bounds(cx, &format!("body-{BLOCKED}")).is_some());
}

#[gpui::test]
fn an_engine_ask_renders_bold_from_the_planner(cx: &mut TestAppContext) {
    let words = "The planner said: \"**Why it stopped:** e2e/json_option/check_json_option.py calls run_measure.\"";
    let (_rig, page, cx) = open_on(cx, &blocked_with(words, None), MARKED, MARKED);
    select(cx, BLOCKED);
    page.read_with(cx, |page, cx| {
        let (_, body) = page.bodies.get(BLOCKED).expect("the Markdown detail");
        let words = body.read(cx).rendered_text();
        assert!(words
            .as_str()
            .contains("Why it stopped: e2e/json_option/check_json_option.py calls run_measure."));
        assert!(!words.as_str().contains("**"));
    });
}

#[gpui::test]
fn a_blocked_ask_with_no_plain_sentence_shows_its_technical_line_when_that_is_readable(
    cx: &mut TestAppContext,
) {
    let reason = "the code reviewer did not approve the contract: a correct README can make R12 pass when the example is not under a usage heading; the parser must skip fenced code blocks";
    let (_rig, page, cx) = open_on(cx, &blocked_with("", Some(reason)), MARKED, MARKED);
    select(cx, BLOCKED);
    assert_eq!(
        page.read_with(cx, |page, _| page.body_text(BLOCKED))
            .as_deref(),
        Some(reason),
        "its own words, not the generic sentence"
    );
    assert!(
        bounds(cx, &format!("ask-details-{BLOCKED}")).is_none(),
        "nothing hidden"
    );
}

/// The fixture whose lane `gated` has the progress `progress`.
fn gated_with_progress(progress: serde_json::Value) -> String {
    let mut snapshot: serde_json::Value = serde_json::from_str(SNAPSHOT).expect("the fixture");
    let items = snapshot["items"].as_array_mut().expect("the items");
    let gated = items
        .iter_mut()
        .find(|item| item["key"][2] == "gated")
        .expect("gated");
    gated["progress"] = progress;
    snapshot.to_string()
}

#[gpui::test]
fn a_lane_with_nothing_to_count_leaves_no_empty_panel_under_its_ask(cx: &mut TestAppContext) {
    let progress = serde_json::json!({"phase": "pull_request", "done": 0, "total": 0});
    let (_rig, _page, cx) = open_on(cx, &gated_with_progress(progress), MARKED, MARKED);
    select(cx, APPROVAL);
    assert!(
        bounds(cx, "inbox-context").is_none(),
        "a panel with only a phase word says nothing"
    );
}

#[gpui::test]
fn a_lane_that_counts_its_steps_says_so_under_its_ask(cx: &mut TestAppContext) {
    let progress = serde_json::json!({"phase": "build", "done": 1, "total": 3});
    let (_rig, _page, cx) = open_on(cx, &gated_with_progress(progress), MARKED, MARKED);
    select(cx, APPROVAL);
    assert!(bounds(cx, "inbox-context").is_some());
}

/// The ask the engine makes when checks are turned on during a lane or their commands changed (its commit 853d8a4):
/// the checks gate's own approval, in the shape of the other asks.
fn checks_ask_of_a_running_lane() -> String {
    serde_json::json!({
        "id": r#"["wait","/work/repo","gated","approval","checks"]"#,
        "key": ["wait", "/work/repo", "gated", "approval", "checks"], "kind": "approval",
        "repo": "/work/repo", "project": "repo", "task": "gated", "since": null,
        "title": "Run these 2 checks?",
        "detail": "They prove the lane's rows pass.\n\n- cargo test\n- cargo clippy -- -D warnings",
        "technical": "2 contract commands changed and wait for the person's approval; see sakura flow status",
        "actions": [{"key": "a", "label": "Approve", "needs": [], "optional": ["remember"], "confirm": true}]
    })
    .to_string()
}

#[test]
fn the_checks_ask_of_a_running_lane_offers_approve_and_says_its_commands_with_no_command_line_of_the_engine_s(
) {
    let snapshot = snapshot_of(&[checks_ask_of_a_running_lane()]);
    let wait = &snapshot.waits[0];
    let verbs = model::verbs(wait, None);
    assert_eq!(verbs.primary.expect("Approve").say, Say::Approve);
    let words = model::plain_reason(wait).words;
    assert!(
        words.contains("- cargo test") && words.contains("- cargo clippy"),
        "{words}"
    );
    assert!(!words.contains("sakura"), "{words}");
}

#[gpui::test]
fn polish_zd_checks_approval_lists_every_command(cx: &mut TestAppContext) {
    let commands = [
        "python3 e2e/version_option.py exit",
        "python3 e2e/version_option.py usage-last",
    ];
    let mut fixture: serde_json::Value = serde_json::from_str(&at_checks_gate()).unwrap();
    let ask = fixture["waits"][0]["id"].as_str().unwrap().to_string();
    fixture["waits"][0]["detail"] = format!(
        "They prove the change works.\n\n- {}\n- {}",
        commands[0], commands[1]
    )
    .into();
    let (_rig, _page, cx) = open_with_sidebar(
        cx,
        &fixture.to_string(),
        MARKED,
        MARKED,
        crate::theme::SIDEBAR_WIDTH,
    );
    cx.simulate_resize(gpui::size(gpui::px(1280.), gpui::px(800.)));
    select(cx, &ask);
    assert!(bounds(cx, "inbox-checks-summary").is_some());
    for command in commands {
        assert!(bounds(cx, &format!("inbox-check-command-{command}")).is_some());
    }
}

#[test]
fn an_approval_says_every_command_it_asks_to_approve_word_for_word_even_one_that_names_the_engine_s_program(
) {
    let mut ask: serde_json::Value =
        serde_json::from_str(&checks_ask_of_a_running_lane()).expect("an ask");
    ask["detail"] =
        "They prove the rows pass.\n\n- cargo test\n- run sakura flow status --json".into();
    let snapshot = snapshot_of(&[ask.to_string()]);
    let words = model::plain_reason(&snapshot.waits[0]).words;
    assert!(words.contains("- run sakura flow status --json"), "{words}");
}

#[test]
fn needs_you_precedes_triage_and_group_ids_merge_source_items() {
    let document = |task: &str, kind: &str, section: &str, since: &str| {
        let mut wait: serde_json::Value =
            serde_json::from_str(&wait(task, kind, Some(since), &[])).unwrap();
        wait["section"] = section.into();
        wait["group"] = serde_json::json!({"id": "source", "label": "Shared source"});
        wait.to_string()
    };
    let snapshot = snapshot_of(&[
        document("first", "proposal", "triage", "2026-11-18T08:00:00Z"),
        document("second", "proposal", "triage", "2026-11-18T10:00:00Z"),
        document("third", "review", "needs_you", "2026-11-17T04:00:00Z"),
    ]);
    let groups = model::groups(&snapshot);
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0].asks[0].task, "third");
    assert_eq!(groups[1].title.as_ref(), "Shared source");
    assert_eq!(groups[1].asks[0].task, "second");
    assert_eq!(groups[1].asks[1].task, "first");
}

#[gpui::test]
fn return_marks_a_review_reviewed_through_its_wait_id(cx: &mut TestAppContext) {
    let ask = wait("review", "review", None, &[("markreviewed", &[], &[])]);
    let document = format!(r#"{{"api":1,"waits":[{ask}],"items":[],"events":[],"problems":[]}}"#);
    let snapshot = parse(&document);
    let (rig, _page, cx) = open_on(cx, &document, MARKED, MARKED);
    select(cx, &snapshot.waits[0].id);
    cx.simulate_keystrokes("enter");
    wait_for(cx, Duration::from_secs(7));
    wait_until_sent(&rig, 1);
    assert!(rig.acts()[0].contains("markreviewed"));
}

#[gpui::test]
fn attention_shortcuts_send_read_and_snooze_without_resolving(cx: &mut TestAppContext) {
    let ask = wait(
        "attention",
        "question",
        None,
        &[
            ("read", &[], &[]),
            ("unread", &[], &[]),
            ("snooze", &["until"], &[]),
        ],
    );
    let document = format!(r#"{{"api":1,"waits":[{ask}],"items":[],"events":[],"problems":[]}}"#);
    let (rig, page, cx) = open_on(cx, &document, MARKED, MARKED);
    let id = parse(&document).waits[0].id.clone();
    page.update(cx, |page, _| assert!(page.held_words(&id).is_none()));
    cx.update(|window, cx| page.update(cx, |page, cx| window.focus(&page.focus, cx)));
    rig.say_delay("0.3");
    cx.simulate_keystrokes("u");
    assert!(
        bounds(cx, &format!("dot-ask-{id}")).is_some(),
        "the unread dot stays until the reply"
    );
    assert!(bounds(cx, "inbox-attention-pending").is_some());
    assert!(rig.acts().is_empty());
    cx.simulate_keystrokes("u");
    wait_until_sent(&rig, 1);
    assert!(rig.acts()[0].contains(" read "));
    wait_until(cx, "read result", |cx| {
        told(&page, cx, &crate::ui::answers::lane_key(&id, "attention")).is_some()
    });
    assert_eq!(rig.acts().len(), 1);
    assert!(
        bounds(cx, &format!("dot-ask-{id}")).is_none(),
        "the dot stays off after the reply"
    );
    cx.simulate_keystrokes("u");
    assert!(bounds(cx, &format!("dot-ask-{id}")).is_none());
    wait_until_sent(&rig, 2);
    assert!(rig.acts()[1].contains(" unread "));
    wait_until(cx, "unread result", |cx| {
        told(&page, cx, &crate::ui::answers::lane_key(&id, "attention")).is_some()
    });
    cx.simulate_keystrokes("h");
    wait_until(cx, "snooze choices", |cx| {
        bounds(cx, "inbox-snooze-tomorrow").is_some()
    });
    click(cx, "inbox-snooze-tomorrow");
    wait_until_sent(&rig, 3);
    assert!(rig.acts()[2].contains("snooze"));
    let calls = rig.acts();
    let wake = calls[2]
        .split_once("--until ")
        .unwrap()
        .1
        .split_whitespace()
        .next()
        .unwrap();
    assert!(
        sakura_core::engine::moment::iso_seconds(wake).is_some(),
        "{wake}"
    );
    assert!(page.read_with(cx, |page, _| page.held_words(&id).is_none()));
}

#[test]
fn snoozed_status_uses_today_or_the_local_date_and_keeps_the_clock_time() {
    use chrono::{Local, Timelike as _};

    crate::l10n::set_for_test(crate::l10n::Language::English);
    let today = Local::now().with_hour(17).unwrap().with_minute(29).unwrap();
    let tomorrow = today
        .date_naive()
        .succ_opt()
        .unwrap()
        .and_time(today.time())
        .and_local_timezone(Local)
        .single()
        .unwrap();
    let mut wait = parse(SNAPSHOT).waits.remove(0);
    wait.snoozed_until = Some(today.to_rfc3339());
    assert_eq!(model::snoozed_status(&wait), "Snoozed until today at 17:29");
    assert_eq!(
        model::wake_words(&wait).as_deref(),
        Some("Wakes today at 17:29")
    );
    wait.snoozed_until = Some(tomorrow.to_rfc3339());
    assert_eq!(
        model::snoozed_status(&wait).as_ref(),
        format!("Snoozed until {} at 17:29", tomorrow.format("%Y-%m-%d"))
    );
    assert_eq!(
        model::wake_words(&wait).unwrap().as_ref(),
        format!("Wakes {} at 17:29", tomorrow.format("%Y-%m-%d"))
    );
}

#[gpui::test]
fn a_snoozed_wait_is_hidden_from_to_do_even_when_the_engine_lists_it(cx: &mut TestAppContext) {
    let (rig, page, cx) =
        open_with_sidebar(cx, SNAPSHOT, MARKED, MARKED, crate::theme::SIDEBAR_WIDTH);
    cx.simulate_resize(gpui::size(gpui::px(1280.), gpui::px(800.)));
    let mut document: serde_json::Value = serde_json::from_str(SNAPSHOT).unwrap();
    for wait in document["waits"].as_array_mut().unwrap() {
        wait["snoozed_until"] = "2099-11-22T18:00:00Z".into();
    }
    rig.say_snapshot(&document.to_string());
    rig.facts.update(cx, |facts, cx| facts.read(cx));
    wait_until(cx, "the snoozed snapshot", |cx| {
        rig.facts.read_with(cx, |facts, _| {
            facts.snapshot().unwrap().waits[0].snoozed_until.is_some()
        })
    });
    assert!(bounds(cx, "inbox-empty").is_some());
    page.update(cx, |page, cx| {
        page.set_filter(
            model::Filter {
                snoozed: true,
                ..Default::default()
            },
            cx,
        );
    });
    select(cx, APPROVAL);
    let snoozed = bounds(cx, "inbox-property-snoozed").expect("Snoozed status with the wake time");
    let properties = bounds(cx, "inbox-properties").expect("Properties");
    assert!(
        snoozed.right() <= properties.right()
            && snoozed.size.height > crate::theme::TEXT_LABEL * 2.,
        "snoozed status {snoozed:?} does not wrap inside Properties {properties:?}"
    );
    let wake = bounds(cx, "inbox-snooze-wake").expect("wake time");
    let body = bounds(cx, &format!("body-{APPROVAL}")).expect("the spec");
    assert!(body.top() - wake.bottom() >= crate::theme::GAP);
    assert!(bounds(cx, "inbox-list").is_some());
}

#[gpui::test]
fn a_snoozed_engine_wait_offers_unsnooze_and_returns_to_to_do(cx: &mut TestAppContext) {
    let active = parse(&one_group()).waits[0].id.clone();
    let mut document: serde_json::Value = serde_json::from_str(&one_group()).unwrap();
    let waits = document["waits"].as_array_mut().unwrap();
    waits.truncate(1);
    let unsnoozed = document.to_string();
    let (rig, page, cx) = open_on(cx, &unsnoozed, MARKED, MARKED);
    document["waits"][0]["snoozed_until"] = "2099-11-22T18:00:00Z".into();
    document["waits"][0]["actions"] = serde_json::json!([{"key": "unsnooze", "label": "Unsnooze"}]);
    rig.say_snapshot(&document.to_string());
    rig.facts.update(cx, |facts, cx| facts.read(cx));
    wait_until(cx, "To do is empty", |cx| {
        bounds(cx, "inbox-empty").is_some()
    });
    page.update(cx, |page, cx| {
        page.set_filter(
            model::Filter {
                snoozed: true,
                ..Default::default()
            },
            cx,
        );
    });
    wait_until(cx, "the snoozed wait", |cx| {
        bounds(cx, "inbox-snooze-wake").is_some()
    });
    assert!(
        bounds(cx, "inbox-unsnooze").is_some(),
        "the engine offered Unsnooze"
    );
    rig.say_snapshot(&unsnoozed);
    click(cx, "inbox-unsnooze");
    wait_until_sent(&rig, 1);
    assert!(rig.acts()[0].starts_with(&format!("cockpit act unsnooze {active}")));
    wait_until(cx, "Snoozed is empty", |cx| {
        bounds(cx, "inbox-snoozed-empty").is_some()
    });
    page.update(cx, |page, cx| page.show(Shown::Open, cx));
    assert!(bounds(cx, &format!("ask-{active}")).is_some());
}

#[gpui::test]
fn a_desktop_snooze_remains_visible_when_the_engine_omits_the_wait(cx: &mut TestAppContext) {
    let ask = wait(
        "attention",
        "question",
        None,
        &[("snooze", &["until"], &[])],
    );
    let document = format!(r#"{{"api":1,"waits":[{ask}],"items":[],"events":[],"problems":[]}}"#);
    let (rig, page, cx) = open_on(cx, &document, MARKED, MARKED);
    cx.simulate_keystrokes("h");
    wait_until(cx, "snooze choices", |cx| {
        bounds(cx, "inbox-snooze-1h").is_some()
    });
    rig.say_snapshot(NOTHING);
    click(cx, "inbox-snooze-1h");
    wait_until(cx, "the omitted wait", |cx| {
        rig.facts
            .read_with(cx, |facts, _| facts.snapshot().unwrap().waits.is_empty())
    });
    page.update(cx, |page, cx| {
        page.set_filter(
            model::Filter {
                snoozed: true,
                ..Default::default()
            },
            cx,
        );
    });
    assert!(bounds(cx, "inbox-snooze-wake").is_some());
}

#[gpui::test]
fn snooze_uses_the_same_label_and_control_size_as_mark_read(cx: &mut TestAppContext) {
    let ask = wait(
        "attention",
        "question",
        None,
        &[("read", &[], &[]), ("snooze", &["until"], &[])],
    );
    let document = format!(r#"{{"api":1,"waits":[{ask}],"items":[],"events":[],"problems":[]}}"#);
    let (_rig, _page, cx) = open_on(cx, &document, MARKED, MARKED);
    let read = bounds(cx, "inbox-read-label").expect("the read label");
    let snooze = bounds(cx, "inbox-snooze-label").expect("the Snooze label");
    assert_eq!(snooze.size.height, read.size.height);
    assert_eq!(
        bounds(cx, "inbox-snooze").unwrap().size.height,
        bounds(cx, "inbox-read").unwrap().size.height
    );
}

#[gpui::test]
fn starting_a_proposal_sends_the_operation_id_and_editable_defaults(cx: &mut TestAppContext) {
    let mut document: serde_json::Value = serde_json::from_str(NOTHING).unwrap();
    let mut proposal: serde_json::Value = serde_json::from_str(&wait(
        "incoming",
        "proposal",
        None,
        &[("start", &["op"], &["project", "title", "preset", "account"])],
    ))
    .unwrap();
    proposal["section"] = "triage".into();
    proposal["proposal"] = serde_json::json!({"op":"shared-op", "project":"/work/repo",
        "preset":"lean", "account":"claude/work", "title":"Handle incoming request"});
    document["waits"] = serde_json::json!([proposal]);
    let (rig, _page, cx) = open_on(cx, &document.to_string(), DONE, DONE);
    click(cx, "verb-start");
    wait_until(cx, "proposal options", |cx| {
        bounds(cx, "inbox-proposal-editor").is_some()
    });
    assert!(rig.acts().is_empty());
    click(cx, "proposal-submit");
    wait_for(cx, Duration::from_secs(7));
    wait_until_sent(&rig, 1);
    let act = &rig.acts()[0];
    for part in [
        "cockpit act start ",
        "--op shared-op",
        "--project /work/repo",
        "--preset lean",
        "--account claude/work",
        "--title Handle incoming request",
    ] {
        assert!(act.contains(part), "{act}");
    }
}

#[gpui::test]
fn a_phone_answer_replaces_the_selected_wait_with_history_on_refresh(cx: &mut TestAppContext) {
    let (rig, page, cx) = open(cx, MARKED, MARKED);
    select(cx, OWNER_ROW);
    let mut snapshot: serde_json::Value = serde_json::from_str(SNAPSHOT).unwrap();
    snapshot["waits"]
        .as_array_mut()
        .unwrap()
        .retain(|wait| wait["id"] != OWNER_ROW);
    snapshot["decided"] = serde_json::json!([{
        "id": OWNER_ROW, "kind":"owner_row", "project":"repo", "task":"demo",
        "answer":"mark_ok", "actor":"overlay:me:handset", "place":"phone"
    }]);
    rig.say_snapshot(&snapshot.to_string());
    rig.facts.update(cx, |facts, cx| facts.read(cx));
    wait_until(cx, "the answered wait leaves", |cx| {
        bounds(cx, &format!("ask-{OWNER_ROW}")).is_none()
    });
    page.update(cx, |page, cx| page.show(Shown::Done, cx));
    wait_until(cx, "the answer from the phone", |cx| {
        bounds(cx, "inbox-resolved-by").is_some()
    });
    assert!(rig.acts().is_empty());
}

#[gpui::test]
fn fix37_resume_note_is_full_width_and_keeps_newlines(cx: &mut TestAppContext) {
    let (rig, _page, cx) = open_on(cx, &resume_note_snapshot(), MARKED, MARKED);
    select(cx, BLOCKED);
    click(cx, "verb-approve");
    let note = bounds(cx, "inbox-note").expect("Resume opened the note");
    let body = bounds(cx, &format!("body-{BLOCKED}")).unwrap();
    assert!(note.bottom() <= body.top());
    assert!(bounds(cx, "resume-without-note").is_some());
    let send = bounds(cx, "note-send").unwrap();
    assert!(send.top() > note.top() + send.size.height);
    assert_eq!(
        bounds(cx, "inbox-note-field").unwrap().size.width,
        note.size.width
    );
    click(cx, "inbox-note-field");
    cx.simulate_input("Retry with a clean worktree");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("Run the tests again");
    click(cx, "note-send");
    wait_for(cx, Duration::from_secs(7));
    wait_until_sent(&rig, 1);
    let sent = &rig.acts()[0];
    assert!(
        sent.starts_with(&format!("cockpit act R {BLOCKED}")),
        "{sent}"
    );
    assert!(
        sent.contains("--note Retry with a clean worktree"),
        "{sent}"
    );
    let log = std::fs::read_to_string(rig.dir.path().join("acts.log")).unwrap();
    assert!(log.contains("Retry with a clean worktree\nRun the tests again"));
}

fn resume_note_snapshot() -> String {
    let mut fixture: serde_json::Value = serde_json::from_str(SNAPSHOT).unwrap();
    let blocked = fixture["waits"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|wait| wait["id"] == BLOCKED)
        .unwrap();
    blocked["wants_note"] = true.into();
    blocked["actions"] = serde_json::json!([
        {"key": "R", "label": "Resume", "optional": ["note"]}
    ]);
    fixture.to_string()
}

#[gpui::test]
fn fix38_an_oversized_resume_note_stays_in_the_editor_and_is_never_sent(cx: &mut TestAppContext) {
    let (rig, page, cx) = open_on(cx, &resume_note_snapshot(), MARKED, MARKED);
    select(cx, BLOCKED);
    click(cx, "verb-approve");
    let limit = "é".repeat(2000);
    assert!(bounds(cx, "resume-note-count").is_none());
    cx.update(|window, cx| {
        let field = page.read(cx).resume_field().cloned().unwrap();
        field.update(cx, |field, cx| field.set_value(limit.clone(), window, cx));
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let has_count = bounds(cx, "resume-note-count").is_some();
    click(cx, "inbox-note-field");
    cx.simulate_input("x");
    click(cx, "note-send");
    wait_for(cx, Duration::from_secs(7));
    assert!(
        bounds(cx, "inbox-note").is_some(),
        "oversize Send closed the editor"
    );
    assert!(rig.acts().is_empty());
    assert!(has_count, "the counter must be visible at the limit");
    cx.update(|window, cx| {
        let field = page.read(cx).resume_field().cloned().unwrap();
        assert_eq!(field.read(cx).value().chars().count(), 2001);
        field.update(cx, |field, cx| field.set_value(limit.clone(), window, cx));
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    click(cx, "note-send");
    wait_for(cx, Duration::from_secs(7));
    wait_until_sent(&rig, 1);
}

#[gpui::test]
fn fix39_invalid_resume_characters_stay_in_the_editor_and_are_never_sent(cx: &mut TestAppContext) {
    let (rig, page, cx) = open_on(cx, &resume_note_snapshot(), MARKED, MARKED);
    select(cx, BLOCKED);
    click(cx, "verb-approve");
    let field = page.read_with(cx, |page, _| page.resume_field().cloned().unwrap());
    for invalid in ['\u{200b}', '\u{202e}', '\u{e0061}', '\u{0378}', '\u{001b}'] {
        let draft = format!("Try{invalid} again");
        cx.update(|window, cx| {
            field.update(cx, |field, cx| field.set_value(draft.clone(), window, cx));
            window.draw(cx).clear(cx);
        });
        assert!(bounds(cx, "resume-note-problem").is_some(), "{invalid:?}");
        click(cx, "note-send");
        cx.update(|window, cx| page.update(cx, |page, cx| page.send_note(window, cx)));
        wait_for(cx, Duration::from_secs(7));
        assert_eq!(field.read_with(cx, |field, _| field.value()), draft);
        assert!(bounds(cx, "inbox-note").is_some());
        assert!(rig.acts().is_empty());
    }
    cx.update(|window, cx| {
        field.update(cx, |field, cx| field.set_value("Try\n\tagain", window, cx));
        window.draw(cx).clear(cx);
    });
    assert!(bounds(cx, "resume-note-problem").is_none());
    click(cx, "note-send");
    wait_for(cx, Duration::from_secs(7));
    wait_until_sent(&rig, 1);
}

#[gpui::test]
fn fix39_a_refused_resume_keeps_the_multiline_draft_and_shows_the_reason(cx: &mut TestAppContext) {
    let refusal = r#"{"result":"refused","reason":"invalid note: refused text with a hidden character (bidi, zero-width, tag or unassigned)"}"#;
    let (rig, page, cx) = open_on(cx, &resume_note_snapshot(), refusal, MARKED);
    select(cx, BLOCKED);
    click(cx, "verb-approve");
    let draft = "Try again\nwith the revised checks";
    click(cx, "inbox-note-field");
    cx.simulate_input("Try again");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("with the revised checks");
    click(cx, "note-send");
    wait_for(cx, Duration::from_secs(7));
    wait_until_sent(&rig, 1);
    let key =
        sakura_core::engine::snapshot::Snapshot::parse(resume_note_snapshot().as_bytes()).unwrap();
    let wait = key.waits.iter().find(|wait| wait.id == BLOCKED).unwrap();
    let key = InboxPage::verbs_of(wait, &key).primary.unwrap().key;
    wait_until(cx, "the refusal", |cx| told(&page, cx, &key).is_some());
    assert!(
        bounds(cx, "inbox-note").is_some(),
        "refusal closed the editor"
    );
    assert_eq!(
        page.read_with(cx, |page, cx| page.resume_field().unwrap().read(cx).value()),
        draft
    );
    assert_eq!(
        told(&page, cx, &key).as_deref(),
        Some("The note has a hidden character: remove it and send again.")
    );
    let editor = bounds(cx, "inbox-note").unwrap();
    assert!(editor.contains(&bounds(cx, &format!("told-{key}")).unwrap().center()));
    rig.say_first(MARKED);
    click(cx, "note-send");
    wait_for(cx, Duration::from_secs(7));
    wait_until_sent(&rig, 2);
    wait_until(cx, "the editor closes on success", |cx| {
        bounds(cx, "inbox-note").is_none()
    });
}

#[gpui::test]
fn fix38_missing_folder_actions_explain_why_and_the_row_names_the_problem(cx: &mut TestAppContext) {
    use crate::ui::tip::{Tips, TipsHandle, COLD_DELAY};
    let tips = cx.update(|cx| {
        let tips = cx.new(|_| Tips::new());
        cx.set_global(TipsHandle(tips.clone()));
        tips
    });
    let (rig, page, cx) = open_on(cx, &missing_folder_snapshot(), MARKED, MARKED);
    select(cx, BLOCKED);
    let resume = bounds(cx, "verb-approve").unwrap();
    cx.simulate_mouse_move(resume.center(), None, Modifiers::none());
    cx.executor().advance_clock(COLD_DELAY);
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let shown = tips.read_with(cx, |tips, _| tips.shown().map(|shown| shown.words.clone()));
    let shown = shown.expect("disabled Resume has a tooltip");
    assert_eq!(shown.label.as_ref(), "The project folder is missing.");
    assert_eq!(shown.shortcut, None);
    assert!(bounds(cx, "inbox-folder-missing").is_some());
    let sub = page.read_with(cx, |page, cx| {
        let snapshot = page.snapshot(cx).unwrap();
        model::row_context(page.selected_ask(&snapshot).unwrap(), &snapshot)
    });
    assert_eq!(sub.as_ref(), "Folder missing");
    assert!(bounds(cx, &format!("sub-ask-{BLOCKED}")).is_some());
    cx.simulate_click(resume.center(), Modifiers::none());
    assert!(rig.acts().is_empty());
}

#[gpui::test]
fn fix38_disabled_resume_keeps_its_painted_face_on_hover(cx: &mut TestAppContext) {
    let (_rig, _page, cx) = open_on(cx, &missing_folder_snapshot(), MARKED, MARKED);
    select(cx, BLOCKED);
    let resume = bounds(cx, "verb-approve").unwrap();
    let painted = |cx: &mut VisualTestContext| {
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            let resume = resume.scale(window.scale_factor());
            window
                .painted_quads()
                .into_iter()
                .find(|quad| quad.bounds == resume)
                .expect("Resume's background quad")
                .background
        })
    };
    let before = painted(cx);
    cx.simulate_mouse_move(resume.center(), None, Modifiers::none());
    assert_eq!(painted(cx), before);
}

fn missing_folder_snapshot() -> String {
    let mut fixture: serde_json::Value = serde_json::from_str(&resume_note_snapshot()).unwrap();
    let project = fixture["projects"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|project| project["folder"] == "/work/other")
        .unwrap();
    project["folder_missing"] = true.into();
    fixture.to_string()
}

#[gpui::test]
fn fix36_checks_toast_adds_the_engine_summary_when_it_arrives(cx: &mut TestAppContext) {
    let summary = r#"{"result":"confirm","token":"17.ab","expires":"2026-11-16T08:02:00Z","summary":"Approve two checks: /work/repo: sakura flow approve gated checks"}"#;
    let (rig, _page, cx) = open_on(cx, &at_checks_gate(), summary, DONE);
    rig.say_delay("0.3");
    select(cx, r#"["wait","/work/repo","gated","approval","checks"]"#);
    cx.simulate_keystrokes("a");
    assert_eq!(
        cx.update(|window, cx| Toast::text_for_test(window, cx))
            .as_deref(),
        Some("Approving the checks of gated")
    );
    assert!(bounds(cx, "toast-detail").is_none());
    wait_until(cx, "the engine summary", |cx| {
        bounds(cx, "toast-detail").is_some()
    });
}

#[gpui::test]
fn review_resume_drafts_survive_switching_between_waits(cx: &mut TestAppContext) {
    let mut fixture: serde_json::Value = serde_json::from_str(&resume_note_snapshot()).unwrap();
    let waits = fixture["waits"].as_array_mut().unwrap();
    let mut other = waits
        .iter()
        .find(|wait| wait["id"] == BLOCKED)
        .unwrap()
        .clone();
    other["id"] = "other-resume".into();
    other["task"] = "other-task".into();
    waits.push(other);
    let (_rig, page, cx) = open_on(cx, &fixture.to_string(), MARKED, MARKED);
    for (id, draft) in [(BLOCKED, "first draft"), ("other-resume", "second draft")] {
        cx.update(|window, cx| {
            page.update(cx, |page, cx| {
                page.select(id.to_string(), cx);
                page.press_primary(window, cx);
                page.resume_field().unwrap().update(cx, |field, cx| {
                    field.set_value(draft, window, cx);
                });
            })
        });
    }
    for (id, draft) in [(BLOCKED, "first draft"), ("other-resume", "second draft")] {
        cx.update(|window, cx| {
            page.update(cx, |page, cx| {
                page.select(id.to_string(), cx);
                page.press_primary(window, cx);
                assert_eq!(page.resume_field().unwrap().read(cx).value(), draft);
            })
        });
    }
}

fn proposal_with_destination(is_missing: bool) -> String {
    let mut proposal: serde_json::Value = serde_json::from_str(&wait(
        "incoming",
        "proposal",
        None,
        &[("start", &["op"], &["project", "title"])],
    ))
    .unwrap();
    proposal["section"] = "triage".into();
    proposal["proposal"] = serde_json::json!({
        "op": "incoming", "project": "/work/destination", "title": "Edited title"
    });
    serde_json::json!({
        "api": 1, "waits": [proposal], "items": [],
        "projects": [
            {"id": "repo", "name": "Repo", "folder": "/work/repo"},
            {"id": "destination", "name": "Destination", "folder": "/work/destination",
             "folder_missing": is_missing}
        ]
    })
    .to_string()
}

fn mark_proposal_destination_missing(rig: &Rig, cx: &mut VisualTestContext) {
    rig.say_snapshot(&proposal_with_destination(true));
    rig.facts.update(cx, |facts, cx| facts.read(cx));
    wait_until(cx, "the destination folder is gone", |cx| {
        rig.facts.read_with(cx, |facts, _| {
            facts
                .snapshot()
                .unwrap()
                .is_project_folder_missing("/work/destination")
        })
    });
}

#[gpui::test]
fn review_proposal_submit_keeps_the_editor_if_the_destination_disappears(cx: &mut TestAppContext) {
    let (rig, _page, cx) = open_on(cx, &proposal_with_destination(false), DONE, DONE);
    click(cx, "verb-start");
    wait_until(cx, "the editor", |cx| {
        bounds(cx, "inbox-proposal-editor").is_some()
    });
    mark_proposal_destination_missing(&rig, cx);
    click(cx, "proposal-submit");
    assert!(bounds(cx, "inbox-proposal-editor").is_some());
    assert!(rig.acts().is_empty());
}

#[gpui::test]
fn review_proposal_hold_rechecks_its_destination_before_dispatch(cx: &mut TestAppContext) {
    let (rig, page, cx) = open_on(cx, &proposal_with_destination(false), DONE, DONE);
    let ask = selected(&page, cx).unwrap();
    click(cx, "verb-start");
    wait_until(cx, "the editor", |cx| {
        bounds(cx, "inbox-proposal-editor").is_some()
    });
    click(cx, "proposal-submit");
    mark_proposal_destination_missing(&rig, cx);
    wait_for(cx, Duration::from_secs(7));
    assert!(page.read_with(cx, |page, _| page.held_words(&ask).is_none()));
    assert!(page.read_with(cx, |page, cx| !page.is_away(&ask, cx)));
    assert!(rig.acts().is_empty());
}

#[gpui::test]
fn review_resume_opening_keeps_focus_on_the_invoking_control(cx: &mut TestAppContext) {
    let (_rig, page, cx) = open_on(cx, &resume_note_snapshot(), MARKED, MARKED);
    cx.update(|window, cx| {
        page.update(cx, |page, cx| {
            assert!(page.focus.is_focused(window));
            page.select(BLOCKED.to_string(), cx);
            page.press_primary(window, cx);
            assert!(page.resume_field().is_some());
            assert!(page.focus.is_focused(window));
        });
    });
}

#[gpui::test]
fn each_inbox_pane_keeps_its_own_resume_draft(cx: &mut TestAppContext) {
    use crate::ui::engine_desk_tests::make_split;
    use sakura_core::tabs::Page;

    let (rig, desk, cx) =
        crate::ui::lane_tests::open_answering(cx, &resume_note_snapshot(), MARKED, MARKED);
    let (left, right) = make_split(&desk, Page::Inbox, Page::Inbox, 5000, cx);
    wait_until(cx, "the two Inbox pages", |cx| {
        rig.facts
            .read_with(cx, |facts, _| facts.snapshot().is_some())
            && desk.read_with(cx, |desk, _| desk.inbox_for_test(right).is_some())
    });
    let pages = desk.read_with(cx, |desk, _| {
        [left, right].map(|tab| desk.inbox_for_test(tab).unwrap().clone())
    });
    for (page, draft) in pages.iter().zip(["left draft", "right draft"]) {
        cx.update(|window, cx| {
            page.update(cx, |page, cx| {
                page.select(BLOCKED.to_string(), cx);
                page.press_primary(window, cx);
                page.resume_field().unwrap().update(cx, |field, cx| {
                    field.set_value(draft, window, cx);
                });
            });
        });
    }
    for (page, draft) in pages.iter().zip(["left draft", "right draft"]) {
        page.read_with(cx, |page, cx| {
            assert_eq!(page.resume_field().unwrap().read(cx).value(), draft);
        });
    }
}

#[gpui::test]
fn quit_waits_for_latest_tabs_write_after_resize_or_close(cx: &mut TestAppContext) {
    use crate::prefs::Preferences;
    use sakura_core::tabs::{Page, Tabs};

    for is_close in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("prefs.json");
        let mut tabs = Tabs::new();
        let left = tabs.active().id();
        let right = tabs.open_new(Page::Master);
        tabs.create_split(left, right);
        let older = tabs.to_json();
        if is_close {
            tabs.close(right);
        } else {
            tabs.set_split_share(left, right, 5900);
        }
        let latest = tabs.to_json();
        let written = Rc::new(RefCell::new(Vec::new()));
        cx.update(|cx| {
            Preferences::default().install(Some(path.clone()), cx);
            Preferences::change(cx, |prefs| prefs.tabs = Some(older));
            Preferences::change(cx, |prefs| prefs.tabs = Some(latest.clone()));
            for _ in 0..2 {
                let written = written.clone();
                let path = path.clone();
                super::quit::quit_after_answers_then(cx, move |_| {
                    written
                        .borrow_mut()
                        .push(Preferences::load_from(&path).tabs);
                });
            }
            assert!(written.borrow().is_empty());
        });
        cx.run_until_parked();
        assert_eq!(*written.borrow(), [Some(latest)]);
    }
}

#[gpui::test]
fn done_uses_the_recorded_title_and_answer_without_showing_a_resume_note(cx: &mut TestAppContext) {
    let mut snapshot: serde_json::Value = serde_json::from_str(SNAPSHOT).unwrap();
    snapshot["decided"] = serde_json::json!([{
        "id": "blocked-ask", "kind": "blocked", "project": "repo", "task": "stuck",
        "title": "This lane is stuck", "answer": "resume", "text": "Add one contract row.",
        "at": "2026-11-16T02:00:00Z"
    }]);
    let (_rig, page, cx) = open_on(cx, &snapshot.to_string(), MARKED, MARKED);
    page.update(cx, |page, cx| page.show(Shown::Done, cx));
    wait_until(cx, "the recorded answer", |cx| {
        bounds(cx, "inbox-decided").is_some()
    });
    assert!(bounds(cx, "title-blocked-ask@2026-11-16T02:00:00Z").is_some());
    assert!(bounds(cx, "inbox-note-text").is_none());
}

#[gpui::test]
fn polish_s_done_keeps_the_spec_after_an_ask_leaves_the_open_list(cx: &mut TestAppContext) {
    let mut snapshot: serde_json::Value = serde_json::from_str(SNAPSHOT).unwrap();
    snapshot["waits"][0]["title"] = "Approve the revised plan".into();
    let original = "## Original plan\n\nMay the lane change the database schema?";
    snapshot["waits"][0]["detail"] = original.into();
    snapshot["waits"][0]["since"] = "2026-11-15T01:00:00Z".into();
    let (rig, page, cx) = open_on(cx, &snapshot.to_string(), MARKED, MARKED);
    let ask = snapshot["waits"].as_array_mut().unwrap().remove(0);
    snapshot["decided"] = serde_json::json!([{
        "id": ask["id"], "key": ask["key"], "kind": "approval", "project": "repo",
        "task": "gated", "title": ask["title"], "answer": "approve",
        "at": "2026-11-16T02:00:00Z"
    }]);
    rig.say_snapshot(&snapshot.to_string());
    rig.facts.update(cx, |facts, cx| facts.read(cx));
    page.update(cx, |page, cx| page.show(Shown::Done, cx));
    wait_until(cx, "the recorded answer", |cx| {
        bounds(cx, "inbox-decided").is_some()
    });
    let id = format!("{}@2026-11-16T02:00:00Z", ask["id"].as_str().unwrap());
    assert!(bounds(cx, &format!("title-{id}")).is_some());
    assert!(bounds(cx, &format!("body-{id}")).is_some());
    assert_eq!(
        page.read_with(cx, |page, _| page.body_text(&id)).as_deref(),
        Some(original)
    );
    let since = rig.facts.read_with(cx, |facts, _| {
        let decision = &facts.snapshot().unwrap().decided.as_ref().unwrap()[0];
        facts.decided_wait(decision).unwrap().since.clone()
    });
    assert_eq!(since.as_deref(), Some("2026-11-15T01:00:00Z"));
}

#[gpui::test]
fn done_restores_the_original_spec_and_since_after_relaunch(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("preferences.json");
    let mut snapshot: serde_json::Value = serde_json::from_str(SNAPSHOT).unwrap();
    let spec = "## Original spec\n\nKeep one row per lane.";
    let since = "2026-11-15T01:00:00Z";
    snapshot["waits"][0]["detail"] = spec.into();
    snapshot["waits"][0]["since"] = since.into();
    cx.update(|cx| crate::prefs::Preferences::default().install(Some(path.clone()), cx));
    {
        let (rig, _, cx) = open_on(cx, &snapshot.to_string(), MARKED, MARKED);
        let ask = snapshot["waits"].as_array_mut().unwrap().remove(0);
        snapshot["decided"] = serde_json::json!([{
            "id": ask["id"], "key": ask["key"], "kind": "approval", "project": "repo",
            "task": "gated", "title": ask["title"], "answer": "approve",
            "at": "2026-11-16T02:00:00Z"
        }]);
        rig.say_snapshot(&snapshot.to_string());
        rig.facts.update(cx, |facts, cx| facts.read(cx));
        wait_until(cx, "the ask's decision", |cx| {
            rig.facts.read_with(cx, |facts, _| {
                facts
                    .snapshot()
                    .is_some_and(|snapshot| snapshot.decided.is_some())
            })
        });
        cx.run_until_parked();
        cx.update(|window, _| window.remove_window());
    }

    cx.update(|cx| crate::prefs::Preferences::load_from(&path).install(Some(path), cx));
    let (rig, page, cx) = open_on(cx, &snapshot.to_string(), MARKED, MARKED);
    page.update(cx, |page, cx| page.show(Shown::Done, cx));
    wait_until(cx, "Done after relaunch", |cx| {
        bounds(cx, "inbox-decided").is_some()
    });
    let id = model::decided_key(&parse(&snapshot.to_string()).decided.unwrap()[0]);
    assert!(bounds(cx, &format!("body-{id}")).is_some());
    assert_eq!(
        page.read_with(cx, |page, _| page.body_text(&id)).as_deref(),
        Some(spec)
    );
    assert!(bounds(cx, "inbox-property-since").is_some());
    let restored_since = rig.facts.read_with(cx, |facts, _| {
        let decision = &facts.snapshot().unwrap().decided.as_ref().unwrap()[0];
        facts
            .decided_wait(decision)
            .and_then(|wait| wait.since.clone())
    });
    assert_eq!(restored_since.as_deref(), Some(since));
}

#[gpui::test]
fn a_spec_code_identifier_breaks_at_a_separator(cx: &mut TestAppContext) {
    let spec = "## Goal\n\n`{id}`.";
    let prefix = format!("{}_", "filename".repeat(4));
    let identifier = format!("{prefix}{}_ending", "identifier".repeat(4));
    let prefix_width = pytest_identifier_boxes(cx, spec, &prefix)[0].size.width.0;
    let boxes = pytest_identifier_boxes(cx, spec, &identifier);
    assert!(boxes.last().unwrap().top() > boxes[0].top());
    assert!(
        (boxes[0].size.width.0 - prefix_width).abs() <= 1.,
        "the first line ends after {prefix}: {boxes:?}"
    );
}

#[gpui::test]
fn polish_x_track_decisions_have_track_properties_and_answer_words(cx: &mut TestAppContext) {
    let fixture = include_str!("../../../../sakura-core/tests/fixtures/engine-real-tracks.json");
    let snapshot = parse(fixture);
    let wait = snapshot
        .waits
        .iter()
        .find(|wait| wait.track.is_some())
        .unwrap();
    let (_rig, page, cx) = open_on(cx, fixture, MARKED, MARKED);
    select(cx, &wait.id);
    let open_track = bounds(cx, "inbox.property.track").is_some();
    page.update(cx, |page, cx| page.show(Shown::Done, cx));
    wait_until(cx, "the decision answer", |cx| {
        bounds(cx, "inbox-note-text").is_some()
    });
    assert_eq!(
        (
            open_track,
            bounds(cx, "inbox.property.track").is_some(),
            bounds(cx, "inbox.note.track_answer").is_some()
        ),
        (true, true, true)
    );
}

#[test]
fn polish_x_track_history_times_in_the_same_minute_have_the_same_age() {
    let now = sakura_core::graph::parse_time("2026-11-18T11:37:30Z").unwrap();
    let early = model::relative_age(Some("2026-11-18T11:36:01Z"), now);
    let late = model::relative_age(Some("2026-11-18T11:36:59Z"), now);
    assert_eq!(early, late);
}

#[gpui::test]
fn track_answer_draft_leaves_when_the_phone_settles_the_open_wait(cx: &mut TestAppContext) {
    let fixture = include_str!("../../../../sakura-core/tests/fixtures/engine-real-tracks.json");
    let mut document: serde_json::Value = serde_json::from_str(fixture).unwrap();
    let wait = document["waits"]
        .as_array()
        .unwrap()
        .iter()
        .find(|wait| wait["subkind"] == "track_decision")
        .unwrap()
        .clone();
    let id = wait["id"].as_str().unwrap();
    let (rig, page, cx) = open_on(cx, fixture, MARKED, MARKED);
    select(cx, id);
    click(cx, "verb-reply");
    click(cx, "inbox-note-field");
    cx.simulate_input("Keep the draft here");
    assert!(bounds(cx, "note-send").is_some());

    document["waits"]
        .as_array_mut()
        .unwrap()
        .retain(|wait| wait["id"] != id);
    let history = serde_json::json!([{
        "id":id, "key":wait["key"], "kind":"question", "project":wait["project"],
        "task":"", "title":wait["title"], "answer":"reply", "text":"Phone answer",
        "actor":"overlay:fixture:phone", "place":"phone"
    }]);
    document["decided"] = history;
    rig.say_snapshot(&document.to_string());
    rig.facts.update(cx, |facts, cx| facts.read(cx));
    wait_until(cx, "the open decision and its draft are gone", |cx| {
        bounds(cx, &format!("ask-{id}")).is_none() && bounds(cx, "note-send").is_none()
    });
    page.update(cx, |page, cx| page.show(Shown::Done, cx));
    wait_until(cx, "the answer from the phone", |cx| {
        bounds(cx, "inbox-resolved-by").is_some()
    });
    assert!(!rig.acts().iter().any(|call| call.contains("tracksettle")));
}

#[gpui::test]
fn the_inbox_view_has_one_selected_fill_even_when_hovering_done(cx: &mut TestAppContext) {
    let (_rig, desk, cx) = super::super::lane_tests::open_answering(cx, SNAPSHOT, "{}", "{}");
    desk.update(cx, |desk, cx| {
        desk.navigate(sakura_core::tabs::Page::Inbox, false, cx)
    });
    wait_until(cx, "the view switch", |cx| {
        cx.debug_bounds("inbox-view-1").is_some()
    });
    let todo = cx.debug_bounds("inbox-view-0").unwrap();
    let done = cx.debug_bounds("inbox-view-1").unwrap();
    cx.simulate_mouse_move(done.center(), None, Modifiers::none());
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
        let scale = window.scale_factor();
        let palette = crate::theme::Theme::palette(cx);
        let painted = window.painted_quads();
        let face = |bounds: gpui::Bounds<gpui::Pixels>| {
            painted
                .iter()
                .find(|quad| quad.bounds == bounds.scale(scale))
                .map(|quad| quad.background)
        };
        assert_ne!(face(todo), face(done));
        assert_eq!(face(todo), Some(palette.segment_selected.into()));
    });
}

#[gpui::test]
fn reaudit_mark_done_toast_names_the_action_and_item(cx: &mut TestAppContext) {
    let (_, _, cx) = open(cx, MARKED, MARKED);
    select(cx, OWNER_ROW);
    click(cx, "verb-approve");
    let words = cx.update(|window, cx| Toast::text_for_test(window, cx));
    assert_eq!(words.as_deref(), Some("Marked done: Check row R1"));
    assert!(bounds(cx, "toast-undo").is_some());
}

#[test]
fn reaudit3_blocked_item_title_identifies_its_lane() {
    let snapshot = Snapshot::parse(SNAPSHOT.as_bytes()).unwrap();
    let wait = snapshot
        .waits
        .iter()
        .find(|wait| wait.kind == "blocked")
        .unwrap();
    let title = super::detail::ask_title(wait, &snapshot);
    assert_ne!(title, "Blocked");
    assert!(title.contains("stuck"), "{title}");
}

#[test]
fn reaudit3_unnamed_lanes_are_individually_identifiable() {
    let snapshot = Snapshot::parse(SNAPSHOT.as_bytes()).unwrap();
    let titles = snapshot
        .items
        .iter()
        .filter(|item| item.title.is_empty())
        .map(|item| crate::ui::lanes::lane_words(item, false))
        .collect::<Vec<_>>();
    assert!(titles.len() > 1);
    assert_eq!(
        titles
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        titles.len(),
        "{titles:?}"
    );
}

#[gpui::test]
fn reaudit3_stacked_inbox_properties_use_the_reading_width(cx: &mut TestAppContext) {
    let (_, _, window) = crate::ui::engine_desk_tests::open(cx, SNAPSHOT);
    window.simulate_resize(gpui::size(
        crate::theme::WINDOW_MIN_WIDTH,
        crate::theme::WINDOW_MIN_HEIGHT,
    ));
    select(window, QUESTION);
    let properties = bounds(window, "inbox-properties").unwrap();
    let pane = bounds(window, "inbox-detail").unwrap();
    assert!(
        properties.size.width > pane.size.width * 0.6,
        "{properties:?} in {pane:?}"
    );
}

#[gpui::test]
fn reaudit3_proposal_title_uses_the_shared_title_field_height(cx: &mut TestAppContext) {
    let (_, _, window) = open_on(cx, &proposal_with_destination(false), DONE, DONE);
    click(window, "verb-start");
    wait_until(window, "proposal title", |cx| {
        bounds(cx, "proposal-title").is_some()
    });
    let title = bounds(window, "proposal-title").unwrap();
    assert_eq!(title.size.height, crate::theme::CONTROL_HEIGHT);
}

#[gpui::test]
fn polish_zd_inbox_actions_share_one_row_at_minimum_width(cx: &mut TestAppContext) {
    let fixture = at_checks_gate();
    let (_rig, _page, cx) =
        open_with_sidebar(cx, &fixture, MARKED, MARKED, crate::theme::SIDEBAR_WIDTH);
    cx.simulate_resize(gpui::size(gpui::px(1100.), gpui::px(700.)));
    let ask = serde_json::from_str::<serde_json::Value>(&fixture).unwrap()["waits"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    select(cx, &ask);
    let approve = bounds(cx, "verb-approve").unwrap();
    let open = bounds(cx, "open-lane").unwrap();
    assert_eq!(approve.center().y, open.center().y);
}

#[gpui::test]
fn polish_zd_track_answer_has_a_filled_button_face(cx: &mut TestAppContext) {
    let fixture = include_str!("../../../../sakura-core/tests/fixtures/engine-real-tracks.json");
    let snapshot = parse(fixture);
    let decision = snapshot
        .waits
        .iter()
        .find(|wait| wait.original_kind() == "track_decision")
        .unwrap();
    let (_, _, cx) = open_on(cx, fixture, MARKED, MARKED);
    select(cx, &decision.id);
    let button = bounds(cx, "verb-reply").unwrap();
    cx.update(|window, cx| {
        let palette = crate::theme::Theme::palette(cx);
        assert!(window.painted_quads().iter().any(|quad| {
            quad.bounds == button.scale(window.scale_factor())
                && quad.background == palette.primary.into()
        }));
    });
}
