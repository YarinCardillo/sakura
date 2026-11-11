//! One window with a terminal running a command in a local pty.
//!
//! `cargo run -p sakura-terminal --example term -- [command [args...]]` (default: the login shell).
//!
//! Environment:
//! - `SAKURA_NO_ACTIVATE=1`: do not bring the app to the front at launch.
//! - `SAKURA_APPEARANCE=dark|light`: colours (default: the system appearance).
//! - `SAKURA_TERM_HOLD=1`: keep the window open after the command exits.
//! - `SAKURA_TERM_REPAINT_EVERY_MS=<ms>`: notify the view (not the terminal) at that period, the same
//!   invalidation a cursor blink makes, to measure what a blink costs without focusing the window.
//! - `SAKURA_TERM_UNTHROTTLED=1`: draw at the display's rate even while the window is inactive (GPUI limits
//!   inactive windows to about 30 fps), to measure frame times without activating the app.
//! - `SAKURA_TERM_STATS=<file>`: write render statistics to the file every second.
//! - `SAKURA_TERM_KEYS=<steps>`: scripted input, steps separated by `|`: `wait:<ms>`, `type:<text>` (sent to the
//!   model as typed text), `key:<keystroke>` (dispatched through the window, e.g. `key:escape`, `key:ctrl-c`),
//!   and mouse input dispatched through the window in window points: `click:<x>,<y>,<count>`,
//!   `drag:<x1>,<y1>,<x2>,<y2>`, `scroll:<x>,<y>,<pixels>` (positive scrolls back into history).
//! - With `--features test-support`, `SAKURA_SNAPSHOT=<file.png>`: after `SAKURA_SNAPSHOT_DELAY_MS` (default 1500) the
//!   window draws a fresh frame, renders it to the file through GPUI's renderer and the app quits. For visual
//!   checks using offscreen rendering.

use gpui::{
    point, px, size, App, AppContext, AsyncApp, Bounds, Entity, Focusable, Keystroke, Modifiers,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, OperatingSystem, Pixels,
    PlatformInput, Point, ScrollDelta, ScrollEvent, SharedString, TitlebarOptions, TouchPhase,
    WeakEntity, Window, WindowAppearance, WindowBounds, WindowOptions,
};
use sakura_terminal::{
    GridSize, LocalPty, RenderStats, TerminalChange, TerminalPalette, TerminalStyle, TerminalView,
};
use std::fmt::Write as _;
use std::time::Duration;

const FONT_FAMILY: &str = "Geist Mono";
/// Sakura's panel colours.
const PANEL_DARK: u32 = 0x000000;
const PANEL_LIGHT: u32 = 0xfdfdfd;
const STATS_INTERVAL: Duration = Duration::from_secs(1);
/// Frame intervals longer than this are idle gaps, not frames of an ongoing burst.
const ACTIVE_FRAME_GAP: Duration = Duration::from_millis(100);

fn main() {
    let command: Vec<String> = std::env::args().skip(1).collect();
    gpui_platform::application().run(move |cx: &mut App| {
        if let Err(error) = open_window(command, cx) {
            eprintln!("term: {error}");
            cx.quit();
        }
    });
}

fn open_window(command: Vec<String>, cx: &mut App) -> Result<(), String> {
    let should_activate = std::env::var("SAKURA_NO_ACTIVATE").as_deref() != Ok("1");
    let bounds = Bounds::centered(None, size(px(960.), px(600.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some(SharedString::from("term")),
            ..Default::default()
        }),
        focus: should_activate,
        ..Default::default()
    };
    let options = if std::env::var("SAKURA_TERM_UNTHROTTLED").as_deref() == Ok("1") {
        WindowOptions {
            inactive_frame_interval: None,
            ..options
        }
    } else {
        options
    };
    cx.open_window(options, |window, cx| build_view(command, window, cx))
        .map_err(|error| error.to_string())?;
    if should_activate {
        cx.activate(true);
    }
    Ok(())
}

