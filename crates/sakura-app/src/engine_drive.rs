//! Scripted input for visual checks of the engine's window (snapshot builds only): `SAKURA_SNAPSHOT_DRIVE`
//! holds steps separated by `|`, run in order:
//! - `master`: shows the Master page;
//! - `say:<text>`: sends the text to the master, as its conversation's composer does;
//! - `time-say:<text>`: sends the text to the master and says on stderr how long the first words of the answer
//!   and the end of the turn took, in milliseconds;
//! - `attach:<path>`: attaches the file to the master's next message, as a drop on the conversation does;
//! - `agent:<claude|codex>`: switches the master to that agent, as the header's switch does;
//! - `waiting`: shows the Inbox, and `done` its answered asks;
//! - `lanes`: shows the Lanes page;
//! - `engine`: shows the Engine page;
//! - `catalogue` or `catalogue:<part>`: shows the catalogue page (every component in every state, the scale), or one
//!   of its five parts (0 to 4), in the page's place;
//! - `approve:<wait>`, `accept:<wait>`, `reject:<wait>`: presses Approve, Accept or Reject (a proposal) on the wait named by its task,
//!   or part of its id or title, and `confirm:<wait>` confirms what the engine summarised for it;
//! - `approve-overlap:<wait>`: presses Approve on the overlap gate's wait named so, which lets a delivery that
//!   overlaps another lane go on;
//! - `resume:<lane>`, `stop:<lane>`, `clean:<lane>`: presses that action on the lane of that task, when the
//!   snapshot lists it for the lane, and `confirm:<lane>` confirms what the engine summarised;
//! - `reopen`: makes the Master's conversation again, reading the kept sessions of `SAKURA_SNAPSHOT_DATA` (the
//!   private data folder of the run, where they are also written), so that it takes up its session;
//! - `lane:<task>`: shows the lane of that task;
//! - `write:<text>`: sends the text to the lane on screen, as its composer's button does;
//! - `report`: says on stderr where the Master's conversation stands;
//! - `click:<x>,<y>`, `type:<text>`, `key:<keystroke>`: the input of a person, into the window's own event
//!   pipeline (a click at a point of the window in points, hit-testing and handlers as for a real click;
//!   the text typed key by key into whatever has the keyboard, `\n` is Return; one keystroke as `cmd-n` or
//!   `enter`);
//! - `drag:<x1>,<y1>,<x2>,<y2>`: the left button pressed at the first point, the pointer moved in steps to the
//!   second with the button held, and the button released there (a tab taken and dropped);
//! - `hold:<x>,<y>`, `pull:<x>,<y>`, `let-go:<x>,<y>`: the parts of a drag, to photograph it on its way: the left
//!   button pressed and kept down, the pointer moved with it held, the button released;
//! - `scroll:<x>,<y>,<dy>`: the wheel turned by `dy` points (negative moves the content up) at that point, through
//!   the window's own event pipeline;
//! - `hover:<x>,<y>`: the pointer moved to that point and left there, no button (a hover fill, a tooltip after
//!   a `wait`);
//! - `pick-folder:<path>`: the folder the next folder panel returns, instead of showing it;
//! - `pick-file:<path>[,<path>]`: the files the next file chooser (the master's + button) returns, instead of showing it;
//! - `quit`: ends the app;
//! - `snap:<path>`: saves the window as it is now to that PNG, and goes on;
//! - `wait:<ms>`: waits, a frame drawn every tenth of a second.

use crate::drive_input::{click, drag, hold, hover, let_go, press, press_each, pull, scroll};
use crate::ui::engine_desk::Desk;
use gpui::{AsyncWindowContext, Entity, Window};
use sakura_core::tabs::Page;
use std::time::Duration;

const DRIVE_VAR: &str = "SAKURA_SNAPSHOT_DRIVE";
/// How often a frame is drawn while a script waits: a window that is not presented gets none from the
/// system, and what the agent said meanwhile would never be drawn.
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

