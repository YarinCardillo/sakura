//! Correctness of the emulator model: bytes in, grid and replies out. No window is involved.

use crate::core::{
    selection_spans, GridFrame, SelectionKind, SelectionSpan, TermCore, ViewportPoint,
    SCROLLBACK_LINES,
};
use crate::palette::TerminalPalette;
use crate::rows::{CellStyle, Underline};
use crate::size::GridSize;
use alacritty_terminal::index::Side;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::TermMode;
use alacritty_terminal::vte::ansi::CursorShape;

const BACKGROUND: u32 = 0x000000;

fn term(columns: u16, rows: u16) -> TermCore {
    TermCore::new(
        GridSize::new(columns, rows),
        TerminalPalette::one_dark(BACKGROUND),
    )
}

#[test]
fn a_line_the_terminal_wrapped_is_read_as_one_line() {
    let mut core = term(20, 6);
    feed(
        &mut core,
        "short\r\nvisit: https://example.com/a/long/path?x=1 now\r\nlast",
    );
    assert_eq!(
        core.logical_lines()[..3],
        [
            "short",
            "visit: https://example.com/a/long/path?x=1 now",
            "last"
        ]
    );
}

#[test]
fn the_address_under_a_cell_is_found_across_the_rows_it_wraps_onto() {
    let mut core = term(20, 6);
    feed(&mut core, "visit: https://example.com/a/long/path?x=1 now");
    let on_second_row = crate::core::ViewportPoint { line: 1, column: 3 };
    let link = core.link_at(on_second_row).expect("an address");
    assert_eq!(link.url, "https://example.com/a/long/path?x=1");
    assert_eq!(link.spans, vec![(0, 7..20), (1, 0..20), (2, 0..2)]);
    let on_a_word = crate::core::ViewportPoint { line: 0, column: 2 };
    assert_eq!(core.link_at(on_a_word), None);
    let after_it = crate::core::ViewportPoint { line: 2, column: 4 };
    assert_eq!(core.link_at(after_it), None);
}

#[test]
fn an_address_that_is_not_http_or_https_is_no_link() {
    let mut core = term(40, 4);
    feed(&mut core, "file:///etc/passwd javascript:alert(1)");
    for column in [3, 25] {
        let point = crate::core::ViewportPoint { line: 0, column };
        assert_eq!(core.link_at(point), None);
    }
}

fn feed(core: &mut TermCore, text: &str) -> crate::core::Advance {
    core.advance(text.as_bytes())
}

fn at(line: usize, column: usize) -> ViewportPoint {
    ViewportPoint { line, column }
}

/// The style of the narrow, non-combining character at `column` of visible `line`.
fn style_at(core: &TermCore, line: usize, column: usize) -> CellStyle {
    let row = core.layout_viewport_row(line);
    let segment = row
        .segments
        .iter()
        .find(|segment| (segment.column..segment.column + segment.cells).contains(&column))
        .unwrap_or_else(|| panic!("no text at {line}:{column} in {row:?}"));
    let byte = segment
        .text
        .char_indices()
        .nth(column - segment.column)
        .map(|(index, _)| index)
        .expect("character in segment");
    let mut start = 0;
    for run in &segment.runs {
        if byte < start + run.len {
            return run.style;
        }
        start += run.len;
    }
    panic!("no run covers byte {byte}");
}

fn background_at(core: &TermCore, line: usize, column: usize) -> Option<u32> {
    core.layout_viewport_row(line)
        .backgrounds
        .iter()
        .find(|span| (span.column..span.column + span.width).contains(&column))
        .map(|span| span.color)
}

#[test]
fn colours_16_and_bright_come_from_the_palette() {
    let mut core = term(20, 3);
    feed(&mut core, "\x1b[31mR\x1b[92mG\x1b[44mB\x1b[0mn");
    assert_eq!(style_at(&core, 0, 0).fg, 0xe06c75);
    assert_eq!(style_at(&core, 0, 1).fg, 0xaad581);
    assert_eq!(background_at(&core, 0, 2), Some(0x61afef));
    assert_eq!(style_at(&core, 0, 3).fg, 0xabb2bf);
    assert_eq!(background_at(&core, 0, 3), None);
}

#[test]
fn colours_256_and_true_colour_foreground_and_background() {
    let mut core = term(20, 3);
    feed(
        &mut core,
        "\x1b[38;5;196ma\x1b[48;5;21mb\x1b[0m\x1b[38;2;1;2;3;48;2;250;128;7mc",
    );
    assert_eq!(style_at(&core, 0, 0).fg, 0xff0000);
    assert_eq!(background_at(&core, 0, 1), Some(0x0000ff));
    assert_eq!(style_at(&core, 0, 2).fg, 0x010203);
    assert_eq!(background_at(&core, 0, 2), Some(0xfa8007));
}

