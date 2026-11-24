//! The Master tab driven on GPUI's test platform against a fake engine (`master spec` and `master decide`
//! answered from files) and the fake agent of `sakura-acp` as the adapter: what the first message starts, what
//! the engine's three decisions do to the conversation, and the session kept across launches.

use super::engine_desk::{Desk, DeskData};
use super::engine_rig_tests::{self, Rig, SNAPSHOT};
use super::thread::{MenuPick, Phase, ThreadView};
use crate::prefs::Preferences;
use crate::theme::thread::EVENT_FRAME;
use gpui::{AppContext as _, Entity, TestAppContext, VisualTestContext};
use sakura_acp::policy::Reason;
use sakura_acp::thread::EntryKind;
use sakura_core::tabs::Page;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// The fake engine's script: the snapshot, the master's spec, and one decision for every request (the
/// request itself kept in a file of its own).
const ENGINE: &str = r#"d="$(dirname "$0")"
printf '%s\n' "$*" >> "$d/calls.log"
case "$1 $2" in
  "cockpit snapshot") cat "$d/snapshot.json" ;;
  "master spec") cat "$d/spec.json" ;;
  "engine rules") cat "$d/rules.json" ;;
  "master decide") cat > "$d/request-$$.json"; cat "$d/decision.json" ;;
  *) case "$*" in *--confirm*) cat "$d/second.json" ;; *) cat "$d/first.json" ;; esac ;;
esac"#;

/// The fake agent, beside the binaries of this build.
fn fake_agent() -> PathBuf {
    let test_binary = std::env::current_exe().expect("the test binary");
    let build = test_binary.parent().and_then(Path::parent);
    let agent = build.expect("the build's folder").join("sakura-fake-agent");
    assert!(
        agent.exists(),
        "no fake agent at {}: cargo build -p sakura-acp --bin sakura-fake-agent",
        agent.display()
    );
    agent
}

/// What the fake engine says the master is started with: the fake agent as the adapter, logging to `log`.
fn spec(rig: &Rig, argv: Vec<String>) -> String {
    let log = rig.dir.path().join("agent.log");
    let workspace = rig.dir.path().join("master");
    std::fs::create_dir_all(&workspace).expect("the master's workspace");
    serde_json::json!({
        "argv": argv,
        "env": { "SAKURA_FAKE_LOG": log, "SAKURA_FAKE_ENV_NAMES": "HOME,PATH" },
        "cwd": workspace,
        "options": { "settings": { "permissions": {} }, "settingSources": [], "model": "haiku" },
        "mcp_servers": [{ "name": "sakura-master", "command": "/bin/sakura", "args": ["master", "serve", "p"], "env": [] }],
        "start_mode": "default",
        "close_sessions": true,
        "unset_env": ["CLAUDECODE"],
        "env_allow": ["HOME"],
        "system_prompt_append": "You are the master.",
    })
    .to_string()
}

fn agent_argv() -> Vec<String> {
    let agent = fake_agent().display().to_string();
    vec![agent, "--delay-ms".to_string(), "0".to_string()]
}

/// The desk over a fake engine that decides every request of the master with `decision`.
fn open<'a>(
    cx: &'a mut TestAppContext,
    decision: &str,
    argv: Vec<String>,
) -> (Rig, Entity<Desk>, &'a mut VisualTestContext) {
    open_with(cx, decision, argv, None)
}

/// The desk of a person whose tabs were saved before tabs kept conversations of their own: a Master tab holds
/// the conversation kept under no tab, which the sessions the test stores for "an earlier launch" are.
fn open_legacy<'a>(
    cx: &'a mut TestAppContext,
    decision: &str,
    argv: Vec<String>,
    sessions: Option<crate::data::sessions::Sessions>,
) -> (Rig, Entity<Desk>, &'a mut VisualTestContext) {
    let kept = Preferences {
        tabs: Some(serde_json::json!({"version": 3, "active": 0,
            "tabs": [{"page": "inbox"}, {"page": "master"}]})),
        ..Preferences::default()
    };
    open_kept(cx, (SNAPSHOT, "{}", "{}"), decision, argv, (sessions, kept))
}

/// The same, over `sessions` when given (the kept sessions of the rig otherwise).
fn open_with<'a>(
    cx: &'a mut TestAppContext,
    decision: &str,
    argv: Vec<String>,
    sessions: Option<crate::data::sessions::Sessions>,
) -> (Rig, Entity<Desk>, &'a mut VisualTestContext) {
    open_over(cx, (SNAPSHOT, "{}", "{}"), decision, argv, sessions)
}

/// The same over an engine that shows `snapshot` and answers an action with `first`, and with `second` once
/// a confirmation token is sent.
fn open_over<'a>(
    cx: &'a mut TestAppContext,
    engine: (&str, &str, &str),
    decision: &str,
    argv: Vec<String>,
    sessions: Option<crate::data::sessions::Sessions>,
) -> (Rig, Entity<Desk>, &'a mut VisualTestContext) {
    open_kept(
        cx,
        engine,
        decision,
        argv,
        (sessions, Preferences::default()),
    )
}

/// The same with the preferences the last launch kept.
fn open_kept<'a>(
    cx: &'a mut TestAppContext,
    (snapshot, first, second): (&str, &str, &str),
    decision: &str,
    argv: Vec<String>,
    (sessions, kept_preferences): (Option<crate::data::sessions::Sessions>, Preferences),
) -> (Rig, Entity<Desk>, &'a mut VisualTestContext) {
    let rig = master_rig(cx, (snapshot, first, second), decision, argv);
    let (desk, cx) = build_desk(cx, &rig, sessions, kept_preferences);
    settle(cx, "the lanes", |cx| {
        rig.facts
            .read_with(cx, |facts, _| facts.snapshot().is_some())
    });
    (rig, desk, cx)
}

fn master_rig(
    cx: &mut TestAppContext,
    engine: (&str, &str, &str),
    decision: &str,
    argv: Vec<String>,
) -> Rig {
    let rig = engine_rig_tests::open_with_script(cx, engine, ENGINE);
    std::fs::write(rig.dir.path().join("spec.json"), spec(&rig, argv)).expect("the spec");
    let said = format!(r#"{{"decision": "{decision}", "reason": "a reason"}}"#);
    std::fs::write(rig.dir.path().join("decision.json"), said).expect("the decision");
    rig
}

/// The desk over `rig`, its facts not waited for.
fn build_desk<'a>(
    cx: &'a mut TestAppContext,
    rig: &Rig,
    sessions: Option<crate::data::sessions::Sessions>,
    kept_preferences: Preferences,
) -> (Entity<Desk>, &'a mut VisualTestContext) {
    cx.update(|cx| kept_preferences.clone().install(None, cx));
    let sessions = sessions.unwrap_or_else(|| rig.sessions.clone());
    let (facts, source) = (rig.facts.clone(), rig.source.clone());
    let slot = Rc::new(RefCell::new(None));
    let kept = slot.clone();
    let (_, cx) = cx.add_window_view(move |window, cx| {
        let data = DeskData {
            facts,
            source,
            sessions,
        };
        let desk = cx.new(|cx| Desk::new(data, &kept_preferences, window, cx));
        *kept.borrow_mut() = Some(desk.clone());
        Desk::root(desk, window, cx)
    });
    let desk = slot.borrow_mut().take().expect("the desk was built");
    cx.update(|window, _| window.activate_window());
    (desk, cx)
}

/// How long a test waits for the engine's and the agent's processes: long, as the Mac may be busy with other
/// builds, and only a hang ends it.
const PATIENCE: std::time::Duration = std::time::Duration::from_secs(240);

/// Lets the window, the fake engine and the agent's process work until `is_done`: the clock moves a frame at
/// a time, as the conversation reads the agent's events once per frame.
fn settle(
    cx: &mut VisualTestContext,
    what: &str,
    mut is_done: impl FnMut(&mut VisualTestContext) -> bool,
) {
    let limit = std::time::Instant::now() + PATIENCE;
    loop {
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.executor().advance_clock(EVENT_FRAME);
        if is_done(cx) {
            return;
        }
        assert!(std::time::Instant::now() < limit, "waited for {what}");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

fn wait_for_update<T: 'static>(
    cx: &mut VisualTestContext,
    entity: &Entity<T>,
    ready: impl Fn(&T, &gpui::App) -> bool + 'static,
) {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let mut sender = Some(sender);
    let subscription = cx.update(|_, cx| {
        if ready(entity.read(cx), cx) {
            return None;
        }
        Some(cx.observe(entity, move |entity, cx| {
            if ready(entity.read(cx), cx) {
                if let Some(sender) = sender.take() {
                    let _ = sender.send(());
                }
            }
        }))
    });
    if let Some(_subscription) = subscription {
        cx.foreground_executor()
            .block_test(receiver)
            .expect("the entity was updated");
    }
}

/// Shows the Master tab and gives its conversation.
fn show_master(desk: &Entity<Desk>, cx: &mut VisualTestContext) -> Entity<ThreadView> {
    desk.update(cx, |desk, cx| desk.navigate(Page::Master, false, cx));
    cx.run_until_parked();
    desk.read_with(cx, |desk, cx| desk.master_conversation(cx))
}

/// Writes `text` in the conversation's composer and sends it, as the person does.
fn say(view: &Entity<ThreadView>, text: &str, cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.write(text, window, cx);
            view.send(window, cx);
        });
    });
}

/// What the fake agent received so far, one JSON object per message.
fn received(rig: &Rig) -> Vec<serde_json::Value> {
    let text = std::fs::read_to_string(rig.dir.path().join("agent.log")).unwrap_or_default();
    let lines = text.lines();
    lines
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

fn methods(rig: &Rig) -> Vec<String> {
    let all = received(rig);
    let method = |message: &serde_json::Value| message["method"].as_str().map(str::to_string);
    all.iter().filter_map(method).collect()
}

/// How many of the engine's calls were `master decide`.
fn decisions_asked(rig: &Rig) -> usize {
    let calls = std::fs::read_to_string(rig.dir.path().join("calls.log")).unwrap_or_default();
    calls
        .lines()
        .filter(|line| line.starts_with("master decide"))
        .count()
}

fn turn_ended(view: &Entity<ThreadView>, cx: &mut VisualTestContext) -> bool {
    view.read_with(cx, |view, _| {
        view.thread().turn() == sakura_acp::thread::Turn::Idle
            && !view.thread().entries().is_empty()
            && *view.phase() == Phase::Ready
    })
}

#[gpui::test]
fn the_master_page_replaces_the_inbox_in_the_tab_on_screen(cx: &mut TestAppContext) {
    let (_rig, desk, cx) = open(cx, "ask", agent_argv());
    let pages = |desk: &Entity<Desk>, cx: &mut VisualTestContext| {
        desk.read_with(cx, |desk, cx| {
            let tabs = desk.bar.read(cx).tabs();
            (tabs.len(), tabs.active().page().clone())
        })
    };
    assert_eq!(pages(&desk, cx), (1, Page::Inbox));
    show_master(&desk, cx);
    assert_eq!(pages(&desk, cx), (1, Page::Master));
}

#[gpui::test]
fn looking_at_the_master_reads_the_spec_without_starting_the_adapter(cx: &mut TestAppContext) {
    let (rig, desk, cx) = open(cx, "ask", agent_argv());
    let view = show_master(&desk, cx);
    settle(cx, "the model in the spec", |cx| {
        view.read_with(cx, |view, _| view.agent_menu().unwrap().name == "Haiku")
    });
    assert!(received(&rig).is_empty(), "no adapter was started");
}

#[gpui::test]
fn first_message_starts_the_master_with_the_engine_s_options(cx: &mut TestAppContext) {
    let (rig, desk, cx) = open(cx, "ask", agent_argv());
    let view = show_master(&desk, cx);
    say(&view, "plain hello", cx);
    settle(cx, "the first turn", |cx| turn_ended(&view, cx));
    let all = received(&rig);
    let new = all
        .iter()
        .find(|message| message["method"] == "session/new")
        .expect("a session/new");
    assert_eq!(
        new["params"]["_meta"]["claudeCode"]["options"]["model"],
        "haiku"
    );
    assert_eq!(new["params"]["mcpServers"][0]["name"], "sakura-master");
    assert_eq!(
        new["params"]["_meta"]["systemPrompt"]["append"], "You are the master.",
        "the engine's instructions reach the adapter"
    );
    let started = &all[0];
    assert_eq!(
        started["environment"]["HOME"], true,
        "an allowed name reaches the adapter"
    );
    assert_eq!(
        started["environment"]["PATH"], false,
        "the rest of the app's environment does not"
    );
    let methods = methods(&rig);
    let at = |name: &str| methods.iter().position(|method| method == name).unwrap();
    assert!(at("session/new") < at("session/set_mode"));
    assert!(
        at("session/set_mode") < at("session/prompt"),
        "the mode is set before the first prompt"
    );
    let stored = rig.sessions.stored_any(super::master_page::MASTER_SESSION);
    assert!(stored.is_some(), "the session is kept for the next launch");
}

#[gpui::test]
fn the_adapter_starts_in_the_master_mode_before_the_first_prompt(cx: &mut TestAppContext) {
    let (rig, desk, cx) = open(cx, "ask", agent_argv());
    let view = show_master(&desk, cx);
    assert_start_mode(&rig, &view, cx, "default");
}

#[gpui::test]
fn the_adapter_starts_in_the_mode_picked_before_the_first_prompt(cx: &mut TestAppContext) {
    let (rig, desk, cx) = open(cx, "ask", agent_argv());
    let view = show_master(&desk, cx);
    view.update(cx, |view, cx| view.pick_mode("plan", cx));
    assert_start_mode(&rig, &view, cx, "plan");
}

fn assert_start_mode(rig: &Rig, view: &Entity<ThreadView>, cx: &mut VisualTestContext, mode: &str) {
    say(view, "plain hello", cx);
    settle(cx, "the first turn", |cx| turn_ended(view, cx));
    let opened = set_calls(rig, "session/new");
    let options = &opened[0]["params"]["_meta"]["claudeCode"]["options"];
    assert_eq!(options["settings"]["permissions"]["defaultMode"], mode);
    assert_eq!(options["extraArgs"]["permission-mode"], mode);
    assert_eq!(options["allowDangerouslySkipPermissions"], false);
    assert_eq!(
        set_calls(rig, "session/set_mode")[0]["params"]["modeId"],
        mode
    );
}

#[gpui::test]
fn a_master_resume_keeps_the_mode_and_the_session_effort(cx: &mut TestAppContext) {
    let argv = agent_argv_with(&[
        "--choices",
        "--reported-effort",
        "high",
        "--first-is-stored",
    ]);
    let (rig, desk, cx) = open(cx, "ask", argv);
    let view = show_master(&desk, cx);
    view.update(cx, |view, cx| view.pick_mode("plan", cx));
    say(&view, "plain hello", cx);
    settle(cx, "the first turn", |cx| turn_ended(&view, cx));
    let page = desk.read_with(cx, |desk, cx| desk.master_page(cx));
    assert_eq!(
        view.read_with(cx, |view, _| view.agent_menu().unwrap().name),
        "Haiku, plan"
    );
    let first = page.update(cx, |page, cx| {
        let first = page.stored_session_for_test();
        page.start_fresh(cx);
        first
    });
    page.update(cx, |page, cx| page.resume(&first, cx));
    settle(cx, "the resumed session", |cx| {
        view.read_with(cx, |view, _| *view.phase() == Phase::Ready)
    });
    assert_eq!(
        view.read_with(cx, |view, _| view.agent_menu().unwrap().name),
        "Haiku, plan"
    );
    let new = set_calls(&rig, "session/new");
    let load = set_calls(&rig, "session/load");
    let options =
        |request: &serde_json::Value| request["params"]["_meta"]["claudeCode"]["options"].clone();
    assert!(options(&new[0]).get("effort").is_none());
    assert!(options(&load[0]).get("effort").is_none());
    assert_eq!(options(&load[0])["settings"]["effortLevel"], "high");
    assert_eq!(options(&load[0])["extraArgs"]["permission-mode"], "plan");
    assert_eq!(
        options(&load[0])["settings"]["permissions"]["defaultMode"],
        "plan"
    );
    assert_eq!(
        set_calls(&rig, "session/set_mode").last().unwrap()["params"]["modeId"],
        "plan"
    );
    assert_eq!(
        view.read_with(cx, |view, _| view
            .session_choices()
            .efforts
            .as_ref()
            .unwrap()
            .current
            .clone()),
        "high"
    );
}

#[gpui::test]
fn engine_deny_never_reaches_the_person(cx: &mut TestAppContext) {
    let (rig, desk, cx) = open(cx, "deny", agent_argv());
    let view = show_master(&desk, cx);
    say(&view, "permission", cx);
    settle(cx, "the turn", |cx| turn_ended(&view, cx));
    assert_eq!(decisions_asked(&rig), 1, "the engine was asked");
    view.read_with(cx, |view, _| {
        assert!(!view.waits_for_person());
        let cards = view
            .thread()
            .entries()
            .iter()
            .filter(|entry| matches!(entry.kind, EntryKind::Permission(_)))
            .count();
        assert_eq!(cards, 0, "no card, not even a closed one");
    });
}

#[gpui::test]
fn engine_allow_is_answered_without_asking(cx: &mut TestAppContext) {
    let (rig, desk, cx) = open(cx, "allow", agent_argv());
    let view = show_master(&desk, cx);
    say(&view, "permission", cx);
    settle(cx, "the turn", |cx| turn_ended(&view, cx));
    assert_eq!(decisions_asked(&rig), 1);
    view.read_with(cx, |view, _| assert!(!view.waits_for_person()));
}

#[gpui::test]
fn engine_ask_shows_question_in_conversation(cx: &mut TestAppContext) {
    let (rig, desk, cx) = open(cx, "ask", agent_argv());
    let view = show_master(&desk, cx);
    say(&view, "permission", cx);
    settle(cx, "the question", |cx| {
        view.read_with(cx, |view, _| view.waits_for_person())
    });
    assert_eq!(decisions_asked(&rig), 1);
    let reasons: Vec<Reason> = view.read_with(cx, |view, _| {
        let entries = view.thread().entries();
        entries
            .iter()
            .filter_map(|entry| match &entry.kind {
                EntryKind::Permission(permission) => Some(permission.reason),
                _ => None,
            })
            .collect()
    });
    assert_eq!(reasons, [Reason::EngineAsks]);
}

#[gpui::test]
fn master_session_reopened_after_relaunch(cx: &mut TestAppContext) {
    let (rig, desk, cx) = open_legacy(cx, "ask", agent_argv(), None);
    // The session of an earlier launch, which the fake agent still has.
    rig.sessions
        .store(super::master_page::MASTER_SESSION, "", "fake-session-1");
    let view = show_master(&desk, cx);
    settle(cx, "the replayed conversation", |cx| {
        view.read_with(cx, |view, _| {
            *view.phase() == Phase::Ready && !view.thread().entries().is_empty()
        })
    });
    let all = received(&rig);
    let load = all
        .iter()
        .find(|message| message["method"] == "session/load");
    let load = load.expect("the kept session is loaded");
    assert_eq!(load["params"]["sessionId"], "fake-session-1");
    assert_eq!(
        load["params"]["_meta"]["claudeCode"]["options"]["model"], "haiku",
        "the engine's options stand for a loaded session too"
    );
    assert_eq!(
        load["params"]["_meta"]["systemPrompt"]["append"], "You are the master.",
        "and so do its instructions"
    );
    assert!(methods(&rig).contains(&"session/set_mode".to_string()));
}

#[gpui::test]
fn adapter_the_engine_does_not_name_is_said_in_the_conversation(cx: &mut TestAppContext) {
    let (_rig, desk, cx) = open(cx, "ask", Vec::new());
    let view = show_master(&desk, cx);
    say(&view, "plain hello", cx);
    settle(cx, "the problem", |cx| {
        view.read_with(cx, |view, _| matches!(view.phase(), Phase::Ended(_)))
    });
    view.read_with(cx, |view, _| {
        let Phase::Ended(problem) = view.phase() else {
            unreachable!("waited for the end");
        };
        assert!(problem.detail.contains("SAKURA_ACP_ADAPTER"), "{problem:?}");
    });
}

#[gpui::test]
fn master_opened_before_the_kept_sessions_are_known_takes_up_its_session_when_they_are(
    cx: &mut TestAppContext,
) {
    let runtime = std::sync::Arc::new(tokio::runtime::Runtime::new().expect("a runtime"));
    let sessions = crate::data::sessions::Sessions::loading(runtime);
    let (rig, desk, cx) = open_legacy(cx, "ask", agent_argv(), Some(sessions.clone()));
    let view = show_master(&desk, cx);
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(500));
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        received(&rig).is_empty(),
        "nothing starts while the kept sessions are being read"
    );
    // The file is read: it holds the session of an earlier launch, which the fake agent still has.
    sessions.store(super::master_page::MASTER_SESSION, "", "fake-session-1");
    sessions.finish_loading();
    settle(cx, "the replayed conversation", |cx| {
        view.read_with(cx, |view, _| {
            *view.phase() == Phase::Ready && !view.thread().entries().is_empty()
        })
    });
    let all = received(&rig);
    let load = all
        .iter()
        .find(|message| message["method"] == "session/load");
    assert_eq!(
        load.expect("the kept session is loaded")["params"]["sessionId"],
        "fake-session-1"
    );
}

