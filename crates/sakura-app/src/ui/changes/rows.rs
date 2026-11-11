//! The review list as rows: a header per file, and under an open file its hunks and lines, or one sentence
//! when it has none to show. Every row has a known height, so the list is virtualised exactly and a scroll
//! position can be carried across a refresh by path.

use crate::l10n::{tr, tr_with, Key};
use crate::theme;
use sakura_core::patch::{Body, Change, FilePatch, Patch};

/// One row of the review list; indices point into the patch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    Header {
        file: usize,
    },
    Hunk {
        file: usize,
        hunk: usize,
    },
    Line {
        file: usize,
        hunk: usize,
        line: usize,
    },
    Note {
        file: usize,
    },
}

impl Row {
    /// The file the row belongs to.
    pub fn file(self) -> usize {
        match self {
            Row::Header { file }
            | Row::Hunk { file, .. }
            | Row::Line { file, .. }
            | Row::Note { file } => file,
        }
    }

    /// The row's height in pixels; for a code line, the height of one of the rows it wraps onto.
    pub fn height(self) -> f32 {
        match self {
            Row::Header { .. } => theme::CHANGE_HEADER_HEIGHT,
            Row::Hunk { .. } => theme::HUNK_HEADER_HEIGHT,
            Row::Line { .. } => theme::DIFF_LINE_HEIGHT,
            Row::Note { .. } => theme::DIFF_NOTE_HEIGHT,
        }
    }
}

/// The rows of `patch`, with the files `is_open` says are open shown in full.
pub fn rows(patch: &Patch, is_open: impl Fn(&str) -> bool) -> Vec<Row> {
    let mut rows = Vec::new();
    for (index, file) in patch.files.iter().enumerate() {
        rows.push(Row::Header { file: index });
        if !is_open(&file.path) {
            continue;
        }
        if note(file).is_some() {
            rows.push(Row::Note { file: index });
            continue;
        }
        for (hunk_index, hunk) in file.hunks.iter().enumerate() {
            rows.push(Row::Hunk {
                file: index,
                hunk: hunk_index,
            });
            rows.extend((0..hunk.lines.len()).map(|line| Row::Line {
                file: index,
                hunk: hunk_index,
                line,
            }));
        }
    }
    rows
}

/// The word a file header shows beside its figures, when the file as a whole changed.
pub fn change_word(file: &FilePatch) -> Option<&'static str> {
    match file.change {
        Change::Modified => None,
        Change::New => Some(tr(Key("changes.file.new"))),
        Change::Deleted => Some(tr(Key("changes.file.deleted"))),
        Change::Renamed => Some(tr(Key("changes.file.renamed"))),
    }
}

/// The word after the figures of a file whose hunk was cut: they are only the part `box` showed.
pub fn partial_word(file: &FilePatch) -> Option<&'static str> {
    (!file.is_complete).then_some(tr(Key("changes.file.partial")))
}

/// The one sentence an open file shows instead of lines, when it has no lines to show.
pub fn note(file: &FilePatch) -> Option<String> {
    match file.body {
        Body::Binary => return Some(tr(Key("changes.note.binary")).into()),
        Body::Omitted { kilobytes } => {
            let size = crate::l10n::grouped(kilobytes);
            let note = tr_with(Key("changes.note.too_large"), &[("size", &size)]);
            return Some(note.to_string());
        }
        Body::Text if !file.hunks.is_empty() => return None,
        Body::Text => {}
    }
    Some(match (&file.mode, &file.previous_path) {
        (Some(mode), _) => {
            let values = [("old", mode.old.as_str()), ("new", mode.new.as_str())];
            tr_with(Key("changes.note.mode_only"), &values).to_string()
        }
        (None, Some(previous)) => {
            let previous = super::line::display_text(previous);
            tr_with(Key("changes.note.moved"), &[("previous", &previous)]).to_string()
        }
        (None, None) => tr(Key("changes.note.empty")).to_string(),
    })
}

/// Where the list is scrolled, by meaning: the file at the top, the row inside it, and the pixels of that
/// row already above the top edge.
#[derive(Debug, Clone, PartialEq)]
pub struct Anchor {
    pub path: String,
    pub row_in_file: usize,
    pub into_row: f32,
}

/// The rows of a list and the height of each as it is laid out now (a wrapped line is taller).
#[derive(Clone, Copy)]
pub struct Laid<'a> {
    pub rows: &'a [Row],
    pub heights: &'a [f32],
}

