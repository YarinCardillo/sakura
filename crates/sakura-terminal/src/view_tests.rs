//! The view in a headless GPUI window: keys, paste, copy, focus reports, mouse selection and reports, wheel
//! scrolling, resize, titles, links, input methods, output coalescing and the cursor blink. Run with
//! `cargo test --features test-support`.

use crate::palette::TerminalPalette;
use crate::size::GridSize;
use crate::source::{OutputSink, SourceError, TerminalSource, MAX_QUEUED_BYTES};
use crate::style::TerminalStyle;
use crate::terminal::TerminalChange;
use crate::view::TerminalView;
use gpui::{
    point, px, size, ClipboardItem, Entity, EntityInputHandler, Focusable, Modifiers, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollDelta, ScrollEvent,
    TestAppContext, TouchPhase, VisualTestContext,
};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// A source that records what the terminal writes and lets the test play the child's output.
#[derive(Clone, Default)]
struct FakeSource {
    state: Arc<Mutex<FakeState>>,
}

#[derive(Default)]
struct FakeState {
    written: Vec<u8>,
    sizes: Vec<GridSize>,
    sink: Option<OutputSink>,
    is_stuck: bool,
}

impl TerminalSource for FakeSource {
    fn start(&self, sink: OutputSink) {
        self.state.lock().unwrap().sink = Some(sink);
    }

    fn write(&self, bytes: &[u8]) -> Result<(), SourceError> {
        while self.state.lock().unwrap().is_stuck {
            std::thread::sleep(Duration::from_millis(2));
        }
        self.state.lock().unwrap().written.extend_from_slice(bytes);
        Ok(())
    }

    fn resize(&self, size: GridSize) -> Result<(), SourceError> {
        self.state.lock().unwrap().sizes.push(size);
        Ok(())
    }

    fn close(&self) {
        self.state.lock().unwrap().is_stuck = false;
    }
}

impl FakeSource {
    /// Plays output as if the child wrote it.
    fn output(&self, text: &str) {
        let sink = self.state.lock().unwrap().sink.clone().expect("started");
        sink.bytes(text.as_bytes());
    }

    /// Waits (in real time: writes go through the writer thread) until the written bytes end with `tail`.
    fn wait_written(&self, tail: &[u8]) -> Vec<u8> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let written = self.state.lock().unwrap().written.clone();
            if written.ends_with(tail) || Instant::now() > deadline {
                return written;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn clear_written(&self) {
        self.state.lock().unwrap().written.clear();
    }

    fn written(&self) -> Vec<u8> {
        self.state.lock().unwrap().written.clone()
    }

    /// Waits (in real time, through the writer thread) until `count` resizes reached the child.
    fn wait_sizes(&self, count: usize) -> Vec<GridSize> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let sizes = self.state.lock().unwrap().sizes.clone();
            if sizes.len() >= count || Instant::now() > deadline {
                return sizes;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

struct Rig<'a> {
    view: Entity<TerminalView>,
    cx: &'a mut VisualTestContext,
    source: FakeSource,
}

fn rig(app: &mut TestAppContext) -> Rig<'_> {
    let source = FakeSource::default();
    let started = source.clone();
    let style = TerminalStyle::new("Geist Mono", TerminalPalette::one_dark(0x000000));
    let (view, cx) =
        app.add_window_view(|window, cx| TerminalView::for_source(started, style, window, cx));
    cx.update(|window, cx| {
        window.activate_window();
        window.focus(&view.focus_handle(cx), cx);
    });
    cx.run_until_parked();
    Rig { view, cx, source }
}

impl Rig<'_> {
    fn output(&mut self, text: &str) {
        self.source.output(text);
        self.cx.run_until_parked();
    }

    /// The window position of the middle of cell (`line`, `column`).
    fn cell_center(&mut self, line: usize, column: usize) -> Point<Pixels> {
        self.cell_point(line, column, 0.5)
    }

    /// The window position `fraction` of the way across cell (`line`, `column`), at mid height.
    fn cell_point(&mut self, line: usize, column: usize, fraction: f32) -> Point<Pixels> {
        let geometry = self
            .view
            .read_with(self.cx, |view, _| view.info.get().geometry);
        let geometry = geometry.expect("the grid was laid out");
        let bounds = geometry.cell_bounds(line, column, 1);
        point(
            bounds.origin.x + bounds.size.width * fraction,
            bounds.origin.y + bounds.size.height / 2.,
        )
    }

