//! The input a scripted run (`SAKURA_SNAPSHOT_DRIVE`) gives a window as a person would: a click at a point, the
//! pointer left at a point, a drag from one point to another (or its parts: hold, pull, let go), a turn of the wheel at a point, one keystroke, text typed key by key. Each goes into the window's own event
//! pipeline, so hit-testing, focus and every handler run as they do for real input. Snapshot builds only,
//! which are debug builds (`drive_guard`): this file does not exist in any other.

use gpui::{Keystroke, Window};

/// The point `x,y` names, in points of the window.
fn place(at: &str) -> Option<gpui::Point<gpui::Pixels>> {
    let (x, y) = at.split_once(',')?;
    let (x, y) = (x.trim().parse::<f32>().ok()?, y.trim().parse::<f32>().ok()?);
    Some(gpui::point(gpui::px(x), gpui::px(y)))
}

#[cfg(not(feature = "boxes"))]
/// How many moves a drag makes between its two points: enough for a drag to start and for the thing under
/// the pointer to follow it, as a hand makes them.
const DRAG_MOVES: u32 = 8;

#[cfg(not(feature = "boxes"))]
/// The four numbers `x1,y1,x2,y2` name: where a drag starts and where it ends.
fn course(at: &str) -> Option<(gpui::Point<gpui::Pixels>, gpui::Point<gpui::Pixels>)> {
    let mut numbers = at
        .split(',')
        .map(|number| number.trim().parse::<f32>().ok());
    let mut next = || numbers.next().flatten();
    let (x1, y1, x2, y2) = (next()?, next()?, next()?, next()?);
    numbers.next().is_none().then(|| {
        (
            gpui::point(gpui::px(x1), gpui::px(y1)),
            gpui::point(gpui::px(x2), gpui::px(y2)),
        )
    })
}

#[cfg(not(feature = "boxes"))]
/// The points a drag passes through after it starts, the last being where it ends.
fn moves(
    from: gpui::Point<gpui::Pixels>,
    to: gpui::Point<gpui::Pixels>,
) -> Vec<gpui::Point<gpui::Pixels>> {
    (1..=DRAG_MOVES)
        .map(|step| {
            let share = step as f32 / DRAG_MOVES as f32;
            gpui::point(
                from.x + (to.x - from.x) * share,
                from.y + (to.y - from.y) * share,
            )
        })
        .collect()
}

#[cfg(not(feature = "boxes"))]
/// The three numbers `x,y,dy` name: where the wheel turns and by how much (negative scrolls the content up).
fn wheel(at: &str) -> Option<(gpui::Point<gpui::Pixels>, f32)> {
    let mut numbers = at
        .split(',')
        .map(|number| number.trim().parse::<f32>().ok());
    let mut next = || numbers.next().flatten();
    let (x, y, dy) = (next()?, next()?, next()?);
    numbers
        .next()
        .is_none()
        .then(|| (gpui::point(gpui::px(x), gpui::px(y)), dy))
}

#[cfg(not(feature = "boxes"))]
/// The wheel turned by `dy` points at `x,y`, through the window's own event pipeline: what a mouse wheel or a
/// trackpad does to the list under the pointer.
pub fn scroll(at: &str, window: &mut Window, cx: &mut gpui::App) {
    let Some((position, dy)) = wheel(at) else {
        return eprintln!("sakura: a scroll needs x,y,dy in numbers, not {at:?}");
    };
    let turned = gpui::ScrollEvent {
        position,
        delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(dy))),
        modifiers: gpui::Modifiers::default(),
        touch_phase: gpui::TouchPhase::Moved,
        ..Default::default()
    };
    window.dispatch_event(gpui::PlatformInput::Scroll(turned), cx);
}

#[cfg(not(feature = "boxes"))]
/// The left button pressed at `position`.
fn press_at(position: gpui::Point<gpui::Pixels>, window: &mut Window, cx: &mut gpui::App) {
    let down = gpui::MouseDownEvent {
        button: gpui::MouseButton::Left,
        position,
        modifiers: gpui::Modifiers::default(),
        click_count: 1,
        first_mouse: false,
    };
    window.dispatch_event(gpui::PlatformInput::MouseDown(down), cx);
}

