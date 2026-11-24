//! Adding an account as a person meets it: the program and a name, then a new login (the engine makes the folder,
//! its command opens in a terminal, the person logs in there and the account is added) or a login folder they
//! already have. A fake engine answers the two actions; the terminal is a recorder.

use super::account_add::{AddAccount, AddAccountEvent, Stage};
use super::engine_rig_tests::{open, wait_until, Rig, SNAPSHOT};
use super::lane_tests::click;
use super::master_agent::MasterAgent;
use gpui::{AppContext as _, Entity, TestAppContext, VisualTestContext};
use std::cell::RefCell;
use std::rc::Rc;

const FOLDER: &str = "/Users/person/.claude-2";
const COMMAND: &str = "env CLAUDE_CONFIG_DIR=/Users/person/.claude-2 claude auth login";

/// What the engine answers to `g`: the folder it made and the command to log in.
fn made() -> String {
    let output = serde_json::json!({
        "provider": "claude", "folder": FOLDER, "command": COMMAND,
        "argv": ["claude", "auth", "login"], "env": {"CLAUDE_CONFIG_DIR": FOLDER},
    });
    serde_json::json!({"result": "done", "exit_code": 0, "output": output.to_string()}).to_string()
}

/// What the engine answers to `j`: the account it added.
fn added() -> String {
    let output = serde_json::json!({"id": "claude-2", "provider": "claude", "label": "Studio"});
    serde_json::json!({"result": "done", "exit_code": 0, "output": output.to_string()}).to_string()
}

fn refused(reason: &str) -> String {
    serde_json::json!({"result": "refused", "reason": reason}).to_string()
}

struct Sheet {
    rig: Rig,
    sheet: Entity<AddAccount>,
    /// The lines the sheet asked the terminal to run.
    terminal: Rc<RefCell<Vec<String>>>,
    /// What the sheet said happened: added or cancelled.
    events: Rc<RefCell<Vec<AddAccountEvent>>>,
}

fn open_sheet(cx: &mut TestAppContext, terminal_works: bool) -> (Sheet, &mut VisualTestContext) {
    let rig = open(cx, SNAPSHOT, &made(), &added());
    let (source, facts) = (rig.source.clone(), rig.facts.clone());
    let terminal = Rc::new(RefCell::new(Vec::new()));
    let ran = terminal.clone();
    let slot = Rc::new(RefCell::new(None));
    let kept = slot.clone();
    let (_, cx) = cx.add_window_view(move |window, cx| {
        let opener: super::account_add::TerminalOpener = Rc::new(move |line: &str, _| {
            ran.borrow_mut().push(line.to_string());
            gpui::Task::ready(match terminal_works {
                true => Ok(()),
                false => Err(std::io::Error::other("no terminal")),
            })
        });
        let sheet = cx.new(|cx| AddAccount::new(source, facts, opener, window, cx));
        *kept.borrow_mut() = Some(sheet.clone());
        gpui_component::Root::new(sheet, window, cx)
    });
    let sheet = slot.borrow_mut().take().expect("the sheet");
    let events = Rc::new(RefCell::new(Vec::new()));
    let seen = events.clone();
    cx.update(|_, cx| {
        cx.subscribe(&sheet, move |_, event: &AddAccountEvent, _| {
            seen.borrow_mut().push(event.clone())
        })
        .detach()
    });
    (
        Sheet {
            rig,
            sheet,
            terminal,
            events,
        },
        cx,
    )
}

fn name_it(sheet: &Sheet, name: &str, cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        sheet
            .sheet
            .update(cx, |sheet, cx| sheet.set_name(name, window, cx))
    });
}

