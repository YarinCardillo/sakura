//! The Agente view's terminal driven in a test window, with a fake `box` script in place of agentbox: one
//! attach per task for the life of the app, a refusal shown with its reason, and the Edit menu's paste.

use super::centre::CentreView;
use super::console::rules::ConsoleKind;
use super::console::{ConsoleState, Consoles};
use super::desk::Desk;
use super::interaction_tests::{click, draw, open_with, wait_for};
use super::status_bar::TerminalItem;
use crate::l10n::Said;
use crate::theme::{self, Appearance, Theme};
use gpui::{Entity, EntityId, Focusable as _, TestAppContext, VisualTestContext};
use std::path::PathBuf;
use std::time::Duration;

/// The fixture task whose agent runs.
const RUNNING: &str = "sample-folders";
/// Lets late output reach the terminal before checking that nothing more arrived.
const SETTLE: Duration = Duration::from_millis(300);

/// A `box` that logs its arguments to `calls` beside it, then runs `body`.
fn fake_box(name: &str, body: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::Builder::new()
        .prefix(name)
        .tempdir()
        .expect("a temporary directory");
    let script = dir.path().join("box");
    let calls = dir.path().join("calls");
    let text = format!("#!/bin/sh\necho \"$@\" >> '{}'\n{body}\n", calls.display());
    sakura_core::test_support::write_script(&script, &text);
    (dir, script)
}

