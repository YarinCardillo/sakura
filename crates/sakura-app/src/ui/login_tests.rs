//! The sign-in driven in a test window on a fake `box`: asked from the conversation of a lavoro whose
//! agent has no account to start with, one that ends well and starts the agent with its account, one
//! that fails and says why, and one that is cancelled. Nobody types an account's name.

use super::console::login::LoginFailure;
use super::console::ConsoleState;
use super::desk::Desk;
use super::interaction_tests::{click, draw, wait_for};
use super::launch_tests::{calls, fake_agentbox, open_live, Names, TASK};
use gpui::{Entity, TestAppContext, VisualTestContext};
use std::path::PathBuf;

/// A `box` whose accounts are the file `accounts.json` beside it and whose `login` runs `login`.
fn login_box(login: &str) -> (tempfile::TempDir, PathBuf) {
    let cases = format!(
        r#"  accounts) cat "$d/accounts.json" 2>/dev/null || echo '[]' ;;
  login) {login} ;;"#
    );
    fake_agentbox(Names::Listed, "0", &cases)
}

/// The name the first sign-in of the boxes runs under.
const FIRST: &str = "claude";

/// The desk on `script`, the conversation of the task drawn.
fn open(cx: &mut TestAppContext, script: PathBuf) -> (&mut VisualTestContext, Entity<Desk>) {
    let (cx, desk, _) = open_live(cx, script, Some(TASK), true);
    wait_for(cx, "the conversation", |cx| {
        cx.debug_bounds("thread-composer").is_some()
    });
    (cx, desk)
}

/// Writes to the agent: with no account signed in, the conversation asks for a sign-in instead.
fn write_to_the_agent(cx: &mut VisualTestContext, desk: &Entity<Desk>) {
    let conversation = desk.read_with(cx, |desk, _| desk.conversation_of(TASK));
    let conversation = conversation.expect("the conversation");
    cx.update(|window, cx| {
        conversation.update(cx, |conversation, cx| {
            conversation.write("wave", window, cx);
            conversation.send(window, cx);
        })
    });
    wait_for(cx, "the sign-in to be asked", |cx| {
        cx.debug_bounds("thread-sign-in").is_some()
    });
}

/// Asks for the sign-in from the conversation.
fn sign_in(cx: &mut VisualTestContext, desk: &Entity<Desk>) {
    write_to_the_agent(cx, desk);
    click(cx, "thread-sign-in");
}

fn state(cx: &mut VisualTestContext, desk: &Entity<Desk>) -> Option<ConsoleState> {
    desk.read_with(cx, |desk, cx| {
        let console = desk.consoles().read(cx).get(TASK)?;
        Some(console.state().clone())
    })
}

fn is_on_screen(cx: &mut VisualTestContext, selector: &'static str) -> bool {
    cx.debug_bounds(selector).is_some()
}

#[gpui::test]
fn a_sign_in_that_ends_well_starts_the_agent_with_its_account(cx: &mut TestAppContext) {
    let signed_in = r#"printf '[{"name":"%s","loggedIn":true}]' "$2" > "$d/accounts.json"; exit 0"#;
    let (dir, script) = login_box(signed_in);
    let (cx, desk) = open(cx, script);
    sign_in(cx, &desk);
    let started = format!("acp {TASK} {FIRST}");
    wait_for(cx, "the agent to start with the account", |_| {
        calls(&dir).contains(&started)
    });
    assert!(calls(&dir).contains(&format!("login {FIRST}")));
    assert_eq!(state(cx, &desk), None, "the sign-in's terminal is put away");
    let asked = calls(&dir);
    let first_acp = asked.iter().position(|call| *call == started);
    let login = asked.iter().position(|call| call.starts_with("login "));
    assert!(login < first_acp, "no agent was started before the sign-in");
}

#[gpui::test]
fn a_login_that_fails_says_why_with_riprova_annulla_and_the_details(cx: &mut TestAppContext) {
    let refused = r#"echo 'opening the sign-in'; echo "box: no connection to the sign-in service; is the proxy running (box up)?" >&2; exit 1"#;
    let (dir, script) = login_box(refused);
    let (cx, desk) = open(cx, script);
    sign_in(cx, &desk);
    wait_for(cx, "the failure", |cx| is_on_screen(cx, "login-failed"));
    let Some(ConsoleState::LoginFailed { failure, .. }) = state(cx, &desk) else {
        panic!("the login failed");
    };
    assert_eq!(failure, LoginFailure::NoConnection);
    assert!(is_on_screen(cx, "login-retry") && is_on_screen(cx, "login-cancel"));
    assert!(
        is_on_screen(cx, "login-details"),
        "what it printed is kept below"
    );
    click(cx, "login-retry");
    wait_for(cx, "the second login", |_| {
        let logins = calls(&dir)
            .iter()
            .filter(|call| *call == "login claude")
            .count();
        logins == 2
    });
    wait_for(cx, "the second failure", |cx| {
        is_on_screen(cx, "login-failed")
    });
    click(cx, "login-cancel");
    assert_eq!(state(cx, &desk), None);
    assert!(
        is_on_screen(cx, "thread-sign-in"),
        "back to the conversation, which still asks for the sign-in"
    );
}

