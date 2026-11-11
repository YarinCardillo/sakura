//! What the person asks `box` to do: verify, cancel a verification, take, start an agent; and the log tails the Checks rows open
//! to. Each action runs at most once at a time (a second press while the first runs is ignored), shows its
//! pending word at once and its outcome only when `box` answers. After take, stop, remove and start agent
//! succeed, the action stays pending until facts read after it arrive, so the old facts never offer it again;
//! when they do not come within [`SETTLE_LIMIT`] it says plainly that the outcome is not confirmed yet.
//! Nothing here is remembered across launches.

use super::facts::{Facts, StatusRead};
use super::retain_tasks;
use super::source::{Pending, QueueEnd, Source};
use crate::l10n::Key;
use gpui::{Context, Entity, SharedString, Subscription};
use sakura_core::box_words;
use sakura_core::error::{BridgeError, ErrorCode};
use sakura_core::models::{Account, QueuedRequest, RunnerPhase};
use sakura_core::projects::NewLavoro;
use sakura_core::said::Said;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

/// How long an action that `box` finished waits for facts read after it before saying so.
pub const SETTLE_LIMIT: Duration = Duration::from_secs(90);

/// What an action shows when `box` finished it but no facts read since have arrived within [`SETTLE_LIMIT`].
pub const UNCONFIRMED: Key = Key("action.not_confirmed");

/// One action on one task.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ActionKey {
    Verify { task: String, verb: String },
    Cancel { task: String, verb: String },
    New { task: String },
    Take { task: String },
    StopAgent { task: String },
    Remove { task: String },
}

impl ActionKey {
    /// The take of `task`.
    pub fn take(task: &str) -> Self {
        ActionKey::Take {
            task: task.to_string(),
        }
    }

    /// The agent stop of `task`.
    pub fn stop_agent(task: &str) -> Self {
        ActionKey::StopAgent {
            task: task.to_string(),
        }
    }

    /// The removal of `task`.
    pub fn remove(task: &str) -> Self {
        ActionKey::Remove {
            task: task.to_string(),
        }
    }

    /// The creation of `task`.
    pub fn new_task(task: &str) -> Self {
        ActionKey::New {
            task: task.to_string(),
        }
    }

    /// The cancelling of `verb` for `task`.
    pub fn cancel(task: &str, verb: &str) -> Self {
        ActionKey::Cancel {
            task: task.to_string(),
            verb: verb.to_string(),
        }
    }

    /// The task the action is on.
    pub fn task(&self) -> &str {
        match self {
            ActionKey::Verify { task, .. }
            | ActionKey::Cancel { task, .. }
            | ActionKey::New { task }
            | ActionKey::Take { task }
            | ActionKey::StopAgent { task }
            | ActionKey::Remove { task } => task,
        }
    }
}

/// When a run of the checks that wait ended and what it ended with, as it is kept.
fn ended_of(ended: Result<QueueEnd, BridgeError>) -> (Instant, QueueOutcome) {
    let (at, ended) = ended.unwrap_or_else(|error| (Instant::now(), Err(error)));
    (at, ended.map_err(|error| (error.said, error.details)))
}

/// Why a run left the requests it was started for: its failure, or the general sentence with the warning
/// `box` ended with.
fn why_left(ended: &QueueOutcome) -> Failure {
    match ended {
        Ok(None) => (Said::new(QUEUE_STALLED), None),
        // The line `box` ended with says why, when it is one the app has a sentence for.
        Ok(Some(line)) => {
            let told = box_words::said(line);
            let is_known = told.said != Said::from(box_words::UNKNOWN);
            let said = if is_known {
                told.said
            } else {
                Said::new(QUEUE_STALLED)
            };
            (said, Some(line.clone()))
        }
        Err(failure) => failure.clone(),
    }
}

/// Whether facts read after an action that finished at `finished_at` have arrived: a read of its task, or a
/// full `box status` (which also says a removed task is gone), started after it.
pub fn is_settled(
    finished_at: Instant,
    task_requested_at: Option<Instant>,
    status_started_at: Option<Instant>,
) -> bool {
    let is_after = |at: Option<Instant>| at.is_some_and(|at| at > finished_at);
    is_after(task_requested_at) || is_after(status_started_at)
}