#[gpui::test]
fn the_scripted_reopen_reads_the_kept_sessions_of_the_data_folder_and_takes_up_the_session(
    cx: &mut TestAppContext,
) {
    // The sessions are kept in a private data folder: an earlier run wrote the session of the master there,
    // which the fake agent still has.
    let data = tempfile::tempdir().expect("a data folder");
    let runtime = std::sync::Arc::new(tokio::runtime::Runtime::new().expect("a runtime"));
    let earlier = crate::data::sessions::Sessions::new(Some(data.path()), runtime);
    earlier.store(super::master_page::MASTER_SESSION, "", "fake-session-1");
    let (rig, desk, cx) = open_legacy(cx, "ask", agent_argv(), None);
    settle(cx, "the file", |_| {
        let text =
            std::fs::read_to_string(data.path().join("acp-sessions.json")).unwrap_or_default();
        text.contains("fake-session-1")
    });
    assert!(
        rig.sessions
            .stored_any(super::master_page::MASTER_SESSION)
            .is_none(),
        "the conversation of the window knows nothing of the folder until it is reopened"
    );
    // Reopen: a conversation made again over the folder's file takes up the kept session.
    cx.update(|window, cx| {
        desk.update(cx, |desk, cx| {
            desk.drive_reopen_master(Some(data.path()), window, cx)
        })
    });
    let view = show_master(&desk, cx);
    settle(cx, "the kept session loaded", |cx| {
        methods(&rig).iter().any(|method| method == "session/load")
            && view.read_with(cx, |view, _| *view.phase() == Phase::Ready)
    });
}

// The cards of what the master does, the project it is told about, and the conversations left behind.

use sakura_acp::model::{ToolCallFields, ToolContent, ToolKind, ToolStatus, Update};
use sakura_acp::thread::ThreadEvent;

/// A finished call of one of the master's tools, as the adapter reports it: its title, its input and its
/// answer.
fn master_call(id: &str, tool: &str, input: serde_json::Value, answer: &str) -> ThreadEvent {
    ThreadEvent::Update(Update::ToolCall(ToolCallFields {
        id: id.to_string(),
        title: Some(format!("mcp__sakura-master__{tool}")),
        kind: Some(ToolKind::Other),
        status: Some(ToolStatus::Completed),
        content: Some(vec![ToolContent::text(answer)]),
        raw_input: Some(input),
        ..ToolCallFields::default()
    }))
}

fn show(view: &Entity<ThreadView>, events: Vec<ThreadEvent>, cx: &mut VisualTestContext) {
    view.update(cx, |view, cx| view.apply(events, cx));
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

fn active_page(desk: &Entity<Desk>, cx: &mut VisualTestContext) -> Page {
    desk.read_with(cx, |desk, cx| {
        desk.bar.read(cx).tabs().active().page().clone()
    })
}

#[gpui::test]
fn a_lane_the_master_started_is_a_card_that_opens_the_lane(cx: &mut TestAppContext) {
    let (_rig, desk, cx) = open(cx, "ask", agent_argv());
    let view = show_master(&desk, cx);
    let input =
        serde_json::json!({ "project": "repo", "text": "Make uploads retry\nwith backoff" });
    show(
        &view,
        vec![master_call("c1", "lane_open", input, r#"{"lane": "demo"}"#)],
        cx,
    );
    let card = "master-lane-card-c1";
    assert!(
        super::lane_tests::bounds(cx, card).is_some(),
        "the call is a card"
    );
    super::lane_tests::click(cx, card);
    cx.run_until_parked();
    let lane = engine_rig_tests::lane_key("demo");
    assert_eq!(
        active_page(&desk, cx),
        Page::Lane(lane),
        "a click opens the lane"
    );
}

#[gpui::test]
fn a_lane_the_engine_does_not_list_is_a_card_that_opens_nothing(cx: &mut TestAppContext) {
    let (_rig, desk, cx) = open(cx, "ask", agent_argv());
    let view = show_master(&desk, cx);
    let input = serde_json::json!({ "project": "repo", "text": "Make uploads retry" });
    show(
        &view,
        vec![master_call("c1", "lane_open", input, "No such lane yet")],
        cx,
    );
    super::lane_tests::click(cx, "master-lane-card-c1");
    cx.run_until_parked();
    assert_eq!(active_page(&desk, cx), Page::Master);
}

const PROPOSAL: &str = r#"["wait","/work/repo","master","proposal","note-1"]"#;
const PROPOSAL_CONFIRM: &str = r#"{"result": "confirm", "token": "17.ab", "expires": "2026-11-16T08:02:00Z", "summary": "Add the note notes/parser.md to the knowledge folder: 112 bytes, text exactly as shown"}"#;
const PROPOSAL_DONE: &str = r#"{"result": "done", "exit_code": 0, "output": "accepted\n"}"#;

/// The fixture with a proposal of the master waiting, which offers Accept and Reject.
fn with_proposal() -> String {
    let mut snapshot: serde_json::Value = serde_json::from_str(SNAPSHOT).expect("the fixture");
    let offered = |key: &str, label: &str| serde_json::json!({"key": key, "label": label, "needs": [], "optional": [], "confirm": true});
    let wait = serde_json::json!({
        "id": PROPOSAL, "key": ["wait", "/work/repo", "master", "proposal", "note-1"],
        "kind": "proposal", "repo": "/work/repo", "project": "repo", "task": "master",
        "title": "Add the note notes/parser.md",
        "detail": "The parser reads one line at a time.\nIt never backtracks.", "since": null,
        "actions": [offered("y", "Accept"), offered("N", "Reject")]
    });
    snapshot["waits"]
        .as_array_mut()
        .expect("the waits")
        .push(wait);
    snapshot.to_string()
}

/// The fixture whose proposal says twenty lines, the fifteenth being the consequential one.
fn with_long_proposal() -> String {
    let mut snapshot: serde_json::Value =
        serde_json::from_str(&with_proposal()).expect("the fixture");
    let lines: Vec<String> = (1..=20).map(|n| format!("line {n}")).collect();
    let waits = snapshot["waits"].as_array_mut().expect("the waits");
    let proposal = waits.iter_mut().find(|wait| wait["id"] == PROPOSAL);
    proposal.expect("the proposal")["detail"] = lines.join("\n").into();
    snapshot.to_string()
}

#[gpui::test]
fn a_proposal_longer_than_the_card_is_approved_only_after_all_of_it_is_shown(
    cx: &mut TestAppContext,
) {
    let snapshot = with_long_proposal();
    let (_rig, desk, cx) = open_over(
        cx,
        (&snapshot, PROPOSAL_CONFIRM, PROPOSAL_DONE),
        "ask",
        agent_argv(),
        None,
    );
    let view = show_master(&desk, cx);
    show(&view, vec![proposal_call()], cx);
    let bounds = |cx: &mut VisualTestContext, name: &str| super::lane_tests::bounds(cx, name);
    assert!(
        bounds(cx, "master-proposal-line-13").is_some(),
        "the 14th line"
    );
    assert!(
        bounds(cx, "master-proposal-line-14").is_none(),
        "the 15th is not drawn yet"
    );
    assert!(
        bounds(cx, &format!("master-approve-{PROPOSAL}")).is_none(),
        "no Approve over a proposal that is partly hidden"
    );
    assert!(bounds(cx, &format!("master-decline-{PROPOSAL}")).is_some());
    super::lane_tests::click(cx, "master-proposal-expand-p1");
    settle(cx, "the whole proposal", |cx| {
        bounds(cx, "master-proposal-line-19").is_some()
    });
    assert!(
        bounds(cx, &format!("master-approve-{PROPOSAL}")).is_some(),
        "Approve once every line is shown"
    );
}

#[gpui::test]
fn a_proposal_that_fits_the_card_keeps_approve_and_has_nothing_to_expand(cx: &mut TestAppContext) {
    let snapshot = with_proposal();
    let (_rig, desk, cx) = open_over(
        cx,
        (&snapshot, PROPOSAL_CONFIRM, PROPOSAL_DONE),
        "ask",
        agent_argv(),
        None,
    );
    let view = show_master(&desk, cx);
    show(&view, vec![proposal_call()], cx);
    assert!(super::lane_tests::bounds(cx, &format!("master-approve-{PROPOSAL}")).is_some());
    assert!(super::lane_tests::bounds(cx, "master-proposal-expand-p1").is_none());
}

fn proposal_call() -> ThreadEvent {
    // What the agent says in its input is not what the card shows: only the answer's exact name counts.
    let input = serde_json::json!({
        "folder": "project", "path": "somewhere/else.md", "text": "Something the engine never said",
        "reason": "Requested",
    });
    master_call(
        "p1",
        "knowledge_propose",
        input,
        r#"{"proposal": "note-1"}"#,
    )
}

fn acts(rig: &Rig) -> Vec<String> {
    let calls = std::fs::read_to_string(rig.dir.path().join("calls.log")).unwrap_or_default();
    calls
        .lines()
        .filter(|line| line.starts_with("cockpit act"))
        .map(str::to_string)
        .collect()
}

#[gpui::test]
fn a_note_the_master_proposes_is_a_card_whose_approve_asks_the_engine_to_accept_it(
    cx: &mut TestAppContext,
) {
    let snapshot = with_proposal();
    let (rig, desk, cx) = open_over(
        cx,
        (&snapshot, PROPOSAL_CONFIRM, PROPOSAL_DONE),
        "ask",
        agent_argv(),
        None,
    );
    let view = show_master(&desk, cx);
    show(&view, vec![proposal_call()], cx);
    let approve = format!("master-approve-{PROPOSAL}");
    assert!(
        super::lane_tests::bounds(cx, &approve).is_some(),
        "Approve is on the card"
    );
    assert!(super::lane_tests::bounds(cx, &format!("master-decline-{PROPOSAL}")).is_some());
    super::lane_tests::click(cx, &approve);
    settle(cx, "the call that asks", |_| acts(&rig).len() == 1);
    assert!(
        acts(&rig)[0].starts_with("cockpit act y "),
        "{:?}",
        acts(&rig)
    );
    assert!(!acts(&rig)[0].contains("--confirm"), "{:?}", acts(&rig));
    settle(cx, "the confirmed call", |cx| {
        release_hold(cx);
        acts(&rig).len() == 3
    });
    assert!(
        acts(&rig)[2].contains("--confirm 17.ab"),
        "{:?}",
        acts(&rig)
    );
}

/// Lets the six seconds an answer is held for Undo pass.
fn release_hold(cx: &mut VisualTestContext) {
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(7));
    cx.run_until_parked();
}

#[gpui::test]
fn a_card_s_approve_is_held_under_undo_for_six_seconds_and_the_card_says_so(
    cx: &mut TestAppContext,
) {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let snapshot = with_proposal();
    let (rig, desk, cx) = open_over(
        cx,
        (&snapshot, PROPOSAL_CONFIRM, PROPOSAL_DONE),
        "ask",
        agent_argv(),
        None,
    );
    let view = show_master(&desk, cx);
    show(&view, vec![proposal_call()], cx);
    super::lane_tests::click(cx, &format!("master-approve-{PROPOSAL}"));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        super::lane_tests::bounds(cx, "toast-undo").is_some(),
        "the answer is held under the Undo toast"
    );
    assert!(
        super::lane_tests::bounds(cx, &format!("master-approve-{PROPOSAL}")).is_none(),
        "the card no longer offers the answer it holds"
    );
    assert_eq!(active_page(&desk, cx), Page::Master, "the page stays");
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(5));
    cx.run_until_parked();
    assert!(
        !acts(&rig).iter().any(|act| act.contains("--confirm")),
        "held: {:?}",
        acts(&rig)
    );
    settle(cx, "the confirmed call", |cx| {
        release_hold(cx);
        acts(&rig).len() == 3
    });
}

#[gpui::test]
fn undo_takes_a_card_s_answer_back_and_nothing_is_sent(cx: &mut TestAppContext) {
    let snapshot = with_proposal();
    let (rig, desk, cx) = open_over(
        cx,
        (&snapshot, PROPOSAL_CONFIRM, PROPOSAL_DONE),
        "ask",
        agent_argv(),
        None,
    );
    let view = show_master(&desk, cx);
    show(&view, vec![proposal_call()], cx);
    super::lane_tests::click(cx, &format!("master-decline-{PROPOSAL}"));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    // The engine's question may replace the toast at any moment: press Undo on whichever one is there.
    settle(cx, "the card offers the answer again", |cx| {
        if super::lane_tests::bounds(cx, "toast-undo").is_some() {
            super::lane_tests::click(cx, "toast-undo");
        }
        super::lane_tests::bounds(cx, &format!("master-approve-{PROPOSAL}")).is_some()
    });
    release_hold(cx);
    assert!(
        !acts(&rig).iter().any(|act| act.contains("--confirm")),
        "only the call that asks may have gone: {:?}",
        acts(&rig)
    );
}

#[gpui::test]
fn declining_a_proposal_sends_the_engine_s_own_reject(cx: &mut TestAppContext) {
    let snapshot = with_proposal();
    let (rig, desk, cx) = open_over(
        cx,
        (&snapshot, PROPOSAL_CONFIRM, PROPOSAL_DONE),
        "ask",
        agent_argv(),
        None,
    );
    let view = show_master(&desk, cx);
    show(&view, vec![proposal_call()], cx);
    super::lane_tests::click(cx, &format!("master-decline-{PROPOSAL}"));
    settle(cx, "the confirmed call", |cx| {
        release_hold(cx);
        acts(&rig).len() == 3
    });
    assert!(
        acts(&rig)[0].starts_with("cockpit act N "),
        "{:?}",
        acts(&rig)
    );
}

#[gpui::test]
fn a_proposal_answer_that_only_mentions_a_name_binds_no_wait_and_offers_the_inbox(
    cx: &mut TestAppContext,
) {
    let snapshot = with_proposal();
    let (rig, desk, cx) = open_over(
        cx,
        (&snapshot, PROPOSAL_CONFIRM, PROPOSAL_DONE),
        "ask",
        agent_argv(),
        None,
    );
    let view = show_master(&desk, cx);
    let input = serde_json::json!({ "path": "notes/parser.md", "text": "x" });
    show(
        &view,
        vec![master_call(
            "p1",
            "knowledge_propose",
            input,
            "Proposed note-1 for you",
        )],
        cx,
    );
    assert!(super::lane_tests::bounds(cx, &format!("master-approve-{PROPOSAL}")).is_none());
    assert!(super::lane_tests::bounds(cx, &format!("master-decline-{PROPOSAL}")).is_none());
    super::lane_tests::click(cx, "master-open-inbox-p1");
    cx.run_until_parked();
    assert_eq!(active_page(&desk, cx), Page::Inbox);
    assert!(acts(&rig).is_empty(), "nothing was sent to the engine");
}

#[gpui::test]
fn a_project_chosen_is_the_first_line_of_what_the_master_is_sent_and_not_of_what_is_shown(
    cx: &mut TestAppContext,
) {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let (rig, desk, cx) = open(cx, "ask", agent_argv());
    let view = show_master(&desk, cx);
    view.update(cx, |view, cx| {
        view.set_project(Some("repo".to_string()), cx)
    });
    say(&view, "plain hello", cx);
    settle(cx, "the turn", |cx| turn_ended(&view, cx));
    let sent = received(&rig)
        .into_iter()
        .find(|message| message["method"] == "session/prompt")
        .expect("a prompt");
    assert_eq!(
        sent["params"]["prompt"][0]["text"],
        "Project: repo.\nplain hello"
    );
    view.read_with(cx, |view, _| {
        let shown = view
            .thread()
            .entries()
            .iter()
            .find_map(|entry| match &entry.kind {
                EntryKind::User(text) => Some(text.clone()),
                _ => None,
            });
        assert_eq!(shown.as_deref(), Some("plain hello"));
    });
}

#[gpui::test]
fn starting_fresh_leaves_the_conversation_in_history_and_the_next_message_opens_a_new_session(
    cx: &mut TestAppContext,
) {
    let (rig, desk, cx) = open(cx, "ask", agent_argv());
    let view = show_master(&desk, cx);
    say(&view, "plain hello", cx);
    settle(cx, "the first turn", |cx| turn_ended(&view, cx));
    let first = rig
        .sessions
        .stored_any(super::master_page::MASTER_SESSION)
        .expect("kept");
    let page = desk.read_with(cx, |desk, cx| desk.master_page(cx));
    page.update(cx, |page, cx| page.start_fresh(cx));
    cx.run_until_parked();
    view.read_with(cx, |view, _| {
        assert!(view.thread().entries().is_empty(), "an empty conversation")
    });
    assert!(rig
        .sessions
        .stored_any(super::master_page::MASTER_SESSION)
        .is_none());
    say(&view, "plain again", cx);
    settle(cx, "the second turn", |cx| turn_ended(&view, cx));
    assert!(
        rig.sessions
            .stored_any(super::master_page::MASTER_SESSION)
            .is_some(),
        "the new session is kept"
    );
    let opened = methods(&rig)
        .iter()
        .filter(|method| *method == "session/new")
        .count();
    assert_eq!(opened, 2);
    let history = page.read_with(cx, |page, _| page.history_for_test());
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].session, first);
    assert_eq!(history[0].title, "plain hello");
}

#[gpui::test]
fn an_earlier_conversation_from_history_is_loaded_again_and_the_open_one_takes_its_place_there(
    cx: &mut TestAppContext,
) {
    let (rig, desk, cx) = open(cx, "ask", agent_argv());
    let view = show_master(&desk, cx);
    say(&view, "plain hello", cx);
    settle(cx, "the first turn", |cx| turn_ended(&view, cx));
    let open_session = rig
        .sessions
        .stored_any(super::master_page::MASTER_SESSION)
        .expect("kept");
    let page = desk.read_with(cx, |desk, cx| desk.master_page(cx));
    page.update(cx, |page, cx| {
        page.add_past_for_test("fake-session-1", "An older question");
        page.resume("fake-session-1", cx)
    });
    settle(cx, "the replayed conversation", |cx| {
        view.read_with(cx, |view, _| {
            *view.phase() == Phase::Ready && !view.thread().entries().is_empty()
        })
    });
    let loaded = received(&rig)
        .into_iter()
        .find(|message| message["method"] == "session/load");
    assert_eq!(
        loaded.expect("loaded")["params"]["sessionId"],
        "fake-session-1"
    );
    let history = page.read_with(cx, |page, _| page.history_for_test());
    assert_eq!(
        history
            .iter()
            .map(|past| past.session.as_str())
            .collect::<Vec<_>>(),
        [open_session.as_str()]
    );
}