#[gpui::test]
fn a_new_claude_login_asks_the_engine_for_a_folder_and_opens_its_command_in_a_terminal(
    cx: &mut TestAppContext,
) {
    let (shown, cx) = open_sheet(cx, true);
    name_it(&shown, "Studio", cx);
    shown.sheet.update(cx, |sheet, cx| {
        sheet.set_agent(MasterAgent::Claude, cx);
        sheet.log_in_new(cx)
    });
    assert_eq!(
        shown.sheet.read_with(cx, |s, _| s.stage().clone()),
        Stage::Confirming
    );
    assert!(
        shown.rig.acts().is_empty(),
        "nothing is made before the person confirms"
    );
    assert!(shown.terminal.borrow().is_empty());
    shown.sheet.update(cx, |sheet, cx| sheet.confirm_login(cx));
    wait_until(cx, "the folder", |cx| {
        shown
            .sheet
            .read_with(cx, |sheet, _| matches!(sheet.stage(), Stage::Waiting(_)))
    });
    assert_eq!(*shown.terminal.borrow(), [COMMAND]);
    let acts = shown.rig.acts();
    assert!(
        acts[0].contains(" g ") && acts[0].contains("--provider claude"),
        "{acts:?}"
    );
    cx.run_until_parked();
    let stage = shown.sheet.read_with(cx, |sheet, _| sheet.stage().clone());
    assert_eq!(stage, Stage::Waiting(FOLDER.to_string()));
}

#[gpui::test]
fn after_the_login_the_account_is_added_with_the_folder_the_engine_made_and_the_name_typed(
    cx: &mut TestAppContext,
) {
    let (shown, cx) = open_sheet(cx, true);
    name_it(&shown, "Studio", cx);
    shown.sheet.update(cx, |sheet, cx| {
        sheet.set_agent(MasterAgent::Claude, cx);
        sheet.log_in_new(cx)
    });
    shown.sheet.update(cx, |sheet, cx| sheet.confirm_login(cx));
    wait_until(cx, "the folder", |cx| {
        shown
            .sheet
            .read_with(cx, |sheet, _| matches!(sheet.stage(), Stage::Waiting(_)))
    });
    shown.rig.say_first(&added());
    shown
        .sheet
        .update(cx, |sheet, cx| sheet.add_after_login(cx));
    wait_until(cx, "the account", |_| !shown.events.borrow().is_empty());
    assert_eq!(*shown.events.borrow(), [AddAccountEvent::Added]);
    let call = shown.rig.acts().last().cloned().expect("a call");
    for part in [
        " j ",
        "--folder /Users/person/.claude-2",
        "--provider claude",
        "--label Studio",
    ] {
        assert!(call.contains(part), "{part} in {call}");
    }
}

#[gpui::test]
fn a_login_folder_the_person_already_has_is_added_at_once_without_a_terminal(
    cx: &mut TestAppContext,
) {
    let (shown, cx) = open_sheet(cx, true);
    shown.rig.say_first(&added());
    name_it(&shown, "Studio", cx);
    shown.sheet.update(cx, |sheet, cx| {
        sheet.set_agent(MasterAgent::Claude, cx);
        sheet.add_folder("/Users/person/.claude-3".into(), cx)
    });
    wait_until(cx, "the account", |_| !shown.events.borrow().is_empty());
    let call = shown.rig.acts().last().cloned().expect("a call");
    for part in [
        " j ",
        "--folder /Users/person/.claude-3",
        "--provider claude",
        "--label Studio",
    ] {
        assert!(call.contains(part), "{part} in {call}");
    }
    assert!(shown.terminal.borrow().is_empty());
}

#[gpui::test]
fn what_the_engine_refuses_is_said_in_its_words_and_nothing_is_added(cx: &mut TestAppContext) {
    let (shown, cx) = open_sheet(cx, true);
    shown
        .rig
        .say_first(&refused("that folder is already an account"));
    name_it(&shown, "Studio", cx);
    shown.sheet.update(cx, |sheet, cx| {
        sheet.add_folder("/Users/person/.claude-3".into(), cx)
    });
    wait_until(cx, "the refusal", |cx| {
        shown
            .sheet
            .read_with(cx, |sheet, _| sheet.refusal().is_some())
    });
    let said = shown
        .sheet
        .read_with(cx, |sheet, _| sheet.refusal().cloned());
    assert_eq!(said.as_deref(), Some("that folder is already an account"));
    assert!(shown.events.borrow().is_empty());
}

