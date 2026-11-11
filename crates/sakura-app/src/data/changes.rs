//! The changed files of the task on screen: its patch (all changes, or one commit) and its commits ahead,
//! read off the UI thread through the source. The patch is read again on every new read of the task's facts,
//! never more often than the last read took; the previous patch stays on screen while the next one loads, and
//! a failed read keeps it with the error and its age. Only the task on screen is read: every `box diff`
//! starts a container.

use super::facts::{Facts, StatusRead};
use super::retain_tasks;
use super::source::Source;
use gpui::{Context, Entity, Subscription};
use sakura_core::model;
use sakura_core::models::Commit;
use sakura_core::patch::Patch;
use sakura_core::said::Said;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The shortest gap between two reads of one task's changes, however fast the last one was.
pub const MIN_REFETCH_GAP: Duration = Duration::from_secs(2);

/// Which changes are shown: everything against the base, or one commit ahead.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Scope {
    #[default]
    All,
    Commit(String),
}

/// What is known of one task's changes.
#[derive(Default)]
pub struct TaskChanges {
    patch: Option<Arc<Patch>>,
    fetched_at: Option<Instant>,
    error: Option<Said>,
    is_loading: bool,
    commits: Option<Arc<Vec<Commit>>>,
    scope: Scope,
    /// When the facts the last read followed last said the workspace may have changed: a later word of a
    /// change makes the patch out of date, even when every count is the same (an edit can keep every
    /// figure).
    facts_read_at: Option<Instant>,
    last_started: Option<Instant>,
    last_took: Duration,
    is_stale: bool,
    is_scheduled: bool,
    generation: u64,
}

impl TaskChanges {
    /// The last patch read, kept while a new read runs or after one failed.
    pub fn patch(&self) -> Option<&Arc<Patch>> {
        self.patch.as_ref()
    }

    /// The commits ahead of the base, newest first, once read.
    pub fn commits(&self) -> Option<&Arc<Vec<Commit>>> {
        self.commits.as_ref()
    }

    /// Which changes are requested.
    pub fn scope(&self) -> &Scope {
        &self.scope
    }

    /// When the last read failed: one sentence with the error and the age of the patch still shown.
    pub fn failure(&self, now: Instant) -> Option<Said> {
        let error = self.error.clone()?;
        let age = self
            .fetched_at
            .and_then(|at| model::facts_age(now.saturating_duration_since(at).as_secs()));
        Some(match (&self.patch, age) {
            (Some(_), Some(age)) => Said::new("changes.failure.stale")
                .said("age", age)
                .said("error", error),
            _ => Said::new("changes.failure").said("error", error),
        })
    }
}

/// How long to wait before reading again: never sooner than the last read took, nor than
/// [`MIN_REFETCH_GAP`], counted from when it started.
pub fn refetch_delay(last_started: Option<Instant>, last_took: Duration, now: Instant) -> Duration {
    let Some(started) = last_started else {
        return Duration::ZERO;
    };
    let gap = last_took.max(MIN_REFETCH_GAP);
    gap.saturating_sub(now.saturating_duration_since(started))
}

/// The changes of every task looked at in this window.
pub struct Changes {
    source: Source,
    facts: Entity<Facts>,
    tasks: HashMap<String, TaskChanges>,
    watched: Option<String>,
    _observer: Subscription,
    _status_read: Subscription,
}

impl Changes {
    /// Nothing read yet; reads follow [`Changes::watch`] and the facts.
    pub fn new(source: Source, facts: Entity<Facts>, cx: &mut Context<Self>) -> Self {
        let observer = cx.observe(&facts, |changes, _, cx| changes.on_facts(cx));
        let status_read = cx.subscribe(&facts, |changes, facts, _: &StatusRead, cx| {
            let facts = facts.read(cx);
            retain_tasks(&mut changes.tasks, String::as_str, |task| {
                facts.has_task(task)
            });
        });
        Self {
            source,
            facts,
            tasks: HashMap::new(),
            watched: None,
            _observer: observer,
            _status_read: status_read,
        }
    }

