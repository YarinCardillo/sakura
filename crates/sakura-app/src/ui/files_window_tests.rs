//! Slice B in a test window, on the real chain: a fake `box` that answers `tree`, `file`, `search` and
//! `graph` in the shapes the service documents, and whose `acp` runs the fake agent. What `box` was asked
//! is read back, so a test says which reads an action of the person started and which it did not.

use super::centre::CentreView;
use super::conversation_tests::{conversation, fake_agent, say, wait_for};
use super::desk::Desk;
use super::interaction_tests::{click, draw};
use super::launch_tests::{calls, fake_agentbox, open_live, Names, TASK};
use super::right_pane::PaneView;
use gpui::{point, px, Entity, Modifiers, ScrollDelta, ScrollEvent, TestAppContext};
use gpui::{TouchPhase, VisualTestContext};
use sakura_core::models::BoxStatus;
use std::path::{Path, PathBuf};

/// The lavoro of these tests: in a box, so its agent works in `/work/demo`, where the fake agent's tool
/// calls say they are.
const LAVORO: &str = "demo";
/// Another lavoro of the fixture, for a second tab.
const OTHER: &str = "headings-26";
/// The file the fake agent reads at line 12, and its row in the tree.
const FILE: &str = "src/folders/sort.rs";
const FILE_ROW: &str = "files-row-src/folders/sort.rs";
/// The commit of the lavoro in the graph.
const COMMIT: &str = "c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2";
const BASE_COMMIT: &str = "c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1";

const PATCH: &str = "diff --git a/src/folders/sort.rs b/src/folders/sort.rs
index 1111111..2222222 100644
--- a/src/folders/sort.rs
+++ b/src/folders/sort.rs
@@ -1,3 +1,4 @@
 riga 1
-riga 2
+line two
+line two e mezzo
 riga 3
";

/// The cases of the fake `box` for the read verbs: each answers from a file beside the script.
const READ_VERBS: &str = r#"  tree) path=""; while [ "$#" -gt 0 ]; do case "$1" in --path) path="$2"; shift 2 ;; *) shift ;; esac; done
    cat "$d/tree-$(printf '%s' "${path:-root}" | tr '/' '+').json" ;;
  file) cat "$d/file-$(printf '%s' "$3" | tr '/' '+')" ;;
  search) cat "$d/search.json" ;;
  graph) cat "$d/graph.json" ;;
  diff) cat "$d/patch" ;;"#;

/// How many times `box` was requested a command starting with `start`.
fn asked(dir: &tempfile::TempDir, start: &str) -> usize {
    calls(dir)
        .iter()
        .filter(|call| call.starts_with(start))
        .count()
}

fn entry(name: &str, kind: &str, figures: Option<(u32, u32)>) -> serde_json::Value {
    let (added, removed) = match figures.filter(|_| kind == "file") {
        Some((added, removed)) => (Some(added), Some(removed)),
        None => (None, None),
    };
    serde_json::json!({
        "name": name, "kind": kind, "changed": figures.is_some(), "added": added, "removed": removed
    })
}

fn write(dir: &Path, name: &str, text: impl AsRef<[u8]>) {
    std::fs::write(dir.join(name), text).expect("an answer of the fake box");
}

fn level(path: &str, entries: &[serde_json::Value]) -> String {
    serde_json::json!({ "path": path, "entries": entries, "truncated": false }).to_string()
}

