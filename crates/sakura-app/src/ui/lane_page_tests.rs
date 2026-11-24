//! The page of a lane an outside agent opened: its title and "From" line, its links, what it has not
//! read yet, the composer that writes to it and Close in its menu, which asks the engine first.

use super::engine_rig_tests::{self, wait_until, SNAPSHOT};
use super::lane_tests::{bounds, click, open_answering};
use gpui::{TestAppContext, VisualTestContext};
use sakura_core::engine::snapshot::{Item, Snapshot};

const LANE: &str = r#"["outside","measure-help"]"#;
const CONFIRM: &str = r#"{"result": "confirm", "token": "17.ab", "expires": "2026-11-16T08:02:00Z", "summary": "Close measure-help as dropped: run in the cockpit: close the outside lane measure-help"}"#;
const DONE: &str = r#"{"result": "done", "exit_code": 0, "output": "closed measure-help\n"}"#;

/// The fixture with three of the person's answers the agent has not read yet.
fn unread_snapshot() -> String {
    let mut snapshot: serde_json::Value = serde_json::from_str(SNAPSHOT).expect("the fixture");
    let items = snapshot["items"].as_array_mut().expect("the items");
    let lane = items.iter_mut().find(|item| item["kind"] == "outside");
    lane.expect("the outside lane")["unread"] = 3.into();
    snapshot.to_string()
}

fn open_lane_page<'a>(
    cx: &'a mut TestAppContext,
    snapshot: &str,
) -> (super::engine_rig_tests::Rig, &'a mut VisualTestContext) {
    open_page_of(cx, snapshot, Item::is_outside)
}

/// The page of the first lane of `snapshot` that `is_lane` accepts.
fn open_page_of<'a>(
    cx: &'a mut TestAppContext,
    snapshot: &str,
    is_lane: fn(&Item) -> bool,
) -> (super::engine_rig_tests::Rig, &'a mut VisualTestContext) {
    let (rig, desk, cx) = open_answering(cx, snapshot, CONFIRM, DONE);
    // The lane's key holds its folder, which a test may change: it is read from the snapshot shown.
    let shown = sakura_core::engine::snapshot::Snapshot::parse(snapshot.as_bytes());
    let shown = shown.expect("the snapshot");
    let key = shown.items.iter().find(|item| is_lane(item));
    let key = key.expect("the lane").lane_key();
    desk.update(cx, |desk, cx| desk.open_lane(&key, cx));
    wait_until(cx, "the lane page", |cx| {
        bounds(cx, "lane-page").is_some() && bounds(cx, "lane-header").is_some()
    });
    (rig, cx)
}

#[gpui::test]
fn outside_lane_page_shows_links_unread_and_composer(cx: &mut TestAppContext) {
    let (_rig, cx) = open_lane_page(cx, &unread_snapshot());
    assert!(bounds(cx, "lane-from").is_some(), "who the lane is from");
    assert!(bounds(cx, "lane-phase").is_some());
    assert!(bounds(cx, "lane-header").is_some());
    assert!(bounds(cx, "link-0").is_some());
    assert!(bounds(cx, "link-1").is_none(), "one link, one row");
    assert!(bounds(cx, "lane-unread").is_some());
    assert!(bounds(cx, "lane-composer").is_some(), "the composer");
    assert!(bounds(cx, "lane-more").is_some(), "Close is in the menu");
    assert!(
        bounds(cx, "section-roles").is_none(),
        "an outside lane has no roles"
    );
    click(cx, "link-0");
    assert_eq!(
        cx.opened_url().as_deref(),
        Some("https://example.com/notes")
    );
}

#[gpui::test]
fn outside_lane_links_show_the_real_host_and_hide_what_the_validator_refuses(
    cx: &mut TestAppContext,
) {
    let mut snapshot: serde_json::Value = serde_json::from_str(SNAPSHOT).expect("the fixture");
    let items = snapshot["items"].as_array_mut().expect("the items");
    let lane = items.iter_mut().find(|item| item["kind"] == "outside");
    lane.expect("the outside lane")["links"] = serde_json::json!([
        {"label": "github.com/acme/app", "url": "https://evil.example/x"},
        {"label": "Docs", "url": "https://github.com@evil.example/"},
        {"label": "Run", "url": "javascript:alert(1)"},
        {"label": "Notes", "url": "https://example.com/notes"},
    ]);
    let (_rig, cx) = open_lane_page(cx, &snapshot.to_string());
    assert!(
        bounds(cx, "link-0-host").is_some(),
        "the host beside the label"
    );
    assert!(bounds(cx, "link-1-host").is_some());
    assert!(bounds(cx, "link-2").is_none(), "two of four are refused");
    click(cx, "link-0");
    assert_eq!(cx.opened_url().as_deref(), Some("https://evil.example/x"));
}

#[gpui::test]
fn outside_lane_page_without_unread_says_nothing_of_it(cx: &mut TestAppContext) {
    let (_rig, cx) = open_lane_page(cx, SNAPSHOT);
    assert!(bounds(cx, "lane-unread").is_none());
}

