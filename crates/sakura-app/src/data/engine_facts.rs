//! Snapshots follow the watcher, with a slower recovery read for missed events.
//! A failed read keeps the last good snapshot.

use crate::prefs::Preferences;
use gpui::{Context, EventEmitter, Task as GpuiTask};
use sakura_core::engine::project_folders::ProjectFolders;
use sakura_core::engine::snapshot::{Decided, Project, Snapshot, Wait};
use sakura_core::engine::watch::EngineWatcher;
use sakura_core::error::{BridgeError, BridgeResult};
use sakura_core::said::Said;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch::{self, Receiver, Sender};

/// How long changes are gathered into one read.
pub const CHANGE_DEBOUNCE: Duration = Duration::from_millis(300);
pub const AGES_TICK: Duration = Duration::from_secs(15);
pub const RECOVERY_TICK: Duration = Duration::from_secs(120);

/// A call in flight on the runtime.
pub type Pending<T> = Pin<Box<dyn Future<Output = BridgeResult<T>> + Send>>;

/// Where snapshots come from. The window's source reads the real engine; a test scripts the answers.
pub trait EngineRead: Send + Sync {
    /// Reads the engine's snapshot, within its deadline.
    fn snapshot(&self) -> Pending<Snapshot>;
}

/// Why the last read failed: the sentence for the person and what the engine itself wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub said: Said,
    pub details: Option<String>,
}

impl Problem {
    /// The problem in one sentence: what is told, then what the engine itself wrote.
    pub fn text(&self) -> gpui::SharedString {
        crate::l10n::with_details(&crate::l10n::say(&self.said), self.details.as_deref()).into()
    }
}

impl From<BridgeError> for Problem {
    fn from(error: BridgeError) -> Self {
        Self {
            said: error.said,
            details: error.details,
        }
    }
}

/// Emitted when the set of waits changed between two reads, for the notifications.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WaitsChanged;

impl EventEmitter<WaitsChanged> for EngineFacts {}

/// Emitted after every read of the engine, whether or not it changed anything: what depends on files the
/// snapshot does not carry (the rules) looks at them then.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Refreshed;

impl EventEmitter<Refreshed> for EngineFacts {}

/// Emitted once per debounced watcher burst.
pub struct WatchedRefresh;

