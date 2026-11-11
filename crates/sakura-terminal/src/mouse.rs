//! Mouse positions as grid cells, and mouse events as the reports full-screen programs ask for (X10, UTF-8
//! and SGR encodings).
//!
//! Adapted from Zed's `crates/terminal/src/mappings/mouse.rs` (zed@d2a4fb6, GPL-3.0-or-later, Copyright Zed
//! Industries), whose constants come from Alacritty.
//!
//! Modified for Sakura in November 2026: this file is a changed adaptation, not Zed's original.

use crate::core::ViewportPoint;
use alacritty_terminal::index::Side;
use alacritty_terminal::term::TermMode;

/// Largest coordinate the X10 encoding can carry.
const X10_LIMIT: usize = 223;
/// Largest coordinate the UTF-8 encoding can carry.
const UTF8_LIMIT: usize = 2015;

/// A mouse button as the terminal protocol numbers it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Button {
    Left = 0,
    Middle = 1,
    Right = 2,
    /// Motion with no button pressed.
    None = 3,
    WheelUp = 64,
    WheelDown = 65,
}

/// Modifier keys held during a mouse event.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct MouseMods {
    pub shift: bool,
    pub alt: bool,
    pub control: bool,
}

impl MouseMods {
    fn bits(self) -> u8 {
        4 * u8::from(self.shift) + 8 * u8::from(self.alt) + 16 * u8::from(self.control)
    }
}

/// Size of the grid and of a cell, to turn pixel offsets into cells.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CellMetrics {
    pub cell_width: f32,
    pub line_height: f32,
    pub columns: usize,
    pub rows: usize,
}

/// The cell under a position given in pixels from the grid's top left corner, clamped to the grid, with the
/// half of the cell it falls in (selection uses the side to include or skip the cell).
pub(crate) fn cell_at(x: f32, y: f32, metrics: CellMetrics) -> (ViewportPoint, Side) {
    let last_column = metrics.columns.saturating_sub(1);
    let last_line = metrics.rows.saturating_sub(1);
    let column = (x.max(0.0) / metrics.cell_width) as usize;
    let within = x.max(0.0) % metrics.cell_width;
    let mut side = if within > metrics.cell_width / 2.0 {
        Side::Right
    } else {
        Side::Left
    };
    if column > last_column {
        side = Side::Right;
    }
    let line = if y < 0.0 {
        side = Side::Left;
        0
    } else {
        let line = (y / metrics.line_height) as usize;
        if line > last_line {
            side = Side::Right;
        }
        line.min(last_line)
    };
    let point = ViewportPoint {
        line,
        column: column.min(last_column),
    };
    (point, side)
}

/// Whether mouse events go to the program instead of selecting text. Shift always selects.
pub(crate) fn reports_mouse(mode: TermMode, shift: bool) -> bool {
    mode.intersects(TermMode::MOUSE_MODE) && !shift
}

/// The report for a button press or release, when the program requested clicks.
pub(crate) fn button_report(
    point: ViewportPoint,
    button: Button,
    mods: MouseMods,
    pressed: bool,
    mode: TermMode,
) -> Option<Vec<u8>> {
    if !mode.intersects(TermMode::MOUSE_MODE) {
        return None;
    }
    report(point, button as u8 + mods.bits(), pressed, mode)
}

/// The report for motion, when the program requested motion (any) or drags (with a button held).
pub(crate) fn motion_report(
    point: ViewportPoint,
    button: Button,
    mods: MouseMods,
    mode: TermMode,
) -> Option<Vec<u8>> {
    let wants_motion = mode.contains(TermMode::MOUSE_MOTION)
        || (mode.contains(TermMode::MOUSE_DRAG) && button != Button::None);
    if !wants_motion {
        return None;
    }
    report(point, 32 + button as u8 + mods.bits(), true, mode)
}

/// Wheel reports, one per line scrolled; positive lines scroll up.
pub(crate) fn wheel_reports(
    point: ViewportPoint,
    lines: i32,
    mods: MouseMods,
    mode: TermMode,
) -> Vec<u8> {
    let button = if lines > 0 {
        Button::WheelUp
    } else {
        Button::WheelDown
    };
    let one = button_report(point, button, mods, true, mode).unwrap_or_default();
    one.repeat(lines.unsigned_abs() as usize)
}

/// Arrow keys for the wheel in the alternate screen, for programs that do not take the mouse (less, man).
pub(crate) fn alternate_scroll(lines: i32, mode: TermMode) -> Vec<u8> {
    let arrow: &[u8] = match (lines > 0, mode.contains(TermMode::APP_CURSOR)) {
        (true, true) => b"\x1bOA",
        (true, false) => b"\x1b[A",
        (false, true) => b"\x1bOB",
        (false, false) => b"\x1b[B",
    };
    arrow.repeat(lines.unsigned_abs() as usize)
}

fn report(point: ViewportPoint, code: u8, pressed: bool, mode: TermMode) -> Option<Vec<u8>> {
    if mode.contains(TermMode::SGR_MOUSE) {
        let end = if pressed { 'M' } else { 'm' };
        let text = format!("\x1b[<{code};{};{}{end}", point.column + 1, point.line + 1);
        return Some(text.into_bytes());
    }
    // X10 and UTF-8 report a release as button 3, keeping the modifier bits.
    let code = if pressed { code } else { 3 + (code & 0b1_1100) };
    normal_report(point, code, mode.contains(TermMode::UTF8_MOUSE))
}

