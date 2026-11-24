//! Test support for the engine's window: a fake `sakura` that prints a snapshot from a file and answers an
//! action from two files (the first call, and the call with a confirmation token), logging every call.

use crate::data::engine_facts::EngineFacts;
use crate::data::source::Source;
use crate::theme::{Appearance, Theme};
use gpui::{AppContext as _, Entity, TestAppContext, VisualTestContext};
use sakura_core::engine::cli::EngineCli;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::runtime::Runtime;

/// The snapshot of the engine's own fixture, with two projects and four lanes.
pub(super) const SNAPSHOT: &str =
    include_str!("../../../sakura-core/tests/fixtures/engine-snapshot.json");

/// What `sakura engine rules --json` prints in the rig until a test says another: the engine's own fixture.
pub(super) const PHONE: &str =
    include_str!("../../../sakura-core/tests/fixtures/engine-phone.json");
pub(super) const RULES: &str =
    include_str!("../../../sakura-core/tests/fixtures/engine-rules.json");

/// The key of the lane of the fixture whose id is `task`.
pub(super) fn lane_key(task: &str) -> String {
    let snapshot = sakura_core::engine::snapshot::Snapshot::parse(SNAPSHOT.as_bytes());
    let items = snapshot.expect("the fixture").items;
    let found = items.iter().find(|item| item.task() == task);
    found.expect("the lane").lane_key()
}

/// The fake engine, its folder and the facts read from it.
pub(super) struct Rig {
    pub dir: tempfile::TempDir,
    pub source: Source,
    /// The master's kept sessions, in memory.
    pub sessions: crate::data::sessions::Sessions,
    pub facts: Entity<EngineFacts>,
    _runtime: Arc<Runtime>,
}

impl Rig {
    /// Writes the snapshot the fake engine prints.
    pub fn say_snapshot(&self, text: &str) {
        std::fs::write(self.dir.path().join("snapshot.json"), text).expect("the snapshot");
    }

    /// Writes what the fake engine prints to the first call of an action, from now on.
    pub fn say_first(&self, text: &str) {
        std::fs::write(self.dir.path().join("first.json"), text).expect("the first answer");
    }

    /// Makes every call of an action take `seconds` before it is recorded and answered, from now on.
    pub fn say_delay(&self, seconds: &str) {
        std::fs::write(self.dir.path().join("delay"), seconds).expect("the delay");
    }

    /// Writes what `sakura engine rules --json` prints.
    pub fn say_rules(&self, text: &str) {
        std::fs::write(self.dir.path().join("rules.json"), text).expect("the rules");
    }

    /// Writes what `sakura cockpit phone --json` prints.
    pub fn say_phone(&self, text: &str) {
        std::fs::write(self.dir.path().join("phone.json"), text).expect("the phone setup");
    }

    /// The calls of `sakura cockpit act` so far, one line each.
    pub fn acts(&self) -> Vec<String> {
        let text = std::fs::read_to_string(self.dir.path().join("acts.log")).unwrap_or_default();
        text.lines().map(str::to_string).collect()
    }
}

/// Sets the app up for a window: the kit, the fonts, the theme, motion reduced.
pub(super) fn prepare(cx: &mut TestAppContext) {
    // The fake engine answers from another thread, as the real one does.
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_component::init(cx);
        crate::fonts::register(cx);
        Theme::set(Appearance::Dark, cx);
        cx.set_reduce_motion(true);
    });
    crate::l10n::set_for_test(crate::l10n::Language::English);
}

/// A fake engine that answers an action with `first`, and with `second` once a confirmation token is sent.
pub(super) fn open(cx: &mut TestAppContext, snapshot: &str, first: &str, second: &str) -> Rig {
    let body = r#"d="$(dirname "$0")"
if [ "$2" = snapshot ]; then cat "$d/snapshot.json"; exit 0; fi
if [ "$1 $2" = "engine rules" ]; then cat "$d/rules.json"; exit 0; fi
if [ "$1 $2" = "cockpit phone" ]; then cat "$d/phone.json"; exit 0; fi
case "$1 $2" in
  "scheduled history"|"scheduled conversation")
    printf '%s\n' "$*" >> "$d/scheduled.log"
    cat "$d/$2.json"
    exit $?
    ;;
esac
if [ "$*" = "setup detect --json" ]; then
    printf '%s\n' "$*" >> "$d/acts.log"
    cat "$d/setup-detect.json"
    exit $?
fi
while [ -f "$d/hold-actions" ]; do sleep 0.02; done
if [ -f "$d/delay" ]; then sleep "$(cat "$d/delay")"; fi
printf '%s\n' "$*" >> "$d/acts.log"
case "$*" in *--confirm*) f=second;; *) f=first;; esac
cat "$d/$f.json"
exit "$(cat "$d/exit" 2>/dev/null || echo 0)""#;
    open_with_script(cx, (snapshot, first, second), body)
}

pub(super) fn open_with_script(
    cx: &mut TestAppContext,
    (snapshot, first, second): (&str, &str, &str),
    body: &str,
) -> Rig {
    prepare(cx);
    let dir = tempfile::tempdir().expect("a folder");
    write(dir.path(), "snapshot.json", snapshot);
    write(dir.path(), "first.json", first);
    write(dir.path(), "second.json", second);
    write(dir.path(), "rules.json", RULES);
    write(dir.path(), "phone.json", PHONE);
    let script = dir.path().join("sakura");
    sakura_core::test_support::write_script(&script, &format!("#!/bin/sh\n{body}\n"));
    let runtime = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("a runtime"),
    );
    let cli = EngineCli::at(script).with_deadline(Duration::from_secs(60));
    let source = Source::over(cli, runtime.clone());
    let sessions = crate::data::sessions::Sessions::new(None, runtime.clone());
    let reader = Arc::new(source.clone());
    let (_sender, changes) = tokio::sync::watch::channel(());
    let facts = cx.new(|cx| EngineFacts::new(reader, changes, None, cx));
    Rig {
        dir,
        source,
        sessions,
        facts,
        _runtime: runtime,
    }
}

fn write(dir: &Path, name: &str, text: &str) {
    std::fs::write(dir.join(name), text).expect("a file");
}

/// Lets the window and the fake engine's answers settle until `is_done`, or fails after a minute.
pub(super) fn wait_until(
    cx: &mut VisualTestContext,
    what: &str,
    mut is_done: impl FnMut(&mut VisualTestContext) -> bool,
) {
    let limit = Instant::now() + Duration::from_secs(60);
    loop {
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        if is_done(cx) {
            return;
        }
        assert!(Instant::now() < limit, "waited for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// How many boxes the window drew in the accent: the fuchsia that means "an ask nobody answered", which
/// no title, header, tab, glyph or button wears.
pub(super) fn accent_quads(cx: &mut gpui::VisualTestContext) -> usize {
    let accent: gpui::Background = crate::theme::Palette::dark().attention.into();
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
        window
            .painted_quads()
            .iter()
            .filter(|quad| quad.background == accent)
            .count()
    })
}