fn calls(dir: &tempfile::TempDir) -> Vec<String> {
    std::fs::read_to_string(dir.path().join("calls"))
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

fn consoles(cx: &mut VisualTestContext, desk: &Entity<Desk>) -> Entity<Consoles> {
    desk.read_with(cx, |desk, _| desk.consoles().clone())
}

fn state(cx: &mut VisualTestContext, consoles: &Entity<Consoles>) -> Option<ConsoleState> {
    consoles.read_with(cx, |consoles, _| {
        consoles.get(RUNNING).map(|c| c.state().clone())
    })
}

/// The terminal's entity id and its last line of text.
fn terminal(cx: &mut VisualTestContext, consoles: &Entity<Consoles>) -> Option<(EntityId, String)> {
    consoles.read_with(cx, |consoles, cx| {
        let view = consoles.get(RUNNING)?.view()?;
        let line = view.read(cx).terminal().read(cx).last_line()?;
        Some((view.entity_id(), line))
    })
}

fn settle(cx: &mut VisualTestContext) {
    std::thread::sleep(SETTLE);
    cx.run_until_parked();
    draw(cx);
}

#[gpui::test]
fn a_running_agent_is_attached_once_and_kept_across_tasks_and_views(cx: &mut TestAppContext) {
    let (dir, script) = fake_box("sakura-attach", "printf 'attached %s\\n' \"$2\"\nexec cat");
    let (cx, desk, _) = open_with(cx, Some(script));
    click(cx, "row-sample-folders");
    let consoles = consoles(cx, &desk);
    wait_for(cx, "the attach", |cx| {
        terminal(cx, &consoles).is_some_and(|(_, line)| line == "attached sample-folders")
    });
    let (first, _) = terminal(cx, &consoles).expect("a terminal");
    click(cx, "row-sample-accent");
    click(cx, "row-sample-folders");
    click(cx, "pane-view-checks");
    click(cx, "pane-view-changes");
    settle(cx);
    let (again, line) = terminal(cx, &consoles).expect("the same terminal");
    assert_eq!(again, first, "never attached again");
    assert_eq!(line, "attached sample-folders", "scrollback kept");
    assert_eq!(calls(&dir), vec![format!("attach {RUNNING}")]);
    cx.update(|window, cx| assert!(window.focused(cx).is_none(), "no autofocus"));
}

#[gpui::test]
fn hiding_and_showing_the_panes_keeps_the_terminal_its_session_and_its_scrollback(
    cx: &mut TestAppContext,
) {
    let (dir, script) = fake_box("sakura-panes", "printf 'attached %s\\n' \"$2\"\nexec cat");
    let (cx, desk, _) = open_with(cx, Some(script));
    cx.update(|_, cx| cx.set_reduce_motion(true));
    click(cx, "row-sample-folders");
    let consoles = consoles(cx, &desk);
    wait_for(cx, "the attach", |cx| {
        terminal(cx, &consoles).is_some_and(|(_, line)| line == "attached sample-folders")
    });
    let (first, _) = terminal(cx, &consoles).expect("a terminal");
    let view = consoles.read_with(cx, |consoles, _| {
        consoles.get(RUNNING).and_then(|c| c.view().cloned())
    });
    cx.update(|window, cx| window.focus(&view.expect("a terminal").focus_handle(cx), cx));
    for keys in ["cmd-b", "alt-cmd-b", "cmd-b", "alt-cmd-b"] {
        cx.simulate_keystrokes(keys);
        settle(cx);
    }
    let shown = desk.read_with(cx, |desk, _| desk.panes_shown());
    assert!(
        shown.sidebar && shown.changes,
        "the keys reached the desk from the terminal"
    );
    let (again, line) = terminal(cx, &consoles).expect("the same terminal");
    assert_eq!(again, first, "the same terminal");
    assert_eq!(
        line, "attached sample-folders",
        "scrollback kept, nothing typed into it"
    );
    assert_eq!(
        calls(&dir),
        vec![format!("attach {RUNNING}")],
        "nothing restarted"
    );
}

#[gpui::test]
fn a_refused_attach_shows_the_reason_and_retry_attaches_again(cx: &mut TestAppContext) {
    let body = "echo \"box: no agent is running for task '$2'\" >&2\nexit 3";
    let (dir, script) = fake_box("sakura-refused", body);
    let (cx, desk, _) = open_with(cx, Some(script));
    click(cx, "row-sample-folders");
    let consoles = consoles(cx, &desk);
    wait_for(cx, "the refusal", |cx| {
        matches!(state(cx, &consoles), Some(ConsoleState::Failed { .. }))
    });
    let Some(ConsoleState::Failed { reason, .. }) = state(cx, &consoles) else {
        panic!("refused");
    };
    assert_eq!(
        reason.details.as_deref(),
        Some("box: no agent is running for task 'sample-folders'"),
        "the line of box is kept whole: the pane says it in its own words and shows it under the details"
    );
    assert_eq!(reason.said, Said::new("box.failure.no_agent"));
    assert!(cx.debug_bounds("refusal-details").is_some());
    click(cx, "console-next");
    wait_for(cx, "the second attach", |_| calls(&dir).len() == 2);
}

#[gpui::test]
fn paste_by_key_or_from_the_edit_menu_reaches_the_focused_terminal_once(cx: &mut TestAppContext) {
    let (_dir, script) = fake_box("sakura-paste", "printf 'ready\\n'\nexec cat");
    let (cx, desk, _) = open_with(cx, Some(script));
    click(cx, "row-sample-folders");
    let consoles = consoles(cx, &desk);
    wait_for(cx, "the attach", |cx| {
        terminal(cx, &consoles).is_some_and(|(_, line)| line == "ready")
    });
    let view = consoles.read_with(cx, |consoles, _| {
        consoles.get(RUNNING).and_then(|c| c.view().cloned())
    });
    let view = view.expect("a terminal");
    cx.update(|window, cx| window.focus(&view.focus_handle(cx), cx));
    cx.write_to_clipboard(gpui::ClipboardItem::new_string("ab".into()));
    cx.simulate_keystrokes("cmd-v");
    wait_for(cx, "the echo", |cx| {
        terminal(cx, &consoles).is_some_and(|(_, line)| line.contains("ab"))
    });
    settle(cx);
    assert_eq!(
        terminal(cx, &consoles).map(|(_, line)| line),
        Some("ab".into())
    );
    cx.dispatch_action(gpui_component::input::Paste);
    wait_for(cx, "the menu's paste", |cx| {
        terminal(cx, &consoles).is_some_and(|(_, line)| line == "abab")
    });
}

#[gpui::test]
fn the_terminal_follows_a_live_appearance_change(cx: &mut TestAppContext) {
    let (_dir, script) = fake_box("sakura-theme", "printf 'ready\\n'\nexec cat");
    let (cx, desk, _) = open_with(cx, Some(script));
    click(cx, "row-sample-folders");
    let consoles = consoles(cx, &desk);
    wait_for(cx, "the attach", |cx| terminal(cx, &consoles).is_some());
    let background = |cx: &mut VisualTestContext| {
        consoles.read_with(cx, |consoles, cx| {
            let view = consoles
                .get(RUNNING)
                .and_then(|c| c.view())
                .expect("a terminal");
            view.read(cx).style().palette.background
        })
    };
    let dark = theme::terminal_style(Appearance::Dark).palette.background;
    let light = theme::terminal_style(Appearance::Light).palette.background;
    assert_eq!(background(cx), dark);
    cx.update(|_, cx| Theme::set(Appearance::Light, cx));
    draw(cx);
    assert_eq!(background(cx), light);
}

#[gpui::test]
fn apri_una_shell_opens_box_shell_in_the_pane_and_closes_it_again(cx: &mut TestAppContext) {
    let (dir, script) = fake_box("sakura-shell", "printf 'shell %s\\n' \"$2\"\nexec cat");
    let (cx, desk, _) = open_with(cx, Some(script));
    click(cx, "row-sample-accent");
    let consoles = consoles(cx, &desk);
    cx.update(|window, cx| {
        desk.update(cx, |desk, cx| {
            desk.toggle_terminal("sample-accent", window, cx)
        })
    });
    wait_for(cx, "the shell", |cx| {
        consoles.read_with(cx, |consoles, cx| {
            let view = consoles.terminal("sample-accent").and_then(|c| c.view());
            view.and_then(|view| view.read(cx).terminal().read(cx).last_line())
                .is_some_and(|line| line == "shell sample-accent")
        })
    });
    cx.update(|window, cx| {
        desk.update(cx, |desk, cx| {
            desk.toggle_terminal("sample-accent", window, cx)
        })
    });
    let is_gone = consoles.read_with(cx, |consoles, _| {
        consoles.terminal("sample-accent").is_none()
    });
    assert!(is_gone, "the second press closes it");
    assert_eq!(calls(&dir), vec!["shell sample-accent".to_string()]);
}

#[gpui::test]
fn a_terminal_opens_off_the_ui_thread_and_a_superseded_opening_is_closed(cx: &mut TestAppContext) {
    let (_dir, script) = fake_box("sakura-opening", "printf 'shell\\n'\nexec cat");
    let (cx, desk, _) = open_with(cx, Some(script));
    click(cx, "row-sample-accent");
    let consoles = consoles(cx, &desk);
    let task = "sample-accent";
    let state_now = cx.update(|window, cx| {
        for _ in 0..2 {
            desk.update(cx, |desk, cx| {
                desk.open_console(task, ConsoleKind::Shell, window, cx)
            });
        }
        consoles
            .read(cx)
            .terminal(task)
            .map(|console| console.state().clone())
    });
    assert_eq!(
        state_now,
        Some(ConsoleState::Opening),
        "the pane says it opens"
    );
    wait_for(cx, "the shown terminal", |cx| {
        terminal_line(cx, &consoles, task).is_some_and(|line| line == "shell")
    });
    // The two sessions open on the background executor, in either order: whichever opened for the newer
    // request is the one shown, and the other is closed.
    let alive = consoles.read_with(cx, |consoles, _| {
        let terminals = consoles.terminals().expect("terminals");
        let is_open = |id: &str| terminals.write(id, b"").is_ok();
        [is_open("t1"), is_open("t2")]
    });
    assert!(
        alive == [false, true] || alive == [true, false],
        "the superseded session was closed and the shown one runs: {alive:?}"
    );
}

fn terminal_line(
    cx: &mut VisualTestContext,
    consoles: &Entity<Consoles>,
    task: &str,
) -> Option<String> {
    consoles.read_with(cx, |consoles, cx| {
        let view = consoles.terminal(task)?.view()?;
        view.read(cx).terminal().read(cx).last_line()
    })
}

/// A `box` whose `attach` and `shell` each say what they are and stay; the shell ends, with the code it
/// is told, when it reads a line (as a shell does on `exit`).
const AGENT_AND_SHELL: &str = r#"case "$1" in
  attach) printf 'agent of %s\n' "$2"; exec cat ;;
  shell) printf 'shell of %s\n' "$2"; read line; exit "${line:-0}" ;;