#[cfg(not(feature = "boxes"))]
/// The pointer moved to `position` with the left button held.
fn pull_to(position: gpui::Point<gpui::Pixels>, window: &mut Window, cx: &mut gpui::App) {
    let moved = gpui::MouseMoveEvent {
        position,
        pressed_button: Some(gpui::MouseButton::Left),
        modifiers: gpui::Modifiers::default(),
    };
    window.dispatch_event(gpui::PlatformInput::MouseMove(moved), cx);
}

#[cfg(not(feature = "boxes"))]
/// The left button released at `position`.
fn release_at(position: gpui::Point<gpui::Pixels>, window: &mut Window, cx: &mut gpui::App) {
    let up = gpui::MouseUpEvent {
        button: gpui::MouseButton::Left,
        position,
        modifiers: gpui::Modifiers::default(),
        click_count: 1,
    };
    window.dispatch_event(gpui::PlatformInput::MouseUp(up), cx);
}

#[cfg(not(feature = "boxes"))]
/// The left button pressed at `x,y` and kept down: the first part of a drag a script photographs on its way.
pub fn hold(at: &str, window: &mut Window, cx: &mut gpui::App) {
    match place(at) {
        Some(position) => press_at(position, window, cx),
        None => eprintln!("sakura: a hold needs x,y in numbers, not {at:?}"),
    }
}

#[cfg(not(feature = "boxes"))]
/// The pointer moved to `x,y` with the left button held.
pub fn pull(at: &str, window: &mut Window, cx: &mut gpui::App) {
    match place(at) {
        Some(position) => pull_to(position, window, cx),
        None => eprintln!("sakura: a pull needs x,y in numbers, not {at:?}"),
    }
}

#[cfg(not(feature = "boxes"))]
/// The left button released at `x,y`.
pub fn let_go(at: &str, window: &mut Window, cx: &mut gpui::App) {
    match place(at) {
        Some(position) => release_at(position, window, cx),
        None => eprintln!("sakura: a let-go needs x,y in numbers, not {at:?}"),
    }
}

#[cfg(not(feature = "boxes"))]
/// The left button pressed at `x1,y1`, the pointer moved to `x2,y2` in steps with the button held, and the
/// button released there: what a hand does to drag a tab.
pub fn drag(at: &str, window: &mut Window, cx: &mut gpui::App) {
    let Some((from, to)) = course(at) else {
        return eprintln!("sakura: a drag needs x1,y1,x2,y2 in numbers, not {at:?}");
    };
    press_at(from, window, cx);
    for position in moves(from, to) {
        pull_to(position, window, cx);
    }
    release_at(to, window, cx);
}

#[cfg_attr(not(feature = "boxes"), allow(dead_code))]
pub fn hover(at: &str, window: &mut Window, cx: &mut gpui::App) {
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

pub fn click(at: &str, window: &mut Window, cx: &mut gpui::App) {
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

/// One keystroke, named as a key binding names it.
pub fn press(keystroke: &str, window: &mut Window, cx: &mut gpui::App) {
    match Keystroke::parse_with_os(keystroke, cx.platform_info().os) {
        Ok(keystroke) => drop(window.dispatch_keystroke(keystroke, cx)),
        Err(error) => eprintln!("sakura: {keystroke:?} is not a keystroke: {error}"),
    }
}

/// Types `text` key by key into whatever has the keyboard.
pub fn press_each(text: &str, window: &mut Window, cx: &mut gpui::App) {
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

#[cfg(all(test, not(feature = "boxes")))]
mod tests {
    use super::*;
    use gpui::{point, px};

    #[test]
    fn a_drag_names_four_numbers_and_nothing_else() {
        assert_eq!(
            course("10, 20,300.5,40"),
            Some((point(px(10.), px(20.)), point(px(300.5), px(40.))))
        );
        assert_eq!(course("10,20,30"), None);
        assert_eq!(course("10,20,30,40,50"), None);
        assert_eq!(course("a,b,c,d"), None);
    }

    #[test]
    fn a_drag_moves_in_steps_and_ends_where_it_was_told() {
        let path = moves(point(px(0.), px(10.)), point(px(80.), px(10.)));
        assert_eq!(path.len(), DRAG_MOVES as usize);
        assert_eq!(path.last(), Some(&point(px(80.), px(10.))));
        assert!(path.windows(2).all(|pair| pair[0].x < pair[1].x));
    }
}
