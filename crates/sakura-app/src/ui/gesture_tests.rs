//! The whole gesture in a test window, on a fake `box` whose `acp` runs the fake agent: Cmd+N, a sentence,
//! Enter. One `box new`, the lavoro's tab with the person's message at once, then one `box acp` and the
//! answer streamed; a `box new` that fails is said in place with "Try again" and the message is kept; a
//! lavoro that takes long to prepare shows its tab and its message at once all the same.

use super::desk::Desk;
use super::interaction_tests::{click, draw, take_place};
use super::launch_tests::{calls, fake_agentbox, open_live, Names, TASK};
use super::new_task_tests::open_with_the_key;
use super::thread::{Login, Phase, ThreadView};
use gpui::{Entity, TestAppContext, VisualTestContext};
use sakura_acp::thread::{EntryKind, Turn};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const STATUS: &str = include_str!("../../../sakura-core/tests/fixtures/status.json");
/// What the person writes: its first word picks the fake agent's plain answer.
const SENTENCE: &str = "plain sort the folders";
/// The lavoro's name, made from the sentence.
const NAME: &str = "plain-sort-folders";
const PROJECT: &str = "demo-site";
const PROJECTS: &str = r#"[{"name":"demo-site","base":"main","where":"mac","account":"personal"}]"#;
const ACCOUNTS: &str = r#"[{"name":"personal","where":"mac","exists":true,"loggedIn":true}]"#;
/// A word of the fake agent's plain answer.
const ANSWER: &str = "normalization";

const WAIT_STEP: Duration = Duration::from_millis(10);
/// A frame of the test clock: the conversation takes the agent's events once per frame, on a timer.
const FRAME: Duration = Duration::from_millis(20);
/// How long after Enter the tab and the message may take to be on screen.
const SHOWN_WITHIN: Duration = Duration::from_secs(1);

fn wait_for(
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

/// What `box` answers once the lavoro is made: it is listed, on the Mac, with its folder.
fn write_the_lavoro(dir: &Path) {
    let mut status: serde_json::Value = serde_json::from_str(STATUS).expect("the fixture");
    let tasks = status["tasks"].as_array_mut().expect("the tasks");
    let mut made = tasks[2].clone();
    made["name"] = NAME.into();
    made["profile"] = PROJECT.into();
    made["where"] = "mac".into();
    made["folder"] = dir
        .join("work")
        .join(NAME)
        .to_string_lossy()
        .into_owned()
        .into();
    made["taken"] = serde_json::Value::Null;
    tasks.push(made.clone());
    let names: Vec<serde_json::Value> = tasks
        .iter()
        .map(|task| {
            serde_json::json!({"name": task["name"], "profile": task["profile"], "base": task["base"], "taken": null})
        })
        .collect();
    let write = |name: &str, value: &serde_json::Value| {
        std::fs::write(dir.join(name), value.to_string()).expect("an answer")
    };
    write("made-status.json", &status);
    write("made-tasks.json", &serde_json::Value::from(names));
    write("made-facts.json", &made);
}

/// What `box new` does when it goes well, after `before` (a shell command).
fn made_after(before: &str) -> String {
    format!(
        r#"echo 'making the worktree'; {before}; cp "$d/made-status.json" "$d/status.json"; cp "$d/made-tasks.json" "$d/tasks.json"; cp "$d/made-facts.json" "$d/facts-$2.json""#
    )
}

/// A `box` with one project on the Mac, whose `new` runs `new` and whose `acp` runs the fake agent.
fn gesture_box(new: &str) -> (tempfile::TempDir, PathBuf) {
    gesture_box_with(new, "--delay-ms 2")
}

/// The same `box`, its fake agent started with `flags`.
fn gesture_box_with(new: &str, flags: &str) -> (tempfile::TempDir, PathBuf) {
    let agent = fake_agent();
    let cases = format!(
        r#"  projects) cat "$d/projects.json" ;;
  branches) echo '{{"project":"demo-site","default":"main","branches":[{{"name":"main"}}]}}' ;;
  accounts) cat "$d/accounts.json" ;;
  new) {new} ;;
  acp) exec "{}" {flags} ;;
  shell|attach|login) exec cat ;;"#,
        agent.display()
    );
    let (dir, script) = fake_agentbox(Names::Listed, "0", &cases);
    std::fs::write(dir.path().join("projects.json"), PROJECTS).expect("the projects");
    std::fs::write(dir.path().join("accounts.json"), ACCOUNTS).expect("the accounts");
    write_the_lavoro(dir.path());
    (dir, script)
}