/// The tree, the file, the search, the graph and the patch the fake `box` answers with.
fn write_files(dir: &Path) {
    let touched = Some((2, 1));
    write(
        dir,
        "tree-root.json",
        level(
            "",
            &[
                entry("src", "dir", touched),
                entry("README.md", "file", None),
            ],
        ),
    );
    write(
        dir,
        "tree-src.json",
        level("src", &[entry("folders", "dir", touched)]),
    );
    let files = [
        entry("sort.rs", "file", touched),
        entry("old.rs", "file", None),
    ];
    write(dir, "tree-src+folders.json", level("src/folders", &files));
    let lines: Vec<String> = (1..=40).map(|n| format!("line {n}")).collect();
    write(dir, "file-src+folders+sort.rs", lines.join("\n"));
    write(dir, "file-README.md", "# Demo\n");
    let found = serde_json::json!({ "query": "ord", "matches": [FILE], "truncated": false });
    write(dir, "search.json", found.to_string());
    write(dir, "patch", PATCH);
    let graph = serde_json::json!({
        "project": "example-app", "base": "main", "limit": 100,
        "lanes": [
            { "lavoro": null, "branch": "main", "where": null, "head": BASE_COMMIT, "base": null, "readable": true },
            { "lavoro": LAVORO, "branch": "sakura/demo", "where": "box", "head": COMMIT, "base": BASE_COMMIT, "readable": true }
        ],
        "commits": [
            { "sha": COMMIT, "parents": [BASE_COMMIT], "subject": "Sort the folders", "time": "2026-11-10T16:00:00Z", "lavoro": LAVORO, "branches": ["sakura/demo"] },
            { "sha": BASE_COMMIT, "parents": [], "subject": "Inizio", "time": "2026-11-09T16:00:00Z", "lavoro": null, "branches": ["main"] }
        ]
    });
    write(dir, "graph.json", graph.to_string());
}

/// Gives the fixture's lavoro [`TASK`] the name [`LAVORO`] in everything the fake `box` answers.
fn rename_the_lavoro(dir: &Path) {
    let text = std::fs::read_to_string(dir.join("status.json")).expect("the status");
    let mut status: BoxStatus = serde_json::from_str(&text).expect("the status parses");
    let task = status.tasks.iter_mut().find(|task| task.name == TASK);
    let task = task.expect("the fixture's lavoro");
    task.name = LAVORO.to_string();
    write(
        dir,
        &format!("facts-{LAVORO}.json"),
        serde_json::to_string(task).expect("the facts"),
    );
    let names: Vec<serde_json::Value> = status
        .tasks
        .iter()
        .map(|task| serde_json::json!({ "name": task.name, "profile": task.profile, "base": task.base, "taken": null }))
        .collect();
    write(
        dir,
        "tasks.json",
        serde_json::Value::from(names).to_string(),
    );
    write(
        dir,
        "status.json",
        serde_json::to_string(&status).expect("the status"),
    );
}

/// A `box` with the read verbs, one signed-in account and the fake agent, and the desk on [`LAVORO`].
fn open(cx: &mut TestAppContext) -> (&mut VisualTestContext, Entity<Desk>, tempfile::TempDir) {
    let agent = fake_agent();
    let cases = format!(
        "  accounts) echo '[{{\"name\":\"personal\",\"loggedIn\":true}}]' ;;\n  acp) exec \"{}\" --delay-ms 2 ;;\n  shell|attach|login) exec cat ;;\n{READ_VERBS}",
        agent.display()
    );
    let (dir, script): (tempfile::TempDir, PathBuf) = fake_agentbox(Names::Listed, "0", &cases);
    write_files(dir.path());
    rename_the_lavoro(dir.path());
    let (cx, desk, _) = open_live(cx, script, Some(LAVORO), true);
    wait_for(cx, "the conversation", |cx| {
        cx.debug_bounds("thread-composer").is_some()
    });
    (cx, desk, dir)
}

fn is_on_screen(cx: &mut VisualTestContext, selector: &'static str) -> bool {
    cx.debug_bounds(selector).is_some()
}

fn pane_view(cx: &mut VisualTestContext, desk: &Entity<Desk>) -> PaneView {
    desk.read_with(cx, |desk, _| desk.pane_view(LAVORO))
}

/// The path of the file open in the lavoro's "Files" view.
fn open_file(cx: &mut VisualTestContext, desk: &Entity<Desk>) -> Option<String> {
    desk.read_with(cx, |desk, cx| {
        let pane = &desk.files.get(LAVORO)?.pane;
        pane.read(cx).open_path(cx).map(str::to_string)
    })
}

fn is_following(cx: &mut VisualTestContext, desk: &Entity<Desk>) -> Option<bool> {
    desk.read_with(cx, |desk, cx| desk.follow_item(cx))
}

