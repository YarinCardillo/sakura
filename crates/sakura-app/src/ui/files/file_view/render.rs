//! How the open file is drawn: one row with its name and folder, then the kit's editor over the rest,
//! the rows of the lines the task changed tinted, and "Ask the agent" over its corner while lines are
//! selected. The whole view is watched for the
//! person taking it: a scroll or a press anywhere on it, the text or not, stops following the agent.

use super::{FileView, Open, Shown};
use crate::l10n::{tr, Key};
use crate::theme::{self, Palette, Theme};
use crate::ui::changes::line;
use crate::ui::chrome::{mono, mono_features};
use gpui::{
    canvas, div, fill, point, px, relative, AnyElement, Bounds, ContentMask, Context,
    DispatchPhase, Div, Hsla, InteractiveElement as _, IntoElement, MouseDownEvent,
    ParentElement as _, Pixels, Render, ScrollEvent, StatefulInteractiveElement as _, Styled as _,
    WeakEntity, Window,
};
use gpui_component::input::{Editor, EditorState, RopeExt as _};
use std::ops::RangeInclusive;

impl FileView {
    /// The row over the file: its name, and the folder it is in beside it, quieter and cut when the row
    /// is short. A path is code: it is set in the monospaced face.
    fn header(&self, open: &Open, palette: &Palette) -> Div {
        let (folder, name) = open.path.rsplit_once('/').unwrap_or(("", &open.path));
        let name = div()
            .debug_selector(|| "files-open-name".into())
            .flex_none()
            .text_color(palette.text)
            .child(sakura_core::display::label(name));
        let folder = div()
            .flex_1()
            .min_w_0()
            .truncate()
            .text_color(palette.text_tertiary)
            .child(sakura_core::display::label(folder));
        mono()
            .debug_selector(|| "files-open-path".into())
            .h(theme::TOOLBAR_HEIGHT)
            .flex_none()
            .px(theme::SIDE_PANE_INSET)
            .flex()
            .items_center()
            .gap(theme::GAP)
            .overflow_hidden()
            .text_size(theme::TEXT_LABEL)
            .child(name)
            .child(folder)
    }

    /// "Ask the agent", over the file's top right corner while lines are selected: it stands on the
    /// code, so a selection made or dropped moves nothing.
    fn ask(&self, palette: &Palette, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        self.can_ask.then_some(())?;
        self.reference(cx)?;
        let (fill, hover) = (palette.fill, palette.fill_hover);
        let ask = div()
            .id("files-ask-agent")
            .debug_selector(|| "files-ask-agent".into())
            .absolute()
            .top(theme::GAP)
            .right(theme::SIDE_PANE_INSET)
            .occlude()
            .h(theme::STATUS_CONTROL_HEIGHT)
            .px(theme::CONTROL_PADDING_X)
            .flex()
            .items_center()
            .rounded(theme::RADIUS_CONTROL)
            .bg(fill)
            .hover(move |style| style.bg(hover))
            .text_size(theme::TEXT_SMALL)
            .font_weight(theme::WEIGHT_MEDIUM)
            .text_color(palette.text)
            .on_click(cx.listener(|view, _, _, cx| view.ask_agent(cx)))
            .child(tr(Key("files.ask_agent")));
        Some(ask)
    }

    /// The editor over the whole body, with "Ask the agent" over its corner while lines are selected.
    fn text(&self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let editor = Editor::new(&self.editor)
            .readonly(true)
            .appearance(false)
            .bordered(false)
            .h(relative(1.))
            .text_size(theme::DIFF_TEXT)
            .line_height(px(theme::DIFF_LINE_HEIGHT))
            .font_features(mono_features());
        div()
            .size_full()
            .relative()
            .child(self.changed_rows(line::tint(palette.success)))
            .child(editor)
            .children(self.ask(palette, cx))
            .into_any_element()
    }

    /// Paint the change bands after layout, behind the transparent editor and outside its gutter.
    fn changed_rows(&self, tint: Hsla) -> impl IntoElement {
        let (editor, tinted) = (self.editor.clone(), self.tinted.clone());
        let paint = move |bounds: Bounds<Pixels>, (), window: &mut Window, cx: &mut gpui::App| {
            let bands = row_bands(editor.read(cx), &tinted, bounds);
            window.with_content_mask(Some(ContentMask { bounds }), |window| {
                for band in bands {
                    window.paint_quad(fill(band, tint));
                }
            });
        };
        canvas(|_, _, _| (), paint).absolute().size_full()
    }
}