/// The log tail of one verb of one task.
#[derive(Debug, Clone, PartialEq)]
pub enum LogTail {
    Loading,
    Loaded(SharedString),
    Missing,
    Failed(Said),
}

/// Which result a log belongs to: the commit that was checked and when the check ended.
type ResultId = (String, String);

/// A log that was requested, and the result it is the log of.
struct OpenLog {
    tail: LogTail,
    result: Option<ResultId>,
}

/// The accounts an agent can start with, once `box accounts` answered.
#[derive(Debug, Clone, PartialEq)]
pub enum Accounts {
    Loading,
    Loaded(Vec<Account>),
    Failed(Said),
}

/// Actions in flight, their errors, and the log tails read so far.
pub struct Actions {
    source: Source,
    facts: Entity<Facts>,
    running: HashSet<ActionKey>,
    /// Actions `box` finished, with when, until facts read after them arrive.
    settling: HashMap<ActionKey, Instant>,
    /// The failure of the last run of an action: Sakura's sentence, and the line `box` wrote when there
    /// is one.
    errors: HashMap<ActionKey, (Said, Option<String>)>,
    queued_at: HashMap<(String, String), Instant>,
    /// The log tails requested, each bound to the result it was read for.
    logs: HashMap<(String, String), OpenLog>,
    accounts: Accounts,
    /// When the read that brought `accounts` was asked: a login waits for one asked after it ended.
    accounts_asked_at: Option<Instant>,
    /// Whether this `box` can cancel a check; false until it has said so.
    can_cancel: bool,
    /// The app's own run of the checks that wait.
    queue_run: QueueRun,
    _facts_observer: Subscription,
    _status_read: Subscription,
    _quit: Subscription,
}

/// What is said when a run of the checks that wait left them waiting.
const QUEUE_STALLED: &str = "verify.queue.stalled";

/// The app's own run of the checks that wait (`box runner --once`): one at a time, started when the facts
/// say requests wait and no runner is alive, never with nothing waiting.
#[derive(Default)]
struct QueueRun {
    is_running: bool,
    /// When the last run ended: the next one waits for a status read after it.
    ended_at: Option<Instant>,
    /// The requests the last run was started for, and the warning or the failure it ended with.
    last: Option<(Vec<QueuedRequest>, QueueOutcome)>,
    /// Why the requests that wait are not run: said in "Checks" until the person asks again or nothing
    /// waits any more.
    stalled: Option<Failure>,
}

/// A failure as it is said: the sentence, and the line `box` wrote when there is one.
type Failure = (Said, Option<String>);
/// How a run of the checks that wait ended: the warning `box` ended with, or its failure.
type QueueOutcome = Result<Option<String>, Failure>;

impl Actions {
    /// No action yet; the accounts are read once now.
    pub fn new(source: Source, facts: Entity<Facts>, cx: &mut Context<Self>) -> Self {
        let facts_observer = cx.observe(&facts, |actions, _, cx| {
            actions.settle(cx);
            actions.reload_stale_logs(cx);
            actions.keep_queue_moving(cx);
        });
        let quit = cx.on_app_quit(|actions, _| {
            actions.source.end_queue();
            async {}
        });
        let status_read = cx.subscribe(&facts, |actions, facts, _: &StatusRead, cx| {
            let facts = facts.read(cx);
            actions.forget_missing(|task| facts.has_task(task));
        });
        let mut actions = Self {
            source,
            facts,
            running: HashSet::new(),
            settling: HashMap::new(),
            errors: HashMap::new(),
            queued_at: HashMap::new(),
            logs: HashMap::new(),
            accounts: Accounts::Loading,
            accounts_asked_at: None,
            can_cancel: false,
            queue_run: QueueRun::default(),
            _facts_observer: facts_observer,
            _status_read: status_read,
            _quit: quit,
        };
        actions.reload_accounts(cx);
        actions.ask_can_cancel(cx);
        actions
    }

    /// Whether a queued or running check can be cancelled: "Cancel" is offered only then.
    pub fn can_cancel(&self) -> bool {
        self.can_cancel
    }

    /// Whether `key` runs now, or `box` finished it and facts read since have not arrived yet: either way it
    /// shows its pending word and cannot be pressed.
    pub fn is_running(&self, key: &ActionKey) -> bool {
        self.running.contains(key) || self.settling.contains_key(key)
    }