/// Shows "Files" and opens the folders down to [`FILE`].
fn open_the_tree(cx: &mut VisualTestContext) {
    click(cx, "pane-view-files");
    wait_for(cx, "the root", |cx| is_on_screen(cx, "files-row-src"));
    click(cx, "files-row-src");
    wait_for(cx, "the folder", |cx| {
        is_on_screen(cx, "files-row-src/folders")
    });
    click(cx, "files-row-src/folders");
    wait_for(cx, "the files", |cx| is_on_screen(cx, FILE_ROW));
}

/// Opens [`FILE`] from the tree.
fn open_the_file(cx: &mut VisualTestContext, desk: &Entity<Desk>) {
    open_the_tree(cx);
    click(cx, FILE_ROW);
    wait_for(cx, "the file", |cx| {
        open_file(cx, desk).as_deref() == Some(FILE) && is_on_screen(cx, "files-open-name")
    });
}

/// Lets what was started end: every call of `box` answered and no new one for a moment.
fn settle(cx: &mut VisualTestContext, dir: &tempfile::TempDir) {
    super::launch_tests::settled_calls(cx, dir);
    draw(cx);
}

#[gpui::test]
fn looking_reads_no_file_and_the_tree_is_read_one_level_at_a_time(cx: &mut TestAppContext) {
    let (cx, _desk, dir) = open(cx);
    settle(cx, &dir);
    for verb in ["tree", "file", "search", "graph"] {
        assert_eq!(asked(&dir, verb), 0, "{verb} before any view requested it");
    }
    click(cx, "pane-view-files");
    wait_for(cx, "the root", |cx| is_on_screen(cx, "files-row-src"));
    settle(cx, &dir);
    assert_eq!(asked(&dir, "tree"), 1, "the root and nothing under it");
    assert!(is_on_screen(cx, "files-row-README.md"));
    click(cx, "files-row-src");
    wait_for(cx, "the folder", |cx| {
        is_on_screen(cx, "files-row-src/folders")
    });
    settle(cx, &dir);
    assert_eq!(asked(&dir, "tree demo --path src --json"), 1);
    assert_eq!(asked(&dir, "tree"), 2, "one level for one folder opened");
    click(cx, "files-only-touched");
    wait_for(cx, "only what differs from the base", |cx| {
        !is_on_screen(cx, "files-row-README.md") && is_on_screen(cx, FILE_ROW)
    });
    assert!(!is_on_screen(cx, "files-row-src/folders/old.rs"));
    settle(cx, &dir);
    let at_rest = calls(&dir).len();
    settle(cx, &dir);
    assert_eq!(calls(&dir).len(), at_rest, "nothing is read at rest");
}

#[gpui::test]
fn a_file_has_its_gutter_marks_and_is_read_again_once_when_the_lavoro_changes(
    cx: &mut TestAppContext,
) {
    let (cx, desk, dir) = open(cx);
    open_the_file(cx, &desk);
    let marks = |cx: &mut VisualTestContext| {
        desk.read_with(cx, |desk, cx| {
            let pane = desk.files.get(LAVORO).expect("the files").pane.read(cx);
            pane.file_view().read(cx).mark_count(cx)
        })
    };
    wait_for(cx, "the marks of the changed lines", |cx| marks(cx) > 0);
    settle(cx, &dir);
    assert_eq!(asked(&dir, "file demo"), 1);
    // The lavoro's files changed: the watcher says so, its facts are read, and the open file with them.
    let facts = desk.read_with(cx, |desk, _| desk.facts.clone());
    facts.update(cx, |facts, cx| facts.refresh_task(LAVORO.into(), cx));
    wait_for(cx, "the file to be read again", |_| {
        asked(&dir, "file demo") == 2
    });
    settle(cx, &dir);
    assert_eq!(asked(&dir, "file demo"), 2, "once for one change");
    // While another view is shown a change reads nothing; it is read when "Files" comes back.
    click(cx, "pane-view-checks");
    facts.update(cx, |facts, cx| facts.refresh_task(LAVORO.into(), cx));
    settle(cx, &dir);
    assert_eq!(asked(&dir, "file demo"), 2, "not while the view is away");
    click(cx, "pane-view-files");
    wait_for(cx, "the file to be read on the way back", |_| {
        asked(&dir, "file demo") == 3
    });
}