#[gpui::test]
fn a_login_that_ends_well_for_an_account_that_is_still_not_logged_in_says_so(
    cx: &mut TestAppContext,
) {
    let (_dir, script) = login_box("exit 0");
    let (cx, desk) = open(cx, script);
    sign_in(cx, &desk);
    wait_for(cx, "the check", |cx| is_on_screen(cx, "login-failed"));
    let Some(ConsoleState::LoginFailed { failure, .. }) = state(cx, &desk) else {
        panic!("the login failed");
    };
    assert_eq!(failure, LoginFailure::NotLoggedIn);
}

#[gpui::test]
fn a_running_login_says_what_is_happening_and_annulla_hangs_it_up(cx: &mut TestAppContext) {
    let (_dir, script) = login_box("sleep 20");
    let (cx, desk) = open(cx, script);
    sign_in(cx, &desk);
    wait_for(cx, "the sign-in", |cx| {
        state(cx, &desk) == Some(ConsoleState::Live)
    });
    assert!(is_on_screen(cx, "login-cancel"));
    assert!(!is_on_screen(cx, "login-retry"));
    click(cx, "login-cancel");
    assert_eq!(state(cx, &desk), None);
}

/// The address a login prints, as `claude auth login` writes it.
const SIGN_IN: &str = "https://claude.com/cai/oauth/authorize?code=true&client_id=9d1c250a-e61b-44d9-88ed-5944d1962f5e&response_type=code&redirect_uri=https%3A%2F%2Fplatform.claude.com%2Foauth%2Fcode%2Fcallback&scope=user%3Ainference&state=abc";

/// A login that prints `lines` (each one a line of its output) and then waits for a code.
fn printing(lines: &[&str]) -> String {
    let prints: Vec<String> = lines
        .iter()
        .map(|line| format!("printf '%s\\n' '{}'", line.replace('\'', r"'\''")))
        .collect();
    format!("{}; sleep 20", prints.join("; "))
}

/// Starts the sign-in and waits until its terminal shows `shown`.
fn start_login(cx: &mut VisualTestContext, desk: &Entity<Desk>, shown: &'static str) {
    sign_in(cx, desk);
    wait_for(cx, "the login's output", |cx| {
        desk.read_with(cx, |desk, cx| {
            let view = desk.consoles().read(cx).get(TASK).and_then(|c| c.view());
            view.is_some_and(|view| {
                let lines = view.read(cx).terminal().read(cx).logical_lines();
                lines.iter().any(|line| line.contains(shown))
            })
        })
    });
    for _ in 0..10 {
        cx.run_until_parked();
        draw(cx);
    }
}

#[gpui::test]
fn the_sign_in_address_a_login_prints_is_opened_once_in_the_browser(cx: &mut TestAppContext) {
    let second = "https://claude.ai/another?x=1";
    let login = printing(&[
        "Opening browser to sign in…",
        &format!("If the browser didn't open, visit: {SIGN_IN}"),
        &format!("or visit: {second}"),
        "Paste code here if prompted >",
    ]);
    let (_dir, script) = login_box(&login);
    let (cx, desk) = open(cx, script);
    assert_eq!(cx.opened_url(), None);
    start_login(cx, &desk, "Paste code here");
    assert_eq!(
        cx.opened_url().as_deref(),
        Some(SIGN_IN),
        "exactly that address"
    );
    let opened = desk.read_with(cx, |desk, cx| {
        let console = desk.consoles().read(cx).get(TASK).expect("the login");
        console.sign_in().map(str::to_string)
    });
    assert_eq!(
        opened.as_deref(),
        Some(SIGN_IN),
        "the second one is never opened"
    );
    assert!(is_on_screen(cx, "login-reopen") && is_on_screen(cx, "login-cancel"));
    click(cx, "login-reopen");
    assert_eq!(
        cx.opened_url().as_deref(),
        Some(SIGN_IN),
        "Apri di nuovo opens the same"
    );
}

