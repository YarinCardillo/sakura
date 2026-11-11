//! A tool call's diff, in the review list's style: the file's name and figures, hunk headers, and code
//! lines with their old and new numbers, tinted at the theme's diff alpha with a bar at the left edge.
//! Unlike the review list, a line that does not fit the pane wraps under itself, so nothing of a change is
//! ever cut in a narrow pane. A long diff shows its first lines and a row that shows it whole.

use super::{copy, permission, tool};
use crate::l10n::{tr, Key};
use crate::theme::thread as tokens;
use crate::theme::{self, Palette};
use crate::ui::changes::line;
use crate::ui::chrome::{mono, tabular, text};
use gpui::{
    div, px, AnyElement, App, ClickEvent, Font, Hsla, IntoElement, ParentElement, Pixels,
    StatefulInteractiveElement as _, Styled, Window,
};
use sakura_acp::model::DiffContent;
use sakura_acp::paths::{self, Place};
use sakura_core::patch::{Change, FilePatch, Line, LineKind};
use std::path::Path;

/// One diff of a tool call, ready to draw.
pub(super) struct Block<'a> {
    /// The id of the call's entry.
    pub id: u64,
    pub content: &'a DiffContent,
    /// The agent's workspace: paths are shown from it.
    pub workspace: &'a str,
    /// Whether a long diff is shown whole.
    pub is_whole: bool,
}

impl Block<'_> {
    pub(super) fn render(
        &self,
        unfold: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
        palette: &Palette,
        window: &Window,
    ) -> AnyElement {
        let Some(patch) = self.content.patch.as_ref() else {
            return tool::quiet_note(tr(copy::DIFF_TOO_LARGE).to_string(), palette);
        };
        let total = patch.line_count();
        let limit = match self.is_whole {
            true => total,
            false => tokens::DIFF_FOLDED_LINES,
        };
        let more =
            (total > limit).then(|| tool::unfold_row(self.id, total, palette).on_click(unfold));
        let frame = div()
            .w_full()
            .rounded(theme::RADIUS_CONTROL)
            .border_1()
            .border_color(palette.border)
            .overflow_hidden()
            .child(self.header(patch, palette))
            .children(lines(patch, limit, palette, window));
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(theme::GAP_TIGHT)
            .child(frame)
            .children(more)
            .into_any_element()
    }

    /// The file and its added and removed lines in their tones. A file inside the workspace is named from
    /// it, its folder muted; one outside is named by its whole path and marked as outside. A long path
    /// wraps: it is never cut.
    fn header(&self, patch: &FilePatch, palette: &Palette) -> AnyElement {
        let figure = |sign: &str, count: u32, colour| {
            text(theme::TEXT_LABEL, colour)
                .font_features(tabular())
                .flex_none()
                .child(format!("{sign}{count}"))
        };
        let new = (patch.change == Change::New).then(|| {
            text(theme::TEXT_LABEL, palette.text_tertiary)
                .flex_none()
                .child(tr(copy::NEW_FILE))
        });
        let (name, outside) = match shown_file(&patch.path, self.workspace) {
            ShownFile::Inside { folder, name } => (inside_name(&folder, &name, palette), None),
            ShownFile::Outside(path) => {
                let whole = div().min_w_0().text_color(palette.text).child(path);
                (whole, Some(permission::outside_mark(palette)))
            }
        };
        div()
            .w_full()
            .min_h(tokens::LINE_ROW_HEIGHT)
            .px(tokens::BLOCK_PADDING_X)
            .py(theme::GAP_TIGHT)
            .flex()
            .flex_wrap()
            .items_center()
            .gap_x(tokens::LINE_ROW_GAP)
            .bg(palette.inset)
            .text_size(theme::TEXT_SMALL)
            .line_height(tokens::QUIET_LINE_HEIGHT)
            .child(name)
            .children(outside)
            .children(new)
            .child(div().flex_1())
            .child(figure("+", patch.added, palette.success))
            .child(figure("\u{2212}", patch.removed, palette.danger))
            .into_any_element()
    }
}

