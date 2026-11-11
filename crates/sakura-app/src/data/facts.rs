//! The facts the window shows, kept in one GPUI entity. Reads run on the tokio runtime; their answers are
//! applied here. A read is requested by an event (a workspace, a result or the journal changed; an action
//! ended), never by the periodic tick, which reads only the machine line (see [`Facts::on_tick`]). Rows keep their last facts while a refetch runs, and a failed read leaves them in place with
//! the error written once.

use super::refresh::{self, RefreshPlan};
use super::source::{FactsEvents, Pending, Signal, Source};
use gpui::{AsyncApp, Context, EventEmitter, Task as GpuiTask, WeakEntity};
use sakura_core::error::BridgeError;
use sakura_core::models::{BoxStatus, JournalEvent, Machine, RunnerState, Task, TaskSummary};
use sakura_core::said::Said;
use sakura_core::service::is_environment_error;
use sakura_core::watcher::Watcher;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

/// Whether the window can show tasks.
#[derive(Debug, Clone, PartialEq)]
pub enum Phase {
    /// Neither the task names nor the first `box status` have answered yet.
    Starting,
    /// Facts are shown.
    Ready,
    /// `box` is missing or Docker is down; retried on every tick.
    Blocked(BridgeError),
}

/// One task with the time its facts were read.
#[derive(Debug, Clone)]
pub struct Row {
    pub task: Task,
    pub fetched_at: Instant,
    /// When the facts last said the workspace may have changed: a read asked because the task's files
    /// changed, or one whose answer says something else of the work than the row it replaces. The task's
    /// changes are read again only when this moves, so reads that bring the same facts (the first facts,
    /// then every task's, at launch) cost one `box diff`, not one each.
    pub work_read_at: Instant,
    /// When the read that brought these facts was started: an answer to an older request never replaces them.
    pub requested_at: Instant,
    pub is_refreshing: bool,
    /// Only the name, profile and base are known (from `box tasks`): the facts have not been read yet, so
    /// nothing may be said about the task's state.
    pub is_pending: bool,
}

/// Why one task's facts are read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Why {
    /// Something about the task changed (its files, an action on it, a take): its changes are read again
    /// even when every figure in the facts is the same, because an edit can keep every figure.
    Changed,
    /// Nothing is known to have changed (the first facts of a row, the once-a-minute look at an agent):
    /// its changes are read again only when the facts say something else of the work.
    Asked,
}

/// A read that failed without blocking the window, and the task it was for (`None` for `box status`).
#[derive(Debug, Clone, PartialEq)]
struct ReadError {
    task: Option<String>,
    said: Said,
    /// What `box` itself wrote, for "Dettagli".
    details: Option<String>,
}

/// Emitted after a `box status` answered: the rows now list every task that exists, so state kept for any
/// other task can go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatusRead;

/// All facts of the window.
pub struct Facts {
    source: Source,
    phase: Phase,
    rows: Vec<Row>,
    runner: Option<RunnerState>,
    machine: Option<Machine>,
    error: Option<ReadError>,
    journal_cursor: Option<String>,
    /// The journal was read once: from then on every event read is new, also when that first read found
    /// the journal empty and so left no cursor.
    is_journal_read: bool,
    is_status_running: bool,
    /// A full read was requested while one ran, by [`Facts::refresh_all`] or by a journal event: one more
    /// starts when that one answers, because the one under way may predate what asked. Ticks never
    /// queue one, and any number of askers queue one read.
    is_status_queued: bool,
    status_started_at: Option<Instant>,
    is_journal_running: bool,
    /// The journal changed while it was being read: it is read once more when that read answers, since
    /// the read under way may have missed what changed.
    is_journal_queued: bool,
    /// The tasks whose own facts (`box facts`) are being read now.
    reading: HashSet<String>,
    /// The tasks a full read took off the list, with when that read was asked: facts of one of them that
    /// were asked before it are about a task that is gone, and must not list it again.
    removed: HashMap<String, Instant>,
    /// The lavori whose agent asked the person something in its conversation and waits for the answer:
    /// known to the window alone, and shown as the lavoro's state before anything `box` says.
    waiting: HashSet<String>,
    /// What each lavoro's conversation in this window says of its agent (at work, answered).
    live: HashMap<String, sakura_core::model::Live>,
    /// The lavori whose conversation is kept from an earlier run of the app.
    kept: HashSet<String>,
    /// The tasks whose agent's terminal is attached in the window.
    attached: HashSet<String>,
    /// When each running agent without an attached terminal was last asked about on the timer.
    checked_at: HashMap<String, Instant>,
    /// The task the window opens on, when it is known before any read: its facts are asked first.
    first: Option<String>,
    /// When the facts began to be read, to say once on stderr how long the names and the first full read
    /// took.
    began_at: Instant,
    is_first_read_said: bool,
    _watcher: Option<Arc<Watcher>>,
    _listener: GpuiTask<()>,
}