#[test]
fn attributes_bold_dim_italic_underline_strike_and_curl() {
    let mut core = term(20, 3);
    feed(
        &mut core,
        "\x1b[1mb\x1b[0;2md\x1b[0;3mi\x1b[0;4mu\x1b[0;9ms\x1b[0;4:3mc\x1b[0m",
    );
    assert!(style_at(&core, 0, 0).bold);
    assert!(style_at(&core, 0, 1).faint);
    assert!(style_at(&core, 0, 2).italic);
    assert_eq!(
        style_at(&core, 0, 3).underline,
        Some((Underline::Straight, 0xabb2bf))
    );
    assert!(style_at(&core, 0, 4).strikeout);
    assert_eq!(
        style_at(&core, 0, 5).underline.map(|(kind, _)| kind),
        Some(Underline::Curly)
    );
    let plain = style_at(&core, 0, 0);
    assert!(!plain.italic && !plain.faint && plain.underline.is_none());
}

#[test]
fn inverse_swaps_foreground_and_background() {
    let mut core = term(20, 3);
    feed(&mut core, "\x1b[7mx\x1b[0m\x1b[31;7my");
    assert_eq!(style_at(&core, 0, 0).fg, BACKGROUND);
    assert_eq!(background_at(&core, 0, 0), Some(0xabb2bf));
    assert_eq!(style_at(&core, 0, 1).fg, BACKGROUND);
    assert_eq!(background_at(&core, 0, 1), Some(0xe06c75));
}

#[test]
fn adjacent_cells_with_one_style_are_batched_into_one_run() {
    let mut core = term(30, 3);
    feed(&mut core, "\x1b[32mgreen\x1b[0m plain text");
    let row = core.layout_viewport_row(0);
    let first = &row.segments[0];
    assert_eq!(
        (first.column, first.text.as_str(), first.runs.len()),
        (0, "green", 1)
    );
    assert_eq!(row.text(), "green plain text");
}

#[test]
fn wide_characters_take_two_cells_and_stay_on_the_grid() {
    let mut core = term(20, 3);
    feed(&mut core, "a中b😀c");
    assert!(core.cell(0, 1).flags.contains(Flags::WIDE_CHAR));
    assert!(core.cell(0, 2).flags.contains(Flags::WIDE_CHAR_SPACER));
    assert_eq!(core.cell(0, 3).c, 'b');
    assert!(core.cell(0, 4).flags.contains(Flags::WIDE_CHAR));
    assert_eq!(core.cell(0, 6).c, 'c');
    let row = core.layout_viewport_row(0);
    let placed: Vec<(usize, &str, usize)> = row
        .segments
        .iter()
        .map(|segment| (segment.column, segment.text.as_str(), segment.cells))
        .collect();
    assert_eq!(
        placed,
        vec![
            (0, "a", 1),
            (1, "中", 2),
            (3, "b", 1),
            (4, "😀", 2),
            (6, "c", 1)
        ]
    );
}

#[test]
fn combining_characters_stay_in_their_base_cell() {
    let mut core = term(20, 3);
    feed(&mut core, "e\u{301}x");
    assert_eq!(core.cell(0, 0).zerowidth(), Some(&['\u{301}'][..]));
    assert_eq!(core.cell(0, 1).c, 'x');
    assert_eq!(core.line_text(0), "e\u{301}x");
}

#[test]
fn cursor_shapes_and_visibility_follow_the_program() {
    let mut core = term(20, 3);
    feed(&mut core, "ab");
    let cursor = core.cursor().expect("visible cursor");
    assert_eq!((cursor.point, cursor.shape), (at(0, 2), CursorShape::Block));
    feed(&mut core, "\x1b[6 q");
    assert_eq!(
        core.cursor().map(|cursor| cursor.shape),
        Some(CursorShape::Beam)
    );
    feed(&mut core, "\x1b[4 q");
    assert_eq!(
        core.cursor().map(|cursor| cursor.shape),
        Some(CursorShape::Underline)
    );
    feed(&mut core, "\x1b[?25l");
    assert_eq!(core.cursor(), None);
    feed(&mut core, "\x1b[?25h\x1b[2 q");
    assert_eq!(
        core.cursor().map(|cursor| cursor.shape),
        Some(CursorShape::Block)
    );
}

