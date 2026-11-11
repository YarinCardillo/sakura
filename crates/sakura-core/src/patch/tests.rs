use super::*;

const FIXTURE: &str = include_str!("../../tests/fixtures/changes.patch");

fn file<'a>(patch: &'a Patch, path: &str) -> &'a FilePatch {
    match patch.files.iter().find(|file| file.path == path) {
        Some(file) => file,
        None => panic!("no file {path} in {:?}", paths(patch)),
    }
}

fn paths(patch: &Patch) -> Vec<&str> {
    patch.files.iter().map(|file| file.path.as_str()).collect()
}

#[test]
fn empty_patch_has_no_files_and_is_complete() {
    let patch = Patch::parse("");
    assert!(patch.files.is_empty());
    assert!(patch.is_complete);
}

#[test]
fn fixture_lists_every_file_in_order() {
    let patch = Patch::parse(FIXTURE);
    assert_eq!(
        paths(&patch),
        vec![
            "src/lib/library/scope.ts",
            "src/lib/library/visible.ts",
            "docs/archive/old.md",
            "src/components/library/ItemPicker.tsx",
            "src/components/library/old-picker.tsx",
            "public/img/cover.png",
            "src/generated/api-contract.json",
            "scripts/build.sh",
            "docs/notes with spaces.md",
            "config/windows.ini",
        ]
    );
    assert!(patch.is_complete);
}

#[test]
fn modified_file_counts_lines_and_numbers_both_sides() {
    let patch = Patch::parse(FIXTURE);
    let scope = file(&patch, "src/lib/library/scope.ts");
    assert_eq!(scope.change, Change::Modified);
    assert_eq!((scope.added, scope.removed), (3, 1));
    assert_eq!(scope.hunks.len(), 2);
    let first = &scope.hunks[0];
    assert_eq!(first.header, "@@ -1,9 +1,9 @@");
    let removed = &first.lines[2];
    assert_eq!(removed.kind, LineKind::Removed);
    assert_eq!((removed.old, removed.new), (Some(3), None));
    let added = &first.lines[3];
    assert_eq!(added.kind, LineKind::Added);
    assert_eq!((added.old, added.new), (None, Some(3)));
    assert_eq!(
        added.text,
        "export const LibraryScope = z.enum([\"mine\", \"team\", \"all\"]);"
    );
    let context = &first.lines[4];
    assert_eq!((context.old, context.new), (Some(4), Some(4)));
}

#[test]
fn hunk_header_keeps_the_scope_git_reports() {
    let patch = Patch::parse(FIXTURE);
    let scope = file(&patch, "src/lib/library/scope.ts");
    assert_eq!(
        scope.hunks[1].header,
        "@@ -12,5 +12,7 @@ export function scopeLabel(scope: LibraryScope): string {"
    );
    assert_eq!(scope.hunks[1].lines[0].old, Some(12));
    assert_eq!(scope.hunks[1].lines[0].new, Some(12));
}

#[test]
fn rename_with_changes_keeps_the_previous_path() {
    let patch = Patch::parse(FIXTURE);
    let renamed = file(&patch, "src/lib/library/visible.ts");
    assert_eq!(renamed.change, Change::Renamed);
    assert_eq!(
        renamed.previous_path.as_deref(),
        Some("src/lib/library/visibility.ts")
    );
    assert_eq!((renamed.added, renamed.removed), (1, 1));
}

#[test]
fn rename_without_changes_has_no_hunks() {
    let patch = Patch::parse(FIXTURE);
    let moved = file(&patch, "docs/archive/old.md");
    assert_eq!(moved.change, Change::Renamed);
    assert_eq!(moved.previous_path.as_deref(), Some("docs/notes/old.md"));
    assert!(moved.hunks.is_empty());
    assert_eq!(moved.body, Body::Text);
}

#[test]
fn new_file_counts_its_lines_and_keeps_the_no_newline_note() {
    let patch = Patch::parse(FIXTURE);
    let new = file(&patch, "src/components/library/ItemPicker.tsx");
    assert_eq!(new.change, Change::New);
    assert_eq!((new.added, new.removed), (14, 0));
    let last = new.hunks[0]
        .lines
        .last()
        .map(|line| (line.text.as_str(), line.has_no_newline));
    assert_eq!(last, Some(("}", true)));
    assert_eq!(new.hunks[0].lines.len(), 14);
}