impl EventEmitter<StatusRead> for Facts {}

impl Facts {
    /// Whether `task` is listed.
    pub fn has_task(&self, task: &str) -> bool {
        self.row(task).is_some()
    }

    /// Starts listening to `events` and loads everything once: the task names, then the facts of `first`
    /// (the task the window opens on, when known), then every task's.
    pub fn new(
        source: Source,
        events: FactsEvents,
        watcher: Option<Arc<Watcher>>,
        first: Option<String>,
        cx: &mut Context<Self>,
    ) -> Self {
        let listener = cx.spawn(async move |this, cx| listen(this, events, cx).await);
        let mut facts = Self {
            source,
            phase: Phase::Starting,
            rows: Vec::new(),
            runner: None,
            machine: None,
            error: None,
            journal_cursor: None,
            is_journal_read: false,
            is_status_running: false,
            is_status_queued: false,
            status_started_at: None,
            is_journal_running: false,
            is_journal_queued: false,
            reading: HashSet::new(),
            removed: HashMap::new(),
            waiting: HashSet::new(),
            live: HashMap::new(),
            kept: HashSet::new(),
            attached: HashSet::new(),
            checked_at: HashMap::new(),
            first,
            began_at: Instant::now(),
            is_first_read_said: false,
            _watcher: watcher,
            _listener: listener,
        };
        facts.load_names(cx);
        facts.look_at_journal(cx);
        facts.load_machine(cx);
        facts
    }

    /// Whether the agent of `task` waits for the person's answer.
    pub fn is_waiting(&self, task: &str) -> bool {
        self.waiting.contains(task)
    }

    /// What the conversation of `task` says of its agent. A lavoro whose conversation is kept from
    /// before and was not taken up in this window yet had its answer then: its agent stands by.
    pub fn live(&self, task: &str) -> sakura_core::model::Live {
        use sakura_core::model::Live;
        match self.live.get(task).copied().unwrap_or_default() {
            Live::Unknown if self.kept.contains(task) => Live::Answered,
            live => live,
        }
    }

    /// Notes which lavori have a conversation kept from before.
    pub fn set_kept(&mut self, kept: HashSet<String>, cx: &mut Context<Self>) {
        if self.kept != kept {
            self.kept = kept;
            cx.notify();
        }
    }

    /// Notes what the conversation of `task` says of its agent.
    pub fn set_live(&mut self, task: &str, live: sakura_core::model::Live, cx: &mut Context<Self>) {
        if self.live.insert(task.to_string(), live) != Some(live) {
            cx.notify();
        }
    }

    /// The lavori whose agent waits for the person.
    pub fn waiting(&self) -> &HashSet<String> {
        &self.waiting
    }

    /// Notes that the agent of `task` waits for the person, or no longer does.
    pub fn set_waiting(&mut self, task: &str, is_waiting: bool, cx: &mut Context<Self>) {
        let changed = match is_waiting {
            true => self.waiting.insert(task.to_string()),
            false => self.waiting.remove(task),
        };
        if changed {
            cx.notify();
        }
    }

    /// Whether tasks can be shown.
    pub fn phase(&self) -> &Phase {
        &self.phase
    }

    /// Tasks in `box status` order.
    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// One task by name.
    pub fn row(&self, name: &str) -> Option<&Row> {
        self.rows.iter().find(|row| row.task.name == name)
    }

    /// The runner, once `box status` answered.
    pub fn runner(&self) -> Option<&RunnerState> {
        self.runner.as_ref()
    }

    /// Machine facts, once read.
    pub fn machine(&self) -> Option<&Machine> {
        self.machine.as_ref()
    }

    /// When the last `box status` that answered was started: requests made before it are reflected in it.
    pub fn status_started_at(&self) -> Option<Instant> {
        self.status_started_at
    }