/// The anchor at scroll position `top` (pixels from the start of the list).
pub fn anchor_at(laid: Laid, patch: &Patch, top: f32) -> Option<Anchor> {
    let mut start = 0.0;
    let mut first_of_file = 0;
    for (index, (row, height)) in laid.rows.iter().zip(laid.heights).enumerate() {
        if matches!(row, Row::Header { .. }) {
            first_of_file = index;
        }
        let end = start + height;
        if top < end {
            return Some(Anchor {
                path: patch.files.get(row.file())?.path.clone(),
                row_in_file: index - first_of_file,
                into_row: top - start,
            });
        }
        start = end;
    }
    None
}

/// The scroll position that puts `anchor` back at the top, or `None` when its file is gone. A row that is
/// shorter now than the anchor was deep into it shows its last pixel.
pub fn offset_of(anchor: &Anchor, laid: Laid, patch: &Patch) -> Option<f32> {
    let rows = laid.rows;
    let file = patch
        .files
        .iter()
        .position(|file| file.path == anchor.path)?;
    let header = rows.iter().position(|row| *row == Row::Header { file })?;
    let in_file = rows[header..]
        .iter()
        .take_while(|row| row.file() == file)
        .count();
    let target = header + anchor.row_in_file.min(in_file.saturating_sub(1));
    let above: f32 = laid.heights[..target].iter().sum();
    let into_row = if target == header + anchor.row_in_file {
        anchor.into_row.min(laid.heights[target] - 1.0).max(0.0)
    } else {
        0.0
    };
    Some(above + into_row)
}