/// Cmd+N, the sentence, Enter.
fn ask(cx: &mut VisualTestContext, desk: &Entity<Desk>) {
    let sheet = open_with_the_key(cx, desk);
    wait_for(cx, "the project to be offered", |cx| {
        sheet.read_with(cx, |sheet, cx| sheet.chosen(cx)[0].is_some())
    });
    cx.simulate_input(SENTENCE);
    cx.simulate_keystrokes("enter");
    draw(cx);
}

fn conversation(cx: &mut VisualTestContext, desk: &Entity<Desk>) -> Option<Entity<ThreadView>> {
    desk.read_with(cx, |desk, _| desk.conversation_of(NAME))
}

/// The person's messages in the conversation of the lavoro.
fn messages(cx: &mut VisualTestContext, desk: &Entity<Desk>) -> Vec<String> {
    let Some(conversation) = conversation(cx, desk) else {
        return Vec::new();
    };
    conversation.read_with(cx, |conversation, _| {
        let entries = conversation.thread().entries().iter();
        let said = entries.filter_map(|entry| match &entry.kind {
            EntryKind::User(text) => Some(text.clone()),
            _ => None,
        });
        said.collect()
    })
}

/// Everything the agent answered.
fn answered(cx: &mut VisualTestContext, desk: &Entity<Desk>) -> String {
    let conversation = conversation(cx, desk).expect("the conversation");
    conversation.read_with(cx, |conversation, _| {
        let entries = conversation.thread().entries().iter();
        let texts = entries.filter_map(|entry| match &entry.kind {
            EntryKind::Assistant(message) => Some(message.text.as_str()),
            _ => None,
        });
        texts.collect()
    })
}

fn phase(cx: &mut VisualTestContext, desk: &Entity<Desk>) -> Phase {
    let conversation = conversation(cx, desk).expect("the conversation");
    conversation.read_with(cx, |conversation, _| conversation.phase().clone())
}

/// How many times `box` was requested a command that starts with `start`.
fn asked(dir: &tempfile::TempDir, start: &str) -> usize {
    let asked = calls(dir);
    asked.iter().filter(|call| call.starts_with(start)).count()
}

fn is_on_screen(cx: &mut VisualTestContext, selector: &'static str) -> bool {
    cx.debug_bounds(selector).is_some()
}

fn shown(cx: &mut VisualTestContext, desk: &Entity<Desk>) -> Option<String> {
    desk.read_with(cx, |desk, cx| desk.shown(cx))
}

#[gpui::test]
fn a_sentence_and_enter_make_the_lavoro_and_its_agent_answers(cx: &mut TestAppContext) {
    let (dir, script) = gesture_box(&made_after("sleep 0.3"));
    let (cx, desk, _) = open_live(cx, script, Some(TASK), true);
    ask(cx, &desk);

    assert_eq!(
        shown(cx, &desk).as_deref(),
        Some(NAME),
        "its tab opens at once"
    );
    assert_eq!(messages(cx, &desk), [SENTENCE], "with the person's message");
    assert!(is_on_screen(cx, "thread-prompt"), "on screen");
    assert_eq!(phase(cx, &desk), Phase::Preparing);
    assert_eq!(
        asked(&dir, "acp "),
        0,
        "no agent before the lavoro is there"
    );

    wait_for(cx, "the agent's answer", |cx| {
        let conversation = conversation(cx, &desk).expect("the conversation");
        let turn = conversation.read_with(cx, |conversation, _| conversation.thread().turn());
        turn == Turn::Idle && answered(cx, &desk).contains(ANSWER)
    });
    let new = format!("new {NAME} {PROJECT} --base main --where mac");
    assert_eq!(asked(&dir, "new "), 1, "one box new");
    assert!(
        calls(&dir).contains(&new),
        "with the project, the base and the place"
    );
    assert_eq!(
        asked(&dir, "project set"),
        0,
        "the project's own account is not set again"
    );
    assert_eq!(asked(&dir, "acp "), 1, "one agent");
    assert!(
        calls(&dir).contains(&format!("acp {NAME} personal")),
        "with the project's account, named"
    );
    assert_eq!(
        messages(cx, &desk),
        [SENTENCE],
        "the sentence was sent once"
    );
    assert_eq!(
        answered(cx, &desk).matches(ANSWER).count(),
        1,
        "and answered once"
    );
    assert_eq!(phase(cx, &desk), Phase::Ready);
    assert!(
        is_on_screen(cx, "row-plain-sort-folders"),
        "the lavoro is listed"
    );
    let waiting = desk.read_with(cx, |desk, _| desk.creating.len());
    assert_eq!(waiting, 0, "nothing waits any more");
}