    /// Reads every task again (after one was removed, or when the journal says so). A read already running
    /// is followed by one more, so the asker sees facts read after its request.
    pub fn refresh_all(&mut self, cx: &mut Context<Self>) {
        if self.is_status_running {
            self.is_status_queued = true;
            return;
        }
        self.load_all(cx);
    }

    /// Lists `task`, whose creation just started, unless it is listed already: its row is on screen
    /// before `box` says anything of it.
    pub fn list_being_created(&mut self, task: Task, cx: &mut Context<Self>) {
        if !self.has_task(&task.name) {
            self.removed.remove(&task.name);
            self.rows.push(fresh_row(task, Instant::now()));
            cx.notify();
        }
    }

    /// Reads one task again (after an action on it).
    pub fn refresh_task(&mut self, name: String, cx: &mut Context<Self>) {
        self.load_task(name, Why::Changed, cx);
    }

    /// Reads `name` now, ahead of the read of every task, when only its name is known and its own facts
    /// are not already being read.
    pub fn read_if_pending(&mut self, name: &str, cx: &mut Context<Self>) {
        let is_pending = self.row(name).is_some_and(|row| row.is_pending);
        if is_pending && !self.reading.contains(name) {
            self.load_task(name.to_string(), Why::Asked, cx);
        }
    }

    /// The last read error that did not block the window, prefixed with its task when it was for one.
    pub fn error(&self) -> Option<Said> {
        self.error.as_ref().map(|error| match &error.task {
            Some(task) => Said::new("facts.error.of_task")
                .text("task", task)
                .said("error", error.said.clone()),
            None => error.said.clone(),
        })
    }

    /// What `box` itself wrote when the last read failed, for "Dettagli".
    pub fn error_details(&self) -> Option<&str> {
        self.error.as_ref()?.details.as_deref()
    }

    /// Whether a read of the tasks is under way.
    #[cfg(test)]
    pub fn is_reading(&self) -> bool {
        self.is_status_running || !self.reading.is_empty()
    }

    /// Applies `signal` as if it arrived at `now`.
    #[cfg(test)]
    pub fn signal_at(&mut self, signal: Signal, now: Instant, cx: &mut Context<Self>) {
        self.on_signal(signal, now, cx);
    }

    fn on_signal(&mut self, signal: Signal, now: Instant, cx: &mut Context<Self>) {
        match signal {
            Signal::Changed(Some(name)) => self.load_task(name, Why::Changed, cx),
            Signal::Changed(None) => self.on_change(cx),
            Signal::Tick => self.on_tick(now, cx),
        }
    }

    /// Tells which tasks have their agent's terminal attached in the window: its end says the agent
    /// stopped, so those agents are never asked about on a timer.
    pub fn set_attached(&mut self, attached: HashSet<String>) {
        self.attached = attached;
    }

    /// Time passed: the machine line is read (no `box`), and while `box` cannot be used at all it is tried
    /// again. Nothing else is asked on a timer, with one exception that no file can tell: whether an
    /// agent that no terminal of the window is attached to still runs, once per
    /// [`refresh::RUNNING_FACTS_MAX_AGE`].
    fn on_tick(&mut self, now: Instant, cx: &mut Context<Self>) {
        self.load_machine(cx);
        match self.phase {
            Phase::Blocked(_) => self.load_all(cx),
            Phase::Starting => {}
            Phase::Ready => self.check_unattached_agent(now, cx),
        }
    }

    /// Something not tied to one task changed (the journal, the queue): the journal says what to read.
    fn on_change(&mut self, cx: &mut Context<Self>) {
        match self.phase {
            Phase::Ready => self.look_at_journal(cx),
            Phase::Blocked(_) => self.load_all(cx),
            Phase::Starting => {}
        }
    }

    /// Reads the running agent, among those no terminal is attached to, that was read or checked longest
    /// ago and more than a minute ago: an agent that exits on its own is noticed only by a read.
    fn check_unattached_agent(&mut self, now: Instant, cx: &mut Context<Self>) {
        let unattached = self
            .rows
            .iter()
            .filter(|row| !self.attached.contains(&row.task.name));
        let seen = |row: &Row| {
            let checked = self.checked_at.get(&row.task.name).copied();
            checked.map_or(row.fetched_at, |at| at.max(row.fetched_at))
        };
        let rows = unattached.map(|row| (&row.task, seen(row), row.is_refreshing));
        if let Some(name) = refresh::stale_running(rows, now) {
            self.checked_at.insert(name.clone(), now);
            self.load_task(name, Why::Asked, cx);
        }
    }

