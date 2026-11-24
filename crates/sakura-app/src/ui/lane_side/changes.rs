//! Changes: the files the lane changed since its base with their counts, and under them the patch of the file
//! picked. Everything is read through `sakura flow changes` when the view is shown and when the engine's facts
//! change while it is; the patch is an agent's text, shown as it is.

use crate::data::source::Source;
use crate::l10n::{say, tr, tr_with, Key};
use crate::theme::{self, lane_side as sizes, Palette, Theme};
use crate::ui::changes::line;
use crate::ui::chrome::{mono, tabular, text};
use crate::ui::kit::path_label;
use crate::ui::tip::{self, Words};
use gpui::{
    div, AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    ScrollHandle, SharedString, StatefulInteractiveElement as _, Styled as _, Window,
};
use sakura_core::engine::changes::{ChangedFile, ChangesPage, FilePatch};
use sakura_core::patch::{LineKind, Patch};
use sakura_core::said::Said;
use std::path::PathBuf;

/// What the view has read of the lane's changes.
pub struct LaneChanges {
    source: Source,
    repo: PathBuf,
    task: String,
    page: Option<ChangesPage>,
    /// Why the last read failed, in words.
    problem: Option<String>,
    selected: Option<String>,
    patch: Option<(String, FilePatch)>,
    /// Every changed file is listed, not only the first few.
    is_expanded: bool,
    /// How far the list is scrolled, kept while another view is shown.
    scroll: ScrollHandle,
}

impl LaneChanges {
    /// The view of the lane `task` of `repo`; nothing is read until the view is shown.
    pub fn new(source: Source, repo: PathBuf, task: String, _cx: &mut Context<Self>) -> Self {
        Self {
            source,
            repo,
            task,
            page: None,
            problem: None,
            selected: None,
            patch: None,
            is_expanded: false,
            scroll: ScrollHandle::new(),
        }
    }

    /// Reads the list of changed files again, and the patch of the file picked (the first one by default).
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let pending = self.source.changes(self.repo.clone(), self.task.clone());
        cx.spawn(async move |this, cx| {
            let page = pending.await;
            drop(this.update(cx, |changes, cx| changes.read(page, cx)));
        })
        .detach();
    }

    fn read(
        &mut self,
        page: sakura_core::error::BridgeResult<ChangesPage>,
        cx: &mut Context<Self>,
    ) {
        match page {
            Ok(page) => {
                self.problem = None;
                self.keep_selection_in(&page);
                self.page = Some(page);
                self.read_patch(cx);
            }
            Err(error) => self.problem = Some(failure_words(&error)),
        }
        cx.notify();
    }

    /// The file picked stays picked while it is still changed; else the first file is.
    fn keep_selection_in(&mut self, page: &ChangesPage) {
        let kept = self
            .selected
            .as_ref()
            .filter(|path| page.files.iter().any(|file| &file.path == *path));
        self.selected = kept
            .cloned()
            .or_else(|| page.files.first().map(|file| file.path.clone()));
    }

    /// Picks `path`: its patch is read.
    pub fn select(&mut self, path: &str, cx: &mut Context<Self>) {
        self.selected = Some(path.to_string());
        self.read_patch(cx);
        cx.notify();
    }

    fn read_patch(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.selected.clone() else {
            self.patch = None;
            return;
        };
        let pending = self
            .source
            .changes_file(self.repo.clone(), self.task.clone(), path.clone());
        cx.spawn(async move |this, cx| {
            let patch = pending.await;
            drop(this.update(cx, |changes, cx| changes.patch_read(path, patch, cx)));
        })
        .detach();
    }

    /// The patch of `path` came; one for a file no longer picked is dropped.
    fn patch_read(
        &mut self,
        path: String,
        patch: sakura_core::error::BridgeResult<FilePatch>,
        cx: &mut Context<Self>,
    ) {
        if self.selected.as_deref() != Some(path.as_str()) {
            return;
        }
        match patch {
            Ok(patch) => self.patch = Some((path, patch)),
            Err(error) => self.problem = Some(failure_words(&error)),
        }
        cx.notify();
    }
}

fn failure_words(error: &sakura_core::error::BridgeError) -> String {
    let words = crate::l10n::told(error);
    tr_with(Key("changes.failure"), &[("error", &words)]).to_string()
}