#[test]
fn cursor_on_a_wide_character_reports_it() {
    let mut core = term(20, 3);
    feed(&mut core, "中\x1b[1D");
    let cursor = core.cursor().expect("cursor");
    assert_eq!((cursor.character, cursor.is_wide), ('中', true));
}

#[test]
fn alternate_screen_keeps_the_main_screen() {
    let mut core = term(20, 4);
    feed(&mut core, "main");
    feed(&mut core, "\x1b[?1049h");
    assert!(core.mode().contains(TermMode::ALT_SCREEN));
    assert_eq!(core.line_text(0), "");
    feed(&mut core, "full screen");
    feed(&mut core, "\x1b[?1049l");
    assert!(!core.mode().contains(TermMode::ALT_SCREEN));
    assert_eq!(core.line_text(0), "main");
}

#[test]
fn scroll_region_scrolls_only_inside_its_margins() {
    let mut core = term(10, 5);
    feed(&mut core, "top\r\n1\r\n2\r\n3\r\nbottom");
    // Region lines 2..4 (1-based), cursor to its last line, then a line feed scrolls only the region.
    feed(&mut core, "\x1b[2;4r\x1b[4;1H\n");
    let lines: Vec<String> = (0..5).map(|line| core.line_text(line)).collect();
    assert_eq!(lines, vec!["top", "2", "3", "", "bottom"]);
}

#[test]
fn scrollback_keeps_ten_thousand_lines_and_scrolls_through_them() {
    let mut core = term(20, 10);
    let text: String = (0..SCROLLBACK_LINES + 100)
        .map(|i| format!("line {i}\r\n"))
        .collect();
    feed(&mut core, &text);
    assert_eq!(core.history_size(), SCROLLBACK_LINES);
    core.scroll(3);
    assert_eq!(core.display_offset(), 3);
    assert_eq!(
        core.line_text(0),
        format!("line {}", SCROLLBACK_LINES + 100 - 9 - 3)
    );
    core.scroll(i32::MAX / 2);
    assert_eq!(core.display_offset(), SCROLLBACK_LINES);
    assert_eq!(core.line_text(0), "line 91");
    core.scroll_to_bottom();
    assert_eq!(core.display_offset(), 0);
}

#[test]
fn selection_by_drag_word_and_line() {
    let mut core = term(30, 3);
    feed(&mut core, "hello brave world\r\nsecond line");
    core.start_selection(SelectionKind::Simple, at(0, 0), Side::Left);
    core.extend_selection(at(0, 4), Side::Right);
    assert_eq!(core.selection_text().as_deref(), Some("hello"));
    core.start_selection(SelectionKind::Word, at(0, 8), Side::Left);
    assert_eq!(core.selection_text().as_deref(), Some("brave"));
    core.start_selection(SelectionKind::Line, at(1, 3), Side::Left);
    assert_eq!(core.selection_text().as_deref(), Some("second line\n"));
    core.clear_selection();
    assert!(!core.has_selection());
    assert_eq!(core.selection_text(), None);
}

#[test]
fn selection_across_lines_follows_scrollback() {
    let mut core = term(20, 3);
    let text: String = (0..10).map(|i| format!("row{i}\r\n")).collect();
    feed(&mut core, &text);
    core.scroll(2);
    core.start_selection(SelectionKind::Simple, at(0, 0), Side::Left);
    core.extend_selection(at(1, 3), Side::Right);
    assert_eq!(core.selection_text().as_deref(), Some("row6\nrow7"));
}

#[test]
fn modes_for_paste_mouse_focus_and_cursor_keys() {
    let mut core = term(20, 3);
    feed(
        &mut core,
        "\x1b[?2004h\x1b[?1000h\x1b[?1006h\x1b[?1004h\x1b[?1h",
    );
    let mode = core.mode();
    for flag in [
        TermMode::BRACKETED_PASTE,
        TermMode::MOUSE_REPORT_CLICK,
        TermMode::SGR_MOUSE,
        TermMode::FOCUS_IN_OUT,
        TermMode::APP_CURSOR,
    ] {
        assert!(mode.contains(flag), "{flag:?} missing from {mode:?}");
    }
    feed(&mut core, "\x1b[?1002h");
    assert!(core.mode().contains(TermMode::MOUSE_DRAG));
    feed(&mut core, "\x1b[?1003h");
    assert!(core.mode().contains(TermMode::MOUSE_MOTION));
}

