use super::*;
use crate::box_cli::test_support::fake_box;
use crate::patch::Patch;

const TASK: &str = "sort-folders";
/// The level `box` documents.
const LEVEL: &str = r#"{ "path": "sub/src",
  "entries": [ { "name": "app.ts", "kind": "file", "changed": true, "added": 2, "removed": 1 },
               { "name": "lib", "kind": "dir", "changed": false, "added": null, "removed": null },
               { "name": "gone.ts", "kind": "deleted", "changed": true, "added": null, "removed": 4 },
               { "name": "pipe", "kind": "fifo" } ],
  "truncated": false }"#;

fn entry(name: &str, kind: EntryKind, changed: bool) -> Entry {
    Entry {
        name: name.into(),
        kind,
        changed,
        added: None,
        removed: None,
    }
}

fn level(path: &str, entries: Vec<Entry>) -> Level {
    Level {
        path: path.into(),
        entries,
        truncated: false,
    }
}

/// A root with a touched folder, an untouched one and two files.
fn root() -> Level {
    level(
        "",
        vec![
            entry("docs", EntryKind::Dir, false),
            entry("src", EntryKind::Dir, true),
            entry("README.md", EntryKind::File, false),
            entry("package.json", EntryKind::File, true),
        ],
    )
}

/// Takes `level` into `tree` as the answer for its own folder.
fn read(tree: &mut Tree, level: Level) {
    let dir = level.path.clone();
    tree.set_level(&dir, level).expect("a level box could give");
}

fn names(nodes: &[Node]) -> Vec<&str> {
    nodes.iter().map(|node| node.entry.name.as_str()).collect()
}

/// A fake `box` that records its arguments and answers with `body`.
fn rig(body: &str) -> (tempfile::TempDir, BoxCli) {
    let dir = tempfile::tempdir().unwrap();
    let script = format!(
        r#"d="$(dirname "$0")"; printf '%s\n' "$*" >> "$d/calls.log"
{body}"#
    );
    let cli = fake_box(dir.path(), &script);
    (dir, cli)
}

fn calls(dir: &tempfile::TempDir) -> Vec<String> {
    let log = std::fs::read_to_string(dir.path().join("calls.log")).unwrap_or_default();
    log.lines().map(str::to_string).collect()
}

#[test]
fn a_level_is_read_as_box_documents_it() {
    let level: Level = serde_json::from_str(LEVEL).unwrap();
    assert_eq!(level.path, "sub/src");
    assert!(!level.truncated);
    let app = &level.entries[0];
    assert_eq!((app.kind, app.changed), (EntryKind::File, true));
    assert_eq!((app.added, app.removed), (Some(2), Some(1)));
    assert_eq!(level.entries[1].kind, EntryKind::Dir);
    assert_eq!(level.entries[2].kind, EntryKind::Deleted);
    let unknown = &level.entries[3];
    assert_eq!(unknown.kind, EntryKind::Other, "a kind added later");
    assert!(!unknown.changed, "left out by box: not changed");
}

#[test]
fn matches_are_read_as_box_documents_them() {
    let text = r#"{ "query": "spec", "matches": ["sub/existing.spec.ts", "sub/spec/a.ts"], "truncated": true }"#;
    let matches: Matches = serde_json::from_str(text).unwrap();
    assert_eq!(matches.matches, ["sub/existing.spec.ts", "sub/spec/a.ts"]);
    assert!(matches.truncated);
}