async fn run(step: &str, desk: &Entity<Desk>, cx: &mut AsyncWindowContext) {
    let (verb, argument) = step.split_once(':').unwrap_or((step, ""));
    if verb == "wait" {
        let total = Duration::from_millis(argument.parse().unwrap_or(0));
        let mut waited = Duration::ZERO;
        while waited < total {
            cx.background_executor().timer(FRAME_EVERY).await;
            waited += FRAME_EVERY;
            drop(cx.update(crate::snapshot::draw_frame));
        }
        return;
    }
    if verb == "time-say" {
        return time_say(argument, desk, cx).await;
    }
    if verb == "snap-growth" {
        return snap_growth(argument, desk, cx).await;
    }
    let argument = argument.to_string();
    // A window that is not presented is requested no frame: the page a step acts on is drawn first.
    drop(cx.update(crate::snapshot::draw_frame));
    let done = cx.update(|window, cx| match verb {
        "master" => desk.update(cx, |desk, cx| desk.navigate(Page::Master, false, cx)),
        "say" => say(desk, &argument, window, cx),
        "attach" => attach(desk, &argument, cx),
        "agent" => switch_agent(desk, &argument, cx),
        "agent-model" => pick_model(desk, &argument, cx),
        "waiting" => desk.update(cx, |desk, cx| desk.navigate(Page::Inbox, false, cx)),
        "lanes" => desk.update(cx, |desk, cx| desk.navigate(Page::Lanes, false, cx)),
        "done" => desk.update(cx, |desk, cx| desk.drive_inbox_done(cx)),
        "engine" => desk.update(cx, |desk, cx| desk.navigate(Page::Engine, false, cx)),
        "catalogue" => {
            let part = argument.parse().ok();
            desk.update(cx, |desk, cx| desk.drive_catalogue(part, window, cx))
        }
        "approve" => desk.update(cx, |desk, cx| desk.drive_press("a", &argument, cx)),
        "approve-overlap" => desk.update(cx, |desk, cx| {
            desk.drive_press_gate("a", Some("overlap"), &argument, cx)
        }),
        "resume" => desk.update(cx, |desk, cx| desk.drive_lane_action("R", &argument, cx)),
        "stop" => desk.update(cx, |desk, cx| desk.drive_lane_action("s", &argument, cx)),
        "clean" => desk.update(cx, |desk, cx| desk.drive_lane_action("C", &argument, cx)),
        "reject" => desk.update(cx, |desk, cx| desk.drive_press("N", &argument, cx)),
        "accept" => desk.update(cx, |desk, cx| desk.drive_press("y", &argument, cx)),
        "reopen" => {
            let folder = crate::snapshot::data_folder();
            desk.update(cx, |desk, cx| {
                desk.drive_reopen_master(folder.as_deref(), window, cx)
            })
        }
        "confirm" => desk.update(cx, |desk, cx| desk.drive_confirm(&argument, cx)),
        "lane" => desk.update(cx, |desk, cx| desk.drive_lane(&argument, cx)),
        "write" => desk.update(cx, |desk, cx| desk.drive_write(&argument, cx)),
        "open-agent" => desk.update(cx, |desk, cx| desk.drive_open_agent(&argument, cx)),
        "report" => report(desk, cx),
        "click" => click(&argument, window, cx),
        "drag" => drag(&argument, window, cx),
        "hold" => hold(&argument, window, cx),
        "pull" => pull(&argument, window, cx),
        "let-go" => let_go(&argument, window, cx),
        "hover" => hover(&argument, window, cx),
        "scroll" => scroll(&argument, window, cx),
        "type" => press_each(&argument.replace("\\n", "\n"), window, cx),
        "key" => press(&argument, window, cx),
        "pick-folder" => {
            crate::ui::project_add::pick_next_folder(std::path::PathBuf::from(&argument), cx)
        }
        "pick-file" => crate::ui::thread::pick_next_files(&argument, cx),
        "quit" => cx.quit(),
        "snap" => crate::snapshot::save(window, std::path::Path::new(&argument)),
        _ => eprintln!("sakura: unknown drive step {step:?}"),
    });
    if let Err(error) = done {
        eprintln!("sakura: drive step {step:?} failed: {error}");
    }
}

/// How often `snap-growth` looks for the next growth of the text being written, and draws.
const GROWTH_LOOK: Duration = Duration::from_millis(20);
/// How long after a growth the picture is taken: the new words are part way through their fade.
const GROWTH_DELAY: Duration = Duration::from_millis(10);
/// The longest `snap-growth` waits for a growth.
const GROWTH_WAIT: Duration = Duration::from_secs(10);

