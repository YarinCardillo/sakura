//! The open file: the kit's editor, read only, with the file's bytes from `box file`. Syntax colours and
//! line numbers are the editor's; the lines that differ from the base are marked in its gutter (added,
//! modified, and a mark where lines were removed); a long line wraps onto rows that keep its indent, so
//! nothing is ever cut; the text can be selected and copied, and with lines selected "Ask the agent"
//! hands them over for the conversation's composer. The added and modified lines are also tinted across
//! their rows the way "Changes" tints an added line, so an edit is seen where it lands. A file that is too large or not text is one plain
//! sentence. The file is read again when the task changes and keeps its place.
//!
//! The person taking the view (a scroll, a press) is told to the pane, which stops following the agent.

mod render;

use super::line_index::LineIndex;
use super::source::Source;
use super::syntax;
use crate::l10n::Key;
use crate::theme::{self, Palette, Theme};
use crate::ui::patience::Patience;
use gpui::{
    point, px, AppContext as _, Context, Entity, EventEmitter, Pixels, Subscription, Task, Window,
};
use gpui_component::input::{
    CursorBlink, EditorState, GutterDecoration, GutterDecorationCollection, InputEvent,
};
use sakura_core::error::BridgeResult;
use sakura_core::files::{gutter_marks, FileBody, GutterMark, MarkKind, Reference};
use sakura_core::patch::Patch;
use std::ops::RangeInclusive;
use std::rc::Rc;
use std::sync::Arc;

/// Rows of the file kept above the line a jump goes to, so the line is read with what leads to it.
const ROWS_ABOVE: usize = 3;
/// A jump is applied when the file is drawn and once more on the frame after: the rows a long line
/// wraps onto are only known once the editor has laid the text out at its width.
const JUMP_PASSES: u8 = 2;

/// What the file view tells the pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileViewEvent {
    /// The person scrolled the file or pressed in it.
    PersonActed,
    /// "Ask the agent" was pressed with these lines selected.
    AskAgent(Reference),
}

/// What stands where the file is.
enum Shown {
    Reading,
    /// The editor holds the text; these are its lines.
    Text(LineIndex),
    /// One sentence in place of a file that cannot be shown.
    Note(Key),
    /// A file that could not be read, and the reason in plain words.
    Unreadable(String),
}

struct Open {
    path: String,
    shown: Shown,
    /// The last read failed: what is shown is from the read before.
    is_stale: bool,
}

/// The open file of one task.
pub struct FileView {
    source: Rc<dyn Source>,
    editor: Entity<EditorState>,
    marks: Option<GutterDecorationCollection>,
    /// The lines added or modified, numbered from 1, whose rows are tinted.
    tinted: Rc<[RangeInclusive<u32>]>,
    patch: Option<Arc<Patch>>,
    open: Option<Open>,
    /// The line to go to and the passes left to apply it in.
    jump: Option<(u32, u8)>,
    reading: Option<Task<()>>,
    /// A read of the open file is under way: a jump waits for it, the line may be one only the new text
    /// has.
    is_reading: bool,
    /// The caret policy the app had before the file took the keyboard, to put back when it leaves.
    blink_before: Option<CursorBlink>,
    /// Whether "Ask the agent" is offered over selected lines: not where nothing takes the question.
    can_ask: bool,
    /// When the file began to be read: its line shows only for a slow read.
    patience: Patience,
    _observers: [Subscription; 4],
}

impl EventEmitter<FileViewEvent> for FileView {}