#[test]
fn resize_reflows_wrapped_lines() {
    let mut core = term(20, 4);
    feed(&mut core, "abcdefg");
    core.resize(GridSize::new(5, 4));
    // The cursor row stays put, so the first half of the wrapped line moves into the scrollback.
    assert_eq!(core.line_text(0), "fg");
    core.scroll(1);
    assert_eq!(
        (core.line_text(0), core.line_text(1)),
        ("abcde".into(), "fg".into())
    );
    core.scroll_to_bottom();
    core.resize(GridSize::new(20, 4));
    // Widening joins the halves into one line again (alacritty keeps it just above the cursor row).
    core.scroll(1);
    assert_eq!(core.line_text(0), "abcdefg");
}

#[test]
fn osc_title_is_reported_and_reset() {
    let mut core = term(20, 3);
    assert_eq!(
        feed(&mut core, "\x1b]0;build: ok\x07").title,
        Some(Some("build: ok".into()))
    );
    assert_eq!(
        feed(&mut core, "\x1b]2;two\x1b\\").title,
        Some(Some("two".into()))
    );
    assert_eq!(feed(&mut core, "plain").title, None);
}

#[test]
fn osc_52_clipboard_writes_are_refused() {
    let mut core = term(20, 3);
    let advance = feed(&mut core, "\x1b]52;c;aGVsbG8=\x07\x1b]52;c;?\x07");
    assert_eq!(advance, crate::core::Advance::default());
}

#[test]
fn replies_to_status_and_colour_queries_go_back_to_the_program() {
    let mut core = term(20, 3);
    assert_eq!(feed(&mut core, "ab\x1b[6n").reply, b"\x1b[1;3R");
    let background = feed(&mut core, "\x1b]11;?\x07").reply;
    assert_eq!(background, b"\x1b]11;rgb:0000/0000/0000\x07");
    let red = feed(&mut core, "\x1b]4;1;?\x07").reply;
    assert_eq!(red, b"\x1b]4;1;rgb:e0e0/6c6c/7575\x07");
    assert_eq!(feed(&mut core, "\x1b[c").reply, b"\x1b[?6c");
}

#[test]
fn synchronized_updates_appear_only_when_they_end() {
    let mut core = term(20, 3);
    feed(&mut core, "\x1b[?2026hhidden");
    assert_eq!(core.line_text(0), "");
    assert!(core.sync_deadline().is_some());
    feed(&mut core, "\x1b[?2026l");
    assert_eq!(core.line_text(0), "hidden");
    assert!(core.sync_deadline().is_none());
}

#[test]
fn damage_rebuilds_only_changed_rows_and_matches_a_full_layout() {
    let mut core = term(30, 8);
    let mut frame = GridFrame::default();
    feed(&mut core, "one\r\ntwo\r\nthree");
    assert_eq!(core.refresh(&mut frame), 8);
    feed(&mut core, "\x1b[5;1Hfive");
    let rebuilt = core.refresh(&mut frame);
    assert!(rebuilt <= 3, "rebuilt {rebuilt} rows for a one line change");
    assert_eq!(
        core.refresh(&mut frame),
        1,
        "only the cursor row stays damaged when nothing changed"
    );
    let steps = [
        "\x1b[2J\x1b[H",
        "a\r\nb\x1b[31mc",
        "\x1b[3;3H\x1b[7mX",
        "\r\n\r\n\r\n\r\n\r\n\r\n\r\n\r\nz",
        "\x1b[?1049hA",
        "\x1b[?1049l",
    ];
    for step in steps {
        feed(&mut core, step);
        core.refresh(&mut frame);
        for line in 0..8 {
            assert_eq!(
                frame.rows[line],
                core.layout_viewport_row(line),
                "row {line} after {step:?}"
            );
        }
    }
    core.scroll(1);
    core.refresh(&mut frame);
    for line in 0..8 {
        assert_eq!(
            frame.rows[line],
            core.layout_viewport_row(line),
            "row {line} after scrolling"
        );
    }
}

#[test]
fn selection_spans_cover_whole_middle_rows_and_clip_to_the_viewport() {
    let mut core = term(10, 4);
    feed(&mut core, "aaaa\r\nbbbb\r\ncccc\r\ndddd");
    core.start_selection(SelectionKind::Simple, at(0, 2), Side::Left);
    core.extend_selection(at(2, 1), Side::Right);
    let range = core.selection_range().expect("selection");
    let span = |line, start, end| SelectionSpan { line, start, end };
    assert_eq!(
        selection_spans(&range, 0, 4, 10),
        vec![span(0, 2, 10), span(1, 0, 10), span(2, 0, 2)]
    );
    assert_eq!(
        selection_spans(&range, 0, 2, 10),
        vec![span(0, 2, 10), span(1, 0, 10)]
    );
}