#[gpui::test]
fn outside_lane_close_shows_summary_then_runs_with_token(cx: &mut TestAppContext) {
    let (rig, desk, cx) = open_answering(cx, SNAPSHOT, CONFIRM, DONE);
    let key = engine_rig_tests::lane_key("measure-help");
    desk.update(cx, |desk, cx| desk.open_lane(&key, cx));
    wait_until(cx, "the lane page", |cx| bounds(cx, "lane-more").is_some());
    let page = desk.read_with(cx, |desk, cx| {
        desk.lane_for_test(&key, cx).expect("the page").clone()
    });
    page.update(cx, |page, cx| page.press_lane_action("d", cx));
    wait_until(cx, "the call", |_| rig.acts().len() == 1);
    assert_eq!(
        rig.acts()[0],
        format!("cockpit act d {LANE} --source app --json")
    );
    let summary = format!("summary-{LANE}#d");
    wait_until(cx, "the summary", |cx| bounds(cx, &summary).is_some());
    assert_eq!(rig.acts().len(), 1, "nothing ran yet");
    click(cx, &format!("confirm-{LANE}#d"));
    wait_until(cx, "the second call", |_| rig.acts().len() == 2);
    assert!(
        rig.acts()[1].contains("--confirm 17.ab"),
        "{}",
        rig.acts()[1]
    );
}

#[gpui::test]
fn composer_sends_the_typed_text_to_the_lane_and_shows_the_answer(cx: &mut TestAppContext) {
    let sent = r#"{"result": "done", "exit_code": 0, "output": "sent to measure-help\n"}"#;
    let (rig, desk, cx) = open_answering(cx, &unread_snapshot(), sent, sent);
    let key = engine_rig_tests::lane_key("measure-help");
    desk.update(cx, |desk, cx| desk.open_lane(&key, cx));
    wait_until(cx, "the composer", |cx| bounds(cx, "lane-send").is_some());
    let field = desk.read_with(cx, |desk, cx| {
        let page = desk.lane_for_test(&key, cx).expect("the page");
        page.read(cx).composer_field(cx).expect("the field")
    });
    cx.update(|window, cx| {
        field.update(cx, |field, cx| field.set_value("Use argparse", window, cx))
    });
    wait_until(cx, "the text in the box", |cx| {
        cx.run_until_parked();
        bounds(cx, "lane-send").is_some()
    });
    click(cx, "lane-send");
    wait_until(cx, "the call", |_| rig.acts().len() == 1);
    assert_eq!(
        rig.acts()[0],
        format!("cockpit act w {LANE} --text Use argparse --source app --json")
    );
    wait_until(cx, "the answer", |cx| {
        bounds(cx, &format!("told-{LANE}#w")).is_some()
    });
}

#[gpui::test]
fn opening_outside_lane_page_focuses_no_field(cx: &mut TestAppContext) {
    let (_rig, cx) = open_lane_page(cx, SNAPSHOT);
    assert!(
        bounds(cx, "lane-composer").is_some(),
        "the composer is drawn"
    );
    let focused = cx.update(|window, cx| window.focused(cx).is_some());
    assert!(!focused, "nothing holds the keyboard until the person acts");
}

#[test]
fn the_recently_column_says_sentences_not_the_raw_ledger_lines() {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let raw = r#"[{"source":"flow","scope":"repo/demo","at":null,"text":"[operator] start requested actor=app:me"},
        {"source":"flow","scope":"repo/demo","at":null,"text":"[intake] claude program /home/someone/.local/claude/2.1 (found in PATH)"},
        {"source":"flow","scope":"repo/other","at":null,"text":"[operator] stop requested actor=cli:me"}]"#;
    let mut document: serde_json::Value = serde_json::from_str(SNAPSHOT).expect("the fixture");
    document["events"] = serde_json::from_str(raw).expect("events");
    let snapshot = sakura_core::engine::snapshot::Snapshot::parse(document.to_string().as_bytes())
        .expect("a snapshot");
    let demo = snapshot
        .items
        .iter()
        .find(|item| item.task() == "demo")
        .expect("the lane");
    let said: Vec<String> = super::lane_page::events_of(demo, Some(&snapshot))
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(
        said,
        ["Started by you in the app", "Intake: the agent was found"]
    );
}

fn fixture_lane(task: &str, agent_state: &str) -> sakura_core::engine::snapshot::Item {
    let mut snapshot: serde_json::Value = serde_json::from_str(SNAPSHOT).expect("the fixture");
    let items = snapshot["items"].as_array_mut().expect("the items");
    let item = items.iter_mut().find(|item| item["key"][2] == task);
    item.expect("the lane")["agent_state"] = agent_state.into();
    let read = sakura_core::engine::snapshot::Snapshot::parse(snapshot.to_string().as_bytes());
    let items = read.expect("a snapshot").items;
    items
        .into_iter()
        .find(|item| item.task() == task)
        .expect("the lane")
}