fn build_view(command: Vec<String>, window: &mut Window, cx: &mut App) -> Entity<TerminalView> {
    let style = TerminalStyle::new(FONT_FAMILY, palette(window));
    let pty = match command.split_first() {
        Some((program, args)) => LocalPty::spawn(program, args, None, GridSize::default()),
        None => LocalPty::login_shell(None, GridSize::default()),
    };
    let pty = pty.unwrap_or_else(|error| {
        eprintln!("term: {error}");
        std::process::exit(1);
    });
    let view = cx.new(|cx| TerminalView::for_source(pty, style, window, cx));
    window.focus(&view.focus_handle(cx), cx);
    watch_terminal(&view, window, cx);
    if let Ok(path) = std::env::var("SAKURA_TERM_STATS") {
        spawn_stats_writer(view.downgrade(), path, window, cx);
    }
    if let Some(period) = std::env::var("SAKURA_TERM_REPAINT_EVERY_MS")
        .ok()
        .and_then(|value| value.parse().ok())
    {
        spawn_repainter(view.downgrade(), Duration::from_millis(period), cx);
    }
    if let Ok(steps) = std::env::var("SAKURA_TERM_KEYS") {
        spawn_script(view.downgrade(), steps, window, cx);
    }
    #[cfg(feature = "test-support")]
    schedule_snapshot(window, cx);
    view
}