#[gpui::test]
fn without_a_name_nothing_is_sent_and_the_sheet_asks_for_one(cx: &mut TestAppContext) {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let (shown, cx) = open_sheet(cx, true);
    shown.sheet.update(cx, |sheet, cx| sheet.log_in_new(cx));
    cx.run_until_parked();
    let said = shown
        .sheet
        .read_with(cx, |sheet, _| sheet.refusal().cloned());
    assert_eq!(said.as_deref(), Some("Give the account a name first."));
    assert!(shown.rig.acts().is_empty());
}

#[gpui::test]
fn a_terminal_that_cannot_open_says_the_line_to_run_and_the_account_can_still_be_added(
    cx: &mut TestAppContext,
) {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let (shown, cx) = open_sheet(cx, false);
    name_it(&shown, "Studio", cx);
    shown.sheet.update(cx, |sheet, cx| {
        sheet.set_agent(MasterAgent::Claude, cx);
        sheet.log_in_new(cx)
    });
    shown.sheet.update(cx, |sheet, cx| sheet.confirm_login(cx));
    wait_until(cx, "the folder", |cx| {
        shown
            .sheet
            .read_with(cx, |sheet, _| matches!(sheet.stage(), Stage::Waiting(_)))
    });
    let said = shown
        .sheet
        .read_with(cx, |sheet, _| sheet.refusal().cloned());
    assert!(said.expect("a sentence").contains(COMMAND));
}

