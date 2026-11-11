//! Shaped code lines and line numbers kept across frames, so a scroll shapes only the lines it has never
//! shown. Cleared when the patch changes (lines are keyed by their place in it) or the appearance does (the
//! colour is part of a shaped line); the code alone when the wrap changes (a line is shaped row by row).

use super::code_line::{code_font, run, CodeRow};
use super::line;
use super::wrap::{self, Wrap};
use crate::theme::{self, Appearance};
use crate::ui::chrome::tabular;
use gpui::{Font, Hsla, ShapedLine, SharedString, Window};
use sakura_core::patch::Line;
use std::collections::HashMap;
use std::rc::Rc;

/// Where a line is in the patch: file, hunk, line.
pub type LineKey = (usize, usize, usize);

/// Shaped lines of one review list.
#[derive(Default)]
pub struct ShapeCache {
    appearance: Option<Appearance>,
    code: HashMap<LineKey, Rc<[CodeRow]>>,
    numbers: HashMap<u32, ShapedLine>,
}

impl ShapeCache {
    /// Forgets every shaped line (a new patch).
    pub fn clear(&mut self) {
        self.code.clear();
        self.numbers.clear();
    }

    /// Forgets the shaped code (a new wrap); the numbers stay.
    pub fn clear_code(&mut self) {
        self.code.clear();
    }

    /// Forgets every shaped line when the appearance is not the one they were shaped for.
    pub fn follow(&mut self, appearance: Appearance) {
        if self.appearance != Some(appearance) {
            self.clear();
            self.appearance = Some(appearance);
        }
    }

    /// The code of `line`, at `key`, in `colour`, shaped row by row as `wrap` breaks it.
    pub fn code(
        &mut self,
        key: LineKey,
        line: &Line,
        (wrap, colour): (Wrap, Hsla),
        window: &Window,
    ) -> Rc<[CodeRow]> {
        let shape = || {
            let text = line::display_text(&line.text);
            let rows = wrap::rows(&text, wrap).into_iter().map(|bytes| {
                let part = SharedString::from(text[bytes.clone()].to_string());
                let runs = [run(&part, code_font(), colour)];
                let shaped = window
                    .text_system()
                    .shape_line(part, theme::DIFF_TEXT, &runs, None);
                CodeRow { shaped, bytes }
            });
            rows.collect()
        };
        self.code.entry(key).or_insert_with(shape).clone()
    }

    /// A line number in tabular figures, in `colour`.
    pub fn number(&mut self, value: u32, colour: Hsla, window: &Window) -> ShapedLine {
        self.numbers
            .entry(value)
            .or_insert_with(|| {
                let text = SharedString::from(value.to_string());
                let font = Font {
                    family: theme::FONT_UI.into(),
                    features: tabular(),
                    ..Font::default()
                };
                let runs = [run(&text, font, colour)];
                window
                    .text_system()
                    .shape_line(text, theme::TEXT_LABEL, &runs, None)
            })
            .clone()
    }
}
