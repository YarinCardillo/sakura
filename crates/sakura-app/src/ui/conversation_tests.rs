//! The conversation as the home of a lavoro, on the real chain in a test window: a fake `box` whose `acp`
//! runs the fake agent of `sakura-acp` (built with the workspace's tests, or by
//! `cargo build -p sakura-acp --bin sakura-fake-agent`). What `box` was asked is read back, so a test can say
//! which processes a person's action started and which it did not.

use super::desk::Desk;
use super::interaction_tests::{click, draw};
use super::l10n_tests::{width, width_of};
use super::launch_tests::{calls, fake_agentbox, open_live, Names, TASK};
use super::thread::{Phase, ThreadView};
use crate::l10n::Key;
use crate::theme;
use gpui::{Entity, TestAppContext, VisualTestContext};
use sakura_acp::thread::{EntryKind, Turn};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// A lavoro of the fixture that was taken: it is listed last, and has nothing to take.
const TAKEN: &str = "headings-26";
/// The lavoro of the fixture whose agent runs in a terminal.
const IN_TERMINAL: &str = "sample-folders";
/// The account of the boxes that is signed in.
const ACCOUNT: &str = "personal";
const SIGNED_IN: &str = r#"[{"name":"personal","loggedIn":true}]"#;
/// The fake agent's pace: fast, so a test waits for the app and not for the agent.
const FAST: &str = "--delay-ms 2";

const WAIT_STEP: Duration = Duration::from_millis(10);
/// A frame of the test clock: the conversation takes the agent's events once per frame, on a timer.
const FRAME: Duration = Duration::from_millis(20);

/// Waits for work on other threads (the agent, `box`) until `is_ready`, letting the test clock run.
pub(super) fn wait_for(
    cx: &mut VisualTestContext,
    what: &str,
    is_ready: impl Fn(&mut VisualTestContext) -> bool,
) {
    let deadline = std::time::Instant::now() + super::interaction_tests::HANG;
    while std::time::Instant::now() < deadline {
        cx.executor().advance_clock(FRAME);
        cx.run_until_parked();
        draw(cx);
        if is_ready(cx) {
            return;
        }
        std::thread::sleep(WAIT_STEP);
    }
    panic!("timed out waiting for {what}");
}

/// The fake agent, beside the binaries of this build.
pub(super) fn fake_agent() -> PathBuf {
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

/// A `box` whose `acp` is `acp` (a shell command), with `accounts` as its accounts.
pub(super) fn box_with(acp: &str, accounts: &str) -> (tempfile::TempDir, PathBuf) {
    let cases = format!(
        r#"  accounts) cat "$d/accounts.json" ;;
  acp) {acp} ;;
  shell|attach|login) exec cat ;;"#
    );
    let (dir, script) = fake_agentbox(Names::Listed, "0", &cases);
    std::fs::write(dir.path().join("accounts.json"), accounts).expect("the accounts");
    (dir, script)
}

/// A `box` whose `acp` runs the fake agent with `flags`, and one account signed in.
pub(super) fn agent_box(flags: &str) -> (tempfile::TempDir, PathBuf) {
    let agent = fake_agent();
    box_with(&format!("exec \"{}\" {flags}", agent.display()), SIGNED_IN)
}

/// The desk on `script` with the conversation of `task` drawn.
pub(super) fn open<'a>(
    cx: &'a mut TestAppContext,
    script: PathBuf,
    task: &str,
) -> (&'a mut VisualTestContext, Entity<Desk>) {
    let (cx, desk, _) = open_live(cx, script, Some(task), true);
    wait_for(cx, "the conversation", |cx| {
        cx.debug_bounds("thread-composer").is_some()
    });
    (cx, desk)
}

pub(super) fn conversation(
    cx: &mut VisualTestContext,
    desk: &Entity<Desk>,
    task: &str,
) -> Entity<ThreadView> {
    let kept = desk.read_with(cx, |desk, _| desk.conversation_of(task));
    kept.unwrap_or_else(|| panic!("{task} has no conversation"))
}

/// The request of the question [`ask_a_question`] puts in a conversation, and its choices as the card
/// names them: by their place in the agent's list.
pub(super) const QUESTION: [&str; 4] = [
    "thread-permission-card",
    "thread-permission-7-0",
    "thread-permission-7-2",
    "thread-permission-7-1",
];