#[gpui::test]
fn the_pane_follows_the_agent_s_tool_call_and_stops_when_the_person_scrolls(
    cx: &mut TestAppContext,
) {
    let (cx, desk, dir) = open(cx);
    assert_eq!(pane_view(cx, &desk), PaneView::Modifiche);
    assert_eq!(
        is_following(cx, &desk),
        Some(true),
        "it follows until the person takes it"
    );
    say(cx, &desk, LAVORO, "tools");
    wait_for(cx, "the file the agent reads", |cx| {
        open_file(cx, &desk).as_deref() == Some(FILE)
    });
    assert_eq!(
        pane_view(cx, &desk),
        PaneView::Files,
        "the pane went to the file"
    );
    wait_for(cx, "the file on screen", |cx| {
        is_on_screen(cx, "files-file-area")
    });
    assert_eq!(is_following(cx, &desk), Some(true));
    let over_the_file = cx
        .debug_bounds("files-file-area")
        .expect("the file's area")
        .center();
    cx.simulate_event(ScrollEvent {
        position: over_the_file,
        delta: ScrollDelta::Pixels(point(px(0.), px(-40.))),
        modifiers: Modifiers::none(),
        touch_phase: TouchPhase::Moved,
        ..Default::default()
    });
    wait_for(cx, "following to stop", |cx| {
        is_following(cx, &desk) == Some(false)
    });
    // The status bar's control follows again, at the agent's last place.
    click(cx, "status-follow");
    wait_for(cx, "following again", |cx| {
        is_following(cx, &desk) == Some(true)
    });
    // Another view takes the pane from the agent.
    click(cx, "pane-view-changes");
    wait_for(cx, "following to stop for another view", |cx| {
        is_following(cx, &desk) == Some(false)
    });
    assert_eq!(pane_view(cx, &desk), PaneView::Modifiche);
    drop(dir);
}

#[gpui::test]
fn cmd_p_finds_a_file_by_name_and_opens_it_in_files(cx: &mut TestAppContext) {
    let (cx, desk, dir) = open(cx);
    cx.simulate_keystrokes("cmd-p");
    wait_for(cx, "quick open", |cx| is_on_screen(cx, "quick-open-panel"));
    cx.simulate_input("ord");
    wait_for(cx, "the file found", |cx| {
        is_on_screen(cx, "quick-open-file-src/folders/sort.rs")
    });
    assert_eq!(asked(&dir, "search demo ord --json"), 1);
    cx.simulate_keystrokes("enter");
    wait_for(cx, "the file open", |cx| {
        open_file(cx, &desk).as_deref() == Some(FILE)
    });
    assert_eq!(pane_view(cx, &desk), PaneView::Files);
    assert!(!is_on_screen(cx, "quick-open-panel"), "quick open went");
    assert_eq!(
        is_following(cx, &desk),
        Some(false),
        "the person picked the file"
    );
}

#[gpui::test]
fn ask_the_agent_puts_the_selected_lines_in_the_composer_and_sends_nothing(
    cx: &mut TestAppContext,
) {
    let (cx, desk, dir) = open(cx);
    open_the_file(cx, &desk);
    let editor = desk.read_with(cx, |desk, cx| {
        let pane = desk.files.get(LAVORO).expect("the files").pane.read(cx);
        pane.file_view().read(cx).editor()
    });
    wait_for(cx, "the file's text", |cx| {
        editor.read_with(cx, |editor, _| editor.value().len() > 20)
    });
    // The second and the third line.
    cx.update(|_, cx| editor.update(cx, |editor, cx| editor.set_selected_range(7..20, cx)));
    draw(cx);
    wait_for(cx, "the control over the selection", |cx| {
        is_on_screen(cx, "files-ask-agent")
    });
    click(cx, "files-ask-agent");
    let conversation = conversation(cx, &desk, LAVORO);
    let draft = conversation.read_with(cx, |conversation, cx| conversation.draft(cx));
    assert!(draft.contains(FILE), "the file is named: {draft}");
    assert!(
        draft.contains("2") && draft.contains("3"),
        "with its lines: {draft}"
    );
    let entries =
        conversation.read_with(cx, |conversation, _| conversation.thread().entries().len());
    assert_eq!(entries, 0, "nothing was sent");
    assert_eq!(asked(&dir, "acp"), 0, "and no agent was started");
    let centre = desk.read_with(cx, |desk, cx| desk.centre_view(LAVORO, cx));
    assert_eq!(
        centre,
        CentreView::Agent,
        "the conversation is what the centre shows"
    );
}