#[gpui::test]
fn an_address_that_is_not_a_sign_in_one_is_never_opened_by_the_app(cx: &mut TestAppContext) {
    let login = printing(&[
        "visit: https://claude.com.evil.test/cai/oauth/authorize",
        "visit: https://evil.test/claude.com",
        "visit: http://claude.com/cai/oauth/authorize",
        "visit: https://user@claude.com/cai/oauth/authorize",
        "visit: file:///etc/passwd or javascript:alert(1)",
        "Paste code here if prompted >",
    ]);
    let (_dir, script) = login_box(&login);
    let (cx, desk) = open(cx, script);
    start_login(cx, &desk, "Paste code here");
    assert_eq!(cx.opened_url(), None);
    assert!(!is_on_screen(cx, "login-reopen"));
}

#[gpui::test]
fn a_sign_in_address_printed_by_a_console_that_is_not_a_login_is_never_opened(
    cx: &mut TestAppContext,
) {
    let cases = format!("  shell) printf 'visit: %s\\n' '{SIGN_IN}'; exec cat ;;");
    let (_dir, script) = fake_agentbox(Names::Listed, "0", &cases);
    let (cx, desk, _) = open_live(cx, script, Some(TASK), true);
    wait_for(cx, "the page", |cx| is_on_screen(cx, "thread-composer"));
    cx.update(|window, cx| desk.update(cx, |desk, cx| desk.toggle_terminal(TASK, window, cx)));
    wait_for(cx, "the shell's output", |cx| {
        desk.read_with(cx, |desk, cx| {
            let view = desk
                .consoles()
                .read(cx)
                .terminal(TASK)
                .and_then(|c| c.view());
            view.is_some_and(|view| !view.read(cx).terminal().read(cx).logical_lines().is_empty())
        })
    });
    for _ in 0..10 {
        cx.run_until_parked();
        draw(cx);
    }
    assert_eq!(cx.opened_url(), None);
}

#[gpui::test]
fn cmd_click_opens_an_http_address_in_any_terminal_and_no_other_kind(cx: &mut TestAppContext) {
    let cases = "  shell) printf 'docs https://example.com/a file:///etc/passwd\\n'; exec cat ;;";
    let (_dir, script) = fake_agentbox(Names::Listed, "0", cases);
    let (cx, desk, _) = open_live(cx, script, Some(TASK), true);
    wait_for(cx, "the page", |cx| is_on_screen(cx, "thread-composer"));
    cx.update(|window, cx| desk.update(cx, |desk, cx| desk.toggle_terminal(TASK, window, cx)));
    let view = |cx: &mut VisualTestContext| {
        desk.read_with(cx, |desk, cx| {
            desk.consoles()
                .read(cx)
                .terminal(TASK)
                .and_then(|c| c.view().cloned())
        })
    };
    wait_for(cx, "the shell's output", |cx| {
        view(cx).is_some_and(|view| {
            let lines = view.read_with(cx, |view, cx| view.terminal().read(cx).logical_lines());
            lines.iter().any(|line| line.contains("docs"))
        })
    });
    let view = view(cx).expect("the shell");
    let on_the_file = view.update(cx, |view, cx| view.open_link_at(0, 30, cx));
    assert!(!on_the_file, "a file address is no link");
    assert_eq!(cx.opened_url(), None);
    let on_a_word = view.update(cx, |view, cx| view.open_link_at(0, 1, cx));
    assert!(!on_a_word);
    let on_the_address = view.update(cx, |view, cx| view.open_link_at(0, 10, cx));
    assert!(on_the_address);
    assert_eq!(cx.opened_url().as_deref(), Some("https://example.com/a"));
}

#[gpui::test]
fn the_sign_in_is_of_the_account_the_boxes_already_have_when_it_is_not_signed_in(
    cx: &mut TestAppContext,
) {
    let (dir, script) = login_box("sleep 20");
    let accounts = r#"[{"name":"mio","where":"mac","exists":true,"loggedIn":null},{"name":"lavoro","loggedIn":false}]"#;
    std::fs::write(dir.path().join("accounts.json"), accounts).expect("the accounts");
    let (cx, desk) = open(cx, script);
    sign_in(cx, &desk);
    wait_for(cx, "the login of lavoro", |_| {
        calls(&dir).contains(&"login lavoro".to_string())
    });
    assert!(matches!(
        state(cx, &desk),
        Some(ConsoleState::Opening | ConsoleState::Live)
    ));
    let asked = calls(&dir);
    assert!(
        !asked.iter().any(|call| call.starts_with("acp ")),
        "no agent is started without an account: {asked:?}"
    );
}