    fn press(&mut self, position: Point<Pixels>, click_count: usize) {
        self.cx.simulate_event(MouseDownEvent {
            button: MouseButton::Left,
            position,
            modifiers: Modifiers::default(),
            click_count,
            first_mouse: false,
        });
    }

    fn release(&mut self, position: Point<Pixels>) {
        self.cx.simulate_event(MouseUpEvent {
            button: MouseButton::Left,
            position,
            modifiers: Modifiers::default(),
            click_count: 1,
        });
    }

    fn drag_to(&mut self, position: Point<Pixels>) {
        self.cx.simulate_event(MouseMoveEvent {
            position,
            pressed_button: Some(MouseButton::Left),
            modifiers: Modifiers::default(),
        });
    }

    fn selection(&mut self) -> Option<String> {
        self.view.read_with(self.cx, |view, cx| {
            view.terminal.read(cx).shared().lock().selection_text()
        })
    }

    fn grid_layouts(&mut self) -> u64 {
        self.view
            .read_with(self.cx, |view, _| view.stats().grid_layouts)
    }
}

#[gpui::test]
fn view_typed_text_and_keys_reach_the_child(app: &mut TestAppContext) {
    let mut rig = rig(app);
    rig.cx.simulate_keystrokes("a b enter up ctrl-c");
    let written = rig.source.wait_written(b"\x03");
    assert_eq!(written, b"ab\r\x1b[A\x03");
    rig.source.clear_written();
    rig.output("\x1b[?1h");
    rig.cx.simulate_keystrokes("up");
    assert_eq!(rig.source.wait_written(b"\x1bOA"), b"\x1bOA");
}

#[gpui::test]
fn view_copy_takes_the_selection_and_paste_is_bracketed(app: &mut TestAppContext) {
    let mut rig = rig(app);
    rig.output("hello world");
    // From the left edge of "h" to past the middle of "o": both cells are in.
    let (from, to) = (rig.cell_point(0, 0, 0.1), rig.cell_point(0, 4, 0.75));
    rig.press(from, 1);
    rig.drag_to(to);
    rig.release(to);
    assert_eq!(rig.selection().as_deref(), Some("hello"));
    rig.cx.simulate_keystrokes("cmd-c");
    let copied = rig.cx.read_from_clipboard().and_then(|item| item.text());
    assert_eq!(copied.as_deref(), Some("hello"));
    rig.output("\x1b[?2004h");
    rig.cx
        .write_to_clipboard(ClipboardItem::new_string("x\ny".into()));
    rig.cx.simulate_keystrokes("cmd-v");
    assert_eq!(
        rig.source.wait_written(b"\x1b[201~"),
        b"\x1b[200~x\ny\x1b[201~"
    );
}

#[gpui::test]
fn view_paste_without_bracketing_sends_no_line_break_or_control_character(
    app: &mut TestAppContext,
) {
    let rig = rig(app);
    rig.cx.write_to_clipboard(ClipboardItem::new_string(
        "rm -rf x\nls\r\n\x03\x1b[201~end".into(),
    ));
    rig.cx.simulate_keystrokes("cmd-v");
    assert_eq!(
        rig.source.wait_written(b"[201~end"),
        b"rm -rf x ls [201~end"
    );
}

#[gpui::test]
fn view_query_flood_to_a_child_that_never_reads_stays_bounded_and_refuses_a_paste(
    app: &mut TestAppContext,
) {
    let mut rig = rig(app);
    rig.source.state.lock().unwrap().is_stuck = true;
    let terminal = rig
        .view
        .read_with(rig.cx, |view, _| view.terminal().clone());
    let changes = Rc::new(RefCell::new(Vec::new()));
    let log = changes.clone();
    rig.cx.update(|_, cx| {
        cx.subscribe(&terminal, move |_, change: &TerminalChange, _| {
            log.borrow_mut().push(change.clone());
        })
        .detach();
    });
    // Each cursor position query is answered with six bytes: 400 chunks of 1000 are 2.4 MB of replies.
    for _ in 0..400 {
        rig.output(&"\x1b[6n".repeat(1000));
    }
    let queued = terminal.read_with(rig.cx, |terminal, _| terminal.shared().queued_bytes());
    assert!(queued <= MAX_QUEUED_BYTES, "{queued} bytes queued");
    assert!(queued > MAX_QUEUED_BYTES - 6006, "{queued} bytes queued");
    rig.cx
        .write_to_clipboard(ClipboardItem::new_string("x".repeat(10_000)));
    rig.cx.simulate_keystrokes("cmd-v");
    assert_eq!(changes.borrow().as_slice(), &[TerminalChange::Bell]);
    assert!(rig.source.written().is_empty());
}