#[test]
fn the_state_sentence_says_what_needs_the_person_the_step_that_runs_and_how_it_ended() {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let snapshot =
        sakura_core::engine::snapshot::Snapshot::parse(SNAPSHOT.as_bytes()).expect("a snapshot");
    let say = |task: &str, state: &str| {
        super::lane_page::state_sentence(&fixture_lane(task, state), Some(&snapshot)).to_string()
    };
    assert_eq!(say("gated", "gate"), "Needs you: read and approve its spec");
    assert_eq!(say("demo", "working"), "Running: Build");
    assert_eq!(say("demo", "delivered"), "Finished");
    assert_eq!(say("demo", "cleaned"), "Closed");
    assert_eq!(say("demo", "stopped"), "Stopped");
    assert_eq!(say("fresh", "gate"), "Needs you");
}

#[test]
fn a_lane_at_a_gate_with_no_ask_listed_yet_still_says_what_it_waits_for() {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let mut lane = fixture_lane("gated", "gate");
    let said = |lane: &sakura_core::engine::snapshot::Item| {
        super::lane_page::state_sentence(lane, None).to_string()
    };
    assert_eq!(said(&lane), "Needs you: read and approve its spec");
    lane.next_command = "sakura flow approve gated checks --continue".into();
    assert_eq!(said(&lane), "Needs you: approve the commands of its checks");
    lane.next_command = "sakura flow resume gated".into();
    assert_eq!(said(&lane), "Needs you");
}

#[test]
fn a_lane_the_engine_gives_no_steps_says_the_phase_it_is_in() {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let mut lane = fixture_lane("demo", "working");
    lane.steps.clear();
    assert_eq!(
        super::lane_page::state_sentence(&lane, None),
        "Running: Build"
    );
}

const GATED: &str = r#"["flow","/work/repo","gated"]"#;
const GATED_WAIT: &str = r#"["wait","/work/repo","gated","approval","spec"]"#;

#[gpui::test]
fn an_open_ask_of_the_lane_stands_over_its_agents_with_the_engines_verbs(cx: &mut TestAppContext) {
    let (rig, desk, cx) = open_answering(cx, SNAPSHOT, CONFIRM, DONE);
    desk.update(cx, |desk, cx| desk.open_lane(GATED, cx));
    let ask = format!("lane-ask-{GATED_WAIT}");
    wait_until(cx, "the ask", |cx| bounds(cx, &ask).is_some());
    wait_until(cx, "the agents", |cx| bounds(cx, "lane-agents").is_some());
    let (card, agents) = (
        bounds(cx, &ask).expect("ask"),
        bounds(cx, "lane-agents").expect("the agents"),
    );
    assert!(card.bottom() <= agents.top(), "the ask is over the agents");
    click(cx, &format!("ask-{GATED_WAIT}-0"));
    wait_until(cx, "the call", |_| rig.acts().len() == 1);
    assert_eq!(
        rig.acts()[0],
        format!("cockpit act a {GATED_WAIT} --source app --json")
    );
}

/// The fixture whose blocked ask says `detail` and, when given, `technical`.
fn blocked_snapshot(detail: &str, technical: Option<&str>) -> String {
    let mut snapshot: serde_json::Value = serde_json::from_str(SNAPSHOT).expect("the fixture");
    let waits = snapshot["waits"].as_array_mut().expect("the waits");
    let blocked = waits.iter_mut().find(|wait| wait["kind"] == "blocked");
    let blocked = blocked.expect("a blocked ask");
    blocked["detail"] = detail.into();
    if let Some(technical) = technical {
        blocked["technical"] = technical.into();
    }
    snapshot.to_string()
}

#[test]
fn a_stuck_ask_gives_the_card_the_reason_the_inbox_gives_and_no_other_ask_gives_one() {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let reason =
        "the code reviewer did not approve the contract: the parser must skip fenced code blocks";
    let reason_of = |detail: &str, technical: Option<&str>| {
        let snapshot = Snapshot::parse(blocked_snapshot(detail, technical).as_bytes());
        let snapshot = snapshot.expect("a snapshot");
        let blocked = snapshot.waits.iter().find(|wait| wait.kind == "blocked");
        super::lane_page::ask_reason(blocked.expect("a blocked ask")).map(|(words, _)| words)
    };
    assert_eq!(
        reason_of("", Some(reason)).as_deref(),
        Some(reason),
        "the technical line, when it is plain"
    );
    assert_eq!(
        reason_of(
            "The reviewer requested changes twice.",
            Some("exit 1: /usr/bin/x --y")
        )
        .as_deref(),
        Some("The reviewer requested changes twice."),
        "the engine's own sentence wins"
    );
    assert_eq!(
        reason_of("", Some("exit 2: /usr/local/bin/tool --flag")).as_deref(),
        Some("Something went wrong and the engine gave no plain words for it."),
        "the Inbox's fallback"
    );
    let snapshot = Snapshot::parse(SNAPSHOT.as_bytes()).expect("a snapshot");
    let spec = snapshot.waits.iter().find(|wait| wait.kind == "approval");
    assert_eq!(
        super::lane_page::ask_reason(spec.expect("an approval")),
        None
    );
}