/// Puts a prompt, the start of an answer and a question that waits for the person in the conversation
/// of `task`, as its agent would: the layout tests look at it on the sample data, where no agent runs.
pub(super) fn ask_a_question(cx: &mut VisualTestContext, desk: &Entity<Desk>, task: &str) {
    use sakura_acp::model::{OptionKind, PermissionOption, ToolCallFields, Update};
    use sakura_acp::policy::{Reason, Subject};
    use sakura_acp::thread::ThreadEvent;
    let option = |id: &str, kind| PermissionOption {
        id: id.to_string(),
        name: id.to_string(),
        kind,
    };
    let question = ThreadEvent::PermissionAsked {
        request: 7,
        tool: ToolCallFields {
            title: Some("Scarica".to_string()),
            ..ToolCallFields::new("fetch")
        },
        reason: Reason::Network,
        subject: Subject::default(),
        options: vec![
            option("allow", OptionKind::AllowOnce),
            option("always", OptionKind::AllowAlways),
            option("reject", OptionKind::RejectOnce),
        ],
    };
    let events = vec![
        ThreadEvent::Prompted("Sort the folders per nome.".to_string()),
        ThreadEvent::Update(Update::AgentText("Mi serve una library.".to_string())),
        question,
    ];
    let conversation = conversation(cx, desk, task);
    conversation.update(cx, |conversation, cx| conversation.apply(events, cx));
    wait_for(cx, "the question", |cx| is_on_screen(cx, QUESTION[0]));
}

/// Writes `text` in the composer of `task` and sends it, as Return does.
pub(super) fn say(cx: &mut VisualTestContext, desk: &Entity<Desk>, task: &str, text: &str) {
    let conversation = conversation(cx, desk, task);
    cx.update(|window, cx| {
        conversation.update(cx, |conversation, cx| {
            conversation.write(text, window, cx);
            conversation.send(window, cx);
        })
    });
}

fn phase(cx: &mut VisualTestContext, desk: &Entity<Desk>, task: &str) -> Phase {
    conversation(cx, desk, task).read_with(cx, |conversation, _| conversation.phase().clone())
}

fn turn(cx: &mut VisualTestContext, desk: &Entity<Desk>, task: &str) -> Turn {
    conversation(cx, desk, task).read_with(cx, |conversation, _| conversation.thread().turn())
}

/// Everything the agent answered in the conversation of `task`.
fn answered(cx: &mut VisualTestContext, desk: &Entity<Desk>, task: &str) -> String {
    conversation(cx, desk, task).read_with(cx, |conversation, _| {
        let entries = conversation.thread().entries().iter();
        let texts = entries.filter_map(|entry| match &entry.kind {
            EntryKind::Assistant(message) => Some(message.text.as_str()),
            _ => None,
        });
        texts.collect()
    })
}

/// Waits until the turn of `task` is over and the agent said `words`.
fn wait_for_answer(cx: &mut VisualTestContext, desk: &Entity<Desk>, task: &str, words: &str) {
    wait_for(cx, "the agent's answer", |cx| {
        turn(cx, desk, task) == Turn::Idle && answered(cx, desk, task).contains(words)
    });
}

/// How many times `box` was requested `command` (its whole line).
pub(super) fn asked(dir: &tempfile::TempDir, command: &str) -> usize {
    calls(dir).iter().filter(|call| *call == command).count()
}

/// The calls that start a process which stays: an agent, a terminal, a sign-in.
fn started(dir: &tempfile::TempDir) -> Vec<String> {
    let stays = |call: &String| {
        ["acp ", "attach ", "shell ", "login ", "agent "]
            .iter()
            .any(|command| call.starts_with(command))
    };
    calls(dir).into_iter().filter(stays).collect()
}

fn is_on_screen(cx: &mut VisualTestContext, selector: &'static str) -> bool {
    cx.debug_bounds(selector).is_some()
}

fn top(cx: &mut VisualTestContext, selector: &'static str) -> gpui::Pixels {
    cx.debug_bounds(selector).expect(selector).top()
}

#[gpui::test]
fn looking_at_a_lavoro_starts_nothing_and_the_first_message_starts_its_agent(
    cx: &mut TestAppContext,
) {
    let (dir, script) = agent_box(FAST);
    let (cx, desk) = open(cx, script, TASK);
    click(cx, "row-headings-26");
    click(cx, "row-sample-accent");
    assert_eq!(
        started(&dir),
        Vec::<String>::new(),
        "looking starts no process"
    );
    assert_eq!(phase(cx, &desk, TASK), Phase::Idle);

    say(cx, &desk, TASK, "plain");
    let acp = format!("acp {TASK} {ACCOUNT}");
    wait_for(cx, "the agent to start", |_| asked(&dir, &acp) == 1);
    wait_for_answer(cx, &desk, TASK, "normalization");
    assert_eq!(phase(cx, &desk, TASK), Phase::Ready);
    assert_eq!(started(&dir), [acp], "one agent, and nothing else");
}