    /// What is known of `task`'s changes.
    pub fn task(&self, task: &str) -> Option<&TaskChanges> {
        self.tasks.get(task)
    }

    /// The task whose changes are on screen; it is read now if it never was or its facts moved on.
    pub fn watch(&mut self, task: Option<String>, cx: &mut Context<Self>) {
        if self.watched == task {
            return;
        }
        self.watched = task;
        self.on_facts(cx);
    }

    /// Shows all changes or one commit of `task`, read at once.
    pub fn set_scope(&mut self, task: &str, scope: Scope, cx: &mut Context<Self>) {
        let entry = self.tasks.entry(task.to_string()).or_default();
        if entry.scope == scope {
            return;
        }
        entry.scope = scope;
        entry.generation += 1;
        entry.is_loading = false;
        self.fetch(task.to_string(), cx);
    }

    fn on_facts(&mut self, cx: &mut Context<Self>) {
        let Some(name) = self.watched.clone() else {
            return;
        };
        let Some(facts_read_at) = self.facts_read_at(&name, cx) else {
            return;
        };
        let entry = self.tasks.entry(name.clone()).or_default();
        if entry.needs_read(facts_read_at) {
            self.schedule(name, cx);
        }
    }

    /// When the facts of `name` last said its workspace may have changed; nothing while only its name is
    /// known (a `box diff` starts a container: none is spent before the facts are read) or its workspace
    /// cannot be read.
    fn facts_read_at(&self, name: &str, cx: &Context<Self>) -> Option<Instant> {
        let row = self.facts.read(cx).row(name)?;
        (row.task.readable && !row.is_pending).then_some(row.work_read_at)
    }
}

impl Changes {
    /// Reads `name` as soon as the gap allows; a read already running is followed by one more.
    fn schedule(&mut self, name: String, cx: &mut Context<Self>) {
        let entry = self.tasks.entry(name.clone()).or_default();
        if entry.is_loading {
            entry.is_stale = true;
            return;
        }
        if entry.is_scheduled {
            return;
        }
        let delay = refetch_delay(entry.last_started, entry.last_took, Instant::now());
        if delay.is_zero() {
            return self.fetch(name, cx);
        }
        entry.is_scheduled = true;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            drop(this.update(cx, |changes, cx| {
                if let Some(entry) = changes.tasks.get_mut(&name) {
                    entry.is_scheduled = false;
                }
                changes.fetch(name, cx);
            }));
        })
        .detach();
    }

    fn fetch(&mut self, name: String, cx: &mut Context<Self>) {
        let facts_read_at = self.facts_read_at(&name, cx);
        let entry = self.tasks.entry(name.clone()).or_default();
        entry.facts_read_at = facts_read_at;
        entry.is_loading = true;
        entry.is_stale = false;
        let started = Instant::now();
        entry.last_started = Some(started);
        let generation = entry.generation;
        let commit = match &entry.scope {
            Scope::All => None,
            Scope::Commit(sha) => Some(sha.clone()),
        };
        let patch = self.source.patch(name.clone(), commit);
        let commits = self.source.commits(name.clone());
        cx.spawn(async move |this, cx| {
            let (patch, commits) = (patch.await, commits.await);
            drop(this.update(cx, |changes, cx| {
                let read = Read {
                    generation,
                    started,
                    patch,
                    commits,
                };
                changes.apply(&name, read, cx);
            }));
        })
        .detach();
        cx.notify();
    }

    fn apply(&mut self, name: &str, read: Read, cx: &mut Context<Self>) {
        let Some(entry) = self.tasks.get_mut(name) else {
            return;
        };
        if read.generation != entry.generation {
            return;
        }
        entry.is_loading = false;
        entry.last_took = read.started.elapsed();
        match read.patch {
            Ok(patch) => entry.keep_patch(patch),
            Err(error) => entry.error = Some(error.said),
        }
        if let Ok(commits) = read.commits {
            entry.commits = Some(Arc::new(commits));
        }
        if entry.is_stale {
            self.schedule(name.to_string(), cx);
        }
        cx.notify();
    }
}