#[gpui::test]
fn a_busy_account_sheet_keeps_its_actions_in_view(cx: &mut TestAppContext) {
    let (shown, cx) = open_sheet(cx, true);
    name_it(&shown, "Studio", cx);
    let hold = shown.rig.dir.path().join("hold-actions");
    std::fs::File::create(&hold).expect("hold the engine reply");
    shown.sheet.update(cx, |sheet, cx| {
        sheet.set_agent(MasterAgent::Claude, cx);
        sheet.log_in_new(cx);
        sheet.confirm_login(cx);
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(shown
        .sheet
        .read_with(cx, |sheet, _| sheet.stage() == &Stage::Working));
    assert!(super::lane_tests::bounds(cx, "account-add-log-in").is_some());
    assert!(super::lane_tests::bounds(cx, "account-add-use-folder").is_some());
    std::fs::remove_file(hold).expect("allow the engine reply");
}

#[gpui::test]
fn before_anything_is_made_the_sheet_says_what_will_be_made_and_asks_to_confirm(
    cx: &mut TestAppContext,
) {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let (shown, cx) = open_sheet(cx, true);
    name_it(&shown, "Studio", cx);
    shown.sheet.update(cx, |sheet, cx| {
        sheet.set_agent(MasterAgent::Claude, cx);
        sheet.log_in_new(cx)
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    for selector in ["account-add-plan", "account-add-open", "account-add-cancel"] {
        assert!(
            super::lane_tests::bounds(cx, selector).is_some(),
            "{selector} is shown before anything is made"
        );
    }
    assert!(shown.rig.acts().is_empty());
    assert!(shown.terminal.borrow().is_empty());
}

#[gpui::test]
fn leaving_at_the_confirmation_makes_no_folder_and_opens_no_terminal(cx: &mut TestAppContext) {
    let (shown, cx) = open_sheet(cx, true);
    name_it(&shown, "Studio", cx);
    shown.sheet.update(cx, |sheet, cx| {
        sheet.set_agent(MasterAgent::Claude, cx);
        sheet.log_in_new(cx)
    });
    shown.sheet.update(cx, |sheet, cx| sheet.cancel(cx));
    cx.run_until_parked();
    assert_eq!(*shown.events.borrow(), [AddAccountEvent::Cancelled]);
    assert!(
        shown.rig.acts().is_empty(),
        "the engine was never asked to make a folder"
    );
    assert!(shown.terminal.borrow().is_empty());
}

fn open_codex_confirmation<'a>(rig: &Rig, cx: &'a mut TestAppContext) -> &'a mut VisualTestContext {
    let (_, cx) = cx.add_window_view(|window, cx| {
        let blank = cx.new(|_| gpui::Empty);
        gpui_component::Root::new(blank, window, cx)
    });
    cx.update(|window, cx| {
        super::account_add::open(rig.source.clone(), rig.facts.clone(), window, cx)
    });
    wait_until(cx, "the account form", |cx| {
        cx.debug_bounds("account-add-log-in").is_some()
    });
    click(cx, "account-add-program-1");
    cx.update(|window, cx| window.focus_next(cx));
    cx.simulate_input("Studio");
    click(cx, "account-add-log-in");
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(cx.debug_bounds("account-add-open").is_some());
    cx
}

#[gpui::test]
fn escape_closes_the_codex_confirmation_without_making_a_folder(cx: &mut TestAppContext) {
    let rig = open(cx, SNAPSHOT, &made(), &added());
    let cx = open_codex_confirmation(&rig, cx);

    cx.simulate_keystrokes("escape");
    cx.update(|window, cx| window.draw(cx).clear(cx));

    assert!(cx.debug_bounds("account-add").is_none());
    assert!(rig.acts().is_empty());
}

#[gpui::test]
fn the_account_sheet_places_its_actions_in_the_footer_below_the_name(cx: &mut TestAppContext) {
    let (rig, _desk, cx) = super::lane_tests::open_answering(cx, SNAPSHOT, &made(), &added());
    cx.simulate_resize(gpui::size(
        crate::theme::WINDOW_MIN_WIDTH,
        crate::theme::WINDOW_MIN_HEIGHT,
    ));
    cx.update(|window, cx| super::account_add::open(rig.source, rig.facts, window, cx));
    wait_until(cx, "the add account dialog", |cx| {
        cx.debug_bounds("account-add").is_some()
    });
    let sheet = super::lane_tests::bounds(cx, "account-add").expect("the sheet");
    let footer = super::lane_tests::bounds(cx, "dialog-footer").expect("the footer");
    let mut previous = footer.left();
    let first = cx.debug_bounds("account-add-cancel").unwrap().center().y;
    for selector in [
        "account-add-cancel",
        "account-add-use-folder",
        "account-add-log-in",
    ] {
        let at = super::lane_tests::bounds(cx, selector).expect(selector);
        assert!(
            at.left() >= sheet.left() && at.right() <= sheet.right(),
            "{selector} {at:?} inside {sheet:?}"
        );
        assert_eq!(at.center().y, first, "{selector}: {at:?}");
        assert!(at.left() >= previous && at.top() >= footer.top());
        assert!(at.bottom() <= footer.bottom());
        previous = at.right();
    }
    let name = super::lane_tests::bounds(cx, "account-add-name-label").expect("the label");
    assert!(name.right() <= sheet.right() && name.bottom() < footer.top());
}

/// The sheet over a fake engine that makes a login folder in a folder of its own, adds an account and gives the
/// master's spec, whose `codex` is the fake app-server; `login` is how the fake's login goes.
fn open_codex_sheet<'a>(
    cx: &'a mut TestAppContext,
    login: &str,
) -> (Sheet, std::path::PathBuf, &'a mut VisualTestContext) {
    let (shown, cx) = open_sheet(cx, true);
    let folder = prepare_codex_login(&shown.rig, login);
    name_it(&shown, "Studio", cx);
    shown.sheet.update(cx, |sheet, cx| {
        sheet.set_agent(MasterAgent::Codex, cx);
        sheet.log_in_new(cx)
    });
    (shown, folder, cx)
}

fn prepare_codex_login(rig: &Rig, login: &str) -> std::path::PathBuf {
    let dir = rig.dir.path();
    let folder = dir.join(".codex-2");
    std::fs::create_dir_all(&folder).expect("the folder the engine made");
    let output = serde_json::json!({"provider": "codex", "folder": folder, "command": "codex login",
        "argv": ["codex", "login"], "env": {"CODEX_HOME": folder}});
    let made = serde_json::json!({"result": "done", "exit_code": 0, "output": output.to_string()});
    std::fs::write(dir.join("made.json"), made.to_string()).expect("made");
    let account = serde_json::json!({"id": "codex-2", "provider": "codex", "label": "Studio"});
    let added =
        serde_json::json!({"result": "done", "exit_code": 0, "output": account.to_string()});
    std::fs::write(dir.join("added.json"), added.to_string()).expect("added");
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).expect("a folder");
    std::os::unix::fs::symlink(super::master_page_tests::fake_codex(), bin.join("codex"))
        .expect("a link");
    let spec = serde_json::json!({
        "argv": ["unused"], "env": {"SAKURA_FAKE_LOGIN": login, "SAKURA_FAKE_LOG": dir.join("codex.log")},
        "cwd": dir.join("master"), "path": bin, "options": {"settings": {"permissions": {}}, "settingSources": []},
        "mcp_servers": [], "start_mode": "default", "close_sessions": true, "unset_env": [], "env_allow": ["HOME"],
        "system_prompt_append": "",
    });
    std::fs::write(dir.join("spec.json"), spec.to_string()).expect("spec");
    let script = r#"#!/bin/sh
d="$(dirname "$0")"
if [ "$2" = snapshot ]; then cat "$d/snapshot.json"; exit 0; fi
if [ "$1 $2" = "master spec" ]; then cat "$d/spec.json"; exit 0; fi
if [ "$1 $2" = "engine rules" ]; then cat "$d/rules.json"; exit 0; fi
if [ "$1 $2" = "cockpit phone" ]; then cat "$d/phone.json"; exit 0; fi
printf '%s\n' "$*" >> "$d/acts.log"
case "$*" in *" j "*) cat "$d/added.json" ;; *) cat "$d/made.json" ;; esac
"#;
    sakura_core::test_support::write_script(&dir.join("sakura"), script);
    folder
}