in_every_language! {
fn an_agent_that_asks_raises_needs_you_once_in_each_place_and_the_answer_clears_it(cx: &mut TestAppContext) {
    let (_dir, script) = agent_box(FAST);
    let (cx, desk) = open(cx, script, TAKEN);
    let tab = desk.read_with(cx, |desk, cx| desk.strip.read(cx).tabs().active_id());
    let tab_mark: &'static str = format!("tab-needs-you-{}", tab.expect("a tab").as_u64()).leak();
    let state_at_rest = width(cx, "header-state");
    assert!(top(cx, "row-headings-26") > top(cx, "row-sample-accent"), "taken: listed last");
    assert!(!is_on_screen(cx, tab_mark));

    say(cx, &desk, TAKEN, "permission");
    wait_for(cx, "the question", |cx| is_on_screen(cx, "thread-permission-card"));
    let is_waiting = |cx: &mut VisualTestContext| {
        desk.read_with(cx, |desk, cx| desk.facts.read(cx).is_waiting(TAKEN))
    };
    assert!(is_waiting(cx));
    assert!(
        top(cx, "row-headings-26") < top(cx, "row-sample-accent"),
        "the sidebar lists it first among the ones to look at"
    );
    let needs_you = width_of(cx, Key("state.needs_you"), theme::TEXT_SMALL);
    assert!(
        (width(cx, "header-state") - needs_you).abs() < 1.0,
        "the header's state says it"
    );
    assert!(is_on_screen(cx, tab_mark), "and the tab's mark");

    let request = conversation(cx, &desk, TAKEN).read_with(cx, |conversation, _| {
        let entries = conversation.thread().entries().iter();
        let mut requests = entries.filter_map(|entry| match &entry.kind {
            EntryKind::Permission(permission) => Some(permission.request),
            _ => None,
        });
        requests.next().expect("the request")
    });
    // The first of the agent's options allows it once.
    let allow_once: &'static str = format!("thread-permission-{request}-0").leak();
    click(cx, allow_once);
    wait_for(cx, "the answer to be heard", |cx| !is_waiting(cx));
    wait_for_answer(cx, &desk, TAKEN, "Continuing.");
    assert!(!is_on_screen(cx, "thread-permission-card"));
    assert!(!is_on_screen(cx, tab_mark));
    assert!((width(cx, "header-state") - state_at_rest).abs() < 1.0);
    assert!(top(cx, "row-headings-26") > top(cx, "row-sample-accent"));
}
}

#[gpui::test]
fn stop_ends_the_turn_and_what_was_written_meanwhile_is_kept_for_the_next(cx: &mut TestAppContext) {
    let (dir, script) = agent_box("--delay-ms 20");
    let (cx, desk) = open(cx, script, TASK);
    say(cx, &desk, TASK, "slow");
    wait_for(cx, "the answer to stream", |cx| {
        !answered(cx, &desk, TASK).is_empty()
    });
    assert_eq!(turn(cx, &desk, TASK), Turn::Running);

    // The person writes the next message while the agent works: it waits in the composer.
    let view = conversation(cx, &desk, TASK);
    cx.update(|window, cx| view.update(cx, |view, cx| view.write("plain", window, cx)));
    assert!(!view.read_with(cx, |view, cx| view.can_send(cx)));

    click(cx, "thread-send");
    wait_for(cx, "the turn to stop", |cx| {
        turn(cx, &desk, TASK) == Turn::Idle
    });
    let stopped_at = answered(cx, &desk, TASK).len();
    assert_eq!(view.read_with(cx, |view, cx| view.draft(cx)), "plain");
    assert!(view.read_with(cx, |view, cx| view.can_send(cx)));

    cx.update(|window, cx| view.update(cx, |view, cx| view.send(window, cx)));
    wait_for_answer(cx, &desk, TASK, "normalization");
    assert!(answered(cx, &desk, TASK).len() > stopped_at);
    assert_eq!(started(&dir).len(), 1, "the same agent all along");
}