#[gpui::test]
fn the_stuck_card_of_the_lane_shows_the_reason_under_its_title(cx: &mut TestAppContext) {
    let reason =
        "the code reviewer did not approve the contract: the parser must skip fenced code blocks";
    let snapshot = blocked_snapshot("", Some(reason));
    let (_rig, cx) = open_page_of(cx, &snapshot, |item| item.agent_state == "blocked");
    wait_until(cx, "the reason", |cx| {
        cx.debug_bounds("lane-ask-reason").is_some()
    });
}

#[gpui::test]
fn a_blocked_lane_reveals_a_diagnostic_path_only_after_a_press(cx: &mut TestAppContext) {
    let snapshot = blocked_snapshot("pull request lookup failed: see /tmp/test/lookup.log", None);
    let parsed = Snapshot::parse(snapshot.as_bytes()).expect("a snapshot");
    let blocked = parsed.waits.iter().find(|wait| wait.kind == "blocked");
    let id = &blocked.expect("a blocked ask").id;
    let (trigger, details) = (
        format!("lane-ask-details-{id}"),
        format!("lane-ask-details-text-{id}"),
    );
    let (_rig, cx) = open_page_of(cx, &snapshot, |item| item.agent_state == "blocked");
    wait_until(cx, "the details button", |cx| {
        bounds(cx, &trigger).is_some()
    });
    assert!(bounds(cx, &details).is_none());
    click(cx, &trigger);
    wait_until(cx, "the diagnostic", |cx| bounds(cx, &details).is_some());
}

#[gpui::test]
fn the_stuck_card_shows_the_findings_its_ask_carries(cx: &mut TestAppContext) {
    let mut snapshot: serde_json::Value =
        serde_json::from_str(&blocked_snapshot("The fix builder made no commit.", None))
            .expect("a snapshot");
    let waits = snapshot["waits"].as_array_mut().expect("the waits");
    let blocked = waits.iter_mut().find(|wait| wait["kind"] == "blocked");
    let blocked = blocked.expect("a blocked ask");
    blocked["findings"] = serde_json::json!(["No test covers an empty query."]);
    let id = blocked["id"].as_str().expect("an id").to_string();
    let (_rig, cx) = open_page_of(cx, &snapshot.to_string(), |item| {
        item.agent_state == "blocked"
    });
    wait_until(cx, "the findings", |cx| {
        bounds(cx, &format!("lane-ask-findings-line-{id}-0")).is_some()
    });
}

#[gpui::test]
fn the_card_of_the_ask_to_deliver_although_the_review_wants_changes_shows_the_findings(
    cx: &mut TestAppContext,
) {
    let mut snapshot: serde_json::Value = serde_json::from_str(SNAPSHOT).expect("the fixture");
    let waits = snapshot["waits"].as_array_mut().expect("the waits");
    let ask = waits.iter_mut().find(|wait| wait["kind"] == "approval");
    let ask = ask.expect("an approval");
    ask["findings"] = serde_json::json!(["No test covers an empty query."]);
    let id = ask["id"].as_str().expect("an id").to_string();
    let (_rig, desk, cx) = open_answering(cx, &snapshot.to_string(), CONFIRM, DONE);
    desk.update(cx, |desk, cx| desk.open_lane(GATED, cx));
    wait_until(cx, "the findings", |cx| {
        bounds(cx, &format!("lane-ask-findings-line-{id}-0")).is_some()
    });
}

#[gpui::test]
fn the_ask_cards_approve_is_held_under_undo_and_sent_only_after_it(cx: &mut TestAppContext) {
    let (rig, desk, cx) = open_answering(cx, SNAPSHOT, CONFIRM, DONE);
    desk.update(cx, |desk, cx| desk.open_lane(GATED, cx));
    let first = format!("ask-{GATED_WAIT}-0");
    wait_until(cx, "the ask", |cx| bounds(cx, &first).is_some());
    click(cx, &first);
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        bounds(cx, "toast-undo").is_some(),
        "the answer is held under the Undo toast, as in the Inbox"
    );
    assert!(
        bounds(cx, &first).is_none(),
        "the card offers no other answer while one is held"
    );
    let confirmed =
        |rig: &super::engine_rig_tests::Rig| rig.acts().iter().any(|act| act.contains("--confirm"));
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(5));
    cx.run_until_parked();
    assert!(!confirmed(&rig), "held: {:?}", rig.acts());
    wait_until(cx, "the confirmed call", |cx| {
        cx.executor()
            .advance_clock(std::time::Duration::from_secs(7));
        cx.run_until_parked();
        confirmed(&rig)
    });
}

