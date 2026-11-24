//! A rig for the lane page and the agents' conversations on GPUI's test platform: the desk over a fake engine
//! that answers `flow sessions` with a list of runs, `flow feed --run` with the page the test wrote and
//! `flow steer --run` with an outcome, and logs every call.

use super::engine_desk::{Desk, DeskData};
use super::engine_rig_tests::{self, Rig};
use crate::prefs::Preferences;
use gpui::{AppContext as _, Entity, TestAppContext, VisualTestContext};
use sakura_core::engine::snapshot::Snapshot;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// How long a test waits for the fake engine's processes; only a hang ends it.
const PATIENCE: Duration = Duration::from_secs(240);

/// The fake engine: the snapshot, the lane's agents, the page of one agent's conversation, one answer to
/// every message written to an agent and the lane's changes. Every call is logged.
const ENGINE: &str = r#"d="$(dirname "$0")"
printf '%s\n' "$*" >> "$d/calls.log"
case "$1 $2" in
  "cockpit snapshot") cat "$d/snapshot.json" ;;
  "flow sessions") cat "$d/agents.json" ;;
  "flow steer") [ -f "$d/steer-delay" ] && sleep "$(cat "$d/steer-delay")"; cat "$d/steer.json" ;;
  "flow feed")
    [ -f "$d/feed-fails" ] && { echo "the engine refused the cursor" >&2; exit 2; }
    run=""
    for a in "$@"; do case "$a" in --run=*) run="${a#--run=}" ;; esac; done
    case "$*" in
      *--since=*) [ -f "$d/feed-$run-since.json" ] && cat "$d/feed-$run-since.json" || cat "$d/page2.json" ;;
      *) [ -f "$d/feed-$run.json" ] && cat "$d/feed-$run.json" || cat "$d/page1.json" ;;
    esac ;;
  "flow changes")
    case "$*" in
      *--file*) cat "$d/patch.json" ;;
      *) cat "$d/changes.json" ;;
    esac ;;
  *) cat "$d/first.json" ;;
esac"#;

/// The agents of the fixture, written from the engine's contract.
pub(super) const AGENTS: &str =
    include_str!("../../../sakura-core/tests/fixtures/engine-agents.json");
/// One agent's conversation, written from the engine's contract.
pub(super) const AGENT_FEED: &str =
    include_str!("../../../sakura-core/tests/fixtures/engine-agent-feed.json");
/// What the engine says to a message that went to a running agent.
pub(super) const STEER_DELIVERED: &str =
    r#"{"outcome": "delivered", "run": "build-1.2", "words": "The builder reads it now."}"#;
pub(super) const CHANGES: &str = r#"{"task": "demo", "base": "a1b2c3d", "gone": false, "added": 308, "removed": 15,
 "files": [{"path": "src/upload/queue.ts", "status": "modified", "added": 86, "removed": 12},
           {"path": "tests/queue.spec.ts", "status": "added", "added": 122, "removed": 0},
           {"path": "logo.png", "status": "added", "added": null, "removed": null}]}"#;
pub(super) const PATCH: &str = r#"{"task": "demo", "path": "src/upload/queue.ts", "truncated": false,
 "patch": "diff --git a/src/upload/queue.ts b/src/upload/queue.ts\n--- a/src/upload/queue.ts\n+++ b/src/upload/queue.ts\n@@ -1,2 +1,3 @@\n export const A = 1\n-const b = 2\n+const b = 3\n+const c = 4\n"}"#;
const DELIVERED: &str = r#"{"result": "done", "exit_code": 0, "output": "delivered: steer for demo went to the running role\n"}"#;

/// The desk over the fake engine, whose lane `demo` lives in `repo`, an existing folder, over `snapshot` and the
/// three `pages` of one agent's conversation.
pub(super) fn open_with<'a>(
    cx: &'a mut TestAppContext,
    repo: &std::path::Path,
    snapshot: &str,
    pages: [&str; 3],
) -> (Rig, Entity<Desk>, String, &'a mut VisualTestContext) {
    open_kept(cx, repo, snapshot, pages, Preferences::default())
}