#[test]
fn deleted_file_takes_its_old_path() {
    let patch = Patch::parse(FIXTURE);
    let deleted = file(&patch, "src/components/library/old-picker.tsx");
    assert_eq!(deleted.change, Change::Deleted);
    assert_eq!((deleted.added, deleted.removed), (0, 6));
    assert_eq!(deleted.hunks[0].lines[5].old, Some(6));
}

#[test]
fn binary_file_is_new_and_binary() {
    let patch = Patch::parse(FIXTURE);
    let binary = file(&patch, "public/img/cover.png");
    assert_eq!(binary.change, Change::New);
    assert_eq!(binary.body, Body::Binary);
    assert!(binary.hunks.is_empty());
}

#[test]
fn omitted_file_keeps_its_size() {
    let patch = Patch::parse(FIXTURE);
    let omitted = file(&patch, "src/generated/api-contract.json");
    assert_eq!(omitted.body, Body::Omitted { kilobytes: 612 });
    assert_eq!(omitted.change, Change::Modified);
}

#[test]
fn mode_change_only_keeps_both_modes() {
    let patch = Patch::parse(FIXTURE);
    let script = file(&patch, "scripts/build.sh");
    assert_eq!(
        script.mode,
        Some(ModeChange {
            old: "100644".into(),
            new: "100755".into()
        })
    );
    assert!(script.hunks.is_empty());
    assert_eq!(script.change, Change::Modified);
}

#[test]
fn path_with_spaces_drops_the_trailing_tab() {
    let patch = Patch::parse(FIXTURE);
    let notes = file(&patch, "docs/notes with spaces.md");
    assert_eq!((notes.added, notes.removed), (1, 0));
}

#[test]
fn binary_path_with_spaces_comes_from_the_git_line() {
    let text = "diff --git a/img/la mia foto.png b/img/la mia foto.png\n\
                index 1..2 100644\n\
                Binary files a/img/la mia foto.png and b/img/la mia foto.png differ\n";
    let patch = Patch::parse(text);
    assert_eq!(paths(&patch), vec!["img/la mia foto.png"]);
    assert_eq!(patch.files[0].body, Body::Binary);
}

#[test]
fn quoted_paths_are_unquoted_with_octal_bytes() {
    let text = "diff --git \"a/src/caf\\303\\251s.ts\" \"b/src/caf\\303\\251s.ts\"\n\
                index 1..2 100644\n\
                --- \"a/src/caf\\303\\251s.ts\"\n\
                +++ \"b/src/caf\\303\\251s.ts\"\n\
                @@ -1 +1 @@\n\
                -a\n\
                +b\n";
    let patch = Patch::parse(text);
    assert_eq!(paths(&patch), vec!["src/cafés.ts"]);
    assert!(patch.is_complete);
}

#[test]
fn quoted_path_with_escapes_and_quotes() {
    let text = "diff --git \"a/x\\\"y\\tz\" \"b/x\\\"y\\tz\"\n\
                new file mode 100644\n\
                index 0..1\n\
                Binary files /dev/null and \"b/x\\\"y\\tz\" differ\n";
    let patch = Patch::parse(text);
    assert_eq!(paths(&patch), vec!["x\"y\tz"]);
}

#[test]
fn crlf_content_is_shown_without_carriage_returns() {
    let patch = Patch::parse(FIXTURE);
    let ini = file(&patch, "config/windows.ini");
    let texts: Vec<&str> = ini.hunks[0].lines.iter().map(|l| l.text.as_str()).collect();
    assert_eq!(texts, vec!["[library]", "scope=mine", "scope=all"]);
}

#[test]
fn whole_patch_in_crlf_parses_like_lf() {
    let patch = Patch::parse(&FIXTURE.replace('\n', "\r\n"));
    assert_eq!(patch.files.len(), 10);
    assert!(patch.is_complete);
    assert_eq!(file(&patch, "src/lib/library/scope.ts").added, 3);
}

#[test]
fn a_twenty_kilobyte_line_is_kept_whole() {
    let long = "x".repeat(20 * 1024);
    let text = format!(
        "diff --git a/min.js b/min.js\n--- a/min.js\n+++ b/min.js\n@@ -1 +1 @@\n-a\n+{long}\n"
    );
    let patch = Patch::parse(&text);
    assert_eq!(patch.files[0].hunks[0].lines[1].text.len(), 20 * 1024);
}

#[test]
fn removed_line_that_looks_like_a_header_stays_in_its_hunk() {
    let text = "diff --git a/a.md b/a.md\n--- a/a.md\n+++ b/a.md\n@@ -1,2 +1,1 @@\n--- not a header\n keep\n";
    let patch = Patch::parse(text);
    let lines = &patch.files[0].hunks[0].lines;
    assert_eq!(lines[0].kind, LineKind::Removed);
    assert_eq!(lines[0].text, "-- not a header");
    assert_eq!(patch.files[0].removed, 1);
    assert!(patch.is_complete);
}