#[gpui::test]
fn a_commit_pressed_in_git_shows_its_diff_in_changes(cx: &mut TestAppContext) {
    let (cx, desk, dir) = open(cx);
    click(cx, "pane-view-git");
    let row: &'static str = Box::leak(format!("graph-row-{COMMIT}").into_boxed_str());
    wait_for(cx, "the commits", |cx| is_on_screen(cx, row));
    assert_eq!(asked(&dir, "graph example-app --json"), 1);
    // A commit of the base branch is not a task's: `box` gives no diff of it, a press does nothing.
    let base: &'static str = Box::leak(format!("graph-row-{BASE_COMMIT}").into_boxed_str());
    click(cx, base);
    settle(cx, &dir);
    assert_eq!(asked(&dir, "diff demo --commit"), 0, "no diff is asked");
    assert_eq!(pane_view(cx, &desk), PaneView::Git, "and the view stays");
    click(cx, row);
    wait_for(cx, "the commit's diff to be asked", |_| {
        calls(&dir)
            .iter()
            .any(|call| call.starts_with("diff demo") && call.contains(COMMIT))
    });
    assert_eq!(pane_view(cx, &desk), PaneView::Modifiche);
    settle(cx, &dir);
    assert_eq!(
        asked(&dir, "graph"),
        1,
        "the graph is not read again for a press"
    );
}

#[gpui::test]
fn what_a_tab_shows_of_its_files_is_there_when_the_tab_comes_back(cx: &mut TestAppContext) {
    let (cx, desk, dir) = open(cx);
    open_the_file(cx, &desk);
    assert_eq!(
        is_following(cx, &desk),
        Some(false),
        "the person opened the file"
    );
    settle(cx, &dir);
    let reads = calls(&dir).len();
    cx.update(|_, cx| desk.update(cx, |desk, cx| desk.open_behind(OTHER.into(), cx)));
    cx.update(|_, cx| desk.update(cx, |desk, cx| desk.select(OTHER.into(), cx)));
    wait_for(cx, "the other lavoro", |cx| {
        desk.read_with(cx, |desk, cx| desk.shown(cx).as_deref() == Some(OTHER))
    });
    assert_eq!(
        is_following(cx, &desk),
        Some(true),
        "the other lavoro has its own"
    );
    cx.update(|_, cx| desk.update(cx, |desk, cx| desk.select(LAVORO.into(), cx)));
    wait_for(cx, "the lavoro back", |cx| {
        desk.read_with(cx, |desk, cx| desk.shown(cx).as_deref() == Some(LAVORO))
    });
    draw(cx);
    assert_eq!(pane_view(cx, &desk), PaneView::Files);
    assert_eq!(open_file(cx, &desk).as_deref(), Some(FILE));
    assert_eq!(is_following(cx, &desk), Some(false));
    assert!(is_on_screen(cx, FILE_ROW), "the tree is as it was left");
    settle(cx, &dir);
    let tree_and_file = |all: &[String]| {
        all.iter()
            .filter(|call| call.starts_with("tree") || call.starts_with("file"))
            .count()
    };
    let all = calls(&dir);
    assert_eq!(
        tree_and_file(&all),
        tree_and_file(&all[..reads]),
        "coming back reads no file again"
    );
}
