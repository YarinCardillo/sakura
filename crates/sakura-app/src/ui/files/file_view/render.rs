//! How the open file is drawn: one row with its name and folder, then the kit's editor over the rest,
//! with "Ask the agent" over its corner while lines are selected. The whole view is watched for the
//! person taking it: a scroll or a press anywhere on it, the text or not, stops following the agent.

use super::{FileView, Open, Shown};
use crate::l10n::{tr, Key};
use crate::theme::{self, Palette, Theme};
use crate::ui::chrome::{mono, mono_features};
use gpui::{
    canvas, div, px, relative, AnyElement, Bounds, Context, DispatchPhase, Div,
    InteractiveElement as _, IntoElement, MouseDownEvent, ParentElement as _, Pixels, Render,
    ScrollEvent, StatefulInteractiveElement as _, Styled as _, WeakEntity, Window,
};
use gpui_component::input::Editor;

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
            .child(editor)
            .children(self.ask(palette, cx))
            .into_any_element()
    }
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
fn sentence(words: &'static str, palette: &Palette) -> AnyElement {
    div()
        .size_full()
        .px(theme::SIDE_PANE_INSET)
        .py(theme::GAP)
        .text_size(theme::TEXT_SMALL)
        .text_color(palette.text_tertiary)
        .child(words)
        .into_any_element()
}

impl Render for FileView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Theme::palette(cx);
        self.apply_jump(window, cx);
        let Some(open) = &self.open else {
            return div().into_any_element();
        };
        let body = match &open.shown {
            Shown::Reading => sentence(tr(Key("files.view.reading")), &palette),
            Shown::Note(key) => sentence(tr(*key), &palette),
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