/// One answered read.
struct Read {
    generation: u64,
    started: Instant,
    patch: Result<Patch, sakura_core::error::BridgeError>,
    commits: Result<Vec<Commit>, sakura_core::error::BridgeError>,
}

impl TaskChanges {
    /// Whether the task's facts read at `facts_read_at` call for a new read: nothing was read yet, or these
    /// facts are newer than the ones the last read followed.
    fn needs_read(&self, facts_read_at: Instant) -> bool {
        let is_unread = self.patch.is_none() && self.error.is_none() && !self.is_loading;
        is_unread || self.facts_read_at != Some(facts_read_at)
    }

    /// Takes a fresh patch; an identical one keeps the shared copy so the view has nothing to rebuild.
    fn keep_patch(&mut self, patch: Patch) {
        if self.patch.as_deref() != Some(&patch) {
            self.patch = Some(Arc::new(patch));
        }
        self.fetched_at = Some(Instant::now());
        self.error = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_read_starts_at_once() {
        assert_eq!(
            refetch_delay(None, Duration::ZERO, Instant::now()),
            Duration::ZERO
        );
    }

    #[test]
    fn a_fast_read_still_waits_the_minimum_gap() {
        let now = Instant::now();
        let started = now - Duration::from_millis(500);
        let delay = refetch_delay(Some(started), Duration::from_millis(100), now);
        assert_eq!(delay, MIN_REFETCH_GAP - Duration::from_millis(500));
    }

    #[test]
    fn a_slow_read_waits_as_long_as_it_took() {
        let now = Instant::now();
        let started = now - Duration::from_secs(3);
        let delay = refetch_delay(Some(started), Duration::from_secs(10), now);
        assert_eq!(delay, Duration::from_secs(7));
    }

    #[test]
    fn once_the_gap_has_passed_the_read_starts_at_once() {
        let now = Instant::now();
        let started = now - Duration::from_secs(30);
        assert_eq!(
            refetch_delay(Some(started), Duration::from_secs(10), now),
            Duration::ZERO
        );
    }

    #[test]
    fn newer_facts_with_every_count_unchanged_still_call_for_a_new_read() {
        let first = Instant::now();
        let mut entry = TaskChanges {
            facts_read_at: Some(first),
            ..TaskChanges::default()
        };
        entry.keep_patch(Patch::default());
        assert!(!entry.needs_read(first));
        assert!(entry.needs_read(first + Duration::from_millis(1)));
    }

    #[test]
    fn a_task_never_read_calls_for_a_read() {
        assert!(TaskChanges::default().needs_read(Instant::now()));
    }

    #[test]
    fn an_identical_patch_keeps_the_shared_copy() {
        let mut entry = TaskChanges::default();
        let text = include_str!("../../../sakura-core/tests/fixtures/changes.patch");
        entry.keep_patch(Patch::parse(text));
        let first = entry.patch().cloned();
        entry.keep_patch(Patch::parse(text));
        assert!(Arc::ptr_eq(&first.unwrap(), entry.patch().unwrap()));
    }

    #[test]
    fn a_failure_keeps_the_patch_and_says_how_old_it_is() {
        let mut entry = TaskChanges::default();
        entry.keep_patch(Patch::default());
        let error = Said::new("error.docker.down");
        entry.error = Some(error.clone());
        let later = Instant::now() + Duration::from_secs(180);
        let age = Said::new("age.minutes").number("n", 3);
        assert_eq!(
            entry.failure(later),
            Some(
                Said::new("changes.failure.stale")
                    .said("age", age)
                    .said("error", error)
            )
        );
        assert!(entry.patch().is_some());
    }

    #[test]
    fn a_failure_without_a_patch_says_only_what_failed() {
        let error = Said::new("error.box.missing");
        let entry = TaskChanges {
            error: Some(error.clone()),
            ..TaskChanges::default()
        };
        assert_eq!(
            entry.failure(Instant::now()),
            Some(Said::new("changes.failure").said("error", error))
        );
    }
}
