//! Scripted input for visual checks of the real chain (snapshot builds only): `SAKURA_SNAPSHOT_DRIVE` holds
//! steps separated by `|`, run in order against the selected task:
//! - `console:shell`, `console:agent`, `console:login=<account>`: opens the task's terminal (and
//!   shows it), or that console in the agent's place;
//! - `sign-in:<account>`: asks the sign-in of that account as Settings and "New task" ask it: in the
//!   task on screen or, with none, in the window's own centre;
//! - `say:<text>`: sends the text to the task's agent, as the conversation's composer does;
//! - `click:<x>,<y>`: a click at that point of the window, in points;
//! - `hover:<x>,<y>`: the pointer moved to that point and left there (a tooltip then needs a `wait`);
//! - `new`: opens "New task";
//! - `key:<keystroke>`: one keystroke through the window's key path, as `cmd-n` or `enter`;
//! - `keys:<text>`: the text typed key by key into whatever has the keyboard (`\n` is Return);
//! - `folder:<path>`: the folder the Mac's picker would have given to the page that adds a project;
//! - `add-project`: confirms the project on that page;
//! - `settings:<step>`: one step on the Settings window, opened without being shown (see
//!   `ui::settings::host::drive`): a section's name, `project=<name>`, `add=<folder>`,
//!   `scroll=<points>`, `search=<words>`, `font=<family>`, `size=<points>`, `snap=<path>`;
//! - `play:start`, `play:stop`: starts or stops the Play of the task on screen;
//! - `pane:<view>`: shows that view of the right pane (`play`, `files`, `changes`, `checks`, `git`);
//! - `tab:<task>`, `select:<task>`: opens a background tab or selects its task;
//! - `live:<task>=waiting|answered|working|starting|stopped|unknown`: fixture conversation state;
//! - `snap:<path>`: saves the window as it is now to that PNG, and goes on (a journey photographed along
//!   the way);
//! - `wait:<ms>`: waits;
//! - `type:<text>`: gives the terminal focus and types the text key by key through the window's key path
//!   (`\n` is Return).

use crate::drive_input::{click, hover, press, press_each};
use crate::ui::console::rules::ConsoleKind;
use crate::ui::desk::Desk;
use gpui::{AsyncWindowContext, Entity, Focusable as _, Window};
use std::time::Duration;

const DRIVE_VAR: &str = "SAKURA_SNAPSHOT_DRIVE";

/// How often a frame is drawn while a script waits. A window that is not presented gets no frame from the
/// system, so a view made by the last step (the Git graph) would stay as its first frame left it, and an
/// answer that came meanwhile would never be drawn.
const FRAME_EVERY: Duration = Duration::from_millis(100);

/// Runs the steps of `SAKURA_SNAPSHOT_DRIVE`, if set.
pub fn schedule(desk: Entity<Desk>, window: &Window, cx: &mut gpui::App) {
    let Ok(script) = std::env::var(DRIVE_VAR) else {
        return;
    };
    window
        .spawn(cx, async move |cx| {
            for step in script.split('|') {
                run(step, &desk, cx).await;
            }
        })
        .detach();
}

/// A wait cut in pieces of [`FRAME_EVERY`], a frame drawn after each.
fn wait_slices(total: Duration) -> Vec<Duration> {
    let whole = total.as_millis() / FRAME_EVERY.as_millis();
    let rest = total - FRAME_EVERY * u32::try_from(whole).unwrap_or(u32::MAX);
    let mut slices = vec![FRAME_EVERY; usize::try_from(whole).unwrap_or(usize::MAX)];
    if !rest.is_zero() {
        slices.push(rest);
    }
    slices
}