#[gpui::test]
fn undo_takes_the_ask_cards_answer_back_and_the_card_offers_it_again(cx: &mut TestAppContext) {
    let (rig, desk, cx) = open_answering(cx, SNAPSHOT, CONFIRM, DONE);
    desk.update(cx, |desk, cx| desk.open_lane(GATED, cx));
    let first = format!("ask-{GATED_WAIT}-0");
    wait_until(cx, "the ask", |cx| bounds(cx, &first).is_some());
    click(cx, &first);
    cx.update(|window, cx| window.draw(cx).clear(cx));
    wait_until(cx, "the card offers the answer again", |cx| {
        if bounds(cx, "toast-undo").is_some() {
            click(cx, "toast-undo");
        }
        bounds(cx, &first).is_some()
    });
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(7));
    cx.run_until_parked();
    assert!(
        !rig.acts().iter().any(|act| act.contains("--confirm")),
        "only the call that asks may have gone: {:?}",
        rig.acts()
    );
}

const QUESTION: &str = r#"["wait","/work/repo","gated","question","flag-fate"]"#;

/// The fixture with a question of the flow lane `gated` that offers two choices.
fn question_snapshot() -> String {
    let mut snapshot: serde_json::Value = serde_json::from_str(SNAPSHOT).expect("the fixture");
    let reply = serde_json::json!({"key": "r", "label": "Reply", "needs": [],
        "optional": ["choice", "text"], "confirm": false});
    snapshot["waits"]
        .as_array_mut()
        .expect("the waits")
        .push(serde_json::json!({
            "id": QUESTION, "key": ["wait", "/work/repo", "gated", "question", "flag-fate"],
            "kind": "question", "repo": "/work/repo", "project": "repo", "task": "gated",
            "title": "Keep the old flag?", "detail": "", "since": null,
            "choices": ["Keep it for one release", "Remove it now"], "actions": [reply]
        }));
    snapshot.to_string()
}

/// Presses the second choice of the question on the page of `gated` and waits for the Undo toast.
fn choose_on_the_lane_page(
    cx: &mut TestAppContext,
) -> (super::engine_rig_tests::Rig, &mut VisualTestContext) {
    let (rig, desk, cx) = open_answering(cx, &question_snapshot(), CONFIRM, DONE);
    desk.update(cx, |desk, cx| desk.open_lane(GATED, cx));
    let choice = format!("ask-{QUESTION}-choice-1");
    wait_until(cx, "the choice", |cx| bounds(cx, &choice).is_some());
    click(cx, &choice);
    cx.update(|window, cx| window.draw(cx).clear(cx));
    (rig, cx)
}

#[gpui::test]
fn a_choice_on_the_lane_card_is_held_under_undo_and_undo_sends_nothing(cx: &mut TestAppContext) {
    let (rig, cx) = choose_on_the_lane_page(cx);
    assert!(bounds(cx, "toast-undo").is_some(), "the choice is held");
    assert!(rig.acts().is_empty(), "nothing sent yet: {:?}", rig.acts());
    click(cx, "toast-undo");
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(7));
    cx.run_until_parked();
    assert!(rig.acts().is_empty(), "undone: {:?}", rig.acts());
}

#[gpui::test]
fn a_choice_on_the_lane_card_sends_exactly_one_reply_with_it_after_the_hold(
    cx: &mut TestAppContext,
) {
    let (rig, cx) = choose_on_the_lane_page(cx);
    assert!(rig.acts().is_empty(), "held first: {:?}", rig.acts());
    wait_until(cx, "the reply", |cx| {
        cx.executor()
            .advance_clock(std::time::Duration::from_secs(7));
        cx.run_until_parked();
        !rig.acts().is_empty()
    });
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(7));
    cx.run_until_parked();
    let sent = rig.acts();
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert!(
        sent[0].starts_with(&format!("cockpit act r {QUESTION}"))
            && sent[0].contains("--choice Remove it now"),
        "{}",
        sent[0]
    );
}

/// Types `text` into the box of the outside lane's page and presses the round button.
fn write(
    desk: &gpui::Entity<super::engine_desk::Desk>,
    key: &str,
    text: &str,
    cx: &mut VisualTestContext,
) -> gpui::Entity<gpui_component::input::TextareaState> {
    desk.update(cx, |desk, cx| desk.open_lane(key, cx));
    wait_until(cx, "the composer", |cx| bounds(cx, "lane-send").is_some());
    let field = desk.read_with(cx, |desk, cx| {
        let page = desk.lane_for_test(key, cx).expect("the page");
        page.read(cx).composer_field(cx).expect("the field")
    });
    cx.update(|window, cx| field.update(cx, |field, cx| field.set_value(text, window, cx)));
    cx.run_until_parked();
    click(cx, "lane-send");
    field
}

#[gpui::test]
fn the_box_empties_once_the_engine_took_the_message(cx: &mut TestAppContext) {
    let sent = r#"{"result": "done", "exit_code": 0, "output": "sent to measure-help\n"}"#;
    let (_rig, desk, cx) = open_answering(cx, &unread_snapshot(), sent, sent);
    let key = engine_rig_tests::lane_key("measure-help");
    let field = write(&desk, &key, "Use argparse", cx);
    wait_until(cx, "the box emptied", |cx| {
        cx.update(|window, _| window.refresh());
        field.read_with(cx, |field, _| field.value().is_empty())
    });
}

