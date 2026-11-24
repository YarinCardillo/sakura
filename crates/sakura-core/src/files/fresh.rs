//! Where a task changed since its patch was read before: the files whose changed lines are new, each at
//! the first of those lines. An agent that changes a file with a shell command names no file in its tool
//! call; what changed on disk is then the only word of where it was.

use super::marks::line_after;
use crate::patch::{FilePatch, Hunk, Line, LineKind, Patch};
use std::collections::HashMap;

/// A file of the patch and the first of its lines that is new since the patch before, numbered as the
/// file is now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FreshChange {
    pub path: String,
    pub line: u32,
}

/// Return each file's first newly changed line in patch order. Match by change kind and text,
/// including duplicates, so line shifts do not count. Locate deletions at their replacement position.
pub fn fresh_changes(before: &Patch, now: &Patch) -> Vec<FreshChange> {
    let fresh = now.files.iter().filter_map(|file| {
        let earlier = before
            .files
            .iter()
            .find(|earlier| earlier.path == file.path);
        let line = first_fresh_line(earlier, file)?;
        Some(FreshChange {
            path: file.path.clone(),
            line,
        })
    });
    fresh.collect()
}

/// A changed line as it is matched between two patches: whether it was added, and its text.
type Seen<'a> = HashMap<(bool, &'a str), usize>;

/// The first line of `now` whose change `before` does not have.
fn first_fresh_line(before: Option<&FilePatch>, now: &FilePatch) -> Option<u32> {
    let mut seen = Seen::new();
    let earlier = before.into_iter().flat_map(|file| &file.hunks);
    for (line, _) in earlier.flat_map(placed_changes) {
        *seen.entry(key(line)).or_default() += 1;
    }
    let mut placed = now.hunks.iter().flat_map(placed_changes);
    placed
        .find(|(line, _)| !take(&mut seen, key(line)))
        .map(|(_, at)| at)
}

fn key(line: &Line) -> (bool, &str) {
    (line.kind == LineKind::Added, line.text.as_str())
}

/// Takes one of `key` from what was seen; false when there is none left.
fn take<'a>(seen: &mut Seen<'a>, key: (bool, &'a str)) -> bool {
    match seen.get_mut(&key) {
        Some(count) if *count > 0 => {
            *count -= 1;
            true
        }
        _ => false,
    }
}

/// The changed lines of `hunk`, each with the line of the file as it is now that it stands on: an added
/// line on its own number, a removed one on the line that now follows where it was (as the gutter marks
/// it).
fn placed_changes(hunk: &Hunk) -> impl Iterator<Item = (&Line, u32)> {
    let numbered = hunk.lines.iter().find_map(|line| line.new);
    let mut next = numbered.unwrap_or_else(|| line_after(&hunk.header));
    hunk.lines.iter().filter_map(move |line| {
        let at = line.new.unwrap_or(next);
        if let Some(number) = line.new {
            next = number.saturating_add(1);
        }
        (line.kind != LineKind::Context).then_some((line, at))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEAD: &str = "diff --git a/a.md b/a.md\n--- a/a.md\n+++ b/a.md\n";

    fn patch(text: &str) -> Patch {
        Patch::parse(text)
    }

    fn fresh(before: &str, now: &str) -> Vec<(String, u32)> {
        let changes = fresh_changes(&patch(before), &patch(now));
        changes.into_iter().map(|at| (at.path, at.line)).collect()
    }

    fn at(path: &str, line: u32) -> (String, u32) {
        (path.to_string(), line)
    }

    #[test]
    fn a_file_that_joins_the_patch_is_fresh_at_its_first_changed_line() {
        let now = format!("{HEAD}@@ -1,3 +1,4 @@\n one\n two\n+new\n three\n");
        assert_eq!(fresh("", &now), [at("a.md", 3)]);
    }

    #[test]
    fn lines_that_only_moved_are_not_fresh_and_a_line_added_after_them_is() {
        let before = format!("{HEAD}@@ -1,2 +1,3 @@\n one\n+first\n two\n");
        let moved = format!("{HEAD}@@ -1,2 +1,4 @@\n+top\n one\n+first\n two\n");
        assert_eq!(
            fresh(&before, &moved),
            [at("a.md", 1)],
            "only the line on top"
        );
        let appended = format!("{HEAD}@@ -1,2 +1,4 @@\n one\n+first\n two\n+last\n");
        assert_eq!(fresh(&before, &appended), [at("a.md", 4)]);
        assert_eq!(
            fresh(&before, &before),
            [],
            "the same patch has nothing new"
        );
    }

    #[test]
    fn a_line_there_once_more_than_before_is_fresh_at_its_later_place() {
        let before = format!("{HEAD}@@ -1 +1,2 @@\n one\n+again\n");
        let now = format!("{HEAD}@@ -1 +1,3 @@\n one\n+again\n+again\n");
        assert_eq!(fresh(&before, &now), [at("a.md", 3)]);
    }

    #[test]
    fn a_removed_line_is_placed_on_the_line_that_now_stands_where_it_was() {
        let now = format!("{HEAD}@@ -1,3 +1,2 @@\n one\n-two\n three\n");
        assert_eq!(fresh("", &now), [at("a.md", 2)]);
    }

    #[test]
    fn a_file_back_to_its_base_is_not_fresh_and_files_come_in_the_patch_s_order() {
        let other = "diff --git a/b.md b/b.md\n--- a/b.md\n+++ b/b.md\n@@ -1 +1,2 @@\n b\n+b2\n";
        let before = format!("{HEAD}@@ -1 +1,2 @@\n one\n+x\n");
        assert_eq!(fresh(&before, ""), [], "undone: nothing is new");
        let both = format!("{HEAD}@@ -1 +1,2 @@\n one\n+y\n{other}");
        assert_eq!(fresh("", &both), [at("a.md", 2), at("b.md", 2)]);
    }
}