#[gpui::test]
fn a_reopened_conversation_resumes_with_the_model_and_the_mode_it_had(cx: &mut TestAppContext) {
    let (_rig, desk, cx) = open(cx, "ask", agent_argv_with(&["--choices"]));
    let view = show_master(&desk, cx);
    say(&view, "plain hello", cx);
    settle(cx, "the first turn", |cx| turn_ended(&view, cx));
    view.update(cx, |view, cx| {
        view.pick_model("sonnet", cx);
        view.pick_mode("plan", cx);
    });
    settle(cx, "the picks", |cx| {
        view.read_with(cx, |view, _| {
            let choices = view.session_choices();
            let model = choices
                .models
                .as_ref()
                .map(|models| models.current.as_str());
            model == Some("sonnet")
        })
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let page = desk.read_with(cx, |desk, cx| desk.master_page(cx));
    let first = page.update(cx, |page, cx| {
        let first = page.stored_session_for_test();
        page.start_fresh(cx);
        first
    });
    let start = cx.update(|_, cx| super::master_page::MasterPage::kept_picks_for_test(&first, cx));
    assert_eq!(start.model.as_deref(), Some("sonnet"));
    assert_eq!(start.mode.as_deref(), Some("plan"));
}

/// The first message of the conversation the tab shown holds.
fn shown_first_message(desk: &Entity<Desk>, cx: &mut VisualTestContext) -> Option<String> {
    let view = desk.read_with(cx, |desk, cx| desk.master_conversation(cx));
    view.read_with(cx, |view, _| view.first_message())
}

/// Requirement changed (owner, 20/11): a tab of its own for each conversation, so a new tab no longer moves the
/// earlier conversation to History.
#[gpui::test]
fn each_master_tab_holds_a_conversation_of_its_own_and_keeps_it_when_its_tab_is_shown_again(
    cx: &mut TestAppContext,
) {
    let (rig, desk, cx) = open(cx, "ask", agent_argv());
    let first = show_master(&desk, cx);
    say(&first, "plain one", cx);
    settle(cx, "the first turn", |cx| turn_ended(&first, cx));
    super::lane_tests::click(cx, "bar-new-tab");
    cx.run_until_parked();
    let second = desk.read_with(cx, |desk, cx| desk.master_conversation(cx));
    assert_ne!(
        second.entity_id(),
        first.entity_id(),
        "a conversation of its own"
    );
    assert_eq!(shown_first_message(&desk, cx), None, "an empty one");
    say(&second, "plain two", cx);
    settle(cx, "the second turn", |cx| turn_ended(&second, cx));
    cx.simulate_keystrokes("cmd-t");
    cx.run_until_parked();
    assert_eq!(
        shown_first_message(&desk, cx),
        None,
        "the third tab is empty"
    );
    let third = desk.read_with(cx, |desk, cx| desk.master_conversation(cx));
    say(&third, "plain three", cx);
    settle(cx, "the third turn", |cx| turn_ended(&third, cx));
    for (key, words) in [
        ("cmd-1", "plain one"),
        ("cmd-2", "plain two"),
        ("cmd-3", "plain three"),
    ] {
        cx.simulate_keystrokes(key);
        cx.run_until_parked();
        assert_eq!(
            shown_first_message(&desk, cx).as_deref(),
            Some(words),
            "{key}"
        );
    }
    let page = desk.read_with(cx, |desk, cx| desk.master_page(cx));
    assert!(
        page.read_with(cx, |page, _| page.history_for_test())
            .is_empty(),
        "no conversation was moved to History"
    );
    let opened = methods(&rig).iter().filter(|m| *m == "session/new").count();
    assert_eq!(opened, 3, "each conversation has a session of its own");
}

// Files and images attached to a message.

/// The fake agent with extra flags.
fn agent_argv_with(flags: &[&str]) -> Vec<String> {
    let mut argv = agent_argv();
    argv.extend(flags.iter().map(|flag| flag.to_string()));
    argv
}

/// A folder holding a text file, a picture and a file too large for its kind.
fn files() -> (tempfile::TempDir, [std::path::PathBuf; 3]) {
    let folder = tempfile::tempdir().expect("a folder");
    let notes = folder.path().join("notes.md");
    let picture = folder.path().join("shot.png");
    let large = folder.path().join("large.txt");
    std::fs::write(&notes, "# Notes\nOne line.").expect("a file");
    std::fs::write(&picture, [0x89, b'P', b'N', b'G']).expect("a file");
    std::fs::write(
        &large,
        vec![b'a'; sakura_acp::attachment::MAX_TEXT_BYTES + 1],
    )
    .expect("a file");
    (folder, [notes, picture, large])
}

fn attached(view: &Entity<ThreadView>, cx: &mut VisualTestContext) -> Vec<String> {
    view.read_with(cx, |view, _| view.attachment_names())
}

#[gpui::test]
fn files_attached_show_as_chips_that_can_be_removed(cx: &mut TestAppContext) {
    let (_rig, desk, cx) = open(cx, "ask", agent_argv());
    let view = show_master(&desk, cx);
    let (_folder, [notes, picture, _]) = files();
    view.update(cx, |view, cx| view.attach_paths(&[notes, picture], cx));
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert_eq!(attached(&view, cx), ["notes.md", "shot.png"]);
    assert!(super::lane_tests::bounds(cx, "master-attachment-0").is_some());
    assert!(super::lane_tests::bounds(cx, "master-attachment-1").is_some());
    super::lane_tests::click(cx, "master-attachment-remove-0");
    assert_eq!(attached(&view, cx), ["shot.png"]);
}

#[gpui::test]
fn a_file_is_read_off_the_calling_thread_and_attached_once_the_read_is_done(
    cx: &mut TestAppContext,
) {
    let (_rig, desk, cx) = open(cx, "ask", agent_argv());
    let view = show_master(&desk, cx);
    let (_folder, [notes, ..]) = files();
    view.update(cx, |view, cx| view.attach_paths(&[notes], cx));
    assert!(
        attached(&view, cx).is_empty(),
        "the call returned before the file was read"
    );
    cx.run_until_parked();
    assert_eq!(attached(&view, cx), ["notes.md"]);
}

#[gpui::test]
fn the_first_file_refused_stops_the_ones_after_it_and_the_ones_before_stay(
    cx: &mut TestAppContext,
) {
    let (_rig, desk, cx) = open(cx, "ask", agent_argv());
    let view = show_master(&desk, cx);
    let (_folder, [notes, picture, large]) = files();
    view.update(cx, |view, cx| {
        view.attach_paths(&[notes, large, picture], cx)
    });
    cx.run_until_parked();
    assert_eq!(attached(&view, cx), ["notes.md"]);
    let refusal = view.read_with(cx, |view, _| view.refusal().cloned());
    assert!(matches!(
        refusal,
        Some(sakura_acp::attachment::Refusal::TooLarge { .. })
    ));
}

#[gpui::test]
fn a_file_too_large_is_refused_in_one_plain_line_and_not_attached(cx: &mut TestAppContext) {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let (_rig, desk, cx) = open(cx, "ask", agent_argv());
    let view = show_master(&desk, cx);
    let (_folder, [_, _, large]) = files();
    view.update(cx, |view, cx| view.attach_paths(&[large], cx));
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(attached(&view, cx).is_empty());
    assert!(super::lane_tests::bounds(cx, "master-attachment-refusal").is_some());
    let said = view.read_with(cx, |view, _| {
        super::thread::refusal_words(view.refusal().expect("a refusal")).to_string()
    });
    assert_eq!(
        said,
        "large.txt is larger than 256 KB, so it was not attached."
    );
}

#[gpui::test]
fn attached_files_go_with_the_message_as_content_blocks_and_are_named_in_the_transcript(
    cx: &mut TestAppContext,
) {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let (rig, desk, cx) = open(cx, "ask", agent_argv());
    let view = show_master(&desk, cx);
    let (_folder, [notes, picture, _]) = files();
    view.update(cx, |view, cx| view.attach_paths(&[notes, picture], cx));
    cx.run_until_parked();
    say(&view, "plain look at these", cx);
    settle(cx, "the turn", |cx| turn_ended(&view, cx));
    let sent = received(&rig)
        .into_iter()
        .find(|message| message["method"] == "session/prompt")
        .expect("a prompt");
    let blocks = sent["params"]["prompt"].as_array().expect("blocks");
    assert_eq!(blocks.len(), 3);
    assert_eq!(blocks[0]["text"], "plain look at these");
    assert_eq!(blocks[1]["resource"]["text"], "# Notes\nOne line.");
    assert_eq!(
        (blocks[2]["type"].as_str(), blocks[2]["mimeType"].as_str()),
        (Some("image"), Some("image/png"))
    );
    assert!(
        attached(&view, cx).is_empty(),
        "the chips go with the message"
    );
    view.read_with(cx, |view, _| {
        let shown = view
            .thread()
            .entries()
            .iter()
            .find_map(|entry| match &entry.kind {
                EntryKind::User(text) => Some(text.clone()),
                _ => None,
            });
        assert_eq!(
            shown.as_deref(),
            Some("plain look at these\nAttached: notes.md, shot.png")
        );
    });
}

#[gpui::test]
fn files_dropped_on_the_conversation_are_attached(cx: &mut TestAppContext) {
    use gpui::{point, px, ExternalPaths, FileDropEvent};
    let (_rig, desk, cx) = open(cx, "ask", agent_argv());
    let view = show_master(&desk, cx);
    let (_folder, [notes, ..]) = files();
    let at = point(px(600.), px(300.));
    let paths = ExternalPaths(std::iter::once(notes).collect());
    cx.simulate_event(FileDropEvent::Entered {
        position: at,
        paths,
    });
    cx.simulate_event(FileDropEvent::Submit { position: at });
    cx.run_until_parked();
    assert_eq!(attached(&view, cx), ["notes.md"]);
}

#[gpui::test]
fn the_plus_button_opens_the_file_chooser_and_attaches_what_it_gives(cx: &mut TestAppContext) {
    let (_rig, desk, cx) = open(cx, "ask", agent_argv());
    let view = show_master(&desk, cx);
    cx.update(|window, cx| window.draw(cx).clear(cx));
    super::lane_tests::click(cx, "master-attach");
    assert!(cx.did_prompt_for_paths(), "the system's chooser opened");
    let (_folder, [notes, ..]) = files();
    cx.simulate_path_prompt_response(|options| {
        assert!(options.files && options.multiple && !options.directories);
        Some(vec![notes])
    });
    cx.run_until_parked();
    assert_eq!(attached(&view, cx), ["notes.md"]);
}

#[cfg(feature = "snapshot")]
#[gpui::test]
fn drive_pick_file_answers_the_next_file_chooser_without_showing_it(cx: &mut TestAppContext) {
    let (_rig, desk, cx) = open(cx, "ask", agent_argv());
    let view = show_master(&desk, cx);
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let (_folder, [notes, picture, _]) = files();
    let argument = format!("{},{}", notes.display(), picture.display());
    cx.update(|_, cx| crate::ui::thread::pick_next_files(&argument, cx));
    super::lane_tests::click(cx, "master-attach");
    cx.run_until_parked();
    assert!(!cx.did_prompt_for_paths(), "the chooser was not shown");
    assert_eq!(attached(&view, cx), ["notes.md", "shot.png"]);
    super::lane_tests::click(cx, "master-attach");
    assert!(cx.did_prompt_for_paths(), "the next one is the Mac's own");
}

#[gpui::test]
fn an_image_on_the_clipboard_is_attached_on_paste_and_text_still_goes_to_the_field(
    cx: &mut TestAppContext,
) {
    use gpui::{ClipboardItem, Image, ImageFormat};
    let (_rig, desk, cx) = open(cx, "ask", agent_argv());
    let view = show_master(&desk, cx);
    cx.update(|window, cx| {
        let composer = view.read(cx).composer_handle(cx);
        window.focus(&composer, cx);
    });
    cx.write_to_clipboard(ClipboardItem::new_image(&Image::from_bytes(
        ImageFormat::Png,
        vec![1, 2, 3],
    )));
    cx.simulate_keystrokes("cmd-v");
    assert_eq!(attached(&view, cx), ["pasted-image.png"]);
    cx.write_to_clipboard(ClipboardItem::new_string("some words".to_string()));
    cx.simulate_keystrokes("cmd-v");
    assert_eq!(attached(&view, cx).len(), 1, "text is not an attachment");
    assert_eq!(view.read_with(cx, |view, cx| view.draft(cx)), "some words");
}

#[gpui::test]
fn an_agent_that_takes_no_images_is_not_given_one(cx: &mut TestAppContext) {
    let (rig, desk, cx) = open(cx, "ask", agent_argv_with(&["--no-attachments"]));
    let view = show_master(&desk, cx);
    say(&view, "plain hello", cx);
    settle(cx, "the turn", |cx| turn_ended(&view, cx));
    let (_folder, [notes, picture, _]) = files();
    view.update(cx, |view, cx| view.attach_paths(&[picture], cx));
    cx.run_until_parked();
    assert!(attached(&view, cx).is_empty());
    let refusal = view.read_with(cx, |view, _| view.refusal().cloned());
    assert_eq!(
        refusal,
        Some(sakura_acp::attachment::Refusal::ImagesNotTaken {
            name: "shot.png".into()
        })
    );
    view.update(cx, |view, cx| view.attach_paths(&[notes], cx));
    cx.run_until_parked();
    assert!(attached(&view, cx).is_empty(), "no embedded files either");
    assert!(received(&rig)
        .iter()
        .all(|message| message["method"] != "x"));
}

// Dictation.

#[gpui::test]
fn the_microphone_gives_the_composer_the_keyboard_and_asks_the_system_to_dictate(
    cx: &mut TestAppContext,
) {
    let (_rig, desk, cx) = open(cx, "ask", agent_argv());
    let view = show_master(&desk, cx);
    let asked = Rc::new(std::cell::Cell::new(0));
    let counted = asked.clone();
    view.update(cx, |view, _| {
        view.set_dictation(Rc::new(move || {
            counted.set(counted.get() + 1);
            true
        }))
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let focused = |view: &Entity<ThreadView>, cx: &mut VisualTestContext| {
        cx.update(|window, cx| view.read(cx).composer_handle(cx).is_focused(window))
    };
    assert!(
        !focused(&view, cx),
        "nothing has the keyboard until the person acts"
    );
    super::lane_tests::click(cx, "master-dictate");
    assert_eq!(asked.get(), 1, "the system was asked once");
    assert!(focused(&view, cx), "the dictated words go to the composer");
    assert!(super::lane_tests::bounds(cx, "master-dictation-unavailable").is_none());
}

#[gpui::test]
fn dictation_that_does_not_start_says_so_in_one_line(cx: &mut TestAppContext) {
    let (_rig, desk, cx) = open(cx, "ask", agent_argv());
    let view = show_master(&desk, cx);
    view.update(cx, |view, _| view.set_dictation(Rc::new(|| false)));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    super::lane_tests::click(cx, "master-dictate");
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(super::lane_tests::bounds(cx, "master-dictation-unavailable").is_some());
}

#[gpui::test]
fn words_the_system_inserts_into_the_focused_composer_land_in_it_as_dictation_does(
    cx: &mut TestAppContext,
) {
    let (_rig, desk, cx) = open(cx, "ask", agent_argv());
    let view = show_master(&desk, cx);
    cx.update(|window, cx| {
        let composer = view.read(cx).composer_handle(cx);
        window.focus(&composer, cx);
    });
    // The text input path the window's view gives the platform: `insertText:` of a dictation lands here.
    cx.simulate_input("remind me to review the lane");
    assert_eq!(
        view.read_with(cx, |view, cx| view.draft(cx)),
        "remind me to review the lane"
    );
}

// The model and the mode of the session.

/// The master with an agent that offers choices, its first turn done and drawn.
fn with_choices<'a>(
    cx: &'a mut TestAppContext,
    flags: &[&str],
) -> (Rig, Entity<ThreadView>, &'a mut VisualTestContext) {
    let (rig, desk, cx) = open(cx, "ask", agent_argv_with(flags));
    let view = show_master(&desk, cx);
    say(&view, "plain hello", cx);
    settle(cx, "the first turn", |cx| turn_ended(&view, cx));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    (rig, view, cx)
}

fn set_calls(rig: &Rig, method: &str) -> Vec<serde_json::Value> {
    received(rig)
        .into_iter()
        .filter(|message| message["method"] == method)
        .collect()
}

#[gpui::test]
fn the_master_s_tool_calls_in_a_row_are_one_folded_line_that_a_press_opens(
    cx: &mut TestAppContext,
) {
    let (_rig, desk, cx) = open(cx, "ask", agent_argv());
    let view = show_master(&desk, cx);
    say(&view, "tools", cx);
    settle(cx, "the turn", |cx| turn_ended(&view, cx));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let tools: Vec<u64> = view.read_with(cx, |view, _| {
        let tools = view.thread().entries().iter();
        tools
            .filter(|entry| matches!(entry.kind, EntryKind::Tool(_)))
            .map(|entry| entry.id)
            .collect()
    });
    assert_eq!(tools.len(), 3, "the script makes three calls");
    let group = format!("thread-tools-{}", tools[0]);
    assert!(
        super::lane_tests::bounds(cx, &group).is_some(),
        "one row for the three"
    );
    for id in &tools {
        assert!(
            super::lane_tests::bounds(cx, &format!("thread-tool-{id}")).is_none(),
            "a call alone is not drawn while the row is folded"
        );
    }
    super::lane_tests::click(cx, &group);
    cx.update(|window, cx| window.draw(cx).clear(cx));
    for id in &tools {
        assert!(
            super::lane_tests::bounds(cx, &format!("thread-tool-{id}")).is_some(),
            "opened: every call has its line"
        );
    }
}

#[gpui::test]
fn an_empty_master_says_so_in_the_middle_of_its_page(cx: &mut TestAppContext) {
    let (_rig, desk, cx) = open(cx, "ask", agent_argv());
    let _view = show_master(&desk, cx);
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let thread = super::lane_tests::bounds(cx, "thread").expect("the conversation");
    let empty = super::lane_tests::bounds(cx, "thread-empty").expect("the empty sentence");
    assert!(
        (empty.center().x - thread.center().x).abs() < gpui::px(2.),
        "centred across: {:?} against {:?}",
        empty.center().x,
        thread.center().x
    );
    assert!(
        empty.center().y > thread.origin.y + thread.size.height / 4.
            && empty.center().y < thread.origin.y + thread.size.height * 3. / 4.,
        "not at the top"
    );
}

#[gpui::test]
fn an_empty_master_says_what_to_ask_and_offers_two_requests_to_start_from(cx: &mut TestAppContext) {
    let (rig, desk, cx) = open(cx, "ask", agent_argv());
    let _view = show_master(&desk, cx);
    cx.update(|window, cx| window.draw(cx).clear(cx));
    for example in ["thread-example-0", "thread-example-1"] {
        assert!(
            super::lane_tests::bounds(cx, example).is_some(),
            "{example}"
        );
    }
    super::lane_tests::click(cx, "thread-example-1");
    cx.run_until_parked();
    assert!(
        received(&rig).is_empty(),
        "an example fills the box and sends nothing"
    );
}

#[gpui::test]
fn a_session_that_offers_models_and_modes_shows_one_agent_chip_and_no_model_or_mode_chip(
    cx: &mut TestAppContext,
) {
    let (_rig, view, cx) = with_choices(cx, &["--choices"]);
    assert!(super::lane_tests::bounds(cx, "master-agent-chip").is_some());
    assert!(super::lane_tests::bounds(cx, "master-model-chip").is_none());
    assert!(super::lane_tests::bounds(cx, "master-mode-chip").is_none());
    let models = view.read_with(cx, |view, _| {
        view.session_choices().models.clone().expect("models")
    });
    assert_eq!(
        (models.current.as_str(), models.current_name()),
        ("haiku", "Haiku")
    );
}

#[gpui::test]
fn the_spec_s_model_is_the_one_the_session_runs_and_the_chip_names_it(cx: &mut TestAppContext) {
    let (rig, desk, cx) = open(cx, "ask", agent_argv_with(&["--choices"]));
    let path = rig.dir.path().join("spec.json");
    let text = std::fs::read_to_string(&path).expect("the spec");
    std::fs::write(
        &path,
        text.replace(r#""model":"haiku""#, r#""model":"sonnet""#),
    )
    .expect("spec");
    let view = show_master(&desk, cx);
    say(&view, "plain hello", cx);
    settle(cx, "the first turn", |cx| turn_ended(&view, cx));
    let models = view.read_with(cx, |view, _| {
        view.session_choices().models.clone().expect("models")
    });
    assert_eq!(
        (models.current.as_str(), models.current_name()),
        ("sonnet", "Sonnet"),
        "the agent started on Haiku; the spec says Sonnet"
    );
    assert_eq!(set_calls(&rig, "session/set_config_option").len(), 1);
}

#[gpui::test]
fn a_model_picked_in_the_conversation_is_not_put_back_to_the_spec_s(cx: &mut TestAppContext) {
    let (rig, view, cx) = with_choices(cx, &["--choices"]);
    view.update(cx, |view, cx| view.pick_model("sonnet", cx));
    settle(cx, "the model", |cx| {
        view.read_with(cx, |view, _| {
            view.session_choices()
                .models
                .as_ref()
                .is_some_and(|models| models.current == "sonnet")
        })
    });
    say(&view, "plain again", cx);
    settle(cx, "a turn after the pick", |cx| turn_ended(&view, cx));
    let set = set_calls(&rig, "session/set_config_option");
    assert_eq!(set.len(), 1, "only the person's pick: {set:?}");
}

#[gpui::test]
fn a_session_that_offers_one_of_each_shows_no_chip(cx: &mut TestAppContext) {
    let (_rig, _view, cx) = with_choices(cx, &["--choices", "--single-choice"]);
    assert!(super::lane_tests::bounds(cx, "master-model-chip").is_none());
    assert!(super::lane_tests::bounds(cx, "master-mode-chip").is_none());
}

#[gpui::test]
fn a_session_that_offers_nothing_shows_no_chip(cx: &mut TestAppContext) {
    let (_rig, _view, cx) = with_choices(cx, &[]);
    assert!(super::lane_tests::bounds(cx, "master-model-chip").is_none());
    assert!(super::lane_tests::bounds(cx, "master-mode-chip").is_none());
}

#[gpui::test]
fn picking_a_model_sets_it_on_the_session_and_the_chip_follows(cx: &mut TestAppContext) {
    let (rig, view, cx) = with_choices(cx, &["--choices"]);
    view.update(cx, |view, cx| view.pick_model("sonnet", cx));
    settle(cx, "the model", |cx| {
        view.read_with(cx, |view, _| {
            view.session_choices()
                .models
                .as_ref()
                .is_some_and(|models| models.current == "sonnet")
        })
    });
    let set = set_calls(&rig, "session/set_config_option");
    assert_eq!(
        (
            set[0]["params"]["configId"].as_str(),
            set[0]["params"]["value"].as_str()
        ),
        (Some("model"), Some("sonnet"))
    );
    say(&view, "plain again", cx);
    settle(cx, "a turn after the change", |cx| turn_ended(&view, cx));
}

#[gpui::test]
fn picking_plan_mode_sets_it_and_a_mode_that_stops_asking_is_never_sent(cx: &mut TestAppContext) {
    let (rig, view, cx) = with_choices(cx, &["--choices"]);
    view.update(cx, |view, cx| view.pick_mode("plan", cx));
    settle(cx, "the mode", |cx| {
        view.read_with(cx, |view, _| {
            view.session_choices()
                .modes
                .as_ref()
                .is_some_and(|modes| modes.current == "plan")
        })
    });
    // The master's start mode was set once, then plan: two calls, in that order.
    let modes: Vec<String> = set_calls(&rig, "session/set_mode")
        .iter()
        .map(|call| {
            call["params"]["modeId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assert_eq!(modes, ["default", "plan"]);
    view.update(cx, |view, cx| {
        view.pick_mode("acceptEdits", cx);
        view.pick_mode("bypassPermissions", cx);
    });
    cx.run_until_parked();
    assert_eq!(
        set_calls(&rig, "session/set_mode").len(),
        2,
        "the engine keeps deciding what the master may do"
    );
}

/// The empty master with an agent that offers choices, drawn, before anything is sent.
fn empty_with_choices(
    cx: &mut TestAppContext,
) -> (Rig, Entity<ThreadView>, &mut VisualTestContext) {
    let mut kept = Preferences::default();
    let models =
        [("haiku", "Haiku"), ("sonnet", "Sonnet")].map(|(id, name)| sakura_acp::model::Choice {
            id: id.to_string(),
            name: name.to_string(),
            ..Default::default()
        });
    kept.master_models.insert(
        "claude".into(),
        super::master_agent::models_to_json(&models),
    );
    let (rig, desk, cx) = open_kept(
        cx,
        (SNAPSHOT, "{}", "{}"),
        "ask",
        agent_argv_with(&["--choices"]),
        (None, kept),
    );
    let view = show_master(&desk, cx);
    settle(cx, "the Master chip", |cx| drawn(cx, "master-agent-chip"));
    (rig, view, cx)
}

#[gpui::test]
fn an_empty_conversation_shows_the_agent_chip_before_anything_is_sent(cx: &mut TestAppContext) {
    let (_rig, _view, cx) = empty_with_choices(cx);
    assert!(super::lane_tests::bounds(cx, "master-agent-chip").is_some());
    for chip in ["master-model-chip", "master-mode-chip"] {
        assert!(
            super::lane_tests::bounds(cx, chip).is_none(),
            "{chip} is in the agent chip's menu"
        );
    }
    assert!(
        received(&_rig).is_empty(),
        "looking at the chips starts nothing"
    );
}

#[gpui::test]
fn the_agent_chip_before_the_first_message_opens_a_menu_to_pick_from(cx: &mut TestAppContext) {
    let (_rig, view, cx) = empty_with_choices(cx);
    let chip = super::lane_tests::bounds(cx, "master-agent-chip").expect("the chip");
    cx.simulate_click(chip.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let is_open = view.read_with(cx, |_, _| true);
    assert!(is_open);
    assert!(
        super::lane_tests::bounds(cx, "master-agent-chip").is_some(),
        "the chip is still there after a press"
    );
}

#[gpui::test]
fn the_model_and_the_mode_chosen_before_the_first_message_are_the_ones_the_session_starts_with(
    cx: &mut TestAppContext,
) {
    let (rig, view, cx) = empty_with_choices(cx);
    view.update(cx, |view, cx| {
        view.pick_model("sonnet", cx);
        view.pick_mode("plan", cx);
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(received(&rig).is_empty(), "choosing starts nothing");
    say(&view, "plain hello", cx);
    settle(cx, "the first turn", |cx| turn_ended(&view, cx));
    let (models, modes) = view.read_with(cx, |view, _| {
        let choices = view.session_choices();
        (choices.models.clone(), choices.modes.clone())
    });
    let (models, modes) = (models.expect("models"), modes.expect("modes"));
    assert_eq!(models.current, "sonnet", "the spec says Haiku");
    assert_eq!(modes.current, "plan");
    let set = set_calls(&rig, "session/set_config_option");
    assert_eq!(set[0]["params"]["value"].as_str(), Some("sonnet"));
}

#[gpui::test]
fn the_default_model_is_the_specs_and_nothing_chosen_changes_the_start(cx: &mut TestAppContext) {
    let (_rig, view, cx) = empty_with_choices(cx);
    say(&view, "plain hello", cx);
    settle(cx, "the first turn", |cx| turn_ended(&view, cx));
    let models = view.read_with(cx, |view, _| {
        view.session_choices().models.clone().expect("models")
    });
    assert_eq!(models.current, "haiku");
}

#[gpui::test]
fn a_new_conversation_forgets_what_was_chosen_for_the_one_before(cx: &mut TestAppContext) {
    let (_rig, view, cx) = empty_with_choices(cx);
    view.update(cx, |view, cx| {
        view.pick_model("sonnet", cx);
        view.start_over(cx);
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    say(&view, "plain hello", cx);
    settle(cx, "the first turn", |cx| turn_ended(&view, cx));
    let models = view.read_with(cx, |view, _| {
        view.session_choices().models.clone().expect("models")
    });
    assert_eq!(models.current, "haiku");
}

// Slash commands.

/// Writes `text` in the composer, without sending it.
fn write(view: &Entity<ThreadView>, text: &str, cx: &mut VisualTestContext) {
    cx.update(|window, cx| view.update(cx, |view, cx| view.write(text, window, cx)));
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

fn focus_composer(view: &Entity<ThreadView>, cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        let composer = view.read(cx).composer_handle(cx);
        window.focus(&composer, cx);
    });
}

#[gpui::test]
fn typing_a_slash_opens_the_menu_of_announced_commands_filtered_as_the_word_is_typed(
    cx: &mut TestAppContext,
) {
    let (_rig, view, cx) = with_choices(cx, &["--commands"]);
    assert_eq!(
        view.read_with(cx, |view, _| view.announced_commands().len()),
        4
    );
    assert!(
        super::lane_tests::bounds(cx, "slash-menu").is_none(),
        "closed until a slash is typed"
    );
    let composer = super::lane_tests::bounds(cx, "thread-composer").unwrap();
    let transcript = super::lane_tests::bounds(cx, "thread-transcript").unwrap();
    write(&view, "/", cx);
    let menu = super::lane_tests::bounds(cx, "slash-menu").unwrap();
    assert_eq!(
        super::lane_tests::bounds(cx, "thread-composer"),
        Some(composer)
    );
    assert!(menu.bottom() <= composer.top());
    assert_eq!(
        super::lane_tests::bounds(cx, "thread-transcript"),
        Some(transcript)
    );
    for name in ["compact", "review", "mcp:docs:search", "context"] {
        assert!(
            super::lane_tests::bounds(cx, &format!("slash-row-{name}")).is_some(),
            "{name}"
        );
    }
    write(&view, "/co", cx);
    assert!(super::lane_tests::bounds(cx, "slash-row-compact").is_some());
    assert!(super::lane_tests::bounds(cx, "slash-row-context").is_some());
    assert!(
        super::lane_tests::bounds(cx, "slash-row-review").is_none(),
        "filtered out"
    );
    write(&view, "/compact now", cx);
    assert!(
        super::lane_tests::bounds(cx, "slash-menu").is_none(),
        "closed once the word is done"
    );
    write(&view, "hello /co", cx);
    assert!(
        super::lane_tests::bounds(cx, "slash-menu").is_none(),
        "only at the start"
    );
}

#[gpui::test]
fn return_puts_the_highlighted_command_in_the_composer_and_does_not_send(cx: &mut TestAppContext) {
    let (rig, view, cx) = with_choices(cx, &["--commands"]);
    focus_composer(&view, cx);
    cx.simulate_input("/re");
    cx.simulate_keystrokes("enter");
    assert_eq!(view.read_with(cx, |view, cx| view.draft(cx)), "/review");
    cx.simulate_input("12");
    assert_eq!(
        view.read_with(cx, |view, cx| view.draft(cx)),
        "/review 12",
        "arguments follow the command"
    );
    assert_eq!(
        set_calls(&rig, "session/prompt").len(),
        1,
        "nothing was sent: only the first message of the turn before"
    );
}

#[gpui::test]
fn return_inside_a_code_fence_breaks_the_line_and_cmd_return_sends(cx: &mut TestAppContext) {
    let (rig, view, cx) = with_choices(cx, &["--commands"]);
    focus_composer(&view, cx);
    cx.simulate_input("```py");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("a = 1");
    assert_eq!(
        view.read_with(cx, |view, cx| view.draft(cx)),
        "```py\na = 1"
    );
    assert_eq!(
        set_calls(&rig, "session/prompt").len(),
        1,
        "nothing was sent"
    );
    cx.simulate_keystrokes("cmd-enter");
    settle(cx, "the second turn", |_| {
        set_calls(&rig, "session/prompt").len() == 2
    });
}

#[gpui::test]
fn the_arrows_move_the_highlight_and_return_takes_that_command(cx: &mut TestAppContext) {
    let (_rig, view, cx) = with_choices(cx, &["--commands"]);
    focus_composer(&view, cx);
    cx.simulate_input("/");
    // The window's own commands (/clear, /model, /mode, /plan and /help) stand first, then the agent's.
    cx.simulate_keystrokes("down down down down down down enter");
    assert_eq!(
        view.read_with(cx, |view, cx| view.draft(cx)),
        "/review",
        "the second of the announced"
    );
}

#[gpui::test]
fn clicking_a_command_takes_it(cx: &mut TestAppContext) {
    let (_rig, view, cx) = with_choices(cx, &["--commands"]);
    write(&view, "/", cx);
    scroll_the_slash_menu_down(cx);
    super::lane_tests::click(cx, "slash-row-context");
    assert_eq!(view.read_with(cx, |view, cx| view.draft(cx)), "/context");
}

/// Turns the wheel over the slash menu, as a person does to reach the commands under the last one in view.
fn scroll_the_slash_menu_down(cx: &mut VisualTestContext) {
    let menu = super::lane_tests::bounds(cx, "slash-menu").expect("the menu");
    cx.simulate_event(gpui::ScrollEvent {
        position: menu.center(),
        delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(-300.))),
        ..Default::default()
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

#[gpui::test]
fn a_command_below_the_last_row_in_view_is_reached_by_the_wheel_and_by_the_arrows(
    cx: &mut TestAppContext,
) {
    let (_rig, view, cx) = with_choices(cx, &["--commands"]);
    focus_composer(&view, cx);
    cx.simulate_input("/");
    let menu = super::lane_tests::bounds(cx, "slash-menu").expect("the menu");
    let last = |cx: &mut VisualTestContext| {
        super::lane_tests::bounds(cx, "slash-row-mcp:docs:search").expect("the last row")
    };
    assert!(last(cx).bottom() > menu.bottom(), "it does not fit in view");
    scroll_the_slash_menu_down(cx);
    assert!(
        last(cx).bottom() <= menu.bottom(),
        "the wheel brought it in view"
    );
    // Nine rows: one step up from the first wraps to the last, nine bring it back to the first.
    cx.simulate_keystrokes("up up up up up up up up up");
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let first = super::lane_tests::bounds(cx, "slash-row-clear").expect("the first row");
    assert!(
        first.top() >= menu.top(),
        "the arrows keep the highlighted row in view"
    );
}

#[gpui::test]
fn the_menu_lists_only_the_windows_own_commands_when_the_agent_announced_none(
    cx: &mut TestAppContext,
) {
    let (_rig, view, cx) = with_choices(cx, &[]);
    write(&view, "/", cx);
    assert!(super::lane_tests::bounds(cx, "slash-menu").is_some());
    for name in ["clear", "help"] {
        assert!(super::lane_tests::bounds(cx, &format!("slash-row-{name}")).is_some());
    }
    assert!(super::lane_tests::bounds(cx, "slash-row-compact").is_none());
}

#[gpui::test]
fn slash_plan_puts_the_open_claude_conversation_in_plan_mode_and_sends_nothing(
    cx: &mut TestAppContext,
) {
    let (rig, view, cx) = with_choices(cx, &["--choices"]);
    let before = received(&rig).len();
    say(&view, "/plan", cx);
    settle(cx, "the mode", |cx| {
        view.read_with(cx, |view, _| {
            view.session_choices()
                .modes
                .as_ref()
                .is_some_and(|modes| modes.current == "plan")
        })
    });
    let sent: Vec<_> = received(&rig).into_iter().skip(before).collect();
    assert!(
        sent.iter()
            .all(|message| message["method"] != "session/prompt"),
        "the command is the window's, not a message: {sent:?}"
    );
}

#[gpui::test]
fn slash_plan_on_codex_starts_the_conversation_in_codex_plan_mode(cx: &mut TestAppContext) {
    use super::master_agent::MasterAgent::Codex;
    let (rig, desk, cx) = open(cx, "ask", agent_argv());
    offer_codex(&rig);
    let view = show_master(&desk, cx);
    choose_agent(&desk, cx, Codex);
    say(&view, "/plan", cx);
    cx.run_until_parked();
    say(&view, "settings", cx);
    settle(cx, "codex's turn", |cx| turn_ended(&view, cx));
    view.read_with(cx, |view, _| {
        let last = view
            .thread()
            .entries()
            .last()
            .map(|entry| entry.kind.clone());
        let Some(EntryKind::Assistant(message)) = last else {
            panic!("codex answered with a message, got {last:?}");
        };
        assert!(message.text.contains("mode=plan"), "{}", message.text);
    });
}

#[gpui::test]
fn every_announced_command_runs_through_the_adapter_and_leaves_the_session_usable(
    cx: &mut TestAppContext,
) {
    // One of each kind the adapter announces: no input, an argument, an MCP prompt, a local-only one.
    let (rig, view, cx) = with_choices(cx, &["--commands"]);
    let commands = [
        "/compact",
        "/review 12",
        "/mcp:docs:search tabs",
        "/context",
    ];
    for command in commands {
        say(&view, command, cx);
        settle(cx, command, |cx| turn_ended(&view, cx));
        view.read_with(cx, |view, _| {
            let last = view
                .thread()
                .entries()
                .last()
                .map(|entry| entry.kind.clone());
            let Some(EntryKind::Assistant(message)) = last else {
                panic!("{command} was answered with a message, got {last:?}");
            };
            assert_eq!(message.text, format!("Ran {command}."));
        });
    }
    say(&view, "plain still here", cx);
    settle(cx, "a message after the commands", |cx| {
        turn_ended(&view, cx)
    });
    let sent: Vec<String> = set_calls(&rig, "session/prompt")
        .iter()
        .map(|call| {
            call["params"]["prompt"][0]["text"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assert_eq!(
        &sent[1..],
        [
            "/compact",
            "/review 12",
            "/mcp:docs:search tabs",
            "/context",
            "plain still here"
        ]
    );
}

#[gpui::test]
fn the_undo_toast_stands_clear_of_the_chip_of_the_master_s_composer(cx: &mut TestAppContext) {
    let snapshot = with_proposal();
    let (_rig, desk, cx) = open_over(
        cx,
        (&snapshot, PROPOSAL_CONFIRM, PROPOSAL_DONE),
        "ask",
        agent_argv_with(&["--choices"]),
        None,
    );
    let view = show_master(&desk, cx);
    say(&view, "plain hello", cx);
    settle(cx, "the first turn", |cx| turn_ended(&view, cx));
    show(&view, vec![proposal_call()], cx);
    super::lane_tests::click(cx, &format!("master-approve-{PROPOSAL}"));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let toast = super::lane_tests::bounds(cx, "toast").expect("the held answer's toast");
    let chip = super::lane_tests::bounds(cx, "master-agent-chip").expect("the chip");
    let clearance = chip.left() - toast.right();
    assert!(
        clearance >= gpui::px(4.),
        "the toast {toast:?} is {clearance:?} to the left of {chip:?}"
    );
}

// Commands the adapter does not announce but people type.

/// Writes `text` in the composer and sends it, then lets the window settle.
fn send_typed(view: &Entity<ThreadView>, text: &str, cx: &mut VisualTestContext) {
    say(view, text, cx);
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

/// The prompts the fake agent received, as text.
fn prompts_sent(rig: &Rig) -> Vec<String> {
    let calls = set_calls(rig, "session/prompt");
    let text = |call: &serde_json::Value| {
        call["params"]["prompt"][0]["text"]
            .as_str()
            .map(str::to_string)
    };
    calls.iter().filter_map(text).collect()
}

/// Every filled box of the last frame, as the window painted it.
fn painted(cx: &mut VisualTestContext) -> Vec<gpui::Bounds<gpui::ScaledPixels>> {
    cx.update(|window, _| {
        let quads = window.painted_quads();
        let filled = quads
            .iter()
            .filter(|quad| !quad.background.is_transparent());
        filled.map(|quad| quad.bounds).collect()
    })
}

/// Whether sending `text` opens a menu: a filled box the window did not paint before appears.
fn opens_a_menu(view: &Entity<ThreadView>, text: &str, cx: &mut VisualTestContext) -> bool {
    let before = painted(cx);
    send_typed(view, text, cx);
    painted(cx).iter().any(|quad| !before.contains(quad))
}

#[gpui::test]
fn the_slash_menu_lists_the_windows_own_commands_first_and_narrows_as_the_word_is_typed(
    cx: &mut TestAppContext,
) {
    let (_rig, view, cx) = with_choices(cx, &["--commands", "--choices"]);
    write(&view, "/", cx);
    let top = |cx: &mut VisualTestContext, name: &str| {
        super::lane_tests::bounds(cx, &format!("slash-row-{name}"))
            .unwrap_or_else(|| panic!("{name} is listed"))
            .top()
    };
    let own = ["clear", "model", "mode", "help"].map(|name| top(cx, name));
    let agents = ["compact", "review", "context"].map(|name| top(cx, name));
    assert!(
        own.iter().all(|own| agents.iter().all(|agent| own < agent)),
        "the window's own come first: {own:?} {agents:?}"
    );
    write(&view, "/hel", cx);
    assert!(super::lane_tests::bounds(cx, "slash-row-help").is_some());
    assert!(super::lane_tests::bounds(cx, "slash-row-compact").is_none());
    write(&view, "/help", cx);
    assert!(
        super::lane_tests::bounds(cx, "slash-row-help").is_some(),
        "the menu stays open and narrows to the command typed"
    );
}

#[gpui::test]
fn slash_clear_runs_start_fresh_and_is_not_sent_to_the_agent(cx: &mut TestAppContext) {
    let (rig, desk, cx) = open(cx, "ask", agent_argv());
    let view = show_master(&desk, cx);
    say(&view, "plain hello", cx);
    settle(cx, "the first turn", |cx| turn_ended(&view, cx));
    send_typed(&view, "/clear", cx);
    view.read_with(cx, |view, cx| {
        assert!(view.thread().entries().is_empty(), "an empty conversation");
        assert_eq!(view.draft(cx), "", "the command is spent");
    });
    assert_eq!(prompts_sent(&rig), ["plain hello"], "nothing was sent");
    let page = desk.read_with(cx, |desk, cx| desk.master_page(cx));
    let history = page.read_with(cx, |page, _| page.history_for_test());
    assert_eq!(history.len(), 1, "the conversation is left in History");
    assert_eq!(history[0].title, "plain hello");
}

#[gpui::test]
fn slash_model_and_slash_mode_open_their_chip_s_menu_and_send_nothing(cx: &mut TestAppContext) {
    let (rig, view, cx) = with_choices(cx, &["--choices"]);
    assert!(opens_a_menu(&view, "/model", cx), "the model menu opens");
    cx.simulate_keystrokes("escape");
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(opens_a_menu(&view, "/mode", cx), "the mode menu opens");
    assert_eq!(prompts_sent(&rig), ["plain hello"], "nothing was sent");
}

#[gpui::test]
fn slash_model_opens_the_agent_chip_even_when_the_session_offers_nothing_to_choose(
    cx: &mut TestAppContext,
) {
    let (rig, view, cx) = with_choices(cx, &[]);
    send_typed(&view, "/model", cx);
    assert_eq!(
        prompts_sent(&rig),
        ["plain hello"],
        "the agent chip has the models of both agents: the command is the window's, not text"
    );
}

#[gpui::test]
fn slash_help_lists_the_announced_commands_and_the_windows_own_and_sends_nothing(
    cx: &mut TestAppContext,
) {
    let (rig, view, cx) = with_choices(cx, &["--commands", "--choices"]);
    assert!(
        super::lane_tests::bounds(cx, "slash-menu").is_none(),
        "closed until asked"
    );
    send_typed(&view, "/help", cx);
    for name in [
        "compact",
        "review",
        "mcp:docs:search",
        "context",
        "clear",
        "model",
        "mode",
        "help",
    ] {
        assert!(
            super::lane_tests::bounds(cx, &format!("slash-row-{name}")).is_some(),
            "{name} is listed"
        );
    }
    assert_eq!(prompts_sent(&rig), ["plain hello"], "nothing was sent");
}

#[gpui::test]
fn slash_help_runs_when_the_agent_announces_no_command_and_leaves_no_slash_behind(
    cx: &mut TestAppContext,
) {
    let (rig, view, cx) = with_choices(cx, &[]);
    focus_composer(&view, cx);
    cx.simulate_input("/help");
    cx.simulate_keystrokes("enter");
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert_eq!(
        view.read_with(cx, |view, cx| view.draft(cx)),
        "",
        "the command is spent and no slash is left"
    );
    for name in ["clear", "model", "mode", "help"] {
        assert!(
            super::lane_tests::bounds(cx, &format!("slash-row-{name}")).is_some(),
            "{name} is listed: the agent chip is always there"
        );
    }
    cx.simulate_input("next message");
    assert_eq!(
        view.read_with(cx, |view, cx| view.draft(cx)),
        "next message"
    );
    assert!(
        super::lane_tests::bounds(cx, "slash-menu").is_none(),
        "typing closes the list"
    );
    assert_eq!(prompts_sent(&rig), ["plain hello"], "nothing was sent");
}

#[gpui::test]
fn a_command_of_the_help_list_is_taken_when_it_is_pressed(cx: &mut TestAppContext) {
    let (_rig, view, cx) = with_choices(cx, &["--commands"]);
    send_typed(&view, "/help", cx);
    scroll_the_slash_menu_down(cx);
    super::lane_tests::click(cx, "slash-row-context");
    assert_eq!(view.read_with(cx, |view, cx| view.draft(cx)), "/context");
    assert!(
        super::lane_tests::bounds(cx, "slash-menu").is_none(),
        "the list goes once a command is taken"
    );
}

#[gpui::test]
fn an_unknown_slash_command_is_sent_as_text(cx: &mut TestAppContext) {
    let (rig, view, cx) = with_choices(cx, &["--choices"]);
    say(&view, "/frobnicate now", cx);
    settle(cx, "the turn", |cx| turn_ended(&view, cx));
    say(&view, "/clearly", cx);
    settle(cx, "the second turn", |cx| turn_ended(&view, cx));
    assert_eq!(
        prompts_sent(&rig),
        ["plain hello", "/frobnicate now", "/clearly"]
    );
}

// The master's other agent: Codex.

/// The fake Codex app-server, beside the binaries of this build.
pub(super) fn fake_codex() -> PathBuf {
    let test_binary = std::env::current_exe().expect("the test binary");
    let build = test_binary.parent().and_then(Path::parent);
    let program = build.expect("the build's folder").join("sakura-fake-codex");
    assert!(
        program.exists(),
        "no fake codex at {}: cargo build -p sakura-acp --bin sakura-fake-codex",
        program.display()
    );
    program
}

/// Points the fake engine's spec at a folder whose `codex` is the fake app-server, and gives the master one
/// tool of its own, as the settings the engine writes do.
fn offer_codex(rig: &Rig) {
    let folder = rig.dir.path().join("bin");
    std::fs::create_dir_all(&folder).expect("a folder");
    std::os::unix::fs::symlink(fake_codex(), folder.join("codex")).expect("a link");
    let file = rig.dir.path().join("spec.json");
    let mut spec: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&file).expect("the spec")).expect("json");
    spec["path"] = serde_json::json!(folder);
    spec["env"]["SAKURA_FAKE_WORKSPACE"] = spec["cwd"].clone();
    // Sakura's own Codex home, with its login: Master on Codex never runs in the person's.
    let home = rig.dir.path().join("codex-account");
    std::fs::create_dir_all(&home).expect("a home");
    std::fs::write(home.join("auth.json"), "{}").expect("a login");
    spec["codex_home"] = serde_json::json!(home);
    spec["options"]["settings"]["permissions"] =
        serde_json::json!({ "allow": ["mcp__sakura-master__lane_open"] });
    std::fs::write(file, spec.to_string()).expect("the spec");
}

/// Presses `agent` in the switch at the top of the Master page: a new conversation on that agent.
fn choose_agent(
    desk: &Entity<Desk>,
    cx: &mut VisualTestContext,
    agent: super::master_agent::MasterAgent,
) {
    let page = desk.read_with(cx, |desk, cx| desk.master_page(cx));
    page.update(cx, |page, cx| {
        let entries = page.account_entries(cx);
        let first = entries.iter().find(|entry| entry.agent == agent);
        let id = first.map(|entry| entry.id.clone()).unwrap_or_default();
        page.press_account(agent, &id, cx)
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

fn is_agent(
    desk: &Entity<Desk>,
    agent: super::master_agent::MasterAgent,
    cx: &mut VisualTestContext,
) -> bool {
    desk.read_with(cx, |desk, cx| {
        desk.master_page(cx).read(cx).agent() == agent
    })
}

#[gpui::test]
fn choosing_a_codex_model_in_the_chip_runs_the_master_on_codex_with_its_own_conversation(
    cx: &mut TestAppContext,
) {
    use super::master_agent::MasterAgent::Codex;
    let (rig, desk, cx) = open(cx, "ask", agent_argv());
    offer_codex(&rig);
    let view = show_master(&desk, cx);
    assert!(is_agent(
        &desk,
        super::master_agent::MasterAgent::Claude,
        cx
    ));
    choose_agent(&desk, cx, Codex);
    assert!(is_agent(&desk, Codex, cx), "the pick switched the agent");
    view.read_with(cx, |view, _| {
        assert!(
            view.thread().entries().is_empty(),
            "codex has said nothing yet"
        )
    });
    say(&view, "plain", cx);
    settle(cx, "codex's turn", |cx| turn_ended(&view, cx));
    view.read_with(cx, |view, _| {
        let last = view
            .thread()
            .entries()
            .last()
            .map(|entry| entry.kind.clone());
        let Some(EntryKind::Assistant(message)) = last else {
            panic!("codex answered with a message, got {last:?}");
        };
        assert_eq!(message.text, "Hello there.");
    });
    assert_eq!(
        rig.sessions.stored_any("@master-codex").as_deref(),
        Some("thr-1")
    );
    assert_eq!(
        rig.sessions.stored_any("@master"),
        None,
        "claude's session is its own and was not made"
    );
}

#[gpui::test]
fn the_preset_drives_the_first_chip_and_session_even_with_a_cached_haiku(cx: &mut TestAppContext) {
    check_first_master_model(cx, None, None, "Opus 5.5 Extra high", "opus", Some("xhigh"));
}

#[gpui::test]
fn the_settings_model_overrides_the_preset_in_the_first_chip_and_session(cx: &mut TestAppContext) {
    check_first_master_model(cx, Some("haiku"), None, "Haiku 5.5", "haiku", None);
}

#[gpui::test]
fn the_chip_pick_overrides_settings_in_the_first_chip_and_session(cx: &mut TestAppContext) {
    check_first_master_model(
        cx,
        Some("haiku"),
        Some("sonnet"),
        "Sonnet 5.5",
        "sonnet",
        None,
    );
}

#[gpui::test]
fn an_empty_catalogue_effort_accepts_the_session_default_with_the_preset(cx: &mut TestAppContext) {
    check_empty_catalogue_effort(cx, None, None, "default", "Haiku");
}

#[gpui::test]
fn an_empty_catalogue_effort_accepts_the_session_effort_with_a_chip_pick(cx: &mut TestAppContext) {
    check_empty_catalogue_effort(cx, None, Some("sonnet"), "high", "Sonnet");
}

#[gpui::test]
fn an_empty_catalogue_effort_accepts_the_session_effort_with_a_settings_pick(
    cx: &mut TestAppContext,
) {
    check_empty_catalogue_effort(cx, Some("sonnet"), None, "high", "Sonnet");
}

fn check_empty_catalogue_effort(
    cx: &mut TestAppContext,
    settings: Option<&str>,
    picked: Option<&str>,
    reported: &str,
    name: &str,
) {
    let kept = Preferences {
        new_master_model: settings.map(str::to_string),
        ..Preferences::default()
    };
    let accounts = crate::ui::engine_fixture_tests::full();
    let argv = agent_argv_with(&["--choices", "--reported-effort", reported]);
    let rig = master_rig(cx, (&accounts, "{}", "{}"), "ask", argv);
    rig.say_rules(&rules_with_model_defaults(""));
    let (desk, cx) = build_desk(cx, &rig, None, kept);
    wait_for_update(cx, &rig.facts, |facts, _| facts.reads_ended() > 0);
    rig.facts.read_with(cx, |facts, _| {
        assert!(facts.snapshot().is_some(), "{:?}", facts.problem());
    });
    let view = show_master(&desk, cx);
    if let Some(picked) = picked {
        view.update(cx, |view, cx| view.pick_model(picked, cx));
    }
    settle(cx, "the model preview and catalogue", |cx| {
        view.read_with(cx, |view, _| {
            view.agent_menu().is_some_and(|menu| {
                !menu.name.is_empty()
                    && menu
                        .groups
                        .iter()
                        .flat_map(|group| &group.entries)
                        .any(|entry| entry.name == "Opus 5.5")
            })
        })
    });
    let before = view.read_with(cx, |view, _| view.agent_menu().unwrap().name);
    say(&view, "plain", cx);
    wait_for_update(cx, &view, |view, _| {
        matches!(
            view.phase(),
            Phase::Ended(_) | Phase::SignIn | Phase::NoAccount
        ) || (*view.phase() == Phase::Ready
            && view.thread().turn() == sakura_acp::thread::Turn::Idle
            && !view.thread().entries().is_empty())
    });
    assert_eq!(
        view.read_with(cx, |view, _| view.phase().clone()),
        Phase::Ready
    );
    let new = set_calls(&rig, "session/new");
    let options = &new[0]["params"]["_meta"]["claudeCode"]["options"];
    assert_eq!(
        options["model"].as_str(),
        Some(name.to_lowercase().as_str())
    );
    assert!(options.get("effort").is_none());
    assert_eq!(set_calls(&rig, "session/prompt").len(), 1);
    let after = view.read_with(cx, |view, _| view.agent_menu().unwrap().name);
    assert_eq!(before, format!("{name} 5.5"));
    assert_eq!(after, name);
    let checked_efforts = |view: &Entity<ThreadView>, cx: &mut VisualTestContext| {
        view.read_with(cx, |view, _| {
            view.agent_menu()
                .unwrap()
                .groups
                .into_iter()
                .flat_map(|group| group.entries)
                .filter_map(|entry| match entry.pick {
                    MenuPick::Effort(id) if entry.is_on => Some(id),
                    _ => None,
                })
                .collect::<Vec<_>>()
        })
    };
    assert_eq!(checked_efforts(&view, cx), ["default"]);
    if reported == "high" {
        view.update(cx, |view, cx| {
            view.pick_entry(&MenuPick::Effort("high".into()), cx)
        });
        settle(cx, "the explicit effort", |cx| {
            checked_efforts(&view, cx) == ["high"]
        });
    }
    assert_eq!(
        view.read_with(cx, |view, _| view
            .session_choices()
            .efforts
            .as_ref()
            .unwrap()
            .current
            .clone()),
        reported
    );
}

#[gpui::test]
fn a_failed_first_start_keeps_the_chip_pick_and_accepts_a_new_one(cx: &mut TestAppContext) {
    let (rig, desk, cx) = open(cx, "ask", agent_argv_with(&["--fail-set-mode"]));
    rig.say_rules(&rules_with_claude_models());
    let view = show_master(&desk, cx);
    settle(cx, "the model catalogue", |cx| {
        menu_names(&view, cx)
            .iter()
            .any(|name| name == "Sonnet 5.5")
    });
    view.update(cx, |view, cx| {
        view.pick_model("sonnet", cx);
        view.pick_entry(&MenuPick::Effort("low".into()), cx);
    });
    say(&view, "plain", cx);
    settle(cx, "the failed start", |cx| {
        view.read_with(cx, |view, _| matches!(view.phase(), Phase::Ended(_)))
    });
    assert_eq!(
        view.read_with(cx, |view, _| view.agent_menu().unwrap().name),
        "Sonnet 5.5 Low"
    );
    view.update(cx, |view, cx| view.pick_model("haiku", cx));
    assert_eq!(
        view.read_with(cx, |view, _| view.agent_menu().unwrap().name),
        "Haiku 5.5"
    );
}

#[gpui::test]
fn the_first_message_is_withheld_if_the_adapter_cannot_run_the_shown_model(
    cx: &mut TestAppContext,
) {
    let (rig, desk, cx) = open(cx, "ask", agent_argv_with(&["--choices"]));
    let view = show_master(&desk, cx);
    view.update(cx, |view, cx| view.pick_model("opus", cx));
    say(&view, "plain", cx);
    settle(cx, "the attempted turn", |cx| {
        view.read_with(cx, |view, _| matches!(view.phase(), Phase::Ended(_)))
            || turn_ended(&view, cx)
    });
    assert_eq!(set_calls(&rig, "session/new").len(), 1);
    assert!(set_calls(&rig, "session/prompt").is_empty());
}

fn check_first_master_model(
    cx: &mut TestAppContext,
    settings: Option<&str>,
    picked: Option<&str>,
    label: &str,
    model: &str,
    effort: Option<&str>,
) {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let kept = Preferences {
        master_model: Some("haiku".into()),
        new_master_model: settings.map(str::to_string),
        master_models: [("claude".into(), preset_model_catalogue())].into(),
        ..Preferences::default()
    };
    let accounts = crate::ui::engine_fixture_tests::full();
    let argv = agent_argv();
    let (rig, desk, cx) = open_kept(
        cx,
        (&accounts, "{}", "{}"),
        "ask",
        argv.clone(),
        (None, kept),
    );
    let mut preset_spec: serde_json::Value = serde_json::from_str(&spec(&rig, argv)).unwrap();
    preset_spec["options"]["model"] = "opus".into();
    preset_spec["options"]["effort"] = "xhigh".into();
    std::fs::write(rig.dir.path().join("spec.json"), preset_spec.to_string()).unwrap();
    let view = show_master(&desk, cx);
    if let Some(picked) = picked {
        view.update(cx, |view, cx| view.pick_model(picked, cx));
    }
    settle(cx, label, |cx| {
        view.read_with(cx, |view, _| view.agent_menu().unwrap().name == label)
    });
    assert!(
        received(&rig).is_empty(),
        "looking at the chip starts no adapter"
    );
    say(&view, "plain", cx);
    settle(cx, "the first turn", |cx| turn_ended(&view, cx));
    let opened = set_calls(&rig, "session/new");
    let options = &opened[0]["params"]["_meta"]["claudeCode"]["options"];
    assert_eq!(options["model"].as_str(), Some(model));
    assert_eq!(options["effort"].as_str(), effort);
    assert_eq!(set_calls(&rig, "session/prompt").len(), 1);
    let running = view.read_with(cx, |view, _| view.agent_menu().unwrap().name);
    assert_eq!(running, label);
}

fn preset_model_catalogue() -> serde_json::Value {
    let models = [
        ("haiku", "Haiku 5.5"),
        ("sonnet", "Sonnet 5.5"),
        ("opus", "Opus 5.5"),
    ];
    let choices = models.map(|(id, name)| sakura_acp::model::Choice {
        id: id.into(),
        name: name.into(),
        ..Default::default()
    });
    super::master_agent::models_to_json(&choices)
}

#[gpui::test]
fn pressing_the_other_agent_opens_a_new_conversation_and_the_old_one_stays_whole_in_history(
    cx: &mut TestAppContext,
) {
    use super::master_agent::MasterAgent::{Claude, Codex};
    let (rig, desk, cx) = open(cx, "ask", agent_argv());
    offer_codex(&rig);
    let view = show_master(&desk, cx);
    say(&view, "plain hello", cx);
    settle(cx, "the first turn", |cx| turn_ended(&view, cx));
    choose_agent(&desk, cx, Codex);
    assert!(is_agent(&desk, Codex, cx));
    view.read_with(cx, |view, _| {
        assert!(view.thread().entries().is_empty(), "a new conversation")
    });
    choose_agent(&desk, cx, Claude);
    let past = desk.read_with(cx, |desk, cx| {
        desk.master_page(cx).read(cx).history_for_test()
    });
    assert_eq!(past.len(), 1, "the Claude conversation is in History");
}

#[gpui::test]
fn a_second_message_to_codex_is_sent_and_answered_in_the_same_thread(cx: &mut TestAppContext) {
    use super::master_agent::MasterAgent::Codex;
    let (rig, desk, cx) = open(cx, "ask", agent_argv());
    offer_codex(&rig);
    let view = show_master(&desk, cx);
    choose_agent(&desk, cx, Codex);
    say(&view, "plain", cx);
    settle(cx, "the first turn", |cx| turn_ended(&view, cx));
    say(&view, "plain", cx);
    settle(cx, "the second turn", |cx| {
        view.read_with(cx, |view, _| {
            let users = view
                .thread()
                .entries()
                .iter()
                .filter(|entry| matches!(entry.kind, EntryKind::User(_)))
                .count();
            users == 2 && *view.phase() == Phase::Ready
        })
    });
    assert_eq!(
        rig.sessions.stored_any("@master-codex").as_deref(),
        Some("thr-1"),
        "the same thread"
    );
}

#[gpui::test]
fn a_model_and_an_effort_chosen_in_a_started_codex_conversation_keep_its_thread_and_history(
    cx: &mut TestAppContext,
) {
    use super::master_agent::MasterAgent::Codex;
    let (rig, desk, cx) = open(cx, "ask", agent_argv());
    offer_codex(&rig);
    let view = show_master(&desk, cx);
    choose_agent(&desk, cx, Codex);
    say(&view, "plain", cx);
    settle(cx, "the first turn", |cx| turn_ended(&view, cx));
    let session = rig.sessions.stored_any("@master-codex");
    let low = MenuPick::Effort("low".into());
    view.update(cx, |view, cx| view.pick_entry(&low, cx));
    cx.run_until_parked();
    say(&view, "settings", cx);
    settle(cx, "the second turn", |cx| turn_ended(&view, cx));
    assert_eq!(rig.sessions.stored_any("@master-codex"), session);
    view.read_with(cx, |view, _| {
        let last = view
            .thread()
            .entries()
            .last()
            .map(|entry| entry.kind.clone());
        let Some(EntryKind::Assistant(message)) = last else {
            panic!("codex answered with a message, got {last:?}");
        };
        assert!(message.text.contains("effort=low"), "{}", message.text);
        assert_eq!(view.thread().entries().len(), 4, "the history stays whole");
    });
}

#[gpui::test]
fn a_model_chosen_in_a_started_claude_conversation_keeps_its_session_and_history(
    cx: &mut TestAppContext,
) {
    let (rig, view, cx) = with_choices(cx, &["--choices"]);
    let session = rig.sessions.stored_any("@master");
    assert!(session.is_some());
    let before = view.read_with(cx, |view, _| view.thread().entries().len());
    let sonnet = MenuPick::Model("sonnet".into());
    view.update(cx, |view, cx| view.pick_entry(&sonnet, cx));
    settle(cx, "the model", |cx| {
        view.read_with(cx, |view, _| {
            view.session_choices()
                .models
                .as_ref()
                .is_some_and(|models| models.current == "sonnet")
        })
    });
    assert_eq!(rig.sessions.stored_any("@master"), session);
    assert!(
        set_calls(&rig, "session/new").len() <= 1,
        "no second session"
    );
    view.read_with(cx, |view, _| {
        assert_eq!(
            view.thread().entries().len(),
            before,
            "the history stays whole"
        )
    });
}

#[gpui::test]
fn codex_answers_a_conversation_opened_with_nothing_picked_and_no_model_is_named_to_it(
    cx: &mut TestAppContext,
) {
    use super::master_agent::MasterAgent::Codex;
    let (rig, desk, cx) = open(cx, "ask", agent_argv());
    offer_codex(&rig);
    let view = show_master(&desk, cx);
    choose_agent(&desk, cx, Codex);
    say(&view, "settings", cx);
    settle(cx, "codex's turn", |cx| turn_ended(&view, cx));
    view.read_with(cx, |view, _| {
        let last = view
            .thread()
            .entries()
            .last()
            .map(|entry| entry.kind.clone());
        let Some(EntryKind::Assistant(message)) = last else {
            panic!("codex answered with a message, got {last:?}");
        };
        assert!(message.text.contains("model=none"), "{}", message.text);
    });
}

#[gpui::test]
fn the_agent_chip_of_a_started_session_sets_its_model_and_its_mode(cx: &mut TestAppContext) {
    let (rig, view, cx) = with_choices(cx, &["--choices"]);
    let sonnet = MenuPick::Model("sonnet".into());
    view.update(cx, |view, cx| view.pick_entry(&sonnet, cx));
    settle(cx, "the model", |cx| {
        view.read_with(cx, |view, _| {
            view.session_choices()
                .models
                .as_ref()
                .is_some_and(|models| models.current == "sonnet")
        })
    });
    let plan = MenuPick::Mode("plan".into());
    view.update(cx, |view, cx| view.pick_entry(&plan, cx));
    settle(cx, "the mode", |cx| {
        view.read_with(cx, |view, _| {
            view.session_choices()
                .modes
                .as_ref()
                .is_some_and(|modes| modes.current == "plan")
        })
    });
    let name = view.read_with(cx, |view, _| {
        view.agent_menu().expect("a menu").name.to_string()
    });
    assert_eq!(name, "Sonnet, plan");
    assert_eq!(set_calls(&rig, "session/set_config_option").len(), 1);
}

#[gpui::test]
fn a_command_codex_asks_to_run_is_put_to_the_engine_and_then_to_the_person(
    cx: &mut TestAppContext,
) {
    use super::master_agent::MasterAgent::Codex;
    let (rig, desk, cx) = open(cx, "ask", agent_argv());
    offer_codex(&rig);
    let view = show_master(&desk, cx);
    choose_agent(&desk, cx, Codex);
    say(&view, "command", cx);
    settle(cx, "the card", |cx| {
        view.read_with(cx, |view, _| view.waits_for_person())
    });
    let requests: Vec<_> = std::fs::read_dir(rig.dir.path())
        .expect("the folder")
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("request-"))
        .collect();
    assert_eq!(requests.len(), 1, "the engine decided the request once");
    let asked: serde_json::Value =
        serde_json::from_slice(&std::fs::read(requests[0].path()).expect("the request"))
            .expect("json");
    assert_eq!(
        asked["params"]["toolCall"]["rawInput"]["command"],
        "git status"
    );
    assert_eq!(
        asked["params"]["toolCall"]["rawInput"]["cwd"],
        serde_json::json!(rig.dir.path().join("master").canonicalize().unwrap())
    );
}

#[gpui::test]
fn a_command_the_engine_denies_never_reaches_the_person_on_codex(cx: &mut TestAppContext) {
    use super::master_agent::MasterAgent::Codex;
    let (rig, desk, cx) = open(cx, "deny", agent_argv());
    offer_codex(&rig);
    let view = show_master(&desk, cx);
    choose_agent(&desk, cx, Codex);
    say(&view, "command", cx);
    settle(cx, "the turn", |cx| turn_ended(&view, cx));
    view.read_with(cx, |view, _| {
        assert!(!view.waits_for_person());
        let last = view
            .thread()
            .entries()
            .last()
            .map(|entry| entry.kind.clone());
        let Some(EntryKind::Assistant(message)) = last else {
            panic!("codex went on, got {last:?}");
        };
        assert_eq!(message.text, "command decline");
    });
}

// The composer's tools stay in sight.

/// Every control of the master composer's tools row, left to right.
const COMPOSER_TOOLS: [&str; 6] = [
    "master-attach",
    "master-project-chip",
    "master-model-chip",
    "master-mode-chip",
    "master-dictate",
    "thread-send",
];

#[test]
fn the_composer_tools_stay_inside_its_box_in_a_narrow_window_and_never_overlap() {
    super::real_text::run(|cx| {
        let (_rig, _view, cx) = with_choices(cx, &["--choices"]);
        for width in [1100., 800., 640.] {
            cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(700.)));
            cx.update(|window, cx| window.draw(cx).clear(cx));
            let inside = super::lane_tests::bounds(cx, "thread-composer-box").expect("the box");
            let mut right_of_last = inside.left();
            for name in COMPOSER_TOOLS {
                let Some(tool) = super::lane_tests::bounds(cx, name) else {
                    continue;
                };
                assert!(
                    tool.left() >= right_of_last && tool.right() <= inside.right(),
                    "{name} at {width}: {tool:?} is not between {right_of_last:?} and the box {inside:?}"
                );
                right_of_last = tool.right();
            }
            for name in ["master-dictate", "thread-send"] {
                let tool = super::lane_tests::bounds(cx, name).expect("always drawn");
                assert!(tool.size.width >= gpui::px(24.), "{name} keeps its size");
            }
        }
    });
}

#[test]
fn nothing_in_the_composer_touches_its_edges_the_same_room_all_round() {
    super::real_text::run(|cx| {
        let (_rig, _view, cx) = with_choices(cx, &["--choices"]);
        let inside = super::lane_tests::bounds(cx, "thread-composer-box").expect("the box");
        let room = crate::theme::thread::CARD_PADDING;
        let mut bounds = |name| super::lane_tests::bounds(cx, name).expect(name);
        let field = bounds("thread-composer");
        let (attach, send) = (bounds("master-attach"), bounds("thread-send"));
        let insets = [
            ("field, left", field.left() - inside.left()),
            ("field, top", field.top() - inside.top()),
            ("attach, left", attach.left() - inside.left()),
            ("attach, bottom", inside.bottom() - attach.bottom()),
            ("send, right", inside.right() - send.right()),
            ("send, bottom", inside.bottom() - send.bottom()),
        ];
        for (what, inset) in insets {
            assert!(
                inset >= room,
                "{what}: {inset:?} from the edge, least {room:?}"
            );
        }
    });
}

#[gpui::test]
fn the_words_of_the_masters_chat_are_selected_with_the_mouse_and_copied_with_cmd_c(
    cx: &mut TestAppContext,
) {
    let (_rig, _view, cx) = with_choices(cx, &["--choices"]);
    let answer = cx
        .debug_bounds("thread-answer-1-0")
        .expect("the agent's words");
    let message = cx
        .debug_bounds("thread-prompt")
        .expect("the person's words");
    for (what, block) in [("the agent's", answer), ("the person's", message)] {
        cx.simulate_mouse_down(
            gpui::point(block.left() + gpui::px(24.), block.top() + gpui::px(10.)),
            gpui::MouseButton::Left,
            gpui::Modifiers::none(),
        );
        for step in 1..=8 {
            let x = block.left()
                + gpui::px(24.)
                + (block.size.width - gpui::px(26.)) * (step as f32 / 8.);
            cx.simulate_mouse_move(
                gpui::point(x, block.bottom() - gpui::px(4.)),
                Some(gpui::MouseButton::Left),
                gpui::Modifiers::none(),
            );
        }
        cx.simulate_mouse_up(
            gpui::point(block.right() - gpui::px(2.), block.bottom() - gpui::px(4.)),
            gpui::MouseButton::Left,
            gpui::Modifiers::none(),
        );
        cx.simulate_keystrokes("cmd-c");
        let copied = cx
            .read_from_clipboard()
            .and_then(|item| item.text())
            .unwrap_or_default();
        assert!(!copied.is_empty(), "{what} words were copied");
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(String::new()));
    }
}

#[gpui::test]
fn codex_never_starts_from_an_unreadable_spec(cx: &mut TestAppContext) {
    let (rig, desk, cx) = open(cx, "ask", agent_argv());
    offer_codex(&rig);
    std::fs::write(rig.dir.path().join("spec.json"), "{").unwrap();
    let view = show_master(&desk, cx);
    choose_agent(&desk, cx, super::master_agent::MasterAgent::Codex);
    say(&view, "plain", cx);
    settle(cx, "the unreadable spec", |cx| {
        view.read_with(cx, |view, _| matches!(view.phase(), Phase::Ended(_)))
    });
    assert!(received(&rig).is_empty());
}

#[gpui::test]
fn codex_never_starts_with_the_login_of_a_different_account(cx: &mut TestAppContext) {
    use super::master_agent::MasterAgent::Codex;

    let all = crate::ui::engine_fixture_tests::full();
    let (rig, desk, cx) = open_over(cx, (&all, "{}", "{}"), "ask", agent_argv(), None);
    offer_codex(&rig);
    let file = rig.dir.path().join("spec.json");
    let mut spec: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    spec["account"] = "different-codex-account".into();
    spec["account_provider"] = "codex".into();
    std::fs::write(file, spec.to_string()).unwrap();
    let view = show_master(&desk, cx);
    choose_agent(&desk, cx, Codex);
    say(&view, "plain", cx);
    settle(cx, "the account check", |cx| {
        view.read_with(cx, |view, _| {
            matches!(view.phase(), Phase::Ended(_) | Phase::Ready)
        })
    });
    assert!(said_in_the_tab(&view, cx, "different account"));
    assert!(received(&rig).is_empty());
}

#[gpui::test]
fn fix25_the_codex_chip_does_not_inherit_the_claude_model(cx: &mut TestAppContext) {
    let all = crate::ui::engine_fixture_tests::full();
    let kept = Preferences {
        master_account: "codex".into(),
        master_model: Some("opus".into()),
        ..Preferences::default()
    };
    let (_rig, desk, cx) = open_kept(cx, (&all, "{}", "{}"), "ask", agent_argv(), (None, kept));
    let view = show_master(&desk, cx);
    assert_eq!(chip_label_of(&desk, cx).as_deref(), Some("Codex"));
    assert!(is_agent(&desk, super::master_agent::MasterAgent::Codex, cx));
    let menu = view.read_with(cx, |view, _| view.agent_menu().expect("a menu"));
    assert!(
        menu.name.is_empty(),
        "expected the placeholder, got {}",
        menu.name
    );
    assert!(drawn(cx, "master-agent-chip"));
}

#[gpui::test]
fn fix25_codex_without_a_login_of_sakura_s_own_says_so_and_the_menu_offers_the_login(
    cx: &mut TestAppContext,
) {
    use super::master_agent::MasterAgent::Codex;
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let (rig, desk, cx) = open(cx, "ask", agent_argv());
    // The fake engine's spec has Codex, but Sakura's home has no login.
    let folder = rig.dir.path().join("bin");
    std::fs::create_dir_all(&folder).expect("a folder");
    std::os::unix::fs::symlink(fake_codex(), folder.join("codex")).expect("a link");
    let file = rig.dir.path().join("spec.json");
    let mut spec: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&file).expect("the spec")).expect("json");
    spec["path"] = serde_json::json!(folder);
    spec["env"]["SAKURA_ENGINE_HOME"] = serde_json::json!(rig.dir.path());
    std::fs::write(file, spec.to_string()).expect("the spec");
    let view = show_master(&desk, cx);
    desk.read_with(cx, |desk, cx| {
        desk.master_page(cx)
            .read(cx)
            .set_login_check_for_test(|| false)
    });
    choose_agent(&desk, cx, Codex);
    let menu = view.read_with(cx, |view, _| view.agent_menu().expect("a menu"));
    let first = &menu.groups[0].entries[0];
    assert_eq!(first.name, "Log in to Codex for Sakura");
    say(&view, "plain", cx);
    settle(cx, "the startup outcome", |cx| {
        view.read_with(cx, |view, _| {
            matches!(view.phase(), Phase::SignIn | Phase::Ended(_))
        })
    });
    assert_eq!(
        view.read_with(cx, |view, _| view.phase().clone()),
        Phase::SignIn
    );
    assert!(cx.debug_bounds("thread-codex-login").is_some());
}

/// The fake engine's spec with Codex on the fake app-server and no login in Sakura's own home, which is `home`.
/// `login` is how the fake's login goes (`SAKURA_FAKE_LOGIN`).
fn offer_codex_without_login(rig: &Rig, login: &str) -> PathBuf {
    let folder = rig.dir.path().join("bin");
    std::fs::create_dir_all(&folder).expect("a folder");
    std::os::unix::fs::symlink(fake_codex(), folder.join("codex")).expect("a link");
    let file = rig.dir.path().join("spec.json");
    let mut spec: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&file).expect("the spec")).expect("json");
    spec["path"] = serde_json::json!(folder);
    spec["env"]["SAKURA_ENGINE_HOME"] = serde_json::json!(rig.dir.path());
    spec["env"]["SAKURA_FAKE_LOGIN"] = serde_json::json!(login);
    std::fs::write(file, spec.to_string()).expect("the spec");
    rig.dir.path().join(super::master_agent::CODEX_HOME_FOLDER)
}

/// Opens the Master on Codex with no login, the menu told by the file of the login, and presses what the
/// menu's login entry says.
fn start_the_codex_login<'a>(
    cx: &'a mut TestAppContext,
    login: &str,
) -> (Rig, Entity<ThreadView>, PathBuf, &'a mut VisualTestContext) {
    use super::master_agent::MasterAgent::Codex;
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let (rig, desk, cx) = open(cx, "ask", agent_argv());
    let home = offer_codex_without_login(&rig, login);
    let view = show_master(&desk, cx);
    let kept = home.clone();
    desk.read_with(cx, |desk, cx| {
        desk.master_page(cx)
            .read(cx)
            .set_login_check_for_test(move || kept.join("auth.json").is_file())
    });
    choose_agent(&desk, cx, Codex);
    say(&view, "plain", cx);
    settle(cx, "the sign-in notice", |cx| {
        view.read_with(cx, |view, _| *view.phase() == Phase::SignIn)
    });
    super::lane_tests::click(cx, "thread-codex-login");
    (rig, view, home, cx)
}

fn press_the_login_entry(view: &Entity<ThreadView>, cx: &mut VisualTestContext) {
    let pick = view.read_with(cx, |view, _| {
        view.agent_menu().expect("a menu").groups[0].entries[0]
            .pick
            .clone()
    });
    view.update(cx, |view, cx| view.pick_entry(&pick, cx));
}

fn said_in_the_tab(view: &Entity<ThreadView>, cx: &mut VisualTestContext, words: &str) -> bool {
    view.read_with(cx, |view, _| {
        view.thread().entries().iter().any(|entry| {
            matches!(&entry.kind, EntryKind::Problem(problem) if problem.detail.contains(words))
        })
    })
}

#[gpui::test]
fn the_codex_login_opens_the_browser_and_when_it_completes_the_menu_and_the_tab_follow(
    cx: &mut TestAppContext,
) {
    let (_rig, view, home, cx) = start_the_codex_login(cx, "success");
    settle(cx, "the browser", |cx| cx.opened_url().is_some());
    assert_eq!(
        cx.opened_url().as_deref(),
        Some("https://auth.example.test/login?state=abc")
    );
    settle(cx, "the tab running Codex", |cx| {
        view.read_with(cx, |view, _| *view.phase() == Phase::Ready)
    });
    assert!(home.join("auth.json").is_file());
    let menu = view.read_with(cx, |view, _| view.agent_menu().expect("a menu"));
    assert_ne!(
        menu.groups[0].entries[0].name, "Log in to Codex for Sakura",
        "the menu offers no login once there is one"
    );
}

#[gpui::test]
fn a_codex_login_that_does_not_succeed_is_said_in_the_tab_with_the_servers_reason(
    cx: &mut TestAppContext,
) {
    let (_rig, view, home, cx) = start_the_codex_login(cx, "fail");
    settle(cx, "the words", |cx| {
        said_in_the_tab(&view, cx, "the browser window was closed")
    });
    assert!(said_in_the_tab(&view, cx, "did not finish"));
    assert!(!home.join("auth.json").exists());
}

#[gpui::test]
fn a_codex_login_the_person_cancels_from_the_menu_is_said_cancelled_in_the_tab(
    cx: &mut TestAppContext,
) {
    let (_rig, view, _home, cx) = start_the_codex_login(cx, "hold");
    settle(cx, "the browser", |cx| cx.opened_url().is_some());
    settle(cx, "the menu offering to cancel", |cx| {
        let menu = view.read_with(cx, |view, _| view.agent_menu().expect("a menu"));
        menu.groups[0].entries[0].name == "Cancel the Codex login"
    });
    press_the_login_entry(&view, cx);
    settle(cx, "the words", |cx| {
        said_in_the_tab(&view, cx, "was cancelled")
    });
}

#[gpui::test]
fn master_login_attempt_reserves_its_slot_before_the_spec_arrives(cx: &mut TestAppContext) {
    use super::master_agent::MasterAgent::Codex;
    let (rig, desk, cx) = open(cx, "ask", agent_argv());
    offer_codex_without_login(&rig, "hold");
    let view = show_master(&desk, cx);
    let page = desk.read_with(cx, |desk, cx| desk.master_page(cx));
    page.update(cx, |page, cx| {
        page.set_login_check_for_test(|| false);
        page.press_account(Codex, "codex", cx);
    });
    view.update(cx, |_, cx| {
        cx.emit(super::thread::ThreadViewEvent::CodexLoginRequested);
        cx.emit(super::thread::ThreadViewEvent::CodexLoginRequested);
    });
    settle(cx, "the browser", |cx| cx.opened_url().is_some());
    let until = std::time::Instant::now() + std::time::Duration::from_millis(500);
    settle(cx, "the login requests", |_| {
        std::time::Instant::now() >= until
    });
    assert_eq!(
        engine_calls(&rig)
            .iter()
            .filter(|call| *call == "master spec --json --account codex")
            .count(),
        1
    );
}

#[gpui::test]
fn master_login_attempt_failure_does_not_reach_the_replacement_conversation(
    cx: &mut TestAppContext,
) {
    use super::master_agent::MasterAgent::{Claude, Codex};
    let (rig, desk, cx) = open(cx, "ask", agent_argv());
    offer_codex_without_login(&rig, "late-fail");
    let view = show_master(&desk, cx);
    let page = desk.read_with(cx, |desk, cx| desk.master_page(cx));
    page.update(cx, |page, cx| {
        page.set_login_check_for_test(|| false);
        page.press_account(Codex, "codex", cx);
    });
    view.update(cx, |_, cx| {
        cx.emit(super::thread::ThreadViewEvent::CodexLoginRequested)
    });
    settle(cx, "the browser", |cx| cx.opened_url().is_some());
    page.update(cx, |page, cx| page.press_account(Claude, "personal", cx));
    let until = std::time::Instant::now() + std::time::Duration::from_millis(700);
    settle(cx, "the old login ends", |_| {
        std::time::Instant::now() >= until
    });
    assert_eq!(
        view.read_with(cx, |view, _| view.phase().clone()),
        Phase::Idle
    );
    assert!(!said_in_the_tab(&view, cx, "the browser window was closed"));
}

fn chip_label_of(desk: &Entity<Desk>, cx: &mut VisualTestContext) -> Option<String> {
    desk.read_with(cx, |desk, cx| {
        desk.master_page(cx).read(cx).account_chip_label(cx)
    })
}

#[gpui::test]
fn the_account_chip_says_the_saved_name_of_the_conversation_s_account_while_the_accounts_are_not_listed(
    cx: &mut TestAppContext,
) {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let kept = Preferences {
        master_account: "work".into(),
        master_account_names: [("work".to_string(), "Lavoro".to_string())].into(),
        ..Preferences::default()
    };
    let (_rig, desk, cx) = open_kept(
        cx,
        (SNAPSHOT, "{}", "{}"),
        "ask",
        agent_argv(),
        (None, kept),
    );
    let _view = show_master(&desk, cx);
    assert_eq!(
        chip_label_of(&desk, cx).as_deref(),
        Some("Lavoro"),
        "never the program's name"
    );
}

#[gpui::test]
fn the_account_chip_offers_to_add_one_and_never_names_a_program_when_the_engine_lists_no_account(
    cx: &mut TestAppContext,
) {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let (_rig, desk, cx) = open(cx, "ask", agent_argv());
    let _view = show_master(&desk, cx);
    assert_eq!(chip_label_of(&desk, cx).as_deref(), Some("Add account"));
}

#[gpui::test]
fn the_account_chip_says_the_engine_s_name_of_the_account_and_the_names_are_kept(
    cx: &mut TestAppContext,
) {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let all = crate::ui::engine_fixture_tests::full();
    let (_rig, desk, cx) = open_over(cx, (&all, "{}", "{}"), "ask", agent_argv(), None);
    let _view = show_master(&desk, cx);
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert_eq!(chip_label_of(&desk, cx).as_deref(), Some("Personal"));
    let kept =
        cx.update(|_, cx| Preferences::try_current(cx).map(|kept| kept.master_account_names));
    let kept = kept.unwrap_or_default();
    assert_eq!(
        kept.values().map(String::as_str).collect::<Vec<_>>().len(),
        3
    );
    assert!(kept.values().any(|name| name == "Work"));
}

/// The desk before the engine has said anything: the snapshot is not readable yet.
fn open_before_the_engine_answers(
    cx: &mut TestAppContext,
    kept: Preferences,
) -> (Rig, Entity<Desk>, &mut VisualTestContext) {
    let rig = engine_rig_tests::open(cx, "", "ask", "ask");
    let script = rig.dir.path().join("sakura");
    sakura_core::test_support::write_script(&script, &format!("#!/bin/sh\n{ENGINE}\n"));
    std::fs::write(rig.dir.path().join("spec.json"), spec(&rig, agent_argv())).expect("the spec");
    let (desk, cx) = build_desk(cx, &rig, None, kept);
    (rig, desk, cx)
}

fn drawn(cx: &mut VisualTestContext, selector: &str) -> bool {
    cx.update(|window, cx| window.draw(cx).clear(cx));
    super::lane_tests::bounds(cx, selector).is_some()
}

#[gpui::test]
fn on_a_first_launch_the_composer_placeholder_is_replaced_by_the_agent_chip(
    cx: &mut TestAppContext,
) {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let (rig, desk, cx) = open_before_the_engine_answers(cx, Preferences::default());
    let view = show_master(&desk, cx);
    assert_eq!(chip_label_of(&desk, cx), None, "no name to say yet");
    let name = view.read_with(cx, |view, _| view.agent_menu().expect("a menu").name);
    assert!(
        name.is_empty(),
        "no program name and no lone Default: {name}"
    );
    assert!(drawn(cx, "master-agent-placeholder"));
    for chip in ["master-account-menu", "master-agent-chip"] {
        assert!(
            !drawn(cx, chip),
            "{chip} cannot be pressed before it says anything"
        );
    }
    // The engine answers with its accounts: the name replaces the block.
    let all = crate::ui::engine_fixture_tests::full();
    rig.say_snapshot(&all);
    rig.facts.update(cx, |facts, cx| facts.read(cx));
    settle(cx, "the accounts", |cx| {
        rig.facts
            .read_with(cx, |facts, _| facts.snapshot().is_some())
    });
    assert_eq!(chip_label_of(&desk, cx).as_deref(), Some("Personal"));
    assert!(!drawn(cx, "master-account-menu"));
    assert!(!drawn(cx, "master-agent-placeholder"));
    settle(cx, "the engine's model", |cx| {
        view.read_with(cx, |view, _| {
            view.agent_menu().is_some_and(|menu| menu.name == "Haiku")
        })
    });
    assert!(drawn(cx, "master-agent-chip"));
}

#[gpui::test]
fn a_saved_account_is_shown_while_the_model_waits_for_the_engine(cx: &mut TestAppContext) {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let kept = Preferences {
        master_account: "work".into(),
        master_account_names: [("work".to_string(), "Lavoro".to_string())].into(),
        master_model: Some("sonnet".into()),
        ..Preferences::default()
    };
    let (_rig, desk, cx) = open_before_the_engine_answers(cx, kept);
    let view = show_master(&desk, cx);
    assert_eq!(chip_label_of(&desk, cx).as_deref(), Some("Lavoro"));
    let name = view.read_with(cx, |view, _| view.agent_menu().expect("a menu").name);
    assert!(name.is_empty());
    assert!(drawn(cx, "master-agent-chip"));
    assert!(!drawn(cx, "master-account-menu"));
}

/// The calls the fake engine got, one line each.
fn engine_calls(rig: &Rig) -> Vec<String> {
    std::fs::read_to_string(rig.dir.path().join("calls.log"))
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

#[gpui::test]
fn the_account_menu_lists_every_engine_account_flat_under_its_leeway_name(cx: &mut TestAppContext) {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let all = crate::ui::engine_fixture_tests::full();
    let (_rig, desk, cx) = open_over(cx, (&all, "{}", "{}"), "ask", agent_argv(), None);
    let _view = show_master(&desk, cx);
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let entries = desk.read_with(cx, |desk, cx| {
        desk.master_page(cx).read(cx).account_entries(cx)
    });
    let names: Vec<&str> = entries.iter().map(|entry| entry.label.as_str()).collect();
    assert_eq!(
        names,
        ["Personal", "Work", "Codex"],
        "one flat entry per account, never grouped by provider"
    );
    assert!(super::lane_tests::bounds(cx, "master-agent-chip").is_some());
    assert!(super::lane_tests::bounds(cx, "master-account-menu").is_none());
    assert!(
        super::lane_tests::bounds(cx, "master-agent-switch-0").is_none(),
        "the switch is gone"
    );
}

#[gpui::test]
fn polish_y_three_accounts_fit_without_a_scroll_area(cx: &mut TestAppContext) {
    let all = crate::ui::engine_fixture_tests::full();
    let (_rig, desk, cx) = open_over(cx, (&all, "{}", "{}"), "ask", agent_argv(), None);
    let _view = show_master(&desk, cx);
    super::lane_tests::click(cx, "master-agent-chip");
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let menu = cx
        .debug_bounds("dropdown-surface")
        .expect("the account menu");
    let last = cx.debug_bounds("dropdown-item-2").unwrap();
    let footer = cx.debug_bounds("dropdown-footer").unwrap();
    assert!(last.bottom() <= footer.top() && footer.bottom() <= menu.bottom());
    assert!(cx.debug_bounds("dropdown-scroll").is_none());
    assert!(cx.debug_bounds("dropdown-scrollbar").is_none());
    cx.simulate_event(gpui::ScrollEvent {
        position: last.center(),
        delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(-60.))),
        ..Default::default()
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert_eq!(cx.debug_bounds("dropdown-item-2"), Some(last));
}

#[gpui::test]
fn polish_y_forty_accounts_scroll_under_a_pinned_action(cx: &mut TestAppContext) {
    let all = crate::ui::engine_fixture_tests::edited(|snapshot| {
        let accounts = snapshot["accounts"].as_array_mut().unwrap();
        let model = accounts[0].clone();
        accounts.extend((3..40).map(|index| {
            let mut account = model.clone();
            let id = format!("account-{index}");
            account["id"] = id.clone().into();
            account["key"] = id.into();
            account["label"] = format!("Account {index}").into();
            account
        }));
    });
    let (_rig, desk, cx) = open_over(cx, (&all, "{}", "{}"), "ask", agent_argv(), None);
    let _view = show_master(&desk, cx);
    cx.simulate_resize(gpui::size(
        crate::theme::WINDOW_MIN_WIDTH,
        crate::theme::WINDOW_MIN_HEIGHT,
    ));
    super::lane_tests::click(cx, "master-agent-chip");
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let menu = cx
        .debug_bounds("dropdown-surface")
        .expect("the account menu");
    let list = cx
        .debug_bounds("dropdown-scroll")
        .expect("the list scrolls");
    let footer = cx
        .debug_bounds("dropdown-footer")
        .expect("the action is pinned");
    let first = cx.debug_bounds("dropdown-item-0").unwrap();
    assert!(cx.debug_bounds("dropdown-scrollbar").is_some());
    assert!(list.bottom() <= footer.top() && footer.bottom() <= menu.bottom());
    assert!(menu.top() >= gpui::px(0.) && menu.bottom() <= crate::theme::WINDOW_MIN_HEIGHT);
    assert!(menu.left() >= gpui::px(0.) && menu.right() <= crate::theme::WINDOW_MIN_WIDTH);
    cx.simulate_event(gpui::ScrollEvent {
        position: list.center(),
        delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(-1200.))),
        ..Default::default()
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(cx.debug_bounds("dropdown-item-0").unwrap().top() < first.top());
    assert!(cx.debug_bounds("dropdown-item-39").unwrap().bottom() <= list.bottom());
    assert_eq!(cx.debug_bounds("dropdown-footer"), Some(footer));
    cx.simulate_keystrokes("up enter");
    settle(cx, "Add account opened", |cx| {
        cx.debug_bounds("account-add").is_some()
    });
}

#[gpui::test]
fn picking_another_account_opens_a_new_conversation_on_it_and_keeps_the_first_whole_in_history(
    cx: &mut TestAppContext,
) {
    use super::master_agent::MasterAgent::Claude;
    let all = crate::ui::engine_fixture_tests::full();
    let (rig, desk, cx) = open_over(cx, (&all, "{}", "{}"), "ask", agent_argv(), None);
    let view = show_master(&desk, cx);
    say(&view, "plain hello", cx);
    settle(cx, "the first turn", |cx| turn_ended(&view, cx));
    desk.read_with(cx, |desk, cx| desk.master_page(cx))
        .update(cx, |page, cx| page.press_account(Claude, "work", cx));
    cx.run_until_parked();
    view.read_with(cx, |view, _| {
        assert!(view.thread().entries().is_empty(), "a new conversation")
    });
    let kept = desk.read_with(cx, |desk, cx| {
        desk.master_page(cx).read(cx).history_for_test()
    });
    assert_eq!(kept.len(), 1, "the first stays whole in History");
    say(&view, "plain hello", cx);
    settle(cx, "the second turn", |cx| turn_ended(&view, cx));
    assert!(
        engine_calls(&rig).contains(&"master spec --json --account work".to_string()),
        "the master was started on the account picked: {:?}",
        engine_calls(&rig)
    );
}

#[gpui::test]
fn a_codex_account_starts_codex_in_its_own_codex_home_and_answers(cx: &mut TestAppContext) {
    use super::master_agent::MasterAgent::Codex;
    let all = crate::ui::engine_fixture_tests::full();
    let (rig, desk, cx) = open_over(cx, (&all, "{}", "{}"), "ask", agent_argv(), None);
    offer_codex(&rig);
    let view = show_master(&desk, cx);
    desk.read_with(cx, |desk, cx| desk.master_page(cx))
        .update(cx, |page, cx| page.press_account(Codex, "codex", cx));
    cx.run_until_parked();
    say(&view, "plain", cx);
    settle(cx, "codex's turn", |cx| turn_ended(&view, cx));
    assert!(engine_calls(&rig).contains(&"master spec --json --account codex".to_string()));
}

#[gpui::test]
fn adding_an_account_from_the_master_menu_opens_the_sheet(cx: &mut TestAppContext) {
    let all = crate::ui::engine_fixture_tests::full();
    let (_rig, desk, cx) = open_over(cx, (&all, "{}", "{}"), "ask", agent_argv(), None);
    let _view = show_master(&desk, cx);
    let page = desk.read_with(cx, |desk, cx| desk.master_page(cx));
    cx.update(|window, cx| page.update(cx, |page, cx| page.open_add_account(window, cx)));
    settle(cx, "the sheet", |cx| {
        super::lane_tests::bounds(cx, "account-add").is_some()
    });
}

/// The full snapshot with the engine's accounts in `ids` order.
fn accounts_in_order(ids: &[&str]) -> String {
    crate::ui::engine_fixture_tests::edited(|snapshot| {
        let all = snapshot["accounts"].as_array().expect("accounts").clone();
        let kept: Vec<serde_json::Value> = ids
            .iter()
            .map(|id| all.iter().find(|row| row["id"] == *id).expect(id).clone())
            .collect();
        snapshot["accounts"] = serde_json::Value::Array(kept);
    })
}

/// Opens a new tab for a new conversation and says `plain` in it.
fn say_in_a_new_tab(desk: &Entity<Desk>, cx: &mut VisualTestContext) {
    super::lane_tests::click(cx, "bar-new-tab");
    cx.run_until_parked();
    let view = desk.read_with(cx, |desk, cx| desk.master_conversation(cx));
    say(&view, "plain", cx);
    settle(cx, "the turn", |cx| turn_ended(&view, cx));
}

#[gpui::test]
fn a_new_conversation_starts_on_the_first_claude_account_even_when_a_codex_one_is_listed_first(
    cx: &mut TestAppContext,
) {
    let all = accounts_in_order(&["codex", "personal", "work"]);
    let (rig, desk, cx) = open_over(cx, (&all, "{}", "{}"), "ask", agent_argv(), None);
    let _view = show_master(&desk, cx);
    say_in_a_new_tab(&desk, cx);
    assert!(
        engine_calls(&rig).contains(&"master spec --json --account personal".to_string()),
        "{:?}",
        engine_calls(&rig)
    );
}

#[gpui::test]
fn with_no_claude_account_a_new_conversation_starts_on_the_first_account(cx: &mut TestAppContext) {
    let all = accounts_in_order(&["codex"]);
    let (rig, desk, cx) = open_over(cx, (&all, "{}", "{}"), "ask", agent_argv(), None);
    offer_codex(&rig);
    let _view = show_master(&desk, cx);
    say_in_a_new_tab(&desk, cx);
    assert!(
        engine_calls(&rig).contains(&"master spec --json --account codex".to_string()),
        "{:?}",
        engine_calls(&rig)
    );
}

#[gpui::test]
fn the_account_model_and_effort_set_in_settings_start_every_new_conversation(
    cx: &mut TestAppContext,
) {
    let all = crate::ui::engine_fixture_tests::full();
    let kept = Preferences {
        new_master_account: Some("work".into()),
        new_master_model: Some("sonnet".into()),
        new_master_effort: Some("low".into()),
        ..Preferences::default()
    };
    let (rig, desk, cx) = open_kept(cx, (&all, "{}", "{}"), "ask", agent_argv(), (None, kept));
    let _view = show_master(&desk, cx);
    super::lane_tests::click(cx, "bar-new-tab");
    cx.run_until_parked();
    let page = desk.read_with(cx, |desk, cx| desk.master_page(cx));
    let (account, start) = page.read_with(cx, |page, _| {
        (page.account_for_test(), page.start_for_test())
    });
    assert_eq!(account, "work");
    assert_eq!(start.model.as_deref(), Some("sonnet"));
    assert_eq!(start.effort.as_deref(), Some("low"));
    let view = desk.read_with(cx, |desk, cx| desk.master_conversation(cx));
    say(&view, "plain", cx);
    settle(cx, "the turn", |cx| turn_ended(&view, cx));
    assert!(engine_calls(&rig).contains(&"master spec --json --account work".to_string()));
}

#[gpui::test]
fn starting_fresh_goes_back_to_the_account_set_for_new_conversations(cx: &mut TestAppContext) {
    use super::master_agent::MasterAgent::Claude;
    let all = crate::ui::engine_fixture_tests::full();
    let (_rig, desk, cx) = open_over(cx, (&all, "{}", "{}"), "ask", agent_argv(), None);
    let _view = show_master(&desk, cx);
    let page = desk.read_with(cx, |desk, cx| desk.master_page(cx));
    page.update(cx, |page, cx| page.press_account(Claude, "work", cx));
    assert_eq!(
        page.read_with(cx, |page, _| page.account_for_test()),
        "work"
    );
    page.update(cx, |page, cx| page.start_fresh(cx));
    assert_eq!(
        page.read_with(cx, |page, _| page.account_for_test()),
        "personal"
    );
}

/// What the fake agent was asked to load (`session/load`), by session id, in order.
fn loads_asked(rig: &Rig) -> Vec<String> {
    let all = received(rig);
    let loads = all
        .iter()
        .filter(|message| message["method"] == "session/load");
    loads
        .filter_map(|message| message["params"]["sessionId"].as_str().map(str::to_string))
        .collect()
}

#[gpui::test]
fn fix37_master_reloads_the_tabs_session_after_quitting_from_inbox(cx: &mut TestAppContext) {
    let all = crate::ui::engine_fixture_tests::full();
    let (rig, desk, window) = open_over(
        cx,
        (&all, "{}", "{}"),
        "ask",
        agent_argv_with(&["--no-list"]),
        None,
    );
    super::lane_tests::click(window, "sidebar-master");
    let view = desk.read_with(window, |desk, cx| desk.master_conversation(cx));
    say(&view, "plain hello", window);
    settle(window, "the turn", |window| turn_ended(&view, window));
    let talk = desk.read_with(window, |desk, cx| {
        desk.bar.read(cx).tabs().active().conversation().to_string()
    });
    let key = format!("personal@{talk}");
    let session = rig
        .sessions
        .stored("@master", &key)
        .expect("the first session");
    super::lane_tests::click(window, "sidebar-inbox");
    let preferences = window.update(|_, cx| Preferences::current(cx));
    let saved = sakura_core::tabs::Tabs::restore(preferences.tabs.as_ref());
    assert_eq!(saved.active().page(), &Page::Inbox);
    assert_eq!(saved.active().conversation(), talk);
    window.update(|_, cx| crate::menus::install(cx));
    window.simulate_keystrokes("cmd-q");
    drop(view);
    drop(desk);
    window.update(|window, _| window.remove_window());
    window.run_until_parked();

    let (desk, window) = build_desk(cx, &rig, None, preferences);
    super::lane_tests::click(window, "sidebar-master");
    let view = desk.read_with(window, |desk, cx| desk.master_conversation(cx));
    assert_ne!(
        view.read_with(window, |view, _| view.phase().clone()),
        Phase::Idle
    );
    settle(window, "the saved session loads", |_| {
        !loads_asked(&rig).is_empty()
    });
    assert_eq!(loads_asked(&rig), [session]);
    assert_eq!(set_calls(&rig, "session/new").len(), 1);
}

#[gpui::test]
fn restoring_a_missing_master_conversation_waits_for_a_message_to_start_a_new_one(
    cx: &mut TestAppContext,
) {
    check_missing_master_session(cx, &[], "session/list");
}

#[gpui::test]
fn restoring_an_unloadable_master_conversation_waits_for_a_message_to_start_a_new_one(
    cx: &mut TestAppContext,
) {
    check_missing_master_session(cx, &["--no-list"], "session/load");
}

fn check_missing_master_session(cx: &mut TestAppContext, flags: &[&str], lookup: &str) {
    let mut saved = sakura_core::tabs::Tabs::new();
    saved.open_new(Page::Master);
    let talk = saved.active().conversation();
    let account = format!("personal@{talk}");
    let runtime = std::sync::Arc::new(tokio::runtime::Runtime::new().unwrap());
    let sessions = crate::data::sessions::Sessions::new(None, runtime);
    sessions.store("@master", &account, "missing-session");
    let kept = Preferences {
        tabs: Some(saved.to_json()),
        ..Preferences::default()
    };
    let all = crate::ui::engine_fixture_tests::full();
    let (rig, desk, cx) = open_kept(
        cx,
        (&all, "{}", "{}"),
        "ask",
        agent_argv_with(flags),
        (Some(sessions.clone()), kept),
    );
    let view = desk.read_with(cx, |desk, cx| desk.master_conversation(cx));
    settle(cx, "the lookup", |cx| {
        methods(&rig).iter().any(|method| method == lookup)
            && view.read_with(cx, |view, _| {
                matches!(view.phase(), Phase::Idle | Phase::Ready | Phase::Ended(_))
            })
    });
    assert!(set_calls(&rig, "session/new").is_empty());
    assert_eq!(sessions.stored("@master", &account), None);
    assert!(cx.debug_bounds("thread-kept-lost").is_some());
    say(&view, "plain", cx);
    settle(cx, "the first turn", |cx| turn_ended(&view, cx));
    assert_eq!(set_calls(&rig, "session/new").len(), 1);
    assert_eq!(set_calls(&rig, "session/prompt").len(), 1);
    assert!(cx.debug_bounds("thread-kept-lost").is_none());
}

/// Cause of the blocker of click proof 040m: a conversation was kept under its tab's number, which a tab of the
/// next launch took up again, and only one of the saved Master tabs came back.
#[gpui::test]
fn three_saved_master_tabs_come_back_each_on_its_own_conversation_and_a_new_tab_is_empty(
    cx: &mut TestAppContext,
) {
    let mut saved = sakura_core::tabs::Tabs::new();
    for _ in 0..3 {
        saved.open_new(Page::Master);
    }
    let talks: Vec<String> = saved.tabs()[1..]
        .iter()
        .map(|tab| tab.conversation().to_string())
        .collect();
    let runtime = std::sync::Arc::new(tokio::runtime::Runtime::new().expect("a runtime"));
    let sessions = crate::data::sessions::Sessions::new(None, runtime);
    for (at, talk) in talks.iter().enumerate() {
        let key = format!("personal@{talk}");
        sessions.store("@master", &key, &format!("kept-{at}"));
    }
    let kept = Preferences {
        tabs: Some(saved.to_json()),
        ..Preferences::default()
    };
    let all = crate::ui::engine_fixture_tests::full();
    let (rig, desk, cx) = open_kept(
        cx,
        (&all, "{}", "{}"),
        "ask",
        agent_argv_with(&["--no-list"]),
        (Some(sessions), kept),
    );
    let tabs = desk.read_with(cx, |desk, cx| desk.bar.read(cx).tabs().tabs().len());
    assert_eq!(tabs, 4, "the Inbox and the three Master tabs");
    let mut views = Vec::new();
    for key in ["cmd-2", "cmd-3", "cmd-4"] {
        cx.simulate_keystrokes(key);
        let view = desk.read_with(cx, |desk, cx| desk.master_conversation(cx));
        settle(cx, "the conversation", |cx| {
            view.read_with(cx, |view, _| {
                *view.phase() != Phase::Loading
                    && *view.phase() != Phase::Connecting
                    && *view.phase() != Phase::Preparing
            })
        });
        views.push(view);
    }
    let ids: std::collections::HashSet<_> = views.iter().map(|view| view.entity_id()).collect();
    assert_eq!(ids.len(), 3, "three conversations");
    let mut loaded = loads_asked(&rig);
    loaded.sort();
    assert_eq!(loaded, ["kept-0", "kept-1", "kept-2"], "each loads its own");
    super::lane_tests::click(cx, "bar-new-tab");
    cx.run_until_parked();
    let fresh = desk.read_with(cx, |desk, cx| desk.master_conversation(cx));
    assert!(!views
        .iter()
        .any(|view| view.entity_id() == fresh.entity_id()));
    assert_eq!(fresh.read_with(cx, |view, _| view.first_message()), None);
    assert_eq!(loads_asked(&rig).len(), 3, "the new tab loads nothing");
}

#[gpui::test]
fn a_conversation_that_never_finishes_loading_says_so_in_words_instead_of_loading_for_ever(
    cx: &mut TestAppContext,
) {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let mut saved = sakura_core::tabs::Tabs::new();
    saved.open_new(Page::Master);
    let talk = saved.tabs()[1].conversation().to_string();
    let runtime = std::sync::Arc::new(tokio::runtime::Runtime::new().expect("a runtime"));
    let sessions = crate::data::sessions::Sessions::new(None, runtime);
    sessions.store("@master", &format!("personal@{talk}"), "kept-0");
    let kept = Preferences {
        tabs: Some(saved.to_json()),
        ..Preferences::default()
    };
    let all = crate::ui::engine_fixture_tests::full();
    let (_rig, desk, cx) = open_kept(
        cx,
        (&all, "{}", "{}"),
        "ask",
        agent_argv_with(&["--no-list", "--hang-load"]),
        (Some(sessions), kept),
    );
    let view = desk.read_with(cx, |desk, cx| desk.master_conversation(cx));
    settle(cx, "the loading", |cx| {
        view.read_with(cx, |view, _| *view.phase() == Phase::Loading)
    });
    cx.executor()
        .advance_clock(crate::theme::thread::LOAD_STALL + std::time::Duration::from_secs(1));
    cx.run_until_parked();
    let phase = view.read_with(cx, |view, _| view.phase().clone());
    let Phase::Ended(problem) = phase else {
        panic!("the loading never ended: {phase:?}");
    };
    assert!(
        problem.detail.contains("did not load"),
        "{}",
        problem.detail
    );
}

/// The engine's rules with the models its Claude account lists.
fn rules_with_claude_models() -> String {
    rules_with_model_defaults("high")
}

fn rules_with_model_defaults(effort: &str) -> String {
    crate::ui::engine_fixture_tests::rules_edited(|rules| {
        rules["agents"] = serde_json::json!([
            {"id": "personal", "label": "Personal", "provider": "claude", "models": [
                {"id": "opus", "label": "Opus 5.5", "efforts": ["low", "medium", "high"], "default_effort": effort},
                {"id": "sonnet", "label": "Sonnet 5.5", "efforts": ["low", "medium", "high"], "default_effort": effort},
                {"id": "haiku", "label": "Haiku 5.5", "efforts": [], "default_effort": (effort.is_empty().then_some(effort))}
            ]}
        ]);
    })
}

/// The names the chip's menu offers, in order.
fn menu_names(view: &Entity<ThreadView>, cx: &mut VisualTestContext) -> Vec<String> {
    view.read_with(cx, |view, _| {
        let menu = view.agent_menu().expect("a menu");
        let entries = menu.groups.iter().flat_map(|group| &group.entries);
        entries.map(|entry| entry.name.to_string()).collect()
    })
}

#[gpui::test]
fn before_the_first_message_the_chip_lists_the_models_the_engine_knows_for_the_account(
    cx: &mut TestAppContext,
) {
    let all = crate::ui::engine_fixture_tests::full();
    let (rig, desk, cx) = open_over(cx, (&all, "{}", "{}"), "ask", agent_argv(), None);
    rig.say_rules(&rules_with_claude_models());
    let view = show_master(&desk, cx);
    settle(cx, "the models", |cx| {
        menu_names(&view, cx)
            .iter()
            .any(|name| name == "Sonnet 5.5")
    });
    let names = menu_names(&view, cx);
    for model in ["Opus 5.5", "Sonnet 5.5", "Haiku 5.5"] {
        assert!(
            names.iter().any(|name| name == model),
            "{model} in {names:?}"
        );
    }
}

#[gpui::test]
fn a_conversation_set_to_sonnet_low_comes_back_reading_sonnet_low_with_every_model_in_its_menu(
    cx: &mut TestAppContext,
) {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let choice = |id: &str, name: &str, efforts: &[&str]| sakura_acp::model::Choice {
        id: id.into(),
        name: name.into(),
        efforts: efforts
            .iter()
            .map(|effort| sakura_acp::model::Choice {
                id: (*effort).into(),
                name: (*effort).into(),
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    };
    let models = [
        choice("opus", "Opus 5.5", &["low", "high"]),
        choice("sonnet", "Sonnet 5.5", &["low", "high"]),
        choice("haiku", "Haiku 5.5", &[]),
    ];
    let mut kept = Preferences::default();
    kept.master_models.insert(
        "claude".into(),
        super::master_agent::models_to_json(&models),
    );
    kept.master_picks.insert(
        "kept-1".into(),
        serde_json::json!({"model": "sonnet", "effort": "low", "mode": "default"}),
    );
    kept.tabs = Some(serde_json::json!({"version": 3, "active": 0,
        "tabs": [{"page": "inbox"}, {"page": "master"}]}));
    let runtime = std::sync::Arc::new(tokio::runtime::Runtime::new().expect("a runtime"));
    let sessions = crate::data::sessions::Sessions::new(None, runtime);
    sessions.store("@master", "personal", "kept-1");
    let all = crate::ui::engine_fixture_tests::full();
    let (_rig, desk, cx) = open_kept(
        cx,
        (&all, "{}", "{}"),
        "ask",
        agent_argv_with(&["--no-list"]),
        (Some(sessions), kept),
    );
    let view = show_master(&desk, cx);
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let name = view.read_with(cx, |view, _| {
        view.agent_menu().expect("a menu").name.to_string()
    });
    assert_eq!(name, "Sonnet 5.5 Low");
    let names = menu_names(&view, cx);
    for model in ["Opus 5.5", "Sonnet 5.5", "Haiku 5.5"] {
        assert!(
            names.iter().any(|entry| entry == model),
            "{model} in {names:?}"
        );
    }
}

#[gpui::test]
fn a_model_and_an_effort_saved_with_no_account_chosen_start_a_new_conversation_all_the_same(
    cx: &mut TestAppContext,
) {
    let all = crate::ui::engine_fixture_tests::full();
    let kept = Preferences {
        new_master_model: Some("haiku".into()),
        new_master_effort: Some("low".into()),
        ..Preferences::default()
    };
    let (_rig, desk, cx) = open_kept(cx, (&all, "{}", "{}"), "ask", agent_argv(), (None, kept));
    let _view = show_master(&desk, cx);
    super::lane_tests::click(cx, "bar-new-tab");
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let page = desk.read_with(cx, |desk, cx| desk.master_page(cx));
    let start = page.read_with(cx, |page, _| page.start_for_test());
    assert_eq!(start.model.as_deref(), Some("haiku"));
    assert_eq!(start.effort.as_deref(), Some("low"));
}

#[gpui::test]
fn start_fresh_keeps_a_conversation_that_is_still_loading_in_history(cx: &mut TestAppContext) {
    let runtime = std::sync::Arc::new(tokio::runtime::Runtime::new().expect("a runtime"));
    let sessions = crate::data::sessions::Sessions::new(None, runtime);
    sessions.store("@master", "personal", "kept-0");
    let (_rig, desk, cx) = open_legacy(
        cx,
        "ask",
        agent_argv_with(&["--no-list", "--hang-load"]),
        Some(sessions),
    );
    let view = show_master(&desk, cx);
    settle(cx, "the loading", |cx| {
        view.read_with(cx, |view, _| *view.phase() == Phase::Loading)
    });
    let page = desk.read_with(cx, |desk, cx| desk.master_page(cx));
    page.update(cx, |page, cx| page.start_fresh(cx));
    let kept = page.read_with(cx, |page, _| page.history_for_test());
    let sessions: Vec<&str> = kept.iter().map(|past| past.session.as_str()).collect();
    assert_eq!(
        sessions,
        ["kept-0"],
        "the conversation is in History, to be opened again"
    );
}

#[gpui::test]
fn start_fresh_begins_on_the_model_and_the_effort_set_for_new_conversations(
    cx: &mut TestAppContext,
) {
    let all = crate::ui::engine_fixture_tests::full();
    let kept = Preferences {
        new_master_model: Some("sonnet".into()),
        new_master_effort: Some("low".into()),
        ..Preferences::default()
    };
    let (_rig, desk, cx) = open_kept(cx, (&all, "{}", "{}"), "ask", agent_argv(), (None, kept));
    let _view = show_master(&desk, cx);
    let page = desk.read_with(cx, |desk, cx| desk.master_page(cx));
    page.update(cx, |page, cx| page.start_fresh(cx));
    let start = page.read_with(cx, |page, _| page.start_for_test());
    assert_eq!(start.model.as_deref(), Some("sonnet"));
    assert_eq!(start.effort.as_deref(), Some("low"));
}

#[gpui::test]
fn on_a_fresh_install_the_chip_names_the_spec_model_and_uses_the_catalogue_label(
    cx: &mut TestAppContext,
) {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let mut kept = Preferences::default();
    let models = [
        sakura_acp::model::Choice {
            id: "default".into(),
            name: "Default (recommended)".into(),
            ..Default::default()
        },
        sakura_acp::model::Choice {
            id: "haiku".into(),
            name: "Haiku 5.5".into(),
            ..Default::default()
        },
    ];
    kept.master_models.insert(
        "claude".into(),
        super::master_agent::models_to_json(&models),
    );
    let all = crate::ui::engine_fixture_tests::full();
    let (_rig, desk, cx) = open_kept(cx, (&all, "{}", "{}"), "ask", agent_argv(), (None, kept));
    let view = show_master(&desk, cx);
    settle(cx, "the engine's model", |cx| {
        view.read_with(cx, |view, _| view.agent_menu().unwrap().name == "Haiku 5.5")
    });
}

#[gpui::test]
fn a_conversation_whose_tab_goes_to_another_page_is_in_history(cx: &mut TestAppContext) {
    let (_rig, desk, cx) = open(cx, "ask", agent_argv());
    let view = show_master(&desk, cx);
    say(&view, "plain hello", cx);
    settle(cx, "the turn", |cx| turn_ended(&view, cx));
    // A click in the sidebar shows another page in the tab that held the conversation.
    desk.update(cx, |desk, cx| desk.navigate(Page::Lanes, false, cx));
    cx.run_until_parked();
    let page = desk.read_with(cx, |desk, cx| desk.master_page(cx));
    let titles: Vec<String> = page
        .read_with(cx, |page, _| page.history_for_test())
        .into_iter()
        .map(|past| past.title)
        .collect();
    assert_eq!(titles, ["plain hello"]);
}

#[gpui::test]
fn the_files_attached_and_not_sent_when_the_app_closes_are_named_when_it_opens_again(
    cx: &mut TestAppContext,
) {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let (_rig, desk, cx) = open(cx, "ask", agent_argv());
    let view = show_master(&desk, cx);
    let (_folder, [notes, picture, _]) = files();
    view.update(cx, |view, cx| view.attach_paths(&[notes, picture], cx));
    cx.run_until_parked();
    // Leaving the page keeps the drafts at once, as quitting does.
    desk.update(cx, |desk, cx| desk.navigate(Page::Inbox, false, cx));
    cx.run_until_parked();
    let kept = cx.update(|_, cx| Preferences::current(cx).drafts.clone());
    let drafts = sakura_core::drafts::Drafts::restore(kept.as_ref());
    assert_eq!(drafts.files(&Page::Master), ["notes.md", "shot.png"]);
}

#[gpui::test]
fn a_draft_that_had_files_comes_back_saying_which_were_not_kept_and_sends_without_none_unseen(
    cx: &mut TestAppContext,
) {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let mut drafts = sakura_core::drafts::Drafts::default();
    drafts.set(&Page::Master, "look at these");
    drafts.set_files(
        &Page::Master,
        &["notes.md".to_string(), "shot.png".to_string()],
    );
    let kept = Preferences {
        drafts: Some(drafts.to_json()),
        ..Preferences::default()
    };
    let all = crate::ui::engine_fixture_tests::full();
    let (_rig, desk, cx) = open_kept(cx, (&all, "{}", "{}"), "ask", agent_argv(), (None, kept));
    let view = show_master(&desk, cx);
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let said = view.read_with(cx, |view, _| {
        view.refusal().map(super::thread::refusal_words)
    });
    let said = said.expect("a notice").to_string();
    assert!(
        said.contains("notes.md") && said.contains("shot.png"),
        "{said}"
    );
    assert!(attached(&view, cx).is_empty(), "the files are not there");
    assert!(
        super::lane_tests::bounds(cx, "master-attachment-refusal").is_some(),
        "and it is said on the screen"
    );
}

#[gpui::test]
fn the_list_of_help_closes_when_a_file_is_attached_as_it_does_when_something_is_typed(
    cx: &mut TestAppContext,
) {
    let (_rig, desk, cx) = open(cx, "ask", agent_argv());
    let view = show_master(&desk, cx);
    say(&view, "/help", cx);
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        super::lane_tests::bounds(cx, "slash-row-help").is_some(),
        "the list shows after /help"
    );
    let (_folder, [notes, _, _]) = files();
    view.update(cx, |view, cx| view.attach_paths(&[notes], cx));
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        super::lane_tests::bounds(cx, "slash-row-help").is_none(),
        "the box changed: the list is gone"
    );
}

#[gpui::test]
fn the_chip_menu_opens_at_its_top_with_the_first_group_title_in_view_whatever_entry_is_checked(
    cx: &mut TestAppContext,
) {
    let model = |id: &str, name: &str| sakura_acp::model::Choice {
        id: id.into(),
        name: name.into(),
        ..Default::default()
    };
    let models: Vec<sakura_acp::model::Choice> = (0..14)
        .map(|at| model(&format!("m{at}"), &format!("Model {at}")))
        .collect();
    let mut kept = Preferences::default();
    kept.master_models.insert(
        "claude".into(),
        super::master_agent::models_to_json(&models),
    );
    kept.new_master_model = Some("m9".into());
    let all = crate::ui::engine_fixture_tests::full();
    let (_rig, desk, cx) = open_kept(cx, (&all, "{}", "{}"), "ask", agent_argv(), (None, kept));
    cx.simulate_resize(gpui::size(gpui::px(1280.), gpui::px(800.)));
    let _view = show_master(&desk, cx);
    // A new conversation starts on Model 9, which is far down the list.
    super::lane_tests::click(cx, "bar-new-tab");
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    super::lane_tests::click(cx, "master-agent-chip");
    for _ in 0..12 {
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }
    let chip = super::lane_tests::bounds(cx, "master-agent-chip").expect("the chip");
    let first = super::lane_tests::bounds(cx, "chip-entry-0-0").expect("the first entry");
    let menu_top = chip.top() - super::thread::agent_menu::chip_menu_max_height(gpui::px(800.));
    assert!(
        first.top() >= menu_top && first.top() >= gpui::px(0.),
        "the first entry {first:?} is in the menu, which opens at its top ({menu_top:?} up)"
    );
}

#[gpui::test]
fn reopening_history_in_another_tab_activates_its_owner(cx: &mut TestAppContext) {
    check_history_opens_owner(cx, false);
}

#[gpui::test]
fn reopening_history_from_a_split_pane_activates_its_owner(cx: &mut TestAppContext) {
    check_history_opens_owner(cx, true);
}

fn check_history_opens_owner(cx: &mut TestAppContext, is_split: bool) {
    let runtime = std::sync::Arc::new(tokio::runtime::Runtime::new().expect("a runtime"));
    let sessions = crate::data::sessions::Sessions::new(None, runtime);
    sessions.store("@master", "personal", "fake-session-1");
    let (_rig, desk, cx) = open_legacy(cx, "ask", agent_argv(), Some(sessions.clone()));
    let original_view = show_master(&desk, cx);
    settle(cx, "the original session", |cx| {
        original_view.read_with(cx, |view, _| *view.phase() == Phase::Ready)
    });
    drop(original_view);
    let bar = desk.read_with(cx, |desk, _| desk.bar.clone());
    let original = bar.read_with(cx, |bar, _| bar.tabs().active().id());
    let other = bar.update(cx, |bar, cx| bar.open_new(Page::Master, cx));
    if is_split {
        bar.update(cx, |bar, cx| assert!(bar.create_split(original, other, cx)));
    }
    cx.run_until_parked();
    bar.update(cx, |bar, cx| bar.activate(original, cx));
    desk.update(cx, |desk, cx| desk.navigate(Page::Lanes, false, cx));
    cx.run_until_parked();
    bar.update(cx, |bar, cx| bar.activate(other, cx));
    cx.run_until_parked();
    let slot = bar.read_with(cx, |bar, _| bar.tabs().active().conversation().to_string());
    let page = desk.read_with(cx, |desk, cx| desk.master_page(cx));
    page.update(cx, |page, cx| page.resume("fake-session-1", cx));
    cx.run_until_parked();
    bar.read_with(cx, |bar, _| {
        assert_eq!(bar.tabs().active().id(), original);
        assert_eq!(bar.tabs().active().page(), &Page::Master);
        if is_split {
            assert_eq!(bar.tabs().visible(), (original, Some(other)));
            assert_eq!(bar.tabs().tab(other).unwrap().page(), &Page::Master);
        }
    });
    assert_eq!(sessions.find_slot(&slot), None);
    assert_eq!(
        sessions.stored("@master", "personal").as_deref(),
        Some("fake-session-1")
    );
}

#[gpui::test]
fn master_suggestions_fit_a_narrow_split_pane(cx: &mut TestAppContext) {
    use super::lane_tests::bounds;
    let (_rig, desk, cx) = open(cx, "ask", agent_argv());
    let _view = show_master(&desk, cx);
    cx.simulate_resize(gpui::size(gpui::px(800.), gpui::px(700.)));
    let bar = desk.read_with(cx, |desk, _| desk.bar.clone());
    let master = bar.update(cx, |bar, cx| {
        let master = bar.tabs().active().id();
        let other = bar.open_new(Page::Lanes, cx);
        assert!(bar.create_split(master, other, cx));
        master
    });
    let pane_selector = format!("page-pane-{}", master.as_u64());
    settle(cx, "Master in a split pane", |cx| {
        bounds(cx, &pane_selector).is_some()
    });
    let pane = bounds(cx, &pane_selector).unwrap();
    for name in ["thread-example-0", "thread-example-1"] {
        let chip = bounds(cx, name).expect(name);
        assert!(
            chip.left() >= pane.left() && chip.right() <= pane.right(),
            "{chip:?} exceeds {pane:?}"
        );
    }
}

#[gpui::test]
fn reaudit3_master_does_not_name_the_harness_while_the_model_is_unknown(cx: &mut TestAppContext) {
    let fixture = crate::ui::engine_fixture_tests::full();
    let (_rig, desk, window) = super::engine_desk_tests::open(cx, &fixture);
    let view = show_master(&desk, window);
    engine_rig_tests::wait_until(window, "the Master agent menu", |window| {
        drawn(window, "master-agent-chip")
    });
    assert!(view.read_with(window, |view, _| view.agent_menu().unwrap().name.is_empty()));
    let chip = window.debug_bounds("master-agent-chip").unwrap();
    assert!(
        chip.size.width < gpui::px(105.),
        "model control reads as a program: {chip:?}"
    );
}

#[gpui::test]
fn polish_zd_master_names_the_catalogue_selection_while_the_spec_is_unavailable(
    cx: &mut TestAppContext,
) {
    let all = crate::ui::engine_fixture_tests::full();
    let mut kept = Preferences::default();
    kept.master_models.insert(
        "claude".into(),
        super::master_agent::models_to_json(&[sakura_acp::model::Choice {
            id: "opus".into(),
            name: "Opus 5.5".into(),
            ..Default::default()
        }]),
    );
    let rig = master_rig(cx, (&all, "{}", "{}"), "ask", agent_argv());
    std::fs::write(rig.dir.path().join("spec.json"), "{}").unwrap();
    let (desk, cx) = build_desk(cx, &rig, None, kept);
    let view = show_master(&desk, cx);
    let menu = view.read_with(cx, |view, _| view.agent_menu().unwrap());
    assert_eq!(menu.name.as_ref(), "Opus 5.5");
    assert!(menu
        .groups
        .iter()
        .flat_map(|group| &group.entries)
        .any(|entry| { entry.is_on && entry.name.as_ref() == "Opus 5.5" }));
}