    /// Asks `box tasks` for the names, shows them at once as rows whose state is being read, then asks the
    /// facts: the first task's, then everyone's. A `box` without the command, or a failed read, goes straight
    /// to the facts.
    fn load_names(&mut self, cx: &mut Context<Self>) {
        let requested_at = Instant::now();
        let pending = self.source.tasks();
        await_then(pending, cx, move |facts, result, cx| {
            match result {
                Ok(Some(names)) => facts.apply_names(names, requested_at),
                Ok(None) => {}
                Err(error) if is_environment_error(&error) => facts.apply_error(error, None),
                Err(error) => eprintln!("sakura: task names: {error}"),
            }
            if let Some(first) = facts.first.take().filter(|name| facts.has_task(name)) {
                facts.load_task(first, Why::Asked, cx);
            }
            facts.load_all(cx);
            cx.notify();
        });
    }

    fn load_all(&mut self, cx: &mut Context<Self>) {
        if self.is_status_running {
            return;
        }
        self.is_status_running = true;
        self.rows
            .iter_mut()
            .for_each(|row| row.is_refreshing = true);
        let started_at = Instant::now();
        let pending = self.source.status();
        await_then(pending, cx, move |facts, result, cx| {
            facts.is_status_running = false;
            let is_read = result.is_ok();
            if is_read {
                facts.status_started_at = Some(started_at);
            }
            facts.apply_status(result, started_at);
            if is_read {
                cx.emit(StatusRead);
            }
            if std::mem::take(&mut facts.is_status_queued) {
                facts.load_all(cx);
            }
            cx.notify();
        });
        cx.notify();
    }

    fn load_task(&mut self, name: String, why: Why, cx: &mut Context<Self>) {
        if let Some(row) = self.rows.iter_mut().find(|row| row.task.name == name) {
            row.is_refreshing = true;
        }
        self.reading.insert(name.clone());
        let requested_at = Instant::now();
        let pending = self.source.task_facts(name.clone());
        await_then(pending, cx, move |facts, result, cx| {
            facts.reading.remove(&name);
            facts.apply_task(&name, result, (requested_at, why));
            cx.notify();
        });
    }

    fn look_at_journal(&mut self, cx: &mut Context<Self>) {
        if self.is_journal_running {
            self.is_journal_queued = true;
            return;
        }
        self.is_journal_running = true;
        let pending = self.source.journal(self.journal_cursor.clone());
        await_then(pending, cx, |facts, result, cx| {
            facts.is_journal_running = false;
            facts.apply_journal(result, cx);
            if std::mem::take(&mut facts.is_journal_queued) {
                facts.look_at_journal(cx);
            }
        });
    }

    fn load_machine(&mut self, cx: &mut Context<Self>) {
        let pending = self.source.machine();
        await_then(pending, cx, |facts, result, cx| match result {
            // The window is drawn again only when what it says of the machine changed: a calm machine
            // says nothing, so its numbers moving at rest draw nothing.
            Ok(machine) => {
                let is_said_anew = machine_said(Some(&machine)) != machine_said(facts.machine());
                facts.machine = Some(machine);
                if is_said_anew {
                    cx.notify();
                }
            }
            Err(error) => eprintln!("sakura: machine facts: {error}"),
        });
    }

    /// Lists the tasks `box tasks` named. Facts already read stay as they are: a `box status` that answered
    /// first knows more than the names do.
    fn apply_names(&mut self, names: Vec<TaskSummary>, requested_at: Instant) {
        if self.phase != Phase::Starting {
            return;
        }
        self.rows = named_rows(&self.rows, names, requested_at);
        self.phase = Phase::Ready;
        let elapsed = self.began_at.elapsed().as_millis();
        eprintln!("sakura: {} task names after {elapsed} ms", self.rows.len());
    }