esac"#;

/// Opens the window on the lavoro whose agent runs and waits for its terminal.
fn open_on_the_agent(
    cx: &mut TestAppContext,
    script: PathBuf,
) -> (
    &mut VisualTestContext,
    Entity<Desk>,
    Entity<Consoles>,
    EntityId,
) {
    let (cx, desk, _) = open_with(cx, Some(script));
    click(cx, "row-sample-folders");
    let consoles = consoles(cx, &desk);
    wait_for(cx, "the agent's terminal", |cx| {
        terminal(cx, &consoles).is_some_and(|(_, line)| line == "agent of sample-folders")
    });
    let (agent, _) = terminal(cx, &consoles).expect("the agent's terminal");
    (cx, desk, consoles, agent)
}

/// With an agent on screen, "Open a terminal".
fn open_a_terminal(cx: &mut VisualTestContext, desk: &Entity<Desk>, dir: &tempfile::TempDir) {
    cx.update(|window, cx| desk.update(cx, |desk, cx| desk.toggle_terminal(RUNNING, window, cx)));
    draw(cx);
    wait_for(cx, "box shell", |_| {
        calls(dir).contains(&format!("shell {RUNNING}"))
    });
    settle(cx);
}

fn centre_view(cx: &mut VisualTestContext, desk: &Entity<Desk>) -> CentreView {
    desk.read_with(cx, |desk, cx| desk.centre_view(RUNNING, cx))
}

