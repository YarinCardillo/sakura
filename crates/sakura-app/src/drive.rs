//! Scripted input for visual checks of the real chain (snapshot builds only): `SAKURA_SNAPSHOT_DRIVE` holds
//! steps separated by `|`, run in order against the selected task:
//! - `console:shell`, `console:agent`, `console:login=<account>`: opens the lavoro's terminal (and
//!   shows it), or that console in the agent's place;
//! - `say:<text>`: sends the text to the lavoro's agent, as the conversation's composer does;
//! - `click:<x>,<y>`: a click at that point of the window, in points;
//! - `hover:<x>,<y>`: the pointer moved to that point and left there (a tooltip then needs a `wait`);
//! - `new`: opens "New task";
//! - `key:<keystroke>`: one keystroke through the window's key path, as `cmd-n` or `enter`;
//! - `keys:<text>`: the text typed key by key into whatever has the keyboard (`\n` is Return);
//! - `folder:<path>`: the folder the Mac's picker would have given to the page that adds a project;
//! - `add-project`: confirms the project on that page;
//! - `settings:<step>`: one step on the Settings window, opened without being shown (see
//!   `ui::settings::host::drive`): a section's name, `project=<name>`, `add=<folder>`,
//!   `scroll=<points>`, `font=<family>`, `size=<points>`, `snap=<path>`;
//! - `play:start`, `play:stop`: starts or stops the Play of the lavoro on screen;
//! - `pane:<view>`: shows that view of the right pane (`play`, `files`, `changes`, `checks`, `git`);
//! - `snap:<path>`: saves the window as it is now to that PNG, and goes on (a journey photographed along
//!   the way);
//! - `wait:<ms>`: waits;
//! - `type:<text>`: gives the terminal focus and types the text key by key through the window's key path
//!   (`\n` is Return).

use crate::ui::console::rules::ConsoleKind;
use crate::ui::desk::Desk;
use gpui::{AsyncWindowContext, Entity, Focusable as _, Keystroke, Window};
use std::time::Duration;

const DRIVE_VAR: &str = "SAKURA_SNAPSHOT_DRIVE";

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

async fn run(step: &str, desk: &Entity<Desk>, cx: &mut AsyncWindowContext) {
    let (verb, argument) = step.split_once(':').unwrap_or((step, ""));
    if verb == "wait" {
        let ms = argument.parse().unwrap_or(0);
        cx.background_executor()
            .timer(Duration::from_millis(ms))
            .await;
        return;
    }
    let argument = argument.to_string();
    // A window that is not presented is requested no frame: the page a step acts on is drawn first.
    drop(cx.update(|window, cx| window.draw(cx).clear(cx)));
    let done = cx.update(|window, cx| match verb {
        "console" => open(desk, &argument, window, cx),
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
        "click" => click(&argument, window, cx),
        "hover" => hover(&argument, window, cx),
        _ => eprintln!("sakura: unknown drive step {step:?}"),
    });
    if let Err(error) = done {
        eprintln!("sakura: drive step {step:?} failed: {error}");
    }
}

/// The point `x,y` names, in points of the window.
fn place(at: &str) -> Option<gpui::Point<gpui::Pixels>> {
    let (x, y) = at.split_once(',')?;
    let (x, y) = (x.trim().parse::<f32>().ok()?, y.trim().parse::<f32>().ok()?);
    Some(gpui::point(gpui::px(x), gpui::px(y)))
}

fn hover(at: &str, window: &mut Window, cx: &mut gpui::App) {
    let Some(position) = place(at) else {
        return eprintln!("sakura: a hover needs x,y in numbers, not {at:?}");
    };
    let moved = gpui::MouseMoveEvent {
        position,
        pressed_button: None,
        modifiers: gpui::Modifiers::default(),
    };
    window.dispatch_event(gpui::PlatformInput::MouseMove(moved), cx);
}

fn click(at: &str, window: &mut Window, cx: &mut gpui::App) {
    let Some(position) = place(at) else {
        return eprintln!("sakura: a click needs x,y in numbers, not {at:?}");
    };
    let down = gpui::MouseDownEvent {
        button: gpui::MouseButton::Left,
        position,
        modifiers: gpui::Modifiers::default(),
        click_count: 1,
        first_mouse: false,
    };
    let up = gpui::MouseUpEvent {
        button: gpui::MouseButton::Left,
        position,
        modifiers: gpui::Modifiers::default(),
        click_count: 1,
    };
    window.dispatch_event(gpui::PlatformInput::MouseDown(down), cx);
    window.dispatch_event(gpui::PlatformInput::MouseUp(up), cx);
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
            // The lavoro's own terminal is opened as the person opens it: in its place, and shown.
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

/// One keystroke, named as a key binding names it.
fn press(keystroke: &str, window: &mut Window, cx: &mut gpui::App) {
    match Keystroke::parse_with_os(keystroke, cx.platform_info().os) {
        Ok(keystroke) => drop(window.dispatch_keystroke(keystroke, cx)),
        Err(error) => eprintln!("sakura: {keystroke:?} is not a keystroke: {error}"),
    }
}

/// Types `text` key by key into whatever has the keyboard.
fn press_each(text: &str, window: &mut Window, cx: &mut gpui::App) {
    for c in text.chars() {
        let (key, key_char) = match c {
            '\n' => ("enter".to_string(), None),
            ' ' => ("space".to_string(), Some(" ".to_string())),
            c => (c.to_string(), Some(c.to_string())),
        };
        let keystroke = Keystroke {
            modifiers: gpui::Modifiers::default(),
            key,
            key_char,
        };
        if !window.dispatch_keystroke(keystroke, cx) {
            eprintln!("sakura: the key {c:?} reached nothing");
        }
    }
}