fn normal_report(point: ViewportPoint, code: u8, utf8: bool) -> Option<Vec<u8>> {
    let limit = if utf8 { UTF8_LIMIT } else { X10_LIMIT };
    if point.line >= limit || point.column >= limit {
        return None;
    }
    let mut message = vec![b'\x1b', b'[', b'M', 32 + code];
    for position in [point.column, point.line] {
        let value = 32 + 1 + position;
        if utf8 && position >= 95 {
            message.push((0xC0 + value / 64) as u8);
            message.push((0x80 + (value & 63)) as u8);
        } else {
            message.push(value as u8);
        }
    }
    Some(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    const METRICS: CellMetrics = CellMetrics {
        cell_width: 10.0,
        line_height: 20.0,
        columns: 80,
        rows: 24,
    };

    fn at(line: usize, column: usize) -> ViewportPoint {
        ViewportPoint { line, column }
    }

    #[test]
    fn mouse_cell_at_maps_pixels_to_cells_with_side_and_clamps() {
        assert_eq!(cell_at(15.0, 45.0, METRICS), (at(2, 1), Side::Left));
        assert_eq!(cell_at(17.0, 45.0, METRICS), (at(2, 1), Side::Right));
        assert_eq!(cell_at(5000.0, 5000.0, METRICS), (at(23, 79), Side::Right));
        assert_eq!(cell_at(-3.0, -3.0, METRICS), (at(0, 0), Side::Left));
    }

    #[test]
    fn mouse_sgr_reports_press_and_release_one_based() {
        let mode = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;
        let mods = MouseMods::default();
        let press = button_report(at(4, 9), Button::Left, mods, true, mode);
        assert_eq!(press.as_deref(), Some(&b"\x1b[<0;10;5M"[..]));
        let release = button_report(at(4, 9), Button::Left, mods, false, mode);
        assert_eq!(release.as_deref(), Some(&b"\x1b[<0;10;5m"[..]));
        let ctrl = MouseMods {
            control: true,
            ..mods
        };
        let right = button_report(at(0, 0), Button::Right, ctrl, true, mode);
        assert_eq!(right.as_deref(), Some(&b"\x1b[<18;1;1M"[..]));
    }

    #[test]
    fn mouse_x10_reports_release_as_button_three() {
        let mode = TermMode::MOUSE_REPORT_CLICK;
        let mods = MouseMods::default();
        let press = button_report(at(0, 0), Button::Left, mods, true, mode);
        assert_eq!(press.as_deref(), Some(&[0x1b, b'[', b'M', 32, 33, 33][..]));
        let release = button_report(at(0, 0), Button::Left, mods, false, mode);
        assert_eq!(
            release.as_deref(),
            Some(&[0x1b, b'[', b'M', 35, 33, 33][..])
        );
        assert_eq!(
            button_report(at(0, 300), Button::Left, mods, true, mode),
            None
        );
    }

    #[test]
    fn mouse_utf8_encodes_large_coordinates_in_two_bytes() {
        let mode = TermMode::MOUSE_REPORT_CLICK | TermMode::UTF8_MOUSE;
        let report =
            button_report(at(0, 100), Button::Left, MouseMods::default(), true, mode).unwrap();
        assert_eq!(report, vec![0x1b, b'[', b'M', 32, 0xC2, 0x85, 33]);
    }

    #[test]
    fn mouse_motion_is_reported_only_in_the_mode_that_asks_for_it() {
        let mods = MouseMods::default();
        let drag = TermMode::MOUSE_DRAG | TermMode::SGR_MOUSE;
        assert_eq!(motion_report(at(1, 1), Button::None, mods, drag), None);
        let dragged = motion_report(at(1, 1), Button::Left, mods, drag);
        assert_eq!(dragged.as_deref(), Some(&b"\x1b[<32;2;2M"[..]));
        let any = TermMode::MOUSE_MOTION | TermMode::SGR_MOUSE;
        let moved = motion_report(at(1, 1), Button::None, mods, any);
        assert_eq!(moved.as_deref(), Some(&b"\x1b[<35;2;2M"[..]));
        assert_eq!(
            motion_report(at(1, 1), Button::Left, mods, TermMode::NONE),
            None
        );
    }

    #[test]
    fn mouse_wheel_reports_repeat_per_line_and_alternate_scroll_sends_arrows() {
        let mode = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;
        let reports = wheel_reports(at(0, 0), -2, MouseMods::default(), mode);
        assert_eq!(reports, b"\x1b[<65;1;1M\x1b[<65;1;1M".to_vec());
        assert_eq!(
            alternate_scroll(2, TermMode::NONE),
            b"\x1b[A\x1b[A".to_vec()
        );
        assert_eq!(
            alternate_scroll(-1, TermMode::APP_CURSOR),
            b"\x1bOB".to_vec()
        );
    }

    #[test]
    fn mouse_shift_always_selects() {
        assert!(reports_mouse(TermMode::MOUSE_REPORT_CLICK, false));
        assert!(!reports_mouse(TermMode::MOUSE_REPORT_CLICK, true));
        assert!(!reports_mouse(TermMode::NONE, false));
    }
}