/// Hunk headers and code lines, up to `limit` lines of code; a file without any says so.
fn lines(patch: &FilePatch, limit: usize, palette: &Palette, window: &Window) -> Vec<AnyElement> {
    let number_width = number_width(patch, window);
    if patch.hunks.is_empty() {
        return vec![note(tr(copy::NO_CHANGE), number_width, palette)];
    }
    let mut rows = Vec::new();
    let mut left = limit;
    for hunk in &patch.hunks {
        if left == 0 {
            break;
        }
        rows.push(hunk_header(&hunk.header, number_width, palette));
        let shown = hunk.lines.iter().take(left);
        rows.extend(shown.map(|code| code_line(code, number_width, palette)));
        left = left.saturating_sub(hunk.lines.len());
    }
    rows
}

/// Where code starts, from the row's left edge: the two number columns and the padding.
fn code_start(number_width: Pixels) -> Pixels {
    number_width * 2. + theme::DIFF_CODE_PADDING
}

/// A hunk's `@@` line, muted, aligned with the code.
fn hunk_header(header: &str, number_width: Pixels, palette: &Palette) -> AnyElement {
    mono()
        .w_full()
        .min_h(px(theme::HUNK_HEADER_HEIGHT))
        .flex()
        .items_center()
        .pl(code_start(number_width))
        .pr(theme::DIFF_CODE_PADDING)
        .text_size(theme::DIFF_TEXT)
        .line_height(px(theme::DIFF_LINE_HEIGHT))
        .text_color(palette.text_tertiary)
        .child(div().min_w_0().child(line::display_text(header)))
        .into_any_element()
}

/// The sentence of a file that has no lines to show.
fn note(sentence: &'static str, number_width: Pixels, palette: &Palette) -> AnyElement {
    text(theme::TEXT_SMALL, palette.text_tertiary)
        .w_full()
        .min_h(px(theme::DIFF_NOTE_HEIGHT))
        .flex()
        .items_center()
        .pl(code_start(number_width))
        .pr(theme::DIFF_CODE_PADDING)
        .child(sentence)
        .into_any_element()
}

/// One line of code: its tint and bar when it changed, its old and new numbers, and the code, which wraps
/// under itself when the pane is narrower than the line.
fn code_line(code: &Line, number_width: Pixels, palette: &Palette) -> AnyElement {
    let tone = line_tone(code.kind, palette);
    let number = |value: Option<u32>| {
        text(theme::TEXT_LABEL, palette.text_tertiary)
            .w(number_width)
            .flex_none()
            .flex()
            .justify_end()
            .font_features(tabular())
            .children(value.map(|value| value.to_string()))
    };
    let bar = div()
        .absolute()
        .top_0()
        .bottom_0()
        .left_0()
        .w(theme::DIFF_BAR_WIDTH)
        .bg(tone.unwrap_or(gpui::transparent_black()));
    let no_newline = code.has_no_newline.then(|| {
        text(theme::TEXT_LABEL, palette.text_tertiary)
            .font_family(theme::FONT_UI)
            .child(tr(Key("diff.no_newline")))
    });
    let words = mono()
        .flex_1()
        .min_w_0()
        .pl(theme::DIFF_CODE_PADDING)
        .pr(theme::DIFF_CODE_PADDING)
        .flex()
        .flex_wrap()
        .gap_x(theme::DIFF_CODE_PADDING)
        .text_size(theme::DIFF_TEXT)
        .text_color(palette.text)
        .child(div().min_w_0().child(line::display_text(&code.text)))
        .children(no_newline);
    div()
        .w_full()
        .min_h(px(theme::DIFF_LINE_HEIGHT))
        .relative()
        .flex()
        .items_start()
        .line_height(px(theme::DIFF_LINE_HEIGHT))
        .bg(tone.map_or(gpui::transparent_black(), |tone| {
            tone.opacity(theme::DIFF_TINT_ALPHA)
        }))
        .child(bar)
        .child(number(code.old))
        .child(number(code.new))
        .child(words)
        .into_any_element()
}