#[gpui::test]
fn view_double_and_triple_click_select_word_and_line(app: &mut TestAppContext) {
    let mut rig = rig(app);
    rig.output("one two three\r\nsecond");
    let word = rig.cell_center(0, 5);
    rig.press(word, 2);
    rig.release(word);
    assert_eq!(rig.selection().as_deref(), Some("two"));
    let line = rig.cell_center(1, 2);
    rig.press(line, 3);
    rig.release(line);
    assert_eq!(rig.selection().as_deref(), Some("second\n"));
}

#[gpui::test]
fn view_mouse_goes_to_the_program_when_it_asks_and_shift_still_selects(app: &mut TestAppContext) {
    let mut rig = rig(app);
    rig.output("abc\x1b[?1000h\x1b[?1006h");
    let cell = rig.cell_center(0, 2);
    rig.press(cell, 1);
    rig.release(cell);
    assert_eq!(rig.source.wait_written(b"m"), b"\x1b[<0;3;1M\x1b[<0;3;1m");
    assert_eq!(rig.selection(), None);
}

#[gpui::test]
fn view_focus_changes_are_reported_when_the_program_asks(app: &mut TestAppContext) {
    let mut rig = rig(app);
    rig.output("\x1b[?1004h");
    rig.cx.deactivate_window();
    assert_eq!(rig.source.wait_written(b"\x1b[O"), b"\x1b[O");
    rig.cx.update(|window, _| window.activate_window());
    rig.cx.run_until_parked();
    assert!(rig.source.wait_written(b"\x1b[I").ends_with(b"\x1b[I"));
}

#[gpui::test]
fn view_wheel_scrolls_the_scrollback_by_whole_lines(app: &mut TestAppContext) {
    let mut rig = rig(app);
    let lines: String = (0..200).map(|i| format!("line {i}\r\n")).collect();
    rig.output(&lines);
    let line_height = rig.view.read_with(rig.cx, |view, _| {
        view.info.get().geometry.map(|g| g.line_height)
    });
    let line_height = f32::from(line_height.expect("laid out"));
    let position = rig.cell_center(0, 0);
    for (phase, pixels) in [
        (TouchPhase::Started, 0.),
        (TouchPhase::Moved, line_height * 3.5),
    ] {
        rig.cx.simulate_event(ScrollEvent {
            position,
            delta: ScrollDelta::Pixels(point(px(0.), px(pixels))),
            modifiers: Modifiers::default(),
            touch_phase: phase,
            source: Default::default(),
        });
    }
    let offset = rig.view.read_with(rig.cx, |view, cx| {
        view.terminal.read(cx).shared().lock().display_offset()
    });
    assert_eq!(offset, 3);
}

#[gpui::test]
fn view_flood_of_output_is_laid_out_once(app: &mut TestAppContext) {
    let mut rig = rig(app);
    let before = rig.grid_layouts();
    for i in 0..2000 {
        rig.source.output(&format!("y {i}\r\n"));
    }
    rig.cx.run_until_parked();
    let layouts = rig.grid_layouts() - before;
    assert!(
        (1..=2).contains(&layouts),
        "2000 chunks of output took {layouts} grid layouts"
    );
}

#[gpui::test]
fn view_blink_repaints_the_cursor_without_laying_out_the_grid(app: &mut TestAppContext) {
    let mut rig = rig(app);
    rig.output("prompt$ ");
    let layouts = rig.grid_layouts();
    let paints = rig
        .view
        .read_with(rig.cx, |view, _| view.stats().view_paints);
    let mut shown = Vec::new();
    for _ in 0..6 {
        rig.cx.executor().advance_clock(Duration::from_millis(600));
        rig.cx.run_until_parked();
        shown.push(rig.view.read_with(rig.cx, |view, _| view.is_cursor_shown));
    }
    assert_eq!(shown, vec![false, true, false, true, false, true]);
    let repaints = rig
        .view
        .read_with(rig.cx, |view, _| view.stats().view_paints)
        - paints;
    assert!(repaints >= 6, "the cursor was painted {repaints} times");
    assert_eq!(
        rig.grid_layouts(),
        layouts,
        "a blink laid the grid out again"
    );
}