impl EventEmitter<WatchedRefresh> for EngineFacts {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgesChanged;

impl EventEmitter<AgesChanged> for EngineFacts {}

type DecisionKey = (String, Option<String>);

/// The engine as the window sees it.
pub struct EngineFacts {
    reader: Arc<dyn EngineRead>,
    snapshot: Option<Rc<Snapshot>>,
    decided_waits: HashMap<DecisionKey, Rc<Wait>>,
    problem: Option<Problem>,
    is_reading: bool,
    /// How many reads have ended, whether or not they worked.
    reads_ended: u64,
    /// A change came while a read was under way: one more read follows it.
    is_queued: bool,
    is_debouncing: bool,
    last_age_at: i64,
    projects_watched: Option<Sender<Vec<Project>>>,
    _watcher: Option<EngineWatcher>,
    _tasks: Vec<GpuiTask<()>>,
}

impl EngineFacts {
    /// Keeps the watcher alive for as long as the facts are in use.
    pub fn new(
        reader: Arc<dyn EngineRead>,
        mut changes: Receiver<()>,
        watcher: Option<EngineWatcher>,
        cx: &mut Context<Self>,
    ) -> Self {
        let listener = cx.spawn(async move |this, cx| {
            while changes.changed().await.is_ok() {
                if this.update(cx, |facts, cx| facts.on_change(cx)).is_err() {
                    return;
                }
            }
        });
        let ages = cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(AGES_TICK).await;
            if this
                .update(cx, |facts, cx| {
                    facts.tick_ages(chrono::Utc::now().timestamp(), cx)
                })
                .is_err()
            {
                return;
            }
        });
        let recovery = cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(RECOVERY_TICK).await;
            if this.update(cx, |facts, cx| facts.read(cx)).is_err() {
                return;
            }
        });
        let mut facts = Self {
            reader,
            snapshot: None,
            decided_waits: restore_decided_waits(cx),
            problem: None,
            is_reading: false,
            reads_ended: 0,
            is_queued: false,
            is_debouncing: false,
            last_age_at: chrono::Utc::now().timestamp(),
            projects_watched: None,
            _watcher: watcher,
            _tasks: vec![listener, ages, recovery],
        };
        facts.read(cx);
        facts
    }

    /// Adds project directory events to the snapshot reader.
    pub fn watch_project_folders(&mut self, cx: &mut Context<Self>) {
        let (projects, watch_projects) = watch::channel(Vec::new());
        let (presence, mut changes) = watch::channel(());
        if let Some(snapshot) = self.snapshot() {
            projects.send_replace(snapshot.projects.clone());
        }
        self.projects_watched = Some(projects);
        self._tasks.push(
            cx.background_executor()
                .spawn(monitor_projects(watch_projects, presence)),
        );
        self._tasks.push(cx.spawn(async move |this, cx| {
            while changes.changed().await.is_ok() {
                if this.update(cx, |facts, cx| facts.on_change(cx)).is_err() {
                    return;
                }
            }
        }));
    }

    /// The last snapshot that was read; none before the first one.
    pub fn snapshot(&self) -> Option<&Snapshot> {
        self.snapshot.as_deref()
    }

    /// Keeps the snapshot alive across mutable GPUI calls during rendering.
    pub fn shared_snapshot(&self) -> Option<Rc<Snapshot>> {
        self.snapshot.clone()
    }

    /// The last open version of an ask kept with its decision.
    pub fn decided_wait(&self, decision: &Decided) -> Option<Rc<Wait>> {
        self.decided_waits.get(&decision_key(decision)).cloned()
    }

    /// Why the last read failed; none after a read that worked.
    pub fn problem(&self) -> Option<&Problem> {
        self.problem.as_ref()
    }

    /// How many reads have ended so far: a read requested now is the one numbered
    /// [`Self::reads_ahead`] more than this.
    pub fn reads_ended(&self) -> u64 {
        self.reads_ended
    }

    /// How many reads must end before one requested now has: one, or two when a read is already under way
    /// (it may have started before what the caller changed, so one more follows it).
    pub fn reads_ahead(&self) -> u64 {
        if self.is_reading {
            2
        } else {
            1
        }
    }

    /// Reads now, for what an action just changed. A read already under way is followed by one more.
    pub fn read(&mut self, cx: &mut Context<Self>) {
        if self.is_reading {
            self.is_queued = true;
            return;
        }
        self.is_reading = true;
        let pending = self.reader.snapshot();
        cx.spawn(async move |this, cx| {
            let result = pending.await;
            drop(this.update(cx, |facts, cx| facts.apply(result, cx)));
        })
        .detach();
    }

    /// Changes within [`CHANGE_DEBOUNCE`] are one read.
    fn on_change(&mut self, cx: &mut Context<Self>) {
        if self.is_debouncing {
            return;
        }
        self.is_debouncing = true;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(CHANGE_DEBOUNCE).await;
            drop(this.update(cx, |facts, cx| {
                facts.is_debouncing = false;
                cx.emit(WatchedRefresh);
                facts.read(cx);
            }));
        })
        .detach();
    }

    fn apply(&mut self, result: BridgeResult<Snapshot>, cx: &mut Context<Self>) {
        self.is_reading = false;
        self.reads_ended += 1;
        match result {
            Ok(snapshot) => self.take_snapshot(snapshot, cx),
            Err(error) => self.take_problem(error.into(), cx),
        }
        cx.emit(Refreshed);
        if std::mem::take(&mut self.is_queued) {
            self.read(cx);
        }
    }

    fn tick_ages(&mut self, now: i64, cx: &mut Context<Self>) {
        let before = std::mem::replace(&mut self.last_age_at, now);
        let has_woken = self.snapshot().is_some_and(|snapshot| {
            snapshot
                .active_waits(now)
                .any(|wait| wait.is_snoozed_at(before))
        });
        if has_woken {
            cx.emit(WaitsChanged);
            cx.notify();
        }
        cx.emit(AgesChanged);
    }

    fn take_snapshot(&mut self, snapshot: Snapshot, cx: &mut Context<Self>) {
        if let Some(projects) = &self.projects_watched {
            projects.send_if_modified(|watched| {
                if *watched == snapshot.projects {
                    return false;
                }
                watched.clone_from(&snapshot.projects);
                true
            });
        }
        let was_broken = self.problem.take().is_some();
        let has_changed = self.snapshot.as_deref() != Some(&snapshot);
        let waits_changed = self.snapshot.as_ref().map(|old| &old.waits) != Some(&snapshot.waits);
        if has_changed {
            self.keep_decided_waits(&snapshot, cx);
            self.snapshot = Some(Rc::new(snapshot));
        }
        if waits_changed {
            cx.emit(WaitsChanged);
        }
        if was_broken || has_changed {
            cx.notify();
        }
    }

    fn keep_decided_waits(&mut self, snapshot: &Snapshot, cx: &mut Context<Self>) {
        let Some(decisions) = &snapshot.decided else {
            return;
        };
        let previous = self.snapshot.as_deref();
        if previous.and_then(|old| old.decided.as_ref()) == Some(decisions) {
            return;
        }
        let earlier: HashSet<_> = previous
            .and_then(|old| old.decided.as_ref())
            .into_iter()
            .flatten()
            .map(decision_key)
            .collect();
        let open: HashMap<_, _> = previous
            .into_iter()
            .flat_map(|old| &old.waits)
            .map(|wait| (&wait.id, wait))
            .collect();
        let kept = decisions
            .iter()
            .filter_map(|decision| {
                let key = decision_key(decision);
                let wait = self.decided_waits.get(&key).cloned().or_else(|| {
                    if earlier.contains(&key) {
                        return None;
                    }
                    open.get(&decision.id).map(|wait| Rc::new((*wait).clone()))
                })?;
                Some((key, wait))
            })
            .collect();
        if self.decided_waits != kept {
            self.decided_waits = kept;
            let mut rows: Vec<_> = self.decided_waits.iter().collect();
            rows.sort_unstable_by_key(|(key, _)| *key);
            let rows: Vec<_> = rows
                .into_iter()
                .map(|((_, at), wait)| (at, wait.as_ref()))
                .collect();
            Preferences::keep_decided_waits(cx, serde_json::json!(rows));
        }
    }

    /// The last snapshot stays; the problem is set, and the window told, only when it is another one.
    fn take_problem(&mut self, problem: Problem, cx: &mut Context<Self>) {
        if self.problem.as_ref() != Some(&problem) {
            self.problem = Some(problem);
            cx.notify();
        }
    }
}

fn restore_decided_waits(cx: &gpui::App) -> HashMap<DecisionKey, Rc<Wait>> {
    Preferences::decided_waits(cx)
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|row| {
            let (at, wait): (Option<String>, Wait) = serde_json::from_value(row.clone()).ok()?;
            Some(((wait.id.clone(), at), Rc::new(wait)))
        })
        .collect()
}

fn decision_key(decision: &Decided) -> DecisionKey {
    (decision.id.clone(), decision.at.clone())
}

async fn monitor_projects(mut projects: Receiver<Vec<Project>>, presence: Sender<()>) {
    let (mut folders, mut events) = ProjectFolders::new();
    loop {
        let changed = tokio::select! {
            update = projects.changed() => {
                if update.is_err() {
                    return;
                }
                let next = projects.borrow_and_update().clone();
                folders.replace(&next)
            }
            _ = events.changed() => folders.refresh(),
        };
        if changed {
            presence.send_replace(());
        }
    }
}

#[cfg(test)]
#[path = "engine_facts_tests.rs"]
mod tests;
