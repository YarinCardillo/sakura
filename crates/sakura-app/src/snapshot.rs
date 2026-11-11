//! Visual checks without screen capture: with the `snapshot` feature and `SAKURA_SNAPSHOT=<file.png>`, the
//! window renders its own last frame through GPUI's renderer after `SAKURA_SNAPSHOT_DELAY_MS` (default 1500)
//! and quits. It draws the frame itself first, so the image does not depend on the system showing the
//! window. The traffic lights are drawn by AppKit, not GPUI, so they are not in the image. With
//! `SAKURA_SNAPSHOT_GRID=1` the lines of the layout grid are drawn over the window.
//!
//! The `measure` feature (part of `snapshot`, but also on its own, without GPUI's test support, so it measures
//! what a normal build does) picks the starting task and view and runs the scroll probe.

use gpui::{App, Window};

#[cfg(feature = "snapshot")]
const SNAPSHOT_VAR: &str = "SAKURA_SNAPSHOT";
#[cfg(feature = "snapshot")]
const DELAY_VAR: &str = "SAKURA_SNAPSHOT_DELAY_MS";
/// How long an entrance (a dialog, a menu) is given between its first frame and the image.
#[cfg(feature = "snapshot")]
const ENTRANCE: std::time::Duration = std::time::Duration::from_millis(400);
#[cfg(feature = "snapshot")]
const DEFAULT_DELAY_MS: u64 = 1500;

/// Schedules the snapshot when requested; does nothing otherwise or without the feature.
#[cfg(feature = "snapshot")]
pub fn schedule(window: &Window, cx: &App) {
    let Some(path) = std::env::var_os(SNAPSHOT_VAR) else {
        return;
    };
    let delay = std::env::var(DELAY_VAR)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_DELAY_MS);
    window
        .spawn(cx, async move |cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(delay))
                .await;
            // The system draws no frame for a window it does not show (a sleeping display, a window
            // behind others): draw the frame of now here, so the image is never an old one. What enters
            // with an animation (a dialog, a menu) starts it on the first frame it is drawn in, so one
            // frame is drawn, the animation is given its time, and the frame after it is the image.
            drop(cx.update(|window, cx| window.draw(cx).clear(cx)));
            cx.background_executor().timer(ENTRANCE).await;
            drop(cx.update(|window, cx| {
                window.draw(cx).clear(cx);
                save(window, std::path::Path::new(&path));
                cx.quit();
            }));
        })
        .detach();
}

/// Saves the window's last frame as a PNG at `path`.
#[cfg(feature = "snapshot")]
pub fn save(window: &Window, path: &std::path::Path) {
    let result = window
        .render_to_image()
        .map_err(|error| error.to_string())
        .and_then(|image| image.save(path).map_err(|error| error.to_string()));
    match result {
        Ok(()) => eprintln!("sakura: snapshot saved to {}", path.display()),
        Err(error) => eprintln!("sakura: snapshot failed: {error}"),
    }
}

/// Whether this launch is for a snapshot: its words must not depend on this Mac's language.
#[cfg(feature = "snapshot")]
pub fn is_asked() -> bool {
    std::env::var_os(SNAPSHOT_VAR).is_some()
}

/// Without the `snapshot` feature no launch is for a snapshot.
#[cfg(not(feature = "snapshot"))]
pub fn is_asked() -> bool {
    false
}

/// Without the `snapshot` feature there is nothing to schedule.
#[cfg(not(feature = "snapshot"))]
pub fn schedule(_window: &Window, _cx: &App) {}

/// With the `measure` feature (also part of `snapshot`), `SAKURA_SNAPSHOT_WIDTH=<points>` opens the window
/// that wide (never under its minimum), so a narrow layout can be seen.
#[cfg(feature = "measure")]
pub fn window_width() -> Option<f32> {
    std::env::var("SAKURA_SNAPSHOT_WIDTH").ok()?.parse().ok()
}

/// Without the feature the window opens at its usual width.
#[cfg(not(feature = "measure"))]
pub fn window_width() -> Option<f32> {
    None
}