#[gpui::test]
fn the_box_keeps_the_message_when_the_engine_refused_it(cx: &mut TestAppContext) {
    let refused = r#"{"result": "refused", "reason": "the lane is closed"}"#;
    let (_rig, desk, cx) = open_answering(cx, &unread_snapshot(), refused, refused);
    let key = engine_rig_tests::lane_key("measure-help");
    let field = write(&desk, &key, "Use argparse", cx);
    wait_until(cx, "the refusal", |cx| {
        bounds(cx, &format!("told-{LANE}#w")).is_some()
    });
    assert_eq!(
        field.read_with(cx, |field, _| field.value().to_string()),
        "Use argparse"
    );
}

/// What the real engine prints with an outside lane and a day's morning digest: two news, a check and a
/// decision.
const DIGEST_SNAPSHOT: &str =
    include_str!("../../../sakura-core/tests/fixtures/engine-real-snapshot-outside.json");

fn open_digest_page<'a>(
    cx: &'a mut TestAppContext,
    snapshot: &str,
) -> (super::engine_rig_tests::Rig, &'a mut VisualTestContext) {
    open_page_of(cx, snapshot, Item::is_digest)
}

/// The real digest with its decision answered: it is no longer open and has no wait.
fn digest_with_a_decided_ask() -> String {
    let mut snapshot: serde_json::Value =
        serde_json::from_str(DIGEST_SNAPSHOT).expect("the fixture");
    let digest = snapshot["items"]
        .as_array_mut()
        .expect("the items")
        .iter_mut()
        .find(|item| item["kind"] == "digest")
        .expect("the digest");
    let ask = &mut digest["reading"]["asks"][1];
    ask["state"] = "decided".into();
    ask["id"] = serde_json::Value::Null;
    ask["answer"] = "yes".into();
    snapshot.to_string()
}

#[gpui::test]
fn the_digest_page_reads_its_news_with_their_sources_and_links_and_its_asks_and_no_composer(
    cx: &mut TestAppContext,
) {
    let (_rig, cx) = open_digest_page(cx, DIGEST_SNAPSHOT);
    assert!(bounds(cx, "digest-news-0").is_some(), "its news, listed");
    assert!(bounds(cx, "digest-news-1").is_some());
    assert!(bounds(cx, "digest-news-0-source").is_some(), "the source");
    assert!(
        bounds(cx, "digest-news-0-link-0-host").is_some(),
        "a link of the news, with its host"
    );
    assert!(
        bounds(cx, "digest-news-1-link-0").is_none(),
        "no link, none drawn"
    );
    assert!(bounds(cx, "digest-ask-0").is_some(), "its asks, listed");
    assert!(bounds(cx, "digest-ask-1").is_some());
    assert!(bounds(cx, "digest-date").is_some(), "its day");
    assert!(bounds(cx, "lane-composer").is_none(), "nothing listens");
    assert!(bounds(cx, "lane-from").is_none(), "no From line");
    assert!(
        bounds(cx, "crumb-project").is_none(),
        "a digest is no project"
    );
    assert_eq!(super::engine_rig_tests::accent_quads(cx), 0);
}

#[gpui::test]
fn a_decided_ask_of_the_digest_stays_listed_with_its_answer_and_opens_nothing(
    cx: &mut TestAppContext,
) {
    let (_rig, cx) = open_digest_page(cx, &digest_with_a_decided_ask());
    assert!(bounds(cx, "digest-ask-1").is_some(), "still listed");
    click(cx, "digest-ask-1");
    cx.run_until_parked();
    assert!(
        bounds(cx, "digest-ask-1").is_some(),
        "a decided ask opens no Inbox"
    );
}

#[gpui::test]
fn the_digest_header_is_one_line_with_its_mark_and_how_many_asks_are_left_and_no_second_title(
    cx: &mut TestAppContext,
) {
    let (_rig, cx) = open_digest_page(cx, DIGEST_SNAPSHOT);
    let mark = bounds(cx, "lane-header-glyph").expect("the mark");
    let state = bounds(cx, "lane-state").expect("what is left");
    assert!(state.left() >= mark.right(), "after the mark");
    assert!(
        (state.center().y - mark.center().y).abs() < gpui::px(2.),
        "on its line"
    );
    assert!(
        bounds(cx, "lane-title").is_none(),
        "the breadcrumb says the name"
    );
}

#[gpui::test]
fn the_digest_news_and_the_header_start_on_one_line(cx: &mut TestAppContext) {
    let (_rig, cx) = open_digest_page(cx, DIGEST_SNAPSHOT);
    let header = bounds(cx, "lane-header-glyph").expect("the mark");
    let news = bounds(cx, "digest-news-0").expect("a news entry");
    let row = bounds(cx, "digest-ask-0").expect("an ask");
    assert_eq!(header.left(), news.left(), "one inset for the page");
    assert_eq!(header.left(), row.left());
}