impl Render for LaneChanges {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Theme::palette(cx);
        let body = div()
            .id("lane-side-changes")
            .size_full()
            .track_scroll(&self.scroll)
            .overflow_y_scroll()
            .px(sizes::BODY_PADDING_X)
            .pb(sizes::BODY_PADDING_BOTTOM)
            .flex()
            .flex_col();
        match (&self.problem, &self.page) {
            (Some(problem), _) => body.child(note(
                "changes-problem",
                problem.clone().into(),
                palette.danger,
            )),
            (None, None) => body.child(note(
                "changes-reading",
                tr(Key("changes.reading")).into(),
                palette.text_tertiary,
            )),
            (None, Some(page)) if page.gone => body.child(note(
                "changes-gone",
                tr(Key("lane_side.changes.gone")).into(),
                palette.text_tertiary,
            )),
            (None, Some(page)) if page.files.is_empty() => body.child(note(
                "changes-none",
                tr(Key("changes.none")).into(),
                palette.text_tertiary,
            )),
            (None, Some(page)) => {
                let page = page.clone();
                body.child(self.listing(&page, &palette, cx))
                    .children(self.diff(&palette))
            }
        }
    }
}

fn note(selector: &'static str, words: SharedString, colour: gpui::Hsla) -> AnyElement {
    text(theme::TEXT_SMALL, colour)
        .debug_selector(move || selector.into())
        .pt(sizes::SUBHEAD_GAP_BELOW)
        .child(words)
        .into_any_element()
}

impl LaneChanges {
    /// The totals, the files with their counts and, past a dozen, how many more there are.
    fn listing(&self, page: &ChangesPage, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let shown = match self.is_expanded {
            true => page.files.len(),
            false => sizes::FILES_LISTED.min(page.files.len()),
        };
        let rows: Vec<AnyElement> = page.files[..shown]
            .iter()
            .enumerate()
            .map(|(index, file)| self.file_row(index, file, palette, cx))
            .collect();
        let more = page.files.len() - shown;
        div()
            .flex()
            .flex_col()
            .child(
                text(theme::TEXT_LABEL, palette.text_tertiary)
                    .debug_selector(|| "changes-summary".into())
                    .font_features(tabular())
                    .mt(sizes::SUBHEAD_GAP_BELOW)
                    .mb(sizes::SUBHEAD_GAP_BELOW)
                    .child(summary_words(page)),
            )
            .children(rows)
            .children((more > 0).then(|| self.more_row(more, palette, cx)))
            .into_any_element()
    }

    fn file_row(
        &self,
        index: usize,
        file: &ChangedFile,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let is_picked = self.selected.as_deref() == Some(file.path.as_str());
        let (selector, picked) = (format!("change-file-{index}"), file.path.clone());
        let label = path_label(
            format!("{selector}-path"),
            &file.path,
            (palette.text, palette.text_tertiary),
        )
        .flex_1()
        .text_size(sizes::FILE_TEXT);
        // The whole path is in the tooltip: the row cuts its middle when it is short.
        let path = tip::bind(
            label,
            format!("{selector}-path"),
            Words::plain(file.path.clone()),
            cx,
        );
        let fill = if is_picked {
            palette.selected_row
        } else {
            gpui::transparent_black()
        };
        let hover = palette.hover;
        div()
            .id(SharedString::from(selector.clone()))
            .debug_selector(move || selector)
            .h(sizes::FILE_ROW_HEIGHT)
            .px(theme::GAP)
            .rounded(sizes::FILE_ROW_RADIUS)
            .bg(fill)
            .hover(move |style| style.bg(hover))
            .flex()
            .items_center()
            .gap(theme::GAP)
            .on_click(cx.listener(move |changes, _, _, cx| changes.select(&picked, cx)))
            .child(path)
            .children(counts(file, palette))
            .into_any_element()
    }

    fn more_row(&self, more: usize, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let words = say(&Said::new("lane_side.changes.more").count(more as u64));
        text(theme::TEXT_SMALL, palette.text_tertiary)
            .id("changes-more")
            .debug_selector(|| "changes-more".into())
            .h(sizes::FILE_ROW_HEIGHT)
            .px(theme::GAP)
            .flex()
            .items_center()
            .on_click(cx.listener(|changes, _, _, cx| {
                changes.is_expanded = true;
                cx.notify();
            }))
            .child(words)
            .into_any_element()
    }

