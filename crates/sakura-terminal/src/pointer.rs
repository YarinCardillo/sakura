//! Mouse and trackpad on the terminal: selection by drag, double and triple click, wheel scrolling through the
//! scrollback, and mouse reports when a full-screen program requested them (Shift always selects).
//!
//! The press, drag and release flow follows Zed's `crates/terminal/src/terminal.rs` (`mouse_down`,
//! `mouse_drag`, `mouse_up`, `scroll_wheel`), zed@d2a4fb6, GPL-3.0-or-later, Copyright Zed Industries.
//!
//! Modified for Sakura in November 2026: this file is a changed adaptation, not Zed's original.

use crate::core::{SelectionKind, ViewportPoint};
use crate::links::Link;
use crate::mouse::{
    alternate_scroll, button_report, cell_at, motion_report, reports_mouse, wheel_reports, Button,
    CellMetrics, MouseMods,
};
use crate::scroll::{GesturePhase, ScrollAmount};
use crate::view::TerminalView;
use alacritty_terminal::index::Side;
use alacritty_terminal::term::TermMode;
use gpui::{
    Context, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point,
    ScrollDelta, ScrollEvent, TouchPhase, Window,
};

/// Pixels the pointer must travel before a press becomes a drag selection (a focusing click often jitters).
const DRAG_THRESHOLD: f32 = 2.0;
/// Most lines a drag beyond the top or bottom edge scrolls per move.
const MAX_DRAG_SCROLL_LINES: i32 = 3;

/// What the pointer is doing between press and release.
#[derive(Default)]
pub(crate) struct PointerState {
    pressed_at: Option<Point<Pixels>>,
    is_selecting: bool,
    reported_button: Option<Button>,
    last_reported_cell: Option<ViewportPoint>,
    /// The address under the pointer while Cmd is held: it is underlined, and a click opens it.
    pub(crate) hovered_link: Option<Link>,
}

