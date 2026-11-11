//! The pointer on the review list: a press on a code line starts a selection, a drag extends it, Cmd+C (or
//! the Edit menu) copies it. A press anywhere in the list gives it the keyboard; it never takes it alone.

use super::code_line::{code_font, run};
use super::line;
use super::list::ChangesList;
use super::selection::TextPoint;
use super::wrap;
use crate::theme::{self, Theme};
use gpui::{
    px, App, ClipboardItem, Context, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, Point,
    Window,
};

impl ChangesList {
    /// A press: the list takes the keyboard, and a selection starts on the code line under the pointer.
    pub(super) fn press(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus, cx);
        let had_selection = match self.point_at(event.position, window, cx) {
            Some(point) => {
                self.selection.start(point);
                true
            }
            None => self.selection.clear(),
        };
        if had_selection {
            cx.notify();
        }
    }

    /// A drag with the button down extends the selection to the code line under the pointer.
    pub(super) fn drag(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.selection.is_dragging() {
            return;
        }
        if event.pressed_button != Some(MouseButton::Left) {
            self.selection.end();
            return;
        }
        let moved = self
            .point_at(event.position, window, cx)
            .is_some_and(|point| self.selection.extend(point));
        if moved {
            cx.notify();
        }
    }

    /// The press ended.
    pub(super) fn release(&mut self) {
        self.selection.end();
    }

    /// Copies the selected code to the clipboard.
    pub(super) fn copy(&mut self, cx: &mut Context<Self>) {
        let text = self.selection.text(|row| self.code_text(row));
        if let Some(text) = text {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    /// The row and byte under `position` in the window, when it is over a code line.
    fn point_at(
        &self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &App,
    ) -> Option<TextPoint> {
        let body = self.body.get();
        let y = f32::from(position.y - body.top() - self.scroll.offset().y);
        let row = self
            .origins
            .partition_point(|origin| *origin <= y)
            .checked_sub(1)?;
        let text = self.code_text(row)?;
        let style = self.layout(window, cx).line;
        let rows = wrap::rows(&text, self.wrap);
        let within = (y - self.origins[row]) / theme::DIFF_LINE_HEIGHT;
        let at = (within.max(0.0) as usize).min(rows.len().saturating_sub(1));
        let part = &text[rows.get(at)?.clone()];
        let left = body.left() + style.code_start() + style.indent(at);
        let x = (position.x - left).max(px(0.));
        let runs = [run(part, code_font(), Theme::palette(cx).text)];
        let shaped = window
            .text_system()
            .layout_line(part, theme::DIFF_TEXT, &runs, None);
        Some(TextPoint {
            row,
            index: rows[at].start + shaped.closest_index_for_x(x),
        })
    }

    /// The shown text of `row`, when it is a code line.
    fn code_text(&self, row: usize) -> Option<String> {
        let line = self.line_of(*self.rows.get(row)?)?;
        Some(line::display_text(&line.text))
    }
}