#[gpui::test]
fn a_creation_that_fails_is_said_in_place_and_try_again_keeps_the_message(cx: &mut TestAppContext) {
    // The first `box new` fails as the service does when the branch is there; the second goes well.
    let new = format!(
        r#"if [ ! -f "$d/tried" ]; then touch "$d/tried"; echo "box: the branch sakura/$2 already exists in /repo: choose another name for the lavoro, or remove that branch yourself" >&2; exit 1; fi; {}"#,
        made_after("true")
    );
    let (dir, script) = gesture_box(&new);
    let (cx, desk, _) = open_live(cx, script, Some(TASK), true);
    ask(cx, &desk);
    wait_for(cx, "the failure", |cx| {
        is_on_screen(cx, "thread-creation-failed")
    });
    assert_eq!(
        shown(cx, &desk).as_deref(),
        Some(NAME),
        "said in the lavoro's place"
    );
    assert_eq!(messages(cx, &desk), [SENTENCE], "the message is not lost");
    assert!(is_on_screen(cx, "thread-retry"), "with Try again");
    assert!(
        !is_on_screen(cx, "thread-details-text"),
        "the raw line is not shown"
    );
    click(cx, "thread-details");
    assert!(
        is_on_screen(cx, "thread-details-text"),
        "until Details is requested"
    );
    assert_eq!(
        asked(&dir, "acp "),
        0,
        "no agent for a lavoro that is not there"
    );

    click(cx, "thread-retry");
    wait_for(cx, "the agent's answer", |cx| {
        answered(cx, &desk).contains(ANSWER)
    });
    assert_eq!(asked(&dir, "new "), 2, "asked again once");
    assert_eq!(asked(&dir, "acp "), 1);
    assert_eq!(messages(cx, &desk), [SENTENCE], "and sent once");
    assert!(!is_on_screen(cx, "thread-creation-failed"));
}

#[gpui::test]
fn a_lavoro_that_takes_ten_seconds_to_prepare_shows_its_tab_and_message_within_a_second(
    cx: &mut TestAppContext,
) {
    take_place(true);
    let (dir, script) = gesture_box(&made_after("sleep 10"));
    let (cx, desk, _) = open_live(cx, script, Some(TASK), true);
    let sheet = open_with_the_key(cx, &desk);
    wait_for(cx, "the project to be offered", |cx| {
        sheet.read_with(cx, |sheet, cx| sheet.chosen(cx)[0].is_some())
    });
    cx.simulate_input(SENTENCE);
    let pressed = Instant::now();
    cx.simulate_keystrokes("enter");
    wait_for(cx, "the tab and the message", |cx| {
        shown(cx, &desk).as_deref() == Some(NAME) && is_on_screen(cx, "thread-prompt")
    });
    let waited = pressed.elapsed();
    assert!(waited < SHOWN_WITHIN, "shown after {waited:?}");
    wait_for(cx, "the service's progress line", |cx| {
        is_on_screen(cx, "row-plain-sort-folders") && asked(&dir, "new ") == 1
    });
    assert_eq!(
        phase(cx, &desk),
        Phase::Preparing,
        "it waits for the lavoro"
    );
    assert!(is_on_screen(cx, "thread-status"), "with one quiet line");
    assert_eq!(asked(&dir, "acp "), 0, "and starts no agent meanwhile");
}

#[gpui::test]
fn a_login_of_the_mac_that_the_agent_refuses_is_named_by_the_projects_account(
    cx: &mut TestAppContext,
) {
    let (dir, script) = gesture_box_with(&made_after("true"), "--auth-required");
    // Two logins on this Mac: only the project says which one the lavoro uses.
    let two = r#"[{"name":"personal","where":"mac","exists":true,"loggedIn":true},{"name":"other","where":"mac","exists":true,"loggedIn":true}]"#;
    std::fs::write(dir.path().join("accounts.json"), two).expect("the accounts");
    let (cx, desk, _) = open_live(cx, script, Some(TASK), true);
    ask(cx, &desk);
    wait_for(cx, "the sign-in to be requested", |cx| {
        phase(cx, &desk) == Phase::SignIn
    });
    assert_eq!(messages(cx, &desk), [SENTENCE], "the message stays");
    assert!(is_on_screen(cx, "thread-sign-in-sentence"));
    let conversation = conversation(cx, &desk).expect("the conversation");
    let login = conversation.read_with(cx, |conversation, cx| conversation.login(cx));
    let own = Login::Mac {
        account: Some("personal".to_string()),
    };
    assert_eq!(login, own, "the sentence names the project's account");
}