fn is_on_screen(cx: &mut VisualTestContext, selector: &'static str) -> bool {
    cx.debug_bounds(selector).is_some()
}

#[gpui::test]
fn opening_a_terminal_neither_replaces_nor_ends_the_agent(cx: &mut TestAppContext) {
    let (dir, script) = fake_box("sakura-two", AGENT_AND_SHELL);
    let (cx, desk, consoles, agent) = open_on_the_agent(cx, script);
    assert!(is_on_screen(cx, "centre-view-agent"));
    assert!(!is_on_screen(cx, "centre-view-terminal"), "no terminal yet");
    open_a_terminal(cx, &desk, &dir);
    let (still, line) = terminal(cx, &consoles).expect("the agent's terminal is still there");
    assert_eq!(still, agent, "the same session, never hung up");
    assert_eq!(line, "agent of sample-folders", "with its scrollback");
    assert_eq!(state(cx, &consoles), Some(ConsoleState::Live));
    // The terminal is what the centre shows, and the way back is on screen beside it.
    assert_eq!(centre_view(cx, &desk), CentreView::Terminal);
    assert_eq!(
        terminal_line(cx, &consoles, RUNNING).as_deref(),
        Some("shell of sample-folders")
    );
    assert!(is_on_screen(cx, "centre-view-agent") && is_on_screen(cx, "centre-view-terminal"));
    click(cx, "centre-view-agent");
    assert_eq!(centre_view(cx, &desk), CentreView::Agent);
    click(cx, "centre-view-terminal");
    assert_eq!(centre_view(cx, &desk), CentreView::Terminal);
    settle(cx);
    assert_eq!(
        calls(&dir),
        vec![format!("attach {RUNNING}"), format!("shell {RUNNING}")],
        "switching views opens nothing"
    );
    assert_eq!(terminal(cx, &consoles).map(|(id, _)| id), Some(agent));
}

in_every_language! {
    fn switching_between_the_agent_and_the_terminal_moves_nothing(cx: &mut TestAppContext) {
        let (dir, script) = fake_box("sakura-steady", AGENT_AND_SHELL);
        let (cx, desk, _, _) = open_on_the_agent(cx, script);
        let before_any = cx.debug_bounds("centre-view-agent").expect("the agent's tab");
        open_a_terminal(cx, &desk, &dir);
        let watched = ["centre-views", "centre-view-agent", "centre-view-terminal", "pane-views"];
        let bounds = |cx: &mut VisualTestContext| watched.map(|selector| cx.debug_bounds(selector));
        let on_terminal = bounds(cx);
        assert_eq!(on_terminal[1], Some(before_any), "opening a terminal moved the agent's tab");
        click(cx, "centre-view-agent");
        assert_eq!(bounds(cx), on_terminal, "choosing a view moved something");
        let over = cx.debug_bounds("centre-view-terminal").expect("the tab").center();
        cx.simulate_mouse_move(over, None, gpui::Modifiers::none());
        draw(cx);
        assert_eq!(bounds(cx), on_terminal, "the pointer moved something");
    }
}

#[gpui::test]
fn a_terminal_that_ends_closes_nothing_else_and_starts_nothing(cx: &mut TestAppContext) {
    let (dir, script) = fake_box("sakura-exit", AGENT_AND_SHELL);
    let (cx, desk, consoles, agent) = open_on_the_agent(cx, script);
    let tabs = |cx: &mut VisualTestContext| {
        desk.read_with(cx, |desk, cx| desk.strip().read(cx).tabs().len())
    };
    let tabs_before = tabs(cx);
    open_a_terminal(cx, &desk, &dir);
    // `exit` in the shell: the second session the window opened.
    consoles.read_with(cx, |consoles, _| {
        let terminals = consoles.terminals().expect("terminals");
        terminals
            .write("t2", b"0\n")
            .expect("the shell reads its line");
    });
    wait_for(cx, "the terminal to end", |cx| {
        consoles.read_with(cx, |consoles, _| consoles.terminal(RUNNING).is_none())
    });
    settle(cx);
    let shown = desk.read_with(cx, |desk, cx| desk.shown(cx));
    assert_eq!(
        shown.as_deref(),
        Some(RUNNING),
        "the lavoro stays on screen"
    );
    assert_eq!(tabs(cx), tabs_before, "its tab stays");
    assert_eq!(cx.update(|_, cx| cx.windows().len()), 1, "the window stays");
    let (still, _) = terminal(cx, &consoles).expect("the agent's terminal is still there");
    assert_eq!(still, agent, "the agent was never touched");
    assert_eq!(
        calls(&dir),
        vec![format!("attach {RUNNING}"), format!("shell {RUNNING}")],
        "nothing was attached again"
    );
    // Back on the view that was there before, with one line that says the terminal ended.
    assert_eq!(centre_view(cx, &desk), CentreView::Agent);
    assert!(!is_on_screen(cx, "centre-view-terminal"));
    assert!(is_on_screen(cx, "terminal-ended"), "the line that says so");
    click(cx, "terminal-open-again");
    wait_for(cx, "the terminal again", |_| {
        calls(&dir)
            .iter()
            .filter(|call| call.starts_with("shell"))
            .count()
            == 2
    });
    assert_eq!(centre_view(cx, &desk), CentreView::Terminal);
    assert!(
        !is_on_screen(cx, "terminal-ended"),
        "the line goes with the new terminal"
    );
}

