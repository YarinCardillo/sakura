//! A changed file's header row, one row pitch tall: a chevron, the directory muted then the file name (Geist
//! Mono, as paths in diffs are), the added and removed figures in the interface font with tabular figures,
//! and the word for a file that was created, deleted or moved. The same row is the Changes view's file
//! list entry. The chevron starts on the side pane's inset; the figures and the word end on the panel's
//! content inset, under the header's last action.

use super::code_line;
use super::line::display_text;
use super::rows;
use crate::theme::{self, Palette};
use crate::ui::chrome::{mono, mono_features, tabular};
use crate::ui::glyph;
use gpui::{
    canvas, div, point, prelude::FluentBuilder as _, App, Bounds, ContentMask, Div, Font, Hsla,
    InteractiveElement as _, IntoElement, ParentElement, Pixels, ShapedLine, SharedString, Styled,
    TextAlign, Window,
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

/// What stands for the folders that left the start of a directory.
const ELLIPSIS: &str = "…/";

/// The directory, muted, then the name, drawn in the room the row leaves them. When they do not fit,
/// whole folders leave from the start of the directory and an ellipsis stands for them: the cut is at a
/// separator, never inside a word, and the name stays whole.
fn path(file: &FilePatch, palette: &Palette) -> impl IntoElement {
    let directory = display_text(file.directory());
    let name = display_text(file.file_name());
    let inks = (palette.text_tertiary, palette.text);
    let drawn = canvas(
        move |bounds, window, _| shaped_path(&directory, &name, bounds.size.width, inks, window),
        paint_path,
    );
    mono()
        .debug_selector(|| "changes-path".into())
        .flex_1()
        .min_w_0()
        .h_full()
        .text_size(theme::TEXT_LABEL)
        .child(drawn.size_full())
}

/// A path as it is drawn: what is shown of its directory, then its name.
struct PathLines {
    directory: ShapedLine,
    name: ShapedLine,
}

/// The directory and the name shaped for `room`, in their two inks (the directory's, the name's).
fn shaped_path(
    directory: &str,
    name: &str,
    room: Pixels,
    (muted, ink): (Hsla, Hsla),
    window: &Window,
) -> PathLines {
    let name = shape(name, ink, window);
    let whole = shape(directory, muted, window);
    let ellipsis = shape(ELLIPSIS, muted, window).width;
    let width_from = |at: usize| whole.width - whole.x_for_index(at);
    let shown = shown_directory(directory, room - name.width, ellipsis, width_from);
    let directory = match shown == directory {
        true => whole,
        false => shape(&shown, muted, window),
    };
    PathLines { directory, name }
}

/// What is shown of `directory` (it ends with its separator) in `room`: all of it when it fits, else the
/// ellipsis and the most whole folders of its end that fit after it, down to the ellipsis alone.
/// `width_from` is the width of the directory from a byte of it on.
fn shown_directory(
    directory: &str,
    room: Pixels,
    ellipsis: Pixels,
    width_from: impl Fn(usize) -> Pixels,
) -> String {
    if directory.is_empty() || width_from(0) <= room {
        return directory.to_string();
    }
    let folders = directory.match_indices('/').map(|(slash, _)| slash + 1);
    let mut starts = folders.filter(|start| *start < directory.len());
    let kept = starts.find(|start| ellipsis + width_from(*start) <= room);
    match kept {
        Some(start) => format!("{ELLIPSIS}{}", &directory[start..]),
        None => ELLIPSIS.to_string(),
    }
}

/// `text` in the code font at the label size, in one ink.
fn shape(text: &str, ink: Hsla, window: &Window) -> ShapedLine {
    let font = Font {
        family: theme::FONT_MONO.into(),
        features: mono_features(),
        ..Font::default()
    };
    let text = SharedString::from(text.to_string());
    let runs = [code_line::run(&text, font, ink)];
    window
        .text_system()
        .shape_line(text, theme::TEXT_LABEL, &runs, None)
}

/// Draws the directory then the name on the row's centre line, inside the room they were shaped for.
fn paint_path(bounds: Bounds<Pixels>, lines: PathLines, window: &mut Window, cx: &mut App) {
    let height = window.line_height();
    let top = bounds.top() + (bounds.size.height - height) / 2.;
    let name_left = bounds.left() + lines.directory.width;
    let parts = [(&lines.directory, bounds.left()), (&lines.name, name_left)];
    window.with_content_mask(Some(ContentMask { bounds }), |window| {
        for (line, left) in parts {
            let drawn = line.paint(point(left, top), height, TextAlign::Left, None, window, cx);
            if let Err(error) = drawn {
                eprintln!("sakura: cannot paint the path of a changed file: {error}");
            }
        }
    });
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::interaction_tests::{click, draw, open_with, wait_for};
    use crate::ui::real_text;
    use gpui::{px, size};

    /// Seven points a letter, as a code font has one advance.
    fn shown(directory: &str, room: f32) -> String {
        let width_from = |at: usize| px(7. * directory[at..].chars().count() as f32);
        shown_directory(directory, px(room), px(14.), width_from)
    }

    #[test]
    fn a_directory_that_fits_is_shown_whole_and_one_that_does_not_loses_whole_folders() {
        let directory = "apps/web/src/catalogs/";
        assert_eq!(shown(directory, 154.), directory, "22 letters fit in 154");
        assert_eq!(shown(directory, 153.), "…/web/src/catalogs/");
        assert_eq!(
            shown(directory, 132.),
            "…/src/catalogs/",
            "web/ no longer fits"
        );
        assert_eq!(shown(directory, 104.), "…/catalogs/", "src/ no longer fits");
        assert_eq!(shown(directory, 76.), "…/", "the last folder does not fit");
        assert_eq!(shown(directory, 0.), "…/");
        assert_eq!(shown("", 0.), "", "a file at the top has no directory");
    }

    #[test]
    fn at_the_narrow_pane_a_long_path_is_cut_at_a_separator_and_its_name_stays_whole() {
        real_text::run(|cx| {
            let (cx, _, _) = open_with(cx, None);
            wait_for(cx, "the rows", |cx| {
                cx.debug_bounds("row-sample-accent").is_some()
            });
            click(cx, "row-sample-accent");
            wait_for(cx, "the changes", |cx| {
                cx.debug_bounds("changes-path").is_some()
            });
            cx.simulate_resize(size(px(1100.), px(700.)));
            draw(cx);
            let room = cx.debug_bounds("changes-path").expect("a path").size.width;
            let directory = "apps/web/src/app/features/catalogs/components/tables/";
            let name = "visible.ts";
            let lines = cx.update(|window, cx| {
                let palette = crate::theme::Theme::palette(cx);
                let inks = (palette.text_tertiary, palette.text);
                shaped_path(directory, name, room, inks, window)
            });
            let cut = lines.directory.text.to_string();
            let kept = cut
                .strip_prefix(ELLIPSIS)
                .expect("the ellipsis stands first");
            let before = directory
                .strip_suffix(kept)
                .expect("the end of the directory");
            assert!(
                before.ends_with('/'),
                "whole folders left, at a separator: {cut}"
            );
            assert_eq!(lines.name.text.as_ref(), name, "the name is whole");
            assert!(lines.directory.width + lines.name.width <= room, "{cut}");
            let one_more = before[..before.len() - 1].rfind('/').map_or(0, |at| at + 1);
            let longer = format!("{ELLIPSIS}{}", &directory[one_more..]);
            let longer = cx.update(|window, _| shape(&longer, gpui::black(), window).width);
            assert!(
                longer + lines.name.width > room,
                "no folder left that had room: {cut}"
            );
        });
    }
}