/// The first wait of the fixture with its kind and actions replaced.
fn wait_of(kind: &str, actions: serde_json::Value) -> sakura_core::engine::snapshot::Wait {
    let mut snapshot: serde_json::Value = serde_json::from_str(SNAPSHOT).expect("the fixture");
    let wait = &mut snapshot["waits"][0];
    wait["kind"] = kind.into();
    wait["actions"] = actions;
    let parsed = sakura_core::engine::snapshot::Snapshot::parse(snapshot.to_string().as_bytes());
    parsed.expect("the snapshot").waits.remove(0)
}

#[test]
fn the_lane_card_names_a_verdict_on_a_check_as_the_inbox_does() {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let mark = serde_json::json!([{"key": "k", "label": "Mark", "needs": ["verdict"], "optional": ["note"], "confirm": false}]);
    let wait = wait_of("owner_row", mark);
    let in_the_inbox: Vec<String> = {
        let verbs = super::inbox::model::verbs(&wait, None);
        verbs
            .primary
            .iter()
            .chain(&verbs.secondary)
            .map(|verb| verb.label().to_string())
            .collect()
    };
    assert_eq!(in_the_inbox, ["Mark done", "Not now"]);
    assert_eq!(
        super::lane_page::ask_button_words(&wait, None),
        in_the_inbox,
        "one name per action"
    );
    let approval = serde_json::json!([{"key": "a", "label": "Approve", "needs": [], "optional": [], "confirm": true}]);
    let wait = wait_of("approval", approval);
    assert_eq!(super::lane_page::ask_button_words(&wait, None), ["Approve"]);
}

#[gpui::test]
fn an_error_the_engine_gives_for_an_answer_never_shows_a_command_or_a_path_but_behind_details(
    cx: &mut TestAppContext,
) {
    let refused = r#"{"result": "refused", "reason": "close failed: run gh auth login; see /home/someone/engine/close.log"}"#;
    let (_rig, desk, cx) = open_answering(cx, SNAPSHOT, refused, refused);
    let key = engine_rig_tests::lane_key("measure-help");
    desk.update(cx, |desk, cx| desk.open_lane(&key, cx));
    wait_until(cx, "the lane page", |cx| bounds(cx, "lane-more").is_some());
    let page = desk.read_with(cx, |desk, cx| {
        desk.lane_for_test(&key, cx).expect("the page").clone()
    });
    page.update(cx, |page, cx| page.press_lane_action("d", cx));
    let told = format!("told-{LANE}#d");
    wait_until(cx, "the words", |cx| bounds(cx, &told).is_some());
    let details = format!("told-details-{LANE}#d");
    assert!(
        bounds(cx, &details).is_some(),
        "the raw text is behind Details"
    );
    assert!(bounds(cx, &format!("told-details-text-{LANE}#d")).is_none());
    click(cx, &details);
    wait_until(cx, "the raw text", |cx| {
        bounds(cx, &format!("told-details-text-{LANE}#d")).is_some()
    });
}

/// The lane `demo` of the fixture with its steps as `pipeline` says (id and state), and the older list of its
/// steps (build "now") left as the engine's other field has it.
fn demo_with_pipeline(states: &[(&str, &str)]) -> Item {
    demo_with(states, None)
}

/// The same with the engine's `current_step` (its commit e9b22d6) as given: its id, whether it waits, its note.
fn demo_with(states: &[(&str, &str)], current: Option<(&str, bool, &str)>) -> Item {
    let mut snapshot: serde_json::Value = serde_json::from_str(SNAPSHOT).expect("the fixture");
    let items = snapshot["items"].as_array_mut().expect("the items");
    let demo = items
        .iter_mut()
        .find(|item| item["key"][2] == "demo")
        .expect("demo");
    for step in demo["pipeline"].as_array_mut().expect("a pipeline") {
        let id = step["id"].as_str().unwrap_or_default().to_string();
        if let Some((_, state)) = states.iter().find(|(wanted, _)| *wanted == id) {
            step["state"] = (*state).into();
            step["on"] = (*state != "off").into();
        }
    }
    demo["current_step"] = match current {
        Some((id, waiting, note)) => {
            serde_json::json!({"id": id, "name": id, "waiting": waiting, "note": note})
        }
        None => serde_json::Value::Null,
    };
    let parsed = Snapshot::parse(snapshot.to_string().as_bytes()).expect("a snapshot");
    parsed
        .items
        .into_iter()
        .find(|item| item.task() == "demo")
        .expect("demo")
}

#[test]
fn the_header_says_the_step_the_step_bar_is_on_not_the_one_the_older_list_still_calls_now() {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let item = demo_with_pipeline(&[
        ("build", "done"),
        ("task_review", "done"),
        ("final_review", "running"),
    ]);
    assert_eq!(
        super::lane_page::running_words(&item),
        "Running: Final review"
    );
}

#[test]
fn during_the_final_review_the_header_names_the_step_the_engine_says_the_lane_is_in_though_build_still_runs(
) {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let both = [("build", "running"), ("final_review", "running")];
    let item = demo_with(&both, Some(("final_review", false, "")));
    assert_eq!(
        super::lane_page::running_words(&item),
        "Running: Final review"
    );
    // From an engine that sends no current step: the last step that runs.
    let older = demo_with(&both, None);
    assert_eq!(
        super::lane_page::running_words(&older),
        "Running: Final review"
    );
}