impl FileView {
    /// A view with no file open, reading from `source`.
    pub fn new(source: Rc<dyn Source>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        syntax::install(cx);
        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .soft_wrap(true)
                .searchable(false)
                .folding(false)
                .context_menu(false)
                .gutter_decorations_width(theme::DIFF_BAR_WIDTH)
        });
        // The kit's theme is set again with the window's appearance: the colours follow.
        let on_theme = cx.observe_global::<Theme>(|_, cx| {
            syntax::install(cx);
            cx.notify();
        });
        // A selection made or dropped shows or hides "Ask the agent".
        let on_editor = cx.observe(&editor, |_, _, cx| cx.notify());
        let on_keyboard = cx.subscribe(&editor, Self::keyboard_changed);
        let on_release = cx.on_release(|view, cx| view.restore_caret(cx));
        Self {
            source,
            editor,
            marks: None,
            tinted: Rc::new([]),
            patch: None,
            open: None,
            jump: None,
            reading: None,
            is_reading: false,
            blink_before: None,
            can_ask: true,
            patience: Patience::default(),
            _observers: [on_theme, on_editor, on_keyboard, on_release],
        }
    }

    /// Suppress the shared blink timer while this read-only editor has focus, restoring it on blur.
    fn keyboard_changed(
        &mut self,
        _: Entity<EditorState>,
        event: &InputEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::Focus if self.blink_before.is_none() => {
                let steady = |theme: &mut gpui_component::Theme| {
                    std::mem::replace(&mut theme.cursor_blink, CursorBlink::steady())
                };
                self.blink_before = Some(gpui_component::Theme::update(cx, steady));
            }
            InputEvent::Blur => self.restore_caret(cx),
            _ => {}
        }
    }

    /// Puts back the caret policy the app had before the file took the keyboard.
    fn restore_caret(&mut self, cx: &mut gpui::App) {
        if let Some(before) = self.blink_before.take() {
            gpui_component::Theme::update(cx, |theme| theme.cursor_blink = before);
        }
    }

    /// The view with "Ask the agent" left out, for a place where nothing takes the question.
    #[cfg(not(feature = "boxes"))]
    pub fn without_asking(mut self) -> Self {
        self.can_ask = false;
        self
    }

    pub fn path(&self) -> Option<&str> {
        self.open.as_ref().map(|open| open.path.as_str())
    }

    /// Opens `path` and goes to `line` when one is given. A file already shown is not read again.
    pub fn open(
        &mut self,
        path: &str,
        line: Option<u32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.jump = line.map(|line| (line, JUMP_PASSES));
        let is_shown = self.path() == Some(path) && self.lines().is_some();
        if !is_shown {
            self.open = Some(Open {
                path: path.to_string(),
                shown: Shown::Reading,
                is_stale: false,
            });
            self.read(false, window, cx);
        }
        cx.notify();
    }

    /// Reads the open file again, keeping its place: the task changed.
    pub fn reread(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open.is_some() {
            self.read(true, window, cx);
        }
    }

    /// Takes the task's patch, whose part about the open file marks the gutter.
    pub fn set_patch(&mut self, patch: Option<Arc<Patch>>, cx: &mut Context<Self>) {
        self.patch = patch;
        self.mark_gutter(cx);
    }

    /// The lines the person selected, to ask the agent about; nothing without a selection.
    pub fn reference(&self, cx: &gpui::App) -> Option<Reference> {
        let (path, lines) = (self.path()?, self.lines()?);
        let editor = self.editor.read(cx);
        let selected = editor.selected_range();
        if selected.is_empty() {
            return None;
        }
        // A selection that ends at the start of a line does not take that line.
        let last = lines.line_of(selected.end.saturating_sub(1).max(selected.start));
        let first = lines.line_of(selected.start);
        Some(Reference::new(path, first, last, &editor.selected_value()))
    }

    fn lines(&self) -> Option<&LineIndex> {
        match self.open.as_ref().map(|open| &open.shown) {
            Some(Shown::Text(lines)) => Some(lines),
            _ => None,
        }
    }

    fn read(&mut self, keeps_place: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.path().map(str::to_string) else {
            return;
        };
        let reading = self.source.file(&path);
        self.is_reading = true;
        // A read still under way for another file is dropped with its task.
        self.reading = Some(cx.spawn_in(window, async move |view, cx| {
            let body = reading.await;
            let ended = view.update_in(cx, |view, window, cx| {
                view.read_ended(&path, body, keeps_place, window, cx);
            });
            drop(ended);
        }));
    }

    fn read_ended(
        &mut self,
        path: &str,
        body: BridgeResult<FileBody>,
        keeps_place: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.path() != Some(path) {
            return;
        }
        self.is_reading = false;
        let was_shown = self.lines().is_some();
        match body {
            Ok(FileBody::Text(text)) => self.show_text(&text, keeps_place && was_shown, window, cx),
            Ok(FileBody::Binary) => self.show(Shown::Note(Key("files.view.binary")), false),
            Ok(FileBody::TooLarge) => self.show(Shown::Note(Key("files.view.too_large")), false),
            // What was read before stays, said to be old; a file never read is said unreadable.
            Err(_) if was_shown => self.set_stale(true),
            Err(error) => self.show(Shown::Unreadable(crate::l10n::told(&error)), false),
        }
        cx.notify();
    }

    fn show(&mut self, shown: Shown, is_stale: bool) {
        if let Some(open) = self.open.as_mut() {
            open.shown = shown;
            open.is_stale = is_stale;
        }
    }

    fn set_stale(&mut self, is_stale: bool) {
        if let Some(open) = self.open.as_mut() {
            open.is_stale = is_stale;
        }
    }

    /// Puts `text` in the editor; with `keeps_place` the view stays where the person had it.
    fn show_text(
        &mut self,
        text: &str,
        keeps_place: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(path) = self.path() else {
            return;
        };
        let language = syntax::language_of(path);
        self.show(Shown::Text(LineIndex::of(text)), false);
        // The kit's editor numbers a line after the last line break: a file that ends with one is shown
        // without it, so only the lines the file has are numbered. Every line keeps its bytes.
        let shown = text.strip_suffix('\n').unwrap_or(text).to_string();
        self.editor.update(cx, |editor, cx| {
            let place = editor.scroll_offset();
            editor.set_highlighter(language, cx);
            editor.set_value(shown, window, cx);
            if keeps_place {
                editor.set_scroll_offset(place, cx);
            }
        });
        self.mark_gutter(cx);
    }

    /// Marks the lines of the open file that differ from the base: in the gutter, and the added and
    /// modified ones across their rows.
    fn mark_gutter(&mut self, cx: &mut Context<Self>) {
        let marks = self.file_marks();
        let changed = marks
            .iter()
            .filter(|mark| mark.kind != MarkKind::RemovedAbove);
        self.tinted = changed.map(|mark| mark.lines.clone()).collect();
        let marks = self.gutter_decorations(marks, cx);
        if let Some(old) = self.marks.take() {
            old.dispose(cx);
        }
        let editor = &self.editor;
        self.marks = Some(editor.update(cx, |editor, cx| {
            editor.create_gutter_decorations_collection(marks, cx)
        }));
    }

    /// The marks of the open file's part of the patch; none before its text is shown.
    fn file_marks(&self) -> Vec<GutterMark> {
        let (Some(path), Some(_), Some(patch)) = (self.path(), self.lines(), &self.patch) else {
            return Vec::new();
        };
        let file = patch.files.iter().find(|file| file.path == path);
        file.map(gutter_marks).unwrap_or_default()
    }

    fn gutter_decorations(&self, marks: Vec<GutterMark>, cx: &gpui::App) -> Vec<GutterDecoration> {
        let Some(lines) = self.lines() else {
            return Vec::new();
        };
        let palette = Theme::palette(cx);
        let decoration = |mark: GutterMark| decoration(&mark, lines, &palette);
        marks.into_iter().map(decoration).collect()
    }

    /// Goes to the line of a jump still to apply, once the file's text is read: the line stands a few rows
    /// under the top.
    fn apply_jump(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let is_read = self.lines().is_some() && !self.is_reading;
        let Some((line, passes)) = self.jump.filter(|_| is_read) else {
            return;
        };
        self.editor.update(cx, |editor, cx| {
            let row = editor
                .display_map()
                .buffer_line_to_display_row(line.saturating_sub(1) as usize);
            let line_height = editor.line_height().unwrap_or(px(theme::DIFF_LINE_HEIGHT));
            let top = line_height * row.saturating_sub(ROWS_ABOVE) as f32;
            editor.set_scroll_offset(point(Pixels::ZERO, -top), cx);
        });
        self.jump = (passes > 1).then(|| (line, passes - 1));
        if self.jump.is_some() {
            window.request_animation_frame();
        }
    }

    fn person_acted(&mut self, cx: &mut Context<Self>) {
        // A jump still settling would take the view back from the person.
        self.jump = None;
        cx.emit(FileViewEvent::PersonActed);
    }

    fn ask_agent(&mut self, cx: &mut Context<Self>) {
        if let Some(reference) = self.reference(cx) {
            cx.emit(FileViewEvent::AskAgent(reference));
        }
    }
}