fn codex_methods(dir: &std::path::Path) -> Vec<String> {
    let text = std::fs::read_to_string(dir.join("codex.log")).unwrap_or_default();
    text.lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter_map(|message| message["method"].as_str().map(str::to_string))
        .collect()
}

#[gpui::test]
fn a_new_codex_login_runs_in_the_app_with_the_browser_and_adds_the_account_when_it_completes(
    cx: &mut TestAppContext,
) {
    let (shown, folder, cx) = open_codex_sheet(cx, "success");
    shown.sheet.update(cx, |sheet, cx| sheet.confirm_login(cx));
    wait_until(cx, "the account", |_| !shown.events.borrow().is_empty());
    assert_eq!(*shown.events.borrow(), [AddAccountEvent::Added]);
    assert_eq!(
        cx.opened_url().as_deref(),
        Some("https://auth.example.test/login?state=abc"),
        "the app opened the browser"
    );
    assert!(shown.terminal.borrow().is_empty(), "no terminal for Codex");
    assert!(
        folder.join("auth.json").is_file(),
        "the login is in the new folder"
    );
    let call = shown.rig.acts().last().cloned().expect("a call");
    for part in [
        " j ",
        "--provider codex",
        "--label Studio",
        &folder.display().to_string(),
    ] {
        assert!(call.contains(part), "{part} in {call}");
    }
}

#[gpui::test]
fn a_codex_login_that_does_not_succeed_is_said_in_words_and_adds_nothing(cx: &mut TestAppContext) {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let (shown, _folder, cx) = open_codex_sheet(cx, "fail");
    shown.sheet.update(cx, |sheet, cx| sheet.confirm_login(cx));
    wait_until(cx, "the words", |cx| {
        shown
            .sheet
            .read_with(cx, |sheet, _| sheet.refusal().is_some())
    });
    let said = shown
        .sheet
        .read_with(cx, |sheet, _| sheet.refusal().cloned());
    let said = said.expect("words");
    assert!(said.contains("the browser window was closed"), "{said}");
    assert!(shown.events.borrow().is_empty());
    assert!(shown.terminal.borrow().is_empty());
    assert!(
        !shown.rig.acts().iter().any(|call| call.contains(" j ")),
        "no account was added"
    );
}