async fn run(step: &str, desk: &Entity<Desk>, cx: &mut AsyncWindowContext) {
    let (verb, argument) = step.split_once(':').unwrap_or((step, ""));
    if verb == "wait" {
        for slice in wait_slices(Duration::from_millis(argument.parse().unwrap_or(0))) {
            cx.background_executor().timer(slice).await;
            drop(cx.update(crate::snapshot::draw_frame));
        }
        return;
    }
    let argument = argument.to_string();
    // A window that is not presented is requested no frame: the page a step acts on is drawn first.
    drop(cx.update(crate::snapshot::draw_frame));
    let done = cx.update(|window, cx| match verb {
        "console" => open(desk, &argument, window, cx),
        "sign-in" => desk.update(cx, |desk, cx| desk.sign_in_to(argument, window, cx)),
        "say" => say(desk, &argument, window, cx),
        "new" => desk.update(cx, |desk, cx| desk.open_new_task(window, cx)),
        "type" => type_text(desk, &argument.replace("\\n", "\n"), window, cx),
        "snap" => crate::snapshot::save(window, std::path::Path::new(&argument)),
        "key" => press(&argument, window, cx),
        "keys" => press_each(&argument.replace("\\n", "\n"), window, cx),
        "folder" => {
            let setup = desk.read(cx).project_setup().clone();
            setup.update(cx, |setup, cx| setup.look_at(argument, window, cx));
        }
        "add-project" => {
            let setup = desk.read(cx).project_setup().clone();
            setup.update(cx, |setup, cx| setup.confirm(cx));
        }
        "settings" => crate::ui::settings::host::drive(&argument, cx),
        "play" => desk.update(cx, |desk, cx| desk.drive_play(&argument, cx)),
        "pane" => desk.update(cx, |desk, cx| desk.drive_pane(&argument, cx)),
        "tab" => desk.update(cx, |desk, cx| desk.open_behind(argument, cx)),
        "select" => desk.update(cx, |desk, cx| desk.select(argument, cx)),
        "live" => live(&argument, desk, cx),
        "click" => click(&argument, window, cx),
        "hover" => hover(&argument, window, cx),
        _ => eprintln!("sakura: unknown drive step {step:?}"),
    });
    if let Err(error) = done {
        eprintln!("sakura: drive step {step:?} failed: {error}");
    }
}

fn live(argument: &str, desk: &Entity<Desk>, cx: &mut gpui::App) {
    use sakura_core::model::Live;
    let Some((task, state)) = argument.split_once('=') else {
        return eprintln!("sakura: live needs a task and a state separated by =");
    };
    let live = match state {
        "waiting" | "working" => Live::Working,
        "answered" => Live::Answered,
        "starting" => Live::Starting,
        "stopped" => Live::Stopped,
        "unknown" => Live::Unknown,
        _ => return eprintln!("sakura: unknown fixture live state {state:?}"),
    };
    desk.update(cx, |desk, cx| {
        desk.drive_live(task, live, state == "waiting", cx)
    });
}

fn open(desk: &Entity<Desk>, kind: &str, window: &mut Window, cx: &mut gpui::App) {
    let kind = match kind.split_once('=') {
        Some(("login", account)) => ConsoleKind::Login(account.to_string()),
        _ if kind == "agent" => ConsoleKind::Agent,
        _ => ConsoleKind::Shell,
    };
    desk.update(cx, |desk, cx| {
        let Some(task) = desk.shown(cx) else {
            return;
        };
        match kind {
            // The task's own terminal is opened as the person opens it: in its place, and shown.
            ConsoleKind::Shell => desk.toggle_terminal(&task, window, cx),
            kind => desk.open_console(&task, kind, window, cx),
        }
    });
}

fn say(desk: &Entity<Desk>, text: &str, window: &mut Window, cx: &mut gpui::App) {
    let desk = desk.read(cx);
    let conversation = desk.shown(cx).and_then(|task| desk.conversation_of(&task));
    let Some(conversation) = conversation else {
        return eprintln!("sakura: no conversation to write to");
    };
    conversation.update(cx, |conversation, cx| {
        conversation.write(text, window, cx);
        conversation.send(window, cx);
    });
}

fn type_text(desk: &Entity<Desk>, text: &str, window: &mut Window, cx: &mut gpui::App) {
    let view = desk.read(cx).shown(cx).and_then(|task| {
        let consoles = desk.read(cx).consoles().read(cx);
        consoles.get(&task)?.view().cloned()
    });
    let Some(view) = view else {
        return eprintln!("sakura: no terminal to type into");
    };
    window.focus(&view.focus_handle(cx), cx);
    press_each(text, window, cx);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wait_is_cut_in_frames_and_keeps_its_length() {
        let slices = wait_slices(Duration::from_millis(250));
        assert_eq!(slices.len(), 3, "two whole slices and the rest");
        assert_eq!(slices.iter().sum::<Duration>(), Duration::from_millis(250));
        assert!(wait_slices(Duration::ZERO).is_empty());
    }
}