/// Renders the window's frame to `SAKURA_SNAPSHOT` after a delay, then quits.
#[cfg(feature = "test-support")]
fn schedule_snapshot(window: &mut Window, cx: &mut App) {
    let Some(path) = std::env::var_os("SAKURA_SNAPSHOT") else {
        return;
    };
    let delay = std::env::var("SAKURA_SNAPSHOT_DELAY_MS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(1500);
    window
        .spawn(cx, async move |cx| {
            cx.background_executor()
                .timer(Duration::from_millis(delay))
                .await;
            let saved = cx.update(|window, cx| {
                save_snapshot(window, cx, std::path::Path::new(&path));
                cx.quit();
            });
            if saved.is_err() {
                eprintln!("term: the window closed before the snapshot");
            }
        })
        .detach();
}

/// Draws a fresh frame and writes it to `path`. A window that was never activated may be occluded and stop
/// drawing: the frame it last presented can be stale.
#[cfg(feature = "test-support")]
fn save_snapshot(window: &mut Window, cx: &mut App, path: &std::path::Path) {
    window.refresh();
    window.draw(cx).clear(cx);
    let result = window
        .render_to_image()
        .map_err(|error| error.to_string())
        .and_then(|image| image.save(path).map_err(|error| error.to_string()));
    match result {
        Ok(()) => eprintln!("term: snapshot saved to {}", path.display()),
        Err(error) => eprintln!("term: snapshot failed: {error}"),
    }
}

fn palette(window: &Window) -> TerminalPalette {
    let is_light = match std::env::var("SAKURA_APPEARANCE").as_deref() {
        Ok("light") => true,
        Ok("dark") => false,
        _ => matches!(
            window.appearance(),
            WindowAppearance::Light | WindowAppearance::VibrantLight
        ),
    };
    if is_light {
        TerminalPalette::one_light(PANEL_LIGHT)
    } else {
        TerminalPalette::one_dark(PANEL_DARK)
    }
}

/// Shows the program's title in the window and quits when the program exits (unless asked to hold).
fn watch_terminal(view: &Entity<TerminalView>, window: &mut Window, cx: &mut App) {
    let should_hold = std::env::var("SAKURA_TERM_HOLD").as_deref() == Ok("1");
    let terminal = view.read(cx).terminal().clone();
    window
        .subscribe(
            &terminal,
            cx,
            move |_, change: &TerminalChange, window, cx| match change {
                TerminalChange::TitleChanged(title) => {
                    window.set_window_title(title.as_deref().unwrap_or("term"))
                }
                TerminalChange::Exited(_) if !should_hold => cx.quit(),
                TerminalChange::Exited(_) | TerminalChange::Bell => {}
            },
        )
        .detach();
}

fn spawn_stats_writer(
    view: WeakEntity<TerminalView>,
    path: String,
    window: &mut Window,
    cx: &mut App,
) {
    let handle = window.window_handle();
    cx.spawn(async move |cx: &mut AsyncApp| loop {
        cx.background_executor().timer(STATS_INTERVAL).await;
        let Ok(stats) = view.read_with(cx, |view, _| view.stats()) else {
            break;
        };
        let visibility = handle
            .update(cx, |_, window, _| format!("{:?}", window.visibility()))
            .unwrap_or_default();
        let text = format!("window_visibility {visibility}\n{}", summarize(&stats));
        if let Err(error) = std::fs::write(&path, text) {
            eprintln!("term: cannot write stats to {path}: {error}");
            break;
        }
    })
    .detach();
}

fn spawn_repainter(view: WeakEntity<TerminalView>, period: Duration, cx: &mut App) {
    cx.spawn(async move |cx: &mut AsyncApp| loop {
        cx.background_executor().timer(period).await;
        if view.update(cx, |_, cx| cx.notify()).is_err() {
            break;
        }
    })
    .detach();
}

/// One step of a `SAKURA_TERM_KEYS` script.
enum Step {
    Wait(Duration),
    Type(Vec<u8>),
    Key(Keystroke),
    Mouse(Vec<PlatformInput>),
}

fn parse_step(step: &str) -> Option<Step> {
    let (verb, argument) = step.split_once(':').unwrap_or((step, ""));
    match verb {
        "wait" => Some(Step::Wait(Duration::from_millis(
            argument.parse().unwrap_or(0),
        ))),
        "type" => Some(Step::Type(
            argument
                .replace("\\n", "\n")
                .replace("\\r", "\r")
                .into_bytes(),
        )),
        "key" => match Keystroke::parse_with_os(argument, OperatingSystem::Mac) {
            Ok(keystroke) => Some(Step::Key(keystroke)),
            Err(_) => {
                eprintln!("term: not a keystroke: {argument}");
                None
            }
        },
        "click" | "drag" | "scroll" => Some(Step::Mouse(mouse_events(verb, argument))),
        other => {
            eprintln!("term: unknown script step {other}");
            None
        }
    }
}

fn spawn_script(view: WeakEntity<TerminalView>, steps: String, window: &mut Window, cx: &mut App) {
    let handle = window.window_handle();
    cx.spawn(async move |cx: &mut AsyncApp| {
        for step in steps.split('|').filter_map(parse_step) {
            let is_alive = match step {
                Step::Wait(duration) => {
                    cx.background_executor().timer(duration).await;
                    true
                }
                Step::Type(bytes) => view
                    .update(cx, |view, cx| {
                        view.terminal()
                            .update(cx, |terminal, cx| terminal.input(bytes, cx));
                    })
                    .is_ok(),
                Step::Key(keystroke) => handle
                    .update(cx, |_, window, cx| window.dispatch_keystroke(keystroke, cx))
                    .is_ok(),
                Step::Mouse(events) => handle
                    .update(cx, |_, window, cx| {
                        for event in events {
                            window.dispatch_event(event, cx);
                        }
                    })
                    .is_ok(),
            };
            if !is_alive {
                break;
            }
        }
    })
    .detach();
}

/// The window events for a scripted mouse step; nothing when its numbers do not parse.
fn mouse_events(verb: &str, argument: &str) -> Vec<PlatformInput> {
    let numbers: Vec<f32> = argument
        .split(',')
        .filter_map(|part| part.trim().parse().ok())
        .collect();
    match (verb, numbers.as_slice()) {
        ("click", &[x, y, count]) => click_events(point(px(x), px(y)), count as usize),
        ("drag", &[x1, y1, x2, y2]) => drag_events(point(px(x1), px(y1)), point(px(x2), px(y2))),
        ("scroll", &[x, y, pixels]) => scroll_events(point(px(x), px(y)), pixels),
        _ => {
            eprintln!("term: cannot read {verb}:{argument}");
            Vec::new()
        }
    }
}

fn press(position: Point<Pixels>, click_count: usize) -> PlatformInput {
    PlatformInput::MouseDown(MouseDownEvent {
        button: MouseButton::Left,
        position,
        modifiers: Modifiers::default(),
        click_count,
        first_mouse: false,
    })
}

fn release(position: Point<Pixels>, click_count: usize) -> PlatformInput {
    PlatformInput::MouseUp(MouseUpEvent {
        button: MouseButton::Left,
        position,
        modifiers: Modifiers::default(),
        click_count,
    })
}

fn click_events(position: Point<Pixels>, click_count: usize) -> Vec<PlatformInput> {
    vec![press(position, click_count), release(position, click_count)]
}

/// A press, four moves with the button held, and a release.
fn drag_events(from: Point<Pixels>, to: Point<Pixels>) -> Vec<PlatformInput> {
    let moves = (1..=4).map(|step| {
        let fraction = step as f32 / 4.0;
        PlatformInput::MouseMove(MouseMoveEvent {
            position: point(
                from.x + (to.x - from.x) * fraction,
                from.y + (to.y - from.y) * fraction,
            ),
            pressed_button: Some(MouseButton::Left),
            modifiers: Modifiers::default(),
        })
    });
    std::iter::once(press(from, 1))
        .chain(moves)
        .chain(std::iter::once(release(to, 1)))
        .collect()
}

/// A trackpad gesture: fingers down, one move of `pixels`, fingers up.
fn scroll_events(position: Point<Pixels>, pixels: f32) -> Vec<PlatformInput> {
    [TouchPhase::Started, TouchPhase::Moved, TouchPhase::Ended]
        .into_iter()
        .map(|touch_phase| {
            let delta = if touch_phase == TouchPhase::Moved {
                pixels
            } else {
                0.0
            };
            PlatformInput::Scroll(ScrollEvent {
                position,
                delta: ScrollDelta::Pixels(point(px(0.), px(delta))),
                modifiers: Modifiers::default(),
                touch_phase,
                source: Default::default(),
            })
        })
        .collect()
}

/// Render statistics as text: counts, then percentiles of frame intervals, grid layout times and key latency.
fn summarize(stats: &RenderStats) -> String {
    let mut text = String::new();
    let _ = writeln!(text, "view_paints {}", stats.view_paints);
    let _ = writeln!(text, "grid_layouts {}", stats.grid_layouts);
    let _ = writeln!(text, "rows_rebuilt {}", stats.rows_rebuilt);
    let _ = writeln!(text, "segments_shaped {}", stats.segments_shaped);
    let intervals: Vec<Duration> = stats
        .paint_instants
        .windows(2)
        .map(|pair| pair[1] - pair[0])
        .collect();
    let _ = writeln!(text, "frame_interval_ms {}", percentiles(&intervals));
    // Intervals while frames follow each other (output, typing), without the idle gaps between bursts.
    let active: Vec<Duration> = intervals
        .iter()
        .copied()
        .filter(|interval| *interval <= ACTIVE_FRAME_GAP)
        .collect();
    let _ = writeln!(text, "frame_interval_active_ms {}", percentiles(&active));
    let _ = writeln!(
        text,
        "grid_layout_ms {}",
        percentiles(&stats.grid_layout_times)
    );
    let _ = writeln!(text, "key_to_paint_ms {}", percentiles(&stats.key_to_paint));
    text
}

fn percentiles(samples: &[Duration]) -> String {
    if samples.is_empty() {
        return "n=0".into();
    }
    let mut millis: Vec<f64> = samples
        .iter()
        .map(|sample| sample.as_secs_f64() * 1000.0)
        .collect();
    millis.sort_by(f64::total_cmp);
    let at = |fraction: f64| millis[((millis.len() - 1) as f64 * fraction).round() as usize];
    format!(
        "n={} min={:.2} p50={:.2} p95={:.2} p99={:.2} max={:.2}",
        millis.len(),
        millis[0],
        at(0.5),
        at(0.95),
        at(0.99),
        millis[millis.len() - 1]
    )
}
