//! The diffs of one lavoro, "Changes" in the right pane: every changed file in one virtualised scroll.
//! Files start closed and open in place. Open files and the scroll position are kept by path, so a refresh
//! of the patch moves nothing the person set. A line too long for the list wraps to its width (see [`super::wrap`]): the list
//! is built once its width is known, row heights follow the wrap, and nothing scrolls sideways.

use super::code_line::LineStyle;
use super::fit::fitted;
use super::line::{self, CodeLayout};
use super::rows::{Laid, Row};
use super::selection::DiffSelection;
use super::shape_cache::{LineKey, ShapeCache};
use super::wrap::{self, Wrap};
use super::{header, rows, state};
use crate::data::changes::Changes;
use crate::data::facts::Facts;
use crate::l10n::Said;
use crate::theme::{self, Palette, Theme};
use crate::ui::chrome::{mono_features, tabular};
use gpui::{
    canvas, div, point, prelude::FluentBuilder as _, px, size, AnyElement, App, Bounds, Context,
    Entity, FocusHandle, Font, InteractiveElement as _, IntoElement, MouseButton, ParentElement,
    Pixels, Render, Size, StatefulInteractiveElement as _, Styled, Subscription, Window,
};
use gpui_base::{Scrollbar, VirtualListScrollHandle};
use gpui_component::input::Copy;
use sakura_core::patch::Patch;
use std::cell::Cell;
use std::collections::HashSet;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

/// What a test reads of a list's layout: the columns a row of code holds, the list's width at its last
/// paint, where a full row of code would end, where the widest row of code really ends once shaped, and
/// the height of every row.
#[cfg(test)]
pub struct LayoutNow {
    pub columns: usize,
    pub width: Pixels,
    pub code_end: Pixels,
    pub shaped_end: Pixels,
    pub heights: Vec<f32>,
}

/// The review list of one task.
pub struct ChangesList {
    task: String,
    changes: Entity<Changes>,
    facts: Entity<Facts>,
    pub(super) patch: Option<Arc<Patch>>,
    pub(super) rows: Rc<Vec<Row>>,
    sizes: Rc<Vec<Size<Pixels>>>,
    /// The paths of the files that are open.
    toggled: HashSet<String>,
    /// Where each row starts, from the top of the list, and how tall it is with the wrap of now.
    pub(super) origins: Rc<Vec<f32>>,
    heights: Vec<f32>,
    /// The width of each row's line in columns (0 for a row that is not a code line), read when the rows
    /// are built, and the wrap the heights were laid out for.
    line_columns: Vec<usize>,
    pub(super) wrap: Wrap,
    /// The digits of the largest line number, read once per patch.
    digits: usize,
    pub(super) scroll: VirtualListScrollHandle,
    pub(super) selection: DiffSelection,
    shapes: ShapeCache,
    /// Taken on a pointer press (never on its own), so Cmd+C reaches the list's copy for a selection.
    pub(super) focus: FocusHandle,
    /// The list's bounds in the window at the last paint.
    pub(super) body: Rc<Cell<Bounds<Pixels>>>,
    #[cfg(feature = "measure")]
    probe: Option<crate::snapshot::ScrollProbe>,
    _observer: Subscription,
}

impl ChangesList {
    /// The list of `task`'s changes, filled from `changes` as reads answer.
    pub fn new(
        task: String,
        changes: Entity<Changes>,
        facts: Entity<Facts>,
        cx: &mut Context<Self>,
    ) -> Self {
        let observer = cx.observe(&changes, |_, _, cx| cx.notify());
        Self {
            task,
            changes,
            facts,
            patch: None,
            rows: Rc::default(),
            sizes: Rc::default(),
            toggled: crate::snapshot::open_files().into_iter().collect(),
            origins: Rc::default(),
            heights: Vec::new(),
            line_columns: Vec::new(),
            wrap: Wrap::unknown(),
            digits: 1,
            scroll: VirtualListScrollHandle::new(),
            selection: DiffSelection::default(),
            shapes: ShapeCache::default(),
            focus: cx.focus_handle(),
            body: Rc::new(Cell::new(Bounds::default())),
            #[cfg(feature = "measure")]
            probe: crate::snapshot::ScrollProbe::from_environment(),
            _observer: observer,
        }
    }

