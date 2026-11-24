//! The ChatGPT login of Codex over its app-server (`initialize`, `account/login/start`, the notification
//! `account/login/completed`, `account/login/cancel`) against the fake app-server: the address to open comes
//! first, then how the login ended, and no terminal window is involved.

use sakura_acp::connection::{
    start_codex_login, CodexLoginConfig, LoginEnd, LoginEvent, LoginFailure,
};
use std::time::Duration;
use tokio::sync::mpsc::Receiver;

const PATIENCE: Duration = Duration::from_secs(20);

fn config(home: &std::path::Path, how: &str, log: &std::path::Path) -> CodexLoginConfig {
    CodexLoginConfig {
        command: vec![env!("CARGO_BIN_EXE_sakura-fake-codex").to_string()],
        env: vec![
            ("CODEX_HOME".into(), home.display().to_string()),
            ("SAKURA_FAKE_LOGIN".into(), how.into()),
            ("SAKURA_FAKE_LOG".into(), log.display().to_string()),
        ],
        env_allow: None,
        unset_env: Vec::new(),
        patience: PATIENCE,
    }
}

async fn next(events: &mut Receiver<LoginEvent>) -> LoginEvent {
    tokio::time::timeout(PATIENCE, events.recv())
        .await
        .expect("an event in time")
        .expect("the login still reports")
}

fn sent(log: &std::path::Path) -> Vec<String> {
    let text = std::fs::read_to_string(log).unwrap_or_default();
    text.lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter_map(|message| message["method"].as_str().map(str::to_string))
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_login_gives_the_address_to_open_and_then_ends_done_with_the_login_in_the_home() {
    let folder = tempfile::tempdir().expect("a folder");
    let log = folder.path().join("log");
    let (_login, mut events) = start_codex_login(config(folder.path(), "success", &log));
    assert_eq!(
        next(&mut events).await,
        LoginEvent::Browser("https://auth.example.test/login?state=abc".into())
    );
    assert_eq!(next(&mut events).await, LoginEvent::Ended(LoginEnd::Done));
    assert!(folder.path().join("auth.json").is_file());
    let methods = sent(&log);
    assert_eq!(
        methods,
        ["initialize", "initialized", "account/login/start"]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_login_the_server_says_did_not_succeed_ends_failed_with_its_reason() {
    let folder = tempfile::tempdir().expect("a folder");
    let log = folder.path().join("log");
    let (_login, mut events) = start_codex_login(config(folder.path(), "fail", &log));
    next(&mut events).await;
    assert_eq!(
        next(&mut events).await,
        LoginEvent::Ended(LoginEnd::Failed(LoginFailure::Declined(Some(
            "the browser window was closed".into()
        ))))
    );
    assert!(!folder.path().join("auth.json").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_login_that_is_cancelled_tells_the_server_and_ends_cancelled() {
    let folder = tempfile::tempdir().expect("a folder");
    let log = folder.path().join("log");
    let (login, mut events) = start_codex_login(config(folder.path(), "hold", &log));
    assert!(matches!(next(&mut events).await, LoginEvent::Browser(_)));
    login.cancel();
    assert_eq!(
        next(&mut events).await,
        LoginEvent::Ended(LoginEnd::Cancelled)
    );
    assert!(sent(&log).contains(&"account/login/cancel".to_string()));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_login_nobody_finishes_ends_when_the_patience_is_out() {
    let folder = tempfile::tempdir().expect("a folder");
    let log = folder.path().join("log");
    let mut waiting = config(folder.path(), "hold", &log);
    waiting.patience = Duration::from_millis(300);
    let (_login, mut events) = start_codex_login(waiting);
    assert!(matches!(next(&mut events).await, LoginEvent::Browser(_)));
    assert_eq!(
        next(&mut events).await,
        LoginEvent::Ended(LoginEnd::Failed(LoginFailure::TimedOut))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_that_refuses_the_login_ends_it_failed_with_what_it_said() {
    let folder = tempfile::tempdir().expect("a folder");
    let log = folder.path().join("log");
    let mut refused = config(folder.path(), "success", &log);
    refused
        .env
        .push(("SAKURA_FAKE_REFUSE".into(), "account/login/start".into()));
    let (_login, mut events) = start_codex_login(refused);
    assert_eq!(
        next(&mut events).await,
        LoginEvent::Ended(LoginEnd::Failed(LoginFailure::Refused(
            "account/login/start refused".into()
        )))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_program_that_cannot_start_ends_the_login_failed_not_started() {
    let folder = tempfile::tempdir().expect("a folder");
    let mut missing = config(folder.path(), "success", &folder.path().join("log"));
    missing.command = vec!["/nonexistent/codex".into()];
    let (_login, mut events) = start_codex_login(missing);
    assert!(matches!(
        next(&mut events).await,
        LoginEvent::Ended(LoginEnd::Failed(LoginFailure::NotStarted(_)))
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_address_that_is_not_https_is_never_handed_on_to_open() {
    let folder = tempfile::tempdir().expect("a folder");
    let mut other = config(folder.path(), "hold", &folder.path().join("log"));
    other.env.push((
        "SAKURA_FAKE_LOGIN_URL".into(),
        "file:///Applications/Calculator.app".into(),
    ));
    let (_login, mut events) = start_codex_login(other);
    assert_eq!(
        next(&mut events).await,
        LoginEvent::Ended(LoginEnd::Failed(LoginFailure::Broke))
    );
}
