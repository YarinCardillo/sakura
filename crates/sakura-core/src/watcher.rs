//! Change detection without polling containers: a file watcher on `$AGENTBOX_HOME` (work, results, queue,
//! journal.jsonl) plus a slow tick that only says time passed. Only paths are observed: no file is opened, symlinks are not followed,
//! and the window refetches what it needs when told something changed.

use crate::validate;
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher as _};
use std::collections::BTreeSet;
use std::mem;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, Instant};

/// Debounce window for file events.
pub const DEBOUNCE: Duration = Duration::from_secs(1);
/// Interval of the container-state tick.
pub const TICK: Duration = Duration::from_secs(5);
const TASK_FOLDERS: [&str; 2] = ["work", "results"];
const OTHER_WATCHED: [&str; 2] = ["queue", "journal.jsonl"];
/// The runner's heartbeat, `results/<task>/.runner-alive`, rewritten every 5 s (through a `.tmp` file) while
/// the runner lives. It says nothing about a task's facts; watching it re-read every task every 5 s.
const RUNNER_HEARTBEAT: &str = ".runner-alive";

/// Receives `facts-changed` notifications: `Some(task)` for one task, `None` for "anything".
pub type FactsSink = Arc<dyn Fn(Option<String>) + Send + Sync>;

/// Called on every tick: time passed, and nothing is known to have changed.
pub type TickSink = Arc<dyn Fn() + Send + Sync>;

/// What a changed path means for the window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// A path under `work/<task>` or `results/<task>`.
    Task(String),
    /// Something watched that is not tied to one task (queue, journal, a folder itself).
    Anything,
    /// Not one of the watched locations.
    Ignored,
}

/// `$AGENTBOX_HOME`, else the system's application data folder ([`crate::platform::data_home`]).
pub fn agentbox_home() -> PathBuf {
    crate::platform::data_home()
}

/// Classifies `path` against the watched `root`. Works on the path text only; nothing is read from disk.
pub fn classify(root: &Path, path: &Path) -> Change {
    let Ok(relative) = path.strip_prefix(root) else {
        return Change::Ignored;
    };
    let names: Option<Vec<&str>> = relative
        .components()
        .map(|part| match part {
            Component::Normal(name) => name.to_str(),
            _ => None,
        })
        .collect();
    let Some(mut parts) = names.map(Vec::into_iter) else {
        return Change::Ignored;
    };
    let Some(top) = parts.next() else {
        return Change::Ignored;
    };
    if TASK_FOLDERS.contains(&top) {
        let Some(task) = parts.next() else {
            return Change::Anything;
        };
        let is_heartbeat = top == "results"
            && parts
                .next()
                .is_some_and(|name| name.starts_with(RUNNER_HEARTBEAT));
        if is_heartbeat {
            return Change::Ignored;
        }
        return task_change(task);
    }
    if OTHER_WATCHED.contains(&top) {
        return Change::Anything;
    }
    Change::Ignored
}

/// The notifications for a batch of changed paths: each distinct task once (sorted), then `None` if
/// anything not tied to a task changed.
pub fn notifications(root: &Path, paths: &[PathBuf]) -> Vec<Option<String>> {
    let mut tasks = BTreeSet::new();
    let mut is_general = false;
    for path in paths {
        match classify(root, path) {
            Change::Task(name) => {
                tasks.insert(name);
            }
            Change::Anything => is_general = true,
            Change::Ignored => {}
        }
    }
    let mut out: Vec<Option<String>> = tasks.into_iter().map(Some).collect();
    if is_general {
        out.push(None);
    }
    out
}

fn task_change(component: &str) -> Change {
    match validate::task(component) {
        Ok(name) => Change::Task(name.to_string()),
        Err(_) => Change::Anything,
    }
}

/// What changed since the last notification. Raw events are reduced to these keys at once, so the memory
/// held is bounded by the number of tasks, not of paths an agent touches.
#[derive(Default)]
struct Pending {
    tasks: BTreeSet<String>,
    is_general: bool,
    /// When the pending changes are reported: one debounce window after the first of them.
    due: Option<Instant>,
    is_stopped: bool,
}

impl Pending {
    /// The notifications for what is pending (tasks sorted, then the general one), leaving nothing pending.
    fn take(&mut self) -> Vec<Option<String>> {
        self.due = None;
        let mut batch: Vec<Option<String>> =
            mem::take(&mut self.tasks).into_iter().map(Some).collect();
        if mem::take(&mut self.is_general) {
            batch.push(None);
        }
        batch
    }
}