    /// How many rows the list has.
    #[cfg(test)]
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// The list as it is laid out now.
    #[cfg(test)]
    pub fn layout_now(&mut self, window: &mut Window, cx: &App) -> LayoutNow {
        let layout = self.layout(window, cx);
        let style = layout.line.clone();
        let widest = self.widest_shaped_row(&layout, window);
        LayoutNow {
            columns: self.wrap.first,
            width: self.body.get().size.width,
            code_end: style.code_start() + style.advance * self.wrap.first as f32,
            shaped_end: style.code_start() + widest,
            heights: self.heights.clone(),
        }
    }

    /// How far the widest row of code reaches from where code starts, as the text system shaped it: a
    /// continuation row counts its hanging indent.
    #[cfg(test)]
    fn widest_shaped_row(&mut self, layout: &CodeLayout, window: &Window) -> Pixels {
        let rows = self.rows.clone();
        let Some(patch) = self.patch.clone() else {
            return px(0.);
        };
        let mut widest = px(0.);
        for row in rows.iter() {
            let Row::Line { file, hunk, line } = *row else {
                continue;
            };
            let text = &patch.files[file].hunks[hunk].lines[line];
            let (shaped, _, _) = self.shaped((file, hunk, line), text, layout, window);
            for (index, row) in shaped.iter().enumerate() {
                let indent = if index == 0 { px(0.) } else { layout.line.hang };
                widest = widest.max(indent + row.shaped.width);
            }
        }
        widest
    }

    /// Opens or closes `file` in place.
    pub fn toggle(&mut self, file: usize, cx: &mut Context<Self>) {
        let Some(path) = self.path_of(file) else {
            return;
        };
        if !self.toggled.remove(&path) {
            self.toggled.insert(path);
        }
        self.rebuild_rows();
        cx.notify();
    }

    fn path_of(&self, file: usize) -> Option<String> {
        Some(self.patch.as_ref()?.files.get(file)?.path.clone())
    }

    fn is_open(&self, path: &str) -> bool {
        self.toggled.contains(path)
    }
}

impl ChangesList {
    /// Takes the patch the data layer holds, keeping the scroll position by path when it changed.
    fn sync(&mut self, cx: &App) {
        let fresh = self
            .changes
            .read(cx)
            .task(&self.task)
            .and_then(|task| task.patch().cloned());
        let is_same = match (&fresh, &self.patch) {
            (Some(fresh), Some(shown)) => Arc::ptr_eq(fresh, shown),
            (None, None) => true,
            _ => false,
        };
        if is_same {
            return;
        }
        let anchor = self.anchor();
        self.patch = fresh;
        self.shapes.clear();
        self.digits = self.patch.as_deref().map_or(1, widest_number_digits);
        self.rebuild_rows();
        self.scroll_to_anchor(anchor);
    }

    /// Where the list is scrolled, by meaning.
    fn anchor(&self) -> Option<rows::Anchor> {
        let patch = self.patch.as_ref()?;
        rows::anchor_at(self.laid(), patch, f32::from(-self.scroll.offset().y))
    }

    /// Puts `anchor` back at the top after the rows or their heights changed; the top when it is gone.
    fn scroll_to_anchor(&mut self, anchor: Option<rows::Anchor>) {
        let top = anchor.and_then(|anchor| {
            let patch = self.patch.as_ref()?;
            rows::offset_of(&anchor, self.laid(), patch)
        });
        self.scroll
            .set_offset(point(px(0.), -px(top.unwrap_or(0.))));
    }

