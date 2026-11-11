//! Changes in fixture mode: a realistic patch (every kind of file the review pane shows), a log of three
//! commits, and a generated patch of about 5000 lines for the task with the largest figures, so scrolling
//! and memory can be measured without `box`.

use sakura_core::error::{BridgeError, BridgeResult};
use sakura_core::models::Commit;
use sakura_core::said::Said;

/// The task whose fixture patch is the large one.
pub const LARGE_TASK: &str = "sample-folders";
/// The unreadable fixture task: its patch cannot be read.
const UNREADABLE_TASK: &str = "broken-check";
const REALISTIC: &str = include_str!("../../../sakura-core/tests/fixtures/changes.patch");
const LOG: &str = include_str!("../../../sakura-core/tests/fixtures/log.json");
const GIT_LINE: &str = "diff --git ";

/// Files in the large patch, and the hunks and lines of each.
const LARGE_FILES: usize = 16;
const LARGE_HUNKS: usize = 8;
const LARGE_HUNK_LINES: usize = 39;
/// The long line in the first hunk of each large file, for horizontal scrolling.
const LONG_LINE: usize = 600;

/// The patch `box diff <task> [--commit <sha>]` would print for a fixture task.
pub fn patch(task: &str, commit: Option<&str>) -> BridgeResult<String> {
    if task == UNREADABLE_TASK {
        return Err(BridgeError::failed(Said::new("changes.unreadable")));
    }
    match commit {
        None if task == LARGE_TASK => Ok(large_patch()),
        None => Ok(REALISTIC.to_string()),
        Some(sha) => commit_patch(sha),
    }
}

/// The commits `box log <task>` would list: the fixture log for every readable task.
pub fn log(task: &str) -> BridgeResult<Vec<Commit>> {
    if task == UNREADABLE_TASK {
        return Err(BridgeError::failed(Said::new("changes.unreadable")));
    }
    serde_json::from_str(LOG).map_err(|error| {
        sakura_core::box_cli::internal(format!("the sample log does not parse: {error}"))
    })
}

/// One commit's patch: every third file of the realistic patch, starting at the commit's place in the log.
fn commit_patch(sha: &str) -> BridgeResult<String> {
    let commits = log("")?;
    let Some(place) = commits.iter().position(|commit| commit.sha == sha) else {
        return Err(BridgeError::invalid(Said::new("fixture.commit_missing")));
    };
    let files = REALISTIC.split(GIT_LINE).skip(1);
    let chosen = files
        .enumerate()
        .filter(|(index, _)| index % commits.len() == place);
    Ok(chosen
        .map(|(_, file)| format!("{GIT_LINE}{file}"))
        .collect())
}

/// About 5000 lines over [`LARGE_FILES`] files: context, removals, additions and one very long line each.
pub fn large_patch() -> String {
    let mut text = String::new();
    for file in 0..LARGE_FILES {
        let path = format!("src/generated/module-{file:02}.ts");
        text.push_str(&format!(
            "{GIT_LINE}a/{path} b/{path}\nindex 1111111..2222222 100644\n--- a/{path}\n+++ b/{path}\n"
        ));
        for hunk in 0..LARGE_HUNKS {
            push_large_hunk(&mut text, file, hunk);
        }
    }
    text
}

fn push_large_hunk(text: &mut String, file: usize, hunk: usize) {
    let start = hunk * 100 + 1;
    let prefixes: Vec<char> = (0..LARGE_HUNK_LINES).map(large_prefix).collect();
    let old = prefixes.iter().filter(|prefix| **prefix != '+').count();
    let new = prefixes.iter().filter(|prefix| **prefix != '-').count();
    text.push_str(&format!(
        "@@ -{start},{old} +{start},{new} @@ export function module{file}()\n"
    ));
    for (line, prefix) in prefixes.into_iter().enumerate() {
        let body = if line == 20 && hunk == 0 {
            "x".repeat(LONG_LINE)
        } else {
            format!("const value{line} = calculate({file}, {hunk}, {line});")
        };
        text.push(prefix);
        text.push_str(&body);
        text.push('\n');
    }
}

/// Four removed then four added lines in every ten, the rest unchanged.
fn large_prefix(line: usize) -> char {
    match line % 10 {
        3..=6 => '-',
        7..=9 => '+',
        0 if line > 0 => '+',
        _ => ' ',
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sakura_core::patch::Patch;

    #[test]
    fn realistic_patch_parses_completely_with_every_kind() {
        let patch = Patch::parse(&patch("sample-accent", None).unwrap());
        assert!(patch.is_complete);
        assert_eq!(patch.files.len(), 10);
    }

    #[test]
    fn large_patch_is_about_five_thousand_lines_and_complete() {
        let patch = Patch::parse(&patch(LARGE_TASK, None).unwrap());
        assert!(
            patch.is_complete,
            "the generated hunks must match their counts"
        );
        let lines: usize = patch.files.iter().map(|file| file.line_count()).sum();
        assert!((4900..=5100).contains(&lines), "{lines}");
    }

    #[test]
    fn each_commit_has_its_own_files() {
        let commits = log("sample-accent").unwrap();
        assert_eq!(commits.len(), 3);
        let first = Patch::parse(&patch("sample-accent", Some(&commits[0].sha)).unwrap());
        let second = Patch::parse(&patch("sample-accent", Some(&commits[1].sha)).unwrap());
        assert!(first.is_complete && second.is_complete);
        assert_ne!(first.files[0].path, second.files[0].path);
    }

    #[test]
    fn unreadable_task_has_no_patch() {
        assert!(patch(UNREADABLE_TASK, None).is_err());
    }
}
