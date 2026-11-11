//! The terminal view the app embeds: focus, keys, clipboard, cursor blink and IME text input. The grid is a
//! cached child view; the cursor and input are drawn here by [`crate::overlay::OverlayElement`].

use crate::grid_view::{FrameInfo, GridView};
use crate::keys::{encode_keystroke, encode_paste};
use crate::overlay::OverlayElement;
use crate::scroll::ScrollAccumulator;
use crate::source::TerminalSource;
use crate::stats::{RenderStats, StatsRecorder};
use crate::style::{FocusRing, TerminalStyle};
use crate::terminal::Terminal;
use alacritty_terminal::term::TermMode;
use gpui::{
    div, prelude::*, px, rgb, App, ClipboardItem, Context, Entity, FocusHandle, Focusable,
    KeyDownEvent, Pixels, Point, StyleRefinement, Task, Window,
};
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

/// Half a blink period: the cursor is shown, then hidden, for this long each.
const BLINK_INTERVAL: Duration = Duration::from_millis(600);
const FOCUS_IN_REPORT: &[u8] = b"\x1b[I";
const FOCUS_OUT_REPORT: &[u8] = b"\x1b[O";

/// A terminal on screen. Create it with [`TerminalView::new`] inside `cx.new`.
pub struct TerminalView {
    pub(crate) terminal: Entity<Terminal>,
    pub(crate) style: Rc<TerminalStyle>,
    grid: Entity<GridView>,
    pub(crate) focus: FocusHandle,
    pub(crate) info: Rc<Cell<FrameInfo>>,
    pub(crate) stats: Rc<StatsRecorder>,
    pub(crate) marked_text: Option<String>,
    pub(crate) is_cursor_shown: bool,
    /// Focus as last reported to the program; `None` before the first change.
    has_focus: Option<bool>,
    /// Whether the focus the view holds arrived from the keyboard (Tab), which shows the ring.
    pub(crate) is_keyboard_focus: bool,
    blink_epoch: u64,
    blink_task: Option<Task<()>>,
    pub(crate) pointer: crate::pointer::PointerState,
    pub(crate) scroll: ScrollAccumulator,
    _subscriptions: Vec<gpui::Subscription>,
}

impl TerminalView {
    /// A view of `terminal` drawn with `style`.
    pub fn new(
        terminal: Entity<Terminal>,
        style: TerminalStyle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let style = Rc::new(style);
        let info = Rc::new(Cell::new(FrameInfo::default()));
        let stats = Rc::new(StatsRecorder::default());
        let grid = cx.new(|cx| {
            GridView::new(
                terminal.clone(),
                style.clone(),
                info.clone(),
                stats.clone(),
                cx,
            )
        });
        // A tab stop, so the keyboard reaches the terminal; it never takes focus on its own.
        let focus = cx.focus_handle().tab_stop(true);
        let subscriptions = Self::subscribe(&terminal, &focus, window, cx);
        Self {
            terminal,
            style,
            grid,
            focus,
            info,
            stats,
            marked_text: None,
            is_cursor_shown: true,
            has_focus: None,
            is_keyboard_focus: false,
            blink_epoch: 0,
            blink_task: None,
            pointer: Default::default(),
            scroll: ScrollAccumulator::default(),
            _subscriptions: subscriptions,
        }
    }

    /// Follows the terminal's output and the focus of the view and of its window.
    fn subscribe(
        terminal: &Entity<Terminal>,
        focus: &FocusHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::Subscription> {
        vec![
            // The cursor follows output; observing keeps it moving even if the app caches this view.
            cx.observe(terminal, |_, _, cx| cx.notify()),
            cx.on_focus_in(focus, window, |view, window, cx| {
                view.is_keyboard_focus = window.last_input_was_keyboard();
                view.focus_changed(true, window, cx)
            }),
            cx.on_focus_out(focus, window, |view, _, window, cx| {
                view.is_keyboard_focus = false;
                view.focus_changed(false, window, cx)
            }),
            cx.observe_window_activation(window, |view, window, cx| {
                if view.focus.is_focused(window) {
                    view.focus_changed(window.is_window_active(), window, cx);
                }
            }),
        ]
    }

    /// Starts `source` in a new [`Terminal`] and shows it: the one call the app needs.
    pub fn for_source(
        source: impl TerminalSource,
        style: TerminalStyle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let palette = style.palette.clone();
        let terminal = cx.new(|cx| Terminal::new(source, palette, cx));
        Self::new(terminal, style, window, cx)
    }

    /// The terminal shown, to read its title, watch its [`crate::TerminalChange`]s or send input.
    pub fn terminal(&self) -> &Entity<Terminal> {
        &self.terminal
    }

    /// How the view looks now.
    pub fn style(&self) -> &TerminalStyle {
        &self.style
    }

    /// Changes font, padding or colours (for a theme switch).
    pub fn set_style(&mut self, style: TerminalStyle, cx: &mut Context<Self>) {
        let palette_changed = style.palette != self.style.palette;
        self.style = Rc::new(style);
        let style = self.style.clone();
        self.grid.update(cx, |grid, cx| grid.set_style(style, cx));
        if palette_changed {
            let palette = self.style.palette.clone();
            self.terminal
                .update(cx, |terminal, cx| terminal.set_palette(palette, cx));
        }
        cx.notify();
    }

    /// Render counters since the view was created.
    pub fn stats(&self) -> RenderStats {
        self.stats.snapshot()
    }

    /// Text typed or committed by the input method.
    pub(crate) fn commit_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if text.is_empty() {
            return;
        }
        self.send_input(text.as_bytes().to_vec(), cx);
    }