    fn apply_status(&mut self, result: Result<BoxStatus, BridgeError>, requested_at: Instant) {
        match result {
            Ok(status) => {
                let merged = merge_status(&self.rows, status.tasks, requested_at);
                let gone = self.rows.iter().map(|row| &row.task.name);
                let gone = gone.filter(|name| !merged.iter().any(|row| row.task.name == **name));
                let gone: Vec<String> = gone.cloned().collect();
                self.removed
                    .extend(gone.into_iter().map(|name| (name, requested_at)));
                self.rows = merged;
                let rows = &self.rows;
                self.checked_at
                    .retain(|name, _| rows.iter().any(|row| row.task.name == *name));
                self.waiting
                    .retain(|name| rows.iter().any(|row| row.task.name == *name));
                self.live
                    .retain(|name, _| rows.iter().any(|row| row.task.name == *name));
                self.runner = Some(status.runner);
                self.phase = Phase::Ready;
                self.error = None;
                if !std::mem::replace(&mut self.is_first_read_said, true) {
                    let elapsed = self.began_at.elapsed().as_millis();
                    eprintln!("sakura: every task read after {elapsed} ms");
                }
            }
            Err(error) => self.apply_error(error, None),
        }
        if self.phase != Phase::Ready || self.error.is_some() {
            self.rows
                .iter_mut()
                .for_each(|row| row.is_refreshing = false);
        }
    }

    fn apply_task(
        &mut self,
        name: &str,
        result: Result<Task, BridgeError>,
        (requested_at, why): (Instant, Why),
    ) {
        let is_listed = self.rows.iter().any(|row| row.task.name == name);
        match result {
            // Asked before the read that took the task off the list: these are facts of a task that is
            // gone. A task made again under the same name is listed by a read asked after the removal.
            Ok(_) if !is_listed && self.was_removed_after(name, requested_at) => {}
            Ok(task) => {
                upsert(&mut self.rows, task, requested_at, why);
                self.removed.remove(name);
                if self
                    .error
                    .as_ref()
                    .is_some_and(|e| e.task.as_deref() == Some(name))
                {
                    self.error = None;
                }
            }
            Err(error) if is_environment_error(&error) => self.apply_error(error, None),
            // A task being created or removed: the journal brings it in or takes it out with a full read.
            Err(error) if !is_listed => eprintln!("sakura: facts of {name}: {error}"),
            Err(error) => {
                if let Some(row) = self.rows.iter_mut().find(|row| row.task.name == name) {
                    row.is_refreshing = false;
                }
                self.apply_error(error, Some(name.to_string()));
            }
        }
    }

    /// Whether a full read asked at or after `requested_at` took `name` off the list.
    fn was_removed_after(&self, name: &str, requested_at: Instant) -> bool {
        self.removed
            .get(name)
            .is_some_and(|removed_at| *removed_at >= requested_at)
    }

    fn apply_journal(
        &mut self,
        result: Result<Vec<JournalEvent>, BridgeError>,
        cx: &mut Context<Self>,
    ) {
        let events = match result {
            Ok(events) => events,
            Err(error) => return eprintln!("sakura: journal: {error}"),
        };
        if !std::mem::replace(&mut self.is_journal_read, true) {
            // The first look only finds where the journal ends: what happened before is not acted on.
            self.journal_cursor = refresh::newest(&events);
            return;
        }
        let RefreshPlan {
            is_full,
            tasks,
            cursor,
        } = refresh::plan(&events, self.journal_cursor.as_deref());
        self.journal_cursor = cursor;
        if is_full {
            self.refresh_all(cx);
        }
        let named = tasks.into_iter();
        named.for_each(|name| self.load_task(name, Why::Changed, cx));
    }

    fn apply_error(&mut self, error: BridgeError, task: Option<String>) {
        if is_environment_error(&error) {
            self.phase = Phase::Blocked(error);
            return;
        }
        if self.phase == Phase::Starting {
            self.phase = Phase::Ready;
        }
        self.error = Some(ReadError {
            task,
            said: error.said,
            details: error.details,
        });
    }
}

/// Feeds every notification to the entity until the channel or the entity goes away.
async fn listen(this: WeakEntity<Facts>, mut events: FactsEvents, cx: &mut AsyncApp) {
    while let Some(signal) = events.recv().await {
        if this
            .update(cx, |facts, cx| facts.on_signal(signal, Instant::now(), cx))
            .is_err()
        {
            return;
        }
    }
}

/// Awaits a pending read off the UI thread and applies its answer to the entity.
fn await_then<T: 'static>(
    pending: Pending<T>,
    cx: &mut Context<Facts>,
    apply: impl FnOnce(&mut Facts, Result<T, BridgeError>, &mut Context<Facts>) + 'static,
) {
    cx.spawn(async move |this, cx| {
        let result = pending.await;
        drop(this.update(cx, |facts, cx| apply(facts, result, cx)));
    })
    .detach();
}