#[gpui::test]
fn leaving_the_sheet_while_the_browser_login_waits_cancels_it_at_the_server(
    cx: &mut TestAppContext,
) {
    let (shown, _folder, cx) = open_codex_sheet(cx, "hold");
    shown.sheet.update(cx, |sheet, cx| sheet.confirm_login(cx));
    wait_until(cx, "the browser", |cx| {
        let _ = cx;
        shown.rig.acts().len() == 1
            && codex_methods(shown.rig.dir.path()).contains(&"account/login/start".to_string())
    });
    shown.sheet.update(cx, |sheet, cx| sheet.cancel(cx));
    wait_until(cx, "the server told", |_| {
        codex_methods(shown.rig.dir.path()).contains(&"account/login/cancel".to_string())
    });
    assert_eq!(*shown.events.borrow(), [AddAccountEvent::Cancelled]);
    assert!(!shown.rig.acts().iter().any(|call| call.contains(" j ")));
}

#[gpui::test]
fn escape_closes_the_browser_wait_and_cancels_the_login_at_the_server(cx: &mut TestAppContext) {
    let rig = open(cx, SNAPSHOT, &made(), &added());
    prepare_codex_login(&rig, "hold");
    let cx = open_codex_confirmation(&rig, cx);
    click(cx, "account-add-open");
    wait_until(cx, "the browser", |cx| {
        cx.debug_bounds("account-add-browser").is_some() && cx.opened_url().is_some()
    });

    cx.simulate_keystrokes("escape");
    cx.update(|window, cx| window.draw(cx).clear(cx));

    assert!(cx.debug_bounds("account-add").is_none());
    wait_until(cx, "the server told", |_| {
        codex_methods(rig.dir.path()).contains(&"account/login/cancel".to_string())
    });
    assert_eq!(rig.acts().len(), 1);
}

#[gpui::test]
fn a_second_press_while_the_browser_login_waits_starts_no_second_login(cx: &mut TestAppContext) {
    let (shown, _folder, cx) = open_codex_sheet(cx, "hold");
    shown.sheet.update(cx, |sheet, cx| sheet.confirm_login(cx));
    wait_until(cx, "the login started", |_| {
        codex_methods(shown.rig.dir.path()).contains(&"account/login/start".to_string())
    });
    shown.sheet.update(cx, |sheet, cx| {
        sheet.confirm_login(cx);
        sheet.log_in_new(cx);
        sheet.confirm_login(cx);
    });
    cx.run_until_parked();
    std::thread::sleep(std::time::Duration::from_millis(300));
    cx.run_until_parked();
    let starts = codex_methods(shown.rig.dir.path())
        .iter()
        .filter(|method| *method == "account/login/start")
        .count();
    assert_eq!(starts, 1, "one login, however often it was pressed");
    assert_eq!(shown.rig.acts().len(), 1, "one folder was made");
    shown.sheet.update(cx, |sheet, cx| sheet.cancel(cx));
}

#[gpui::test]
fn reaudit3_login_path_uses_the_full_confirmation_width(cx: &mut TestAppContext) {
    let (rig, _, window) = super::engine_desk_tests::open(cx, SNAPSHOT);
    window.update(|window, cx| {
        super::account_add::open_for(
            rig.source,
            rig.facts,
            MasterAgent::Claude,
            Some("Studio"),
            Some(super::account_add::DetectedSignIn {
                folder: "/fixture/login-folders/example/studio".into(),
                program: "claude".into(),
                is_in_book: false,
            }),
            window,
            cx,
        );
        window.draw(cx).clear(cx);
    });
    let footer = window.debug_bounds("dialog-footer").unwrap();
    let reveal = window.debug_bounds("account-add-location-icon").unwrap();
    assert!(
        footer.right() - reveal.right() <= crate::theme::GAP,
        "{reveal:?} within {footer:?}"
    );
}