    fn laid(&self) -> Laid<'_> {
        Laid {
            rows: &self.rows,
            heights: &self.heights,
        }
    }

    fn rebuild_rows(&mut self) {
        let rows = match &self.patch {
            Some(patch) => rows::rows(patch, |path| self.is_open(path)),
            None => Vec::new(),
        };
        self.line_columns = rows.iter().map(|row| self.columns_of(*row)).collect();
        self.rows = Rc::new(rows);
        self.lay_out();
        // Row indices change with the rows: a selection made on the old ones means nothing now.
        self.selection.clear();
    }

    /// The width in columns of `row`'s line; 0 for a row that is not a code line.
    fn columns_of(&self, row: Row) -> usize {
        self.line_of(row)
            .map_or(0, |line| wrap::columns(&line::display_text(&line.text)))
    }

    /// The patch line of `row`, when it is a code line.
    pub(super) fn line_of(&self, row: Row) -> Option<&sakura_core::patch::Line> {
        let Row::Line { file, hunk, line } = row else {
            return None;
        };
        let patch = self.patch.as_ref()?;
        patch.files.get(file)?.hunks.get(hunk)?.lines.get(line)
    }

    /// The height of every row for the wrap of now, and from them where each row starts.
    fn lay_out(&mut self) {
        let heights: Vec<f32> = self
            .rows
            .iter()
            .zip(&self.line_columns)
            .map(|(row, columns)| self.height_of(*row, *columns))
            .collect();
        self.sizes = Rc::new(heights.iter().map(|h| size(px(0.), px(*h))).collect());
        let origins = heights.iter().scan(0.0, |top, height| {
            let origin = *top;
            *top += height;
            Some(origin)
        });
        self.origins = Rc::new(origins.collect());
        self.heights = heights;
    }

    /// A code line is as tall as the rows it wraps onto; every other row has its own height.
    fn height_of(&self, row: Row, columns: usize) -> f32 {
        let Some(line) = self.line_of(row) else {
            return row.height();
        };
        let text = || line::display_text(&line.text);
        row.height() * wrap::row_count(columns, text, self.wrap) as f32
    }

    /// Wraps the lines to a list `width` wide: when the wrap changes, every height is laid out again and
    /// the line at the top stays at the top.
    fn fit(&mut self, width: Pixels, window: &mut Window, cx: &App) {
        let layout = self.layout(window, cx);
        let code_width = width - layout.code_start() - theme::DIFF_CODE_PADDING;
        let wrap = Wrap::for_width(code_width, layout.line.advance);
        if wrap == self.wrap {
            return;
        }
        let anchor = self.anchor();
        self.wrap = wrap;
        self.shapes.clear_code();
        self.lay_out();
        self.scroll_to_anchor(anchor);
    }

    /// The row the list measures every frame for its width: a code line, the cheapest row, when there is one.
    fn measure_index(&self) -> usize {
        self.rows
            .iter()
            .position(|row| matches!(row, Row::Line { .. }))
            .unwrap_or(0)
    }

    /// A code line's shaped code and numbers, from the cache.
    fn shaped(
        &mut self,
        key: LineKey,
        line: &sakura_core::patch::Line,
        layout: &CodeLayout,
        window: &Window,
    ) -> line::ShapedNumbers {
        let style = &layout.line;
        let code = self.shapes.code(key, line, (self.wrap, style.text), window);
        let mut number =
            |value: Option<u32>| value.map(|value| self.shapes.number(value, style.muted, window));
        (code, number(line.old), number(line.new))
    }

    pub(super) fn layout(&self, window: &mut Window, cx: &App) -> CodeLayout {
        let palette = Theme::palette(cx);
        let digit = figure_width(window);
        let advance = advance(window);
        let line = LineStyle {
            text: palette.text,
            muted: palette.text_tertiary,
            selection: palette.selection,
            number_width: digit * self.digits as f32 + theme::DIFF_GUTTER_PADDING,
            advance,
            hang: advance * self.wrap.hang as f32,
        };
        CodeLayout {
            palette,
            line: Rc::new(line),
        }
    }
}

impl ChangesList {
    fn render_rows(
        &mut self,
        range: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let layout = self.layout(window, cx);
        self.shapes.follow(layout.palette.appearance);
        let Some(patch) = self.patch.clone() else {
            return Vec::new();
        };
        let rows = self.rows.clone();
        range
            .filter_map(|index| Some((index, *rows.get(index)?)))
            .map(|(index, row)| self.render_row(index, row, &patch, &layout, window, cx))
            .collect()
    }

    fn render_row(
        &mut self,
        index: usize,
        row: Row,
        patch: &Patch,
        layout: &CodeLayout,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let file = &patch.files[row.file()];
        match row {
            Row::Header { file: at } => self.header_row(at, layout, patch, cx),
            Row::Hunk { hunk, .. } => line::hunk_header(&file.hunks[hunk].header, layout),
            Row::Line {
                file: at,
                hunk,
                line,
            } => {
                let key = (at, hunk, line);
                let line = &file.hunks[hunk].lines[line];
                let shaped = self.shaped(key, line, layout, window);
                let length = shaped.0.last().map_or(0, |row| row.bytes.end);
                let selected = self.selection.range_in(index, length);
                line::code_line(line, shaped, selected, layout)
            }
            Row::Note { .. } => line::note(rows::note(file).unwrap_or_default(), layout),
        }
    }