    pub(crate) fn send_input(&mut self, bytes: Vec<u8>, cx: &mut Context<Self>) {
        self.stats.key_pressed();
        if self.has_focus == Some(true) {
            self.restart_blink(cx);
        }
        self.terminal
            .update(cx, |terminal, cx| terminal.input(bytes, cx));
    }

    /// The emulator's modes right now.
    pub(crate) fn mode(&self, cx: &App) -> TermMode {
        self.terminal.read(cx).shared().lock().mode()
    }

    fn key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        if keystroke.modifiers.platform {
            self.command_key(keystroke.key.as_str(), cx);
            return;
        }
        if event.prefer_character_input && keystroke.key_char.is_some() {
            return;
        }
        let mode = self.mode(cx);
        if let Some(bytes) = encode_keystroke(keystroke, mode, self.style.option_as_meta) {
            self.send_input(bytes.into_owned().into_bytes(), cx);
            cx.stop_propagation();
        }
    }

    /// Copies the selection to the clipboard; `false` when nothing is selected. For the app's Edit menu.
    pub fn copy(&mut self, cx: &mut Context<Self>) -> bool {
        let text = self.terminal.read(cx).shared().lock().selection_text();
        let Some(text) = text else {
            return false;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        true
    }

    /// Pastes the clipboard's text as the program expects it (bracketed when it asked). For the Edit menu.
    pub fn paste(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            let bytes = encode_paste(&text, self.mode(cx));
            self.send_input(bytes, cx);
        }
    }

    /// Whether the view holds focus that arrived from the keyboard, so its ring shows.
    pub fn is_keyboard_focused(&self, window: &Window) -> bool {
        self.is_keyboard_focus && self.focus.is_focused(window)
    }

    /// Cmd+C copies when there is a selection; Cmd+V pastes. Other Cmd keys belong to the app.
    fn command_key(&mut self, key: &str, cx: &mut Context<Self>) {
        match key {
            "c" => {
                if self.copy(cx) {
                    cx.stop_propagation();
                }
            }
            "v" => {
                self.paste(cx);
                cx.stop_propagation();
            }
            _ => {}
        }
    }

    fn focus_changed(&mut self, is_focused: bool, window: &mut Window, cx: &mut Context<Self>) {
        let is_focused = is_focused && window.is_window_active();
        // Blurring the view and deactivating its window both arrive here; the program hears each change once.
        if self.has_focus == Some(is_focused) {
            return;
        }
        self.has_focus = Some(is_focused);
        let shared = self.terminal.read(cx).shared().clone();
        let reports_focus = {
            let mut core = shared.lock();
            core.set_focused(is_focused);
            core.mode().contains(TermMode::FOCUS_IN_OUT)
        };
        if reports_focus {
            let report = if is_focused {
                FOCUS_IN_REPORT
            } else {
                FOCUS_OUT_REPORT
            };
            self.terminal.read(cx).report(report.to_vec());
        }
        if is_focused {
            self.restart_blink(cx);
        } else {
            self.stop_blink(cx);
        }
    }

    /// Shows the cursor and starts blinking again after a full interval (typing keeps it steady).
    fn restart_blink(&mut self, cx: &mut Context<Self>) {
        self.blink_epoch += 1;
        let epoch = self.blink_epoch;
        if !self.is_cursor_shown {
            self.is_cursor_shown = true;
            cx.notify();
        }
        self.blink_task = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(BLINK_INTERVAL).await;
            let toggled = this.update(cx, |view, cx| {
                if view.blink_epoch != epoch {
                    return false;
                }
                view.is_cursor_shown = !view.is_cursor_shown;
                cx.notify();
                true
            });
            if !matches!(toggled, Ok(true)) {
                break;
            }
        }));
    }

    fn stop_blink(&mut self, cx: &mut Context<Self>) {
        self.blink_epoch += 1;
        self.blink_task = None;
        self.is_cursor_shown = true;
        cx.notify();
    }

    /// Where the IME should place its candidate window: the cursor cell, in window coordinates.
    pub(crate) fn cursor_origin(&self) -> Option<(Point<Pixels>, Pixels, Pixels)> {
        let info = self.info.get();
        let (geometry, cursor) = (info.geometry?, info.cursor?);
        let bounds = geometry.cell_bounds(cursor.point.line, cursor.point.column, 1);
        Some((bounds.origin, geometry.cell_width, geometry.line_height))
    }
}

impl Focusable for TerminalView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ring = self
            .style
            .focus_ring
            .filter(|_| self.is_keyboard_focused(window))
            .map(focus_ring);
        div()
            .id("sakura-terminal")
            .key_context("SakuraTerminal")
            .track_focus(&self.focus)
            .relative()
            .size_full()
            .bg(rgb(self.style.palette.background))
            .py(self.style.padding_px())
            .px(self.style.padding_x_px())
            .on_key_down(cx.listener(Self::key_down))
            .child(
                self.grid
                    .clone()
                    .cached(StyleRefinement::default().size_full()),
            )
            .child(OverlayElement::new(cx.entity()))
            .children(ring)
    }
}

/// The keyboard focus ring, above the grid and the cursor; it takes no pointer input.
fn focus_ring(ring: FocusRing) -> impl IntoElement {
    div()
        .absolute()
        .inset(px(ring.inset))
        .border(px(ring.width))
        .border_color(rgb(ring.colour))
        .rounded(px(ring.radius))
}