#[gpui::test]
fn a_terminal_that_fails_says_its_code_and_the_line_can_be_put_away(cx: &mut TestAppContext) {
    let (dir, script) = fake_box("sakura-exit-code", AGENT_AND_SHELL);
    let (cx, desk, consoles, _) = open_on_the_agent(cx, script);
    open_a_terminal(cx, &desk, &dir);
    consoles.read_with(cx, |consoles, _| {
        let terminals = consoles.terminals().expect("terminals");
        terminals
            .write("t2", b"3\n")
            .expect("the shell reads its line");
    });
    wait_for(cx, "the line", |cx| is_on_screen(cx, "terminal-ended"));
    let code = desk.read_with(cx, |desk, _| desk.ended_terminals.get(RUNNING).copied());
    assert_eq!(code, Some(Some(3)));
    click(cx, "terminal-ended-close");
    assert!(!is_on_screen(cx, "terminal-ended"));
    assert_eq!(centre_view(cx, &desk), CentreView::Agent);
}

#[gpui::test]
fn closing_the_terminal_from_the_menu_ends_only_the_terminal_and_says_nothing(
    cx: &mut TestAppContext,
) {
    let (dir, script) = fake_box("sakura-close", AGENT_AND_SHELL);
    let (cx, desk, consoles, agent) = open_on_the_agent(cx, script);
    open_a_terminal(cx, &desk, &dir);
    cx.update(|window, cx| desk.update(cx, |desk, cx| desk.toggle_terminal(RUNNING, window, cx)));
    draw(cx);
    settle(cx);
    assert!(consoles.read_with(cx, |consoles, _| consoles.terminal(RUNNING).is_none()));
    assert_eq!(terminal(cx, &consoles).map(|(id, _)| id), Some(agent));
    assert_eq!(centre_view(cx, &desk), CentreView::Agent);
    assert!(
        !is_on_screen(cx, "terminal-ended"),
        "the person closed it: nothing to say"
    );
}

#[gpui::test]
fn the_status_bar_s_terminal_opens_one_then_shows_it_and_goes_back_without_opening_another(
    cx: &mut TestAppContext,
) {
    let (dir, script) = fake_box("sakura-bar", AGENT_AND_SHELL);
    let (cx, desk, consoles, agent) = open_on_the_agent(cx, script);
    let item = |cx: &mut VisualTestContext| desk.read_with(cx, |desk, cx| desk.terminal_item(cx));
    assert_eq!(item(cx), Some(TerminalItem::None));
    click(cx, "status-terminal");
    wait_for(cx, "box shell", |_| {
        calls(&dir).contains(&format!("shell {RUNNING}"))
    });
    settle(cx);
    assert_eq!(item(cx), Some(TerminalItem::Shown));
    assert_eq!(centre_view(cx, &desk), CentreView::Terminal);
    click(cx, "status-terminal");
    assert_eq!(
        item(cx),
        Some(TerminalItem::Open),
        "back on the agent, the terminal kept"
    );
    assert_eq!(centre_view(cx, &desk), CentreView::Agent);
    click(cx, "status-terminal");
    assert_eq!(item(cx), Some(TerminalItem::Shown));
    settle(cx);
    assert_eq!(
        calls(&dir),
        vec![format!("attach {RUNNING}"), format!("shell {RUNNING}")],
        "one terminal, opened once"
    );
    assert_eq!(terminal(cx, &consoles).map(|(id, _)| id), Some(agent));
}