impl TerminalView {
    pub(crate) fn mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus, cx);
        // A click shows no ring, also when the view already had focus from the keyboard.
        self.is_keyboard_focus = false;
        let Some((point, side)) = self.cell_under(event.position) else {
            return;
        };
        let is_link_click = event.modifiers.platform && event.button == MouseButton::Left;
        if is_link_click && self.open_link_at(point.line, point.column, cx) {
            return;
        }
        let mode = self.mode(cx);
        if reports_mouse(mode, event.modifiers.shift) {
            self.report_press(point, event, mode, cx);
            return;
        }
        let kind = match (event.button, event.click_count) {
            (MouseButton::Left, 1) => SelectionKind::Simple,
            (MouseButton::Left, 2) => SelectionKind::Word,
            (MouseButton::Left, count) if count >= 3 => SelectionKind::Line,
            _ => return,
        };
        self.pointer.pressed_at = Some(event.position);
        self.pointer.is_selecting = kind != SelectionKind::Simple;
        self.start_selection(kind, point, side, event.modifiers.shift, cx);
    }

    /// Opens in the browser the `http` or `https` address at visible row `line` and `column`. Answers
    /// whether there was one: no other kind of address is ever opened.
    pub fn open_link_at(&mut self, line: usize, column: usize, cx: &mut Context<Self>) -> bool {
        let point = ViewportPoint { line, column };
        let link = self.terminal.read(cx).shared().lock().link_at(point);
        match link {
            Some(link) => {
                cx.open_url(&link.url);
                true
            }
            None => false,
        }
    }

    /// The pointer over the terminal: the text cursor, and the arrow while Cmd is held over an address,
    /// which a press would open (the pointer stays the arrow on everything that can be pressed).
    pub(crate) fn pointer_style(&self) -> gpui::CursorStyle {
        match self.pointer.hovered_link {
            Some(_) => gpui::CursorStyle::Arrow,
            None => gpui::CursorStyle::IBeam,
        }
    }

    /// Underlines the address under `position` while Cmd is held there (`None` when it is not), and
    /// nothing otherwise.
    pub(crate) fn hover_link(&mut self, position: Option<Point<Pixels>>, cx: &mut Context<Self>) {
        let link = position
            .filter(|at| self.is_over_grid(*at))
            .and_then(|at| self.cell_under(at))
            .and_then(|(point, _)| self.terminal.read(cx).shared().lock().link_at(point));
        if link != self.pointer.hovered_link {
            self.pointer.hovered_link = link;
            cx.notify();
        }
    }

    /// Whether a window position is inside the grid.
    fn is_over_grid(&self, position: Point<Pixels>) -> bool {
        self.info.get().geometry.is_some_and(|geometry| {
            let last = geometry.cell_bounds(geometry.rows.saturating_sub(1), 0, geometry.columns);
            let grid = gpui::Bounds::from_corners(geometry.origin, last.bottom_right());
            grid.contains(&position)
        })
    }

    /// Starts a selection, or extends the current one on a Shift press.
    fn start_selection(
        &mut self,
        kind: SelectionKind,
        point: ViewportPoint,
        side: Side,
        is_extending: bool,
        cx: &mut Context<Self>,
    ) {
        let shared = self.terminal.read(cx).shared().clone();
        {
            let mut core = shared.lock();
            let extends = kind == SelectionKind::Simple && is_extending && core.has_selection();
            if !extends || !core.extend_selection(point, side) {
                core.start_selection(kind, point, side);
            }
        }
        self.redraw_grid(cx);
    }

    pub(crate) fn mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        is_hovered: bool,
        cx: &mut Context<Self>,
    ) {
        let over_link = (is_hovered && event.modifiers.platform).then_some(event.position);
        self.hover_link(over_link, cx);
        let is_tracking =
            self.pointer.pressed_at.is_some() || self.pointer.reported_button.is_some();
        if !is_hovered && !is_tracking {
            return;
        }
        let mode = self.mode(cx);
        let wants_motion = self.pointer.reported_button.is_some()
            || (is_hovered && mode.contains(TermMode::MOUSE_MOTION));
        if wants_motion && reports_mouse(mode, event.modifiers.shift) {
            self.report_motion(event, mode, cx);
            return;
        }
        if event.pressed_button == Some(MouseButton::Left) {
            self.drag_selection(event, mode, cx);
        }
    }

    pub(crate) fn mouse_up(&mut self, event: &MouseUpEvent, cx: &mut Context<Self>) {
        self.pointer.pressed_at = None;
        self.pointer.is_selecting = false;
        self.pointer.last_reported_cell = None;
        let Some(button) = self.pointer.reported_button.take() else {
            return;
        };
        let mode = self.mode(cx);
        let point = self.cell_under(event.position).map(|(point, _)| point);
        if let Some(bytes) =
            point.and_then(|point| button_report(point, button, mods(event.modifiers), false, mode))
        {
            self.terminal.read(cx).report(bytes);
        }
    }

    pub(crate) fn scroll_wheel(&mut self, event: &ScrollEvent, cx: &mut Context<Self>) {
        let Some(geometry) = self.info.get().geometry else {
            return;
        };
        let line_height = f32::from(geometry.line_height);
        let lines = self.scroll.lines(
            scroll_amount(event.delta),
            gesture_phase(event.touch_phase),
            line_height,
        );
        if lines == 0 {
            return;
        }
        let mode = self.mode(cx);
        let shift = event.modifiers.shift;
        if reports_mouse(mode, shift) {
            if let Some((point, _)) = self.cell_under(event.position) {
                let bytes = wheel_reports(point, lines, mods(event.modifiers), mode);
                self.terminal.read(cx).report(bytes);
            }
        } else if mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) && !shift {
            self.terminal.read(cx).report(alternate_scroll(lines, mode));
        } else {
            self.terminal.read(cx).shared().lock().scroll(lines);
            self.redraw_grid(cx);
        }
    }

    fn report_press(
        &mut self,
        point: ViewportPoint,
        event: &MouseDownEvent,
        mode: TermMode,
        cx: &mut Context<Self>,
    ) {
        let Some(button) = protocol_button(event.button) else {
            return;
        };
        if let Some(bytes) = button_report(point, button, mods(event.modifiers), true, mode) {
            self.pointer.reported_button = Some(button);
            self.pointer.last_reported_cell = Some(point);
            self.terminal.read(cx).report(bytes);
        }
    }

    fn report_motion(&mut self, event: &MouseMoveEvent, mode: TermMode, cx: &mut Context<Self>) {
        let Some((point, _)) = self.cell_under(event.position) else {
            return;
        };
        if self.pointer.last_reported_cell == Some(point) {
            return;
        }
        self.pointer.last_reported_cell = Some(point);
        let button = event
            .pressed_button
            .and_then(protocol_button)
            .unwrap_or(Button::None);
        if let Some(bytes) = motion_report(point, button, mods(event.modifiers), mode) {
            self.terminal.read(cx).report(bytes);
        }
    }

    fn drag_selection(&mut self, event: &MouseMoveEvent, mode: TermMode, cx: &mut Context<Self>) {
        let Some(pressed_at) = self.pointer.pressed_at else {
            return;
        };
        let moved = (event.position - pressed_at).magnitude();
        if !self.pointer.is_selecting && moved <= f64::from(DRAG_THRESHOLD) {
            return;
        }
        self.pointer.is_selecting = true;
        let Some((point, side)) = self.cell_under(event.position) else {
            return;
        };
        let scroll_lines = self.edge_scroll_lines(event.position.y);
        let shared = self.terminal.read(cx).shared().clone();
        {
            let mut core = shared.lock();
            if scroll_lines != 0 && !mode.contains(TermMode::ALT_SCREEN) {
                core.scroll(scroll_lines);
            }
            core.extend_selection(point, side);
        }
        self.redraw_grid(cx);
    }

    /// Lines to scroll while a drag is above (positive) or below (negative) the grid.
    fn edge_scroll_lines(&self, y: Pixels) -> i32 {
        let Some(geometry) = self.info.get().geometry else {
            return 0;
        };
        let top = geometry.origin.y;
        let bottom = top + geometry.line_height * geometry.rows as f32;
        let lines = |distance: Pixels| (distance / geometry.line_height).ceil() as i32;
        if y < top {
            lines(top - y).min(MAX_DRAG_SCROLL_LINES)
        } else if y > bottom {
            -lines(y - bottom).min(MAX_DRAG_SCROLL_LINES)
        } else {
            0
        }
    }

    /// The cell under a window position, clamped to the grid.
    fn cell_under(&self, position: Point<Pixels>) -> Option<(ViewportPoint, Side)> {
        let geometry = self.info.get().geometry?;
        let metrics = CellMetrics {
            cell_width: f32::from(geometry.cell_width),
            line_height: f32::from(geometry.line_height),
            columns: geometry.columns,
            rows: geometry.rows,
        };
        let offset = position - geometry.origin;
        Some(cell_at(f32::from(offset.x), f32::from(offset.y), metrics))
    }

    /// Selection and scrolling change only what the grid draws.
    fn redraw_grid(&self, cx: &mut Context<Self>) {
        self.terminal.update(cx, |_, cx| cx.notify());
    }
}

fn protocol_button(button: MouseButton) -> Option<Button> {
    match button {
        MouseButton::Left => Some(Button::Left),
        MouseButton::Middle => Some(Button::Middle),
        MouseButton::Right => Some(Button::Right),
        MouseButton::Navigate(_) => None,
    }
}

fn mods(modifiers: Modifiers) -> MouseMods {
    MouseMods {
        shift: modifiers.shift,
        alt: modifiers.alt,
        control: modifiers.control,
    }
}

fn scroll_amount(delta: ScrollDelta) -> ScrollAmount {
    match delta {
        ScrollDelta::Pixels(pixels) => ScrollAmount::Pixels(f32::from(pixels.y)),
        ScrollDelta::Lines(lines) => ScrollAmount::Notches(lines.y),
    }
}

fn gesture_phase(phase: TouchPhase) -> GesturePhase {
    match phase {
        TouchPhase::Started => GesturePhase::Started,
        TouchPhase::Moved => GesturePhase::Moved,
        TouchPhase::Ended | TouchPhase::Cancelled => GesturePhase::Ended,
    }
}