#[test]
fn a_path_that_leaves_the_lavoro_or_reads_as_a_flag_is_refused() {
    for good in ["", "src", "src/app.ts", "a b/c.d", ".env.example", "a..b"] {
        assert_eq!(relative_path(good), Ok(good));
    }
    for bad in ["/etc/passwd", "../x", "src/../../x", "--all", "-x", "a\0b"] {
        assert!(relative_path(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn a_search_text_is_one_to_two_hundred_characters_and_never_a_flag() {
    assert_eq!(query("spec"), Ok("spec"));
    assert_eq!(query("cafés é"), Ok("cafés é"));
    assert!(query("").is_err());
    assert!(query("--json").is_err());
    assert!(query(&"x".repeat(201)).is_err());
    assert!(query(&"è".repeat(200)).is_ok(), "characters, not bytes");
}

#[test]
fn bytes_are_text_unless_they_hold_a_nul_or_are_not_utf8() {
    assert_eq!(
        FileBody::of(b"let x = 1;\n".to_vec()),
        FileBody::Text("let x = 1;\n".into())
    );
    assert_eq!(FileBody::of(Vec::new()), FileBody::Text(String::new()));
    assert_eq!(FileBody::of(b"PNG\0\x01\x02".to_vec()), FileBody::Binary);
    assert_eq!(FileBody::of(vec![0xff, 0xfe, b'a']), FileBody::Binary);
}

#[cfg_attr(windows, ignore = "Unix shell fixture")]
#[tokio::test]
async fn reading_builds_the_documented_argument_vectors() {
    let (dir, cli) = rig(r#"case "$1" in
  tree) dir=""; if [ "$3" = "--path" ]; then dir="$4"; fi
    printf '{"path":"%s","entries":[],"truncated":false}\n' "$dir" ;;
  search) echo '{"query":"q","matches":["a.ts"],"truncated":false}' ;;
  file) printf 'one\ntwo\n' ;;
esac"#);
    assert!(read_level(&cli, TASK, "").await.unwrap().entries.is_empty());
    read_level(&cli, TASK, "src/app").await.unwrap();
    let found = search(&cli, TASK, "app ts").await.unwrap();
    assert_eq!(found.matches, ["a.ts"]);
    let body = read_file(&cli, TASK, "src/app.ts").await.unwrap();
    assert_eq!(body, FileBody::Text("one\ntwo\n".into()));
    assert_eq!(
        calls(&dir),
        [
            "tree sort-folders --json",
            "tree sort-folders --path src/app --json",
            "search sort-folders app ts --json",
            "file sort-folders src/app.ts",
        ]
    );
}

#[cfg_attr(windows, ignore = "Unix shell fixture")]
#[tokio::test]
async fn bad_arguments_never_reach_box() {
    let (dir, cli) = rig("echo '{}'");
    assert!(read_level(&cli, "Bad Name", "").await.is_err());
    assert!(read_level(&cli, TASK, "../outside").await.is_err());
    assert!(read_file(&cli, TASK, "").await.is_err());
    assert!(read_file(&cli, TASK, "/etc/passwd").await.is_err());
    assert!(search(&cli, TASK, "").await.is_err());
    assert!(calls(&dir).is_empty());
}

#[cfg_attr(windows, ignore = "Unix shell fixture")]
#[tokio::test]
async fn a_file_over_the_limit_is_an_answer_and_a_missing_one_a_failure() {
    let (_dir, cli) = rig(r#"case "$3" in
  big.bin) echo "box: the file is larger than 2 MiB: box file does not print it" >&2; exit 82 ;;
  *) echo "box: no such file: $3" >&2; exit 3 ;;
esac"#);
    assert_eq!(
        read_file(&cli, TASK, "big.bin").await,
        Ok(FileBody::TooLarge)
    );
    let missing = read_file(&cli, TASK, "gone.ts").await.unwrap_err();
    assert_eq!(missing.code, ErrorCode::NotFound);
}

#[test]
fn a_new_tree_asks_for_its_root_and_nothing_else() {
    let mut tree = Tree::new();
    assert_eq!(tree.unread(), [""]);
    assert!(tree.nodes().is_empty());
    read(&mut tree, root());
    assert!(tree.unread().is_empty(), "no folder is open");
    assert_eq!(
        names(&tree.nodes()),
        ["docs", "src", "README.md", "package.json"]
    );
    assert_eq!(tree.nodes()[1].children, Children::Unread);
    assert_eq!(tree.nodes()[2].children, Children::None);
}

#[test]
fn opening_a_folder_asks_for_its_level_once_and_closing_it_keeps_what_was_read() {
    let mut tree = Tree::new();
    read(&mut tree, root());
    tree.set_open("src", true);
    assert_eq!(tree.unread(), ["src"]);
    read(
        &mut tree,
        level(
            "src",
            vec![
                entry("app", EntryKind::Dir, true),
                entry("main.ts", EntryKind::File, true),
            ],
        ),
    );
    assert!(tree.unread().is_empty());
    let src = &tree.nodes()[1];
    assert!(src.is_open);
    let Children::Read { nodes, .. } = &src.children else {
        panic!("the level of src was read");
    };
    assert_eq!(names(nodes), ["app", "main.ts"]);
    assert_eq!(nodes[1].path, "src/main.ts");
    assert!(tree.is_level_read("src") && !tree.is_level_read("src/app"));
    tree.set_open("src", false);
    assert!(!tree.nodes()[1].is_open);
    tree.set_open("src", true);
    assert!(tree.unread().is_empty(), "read once");
}

#[test]
fn a_folder_under_a_closed_one_is_not_read() {
    let mut tree = Tree::new();
    read(&mut tree, root());
    tree.set_open("src/app", true);
    assert!(tree.unread().is_empty(), "src itself is closed and unread");
    tree.set_open("src", true);
    assert_eq!(
        tree.unread(),
        ["src"],
        "first the parent: nothing read ahead"
    );
    read(
        &mut tree,
        level("src", vec![entry("app", EntryKind::Dir, true)]),
    );
    assert_eq!(tree.unread(), ["src/app"]);
}

#[test]
fn revealing_a_file_opens_every_folder_above_it() {
    let mut tree = Tree::new();
    read(&mut tree, root());
    tree.reveal("src/app/main.ts");
    assert_eq!(tree.unread(), ["src"]);
    read(
        &mut tree,
        level("src", vec![entry("app", EntryKind::Dir, false)]),
    );
    assert_eq!(tree.unread(), ["src/app"]);
    tree.reveal("README.md");
    assert_eq!(
        tree.unread(),
        ["src/app"],
        "a file of the root opens nothing"
    );
}

#[test]
fn only_touched_shows_what_differs_with_its_folders_open() {
    let mut tree = Tree::new();
    read(&mut tree, root());
    tree.set_only_touched(true);
    assert!(tree.is_only_touched());
    assert_eq!(names(&tree.nodes()), ["src", "package.json"]);
    assert_eq!(tree.unread(), ["src"], "a touched folder opens by itself");
    read(
        &mut tree,
        level(
            "src",
            vec![
                entry("old", EntryKind::Dir, false),
                entry("app", EntryKind::Dir, true),
                entry("gone.ts", EntryKind::Deleted, true),
            ],
        ),
    );
    assert_eq!(tree.unread(), ["src/app"], "only the touched one");
    tree.set_open("src", false);
    assert!(tree.unread().is_empty(), "the person closed it");
    assert!(!tree.nodes()[0].is_open);
    tree.set_only_touched(false);
    assert_eq!(tree.nodes().len(), 4);
    assert!(
        !tree.nodes()[1].is_open,
        "the switch opened it, not the person"
    );
}

#[test]
fn the_levels_on_screen_are_the_root_and_the_open_folders_that_were_read() {
    let mut tree = Tree::new();
    read(&mut tree, root());
    tree.set_open("src", true);
    read(
        &mut tree,
        level("src", vec![entry("app", EntryKind::Dir, true)]),
    );
    tree.set_open("src/app", true);
    read(&mut tree, level("src/app", Vec::new()));
    assert_eq!(tree.shown(), ["", "src", "src/app"]);
    tree.set_open("src", false);
    assert_eq!(
        tree.shown(),
        [""],
        "what is under a closed folder is not shown"
    );
}

#[test]
fn a_truncated_level_says_so() {
    let mut tree = Tree::new();
    let mut cut = root();
    cut.truncated = true;
    read(&mut tree, cut);
    assert!(tree.is_truncated());
}

fn marks(patch: &str) -> Vec<(MarkKind, u32, u32)> {
    let patch = Patch::parse(patch);
    gutter_marks(&patch.files[0])
        .into_iter()
        .map(|mark| (mark.kind, *mark.lines.start(), *mark.lines.end()))
        .collect()
}

#[test]
fn added_modified_and_removed_lines_are_marked_on_the_file_as_it_is_now() {
    let patch = "diff --git a/a.ts b/a.ts\n--- a/a.ts\n+++ b/a.ts\n@@ -1,8 +1,9 @@\n one\n-two\n+TWO\n+two and a half\n three\n-four\n five\n six\n+seven\n eight\n nine\n";
    assert_eq!(
        marks(patch),
        [
            (MarkKind::Modified, 2, 3),
            (MarkKind::RemovedAbove, 5, 5),
            (MarkKind::Added, 7, 7),
        ]
    );
}

#[test]
fn lines_removed_at_the_end_are_marked_after_the_last_line() {
    let patch =
        "diff --git a/a.ts b/a.ts\n--- a/a.ts\n+++ b/a.ts\n@@ -1,3 +1 @@\n one\n-two\n-three\n";
    assert_eq!(marks(patch), [(MarkKind::RemovedAbove, 2, 2)]);
}

#[test]
fn a_new_file_is_added_from_its_first_line_to_its_last() {
    let patch = "diff --git a/n.ts b/n.ts\nnew file mode 100644\n--- /dev/null\n+++ b/n.ts\n@@ -0,0 +1,3 @@\n+a\n+b\n+c\n";
    assert_eq!(marks(patch), [(MarkKind::Added, 1, 3)]);
}

#[test]
fn a_reference_names_the_place_and_carries_the_text_only_when_short() {
    let one = Reference::new("src/app.ts", 12, 12, "let x = 1;");
    assert_eq!(one.place(), "src/app.ts:12");
    assert_eq!(one.for_composer(), "src/app.ts:12\n```\nlet x = 1;\n```\n");
    let several = Reference::new("src/app.ts", 12, 18, "a\nb\n");
    assert_eq!(several.place(), "src/app.ts:12-18");
    assert_eq!(several.for_composer(), "src/app.ts:12-18\n```\na\nb\n```\n");
    let long = Reference::new("src/app.ts", 1, 400, &"line\n".repeat(400));
    assert_eq!(long.text, None);
    assert_eq!(long.for_composer(), "src/app.ts:1-400\n");
    let blank = Reference::new("src/app.ts", 3, 3, "  \n");
    assert_eq!(blank.text, None, "nothing worth quoting");
}

#[test]
fn a_location_is_a_path_inside_the_lavoro_or_nothing() {
    let inside = |location| workspace_relative("/work/demo", location);
    assert_eq!(inside("/work/demo/src/a.rs"), Some("src/a.rs".into()));
    assert_eq!(inside("/work/demo/./src//a.rs"), Some("src/a.rs".into()));
    assert_eq!(inside("src/a.rs"), Some("src/a.rs".into()));
    assert_eq!(inside("/work/demo-other/a.rs"), None, "a sibling folder");
    assert_eq!(inside("/etc/passwd"), None);
    assert_eq!(inside("/work/demo/../secrets"), None);
    assert_eq!(inside("../x"), None);
    assert_eq!(inside("/work/demo"), None, "the folder itself is no file");
    assert_eq!(
        workspace_relative("/work/demo/", "/work/demo/a.rs"),
        Some("a.rs".into())
    );
}

#[test]
fn a_folder_with_no_name_is_refused_and_never_drawn() {
    let mut tree = Tree::new();
    let nameless = level("", vec![entry("", EntryKind::Dir, false)]);
    assert!(tree.set_level("", nameless).is_err());
    assert!(
        tree.nodes().is_empty(),
        "nothing of an invalid level is shown"
    );
    assert!(!tree.is_read(), "the root is still to read");
}

#[test]
fn a_level_with_a_name_that_is_not_one_component_or_is_there_twice_is_refused() {
    let refused = |entries: Vec<Entry>| Tree::new().set_level("", level("", entries)).is_err();
    for name in ["a/b", "..", ".", "a\0b"] {
        assert!(
            refused(vec![entry(name, EntryKind::File, false)]),
            "{name:?}"
        );
    }
    assert!(refused(vec![entry(
        &"x".repeat(256),
        EntryKind::File,
        false
    )]));
    let twice = vec![
        entry("src", EntryKind::Dir, false),
        entry("src", EntryKind::Dir, true),
    ];
    assert!(
        refused(twice),
        "two entries of one name would be drawn as two subtrees"
    );
    assert!(!refused(vec![entry(".env", EntryKind::File, false)]));
}

#[test]
fn a_level_for_another_folder_than_the_one_asked_is_refused() {
    let mut tree = Tree::new();
    read(&mut tree, root());
    assert!(tree.set_level("src", level("docs", Vec::new())).is_err());
    assert!(tree.set_level("src", level("", Vec::new())).is_err());
    assert!(!tree.is_level_read("src") && !tree.is_level_read("docs"));
    assert_eq!(tree.nodes().len(), 4, "the root is as it was");
}

#[test]
fn the_tree_goes_no_deeper_and_grows_no_larger_than_its_bounds() {
    let deep = vec!["d"; 65].join("/");
    assert!(Tree::new()
        .set_level(&deep, level(&deep, Vec::new()))
        .is_err());
    let many = |count: usize| (0..count).map(|n| entry(&format!("f{n}"), EntryKind::File, false));
    assert!(Tree::new()
        .set_level("", level("", many(2001).collect()))
        .is_err());
    let mut tree = Tree::new();
    for folder in 0..50 {
        let dir = format!("d{folder}");
        tree.set_level(&dir, level(&dir, many(2000).collect()))
            .unwrap();
    }
    let one_more = tree.set_level("last", level("last", many(1).collect()));
    assert!(
        one_more.is_err(),
        "a hundred thousand entries are held already"
    );
    tree.set_level("d0", level("d0", many(2000).collect()))
        .expect("a level read again replaces itself");
}

#[cfg_attr(windows, ignore = "Unix shell fixture")]
#[tokio::test]
async fn a_level_box_answers_with_is_checked_before_it_is_handed_on() {
    let (_dir, cli) =
        rig(r#"echo '{"path":"","entries":[{"name":"","kind":"dir"}],"truncated":false}'"#);
    let refused = read_level(&cli, TASK, "").await.unwrap_err();
    assert_eq!(refused.said.key.0, "files.invalid.level");
    assert!(
        read_level(&cli, TASK, "src").await.is_err(),
        "the answer is for the root"
    );
}

#[test]
fn a_level_that_could_not_be_read_is_not_asked_again_until_the_person_asks() {
    let mut tree = Tree::new();
    tree.set_failed("");
    assert!(
        tree.has_failed("") && tree.unread().is_empty(),
        "the root failed"
    );
    tree.retry("");
    assert_eq!(tree.unread(), [""], "asked again on request");
    read(&mut tree, root());
    assert!(!tree.has_failed(""));
    tree.set_open("src", true);
    tree.set_failed("src");
    assert!(
        tree.unread().is_empty(),
        "the folder is not asked by itself"
    );
    assert_eq!(tree.nodes()[1].children, Children::Failed);
    tree.set_open("src", false);
    tree.set_open("src", true);
    assert_eq!(tree.unread(), ["src"], "opened again by the person");
    tree.set_failed("");
    assert!(!tree.has_failed(""), "a level that was read stays as it is");
}

#[test]
fn a_count_of_lines_no_file_has_makes_the_level_invalid() {
    let counted = |added, removed| {
        let mut file = entry("big.ts", EntryKind::File, true);
        (file.added, file.removed) = (Some(added), Some(removed));
        Tree::new().set_level("", level("", vec![file]))
    };
    assert!(counted(u32::MAX, 1).is_err());
    assert!(counted(1, 1_000_000_001).is_err());
    assert!(
        counted(1_000_000_000, 1_000_000_000).is_ok(),
        "the most a count may be"
    );
}

#[test]
fn lines_removed_by_a_hunk_that_leaves_nothing_are_marked_where_they_stood() {
    let hunk = |header: &str| {
        format!("diff --git a/a.ts b/a.ts\n--- a/a.ts\n+++ b/a.ts\n{header}\n-gone\n-gone too\n")
    };
    assert_eq!(
        marks(&hunk("@@ -20,2 +19,0 @@")),
        [(MarkKind::RemovedAbove, 20, 20)],
        "after line 19 of the file as it is now"
    );
    assert_eq!(
        marks(&hunk("@@ -1,2 +0,0 @@")),
        [(MarkKind::RemovedAbove, 1, 1)],
        "the whole file went"
    );
}