#[gpui::test]
fn an_agent_that_ends_offers_restart_and_restart_starts_a_new_one(cx: &mut TestAppContext) {
    let (dir, script) = agent_box(FAST);
    let (cx, desk) = open(cx, script, TASK);
    say(cx, &desk, TASK, "crash");
    wait_for(cx, "the agent to end", |cx| {
        matches!(phase(cx, &desk, TASK), Phase::Ended(_))
    });
    assert_eq!(turn(cx, &desk, TASK), Turn::Idle, "nothing is left running");
    assert!(is_on_screen(cx, "thread-restart"));
    let acp = format!("acp {TASK} {ACCOUNT}");
    assert_eq!(asked(&dir, &acp), 1, "it is not started again by itself");

    click(cx, "thread-restart");
    wait_for(cx, "the new agent", |cx| {
        asked(&dir, &acp) == 2 && phase(cx, &desk, TASK) == Phase::Ready
    });
    assert!(!is_on_screen(cx, "thread-restart"));
    say(cx, &desk, TASK, "plain");
    wait_for_answer(cx, &desk, TASK, "normalization");
}

#[gpui::test]
fn a_box_without_acp_says_so_once_and_offers_the_terminal(cx: &mut TestAppContext) {
    let usage = "echo 'usage: box <command> [args]' >&2; exit 2";
    let (dir, script) = box_with(usage, SIGNED_IN);
    let (cx, desk) = open(cx, script, TASK);
    say(cx, &desk, TASK, "wave");
    wait_for(cx, "the conversation to give up", |cx| {
        phase(cx, &desk, TASK) == Phase::NoConversation
    });
    assert!(is_on_screen(cx, "thread-terminal"));
    assert!(
        !is_on_screen(cx, "thread-restart"),
        "starting it again cannot help"
    );
    let problems = conversation(cx, &desk, TASK).read_with(cx, |conversation, _| {
        let entries = conversation.thread().entries().iter();
        let is_problem =
            |entry: &&sakura_acp::thread::Entry| matches!(entry.kind, EntryKind::Problem(_));
        entries.filter(is_problem).count()
    });
    assert_eq!(problems, 0, "the notice is the one place that says it");

    click(cx, "thread-terminal");
    let shell = format!("shell {TASK}");
    wait_for(cx, "the terminal", |cx| {
        asked(&dir, &shell) == 1 && is_on_screen(cx, "centre-view-terminal")
    });
}

#[gpui::test]
fn a_sign_in_needed_on_the_first_message_is_offered_for_the_account_the_agent_started_with(
    cx: &mut TestAppContext,
) {
    let (dir, script) = agent_box("--auth-on-prompt");
    let (cx, desk) = open(cx, script, TASK);
    say(cx, &desk, TASK, "plain");
    wait_for(cx, "the sign-in to be asked", |cx| {
        phase(cx, &desk, TASK) == Phase::SignIn
    });
    assert!(is_on_screen(cx, "thread-sign-in") && is_on_screen(cx, "thread-retry"));
    assert!(!is_on_screen(cx, "thread-restart"));

    click(cx, "thread-sign-in");
    let login = format!("login {ACCOUNT}");
    wait_for(cx, "the sign-in", |_| asked(&dir, &login) == 1);
}

/// Says in the answers of the fake `box` that `task` lives in a folder of the Mac.
pub(super) fn move_to_the_mac(dir: &Path, task: &str) {
    fn mark(value: &mut serde_json::Value, task: &str) {
        match value {
            serde_json::Value::Array(items) => items.iter_mut().for_each(|item| mark(item, task)),
            serde_json::Value::Object(fields) => {
                if fields.get("name").and_then(|name| name.as_str()) == Some(task) {
                    fields.insert("where".into(), "mac".into());
                    fields.insert("folder".into(), "/tmp/sakura-test-worktree".into());
                }
                fields.values_mut().for_each(|field| mark(field, task));
            }
            _ => {}
        }
    }
    for name in ["status.json", "tasks.json", &format!("facts-{task}.json")] {
        let file = dir.join(name);
        let text = std::fs::read_to_string(&file).expect("an answer");
        let mut answer = serde_json::from_str(&text).expect("JSON");
        mark(&mut answer, task);
        std::fs::write(&file, answer.to_string()).expect("the answer");
    }
}

