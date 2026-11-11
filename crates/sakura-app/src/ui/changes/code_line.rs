//! One code line of a diff as a single element: its tint and bar, the old and new numbers, the code and its
//! selection are painted directly, with no child elements to lay out. A line too long for the pane is as
//! many rows as it wraps onto, the continuation rows indented under the code column. Its text comes shaped
//! from the list's cache, so a scroll shapes only lines it has never shown.

use crate::l10n::{tr, Key};
use crate::theme;
use crate::ui::chrome::mono_features;
use gpui::{
    fill, point, px, size, App, Bounds, ContentMask, CursorStyle, Element, ElementId, Font,
    GlobalElementId, Hitbox, HitboxBehavior, Hsla, InspectorElementId, IntoElement, LayoutId,
    Pixels, ShapedLine, SharedString, Style, TextAlign, TextRun, Window,
};
use std::ops::Range;
use std::rc::Rc;

/// The colours and metrics every code line of a frame shares.
#[derive(Clone)]
pub struct LineStyle {
    pub text: Hsla,
    pub muted: Hsla,
    pub selection: Hsla,
    /// Width of one line-number column.
    pub number_width: Pixels,
    /// The advance of one character of code, for a selection that goes on past a line's end.
    pub advance: Pixels,
    /// How far continuation rows are indented under the code column.
    pub hang: Pixels,
}

impl LineStyle {
    /// Where code starts, from the row's left edge: the two number columns and the padding.
    pub fn code_start(&self) -> Pixels {
        self.number_width * 2. + theme::DIFF_CODE_PADDING
    }

    /// How far row `row` of a line is indented under the code column.
    pub fn indent(&self, row: usize) -> Pixels {
        if row == 0 {
            px(0.)
        } else {
            self.hang
        }
    }
}

/// One row of a wrapped line: its shaped text and the bytes of the line's shown text it holds.
#[derive(Clone)]
pub struct CodeRow {
    pub shaped: ShapedLine,
    pub bytes: Range<usize>,
}

/// One code line, ready to paint.
pub struct CodeLine {
    /// The line's shown text (tabs expanded, invisible characters marked), shaped row by row: never empty.
    pub rows: Rc<[CodeRow]>,
    /// The old and new line numbers, shaped.
    pub old: Option<ShapedLine>,
    pub new: Option<ShapedLine>,
    /// The added or removed tone; `None` for context.
    pub tone: Option<Hsla>,
    /// The selected bytes of the line's shown text, and whether the selection goes on past the line's end.
    pub selected: Option<(Range<usize>, bool)>,
    pub has_no_newline: bool,
    pub style: Rc<LineStyle>,
}

/// The font of code in diffs.
pub fn code_font() -> Font {
    Font {
        family: theme::FONT_MONO.into(),
        features: mono_features(),
        ..Font::default()
    }
}

/// `text` in one run of `font` and `colour`.
pub fn run(text: &str, font: Font, colour: Hsla) -> TextRun {
    TextRun {
        len: text.len(),
        font,
        color: colour,
        background_color: None,
        underline: None,
        strikethrough: None,
    }
}

impl IntoElement for CodeLine {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for CodeLine {
    type RequestLayoutState = ();
    type PrepaintState = Hitbox;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = gpui::relative(1.).into();
        style.size.height = px(theme::DIFF_LINE_HEIGHT * self.rows.len() as f32).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        _: &mut App,
    ) -> Hitbox {
        window.insert_hitbox(bounds, HitboxBehavior::Normal)
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        hitbox: &mut Hitbox,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(tone) = self.tone {
            window.paint_quad(fill(bounds, tone.opacity(theme::DIFF_TINT_ALPHA)));
            let bar = Bounds::new(
                bounds.origin,
                size(theme::DIFF_BAR_WIDTH, bounds.size.height),
            );
            window.paint_quad(fill(bar, tone));
        }
        let left = bounds.left();
        let number_width = self.style.number_width;
        for (number, column) in [(&self.old, left), (&self.new, left + number_width)] {
            if let Some(number) = number {
                let x = column + number_width - number.width;
                paint(number, point(x, bounds.top()), window, cx);
            }
        }
        let code = Bounds::from_corners(
            point(left + self.style.code_start(), bounds.top()),
            bounds.bottom_right(),
        );
        window.set_cursor_style(CursorStyle::IBeam, hitbox);
        window.with_content_mask(Some(ContentMask { bounds: code }), |window| {
            self.paint_rows(code, window, cx)
        });
    }
}

impl CodeLine {
    /// Every row of the line from `code`'s left edge (continuation rows indented), its selection under
    /// it, and git's note after the last.
    fn paint_rows(&self, code: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        let line_height = px(theme::DIFF_LINE_HEIGHT);
        let last = self.rows.len().saturating_sub(1);
        for (index, row) in self.rows.iter().enumerate() {
            let left = code.left() + self.style.indent(index);
            let origin = point(left, code.top() + line_height * index as f32);
            self.paint_selection(row, origin, index == last, window);
            paint(&row.shaped, origin, window, cx);
            if index == last && self.has_no_newline {
                self.paint_no_newline(origin.x + row.shaped.width, origin.y, window, cx);
            }
        }
    }

    /// The part of the selection on `row`, whose text starts at `origin`. On the line's last row a
    /// selection that goes on to the next line covers one more column.
    fn paint_selection(
        &self,
        row: &CodeRow,
        origin: gpui::Point<Pixels>,
        is_last: bool,
        window: &mut Window,
    ) {
        let Some((selected, goes_on)) = &self.selected else {
            return;
        };
        let from = selected.start.max(row.bytes.start);
        let to = selected.end.min(row.bytes.end);
        let runs_past = is_last && *goes_on && selected.end >= row.bytes.end;
        if from > to || (from == to && !runs_past) {
            return;
        }
        let x = |index: usize| row.shaped.x_for_index(index - row.bytes.start);
        let end = if runs_past {
            x(to) + self.style.advance
        } else {
            x(to)
        };
        let quad = Bounds::from_corners(
            point(origin.x + x(from), origin.y),
            point(origin.x + end, origin.y + px(theme::DIFF_LINE_HEIGHT)),
        );
        window.paint_quad(fill(quad, self.style.selection));
    }

    /// Git's "no newline at end of file", said quietly after the line it is about.
    fn paint_no_newline(&self, after: Pixels, top: Pixels, window: &mut Window, cx: &mut App) {
        let text = SharedString::from(tr(Key("diff.no_newline")));
        let font = Font {
            family: theme::FONT_UI.into(),
            ..Font::default()
        };
        let runs = [run(&text, font, self.style.muted)];
        let line = window
            .text_system()
            .shape_line(text, theme::TEXT_LABEL, &runs, None);
        paint(
            &line,
            point(after + theme::DIFF_CODE_PADDING, top),
            window,
            cx,
        );
    }
}

fn paint(line: &ShapedLine, origin: gpui::Point<Pixels>, window: &mut Window, cx: &mut App) {
    let height = px(theme::DIFF_LINE_HEIGHT);
    if let Err(error) = line.paint(origin, height, TextAlign::Left, None, window, cx) {
        eprintln!("sakura: cannot paint a diff line: {error}");
    }
}