    /// The error of the last run of `key`, until it runs again.
    pub fn error(&self, key: &ActionKey) -> Option<&Said> {
        self.errors.get(key).map(|(said, _)| said)
    }

    /// What `box` itself wrote when the last run of `key` failed, for "Dettagli".
    pub fn error_details(&self, key: &ActionKey) -> Option<&str> {
        self.errors.get(key)?.1.as_deref()
    }

    /// When `box` accepted a verification of `verb` for `task`, if no runner state read since shows it.
    pub fn queued_at(&self, task: &str, verb: &str) -> Option<Instant> {
        self.queued_at
            .get(&(task.to_string(), verb.to_string()))
            .copied()
    }

    /// The log tail of `verb` for `task`, if one was requested.
    pub fn log(&self, task: &str, verb: &str) -> Option<&LogTail> {
        let open = self.logs.get(&(task.to_string(), verb.to_string()))?;
        Some(&open.tail)
    }

    /// The accounts for "Start agent".
    pub fn accounts(&self) -> &Accounts {
        &self.accounts
    }

    /// The accounts as a read asked after `at` found them; `None` until such a read has answered.
    pub fn accounts_asked_after(&self, at: Instant) -> Option<&Accounts> {
        self.accounts_asked_at
            .is_some_and(|asked| asked > at)
            .then_some(&self.accounts)
    }

    /// Why the checks that wait are not run, when the app's run of them left them: the sentence and the
    /// line `box` wrote.
    pub fn queue_stalled(&self) -> Option<(&Said, Option<&str>)> {
        let (said, details) = self.queue_run.stalled.as_ref()?;
        Some((said, details.as_deref()))
    }

    /// "Try again": runs the checks that wait again, whatever the last run said.
    pub fn retry_queue(&mut self, cx: &mut Context<Self>) {
        self.queue_run = QueueRun::default();
        self.keep_queue_moving(cx);
        cx.notify();
    }

    /// Keeps the queue of checks moving: when the facts say requests wait and no runner is alive, starts
    /// one run of them, never two at once and never with nothing waiting. A run that left the very
    /// requests it was started for is not started again by itself: why is said in "Checks".
    fn keep_queue_moving(&mut self, cx: &mut Context<Self>) {
        let facts = self.facts.read(cx);
        let Some(runner) = facts.runner() else {
            return;
        };
        let read_at = facts.status_started_at();
        let is_alive = runner.state == RunnerPhase::Running;
        let waiting = runner.queue.clone();
        if is_alive || waiting.is_empty() {
            // Nothing waits for a runner: what the last run said is no longer true.
            let had_stalled = self.queue_run.stalled.take().is_some();
            self.queue_run.last = None;
            if had_stalled {
                cx.notify();
            }
            return;
        }
        let run = &self.queue_run;
        let is_read_since = run
            .ended_at
            .is_none_or(|ended| read_at.is_some_and(|read| read > ended));
        if run.is_running || !is_read_since || run.stalled.is_some() {
            return;
        }
        let left = run.last.as_ref().filter(|(asked, _)| *asked == waiting);
        match left.map(|(_, ended)| why_left(ended)) {
            Some(why) => {
                self.queue_run.stalled = Some(why);
                cx.notify();
            }
            None => self.run_queue(waiting, cx),
        }
    }

    /// Starts one run of the checks that wait, for the requests `waiting` now.
    fn run_queue(&mut self, waiting: Vec<QueuedRequest>, cx: &mut Context<Self>) {
        let Some(pending) = self.source.run_queue() else {
            return;
        };
        self.queue_run = QueueRun {
            is_running: true,
            ..QueueRun::default()
        };
        cx.spawn(async move |this, cx| {
            let (at, ended) = ended_of(pending.await);
            drop(this.update(cx, |actions, cx| {
                actions.queue_run = QueueRun {
                    ended_at: Some(at),
                    last: Some((waiting, ended)),
                    ..QueueRun::default()
                };
                actions.facts.update(cx, |facts, cx| facts.refresh_all(cx));
                cx.notify();
            }));
        })
        .detach();
        cx.notify();
    }