/// Collects changes and releases them in batches, at most one batch per debounce window.
struct Debounce {
    window: Duration,
    pending: Mutex<Pending>,
    changed: Condvar,
}

impl Debounce {
    fn new(window: Duration) -> Self {
        Self {
            window,
            pending: Mutex::default(),
            changed: Condvar::new(),
        }
    }

    /// Notes every path of a raw event; reads and follows nothing.
    fn record(&self, root: &Path, event: &notify::Event) {
        if matches!(event.kind, EventKind::Access(_)) {
            return;
        }
        for path in &event.paths {
            self.note(classify(root, path));
        }
    }

    fn note(&self, change: Change) {
        let mut pending = lock(&self.pending);
        match change {
            Change::Task(name) => {
                pending.tasks.insert(name);
            }
            Change::Anything => pending.is_general = true,
            Change::Ignored => return,
        }
        if pending.due.is_none() {
            pending.due = Some(Instant::now() + self.window);
            self.changed.notify_one();
        }
    }

    /// Waits until a batch is due; `None` once stopped.
    fn next_batch(&self) -> Option<Vec<Option<String>>> {
        let mut pending = lock(&self.pending);
        loop {
            if pending.is_stopped {
                return None;
            }
            let Some(due) = pending.due else {
                pending = self.wait(pending, None);
                continue;
            };
            let now = Instant::now();
            if now >= due {
                return Some(pending.take());
            }
            pending = self.wait(pending, Some(due - now));
        }
    }

    fn wait<'a>(
        &self,
        pending: MutexGuard<'a, Pending>,
        timeout: Option<Duration>,
    ) -> MutexGuard<'a, Pending> {
        match timeout {
            Some(timeout) => {
                self.changed
                    .wait_timeout(pending, timeout)
                    .unwrap_or_else(PoisonError::into_inner)
                    .0
            }
            None => self
                .changed
                .wait(pending)
                .unwrap_or_else(PoisonError::into_inner),
        }
    }

    fn stop(&self) {
        lock(&self.pending).is_stopped = true;
        self.changed.notify_one();
    }
}

/// A live file watcher with its debouncer; dropping it stops both.
struct Attached {
    _watcher: RecommendedWatcher,
    debounce: Arc<Debounce>,
}