#[gpui::test]
fn a_snoozed_gate_wait_disappears_from_its_lane_until_it_wakes(cx: &mut TestAppContext) {
    let mut document: serde_json::Value = serde_json::from_str(SNAPSHOT).unwrap();
    let wait = document["waits"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|wait| wait["task"] == "gated")
        .unwrap();
    wait["is_snoozed"] = true.into();
    wait["snoozed_until"] = "2099-11-22T18:00:00Z".into();
    let sleeping = document.to_string();
    let snapshot = Snapshot::parse(sleeping.as_bytes()).unwrap();
    let item = snapshot
        .items
        .iter()
        .find(|item| item.task() == "gated")
        .unwrap();
    let (rig, cx) = open_page_of(cx, &sleeping, |item| item.task() == "gated");
    assert!(bounds(cx, "lane-asks").is_none());
    assert!(!super::lane_page::state_sentence(item, Some(&snapshot)).contains("Needs you"));
    rig.say_snapshot(SNAPSHOT);
    rig.facts.update(cx, |facts, cx| facts.read(cx));
    wait_until(cx, "the ask wakes on its lane", |cx| {
        bounds(cx, "lane-asks").is_some()
    });
}

#[gpui::test]
fn the_digest_header_excludes_snoozed_asks(cx: &mut TestAppContext) {
    let (rig, cx) = open_digest_page(cx, DIGEST_SNAPSHOT);
    assert!(bounds(cx, "lane-state").is_some());
    let mut document: serde_json::Value = serde_json::from_str(DIGEST_SNAPSHOT).unwrap();
    for wait in document["waits"].as_array_mut().unwrap() {
        if wait["digest"].is_string() {
            wait["is_snoozed"] = true.into();
            wait["snoozed_until"] = "2099-11-22T18:00:00Z".into();
        }
    }
    rig.say_snapshot(&document.to_string());
    rig.facts.update(cx, |facts, cx| facts.read(cx));
    wait_until(cx, "the digest count disappears", |cx| {
        bounds(cx, "lane-state").is_none()
    });
}

#[gpui::test]
fn fix36_the_stuck_card_can_resume_without_a_note(cx: &mut TestAppContext) {
    let mut snapshot: serde_json::Value = serde_json::from_str(SNAPSHOT).unwrap();
    let blocked = snapshot["waits"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|wait| wait["kind"] == "blocked")
        .unwrap();
    blocked["wants_note"] = true.into();
    let id = blocked["id"].as_str().unwrap().to_string();
    let lane = snapshot["items"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|item| item["agent_state"] == "blocked")
        .unwrap();
    let resume = lane["actions"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|action| action["key"] == "R")
        .unwrap();
    resume["optional"] = serde_json::json!(["note"]);
    let lane_id = lane["id"].as_str().unwrap().to_string();
    let (rig, cx) = open_page_of(cx, &snapshot.to_string(), |item| {
        item.agent_state == "blocked"
    });
    click(cx, &format!("ask-{id}-0"));
    let note = bounds(cx, "inbox-note").expect("the note is on the Stuck card");
    assert!(note.bottom() <= bounds(cx, "lane-ask-reason").unwrap().top());
    cx.simulate_input("A draft to leave out");
    click(cx, "resume-without-note");
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        bounds(cx, "toast-undo").is_some(),
        "Resume is held for Undo"
    );
    cx.simulate_mouse_move(gpui::Point::default(), None, gpui::Modifiers::none());
    wait_until(cx, "the confirmed Resume without a note", |cx| {
        cx.executor()
            .advance_clock(std::time::Duration::from_secs(7));
        cx.run_until_parked();
        rig.acts().iter().any(|act| act.contains("--confirm"))
    });
    let ask = format!("cockpit act R {lane_id} --source app --json");
    let confirm = format!("cockpit act R {lane_id} --confirm 17.ab --source app --json");
    assert_eq!(rig.acts(), [ask.clone(), ask, confirm]);
}

#[gpui::test]
fn polish_s_short_step_labels_wrap_in_a_narrow_track(cx: &mut gpui::TestAppContext) {
    use gpui::{
        div, px, AnyElement, Context, IntoElement, ParentElement as _, Render, Styled as _, Window,
    };
    struct Track;
    impl Render for Track {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let segment = super::step_track::TrackSegment {
                id: "plan_review".into(),
                label: "Plan review".into(),
                state: super::step_track::SegmentState::Next,
            };
            let body: AnyElement = super::step_track::step_track_with(
                &[(segment, 1.)],
                &crate::theme::Palette::dark(),
            );
            div().w(px(40.)).child(body)
        }
    }
    let (_, cx) = cx.add_window_view(|_, _| Track);
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let label = super::lane_tests::bounds(cx, "step-label-plan_review").unwrap();
    assert!(
        label.size.height > crate::theme::lane_page::TRACK_LABEL_LINE_HEIGHT,
        "{label:?}"
    );
}