#[gpui::test]
fn view_input_into_an_inactive_window_does_not_start_blinking(app: &mut TestAppContext) {
    let rig = rig(app);
    rig.cx.deactivate_window();
    let paints = rig
        .view
        .read_with(rig.cx, |view, _| view.stats().view_paints);
    rig.view
        .update(rig.cx, |view, cx| view.send_input(b"x".to_vec(), cx));
    for _ in 0..4 {
        rig.cx.executor().advance_clock(Duration::from_millis(600));
        rig.cx.run_until_parked();
    }
    assert!(rig.view.read_with(rig.cx, |view, _| view.is_cursor_shown));
    let repaints = rig
        .view
        .read_with(rig.cx, |view, _| view.stats().view_paints)
        - paints;
    assert!(
        repaints <= 1,
        "an unfocused view repainted {repaints} times"
    );
}

#[gpui::test]
fn view_resize_reflows_at_once_and_tells_the_child_once_resizing_pauses(app: &mut TestAppContext) {
    let rig = rig(app);
    rig.cx.executor().advance_clock(Duration::from_millis(100));
    rig.cx.run_until_parked();
    let settled = rig.source.wait_sizes(1).len();
    for width in [720., 680., 640.] {
        rig.cx.simulate_resize(size(px(width), px(400.)));
        rig.cx.executor().advance_clock(Duration::from_millis(10));
        rig.cx.run_until_parked();
    }
    let grid = rig.view.read_with(rig.cx, |view, cx| {
        view.terminal.read(cx).shared().lock().size()
    });
    rig.cx.executor().advance_clock(Duration::from_millis(100));
    rig.cx.run_until_parked();
    let sizes = rig.source.wait_sizes(settled + 1);
    std::thread::sleep(Duration::from_millis(50));
    let sizes_later = rig.source.wait_sizes(settled + 1);
    assert_eq!(sizes, sizes_later, "more resizes arrived later");
    assert_eq!(
        &sizes[settled..],
        &[grid],
        "the child hears only the final size"
    );
}

#[gpui::test]
fn view_title_from_the_program_reaches_the_app(app: &mut TestAppContext) {
    let mut rig = rig(app);
    let terminal = rig
        .view
        .read_with(rig.cx, |view, _| view.terminal().clone());
    let changes = Rc::new(RefCell::new(Vec::new()));
    let log = changes.clone();
    rig.cx.update(|_, cx| {
        cx.subscribe(&terminal, move |_, change: &TerminalChange, _| {
            log.borrow_mut().push(change.clone());
        })
        .detach();
    });
    rig.output("\x1b]0;build: ok\x07");
    assert_eq!(
        changes.borrow().as_slice(),
        &[TerminalChange::TitleChanged(Some("build: ok".into()))]
    );
    let title = terminal.read_with(rig.cx, |terminal, _| terminal.title().map(str::to_string));
    assert_eq!(title.as_deref(), Some("build: ok"));
}

#[gpui::test]
fn view_hyperlinks_are_never_opened(app: &mut TestAppContext) {
    let mut rig = rig(app);
    rig.output("\x1b]8;;https://example.com\x1b\\link\x1b]8;;\x1b\\");
    let link = rig.cell_center(0, 1);
    rig.press(link, 1);
    rig.release(link);
    rig.cx.simulate_event(MouseDownEvent {
        button: MouseButton::Left,
        position: link,
        modifiers: Modifiers::command(),
        click_count: 1,
        first_mouse: false,
    });
    rig.release(link);
    assert_eq!(rig.cx.opened_url(), None);
}

#[gpui::test]
fn view_input_method_composes_at_the_cursor_then_commits(app: &mut TestAppContext) {
    let rig = rig(app);
    let view = rig.view.clone();
    rig.cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.replace_and_mark_text_in_range(None, "に", None, window, cx);
        });
    });
    let marked = rig
        .view
        .read_with(rig.cx, |view, _| view.marked_text.clone());
    assert_eq!(marked.as_deref(), Some("に"));
    std::thread::sleep(Duration::from_millis(50));
    assert!(
        rig.source.written().is_empty(),
        "composing text reached the child"
    );
    rig.cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.replace_text_in_range(None, "日本", window, cx)
        });
    });
    assert_eq!(
        rig.source.wait_written("日本".as_bytes()),
        "日本".as_bytes()
    );
    assert_eq!(
        rig.view
            .read_with(rig.cx, |view, _| view.marked_text.clone()),
        None
    );
}