impl Drop for Attached {
    fn drop(&mut self) {
        self.debounce.stop();
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Watches the agentbox home and ticks. Dropping it stops both.
pub struct Watcher {
    root: PathBuf,
    sink: FactsSink,
    on_tick: TickSink,
    attached: Mutex<Option<Attached>>,
}

impl Watcher {
    /// Starts watching `root` (as soon as it exists), reporting changes to `sink`, and calling `on_tick`
    /// every `tick`. A tick never reports a change: it only says that time passed.
    pub fn start(
        root: PathBuf,
        (sink, on_tick): (FactsSink, TickSink),
        debounce: Duration,
        tick: Duration,
    ) -> Arc<Self> {
        let watcher = Arc::new(Self {
            root,
            sink,
            on_tick,
            attached: Mutex::new(None),
        });
        watcher.attach(debounce);
        let weak = Arc::downgrade(&watcher);
        std::thread::spawn(move || tick_loop(&weak, debounce, tick));
        watcher
    }

    /// Whether the file watcher is attached (the root exists and could be watched).
    pub fn is_attached(&self) -> bool {
        lock(&self.attached).is_some()
    }

    fn attach(&self, debounce: Duration) {
        let mut slot = lock(&self.attached);
        if slot.is_some() {
            return;
        }
        let Ok(root) = self.root.canonicalize() else {
            return;
        };
        match self.create(&root, debounce) {
            Ok(attached) => {
                *slot = Some(attached);
                (self.sink)(None);
            }
            Err(error) => eprintln!("sakura: cannot watch {}: {error}", root.display()),
        }
    }

    fn create(&self, root: &Path, debounce: Duration) -> notify::Result<Attached> {
        let state = Arc::new(Debounce::new(debounce));
        let (recorder, watched) = (Arc::clone(&state), root.to_path_buf());
        let mut watcher = notify::recommended_watcher(
            move |result: notify::Result<notify::Event>| match result {
                Ok(event) => recorder.record(&watched, &event),
                Err(error) => {
                    eprintln!("sakura: file watcher error: {error}");
                    recorder.note(Change::Anything);
                }
            },
        )?;
        watcher.watch(root, RecursiveMode::Recursive)?;
        let (reporter, sink) = (Arc::clone(&state), Arc::clone(&self.sink));
        std::thread::spawn(move || {
            while let Some(batch) = reporter.next_batch() {
                batch.into_iter().for_each(|task| sink(task));
            }
        });
        Ok(Attached {
            _watcher: watcher,
            debounce: state,
        })
    }
}

fn tick_loop(weak: &Weak<Watcher>, debounce: Duration, tick: Duration) {
    loop {
        std::thread::sleep(tick);
        let Some(watcher) = weak.upgrade() else {
            return;
        };
        watcher.attach(debounce);
        (watcher.on_tick)();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::{channel, Receiver};
    use std::time::Instant;

    const SHORT: Duration = Duration::from_millis(200);
    /// How long a test waits for a notification that must come before it calls it a hang.
    const HANG: Duration = Duration::from_secs(60);
    const SLOW_TICK: Duration = Duration::from_secs(3600);

    fn root() -> PathBuf {
        PathBuf::from("/h")
    }

    #[test]
    fn paths_under_work_and_results_map_to_the_task() {
        assert_eq!(
            classify(&root(), Path::new("/h/work/m1/src/a.ts")),
            Change::Task("m1".into())
        );
        assert_eq!(
            classify(&root(), Path::new("/h/results/sample-accent/test.json")),
            Change::Task("sample-accent".into())
        );
        assert_eq!(
            classify(&root(), Path::new("/h/work/m1")),
            Change::Task("m1".into())
        );
    }

    #[test]
    fn invalid_task_names_and_folder_events_mean_anything() {
        assert_eq!(
            classify(&root(), Path::new("/h/work/.DS_Store")),
            Change::Anything
        );
        assert_eq!(
            classify(&root(), Path::new("/h/work/Bad Name/x")),
            Change::Anything
        );
        assert_eq!(classify(&root(), Path::new("/h/work")), Change::Anything);
        let heartbeat = Path::new("/h/results/m1/.runner-alive");
        assert_eq!(classify(&root(), heartbeat), Change::Ignored);
        let written = Path::new("/h/results/m1/.runner-alive.tmp");
        assert_eq!(classify(&root(), written), Change::Ignored);
        let result = Path::new("/h/results/m1/test.json");
        assert_eq!(classify(&root(), result), Change::Task("m1".into()));
        assert_eq!(
            classify(&root(), Path::new("/h/queue/req-1.json")),
            Change::Anything
        );
        assert_eq!(
            classify(&root(), Path::new("/h/journal.jsonl")),
            Change::Anything
        );
    }

    #[test]
    fn everything_else_is_ignored() {
        assert_eq!(
            classify(&root(), Path::new("/h/runner.log")),
            Change::Ignored
        );
        assert_eq!(classify(&root(), Path::new("/h")), Change::Ignored);
        assert_eq!(
            classify(&root(), Path::new("/elsewhere/work/m1")),
            Change::Ignored
        );
        assert_eq!(
            classify(&root(), Path::new("/h/work/../runner.log")),
            Change::Ignored
        );
    }

    #[test]
    fn a_batch_gives_each_task_once_then_a_general_notice() {
        let paths: Vec<PathBuf> = [
            "/h/work/b/x",
            "/h/work/a/y",
            "/h/work/a/z",
            "/h/journal.jsonl",
            "/h/runner.log",
        ]
        .iter()
        .map(PathBuf::from)
        .collect();
        let out = notifications(&root(), &paths);
        assert_eq!(
            out,
            vec![Some("a".to_string()), Some("b".to_string()), None]
        );
        assert!(notifications(&root(), &[PathBuf::from("/h/runner.log")]).is_empty());
    }

    #[test]
    fn a_storm_of_distinct_paths_under_one_task_is_one_notification_and_a_bounded_map() {
        let debounce = Debounce::new(Duration::from_millis(50));
        for index in 0..200_000 {
            let path = PathBuf::from(format!("/h/work/m1/dir{}/file{index}", index % 97));
            let event = notify::Event::new(EventKind::Create(notify::event::CreateKind::File))
                .add_path(path);
            debounce.record(&root(), &event);
        }
        assert_eq!(lock(&debounce.pending).tasks.len(), 1);
        assert_eq!(debounce.next_batch(), Some(vec![Some("m1".to_string())]));
        assert!(lock(&debounce.pending).tasks.is_empty());
    }

    #[test]
    fn access_events_and_ignored_paths_report_nothing() {
        let debounce = Debounce::new(Duration::from_millis(10));
        let access = notify::Event::new(EventKind::Access(notify::event::AccessKind::Any))
            .add_path(PathBuf::from("/h/work/m1/a"));
        let elsewhere = notify::Event::new(EventKind::Any).add_path(PathBuf::from("/h/runner.log"));
        debounce.record(&root(), &access);
        debounce.record(&root(), &elsewhere);
        assert!(lock(&debounce.pending).due.is_none());
    }

    fn no_tick() -> TickSink {
        Arc::new(|| {})
    }

    fn sink() -> (FactsSink, Receiver<Option<String>>) {
        let (sender, receiver) = channel();
        let sender = Mutex::new(sender);
        let sink: FactsSink = Arc::new(move |task| {
            let _ = sender
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .send(task);
        });
        (sink, receiver)
    }

    fn wait_for(
        receiver: &Receiver<Option<String>>,
        wanted: Option<&str>,
        within: Duration,
    ) -> bool {
        let deadline = Instant::now() + within;
        while Instant::now() < deadline {
            if let Ok(task) = receiver.recv_timeout(Duration::from_millis(100)) {
                if task.as_deref() == wanted {
                    return true;
                }
            }
        }
        false
    }

    #[test]
    fn file_changes_under_a_task_are_reported_for_that_task() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join("work/m1")).unwrap();
        let (sink, receiver) = sink();
        let watcher = Watcher::start(
            home.path().to_path_buf(),
            (sink, no_tick()),
            SHORT,
            SLOW_TICK,
        );
        assert!(watcher.is_attached());
        std::fs::write(home.path().join("work/m1/a.txt"), "x").unwrap();
        assert!(wait_for(&receiver, Some("m1"), HANG));
        std::fs::write(home.path().join("journal.jsonl"), "{}\n").unwrap();
        assert!(wait_for(&receiver, None, HANG));
    }

    #[test]
    fn a_missing_home_is_picked_up_when_it_appears() {
        let parent = tempfile::tempdir().unwrap();
        let home = parent.path().join("agentbox-home");
        let (sink, receiver) = sink();
        let sinks = (sink, no_tick());
        let watcher = Watcher::start(home.clone(), sinks, SHORT, Duration::from_millis(300));
        assert!(!watcher.is_attached());
        std::fs::create_dir_all(home.join("work/late")).unwrap();
        let deadline = Instant::now() + HANG;
        while !watcher.is_attached() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(watcher.is_attached());
        std::fs::create_dir_all(home.join("results/late")).unwrap();
        std::fs::write(home.join("results/late/test.json"), "{}").unwrap();
        assert!(wait_for(&receiver, Some("late"), HANG));
    }

    #[test]
    fn the_tick_goes_to_its_own_sink_and_never_says_that_something_changed() {
        let home = tempfile::tempdir().unwrap();
        let (sink, receiver) = sink();
        let ticks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = Arc::clone(&ticks);
        let on_tick: TickSink = Arc::new(move || {
            counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        });
        let tick = Duration::from_millis(100);
        let _watcher = Watcher::start(home.path().to_path_buf(), (sink, on_tick), SHORT, tick);
        // Attaching to the home says once that anything may have changed; the ticks say nothing.
        assert!(wait_for(&receiver, None, HANG));
        std::thread::sleep(Duration::from_millis(1500));
        assert!(ticks.load(std::sync::atomic::Ordering::SeqCst) >= 3);
        assert!(receiver.try_recv().is_err(), "a tick reported a change");
    }

    #[cfg(unix)]
    #[test]
    fn writes_behind_a_symlink_out_of_the_tree_are_not_followed() {
        let home = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join("work")).unwrap();
        std::os::unix::fs::symlink(outside.path(), home.path().join("work/linked")).unwrap();
        let (sink, receiver) = sink();
        let _watcher = Watcher::start(
            home.path().to_path_buf(),
            (sink, no_tick()),
            SHORT,
            SLOW_TICK,
        );
        std::thread::sleep(Duration::from_millis(1500));
        while receiver.try_recv().is_ok() {}
        std::fs::write(outside.path().join("secret.txt"), "x").unwrap();
        assert!(!wait_for(&receiver, Some("linked"), Duration::from_secs(2)));
    }
}