fn fresh_row(task: Task, requested_at: Instant) -> Row {
    let now = Instant::now();
    Row {
        task,
        fetched_at: now,
        work_read_at: now,
        requested_at,
        is_refreshing: false,
        is_pending: false,
    }
}

/// The row for `task`'s new facts in place of `old`: the workspace is taken as unchanged, and its changes
/// are not read again, when `old` held read facts that say the same of the work.
fn same_work_row(old: Option<&Row>, task: Task, requested_at: Instant) -> Row {
    let kept = old
        .filter(|old| !old.is_pending && work_key(&old.task) == work_key(&task))
        .map(|old| old.work_read_at);
    let fresh = fresh_row(task, requested_at);
    Row {
        work_read_at: kept.unwrap_or(fresh.work_read_at),
        ..fresh
    }
}

/// What the facts say of the work in a task's workspace: its base and commit, and the count of what
/// changed. Two reads with the same key and no word of a change in between show the same changes.
fn work_key(task: &Task) -> impl PartialEq + '_ {
    (
        (task.readable, task.base_sha.as_str(), task.head.as_deref()),
        (task.ahead, task.dirty_files),
        (task.files_changed, task.insertions, task.deletions),
    )
}

/// The rows after `box tasks` answered: one row per name in its order, a row already there kept as it is,
/// a new one marked as being read.
fn named_rows(rows: &[Row], names: Vec<TaskSummary>, requested_at: Instant) -> Vec<Row> {
    names
        .into_iter()
        .map(|summary| {
            let read = rows.iter().find(|row| row.task.name == summary.name);
            read.cloned().unwrap_or_else(|| Row {
                is_pending: true,
                ..fresh_row(unread_task(summary), requested_at)
            })
        })
        .collect()
}

/// The task of a row whose facts have not been read: its name, profile and base, and nothing else known.
fn unread_task(summary: TaskSummary) -> Task {
    Task {
        name: summary.name,
        profile: summary.profile,
        place: summary.place,
        folder: None,
        base: summary.base,
        base_sha: String::new(),
        readable: true,
        branch: None,
        head: None,
        head_subject: None,
        head_time: None,
        ahead: None,
        dirty_files: None,
        files_changed: None,
        insertions: None,
        deletions: None,
        taken: None,
        agent: None,
        other_containers: 0,
        results: BTreeMap::new(),
        checks: None,
        preparing: None,
        disk: None,
    }
}

/// The rows after a `box status` requested at `requested_at`: its tasks in its order, except that a row read by
/// a request started after it is kept as it is, and stays even when the status does not list its task yet.
fn merge_status(rows: &[Row], tasks: Vec<Task>, requested_at: Instant) -> Vec<Row> {
    let newer = |name: &str| {
        rows.iter()
            .find(|row| row.task.name == name && row.requested_at > requested_at)
    };
    let mut merged: Vec<Row> = tasks
        .into_iter()
        .map(|task| match newer(&task.name) {
            Some(row) => row.clone(),
            None => {
                let old = rows.iter().find(|row| row.task.name == task.name);
                same_work_row(old, task, requested_at)
            }
        })
        .collect();
    let unlisted: Vec<Row> = rows
        .iter()
        .filter(|row| row.requested_at > requested_at)
        .filter(|row| !merged.iter().any(|kept| kept.task.name == row.task.name))
        .cloned()
        .collect();
    merged.extend(unlisted);
    merged
}

/// Replaces a task's row with new facts, or appends it when the task is new. Facts from a request older than
/// the row's own are dropped: only the refreshing mark is cleared. A read asked with no word of a change
/// that brings the same facts of the work leaves the task's changes as they are.
fn upsert(rows: &mut Vec<Row>, task: Task, requested_at: Instant, why: Why) {
    match rows.iter_mut().find(|row| row.task.name == task.name) {
        Some(row) if row.requested_at > requested_at => row.is_refreshing = false,
        Some(row) if why == Why::Asked => *row = same_work_row(Some(&*row), task, requested_at),
        Some(row) => *row = fresh_row(task, requested_at),
        None => rows.push(fresh_row(task, requested_at)),
    }
}

