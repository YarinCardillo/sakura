//! A changed file's header row, one row pitch tall: a chevron, the directory muted then the file name (Geist
//! Mono, as paths in diffs are), the added and removed figures in the interface font with tabular figures,
//! and the word for a file that was created, deleted or moved. The same row is the Modifiche view's file
//! list entry. The chevron starts on the side pane's inset; the figures and the word end on the panel's
//! content inset, under the header's last action.

use super::line::display_text;
use super::rows;
use crate::theme::{self, Palette};
use crate::ui::chrome::{mono, tabular};
use crate::ui::glyph;
use gpui::{
    div, prelude::FluentBuilder as _, Div, Hsla, InteractiveElement as _, IntoElement,
    ParentElement, Styled,
};
use sakura_core::patch::{Body, FilePatch};

/// The content of a file's header: what the caller wraps in its own clickable row.
pub fn content(file: &FilePatch, chevron: Option<bool>, palette: &Palette) -> Div {
    div()
        .size_full()
        .flex()
        .items_center()
        .gap(theme::ROW_GAP)
        .when_some(chevron, |row, is_open| {
            row.child(chevron_mark(is_open, palette.text_tertiary))
        })
        .child(path(file, palette))
        .child(trailing(file, palette))
}

/// The right end of the row: the figures, then the words.
fn trailing(file: &FilePatch, palette: &Palette) -> impl IntoElement {
    div()
        .debug_selector(|| "changes-trailing".into())
        .flex_none()
        .flex()
        .items_center()
        .gap(theme::ROW_GAP)
        .child(figures(file, palette))
        .children(rows::partial_word(file).map(|word| quiet_word(word, palette.warning)))
        .children(rows::change_word(file).map(|word| quiet_word(word, palette.text_tertiary)))
}

fn quiet_word(word: &'static str, colour: Hsla) -> impl IntoElement {
    div()
        .flex_none()
        .text_size(theme::TEXT_LABEL)
        .text_color(colour)
        .child(word)
}

/// The directory, muted and shortened first from its start (its end is the useful part), then the name.
fn path(file: &FilePatch, palette: &Palette) -> impl IntoElement {
    mono()
        .flex_1()
        .min_w_0()
        .flex()
        .text_size(theme::TEXT_LABEL)
        .child(
            div()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis_start()
                .text_right()
                .text_color(palette.text_tertiary)
                .child(display_text(file.directory())),
        )
        .child(
            div()
                .flex_none()
                .text_color(palette.text)
                .child(display_text(file.file_name())),
        )
}

/// "+3 −1", each part only when it is not zero; nothing for a binary or omitted file.
fn figures(file: &FilePatch, palette: &Palette) -> impl IntoElement {
    let (added, removed) = crate::l10n::signed_lines(file.added, file.removed);
    let has_text = file.body == Body::Text;
    div()
        .flex_none()
        .flex()
        .gap(theme::GAP_SMALL)
        .text_size(theme::TEXT_LABEL)
        .font_features(tabular())
        .when(has_text && file.added > 0, |row| {
            row.child(div().text_color(palette.success).child(added))
        })
        .when(has_text && file.removed > 0, |row| {
            row.child(div().text_color(palette.danger).child(removed))
        })
}

/// The chevron: pointing right when closed, down when open.
pub fn chevron_mark(is_open: bool, colour: Hsla) -> impl IntoElement {
    let mark = div().debug_selector(|| "changes-chevron".into());
    mark.flex_none().child(glyph::chevron(is_open, colour))
}