/// With the `measure` feature, `SAKURA_SNAPSHOT_HEIGHT=<points>` opens the window that tall (never under
/// its minimum), so the smallest window can be seen whole.
#[cfg(feature = "measure")]
pub fn window_height() -> Option<f32> {
    std::env::var("SAKURA_SNAPSHOT_HEIGHT").ok()?.parse().ok()
}

/// Without the feature the window opens at its usual height.
#[cfg(not(feature = "measure"))]
pub fn window_height() -> Option<f32> {
    None
}

/// What the grid overlay needs to know of the window's layout now.
#[cfg(feature = "snapshot")]
pub struct GridMarks {
    /// The sidebar is fully shown.
    pub has_sidebar: bool,
    /// The panel's outer left edge, and the changes pane's left edge when the pane is shown.
    pub panel_left: gpui::Pixels,
    pub pane_left: Option<gpui::Pixels>,
}

/// Whether `SAKURA_SNAPSHOT_GRID` asks for the lines of the layout grid over the window.
#[cfg(feature = "snapshot")]
pub fn wants_grid() -> bool {
    std::env::var_os("SAKURA_SNAPSHOT_GRID").is_some()
}

/// The lines of the layout grid (`theme.rs`, "Layout grid") drawn over the window, so a snapshot shows
/// what sits on them: the insets of the sidebar, of the panel and of the changes pane as vertical lines;
/// the centre of the window strip, its end, the header's centre line and its end, the status bar's top
/// and its centre as horizontal lines; then, fainter, one line per row pitch down to the status bar.
#[cfg(feature = "snapshot")]
pub fn grid_overlay(marks: GridMarks) -> gpui::AnyElement {
    use gpui::{IntoElement as _, Styled as _};
    gpui::canvas(
        |_, _, _| (),
        move |bounds, (), window, _| paint_grid(bounds, &marks, window),
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
    .into_any_element()
}

#[cfg(feature = "snapshot")]
fn paint_grid(bounds: gpui::Bounds<gpui::Pixels>, marks: &GridMarks, window: &mut Window) {
    use crate::theme;
    use gpui::{fill, hsla, point, px, size, Bounds};
    const THIN: f32 = 0.5;
    let (line, faint) = (hsla(0.04, 0.85, 0.55, 0.8), hsla(0.04, 0.85, 0.55, 0.25));
    let (width, height) = (bounds.size.width, bounds.size.height);
    let sidebar = [
        theme::SIDEBAR_INSET,
        theme::SIDEBAR_WIDTH - theme::SIDEBAR_INSET,
    ];
    let columns = sidebar
        .into_iter()
        .filter(|_| marks.has_sidebar)
        .chain([
            marks.panel_left + theme::CONTENT_INSET,
            width - theme::CONTENT_INSET,
        ])
        .chain(marks.pane_left.map(|left| left + theme::SIDE_PANE_INSET));
    for x in columns {
        let column = Bounds::new(point(x, px(0.)), size(px(THIN), height));
        window.paint_quad(fill(column, line));
    }
    let rows_start = theme::WINDOW_STRIP_HEIGHT + theme::HEADER_HEIGHT;
    let foot = height - theme::STATUS_BAR_HEIGHT;
    let rows = [
        theme::WINDOW_STRIP_HEIGHT / 2.,
        theme::WINDOW_STRIP_HEIGHT,
        theme::HEADER_CENTRE_Y,
        rows_start,
        foot,
        foot + theme::STATUS_BAR_HEIGHT / 2.,
    ];
    for y in rows {
        let row = Bounds::new(point(px(0.), y), size(width, px(THIN)));
        window.paint_quad(fill(row, line));
    }
    let mut y = rows_start + theme::ROW_HEIGHT;
    while y < foot {
        let row = Bounds::new(point(px(0.), y), size(width, px(THIN)));
        window.paint_quad(fill(row, faint));
        y += theme::ROW_HEIGHT;
    }
}

/// With the `measure` feature (also part of `snapshot`), `SAKURA_SNAPSHOT_SELECT=<task>` starts with that task selected, so its page can be seen.
#[cfg(feature = "measure")]
pub fn selection() -> Option<String> {
    std::env::var("SAKURA_SNAPSHOT_SELECT").ok()
}

/// Without the feature the window starts with nothing selected.
#[cfg(not(feature = "measure"))]
pub fn selection() -> Option<String> {
    None
}

/// With the `measure` feature (also part of `snapshot`), `SAKURA_SNAPSHOT_VIEW=changes|checks` picks the pane view, and
/// `SAKURA_SNAPSHOT_LOG=<verb>` opens that verb's log on the Verifiche view.
#[cfg(feature = "measure")]
pub fn view_and_log() -> (Option<String>, Option<String>) {
    let read = |name: &str| std::env::var(name).ok();
    (read("SAKURA_SNAPSHOT_VIEW"), read("SAKURA_SNAPSHOT_LOG"))
}

/// Without the feature the window starts on the Agente view with no log open.
#[cfg(not(feature = "measure"))]
pub fn view_and_log() -> (Option<String>, Option<String>) {
    (None, None)
}

/// With the `measure` feature (also part of `snapshot`), `SAKURA_SNAPSHOT_OPEN=<path>,<path>` opens those files in the changes pane.
#[cfg(feature = "measure")]
pub fn open_files() -> Vec<String> {
    std::env::var("SAKURA_SNAPSHOT_OPEN")
        .map(|paths| paths.split(',').map(str::to_string).collect())
        .unwrap_or_default()
}

/// Without the feature every file starts closed.
#[cfg(not(feature = "measure"))]
pub fn open_files() -> Vec<String> {
    Vec::new()
}

/// With the feature, `SAKURA_MEASURE_SCROLL=<px per frame>` scrolls the changes list on every frame, wrapping
/// at the end, and prints two figures once it has seen [`PROBE_FRAMES`] frames: the interval between frames,
/// and the app's work per frame, from the start of the root view's render to the end of the window's last
/// paint (render, layout, prepaint and paint of the whole window; not the GPU's work or the present).
#[cfg(feature = "measure")]
pub struct ScrollProbe {
    step: f32,
    frames: Vec<std::time::Instant>,
}

/// Frames a scroll probe measures.
#[cfg(feature = "measure")]
const PROBE_FRAMES: usize = 300;

#[cfg(feature = "measure")]
thread_local! {
    /// When this frame's root render began, and each measured frame's work in milliseconds.
    static FRAME_START: std::cell::Cell<Option<(std::time::Instant, f64)>> = const { std::cell::Cell::new(None) };
    static FRAME_WORK: std::cell::RefCell<Vec<f64>> = const { std::cell::RefCell::new(Vec::new()) };
    static FRAME_CPU: std::cell::RefCell<Vec<f64>> = const { std::cell::RefCell::new(Vec::new()) };
    static IS_MEASURING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Renders of each counted view since the probe started.
    static RENDERS: std::cell::RefCell<std::collections::BTreeMap<&'static str, usize>> =
        const { std::cell::RefCell::new(std::collections::BTreeMap::new()) };
}

/// Counts a render of the view `what` while a probe measures, so a report says what each frame drew again.
#[cfg(feature = "measure")]
pub fn count_render(what: &'static str) {
    if IS_MEASURING.get() {
        RENDERS.with_borrow_mut(|renders| *renders.entry(what).or_insert(0) += 1);
    }
}

/// Without the feature nothing is counted.
#[cfg(not(feature = "measure"))]
pub fn count_render(_: &'static str) {}

/// Notes the start of a frame's work (called first thing in the root view's render).
#[cfg(feature = "measure")]
pub fn frame_started() {
    if IS_MEASURING.get() {
        FRAME_START.set(Some((std::time::Instant::now(), thread_cpu_ms())));
    }
}

/// Without the feature no frame is measured.
#[cfg(not(feature = "measure"))]
pub fn frame_started() {}

/// An element painted last in the window, closing the frame's measure of work.
#[cfg(feature = "measure")]
pub fn frame_marker() -> Option<gpui::AnyElement> {
    use gpui::{IntoElement as _, Styled as _};
    if !IS_MEASURING.get() {
        return None;
    }
    let marker = gpui::canvas(
        |_, _, _| (),
        |_, (), _, _| {
            if let Some((at, cpu)) = FRAME_START.take() {
                let work = at.elapsed().as_secs_f64() * 1000.0;
                FRAME_WORK.with_borrow_mut(|frames| frames.push(work));
                FRAME_CPU.with_borrow_mut(|frames| frames.push(thread_cpu_ms() - cpu));
            }
        },
    )
    .absolute()
    .size_0();
    Some(marker.into_any_element())
}

/// Without the feature there is no marker.
#[cfg(not(feature = "measure"))]
pub fn frame_marker() -> Option<gpui::AnyElement> {
    None
}

#[cfg(feature = "measure")]
impl ScrollProbe {
    /// A probe when requested.
    pub fn from_environment() -> Option<Self> {
        let step = std::env::var("SAKURA_MEASURE_SCROLL").ok()?.parse().ok()?;
        IS_MEASURING.set(true);
        Some(Self {
            step,
            frames: Vec::with_capacity(PROBE_FRAMES),
        })
    }

    /// Scrolls one step further on `handle` (content `height` tall) and asks for the next frame.
    pub fn tick(&mut self, handle: &gpui::ScrollHandle, height: f32, window: &mut Window) {
        if self.frames.len() >= PROBE_FRAMES {
            return;
        }
        self.frames.push(std::time::Instant::now());
        let top = -f32::from(handle.offset().y) + self.step;
        let top = if top > height { 0.0 } else { top };
        handle.set_offset(gpui::point(gpui::px(0.), gpui::px(-top)));
        window.request_animation_frame();
        if self.frames.len() == PROBE_FRAMES {
            self.report();
        }
    }

    fn report(&self) {
        let gaps: Vec<f64> = self
            .frames
            .windows(2)
            .map(|pair| (pair[1] - pair[0]).as_secs_f64() * 1000.0)
            .collect();
        Self::print("intervals", gaps);
        // The first frames lay out fonts and the whole patch; the steady state is what scrolling costs.
        let work =
            FRAME_WORK.with_borrow(|frames| frames.iter().skip(PROBE_WARMUP).copied().collect());
        Self::print("work", work);
        let cpu =
            FRAME_CPU.with_borrow(|frames| frames.iter().skip(PROBE_WARMUP).copied().collect());
        Self::print("cpu", cpu);
        RENDERS.with_borrow(|renders| {
            let counted: Vec<String> = renders
                .iter()
                .map(|(what, n)| format!("{what} {n}"))
                .collect();
            eprintln!(
                "sakura: scroll probe renders over {} frames: {}",
                self.frames.len(),
                counted.join(", ")
            );
        });
    }

    fn print(what: &str, mut gaps: Vec<f64>) {
        if gaps.is_empty() {
            return;
        }
        gaps.sort_by(f64::total_cmp);
        let at = |share: f64| gaps[((gaps.len() - 1) as f64 * share) as usize];
        let mean = gaps.iter().sum::<f64>() / gaps.len() as f64;
        let long = gaps.iter().filter(|gap| **gap > at(0.5) * 1.5).count();
        eprintln!(
            "sakura: scroll probe {what} over {} frames, mean {mean:.2} ms, median {:.2}, p95 {:.2}, p99 {:.2}, max {:.2}, over 1.5x median {long}",
            gaps.len(),
            at(0.5),
            at(0.95),
            at(0.99),
            at(1.0)
        );
    }
}

/// CPU time of the calling thread in milliseconds: the frame's own work, apart from time the thread waited
/// for a core while other processes ran.
#[cfg(feature = "measure")]
fn thread_cpu_ms() -> f64 {
    let mut now = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `now` is a valid, writable timespec for the duration of the call.
    let status = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut now) };
    if status != 0 {
        return 0.0;
    }
    now.tv_sec as f64 * 1000.0 + now.tv_nsec as f64 / 1_000_000.0
}

/// Frames left out of the work figures at the start of a probe.
#[cfg(feature = "measure")]
const PROBE_WARMUP: usize = 10;