/// The index of `file`'s header row.
#[cfg(test)]
pub fn header_row(rows: &[Row], file: usize) -> Option<usize> {
    rows.iter().position(|row| *row == Row::Header { file })
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../../../../sakura-core/tests/fixtures/changes.patch");

    fn patch() -> Patch {
        Patch::parse(FIXTURE)
    }

    fn heights(rows: &[Row]) -> Vec<f32> {
        rows.iter().map(|row| row.height()).collect()
    }

    fn file<'a>(patch: &'a Patch, path: &str) -> &'a FilePatch {
        patch.files.iter().find(|file| file.path == path).unwrap()
    }

    #[test]
    fn a_file_whose_hunk_was_cut_says_parziale_and_a_whole_one_says_nothing() {
        crate::l10n::set_for_test(crate::l10n::Language::Italian);
        let cut = Patch::parse("diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1,5 +1,5 @@\n x\n");
        assert_eq!(partial_word(&cut.files[0]), Some("Parziale"));
        let whole = patch();
        assert!(whole.files.iter().all(|file| partial_word(file).is_none()));
    }

    #[test]
    fn closed_files_show_only_their_headers() {
        let patch = patch();
        let rows = rows(&patch, |_| false);
        assert_eq!(rows.len(), patch.files.len());
        assert!(rows.iter().all(|row| matches!(row, Row::Header { .. })));
    }

    #[test]
    fn an_open_file_shows_its_hunks_and_lines_in_place() {
        let patch = patch();
        let rows = rows(&patch, |path| path == "src/lib/library/scope.ts");
        let scope = &patch.files[0];
        assert_eq!(
            rows.len(),
            patch.files.len() + scope.hunks.len() + scope.line_count()
        );
        assert_eq!(rows[1], Row::Hunk { file: 0, hunk: 0 });
        assert_eq!(
            rows[2],
            Row::Line {
                file: 0,
                hunk: 0,
                line: 0
            }
        );
        assert_eq!(rows[rows.len() - 9], Row::Header { file: 1 });
    }

    #[test]
    fn files_without_lines_show_one_sentence() {
        let patch = patch();
        let rows = rows(&patch, |_| true);
        let notes = rows
            .iter()
            .filter(|row| matches!(row, Row::Note { .. }))
            .count();
        assert_eq!(notes, 4, "binary, omitted, mode change and pure rename");
    }

    #[test]
    fn notes_say_what_the_file_is_in_one_sentence() {
        crate::l10n::set_for_test(crate::l10n::Language::Italian);
        let patch = patch();
        let sentence = |path| note(file(&patch, path)).unwrap();
        assert_eq!(
            sentence("public/img/cover.png"),
            "File binario, il contenuto non si può mostrare."
        );
        assert_eq!(
            sentence("src/generated/api-contract.json"),
            "Modifica di 612 kB, troppo grande per mostrarla qui."
        );
        assert_eq!(
            sentence("scripts/build.sh"),
            "Cambiano solo i permessi, da 100644 a 100755."
        );
        assert_eq!(
            sentence("docs/archive/old.md"),
            "Spostato da docs/notes/old.md senza modifiche."
        );
        assert_eq!(note(file(&patch, "src/lib/library/scope.ts")), None);
    }

    #[test]
    fn whole_file_changes_have_a_word_and_edits_have_none() {
        crate::l10n::set_for_test(crate::l10n::Language::Italian);
        let patch = patch();
        let word = |path| change_word(file(&patch, path));
        assert_eq!(word("src/components/library/ItemPicker.tsx"), Some("Nuovo"));
        assert_eq!(
            word("src/components/library/old-picker.tsx"),
            Some("Eliminato")
        );
        assert_eq!(word("src/lib/library/visible.ts"), Some("Rinominato"));
        assert_eq!(word("src/lib/library/scope.ts"), None);
    }

    #[test]
    fn anchor_round_trips_through_the_same_rows() {
        let patch = patch();
        let rows = rows(&patch, |_| true);
        let top = theme::CHANGE_HEADER_HEIGHT * 2.0 + 13.0;
        let heights = heights(&rows);
        let laid = Laid {
            rows: &rows,
            heights: &heights,
        };
        let anchor = anchor_at(laid, &patch, top).unwrap();
        assert_eq!(offset_of(&anchor, laid, &patch), Some(top));
    }

    #[test]
    fn anchor_keeps_its_line_when_lines_above_wrap_onto_more_rows() {
        let patch = patch();
        let rows = rows(&patch, |_| true);
        let flat = heights(&rows);
        let line = rows
            .iter()
            .rposition(|row| matches!(row, Row::Line { .. }))
            .unwrap();
        let top: f32 = flat[..line].iter().sum::<f32>() + 4.0;
        let laid = Laid {
            rows: &rows,
            heights: &flat,
        };
        let anchor = anchor_at(laid, &patch, top).unwrap();
        let mut wrapped = flat.clone();
        wrapped[2] = theme::DIFF_LINE_HEIGHT * 3.0;
        let relaid = Laid {
            rows: &rows,
            heights: &wrapped,
        };
        let moved = offset_of(&anchor, relaid, &patch).unwrap();
        assert_eq!(moved, top + theme::DIFF_LINE_HEIGHT * 2.0);
    }

    #[test]
    fn anchor_deep_in_a_row_that_became_shorter_shows_its_last_pixel() {
        let patch = patch();
        let rows = rows(&patch, |_| true);
        let mut tall = heights(&rows);
        tall[2] = theme::DIFF_LINE_HEIGHT * 3.0;
        let top: f32 = tall[..2].iter().sum::<f32>() + theme::DIFF_LINE_HEIGHT * 2.5;
        let laid = Laid {
            rows: &rows,
            heights: &tall,
        };
        let anchor = anchor_at(laid, &patch, top).unwrap();
        let flat = heights(&rows);
        let relaid = Laid {
            rows: &rows,
            heights: &flat,
        };
        let expected: f32 = flat[..2].iter().sum::<f32>() + theme::DIFF_LINE_HEIGHT - 1.0;
        assert_eq!(offset_of(&anchor, relaid, &patch), Some(expected));
    }

    #[test]
    fn anchor_follows_its_file_when_rows_above_change() {
        let patch = patch();
        let all_open = rows(&patch, |_| true);
        let header = header_row(&all_open, 3).unwrap();
        let top: f32 = all_open[..header + 2]
            .iter()
            .map(|row| row.height())
            .sum::<f32>()
            + 5.0;
        let open_heights = heights(&all_open);
        let laid = Laid {
            rows: &all_open,
            heights: &open_heights,
        };
        let anchor = anchor_at(laid, &patch, top).unwrap();
        assert_eq!(anchor.path, "src/components/library/ItemPicker.tsx");
        let first_closed = rows(&patch, |path| path != patch.files[0].path);
        let closed_heights = heights(&first_closed);
        let relaid = Laid {
            rows: &first_closed,
            heights: &closed_heights,
        };
        let moved = offset_of(&anchor, relaid, &patch).unwrap();
        let header = header_row(&first_closed, 3).unwrap();
        let expected: f32 = first_closed[..header + 2]
            .iter()
            .map(|row| row.height())
            .sum::<f32>();
        assert_eq!(moved, expected + 5.0);
    }

    #[test]
    fn anchor_in_a_file_that_is_gone_is_dropped() {
        let patch = patch();
        let anchor = Anchor {
            path: "non/esiste.ts".into(),
            row_in_file: 0,
            into_row: 0.0,
        };
        let rows = rows(&patch, |_| true);
        let heights = heights(&rows);
        let laid = Laid {
            rows: &rows,
            heights: &heights,
        };
        assert_eq!(offset_of(&anchor, laid, &patch), None);
    }

    #[test]
    fn anchor_past_a_shorter_file_lands_on_its_last_row() {
        let patch = patch();
        let anchor = Anchor {
            path: patch.files[0].path.clone(),
            row_in_file: 500,
            into_row: 7.0,
        };
        let closed = rows(&patch, |_| false);
        let heights = heights(&closed);
        let laid = Laid {
            rows: &closed,
            heights: &heights,
        };
        assert_eq!(offset_of(&anchor, laid, &patch), Some(0.0));
    }
}