/// Where the rows of `lines` (numbered from 1) stand in the editor as it was laid out for this frame, as
/// bands across `bounds`; the ones out of sight are left out. Every row is one line height tall, so the
/// row of the first line on screen places all the others.
pub(super) fn row_bands(
    editor: &EditorState,
    lines: &[RangeInclusive<u32>],
    bounds: Bounds<Pixels>,
) -> Vec<Bounds<Pixels>> {
    let (Some(visible), Some(height)) = (editor.visible_row_range(), editor.line_height()) else {
        return Vec::new();
    };
    let map = editor.display_map();
    // The kit's visible range counts the file's lines, not the rows they wrap onto.
    let first_shown = visible.start;
    let start = editor.text().line_start_offset(first_shown);
    let Some(anchor) = editor.range_to_bounds(&(start..start)) else {
        return Vec::new();
    };
    let anchor_row = map.buffer_line_to_display_row(first_shown) as f32;
    let top_of = |row: usize| anchor.top() + height * (row as f32 - anchor_row);
    let last_line = map.buffer_line_count().saturating_sub(1);
    let line_index = |line: u32| (line.max(1) as usize - 1).min(last_line);
    let band = |lines: &RangeInclusive<u32>| {
        let from = map.buffer_line_to_display_row(line_index(*lines.start()));
        let rows = map.buffer_line_to_display_row_range(line_index(*lines.end()));
        let (top, bottom) = (top_of(from), top_of(rows.map_or(from + 1, |rows| rows.end)));
        let is_seen = bottom > bounds.top() && top < bounds.bottom();
        is_seen
            .then(|| Bounds::from_corners(point(bounds.left(), top), point(bounds.right(), bottom)))
    };
    lines.iter().filter_map(band).collect()
}

/// Sees every scroll and every press over the file before the editor does, and tells `view`: paints
/// nothing and takes no event.
fn person_watch(view: WeakEntity<FileView>) -> impl IntoElement {
    let watch = move |bounds: Bounds<Pixels>, (), window: &mut Window, _: &mut gpui::App| {
        let scrolled = view.clone();
        window.on_mouse_event(move |event: &ScrollEvent, phase, _, cx| {
            if phase == DispatchPhase::Capture && bounds.contains(&event.position) {
                drop(scrolled.update(cx, |view, cx| view.person_acted(cx)));
            }
        });
        let pressed = view.clone();
        window.on_mouse_event(move |event: &MouseDownEvent, phase, _, cx| {
            if phase == DispatchPhase::Capture && bounds.contains(&event.position) {
                drop(pressed.update(cx, |view, cx| view.person_acted(cx)));
            }
        });
    };
    canvas(|_, _, _| (), watch).absolute().size_full()
}

/// One quiet sentence where a file would stand.
fn sentence(words: impl Into<gpui::SharedString>, palette: &Palette) -> AnyElement {
    div()
        .size_full()
        .px(theme::SIDE_PANE_INSET)
        .py(theme::GAP)
        .text_size(theme::TEXT_SMALL)
        .text_color(palette.text_tertiary)
        .child(words.into())
        .into_any_element()
}

impl Render for FileView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Theme::palette(cx);
        self.apply_jump(window, cx);
        let is_reading = matches!(
            self.open.as_ref().map(|open| &open.shown),
            Some(Shown::Reading)
        );
        let shows_reading = match is_reading {
            true => self.patience.shows(cx),
            false => {
                self.patience.reset();
                false
            }
        };
        let Some(open) = &self.open else {
            return div().into_any_element();
        };
        let body = match &open.shown {
            Shown::Reading if shows_reading => sentence(tr(Key("files.view.reading")), &palette),
            Shown::Reading => div().into_any_element(),
            Shown::Note(key) => sentence(tr(*key), &palette),
            Shown::Unreadable(why) => {
                let words = crate::l10n::with_details(tr(Key("files.view.unreadable")), Some(why));
                sentence(words, &palette)
            }
            Shown::Text(_) => self.text(&palette, cx),
        };
        let stale = open.is_stale.then(|| {
            div()
                .flex_none()
                .px(theme::SIDE_PANE_INSET)
                .pb(theme::GAP_TIGHT)
                .text_size(theme::TEXT_LABEL)
                .text_color(palette.text_tertiary)
                .child(tr(Key("files.view.stale")))
        });
        div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            // Over the whole view (the row with the name, the text, a sentence in the file's place)
            // and painted first, so it sees an event before anything of the editor can keep it.
            .child(person_watch(cx.weak_entity()))
            .child(self.header(open, &palette))
            .children(stale)
            .child(div().flex_1().min_h_0().child(body))
            .into_any_element()
    }
}