/// The same with the preferences the last launch kept.
pub(super) fn open_kept<'a>(
    cx: &'a mut TestAppContext,
    repo: &std::path::Path,
    snapshot: &str,
    pages: [&str; 3],
    kept_preferences: Preferences,
) -> (Rig, Entity<Desk>, String, &'a mut VisualTestContext) {
    let snapshot = snapshot.replace("/work/repo", &repo.display().to_string());
    let rig = engine_rig_tests::open(cx, &snapshot, DELIVERED, DELIVERED);
    let script = rig.dir.path().join("sakura");
    sakura_core::test_support::write_script(&script, &format!("#!/bin/sh\n{ENGINE}\n"));
    for (name, text) in [
        ("page1.json", pages[0]),
        ("page2.json", pages[1]),
        ("page3.json", pages[2]),
        ("first.json", DELIVERED),
        ("agents.json", AGENTS),
        ("steer.json", STEER_DELIVERED),
        ("changes.json", CHANGES),
        ("patch.json", PATCH),
    ] {
        std::fs::write(rig.dir.path().join(name), text).expect("a page");
    }
    let read = Snapshot::parse(snapshot.as_bytes()).expect("the snapshot");
    let key = read.items.iter().find(|item| item.task() == "demo");
    let key = key.expect("the demo lane").lane_key();
    cx.update(|cx| kept_preferences.clone().install(None, cx));
    let (facts, source, sessions) = (rig.facts.clone(), rig.source.clone(), rig.sessions.clone());
    let slot = Rc::new(RefCell::new(None));
    let kept = slot.clone();
    let (_, cx) = cx.add_window_view(move |window, cx| {
        let data = DeskData {
            facts,
            source,
            sessions,
        };
        let desk = cx.new(|cx| Desk::new(data, &kept_preferences, window, cx));
        *kept.borrow_mut() = Some(desk.clone());
        Desk::root(desk, window, cx)
    });
    let desk = slot.borrow_mut().take().expect("the desk was built");
    cx.update(|window, _| window.activate_window());
    settle(cx, "the lanes", |cx| {
        rig.facts
            .read_with(cx, |facts, _| facts.snapshot().is_some())
    });
    (rig, desk, key, cx)
}

pub(super) fn settle(
    cx: &mut VisualTestContext,
    what: &str,
    mut is_done: impl FnMut(&mut VisualTestContext) -> bool,
) {
    let limit = Instant::now() + PATIENCE;
    loop {
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        if is_done(cx) {
            return;
        }
        assert!(Instant::now() < limit, "waited for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The calls of the fake engine that start with `prefix`, one line each.
pub(super) fn calls_of(rig: &Rig, prefix: &str) -> Vec<String> {
    let calls = std::fs::read_to_string(rig.dir.path().join("calls.log")).unwrap_or_default();
    calls
        .lines()
        .filter(|line| line.starts_with(prefix))
        .map(str::to_string)
        .collect()
}

pub(super) const EMPTY_PAGE: &str =
    r#"{"entries": [], "cursor": "c2", "live": null, "state": "finished"}"#;

/// A page of one agent's conversation whose entries are tool calls on `paths`, one line each.
pub(super) fn tools_page(paths: &[&str], cursor: &str) -> String {
    let entries: Vec<String> = paths
        .iter()
        .map(|path| {
            format!(
                r#"{{"run": "build-1.2", "role": "builder", "workspace": "/w", "kind": "tool", "text": "", "tool": "Edit", "path": "{path}", "line": 1}}"#
            )
        })
        .collect();
    format!(
        r#"{{"entries": [{}], "cursor": "{cursor}", "live": null, "state": "running"}}"#,
        entries.join(",")
    )
}