    /// The patch of the file picked, under its name.
    fn diff(&self, palette: &Palette) -> Option<AnyElement> {
        let (path, patch) = self.patch.as_ref()?;
        let parsed = Patch::parse_file(path, &patch.patch);
        let hunks = parsed
            .files
            .first()
            .map(|file| file.hunks.as_slice())
            .unwrap_or_default();
        let is_unreadable = hunks.is_empty() && !patch.patch.trim().is_empty();
        let lines = hunks.iter().flat_map(|hunk| {
            let header = text(sizes::FILE_TEXT, palette.text_tertiary)
                .px(theme::GAP)
                .py(theme::GAP_TIGHT)
                .child(hunk.header.clone())
                .into_any_element();
            let rows = hunk
                .lines
                .iter()
                .enumerate()
                .map(|(index, code)| code_line(index, code.kind, &code.text, palette));
            std::iter::once(header).chain(rows)
        });
        Some(
            div()
                .debug_selector(|| "change-diff".into())
                .mt(sizes::SUBHEAD_GAP_ABOVE)
                .flex()
                .flex_col()
                .gap(theme::GAP_SMALL)
                .child(
                    mono()
                        .text_size(sizes::FILE_TEXT)
                        .text_color(palette.text_secondary)
                        .child(path.clone()),
                )
                .child(
                    div()
                        .id("change-diff-lines")
                        .rounded(sizes::DIFF_RADIUS)
                        .bg(palette.inset)
                        .py(theme::GAP)
                        .overflow_x_scroll()
                        .flex()
                        .flex_col()
                        .children(lines)
                        .children(is_unreadable.then(|| {
                            text(theme::TEXT_SMALL, palette.text_tertiary)
                                .debug_selector(|| "change-diff-unreadable".into())
                                .px(theme::GAP)
                                .child(tr(Key("lane_side.changes.unreadable")))
                                .into_any_element()
                        }))
                        .children(patch.truncated.then(|| {
                            text(theme::TEXT_LABEL, palette.text_tertiary)
                                .px(theme::GAP)
                                .child(tr(Key("lane_side.changes.cut")))
                                .into_any_element()
                        })),
                )
                .into_any_element(),
        )
    }
}

/// "+86 -12": the lines added and removed, each in its own colour; nothing for a binary file.
fn counts(file: &ChangedFile, palette: &Palette) -> Option<AnyElement> {
    let (added, removed) = (file.added?, file.removed?);
    Some(
        div()
            .flex_none()
            .flex()
            .gap(theme::GAP_SMALL)
            .text_size(sizes::FILE_TEXT)
            .font_features(tabular())
            .child(div().text_color(palette.success).child(format!("+{added}")))
            .child(
                div()
                    .text_color(palette.danger)
                    .child(format!("-{removed}")),
            )
            .into_any_element(),
    )
}

/// "18 files, +312 -41".
fn summary_words(page: &ChangesPage) -> SharedString {
    let files = say(&Said::new("lane_side.changes.files").count(page.files.len() as u64));
    let (added, removed) = (
        page.added.unwrap_or(0).to_string(),
        page.removed.unwrap_or(0).to_string(),
    );
    tr_with(
        Key("lane_side.changes.summary"),
        &[("files", &files), ("added", &added), ("removed", &removed)],
    )
}

/// One line of a hunk: tinted when added or removed, in the monospaced face, never cut (the block scrolls).
fn code_line(index: usize, kind: LineKind, code: &str, palette: &Palette) -> AnyElement {
    let tint = match kind {
        LineKind::Context => None,
        LineKind::Added => Some(line::tint(palette.success)),
        LineKind::Removed => Some(line::tint(palette.danger)),
    };
    let mark = match kind {
        LineKind::Context => ' ',
        LineKind::Added => '+',
        LineKind::Removed => '-',
    };
    let row = mono()
        .debug_selector(move || format!("change-line-{index}"))
        .px(theme::GAP)
        .text_size(sizes::FILE_TEXT)
        .text_color(palette.text)
        .whitespace_nowrap()
        .child(format!("{mark} {code}"));
    match tint {
        Some(tint) => row.bg(tint),
        None => row,
    }
    .into_any_element()
}