    /// Queues `verb` for `task` with the runner.
    pub fn verify(&mut self, task: String, verb: String, cx: &mut Context<Self>) {
        let key = ActionKey::Verify {
            task: task.clone(),
            verb: verb.clone(),
        };
        let call = (task.clone(), verb.clone());
        let start = move |source: &Source| source.verify(call.0, call.1);
        self.run(key, start, cx, move |actions, (), _| {
            actions.queued_at.insert((task, verb), Instant::now());
        });
    }

    /// Cancels the queued or running check of `verb` for `task`. It stays pending until a runner state
    /// read after `box` answered arrives, so the row never offers the old state again.
    pub fn cancel(&mut self, task: String, verb: String, cx: &mut Context<Self>) {
        let key = ActionKey::cancel(&task, &verb);
        let settled = key.clone();
        let call = (task.clone(), verb.clone());
        let start = move |source: &Source| source.cancel(call.0, call.1);
        self.run(key, start, cx, move |actions, _was_cancelled, cx| {
            actions.queued_at.remove(&(task, verb));
            actions.begin_settling(settled, cx);
            actions.facts.update(cx, |facts, cx| facts.refresh_all(cx))
        });
    }

    /// Starts the creation of `task` as `lavoro` says, its project told first to use `account` when one
    /// is given. `box new` runs as a job: this answers as soon as it started. The row is listed at once
    /// from what the job knows, as being created; its facts are asked then and fill it in when `box`
    /// answers, however long that takes.
    pub fn new_task(
        &mut self,
        task: String,
        lavoro: NewLavoro,
        account: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let key = ActionKey::new_task(&task);
        let name = task.clone();
        let start = move |source: &Source| source.new_task(name, lavoro, account);
        self.run(key, start, cx, move |actions, (), cx| {
            if let Some(row) = actions.source.task_being_created(&task) {
                actions
                    .facts
                    .update(cx, |facts, cx| facts.list_being_created(row, cx));
            }
            actions.refresh(task, cx)
        });
    }

    /// Takes the work of `task`.
    pub fn take(&mut self, task: String, cx: &mut Context<Self>) {
        let name = task.clone();
        let key = ActionKey::Take { task: task.clone() };
        self.run(
            key,
            move |source| source.take(name),
            cx,
            move |actions, _sha, cx| {
                actions.begin_settling(ActionKey::take(&task), cx);
                actions.refresh(task, cx)
            },
        );
    }

    /// Stops the agent of `task`.
    pub fn stop_agent(&mut self, task: String, cx: &mut Context<Self>) {
        let name = task.clone();
        let key = ActionKey::stop_agent(&task);
        self.run(
            key,
            move |source| source.stop_agent(name),
            cx,
            move |actions, (), cx| {
                actions.begin_settling(ActionKey::stop_agent(&task), cx);
                actions.refresh(task, cx)
            },
        );
    }

    /// Removes `task`; `force` discards work that was not taken.
    pub fn remove(&mut self, task: String, force: bool, cx: &mut Context<Self>) {
        let key = ActionKey::remove(&task);
        let settled = key.clone();
        let start = move |source: &Source| source.remove_task(task, force);
        self.run(key, start, cx, move |actions, (), cx| {
            actions.begin_settling(settled, cx);
            actions.facts.update(cx, |facts, cx| facts.refresh_all(cx))
        });
    }

    /// Reads the log tail of `verb` for `task` again. It is kept as the log of the result the facts show
    /// now; an answer that arrives after a read for a newer result began is dropped.
    pub fn load_log(&mut self, task: String, verb: String, cx: &mut Context<Self>) {
        let log_key = (task.clone(), verb.clone());
        let result = self.result_id(&task, &verb, cx);
        let open = OpenLog {
            tail: LogTail::Loading,
            result: result.clone(),
        };
        self.logs.insert(log_key.clone(), open);
        let pending = self.source.result_log(task, verb);
        cx.spawn(async move |this, cx| {
            let tail = log_tail(pending.await);
            drop(this.update(cx, |actions, cx| {
                let open = actions.logs.get_mut(&log_key);
                if let Some(open) = open.filter(|open| open.result == result) {
                    open.tail = tail;
                    cx.notify();
                }
            }));
        })
        .detach();
        cx.notify();
    }