gpui::actions!(view_tests, [FocusNext]);

#[gpui::test]
fn view_tab_reaches_the_terminal_with_a_ring_and_a_click_takes_the_ring_away(
    app: &mut TestAppContext,
) {
    let mut rig = rig(app);
    // The app moves focus on Tab from wherever nothing deeper takes the key; here as in `sakura-app`.
    rig.cx.update(|window, cx| {
        window.blur(cx);
        cx.bind_keys([gpui::KeyBinding::new("tab", FocusNext, None)]);
        let handle = window.window_handle();
        cx.on_action(move |_: &FocusNext, cx| {
            cx.defer(move |cx| drop(handle.update(cx, |_, window, cx| window.focus_next(cx))));
        });
    });
    rig.cx.run_until_parked();
    rig.cx.simulate_keystrokes("tab");
    rig.cx.run_until_parked();
    let is_ring_shown = |rig: &mut Rig| {
        let view = rig.view.clone();
        rig.cx
            .update(|window, cx| view.read(cx).is_keyboard_focused(window))
    };
    assert!(is_ring_shown(&mut rig), "Tab gives keyboard focus");
    let cell = rig.cell_center(0, 0);
    rig.press(cell, 1);
    rig.release(cell);
    assert!(!is_ring_shown(&mut rig), "a click shows no ring");
    let focused = rig.view.clone();
    let is_focused = rig
        .cx
        .update(|window, cx| focused.focus_handle(cx).is_focused(window));
    assert!(is_focused, "the click keeps focus on the terminal");
}

#[gpui::test]
fn view_copy_and_paste_are_reachable_from_the_app(app: &mut TestAppContext) {
    let mut rig = rig(app);
    rig.output("hello");
    let copied = rig.view.update(rig.cx, |view, cx| view.copy(cx));
    assert!(!copied, "nothing selected, nothing copied");
    let (from, to) = (rig.cell_point(0, 0, 0.1), rig.cell_point(0, 4, 0.75));
    rig.press(from, 1);
    rig.drag_to(to);
    rig.release(to);
    assert!(rig.view.update(rig.cx, |view, cx| view.copy(cx)));
    let text = rig.cx.read_from_clipboard().and_then(|item| item.text());
    assert_eq!(text.as_deref(), Some("hello"));
    rig.cx
        .write_to_clipboard(ClipboardItem::new_string("ls".into()));
    rig.view.update(rig.cx, |view, cx| view.paste(cx));
    assert_eq!(rig.source.wait_written(b"ls"), b"ls");
}

#[gpui::test]
fn view_last_line_is_the_last_row_with_text(app: &mut TestAppContext) {
    let mut rig = rig(app);
    rig.output("first\r\nbox: no agent is running for task 'm2'\r\n\r\n");
    let line = rig
        .view
        .read_with(rig.cx, |view, cx| view.terminal().read(cx).last_line());
    assert_eq!(
        line.as_deref(),
        Some("box: no agent is running for task 'm2'")
    );
}

#[gpui::test]
fn view_the_pointer_is_the_arrow_over_an_address_while_cmd_is_held(app: &mut TestAppContext) {
    let mut rig = rig(app);
    rig.output("go to https://example.com now");
    let address = rig.cell_center(0, 12);
    let style = |rig: &mut Rig| rig.view.read_with(rig.cx, |view, _| view.pointer_style());
    rig.cx.simulate_mouse_move(address, None, Modifiers::none());
    assert_eq!(style(&mut rig), gpui::CursorStyle::IBeam, "over text");
    rig.cx
        .simulate_mouse_move(address, None, Modifiers::command());
    assert_eq!(
        style(&mut rig),
        gpui::CursorStyle::Arrow,
        "Cmd held over an address: it can be pressed, and the pointer stays the arrow"
    );
    let words = rig.cell_center(0, 2);
    rig.cx
        .simulate_mouse_move(words, None, Modifiers::command());
    assert_eq!(
        style(&mut rig),
        gpui::CursorStyle::IBeam,
        "Cmd held over plain text"
    );
    rig.cx.simulate_mouse_move(address, None, Modifiers::none());
    assert_eq!(style(&mut rig), gpui::CursorStyle::IBeam, "Cmd let go");
}