#[test]
fn hunk_counts_without_a_comma_mean_one_line() {
    let text = "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -5 +5 @@\n-x\n+y\n";
    let patch = Patch::parse(text);
    let lines = &patch.files[0].hunks[0].lines;
    assert_eq!((lines[0].old, lines[1].new), (Some(5), Some(5)));
}

#[test]
fn garbage_input_never_panics_and_is_flagged() {
    let inputs = [
        "hello\nworld\n",
        "@@ -1,2 +1,2 @@\n x\n",
        "diff --git\n",
        "diff --git a/x b/x\n@@ -a,b +c,d @@\n+x\n",
        "diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1,3 +1,3 @@\n x\n",
        "\u{0}\u{1b}[31m\n\\ No newline at end of file\n",
        "diff --git \"a/unterminated b/x\n",
        "diff --git a/x b/x\n[patch omitted: lots kB]\n",
        "diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -4294967295,2 +4294967295,2 @@\n x\n x\n",
    ];
    for input in inputs {
        let patch = Patch::parse(input);
        assert!(!patch.is_complete, "{input:?}");
    }
}

#[test]
fn garbage_after_good_files_keeps_what_was_parsed() {
    let text = format!("{FIXTURE}this is not a patch line\n");
    let patch = Patch::parse(&text);
    assert_eq!(patch.files.len(), 10);
    assert!(!patch.is_complete);
}

#[test]
fn truncated_hunk_keeps_its_lines_and_is_flagged() {
    let text = "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1,5 +1,5 @@\n x\n-y\n";
    let patch = Patch::parse(text);
    assert_eq!(patch.files[0].hunks[0].lines.len(), 2);
    assert!(!patch.is_complete);
    assert!(!patch.files[0].is_complete);
}

#[test]
fn output_cut_at_the_limit_marks_the_patch_and_the_cut_file_partial() {
    let text = format!(
        "{FIXTURE}diff --git a/z b/z\n--- a/z\n+++ b/z\n@@ -1,9 +1,9 @@\n x\n+y\n[output truncated at 8 MB]\n"
    );
    let patch = Patch::parse(&text);
    assert!(!patch.is_complete);
    let cut = file(&patch, "z");
    assert!(!cut.is_complete);
    assert_eq!(cut.added, 1);
    let whole = patch.files.iter().filter(|file| file.path != "z");
    assert!(whole.into_iter().all(|file| file.is_complete));
}

#[test]
fn every_file_of_a_whole_patch_is_complete() {
    let patch = Patch::parse(FIXTURE);
    assert!(patch.files.iter().all(|file| file.is_complete));
}

#[test]
fn totals_add_every_file() {
    let patch = Patch::parse(FIXTURE);
    assert_eq!(patch.added(), 3 + 1 + 14 + 1 + 1);
    assert_eq!(patch.removed(), 1 + 1 + 6 + 1);
}

#[test]
fn directory_and_name_split_at_the_last_slash() {
    let patch = Patch::parse(FIXTURE);
    let scope = file(&patch, "src/lib/library/scope.ts");
    assert_eq!(scope.directory(), "src/lib/library/");
    assert_eq!(scope.file_name(), "scope.ts");
    let top = FilePatch::named("README.md");
    assert_eq!((top.directory(), top.file_name()), ("", "README.md"));
}

#[test]
fn patch_written_by_git_itself_reads_completely() {
    let patch = Patch::parse(include_str!("../../tests/fixtures/git-real.patch"));
    assert!(patch.is_complete);
    assert_eq!(
        paths(&patch),
        vec![
            "docs/with spaces.md",
            "gone.txt",
            "img/p.png",
            "moved.txt",
            "new.txt",
            "run.sh",
            "src/a.txt",
            "src/cafés.ts",
            "win.ini",
        ]
    );
    assert_eq!(file(&patch, "gone.txt").change, Change::Deleted);
    assert_eq!(file(&patch, "img/p.png").body, Body::Binary);
    assert_eq!(
        file(&patch, "moved.txt").previous_path.as_deref(),
        Some("mv.txt")
    );
    assert!(file(&patch, "src/a.txt").hunks[0].lines[4].has_no_newline);
    assert_eq!(file(&patch, "win.ini").hunks[0].lines[2].text, "B");
}