/// The kit's mark for `mark`: the lines it covers in its tone, or the place lines were removed at.
fn decoration(mark: &GutterMark, lines: &LineIndex, palette: &Palette) -> GutterDecoration {
    let (first, last) = (*mark.lines.start(), *mark.lines.end());
    let (range, colour) = match mark.kind {
        MarkKind::Added => (lines.lines(first, last), palette.success),
        MarkKind::Modified => (lines.lines(first, last), palette.warning),
        MarkKind::RemovedAbove => {
            let at = lines.start(first);
            (at..at, palette.danger)
        }
    };
    GutterDecoration::new(range).with_color(colour)
}

#[cfg(test)]
impl FileView {
    #[cfg(test)]
    /// The kit's editor, for the tests.
    pub(in crate::ui) fn editor(&self) -> Entity<EditorState> {
        self.editor.clone()
    }

    #[cfg(test)]
    /// The sentence that stands in the file's place, when one does.
    pub(in crate::ui) fn note(&self) -> Option<Key> {
        match self.open.as_ref().map(|open| &open.shown) {
            Some(Shown::Note(key)) => Some(*key),
            Some(Shown::Unreadable(_)) => Some(Key("files.view.unreadable")),
            _ => None,
        }
    }

    #[cfg(test)]
    /// Whether what is shown is from before a read that failed.
    pub(in crate::ui) fn is_stale(&self) -> bool {
        self.open.as_ref().is_some_and(|open| open.is_stale)
    }

    #[cfg(test)]
    /// The lines whose rows are tinted.
    pub(in crate::ui) fn tinted_lines(&self) -> Vec<RangeInclusive<u32>> {
        self.tinted.to_vec()
    }

    #[cfg(test)]
    /// The bands the tinted rows are painted in, across the editor.
    pub(in crate::ui) fn tint_bands(&self, cx: &gpui::App) -> Vec<gpui::Bounds<Pixels>> {
        let editor = self.editor.read(cx);
        render::row_bands(editor, &self.tinted, editor.input_bounds())
    }

    #[cfg(test)]
    /// How many marks the gutter has.
    pub(in crate::ui) fn mark_count(&self, cx: &gpui::App) -> usize {
        self.marks
            .as_ref()
            .map_or(0, |marks| marks.get_ranges(cx).len())
    }
}