    fn header_row(
        &self,
        file: usize,
        layout: &CodeLayout,
        patch: &Patch,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = layout.palette;
        let path = &patch.files[file].path;
        let is_open = self.is_open(path);
        let hover = palette.hover;
        div()
            .id(("header", file))
            .debug_selector(move || format!("changes-header-{file}"))
            .w_full()
            .h(px(theme::CHANGE_HEADER_HEIGHT))
            .pl(theme::SIDE_PANE_INSET)
            .pr(theme::CONTENT_PADDING)
            .when(file > 0, |row| {
                row.border_t_1().border_color(palette.border)
            })
            .hover(move |style| style.bg(hover))
            .on_click(cx.listener(move |list, _, _, cx| list.toggle(file, cx)))
            .child(header::content(&patch.files[file], Some(is_open), &palette))
            .into_any_element()
    }
}

impl Render for ChangesList {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync(cx);
        #[cfg(feature = "measure")]
        if let Some(probe) = self.probe.as_mut() {
            let height = self.sizes.iter().map(|size| f32::from(size.height)).sum();
            probe.tick(&self.scroll, height, _window);
        }
        let palette = Theme::palette(cx);
        match state::of(&self.task, self.facts.read(cx), self.changes.read(cx)) {
            state::Pane::Files {
                failure,
                is_partial,
            } => self.render_list(failure, is_partial, &palette, cx),
            sentence => state::sentence_view(sentence, &palette),
        }
    }
}

impl ChangesList {
    fn render_list(
        &mut self,
        failure: Option<Said>,
        is_partial: bool,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let body = self.body.clone();
        let measure = canvas(move |bounds, _, _| body.set(bounds), |_, _, _, _| {})
            .absolute()
            .size_full();
        let list = cx.entity();
        let list = fitted(move |width, window, cx| {
            list.update(cx, |list, cx| list.fitted_list(width, window, cx))
        });
        div()
            .size_full()
            .flex()
            .flex_col()
            .when(is_partial, |pane| pane.child(state::partial_line(palette)))
            .children(failure.map(|failure| state::failure_line(&failure, palette)))
            .child(
                div()
                    .id("changes-body")
                    .debug_selector(|| "changes-body".into())
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .track_focus(&self.focus)
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::press))
                    .on_mouse_move(cx.listener(Self::drag))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|list, _, _, _| list.release()),
                    )
                    .on_action(cx.listener(|list, _: &Copy, _, cx| list.copy(cx)))
                    .child(measure)
                    .child(list)
                    .child(Scrollbar::vertical(&self.scroll)),
            )
            .into_any_element()
    }
}

impl ChangesList {
    /// The virtual list for a body `width` wide, its lines wrapped to it.
    fn fitted_list(
        &mut self,
        width: Pixels,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.fit(width, window, cx);
        gpui_base::v_virtual_list(
            cx.entity(),
            "changes",
            self.sizes.clone(),
            |list, range, window, cx| list.render_rows(range, window, cx),
        )
        .track_scroll(&self.scroll)
        .with_item_to_measure_index(self.measure_index())
        .into_any_element()
    }
}

/// Digits of the largest line number in the patch, at least two.
fn widest_number_digits(patch: &Patch) -> usize {
    let largest = patch
        .files
        .iter()
        .flat_map(|file| &file.hunks)
        .flat_map(|hunk| &hunk.lines)
        .flat_map(|line| [line.old, line.new])
        .flatten()
        .max()
        .unwrap_or(0);
    largest.to_string().len().max(2)
}

/// The advance of one column of code in Geist Mono at the diff size.
fn advance(window: &Window) -> Pixels {
    glyph_width(window, theme::FONT_MONO, mono_features(), theme::DIFF_TEXT)
}

/// The width of one tabular figure in the interface font at the label size.
fn figure_width(window: &Window) -> Pixels {
    glyph_width(window, theme::FONT_UI, tabular(), theme::TEXT_LABEL)
}

fn glyph_width(
    window: &Window,
    family: &'static str,
    features: gpui::FontFeatures,
    font_size: Pixels,
) -> Pixels {
    let font = Font {
        family: family.into(),
        features,
        ..Font::default()
    };
    let text = window.text_system();
    let id = text.resolve_font(&font);
    text.advance(id, font_size, '0')
        .map_or(font_size * theme::FALLBACK_ADVANCE, |advance| advance.width)
}