/// What the window says of a machine: its load only while it is high, its memory only while it is
/// critical (see `ui::status_bar::line`). A calm machine says nothing, whatever its numbers are.
fn machine_said(machine: Option<&Machine>) -> (Option<Said>, bool) {
    let Some(machine) = machine else {
        return (None, false);
    };
    let load = sakura_core::model::is_load_high(machine).then(|| sakura_core::model::load(machine));
    (load, sakura_core::model::is_memory_critical(machine))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn a_calm_machine_says_the_same_whatever_its_numbers_and_a_strained_one_says_its_load() {
        let calm = crate::data::source::calm_machine();
        let busier = Machine {
            load1: calm.load1 + 1.3,
            swap_used_mb: 900,
            disk_free_gb: 12.0,
            ..calm.clone()
        };
        assert_eq!(machine_said(None), machine_said(Some(&calm)));
        assert_eq!(machine_said(Some(&calm)), machine_said(Some(&busier)));
        let strained = Machine {
            load1: f64::from(calm.cpus) + 2.0,
            ..calm.clone()
        };
        assert_ne!(machine_said(Some(&calm)), machine_said(Some(&strained)));
        let more_strained = Machine {
            load1: strained.load1 + 1.0,
            ..strained.clone()
        };
        assert_ne!(
            machine_said(Some(&strained)),
            machine_said(Some(&more_strained)),
            "the load that is said changed"
        );
        let critical = Machine {
            memory_pressure: sakura_core::models::MemoryPressure::Critical,
            ..calm.clone()
        };
        assert_ne!(machine_said(Some(&calm)), machine_said(Some(&critical)));
    }

    fn status() -> BoxStatus {
        serde_json::from_str(include_str!(
            "../../../sakura-core/tests/fixtures/status.json"
        ))
        .unwrap()
    }

    fn rows_read_at(at: Instant) -> Vec<Row> {
        status()
            .tasks
            .into_iter()
            .map(|task| fresh_row(task, at))
            .collect()
    }

    fn summaries(names: &[&str]) -> Vec<TaskSummary> {
        names
            .iter()
            .map(|name| TaskSummary {
                name: (*name).into(),
                place: sakura_core::models::Place::Box,
                profile: "example-app".into(),
                base: "main".into(),
            })
            .collect()
    }

    #[test]
    fn names_list_every_task_at_once_as_rows_being_read() {
        let at = Instant::now();
        let rows = named_rows(&[], summaries(&["m1", "m2"]), at);
        let names: Vec<&str> = rows.iter().map(|row| row.task.name.as_str()).collect();
        assert_eq!(names, vec!["m1", "m2"]);
        assert!(rows.iter().all(|row| row.is_pending));
        assert_eq!(rows[0].task.base, "main");
        assert_eq!(rows[0].task.ahead, None, "nothing is known but the name");
    }

    #[test]
    fn names_never_replace_facts_already_read() {
        let at = Instant::now();
        let read = rows_read_at(at);
        let known = read[1].task.name.clone();
        let rows = named_rows(&read, summaries(&["new", &known]), at);
        assert!(rows[0].is_pending);
        assert!(!rows[1].is_pending);
        assert_eq!(rows[1].task.ahead, read[1].task.ahead);
        assert_eq!(rows.len(), 2, "a task box no longer names is gone");
    }

    #[test]
    fn the_facts_of_one_task_fill_its_row_and_leave_the_others_being_read() {
        let at = Instant::now();
        let listed = status().tasks;
        let names: Vec<&str> = listed.iter().map(|task| task.name.as_str()).collect();
        let mut rows = named_rows(&[], summaries(&names), at);
        upsert(
            &mut rows,
            listed[1].clone(),
            at + Duration::from_millis(1),
            Why::Changed,
        );
        assert!(!rows[1].is_pending);
        assert_eq!(rows[1].task.head, listed[1].head);
        assert!(rows[0].is_pending);
        assert_eq!(rows.len(), listed.len(), "filled in place, never appended");
    }

    #[test]
    fn a_status_fills_every_row_still_being_read() {
        let at = Instant::now();
        let listed = status().tasks;
        let names: Vec<&str> = listed.iter().map(|task| task.name.as_str()).collect();
        let rows = named_rows(&[], summaries(&names), at);
        let merged = merge_status(&rows, listed, at + Duration::from_millis(1));
        assert!(merged.iter().all(|row| !row.is_pending));
    }

    #[test]
    fn upsert_replaces_in_place_and_appends_new_tasks() {
        let at = Instant::now();
        let mut rows = rows_read_at(at);
        rows[0].is_refreshing = true;
        let mut changed = rows[0].task.clone();
        changed.ahead = Some(9);
        upsert(&mut rows, changed, at, Why::Changed);
        assert_eq!(rows[0].task.ahead, Some(9));
        assert!(!rows[0].is_refreshing);
        let mut new_task = rows[1].task.clone();
        new_task.name = "new".into();
        upsert(&mut rows, new_task, at, Why::Changed);
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[4].task.name, "new");
    }

    #[test]
    fn upsert_with_facts_older_than_the_row_keeps_the_row() {
        let status_at = Instant::now();
        let task_at = status_at - Duration::from_secs(1);
        let mut rows = rows_read_at(status_at);
        let mut stale = rows[0].task.clone();
        stale.ahead = Some(42);
        upsert(&mut rows, stale, task_at, Why::Changed);
        assert_ne!(rows[0].task.ahead, Some(42));
        assert_eq!(rows[0].requested_at, status_at);
    }

    #[test]
    fn merge_status_started_before_a_task_read_keeps_that_task_row() {
        let status_at = Instant::now();
        let task_at = status_at + Duration::from_secs(1);
        let mut rows = rows_read_at(status_at - Duration::from_secs(5));
        let mut newer = rows[1].task.clone();
        newer.ahead = Some(42);
        upsert(&mut rows, newer, task_at, Why::Changed);
        let merged = merge_status(&rows, status().tasks, status_at);
        assert_eq!(merged[1].task.ahead, Some(42));
        assert_eq!(merged[1].requested_at, task_at);
        assert_eq!(merged[0].requested_at, status_at);
        assert_eq!(merged.len(), 4);
    }

    #[test]
    fn merge_status_keeps_a_task_created_after_it_started_and_drops_older_missing_ones() {
        let status_at = Instant::now();
        let mut rows = rows_read_at(status_at - Duration::from_secs(5));
        let mut created = rows[0].task.clone();
        created.name = "new".into();
        upsert(
            &mut rows,
            created,
            status_at + Duration::from_secs(1),
            Why::Changed,
        );
        let mut listed = status().tasks;
        let dropped = listed.remove(3).name;
        let merged = merge_status(&rows, listed, status_at);
        let names: Vec<&str> = merged.iter().map(|row| row.task.name.as_str()).collect();
        assert!(!names.contains(&dropped.as_str()));
        assert_eq!(names.last(), Some(&"new"));
        assert_eq!(merged.len(), 4);
    }

    #[test]
    fn a_read_of_every_task_that_says_the_same_of_the_work_does_not_call_for_its_changes_again() {
        let at = Instant::now();
        let rows = rows_read_at(at);
        let read_at: Vec<Instant> = rows.iter().map(|row| row.work_read_at).collect();
        let merged = merge_status(&rows, status().tasks, at + Duration::from_secs(1));
        let kept: Vec<Instant> = merged.iter().map(|row| row.work_read_at).collect();
        assert_eq!(kept, read_at, "the same facts: no workspace changed");
        let mut moved = status().tasks;
        moved[0].head = Some("0123456789abcdef0123456789abcdef01234567".into());
        let merged = merge_status(&rows, moved, at + Duration::from_secs(1));
        assert!(
            merged[0].work_read_at > read_at[0],
            "a new commit is a change"
        );
        assert_eq!(merged[1].work_read_at, read_at[1]);
    }

    #[test]
    fn facts_asked_with_no_word_of_a_change_call_for_the_changes_only_when_they_differ() {
        let at = Instant::now();
        let mut rows = rows_read_at(at);
        let (task, read_at) = (rows[0].task.clone(), rows[0].work_read_at);
        let name = task.name.clone();
        let later = at + Duration::from_secs(1);
        upsert(&mut rows, task.clone(), later, Why::Asked);
        assert_eq!(rows[0].work_read_at, read_at, "{name}: the same facts");
        assert!(
            rows[0].fetched_at >= read_at,
            "the facts themselves are the new ones"
        );
        upsert(&mut rows, task, later, Why::Changed);
        assert!(
            rows[0].work_read_at > read_at,
            "its files changed: an edit can keep every figure"
        );
    }
}