    /// Puts away the log of `verb` for `task`: it was closed.
    pub fn forget_log(&mut self, task: &str, verb: &str) {
        self.logs.remove(&(task.to_string(), verb.to_string()));
    }

    /// Reads again every log kept whose check has another result than the one the log was read for: the
    /// row above an open log and the log under it are always the same run's.
    fn reload_stale_logs(&mut self, cx: &mut Context<Self>) {
        let is_stale = |(task, verb): &(String, String), open: &OpenLog| {
            self.result_id(task, verb, cx) != open.result
        };
        let stale = self.logs.iter().filter(|(key, open)| is_stale(key, open));
        let stale: Vec<(String, String)> = stale.map(|(key, _)| key.clone()).collect();
        for (task, verb) in stale {
            self.load_log(task, verb, cx);
        }
    }

    /// The result of `verb` the facts show for `task` now.
    fn result_id(&self, task: &str, verb: &str, cx: &gpui::App) -> Option<ResultId> {
        let facts = self.facts.read(cx);
        let result = facts.row(task)?.task.results.get(verb)?;
        Some((result.sha.clone(), result.finished_at.clone()))
    }

    /// Reads the accounts again (after a login); the last list stays until `box` answers.
    pub fn reload_accounts(&mut self, cx: &mut Context<Self>) {
        let asked_at = Instant::now();
        let pending = self.source.accounts();
        cx.spawn(async move |this, cx| {
            let accounts = match pending.await {
                Ok(accounts) => Accounts::Loaded(accounts),
                Err(error) => Accounts::Failed(error.said),
            };
            drop(this.update(cx, |actions, cx| {
                // An answer to an older question never replaces a newer one's.
                if actions.accounts_asked_at.is_none_or(|last| asked_at > last) {
                    actions.accounts = accounts;
                    actions.accounts_asked_at = Some(asked_at);
                }
                cx.notify();
            }));
        })
        .detach();
    }

    /// Asks once whether this `box` can cancel a check. A failed question leaves "Cancel" not offered.
    fn ask_can_cancel(&mut self, cx: &mut Context<Self>) {
        let pending = self.source.can_cancel();
        cx.spawn(async move |this, cx| {
            let can_cancel = match pending.await {
                Ok(can_cancel) => can_cancel,
                Err(error) => {
                    eprintln!("sakura: whether box can cancel: {error}");
                    false
                }
            };
            drop(this.update(cx, |actions, cx| {
                actions.can_cancel = can_cancel;
                cx.notify();
            }));
        })
        .detach();
    }

    /// Starts the call as `key` unless `key` already runs (then `start` is never called, so `box` is not
    /// asked twice); `on_ok` applies a success.
    fn run<T: 'static>(
        &mut self,
        key: ActionKey,
        start: impl FnOnce(&Source) -> Pending<T>,
        cx: &mut Context<Self>,
        on_ok: impl FnOnce(&mut Self, T, &mut Context<Self>) + 'static,
    ) {
        if !self.running.insert(key.clone()) {
            return;
        }
        let pending = start(&self.source);
        self.errors.remove(&key);
        cx.spawn(async move |this, cx| {
            let result = pending.await;
            drop(this.update(cx, |actions, cx| {
                actions.running.remove(&key);
                match result {
                    Ok(value) => on_ok(actions, value, cx),
                    Err(error) => {
                        let failure = (error.said, error.details);
                        drop(actions.errors.insert(key, failure))
                    }
                }
                cx.notify();
            }));
        })
        .detach();
        cx.notify();
    }

    /// Drops what is kept for tasks that no longer exist. Actions still running are left to finish.
    fn forget_missing(&mut self, exists: impl Fn(&str) -> bool) {
        retain_tasks(&mut self.settling, ActionKey::task, &exists);
        retain_tasks(&mut self.errors, ActionKey::task, &exists);
        retain_tasks(&mut self.queued_at, |(task, _)| task.as_str(), &exists);
        retain_tasks(&mut self.logs, |(task, _)| task.as_str(), &exists);
    }

    /// Keeps `key` pending from now until facts read after it arrive, or [`SETTLE_LIMIT`] passes.
    fn begin_settling(&mut self, key: ActionKey, cx: &mut Context<Self>) {
        let finished_at = Instant::now();
        self.settling.insert(key.clone(), finished_at);
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SETTLE_LIMIT).await;
            drop(this.update(cx, |actions, cx| {
                if actions.settling.get(&key) == Some(&finished_at) {
                    actions.settling.remove(&key);
                    let unconfirmed = (Said::from(UNCONFIRMED), None);
                    actions.errors.insert(key, unconfirmed);
                    cx.notify();
                }
            }));
        })
        .detach();
    }

    /// Ends the settling of every action whose task has facts read after it finished.
    fn settle(&mut self, cx: &mut Context<Self>) {
        if self.settling.is_empty() {
            return;
        }
        let facts = self.facts.read(cx);
        let status_started_at = facts.status_started_at();
        let before = self.settling.len();
        self.settling.retain(|key, finished_at| {
            let task_requested_at = facts.row(key.task()).map(|row| row.requested_at);
            !is_settled(*finished_at, task_requested_at, status_started_at)
        });
        if self.settling.len() != before {
            cx.notify();
        }
    }

    fn refresh(&self, task: String, cx: &mut Context<Self>) {
        self.facts
            .update(cx, |facts, cx| facts.refresh_task(task, cx));
    }
}