#[gpui::test]
fn on_the_mac_the_agent_takes_the_only_login_an_expired_one_is_named_and_nothing_is_taken(
    cx: &mut TestAppContext,
) {
    let agent = fake_agent();
    let acp = format!("exec \"{}\" --auth-on-prompt", agent.display());
    let own = r#"[{"name":"mio","where":"mac","exists":true,"loggedIn":null}]"#;
    let (dir, script) = box_with(&acp, own);
    move_to_the_mac(dir.path(), TASK);
    let (cx, desk) = open(cx, script, TASK);
    assert!(
        !is_on_screen(cx, "take") && !is_on_screen(cx, "take-hold"),
        "a lavoro on the Mac is already where its work is"
    );
    click(cx, "row-sample-folders");
    assert!(is_on_screen(cx, "take-hold"), "one in a box is taken");
    click(cx, "row-sample-accent");

    say(cx, &desk, TASK, "plain");
    wait_for(cx, "the login to be found expired", |cx| {
        phase(cx, &desk, TASK) == Phase::SignIn
    });
    assert_eq!(
        asked(&dir, &format!("acp {TASK} mio")),
        1,
        "the only login of the Mac is the one the window shows, and the one it names"
    );
    assert!(is_on_screen(cx, "thread-sign-in-sentence"));
    assert!(
        !is_on_screen(cx, "thread-sign-in"),
        "Sakura cannot renew it"
    );
    assert!(is_on_screen(cx, "thread-retry"));
    assert!(!started(&dir).iter().any(|call| call.starts_with("login ")));
}

#[gpui::test]
fn each_lavoro_has_its_own_agent_and_coming_back_finds_its_conversation_as_it_was(
    cx: &mut TestAppContext,
) {
    let (dir, script) = agent_box(FAST);
    let (cx, desk) = open(cx, script, TASK);
    say(cx, &desk, TASK, "plain");
    wait_for_answer(cx, &desk, TASK, "normalization");
    let first = conversation(cx, &desk, TASK);
    let said = answered(cx, &desk, TASK);

    click(cx, "row-headings-26");
    wait_for(cx, "the other conversation", |cx| {
        desk.read_with(cx, |desk, _| desk.conversation_of(TAKEN).is_some())
    });
    assert!(answered(cx, &desk, TAKEN).is_empty(), "its own, and empty");
    say(cx, &desk, TAKEN, "think");
    wait_for(cx, "the second agent", |_| {
        asked(&dir, &format!("acp {TAKEN} {ACCOUNT}")) == 1
    });

    // Back on the first lavoro, then to its terminal and back to the agent: the same view, untouched.
    click(cx, "row-sample-accent");
    click(cx, "status-terminal");
    wait_for(cx, "the terminal", |cx| {
        is_on_screen(cx, "centre-view-terminal")
    });
    click(cx, "centre-view-agent");
    wait_for(cx, "the conversation again", |cx| {
        is_on_screen(cx, "thread-composer")
    });
    assert_eq!(conversation(cx, &desk, TASK), first);
    assert_eq!(answered(cx, &desk, TASK), said);
    assert_eq!(
        phase(cx, &desk, TASK),
        Phase::Ready,
        "its agent is still there"
    );
    let agents = started(&dir);
    let agents: Vec<&String> = agents
        .iter()
        .filter(|call| call.starts_with("acp "))
        .collect();
    assert_eq!(agents.len(), 2, "one agent per lavoro: {agents:?}");
}

/// Keeps `session` for `task` and the test's account, as an earlier run of the app would have.
fn kept(dir: &tempfile::TempDir, task: &str, session: &str) {
    let store = sakura_acp::store::SessionStore::new(dir.path());
    store.set(task, ACCOUNT, session).expect("the kept session");
}

/// Whether every entry of the conversation of `task` was replayed from its stored session.
fn is_all_replayed(cx: &mut VisualTestContext, desk: &Entity<Desk>, task: &str) -> bool {
    conversation(cx, desk, task).read_with(cx, |conversation, _| {
        let entries = conversation.thread().entries();
        !entries.is_empty() && entries.iter().all(|entry| entry.is_replayed)
    })
}

#[gpui::test]
fn a_kept_conversation_is_loaded_when_its_lavoro_is_shown_and_only_for_the_one_shown(
    cx: &mut TestAppContext,
) {
    let (dir, script) = agent_box(FAST);
    // The session the fake agent can replay, kept for two lavori by an earlier run.
    kept(&dir, TASK, "fake-session-1");
    kept(&dir, TAKEN, "fake-session-1");
    untouched(&dir, TASK);
    let (cx, desk) = open(cx, script, TASK);
    wait_for(cx, "the conversation to be shown again", |cx| {
        phase(cx, &desk, TASK) == Phase::Ready && !answered(cx, &desk, TASK).is_empty()
    });
    assert!(is_all_replayed(cx, &desk, TASK), "shown whole, at once");
    assert_eq!(
        started(&dir),
        [format!("acp {TASK} {ACCOUNT}")],
        "one process, of the lavoro shown: the one only listed starts nothing"
    );
    assert!(!is_on_screen(cx, "thread-resume"), "nothing to ask for");
    assert!(!is_on_screen(cx, "thread-kept-lost"));
    assert!(
        is_on_screen(cx, "state-header-idle"),
        "a conversation shown again is at rest"
    );

    click(cx, "row-headings-26");
    wait_for(cx, "the other conversation, now that it is shown", |cx| {
        desk.read_with(cx, |desk, _| desk.conversation_of(TAKEN).is_some())
            && phase(cx, &desk, TAKEN) == Phase::Ready
    });
    assert_eq!(started(&dir).len(), 2, "{:?}", started(&dir));
    click(cx, "row-sample-accent");
    cx.run_until_parked();
    assert_eq!(started(&dir).len(), 2, "coming back starts nothing again");
}