/// The tone of a changed line; none for context.
fn line_tone(kind: LineKind, palette: &Palette) -> Option<Hsla> {
    match kind {
        LineKind::Context => None,
        LineKind::Added => Some(palette.success),
        LineKind::Removed => Some(palette.danger),
    }
}

/// How a diff's file is named in its header.
#[derive(Debug, PartialEq, Eq)]
enum ShownFile {
    /// Inside the workspace: the folder from the workspace (with its trailing slash, or empty) and the name.
    Inside { folder: String, name: String },
    /// Outside the workspace: the whole absolute path.
    Outside(String),
}

/// How the file at `path` is named, by the same reading of paths the confinement policy decides with.
fn shown_file(path: &str, workspace: &str) -> ShownFile {
    match paths::place(path, Path::new(workspace)) {
        Place::Inside(rest) => {
            let split = rest.rfind('/').map_or(0, |slash| slash + 1);
            ShownFile::Inside {
                folder: line::display_text(&rest[..split]),
                name: line::display_text(&rest[split..]),
            }
        }
        Place::Outside(whole) => ShownFile::Outside(line::display_text(&whole)),
    }
}

/// A file's folder, muted, then its name; both whole.
fn inside_name(folder: &str, name: &str, palette: &Palette) -> gpui::Div {
    div()
        .flex()
        .flex_wrap()
        .min_w_0()
        .child(
            div()
                .min_w_0()
                .text_color(palette.text_tertiary)
                .child(folder.to_string()),
        )
        .child(
            div()
                .min_w_0()
                .text_color(palette.text)
                .child(name.to_string()),
        )
}

/// The width of one line-number column: as wide as the largest number of the file, and its padding.
fn number_width(patch: &FilePatch, window: &Window) -> Pixels {
    let numbers = patch.hunks.iter().flat_map(|hunk| &hunk.lines);
    let largest = numbers
        .filter_map(|line| line.old.max(line.new))
        .max()
        .unwrap_or(0);
    let digits = largest.to_string().len().max(2);
    figure_width(window) * digits as f32 + theme::DIFF_GUTTER_PADDING
}

/// The advance of one tabular figure in the interface font at the label size, as the review list
/// measures it.
fn figure_width(window: &Window) -> Pixels {
    let font = Font {
        family: theme::FONT_UI.into(),
        features: tabular(),
        ..Font::default()
    };
    let system = window.text_system();
    let id = system.resolve_font(&font);
    system
        .advance(id, theme::TEXT_LABEL, '0')
        .map_or(theme::TEXT_LABEL * theme::FALLBACK_ADVANCE, |advance| {
            advance.width
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inside(folder: &str, name: &str) -> ShownFile {
        ShownFile::Inside {
            folder: folder.to_string(),
            name: name.to_string(),
        }
    }

    #[test]
    fn shown_file_inside_the_workspace_is_named_from_it() {
        assert_eq!(
            shown_file("/work/m1/src/a/lib.rs", "/work/m1"),
            inside("src/a/", "lib.rs")
        );
        assert_eq!(
            shown_file("/work/m1/Cargo.toml", "/work/m1"),
            inside("", "Cargo.toml")
        );
        assert_eq!(
            shown_file("src/lib.rs", "/work/m1"),
            inside("src/", "lib.rs")
        );
    }

    #[test]
    fn shown_file_of_a_sibling_that_shares_the_workspace_prefix_is_whole_and_outside() {
        let shown = shown_file("/work/m1-evil/src/lib.rs", "/work/m1");
        assert_eq!(
            shown,
            ShownFile::Outside("/work/m1-evil/src/lib.rs".to_string())
        );
    }

    #[test]
    fn shown_file_with_dots_is_resolved_before_it_is_named() {
        let out = shown_file("/work/m1/../other/lib.rs", "/work/m1");
        assert_eq!(out, ShownFile::Outside("/work/other/lib.rs".to_string()));
        assert_eq!(
            shown_file("/work/m1/a/../lib.rs", "/work/m1"),
            inside("", "lib.rs")
        );
    }
}