/// `box result-log` exits 3 when the verb has no result: that is not an error.
fn log_tail(result: Result<String, BridgeError>) -> LogTail {
    match result {
        Ok(text) => LogTail::Loaded(without_colour_codes(&text).into()),
        Err(error) if error.code == ErrorCode::NotFound => LogTail::Missing,
        Err(error) => LogTail::Failed(error.said),
    }
}

/// `box result-log` removes control characters, so colour codes arrive without their ESC byte as text like
/// `[32m` or `[2m`. Those remnants (a `[`, digits and semicolons, an `m`) are removed so the log reads as text.
fn without_colour_codes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('[') {
        out.push_str(&rest[..start]);
        let tail = &rest[start + 1..];
        let code_len = tail
            .find(|c: char| !(c.is_ascii_digit() || c == ';'))
            .unwrap_or(tail.len());
        if code_len > 0 && tail[code_len..].starts_with('m') {
            rest = &tail[code_len + 1..];
        } else {
            out.push('[');
            rest = tail;
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colour_code_remnants_are_removed_and_other_brackets_kept() {
        let line = "[32m✓[39m [2mTest Files [22m [1m[32m1074 passed[39m[22m";
        assert_eq!(without_colour_codes(line), "✓ Test Files  1074 passed");
        assert_eq!(
            without_colour_codes("array[0] and [m] stay"),
            "array[0] and [m] stay"
        );
        assert_eq!(without_colour_codes("ends with ["), "ends with [");
    }

    #[test]
    fn facts_read_before_the_action_finished_do_not_settle_it() {
        let finished = Instant::now();
        let before = finished - Duration::from_secs(1);
        assert!(!is_settled(finished, Some(before), Some(before)));
        assert!(!is_settled(finished, None, None));
        assert!(!is_settled(finished, Some(finished), None));
    }

    #[test]
    fn a_task_read_or_a_status_started_after_the_action_settles_it() {
        let finished = Instant::now();
        let after = finished + Duration::from_millis(1);
        assert!(is_settled(finished, Some(after), None));
        assert!(
            is_settled(finished, None, Some(after)),
            "a removed task has no row"
        );
        assert!(is_settled(
            finished,
            Some(finished - Duration::from_secs(1)),
            Some(after)
        ));
    }

    #[test]
    fn every_action_names_its_task() {
        assert_eq!(ActionKey::remove("m2").task(), "m2");
        let verify = ActionKey::Verify {
            task: "m2".into(),
            verb: "test".into(),
        };
        assert_eq!(verify.task(), "m2");
        assert_eq!(ActionKey::cancel("m3", "test").task(), "m3");
    }

    #[test]
    fn missing_result_log_is_not_an_error() {
        let missing = BridgeError::new(ErrorCode::NotFound, Key("none"));
        assert_eq!(log_tail(Err(missing)), LogTail::Missing);
        let failed = BridgeError::failed(Key("boom"));
        assert_eq!(log_tail(Err(failed)), LogTail::Failed(Said::new("boom")));
        assert_eq!(log_tail(Ok("a\nb".into())), LogTail::Loaded("a\nb".into()));
    }
}