/// Waits for the text of the open agent's conversation to grow once more, then saves the picture a moment
/// after (`PATH@MILLISECONDS`, 10 when none): `snap-growth:PATH`. Two of them in a row show one message growing, the new words fading in.
async fn snap_growth(argument: &str, desk: &Entity<Desk>, cx: &mut AsyncWindowContext) {
    let (path, delay) = argument.rsplit_once('@').unwrap_or((argument, ""));
    let delay = delay.parse().map_or(GROWTH_DELAY, Duration::from_millis);
    let growths = |cx: &mut AsyncWindowContext| {
        cx.update(|_, cx| desk.read(cx).drive_chat_growths(cx))
            .ok()
            .flatten()
    };
    let Some(start) = growths(cx) else {
        return eprintln!("sakura: snap-growth wants a conversation open on the lane on screen");
    };
    let mut waited = Duration::ZERO;
    while growths(cx).is_some_and(|now| now == start) && waited < GROWTH_WAIT {
        cx.background_executor().timer(GROWTH_LOOK).await;
        waited += GROWTH_LOOK;
        drop(cx.update(crate::snapshot::draw_frame));
    }
    let mut after = Duration::ZERO;
    while after < delay {
        cx.background_executor().timer(GROWTH_LOOK).await;
        after += GROWTH_LOOK;
        drop(cx.update(crate::snapshot::draw_frame));
    }
    let path = std::path::PathBuf::from(path);
    drop(cx.update(|window, _| crate::snapshot::save(window, &path)));
}

/// How often `time-say` looks at the conversation, and the longest it waits for the end of the turn.
const TIME_LOOK: Duration = Duration::from_millis(10);
const TIME_LIMIT: Duration = Duration::from_secs(180);

/// Sends `text` to the master and reports how long the answer's first words and the end of the turn took.
async fn time_say(text: &str, desk: &Entity<Desk>, cx: &mut AsyncWindowContext) {
    let before = cx
        .update(|_, cx| {
            desk.read(cx)
                .master_conversation(cx)
                .read(cx)
                .assistant_text_len()
        })
        .unwrap_or_default();
    let began = std::time::Instant::now();
    let sent = cx.update(|window, cx| say(desk, text, window, cx));
    if sent.is_err() {
        return eprintln!("sakura: time-say could not send {text:?}");
    }
    let mut first_words = None;
    while began.elapsed() < TIME_LIMIT {
        cx.background_executor().timer(TIME_LOOK).await;
        drop(cx.update(crate::snapshot::draw_frame));
        let Ok((length, is_idle, summary, said)) = cx.update(|_, cx| {
            let conversation = desk.read(cx).master_conversation(cx);
            let conversation = conversation.read(cx);
            let is_idle = conversation.thread().turn() == sakura_acp::thread::Turn::Idle;
            let said = conversation.last_assistant_text();
            (
                conversation.assistant_text_len(),
                is_idle,
                conversation.summary(),
                said,
            )
        }) else {
            return;
        };
        if first_words.is_none() && length > before {
            first_words = Some(began.elapsed());
        }
        if first_words.is_some() && is_idle {
            let words = first_words.unwrap_or_default().as_millis();
            let ended = began.elapsed().as_millis();
            return eprintln!("sakura: time-say first words after {words} ms, turn ended after {ended} ms ({summary}) said {said:?}");
        }
    }
    eprintln!("sakura: time-say gave up after {TIME_LIMIT:?} (first words: {first_words:?})");
}

fn say(desk: &Entity<Desk>, text: &str, window: &mut Window, cx: &mut gpui::App) {
    let conversation = desk.read(cx).master_conversation(cx);
    conversation.update(cx, |conversation, cx| {
        conversation.write(text, window, cx);
        conversation.send(window, cx);
    });
}

fn attach(desk: &Entity<Desk>, path: &str, cx: &mut gpui::App) {
    let conversation = desk.read(cx).master_conversation(cx);
    let paths = [std::path::PathBuf::from(path)];
    conversation.update(cx, |conversation, cx| conversation.attach_paths(&paths, cx));
}

fn switch_agent(desk: &Entity<Desk>, name: &str, cx: &mut gpui::App) {
    let agent = crate::ui::master_agent::MasterAgent::from_code(Some(name));
    let page = desk.read(cx).master_page(cx);
    page.update(cx, |page, cx| page.set_agent(agent, cx));
}

/// Chooses a model of the conversation's agent in the agent chip, as its menu does.
fn pick_model(desk: &Entity<Desk>, model: &str, cx: &mut gpui::App) {
    let pick = crate::ui::thread::MenuPick::Model(model.to_string());
    let conversation = desk.read(cx).master_conversation(cx);
    conversation.update(cx, |conversation, cx| conversation.pick_entry(&pick, cx));
}

fn report(desk: &Entity<Desk>, cx: &mut gpui::App) {
    let conversation = desk.read(cx).master_conversation(cx);
    eprintln!("sakura: master {}", conversation.read(cx).summary());
}