#[gpui::test]
fn a_kept_conversation_the_service_no_longer_has_is_said_in_one_line(cx: &mut TestAppContext) {
    let (dir, script) = agent_box(FAST);
    kept(&dir, TASK, "a-session-that-is-gone");
    let (cx, desk) = open(cx, script, TASK);
    wait_for(cx, "the new conversation", |cx| {
        phase(cx, &desk, TASK) == Phase::Ready
    });
    assert!(is_on_screen(cx, "thread-kept-lost"));
    conversation(cx, &desk, TASK).read_with(cx, |conversation, _| {
        assert!(
            conversation.thread().entries().is_empty(),
            "nothing is shown in its place"
        );
    });
    assert_eq!(started(&dir), [format!("acp {TASK} {ACCOUNT}")]);

    say(cx, &desk, TASK, "plain");
    wait_for_answer(cx, &desk, TASK, "normalization");
    assert!(
        !is_on_screen(cx, "thread-kept-lost"),
        "said once: the conversation went on"
    );
}

#[gpui::test]
fn an_agent_that_runs_in_a_terminal_keeps_it_and_one_sentence_says_so(cx: &mut TestAppContext) {
    let (dir, script) = agent_box(FAST);
    let (cx, _desk, _) = open_live(cx, script, Some(IN_TERMINAL), true);
    wait_for(cx, "the agent's terminal", |cx| {
        is_on_screen(cx, "agent-in-terminal")
    });
    assert!(
        !is_on_screen(cx, "thread-composer"),
        "one front end at a time"
    );
    let processes = started(&dir);
    assert_eq!(processes.len(), 1, "{processes:?}");
    assert!(processes[0].starts_with(&format!("attach {IN_TERMINAL}")));
}

/// Rewrites what the fake `box` says of `task` as a lavoro nothing was done in: no commit, no change, no
/// agent the service knows of, never taken, no result (a lavoro on the Mac before its first answer).
fn untouched(dir: &tempfile::TempDir, task: &str) {
    let path = dir.path().join("status.json");
    let text = std::fs::read_to_string(&path).expect("the status");
    let mut status: sakura_core::models::BoxStatus =
        serde_json::from_str(&text).expect("the status parses");
    let found = status.tasks.iter_mut().find(|listed| listed.name == task);
    let lavoro = found.expect("the lavoro");
    (lavoro.ahead, lavoro.dirty_files) = (Some(0), Some(0));
    (lavoro.taken, lavoro.agent) = (None, None);
    lavoro.results.clear();
    let facts = serde_json::to_string(lavoro).expect("the facts");
    std::fs::write(dir.path().join(format!("facts-{task}.json")), facts).expect("the facts");
    std::fs::write(path, serde_json::to_string(&status).expect("the status")).expect("the status");
}

/// Says what the conversation of `task` would say of its agent, and draws.
fn seen(
    cx: &mut VisualTestContext,
    desk: &Entity<Desk>,
    task: &str,
    (waits, live): (bool, sakura_core::model::Live),
) {
    desk.update(cx, |desk, cx| {
        desk.facts.update(cx, |facts, cx| {
            facts.set_waiting(task, waits, cx);
            facts.set_live(task, live, cx);
        });
    });
    cx.run_until_parked();
    draw(cx);
}

