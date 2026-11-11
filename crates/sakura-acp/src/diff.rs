//! A tool call's diff (the old and the new text of one file) as the hunks the Changes views already draw:
//! `sakura-core`'s [`FilePatch`], with three lines of context. The texts are untrusted and may be whole files,
//! so the comparison is bounded in size and in time.

use sakura_core::patch::{Body, Change, FilePatch, Hunk, Line, LineKind};
use similar::{ChangeTag, DiffOp, TextDiff};
use std::ops::Range;
use std::time::Duration;

/// Unchanged lines shown around a change, as git does.
const CONTEXT_LINES: usize = 3;
/// Past this many bytes over both texts the diff is not computed.
pub const MAX_DIFF_BYTES: usize = 2 * 1024 * 1024;
/// How long the comparison may search for the smallest diff before it settles for a rougher one.
const DIFF_TIMEOUT: Duration = Duration::from_millis(100);

/// The change from `old` to `new` in the file at `path`; `None` when the texts are too large to compare.
/// A missing `old` is a new file.
pub fn file_patch(path: &str, old: Option<&str>, new: &str) -> Option<FilePatch> {
    let old_text = old.unwrap_or("");
    if old_text.len() + new.len() > MAX_DIFF_BYTES {
        return None;
    }
    let diff = TextDiff::configure()
        .timeout(DIFF_TIMEOUT)
        .diff_lines(old_text, new);
    let hunks: Vec<Hunk> = diff
        .grouped_ops(CONTEXT_LINES)
        .iter()
        .map(|group| hunk(&diff, group))
        .collect();
    let count = |kind| {
        let lines = hunks.iter().flat_map(|hunk| &hunk.lines);
        lines.filter(|line| line.kind == kind).count() as u32
    };
    Some(FilePatch {
        path: path.to_string(),
        previous_path: None,
        change: if old.is_some() {
            Change::Modified
        } else {
            Change::New
        },
        body: Body::Text,
        mode: None,
        added: count(LineKind::Added),
        removed: count(LineKind::Removed),
        hunks,
        is_complete: true,
    })
}

fn hunk<'a>(diff: &TextDiff<'a, 'a, str>, group: &[DiffOp]) -> Hunk {
    let lines: Vec<Line> = group
        .iter()
        .flat_map(|op| diff.iter_changes(op))
        .map(|change| {
            let text = change.value();
            Line {
                kind: kind(change.tag()),
                old: change.old_index().map(number),
                new: change.new_index().map(number),
                text: text.trim_end_matches(['\n', '\r']).to_string(),
                has_no_newline: !text.ends_with('\n'),
            }
        })
        .collect();
    Hunk {
        header: header(group),
        lines,
    }
}

fn kind(tag: ChangeTag) -> LineKind {
    match tag {
        ChangeTag::Equal => LineKind::Context,
        ChangeTag::Delete => LineKind::Removed,
        ChangeTag::Insert => LineKind::Added,
    }
}

/// A one-based line number from a zero-based index.
fn number(index: usize) -> u32 {
    u32::try_from(index + 1).unwrap_or(u32::MAX)
}

/// The `@@ -a,b +c,d @@` line of a group of operations.
fn header(group: &[DiffOp]) -> String {
    let (Some(first), Some(last)) = (group.first(), group.last()) else {
        return String::new();
    };
    let old = first.old_range().start..last.old_range().end;
    let new = first.new_range().start..last.new_range().end;
    format!("@@ -{} +{} @@", side(&old), side(&new))
}

/// One side of a hunk header: its first line and its length; an empty side starts on the line before.
fn side(range: &Range<usize>) -> String {
    let start = if range.is_empty() {
        range.start
    } else {
        range.start + 1
    };
    format!("{start},{}", range.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(patch: &FilePatch) -> Vec<(LineKind, Option<u32>, Option<u32>, &str)> {
        let lines = patch.hunks.iter().flat_map(|hunk| &hunk.lines);
        lines
            .map(|line| (line.kind, line.old, line.new, line.text.as_str()))
            .collect()
    }

    #[test]
    fn file_patch_modified_line_is_a_removal_and_an_addition_with_context() {
        let patch = file_patch("a.rs", Some("one\ntwo\nthree\n"), "one\nTWO\nthree\n").unwrap();
        assert_eq!(patch.change, Change::Modified);
        assert_eq!((patch.added, patch.removed), (1, 1));
        assert_eq!(patch.hunks.len(), 1);
        assert_eq!(patch.hunks[0].header, "@@ -1,3 +1,3 @@");
        assert_eq!(
            kinds(&patch),
            [
                (LineKind::Context, Some(1), Some(1), "one"),
                (LineKind::Removed, Some(2), None, "two"),
                (LineKind::Added, None, Some(2), "TWO"),
                (LineKind::Context, Some(3), Some(3), "three"),
            ]
        );
    }

    #[test]
    fn file_patch_without_old_text_is_a_new_file_of_added_lines() {
        let patch = file_patch("new.rs", None, "a\nb\n").unwrap();
        assert_eq!(patch.change, Change::New);
        assert_eq!((patch.added, patch.removed), (2, 0));
        assert_eq!(patch.hunks[0].header, "@@ -0,0 +1,2 @@");
    }

    #[test]
    fn file_patch_distant_changes_are_separate_hunks() {
        let old: String = (1..=40).map(|n| format!("line {n}\n")).collect();
        let new = old
            .replace("line 3\n", "line three\n")
            .replace("line 30\n", "line thirty\n");
        let patch = file_patch("long.txt", Some(&old), &new).unwrap();
        assert_eq!(patch.hunks.len(), 2);
        assert_eq!(patch.hunks[1].header, "@@ -27,7 +27,7 @@");
        assert_eq!((patch.added, patch.removed), (2, 2));
    }

    #[test]
    fn file_patch_identical_texts_have_no_hunks() {
        let patch = file_patch("same.txt", Some("x\n"), "x\n").unwrap();
        assert!(patch.hunks.is_empty());
        assert_eq!((patch.added, patch.removed), (0, 0));
    }

    #[test]
    fn file_patch_last_line_without_newline_is_marked() {
        let patch = file_patch("end.txt", Some("a\n"), "a\nb").unwrap();
        let last = patch.hunks[0].lines.last().unwrap();
        assert_eq!((last.text.as_str(), last.has_no_newline), ("b", true));
    }

    #[test]
    fn file_patch_texts_over_the_limit_are_not_compared() {
        let big = "x\n".repeat(MAX_DIFF_BYTES / 2 + 1);
        assert_eq!(file_patch("big.txt", Some("small\n"), &big), None);
    }
}
