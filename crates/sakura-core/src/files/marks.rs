//! Which lines of an open file differ from the base, from the file's part of the lavoro's patch: the
//! marks the file view draws in its gutter. Lines are numbered as the file is now (the patch's new side).

use crate::patch::{FilePatch, Hunk, LineKind};
use std::ops::RangeInclusive;

/// What happened at a mark.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkKind {
    /// Lines that are new.
    Added,
    /// Lines that replace others.
    Modified,
    /// Lines were removed right above the mark's line, and nothing took their place.
    RemovedAbove,
}

/// One mark of the gutter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GutterMark {
    pub kind: MarkKind,
    /// The lines marked, numbered from 1. For [`MarkKind::RemovedAbove`] the one line the removed ones
    /// stood before, which is one past the file's last line when they were its end.
    pub lines: RangeInclusive<u32>,
}

/// The marks of a file, in order.
pub fn gutter_marks(file: &FilePatch) -> Vec<GutterMark> {
    file.hunks.iter().flat_map(hunk_marks).collect()
}

/// A run of changed lines between two unchanged ones.
#[derive(Default)]
struct Run {
    removed: u32,
    added: Option<(u32, u32)>,
}

impl Run {
    /// The mark of the run, which ends before line `next` of the file.
    fn mark(&self, next: u32) -> Option<GutterMark> {
        let (kind, lines) = match (self.added, self.removed) {
            (Some((first, last)), 0) => (MarkKind::Added, first..=last),
            (Some((first, last)), _) => (MarkKind::Modified, first..=last),
            (None, 0) => return None,
            (None, _) => (MarkKind::RemovedAbove, next..=next),
        };
        Some(GutterMark { kind, lines })
    }
}

/// The line of the file as it is now that follows a hunk's place, from the new side of its header
/// (`@@ -20,2 +19,0 @@`). A side of no lines names the line before it, so what was removed stood
/// before the line after that one. Never before the first line; the view clamps it to the file's end.
fn line_after(header: &str) -> u32 {
    let side = header.split(' ').find_map(|part| part.strip_prefix('+'));
    let (start, count) = side.map_or(("", ""), |side| side.split_once(',').unwrap_or((side, "1")));
    let start: u32 = start.parse().unwrap_or(0);
    let is_empty_side = count.parse::<u32>() == Ok(0);
    match is_empty_side {
        true => start.saturating_add(1),
        false => start.max(1),
    }
}

fn hunk_marks(hunk: &Hunk) -> Vec<GutterMark> {
    let mut marks = Vec::new();
    let mut run = Run::default();
    // The line of the file the next unchanged or added line of the hunk is. A hunk that only removes
    // has no numbered line of the file: its place is the one its header gives.
    let numbered = hunk.lines.iter().find_map(|line| line.new);
    let mut next = numbered.unwrap_or_else(|| line_after(&hunk.header));
    for line in &hunk.lines {
        match (line.kind, line.new) {
            (LineKind::Removed, _) => run.removed = run.removed.saturating_add(1),
            (LineKind::Added, Some(number)) => {
                let first = run.added.map_or(number, |(first, _)| first);
                run.added = Some((first, number));
                next = number.saturating_add(1);
            }
            (LineKind::Context, Some(number)) => {
                marks.extend(std::mem::take(&mut run).mark(number));
                next = number.saturating_add(1);
            }
            // A line the patch gave no number: nothing to mark it on.
            (LineKind::Added | LineKind::Context, None) => {}
        }
    }
    marks.extend(run.mark(next));
    marks
}