#[gpui::test]
fn the_header_the_sidebar_row_and_the_tab_show_one_state_and_the_group_follows_it(
    cx: &mut TestAppContext,
) {
    use sakura_core::model::Live;
    let (dir, script) = agent_box(FAST);
    untouched(&dir, TAKEN);
    let (cx, desk) = open(cx, script, TAKEN);
    let states = [
        ((false, Live::Unknown), "not-started", "group-idle"),
        ((false, Live::Starting), "preparing", "group-working"),
        ((false, Live::Working), "working", "group-working"),
        ((true, Live::Working), "needs-you", "group-review"),
        ((false, Live::Answered), "idle", "group-idle"),
        ((false, Live::Stopped), "failed", "group-review"),
    ];
    for (said, state, group) in states {
        seen(cx, &desk, TAKEN, said);
        for place in ["header", "row-headings-26", "tab-headings-26"] {
            let mark: &'static str = format!("state-{place}-{state}").leak();
            assert!(is_on_screen(cx, mark), "{place} does not show {state}");
        }
        let row = top(cx, "row-headings-26");
        let label = top(cx, group);
        assert!(label <= row, "{state}: the row is not under {group}");
        let lower = ["group-review", "group-working", "group-idle", "group-taken"];
        let between = lower.into_iter().filter(|other| *other != group);
        let is_under_another = between
            .filter_map(|other| cx.debug_bounds(other))
            .any(|other| other.top() > label && other.top() <= row);
        assert!(
            !is_under_another,
            "{state}: another group stands between {group} and the row"
        );
    }
}

#[gpui::test]
fn a_lavoro_whose_conversation_is_kept_from_before_reads_idle_not_as_never_started(
    cx: &mut TestAppContext,
) {
    let (dir, script) = agent_box(FAST);
    untouched(&dir, TAKEN);
    // Opened on another lavoro: this one is only listed, no conversation of it is drawn.
    let (cx, desk) = open(cx, script, TASK);
    assert!(is_on_screen(cx, "state-row-headings-26-not-started"));
    desk.update(cx, |desk, cx| {
        desk.sessions.store(TAKEN, ACCOUNT, "session-kept");
        desk.tell_states(cx);
    });
    cx.run_until_parked();
    draw(cx);
    assert!(is_on_screen(cx, "state-row-headings-26-idle"));
    assert_eq!(
        calls(&dir)
            .iter()
            .filter(|call| call.starts_with("acp"))
            .count(),
        0
    );
}

/// The project of the fixture's lavori.
const PROJECT: &str = "example-app";
/// Two logins of the Mac: neither is "the only one".
const TWO_OF_THE_MAC: &str = r#"[{"name":"mio","where":"mac","exists":true,"loggedIn":null},
    {"name":"other","where":"mac","exists":true,"loggedIn":null}]"#;

/// A `box` whose project names no account until `box project set` gives it one, with two logins of the
/// Mac and `task` living there. `set` is what its `project set` does besides.
fn box_without_a_project_account(task: &str, set: &str) -> (tempfile::TempDir, PathBuf) {
    let agent = fake_agent();
    let cases = format!(
        r#"  accounts) cat "$d/accounts.json" ;;
  acp) exec "{agent}" {FAST} ;;
  projects) cat "$d/projects.json" ;;
  project) {set} echo "[{{\"name\":\"{PROJECT}\",\"base\":\"main\",\"where\":\"mac\",\"account\":\"$5\"}}]" > "$d/projects.json" ;;
  shell|attach|login) exec cat ;;"#,
        agent = agent.display()
    );
    let (dir, script) = fake_agentbox(Names::Listed, "0", &cases);
    let write = |name: &str, text: &str| std::fs::write(dir.path().join(name), text).expect(name);
    write("accounts.json", TWO_OF_THE_MAC);
    let project = format!(r#"[{{"name":"{PROJECT}","base":"main","where":"mac"}}]"#);
    write("projects.json", &project);
    move_to_the_mac(dir.path(), task);
    (dir, script)
}

#[gpui::test]
fn a_lavoro_whose_project_names_no_account_asks_which_and_starts_with_the_one_chosen(
    cx: &mut TestAppContext,
) {
    let (dir, script) = box_without_a_project_account(TASK, "");
    let (cx, desk) = open(cx, script, TASK);
    say(cx, &desk, TASK, "plain");
    wait_for(cx, "the question which account", |cx| {
        is_on_screen(cx, "thread-account-other")
    });
    assert_eq!(phase(cx, &desk, TASK), Phase::NoAccount);
    assert!(is_on_screen(cx, "thread-no-account"));
    let can_stop = conversation(cx, &desk, TASK).read_with(cx, |view, _| view.can_stop());
    assert!(!can_stop, "nothing runs yet: there is nothing to stop");
    assert!(is_on_screen(cx, "thread-account-mio"));
    assert_eq!(started(&dir), Vec::<String>::new(), "no agent without one");

    click(cx, "thread-account-other");
    wait_for_answer(cx, &desk, TASK, "normalization");
    assert_eq!(
        asked(&dir, &format!("project set {PROJECT} --account other")),
        1
    );
    assert_eq!(
        started(&dir),
        [format!("acp {TASK} other")],
        "the agent starts once, with the account chosen, and the message that waited goes to it"
    );
    assert!(!is_on_screen(cx, "thread-no-account"));
}

#[gpui::test]
fn a_lavoro_whose_project_names_an_account_starts_with_it_among_several_logins(
    cx: &mut TestAppContext,
) {
    let (dir, script) = box_without_a_project_account(TASK, "");
    let project =
        format!(r#"[{{"name":"{PROJECT}","base":"main","where":"mac","account":"other"}}]"#);
    std::fs::write(dir.path().join("projects.json"), project).expect("the project");
    let (cx, desk) = open(cx, script, TASK);
    say(cx, &desk, TASK, "plain");
    wait_for_answer(cx, &desk, TASK, "normalization");
    assert_eq!(started(&dir), [format!("acp {TASK} other")]);
}

#[gpui::test]
fn an_account_the_service_does_not_take_is_said_in_one_line_with_its_words_under_details(
    cx: &mut TestAppContext,
) {
    let refusal = "echo 'box: no such project' >&2; exit 1;";
    let (dir, script) = box_without_a_project_account(TASK, refusal);
    let (cx, desk) = open(cx, script, TASK);
    say(cx, &desk, TASK, "plain");
    wait_for(cx, "the question which account", |cx| {
        is_on_screen(cx, "thread-account-mio")
    });
    click(cx, "thread-account-mio");
    wait_for(cx, "the refusal", |cx| {
        matches!(phase(cx, &desk, TASK), Phase::Ended(_))
    });
    assert!(is_on_screen(cx, "thread-problem-sentence"));
    assert!(
        !is_on_screen(cx, "thread-problem-detail"),
        "the service's own line waits under \"Details\""
    );
    assert_eq!(started(&dir), Vec::<String>::new());
}

#[gpui::test]
fn a_kept_conversation_of_a_lavoro_on_the_mac_is_loaded_with_its_projects_account(
    cx: &mut TestAppContext,
) {
    let (dir, script) = box_without_a_project_account(TASK, "");
    let project =
        format!(r#"[{{"name":"{PROJECT}","base":"main","where":"mac","account":"other"}}]"#);
    std::fs::write(dir.path().join("projects.json"), project).expect("the project");
    // On the Mac a lavoro's conversation is kept under no account's name.
    let store = sakura_acp::store::SessionStore::new(dir.path());
    store
        .set(TASK, "", "fake-session-1")
        .expect("the kept session");
    let (cx, desk) = open(cx, script, TASK);
    wait_for(cx, "the conversation to be shown again", |cx| {
        phase(cx, &desk, TASK) == Phase::Ready && !answered(cx, &desk, TASK).is_empty()
    });
    assert_eq!(
        started(&dir),
        [format!("acp {TASK} other")],
        "the projects are read for the lavoro shown, and name its account among two logins"
    );
    assert_eq!(asked(&dir, "projects --json"), 1);
    assert!(!is_on_screen(cx, "thread-no-account"));
}

#[gpui::test]
fn the_connecting_sign_stands_in_the_conversation_while_the_agent_connects_and_goes_when_it_has(
    cx: &mut TestAppContext,
) {
    let (_dir, script) = agent_box(FAST);
    let (cx, desk) = open(cx, script, TASK);
    let view = conversation(cx, &desk, TASK);
    let is_shown =
        |cx: &mut VisualTestContext| view.read_with(cx, |view, cx| view.is_connecting_shown(cx));
    assert!(
        !is_shown(cx),
        "nothing connects while a lavoro is only looked at"
    );
    assert!(!is_on_screen(cx, "thread-sign"));

    say(cx, &desk, TASK, "plain");
    draw(cx);
    assert_eq!(phase(cx, &desk, TASK), Phase::Connecting);
    assert!(is_shown(cx), "the sign loops while the agent connects");
    let sign = cx.debug_bounds("thread-sign").expect("the sign");
    let composer = cx.debug_bounds("thread-composer").expect("the composer");
    assert!(
        sign.bottom() <= composer.top(),
        "in the conversation's own area, above the composer and its status line"
    );

    wait_for_answer(cx, &desk, TASK, "normalization");
    draw(cx);
    assert!(
        !is_shown(cx),
        "the agent is connected: the sign has stopped"
    );
    assert!(!is_on_screen(cx, "thread-sign"), "and takes no room");
}
