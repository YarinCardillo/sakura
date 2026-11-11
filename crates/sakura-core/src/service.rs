//! The bridge, minus Tauri: every call the window can make that goes through `box`, with its arguments
//! validated first. Commands in `commands.rs` are thin wrappers around these methods, so the logic is
//! testable with a fake `box` and no window.

use crate::box_cli::{ask_to_end, BoxCli, Limit};
use crate::error::{BridgeError, BridgeResult, ErrorCode};
use crate::jobs::{self, JobNotifier, Jobs, NewJob, PendingJob};
use crate::models::{
    Account, BoxStatus, Commit, JournalEvent, Place, Preparing, Task, TaskSummary,
};
use crate::projects::NewLavoro;
use crate::said::Said;
use crate::single_flight::SingleFlight;
use crate::validate;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError};

/// Called after an action with the task it touched (`None` for "anything").
pub type FactsNotifier = Arc<dyn Fn(Option<String>) + Send + Sync>;

/// Runs bridge calls through `box`.
#[derive(Clone)]
pub struct Core {
    cli: BoxCli,
    jobs: Jobs,
    notify: FactsNotifier,
    status_flight: Arc<SingleFlight<BoxStatus>>,
    facts_flights: Arc<Mutex<HashMap<String, Arc<SingleFlight<Task>>>>>,
    /// The process of the run of the checks that wait which this app started, while it runs.
    queue_run: Arc<Mutex<Option<u32>>>,
}

impl Core {
    /// Builds the service around a `box` handle. `notify` is called after every action and job change.
    pub fn new(cli: BoxCli, notify: FactsNotifier) -> Self {
        Self {
            cli,
            jobs: Jobs::default(),
            notify,
            status_flight: Arc::default(),
            facts_flights: Arc::default(),
            queue_run: Arc::default(),
        }
    }

    // ----- reads ---------------------------------------------------------------------------------

    /// `box status --json`, plus tasks still being prepared by a `task_new` job.
    pub async fn status(&self) -> BridgeResult<BoxStatus> {
        let status = self.box_status().await?;
        self.forget_flights_outside(&status);
        Ok(self.with_jobs(status))
    }

    /// `box tasks --json`: the names of the tasks, read without starting a container. `None` when this
    /// `box` does not have the command.
    pub async fn tasks(&self) -> BridgeResult<Option<Vec<TaskSummary>>> {
        match self.cli.run_json(&["tasks", "--json"], Limit::Names).await {
            Ok(tasks) => Ok(Some(tasks)),
            Err(error) if error.code == ErrorCode::Unsupported => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Accounts and whether each is logged in.
    pub async fn accounts(&self) -> BridgeResult<Vec<Account>> {
        self.cli
            .run_json(&["accounts", "--json"], Limit::Read)
            .await
    }

    /// Journal events, newest last, optionally only those since a timestamp.
    pub async fn journal(&self, since: Option<&str>) -> BridgeResult<Vec<JournalEvent>> {
        let mut args = vec!["journal", "--json"];
        if let Some(since) = since {
            args.extend(["--since", validate::since(since)?]);
        }
        self.cli.run_json(&args, Limit::Read).await
    }

    /// The row of a task whose creation is tracked, as known without asking `box`: its name, its profile
    /// and the newest line of its creation. For showing it at once; [`Service::task_facts`] fills it in.
    pub fn task_being_created(&self, task: &str) -> Option<Task> {
        self.jobs.pending(task).map(synthetic_task)
    }

    /// Facts of one task. A task still being prepared answers from its job.
    pub async fn task_facts(&self, task: &str) -> BridgeResult<Task> {
        validate::task(task)?;
        match self.box_facts(task).await {
            Ok(mut facts) => {
                facts.preparing = self.attached_job(task);
                Ok(facts)
            }
            Err(error) => self.jobs.pending(task).map(synthetic_task).ok_or(error),
        }
    }

    /// Unified patch: one commit, or the merge base against the working tree.
    pub async fn task_diff(&self, task: &str, commit: Option<&str>) -> BridgeResult<String> {
        validate::task(task)?;
        let mut args = vec!["diff", task];
        if let Some(commit) = commit {
            args.extend(["--commit", validate::sha(commit)?]);
        }
        self.cli.run_text(&args, Limit::Diff).await
    }

    /// Commits ahead of the base, newest first.
    pub async fn task_log(&self, task: &str) -> BridgeResult<Vec<Commit>> {
        validate::task(task)?;
        self.cli
            .run_json(&["log", task, "--json"], Limit::Read)
            .await
    }

    /// Tail of the log of the last result for a verb.
    pub async fn result_log(&self, task: &str, verb: &str) -> BridgeResult<String> {
        validate::task(task)?;
        validate::verb(verb)?;
        self.cli
            .run_text(&["result-log", task, verb], Limit::Read)
            .await
    }

    // ----- actions -------------------------------------------------------------------------------

    /// Starts `box new` for `lavoro` as a tracked job and returns at once. Calling it again while it runs
    /// does nothing.
    pub fn task_new(&self, name: &str, lavoro: &NewLavoro) -> BridgeResult<()> {
        let arguments = lavoro.arguments(name)?;
        self.cli.check()?;
        let notify = Arc::clone(&self.notify);
        let task = name.to_string();
        let on_change: JobNotifier = Arc::new(move || notify(Some(task.clone())));
        let job = NewJob {
            name: name.to_string(),
            project: lavoro.project.clone(),
            arguments,
        };
        jobs::start_new(self.cli.clone(), self.jobs.clone(), job, on_change);
        (self.notify)(Some(name.to_string()));
        Ok(())
    }

    /// Takes the task's work and returns the sha taken.
    pub async fn task_take(&self, task: &str) -> BridgeResult<String> {
        validate::task(task)?;
        let output = self.cli.run_text(&["take", task], Limit::Take).await?;
        let sha = output
            .lines()
            .rev()
            .map(str::trim)
            .find(|line| !line.is_empty());
        let sha = sha
            .and_then(|line| validate::sha(line).ok())
            .ok_or_else(|| BridgeError::failed(Said::new("error.take.no_commit")))?;
        self.touched(task);
        Ok(sha.to_string())
    }

    /// Removes a task. A task whose creation failed and left no workspace is simply forgotten.
    pub async fn task_remove(&self, task: &str, force: bool) -> BridgeResult<()> {
        validate::task(task)?;
        if self.jobs.preparing(task).is_some_and(|p| !p.failed) {
            return Err(BridgeError::invalid(Said::new("error.task.preparing")));
        }
        if self.is_failed_leftover(task).await? {
            self.jobs.dismiss_failed(task);
            self.touched(task);
            return Ok(());
        }
        self.jobs.dismiss_failed(task);
        let args = if force {
            vec!["rm", task, "--force"]
        } else {
            vec!["rm", task]
        };
        self.cli.run_text(&args, Limit::Action).await?;
        self.touched(task);
        Ok(())
    }

    /// Queues a verification and returns without waiting for it.
    pub async fn verify_request(&self, task: &str, verb: &str) -> BridgeResult<()> {
        validate::task(task)?;
        validate::verb(verb)?;
        self.cli
            .run_text(&["verify", task, verb, "--no-wait"], Limit::Action)
            .await?;
        self.touched(task);
        Ok(())
    }

    /// Cancels the queued or running verification of `verb` for `task`. Answers whether there was one to
    /// cancel: `box` exits 78 when there was none.
    pub async fn verify_cancel(&self, task: &str, verb: &str) -> BridgeResult<bool> {
        validate::task(task)?;
        validate::verb(verb)?;
        let args = ["cancel", task, verb];
        let was_cancelled = match self.cli.run_text(&args, Limit::Action).await {
            Ok(_) => true,
            Err(error) if error.code == ErrorCode::NothingToCancel => false,
            Err(error) => return Err(error),
        };
        self.touched(task);
        Ok(was_cancelled)
    }

    /// Whether this `box` has the cancel command: asked without a task, it prints its usage (exit 2) when
    /// it does not know the command, and refuses the missing arguments when it does.
    pub async fn can_cancel(&self) -> BridgeResult<bool> {
        match self.cli.run_text(&["cancel"], Limit::Names).await {
            Err(error) if error.code == ErrorCode::Unsupported => Ok(false),
            Err(error) if is_environment_error(&error) => Err(error),
            Ok(_) | Err(_) => Ok(true),
        }
    }

    /// Runs the checks that wait, one at a time, and ends when none that can run is left
    /// (`box runner --once`): the app keeps the queue moving with it and leaves nothing running at rest.
    /// The call lasts as long as the checks do. Answers when the run ended and how: `box` warns on its
    /// last line when it left requests waiting (Docker does not answer for a lavoro in a box).
    pub async fn runner_once(&self) -> (std::time::Instant, BridgeResult<Option<String>>) {
        let started = Arc::clone(&self.queue_run);
        let on_start = move |pid| *started.lock().unwrap_or_else(PoisonError::into_inner) = pid;
        let args = ["runner", "--once"];
        let ended = self.cli.run_warned(&args, Limit::Queue, on_start).await;
        let at = std::time::Instant::now();
        *self.queue_run() = None;
        (self.notify)(None);
        (at, ended)
    }

    /// Ends the run of the checks this app started, if one runs: the app quits and leaves no runner.
    /// `box` ends the check it runs and leaves its request in the queue for the next runner.
    pub fn runner_end(&self) {
        ask_to_end(self.queue_run().take());
    }

    fn queue_run(&self) -> std::sync::MutexGuard<'_, Option<u32>> {
        self.queue_run
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Starts the runner loop in the background.
    pub async fn runner_start(&self) -> BridgeResult<()> {
        self.cli
            .run_text(&["runner", "--daemon"], Limit::Action)
            .await?;
        (self.notify)(None);
        Ok(())
    }

    /// Stops the runner loop.
    pub async fn runner_stop(&self) -> BridgeResult<()> {
        self.cli
            .run_text(&["runner", "--stop"], Limit::Action)
            .await?;
        (self.notify)(None);
        Ok(())
    }

    /// Stops the task's agent container.
    pub async fn agent_stop(&self, task: &str) -> BridgeResult<()> {
        validate::task(task)?;
        self.cli.run_text(&["stop", task], Limit::Action).await?;
        self.touched(task);
        Ok(())
    }

    // ----- helpers -------------------------------------------------------------------------------

    /// The `box` handle, for modules that run terminals.
    pub fn cli(&self) -> &BoxCli {
        &self.cli
    }

    /// `box status --json`, sharing a run with concurrent callers.
    async fn box_status(&self) -> BridgeResult<BoxStatus> {
        let run = || async { self.cli.run_json(&["status", "--json"], Limit::Slow).await };
        self.status_flight.run(run).await
    }

    /// `box facts <task> --json`, sharing a run with concurrent callers for the same task.
    async fn box_facts(&self, task: &str) -> BridgeResult<Task> {
        let flight = {
            let mut flights = self
                .facts_flights
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            Arc::clone(flights.entry(task.to_string()).or_default())
        };
        let run = || async {
            self.cli
                .run_json(&["facts", task, "--json"], Limit::Slow)
                .await
        };
        flight.run(run).await
    }

    /// Drops the facts flight of every task `status` no longer lists, unless a read is using it.
    fn forget_flights_outside(&self, status: &BoxStatus) {
        let listed: HashSet<&str> = status.tasks.iter().map(|task| task.name.as_str()).collect();
        let mut flights = self
            .facts_flights
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        flights
            .retain(|task, flight| listed.contains(task.as_str()) || Arc::strong_count(flight) > 1);
    }

    fn touched(&self, task: &str) {
        (self.notify)(Some(task.to_string()));
    }

    /// The job of a task `box` lists: its workspace exists.
    fn attached_job(&self, task: &str) -> Option<Preparing> {
        let preparing = self.jobs.preparing(task)?;
        Some(Preparing {
            has_workspace: true,
            ..preparing
        })
    }

    fn with_jobs(&self, mut status: BoxStatus) -> BoxStatus {
        for task in &mut status.tasks {
            task.preparing = self.attached_job(&task.name);
        }
        let listed: HashSet<&str> = status.tasks.iter().map(|task| task.name.as_str()).collect();
        let pending = self.jobs.pending_outside(&listed);
        status.tasks.extend(pending.into_iter().map(synthetic_task));
        status
    }

    async fn is_failed_leftover(&self, task: &str) -> BridgeResult<bool> {
        if !self.jobs.preparing(task).is_some_and(|p| p.failed) {
            return Ok(false);
        }
        let status = self.box_status().await?;
        Ok(status.tasks.iter().all(|listed| listed.name != task))
    }
}

/// The entry for a task whose workspace does not exist yet: unreadable, with its progress attached.
fn synthetic_task(job: PendingJob) -> Task {
    Task {
        name: job.name,
        profile: job.profile,
        place: Place::default(),
        folder: None,
        base: String::new(),
        base_sha: String::new(),
        readable: false,
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
        preparing: Some(job.preparing),
        disk: None,
    }
}

/// Whether an error means `box` itself is unusable (as opposed to one command failing).
pub fn is_environment_error(error: &BridgeError) -> bool {
    matches!(error.code, ErrorCode::BoxMissing | ErrorCode::DockerDown)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A lavoro of `project` with nothing else said.
    fn of(project: &str) -> NewLavoro {
        NewLavoro {
            project: project.into(),
            base: None,
            place: None,
        }
    }
    use crate::box_cli::test_support::fake_box;
    use std::path::Path;
    use std::sync::Mutex;
    use std::time::Duration;

    const STATUS: &str = include_str!("../tests/fixtures/status.json");
    const SHA: &str = "9b1e0c3a5d7f2468ace013579bdf02468ace1357";

    struct Rig {
        dir: tempfile::TempDir,
        core: Core,
        events: Arc<Mutex<Vec<Option<String>>>>,
    }

    /// A fake box that records every invocation (one line per call, arguments separated by `|`) in
    /// `calls.log`, answers the read commands from fixtures and takes `extra` for the rest.
    fn rig(extra: &str) -> Rig {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("status.json"), STATUS).unwrap();
        let body = format!(
            r#"d="$(dirname "$0")"
first=1; for a in "$@"; do if [ $first = 1 ]; then printf '%s' "$a" >> "$d/calls.log"; first=0; else printf '|%s' "$a" >> "$d/calls.log"; fi; done; echo >> "$d/calls.log"
case "$1" in
{extra}
  status) cat "$d/status.json" ;;
  facts) echo "facts failed" >&2; exit 1 ;;
  take) echo "taking"; echo "{SHA}" ;;
  *) echo ok ;;
esac"#
        );
        let cli = fake_box(dir.path(), &body);
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&events);
        let notify: FactsNotifier = Arc::new(move |task| sink.lock().unwrap().push(task));
        Rig {
            core: Core::new(cli, notify),
            dir,
            events,
        }
    }

    fn calls(dir: &Path) -> Vec<String> {
        std::fs::read_to_string(dir.join("calls.log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    #[cfg_attr(windows, ignore = "Unix shell fixture")]
    #[tokio::test]
    async fn a_run_of_the_queue_answers_with_the_warning_box_ended_with_or_its_failure() {
        let left = "box: Docker is not reachable: the requests of tasks in a box stay queued";
        let warned = rig(&format!("  runner) echo '{left}' >&2 ;;"));
        let (_, ended) = warned.core.runner_once().await;
        assert_eq!(ended.unwrap().as_deref(), Some(left));
        assert_eq!(calls(warned.dir.path()), vec!["runner|--once"]);
        assert_eq!(
            *warned.events.lock().unwrap(),
            vec![None],
            "the facts changed"
        );

        let clean = rig("  runner) ;;");
        assert_eq!(clean.core.runner_once().await.1.unwrap(), None);

        let refused = rig("  runner) echo 'box: another runner is already running' >&2; exit 1 ;;");
        let error = refused.core.runner_once().await.1.unwrap_err();
        let details = error.details.expect("the line of the service");
        assert!(details.contains("another runner is already running"));
    }

    // This fixture probes a Unix PID with signal 0; Windows uses the WSL process-tree boundary.
    #[cfg(unix)]
    #[tokio::test]
    async fn ending_the_run_of_the_queue_leaves_no_runner() {
        let rig = rig(r#"  runner) echo $$ > "$d/runner.pid"; exec sleep 30 ;;"#);
        let core = rig.core.clone();
        let run = tokio::spawn(async move { core.runner_once().await });
        let pid_file = rig.dir.path().join("runner.pid");
        let pid = loop {
            let written = std::fs::read_to_string(&pid_file).ok();
            if let Some(pid) = written.and_then(|text| text.trim().parse::<i32>().ok()) {
                break pid;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        };
        rig.core.runner_end();
        let (_, ended) = run.await.unwrap();
        assert!(ended.is_err(), "a run that was ended is not a clean end");
        // SAFETY: signal 0 sends nothing: it only asks whether the process exists.
        let is_alive = unsafe { libc::kill(pid, 0) } == 0;
        assert!(!is_alive, "no runner is left");
        rig.core.runner_end();
    }

    #[cfg_attr(windows, ignore = "Unix shell fixture")]
    #[tokio::test]
    async fn status_parses_the_fixture() {
        let rig = rig("");
        let status = rig.core.status().await.unwrap();
        assert_eq!(status.tasks.len(), 4);
        assert_eq!(calls(rig.dir.path()), vec!["status|--json"]);
    }

    #[cfg_attr(windows, ignore = "Unix shell fixture")]
    #[tokio::test]
    async fn tasks_reads_the_inventory() {
        let rig =
            rig(r#"  tasks) echo '[{"name":"m1","profile":"p","base":"main","taken":null}]' ;;"#);
        let tasks = rig.core.tasks().await.unwrap().expect("the inventory");
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].name, "m1");
        assert_eq!(calls(rig.dir.path()), vec!["tasks|--json"]);
    }

    #[cfg_attr(windows, ignore = "Unix shell fixture")]
    #[tokio::test]
    async fn tasks_on_a_box_without_the_command_is_not_available_and_not_an_error() {
        let rig = rig("  tasks) echo 'usage: box <command>' >&2; exit 2 ;;");
        assert_eq!(rig.core.tasks().await.unwrap(), None);
        let broken = rig_failing_tasks();
        assert_eq!(
            broken.core.tasks().await.unwrap_err().code,
            ErrorCode::BoxFailed
        );
    }

    fn rig_failing_tasks() -> Rig {
        rig("  tasks) echo 'box: cannot read the tasks' >&2; exit 1 ;;")
    }

    #[cfg_attr(windows, ignore = "Unix shell fixture")]
    #[tokio::test]
    async fn a_burst_of_status_calls_starts_few_box_runs() {
        let rig = rig("  status) sleep 0.4; cat \"$d/status.json\" ;;");
        let calls_before = calls(rig.dir.path()).len();
        let results = futures_burst(&rig.core).await;
        assert!(results.iter().all(Result::is_ok));
        let runs = calls(rig.dir.path()).len() - calls_before;
        assert!(runs <= 2, "{runs} runs");
    }

    async fn futures_burst(core: &Core) -> Vec<BridgeResult<BoxStatus>> {
        let mut handles = Vec::new();
        for _ in 0..6 {
            let core = core.clone();
            handles.push(tokio::spawn(async move { core.status().await }));
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let mut out = Vec::new();
        for handle in handles {
            out.push(handle.await.unwrap());
        }
        out
    }

    #[cfg_attr(windows, ignore = "Unix shell fixture")]
    #[tokio::test]
    async fn bad_arguments_never_reach_the_process() {
        let rig = rig("");
        let core = &rig.core;
        assert!(core.task_diff("../etc", None).await.is_err());
        assert!(core.task_diff("m1", Some("--output=/x")).await.is_err());
        assert!(core.task_log("M1").await.is_err());
        assert!(core.result_log("m1", "1bad").await.is_err());
        assert!(core.task_take("-rf").await.is_err());
        assert!(core.task_remove("a b", true).await.is_err());
        assert!(core.verify_request("m1", "te;st").await.is_err());
        assert!(core.agent_stop("$(id)").await.is_err());
        assert!(core.journal(Some("yesterday; rm")).await.is_err());
        assert!(core.task_facts("").await.is_err());
        assert!(core.task_new("ok-name", &of("bad profile")).is_err());
        let error = core.task_new("Bad", &of("p")).unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidArgument);
        assert!(calls(rig.dir.path()).is_empty());
        assert!(rig.events.lock().unwrap().is_empty());
    }

    #[cfg_attr(windows, ignore = "Unix shell fixture")]
    #[tokio::test]
    async fn diff_and_log_calls_build_the_documented_vectors() {
        let rig = rig("");
        rig.core.task_diff("m1", None).await.unwrap();
        rig.core.task_diff("m1", Some("abc1234")).await.unwrap();
        rig.core.result_log("m1", "test").await.unwrap();
        rig.core.verify_request("m1", "test").await.unwrap();
        rig.core
            .journal(Some("2026-11-10T00:00:00Z"))
            .await
            .unwrap_err();
        rig.core.runner_start().await.unwrap();
        rig.core.runner_stop().await.unwrap();
        rig.core.agent_stop("m1").await.unwrap();
        rig.core.task_remove("m1", true).await.unwrap();
        assert_eq!(
            calls(rig.dir.path()),
            vec![
                "diff|m1",
                "diff|m1|--commit|abc1234",
                "result-log|m1|test",
                "verify|m1|test|--no-wait",
                "journal|--json|--since|2026-11-10T00:00:00Z",
                "runner|--daemon",
                "runner|--stop",
                "stop|m1",
                "rm|m1|--force",
            ]
        );
    }

    #[cfg_attr(windows, ignore = "Unix shell fixture")]
    #[tokio::test]
    async fn cancel_says_whether_there_was_a_check_to_cancel() {
        let rig = rig(
            "  cancel) [ \"$3\" = test ] && exit 0; echo 'box: nothing to cancel' >&2; exit 78 ;;",
        );
        assert!(rig.core.verify_cancel("m1", "test").await.unwrap());
        assert!(!rig.core.verify_cancel("m1", "rules").await.unwrap());
        assert_eq!(
            calls(rig.dir.path()),
            vec!["cancel|m1|test", "cancel|m1|rules"]
        );
        assert_eq!(
            rig.events.lock().unwrap().len(),
            2,
            "both read the task again"
        );
        assert!(rig.core.verify_cancel("m1", "te;st").await.is_err());
    }

    #[cfg_attr(windows, ignore = "Unix shell fixture")]
    #[tokio::test]
    async fn a_box_without_cancel_is_told_apart_from_one_that_has_it() {
        let old = rig("  cancel) echo 'usage: box <command>' >&2; exit 2 ;;");
        assert!(!old.core.can_cancel().await.unwrap());
        let error = old.core.verify_cancel("m1", "test").await.unwrap_err();
        assert_eq!(error.code, ErrorCode::Unsupported);
        let new = rig("  cancel) echo 'box: usage: box cancel <task> <verb>' >&2; exit 1 ;;");
        assert!(new.core.can_cancel().await.unwrap());
    }

    #[cfg_attr(windows, ignore = "Unix shell fixture")]
    #[tokio::test]
    async fn take_returns_the_last_stdout_line_and_notifies() {
        let rig = rig("");
        assert_eq!(rig.core.task_take("m1").await.unwrap(), SHA);
        assert_eq!(
            rig.events.lock().unwrap().as_slice(),
            &[Some("m1".to_string())]
        );
    }

    #[cfg_attr(windows, ignore = "Unix shell fixture")]
    #[tokio::test]
    async fn take_without_a_sha_is_an_error() {
        let rig = rig("  take) echo 'taken, nothing printed' ;;");
        let error = rig.core.task_take("m1").await.unwrap_err();
        assert_eq!(error.code, ErrorCode::BoxFailed);
    }

    #[cfg_attr(windows, ignore = "Unix shell fixture")]
    #[tokio::test]
    async fn failed_action_does_not_notify() {
        let rig = rig("  stop) echo 'no agent' >&2; exit 3 ;;");
        let error = rig.core.agent_stop("m1").await.unwrap_err();
        assert_eq!(error.code, ErrorCode::NotFound);
        assert!(rig.events.lock().unwrap().is_empty());
    }

    async fn wait_for_failed(core: &Core, name: &str) {
        // Waits on the job's state; the deadline only bounds a hang on a busy machine.
        let deadline = std::time::Instant::now() + Duration::from_secs(120);
        while std::time::Instant::now() < deadline {
            if core.jobs.preparing(name).is_some_and(|p| p.failed) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("job never failed");
    }

    #[cfg_attr(windows, ignore = "Unix shell fixture")]
    #[tokio::test]
    async fn new_task_appears_in_status_as_preparing_then_as_failed() {
        let rig = rig("  new) echo 'cloning'; sleep 0.4; echo 'install exploded' >&2; exit 1 ;;");
        rig.core.task_new("fresh", &of("example-app")).unwrap();
        let during = rig.core.status().await.unwrap();
        let entry = during.tasks.iter().find(|t| t.name == "fresh").unwrap();
        assert!(!entry.readable);
        assert_eq!(entry.profile, "example-app");
        assert!(!entry.preparing.as_ref().unwrap().failed);
        wait_for_failed(&rig.core, "fresh").await;
        let after = rig.core.status().await.unwrap();
        let entry = after.tasks.iter().find(|t| t.name == "fresh").unwrap();
        assert_eq!(entry.preparing.as_ref().unwrap().line, "install exploded");
        assert!(entry.preparing.as_ref().unwrap().failed);
        let json = serde_json::to_value(entry).unwrap();
        assert_eq!(json["preparing"]["failed"], true);
        assert_eq!(json["head"], serde_json::Value::Null);
    }

    #[cfg_attr(windows, ignore = "Unix shell fixture")]
    #[tokio::test]
    async fn a_failed_job_says_whether_box_lists_its_workspace() {
        let rig = rig("  new) echo 'bad' >&2; exit 1 ;;");
        let listed: BoxStatus = serde_json::from_str(STATUS).unwrap();
        let existing = listed.tasks[0].name.clone();
        rig.core.task_new(&existing, &of("p")).unwrap();
        rig.core.task_new("fresh", &of("p")).unwrap();
        wait_for_failed(&rig.core, &existing).await;
        wait_for_failed(&rig.core, "fresh").await;
        let status = rig.core.status().await.unwrap();
        let job = |name: &str| {
            let task = status.tasks.iter().find(|t| t.name == name).unwrap();
            task.preparing.clone().unwrap()
        };
        assert!(job(&existing).has_workspace);
        assert!(!job("fresh").has_workspace);
    }

    #[cfg_attr(windows, ignore = "Unix shell fixture")]
    #[tokio::test]
    async fn a_status_forgets_the_facts_flights_of_tasks_it_no_longer_lists() {
        let rig = rig("");
        rig.core.task_facts("gone").await.ok();
        let flights = |core: &Core| core.facts_flights.lock().unwrap().len();
        assert_eq!(flights(&rig.core), 1);
        rig.core.status().await.unwrap();
        assert_eq!(flights(&rig.core), 0);
    }

    #[cfg_attr(windows, ignore = "Unix shell fixture")]
    #[tokio::test]
    async fn removing_a_failed_task_without_a_workspace_only_forgets_it() {
        let rig = rig("  new) echo 'bad' >&2; exit 1 ;;");
        rig.core.task_new("fresh", &of("p")).unwrap();
        wait_for_failed(&rig.core, "fresh").await;
        rig.core.task_remove("fresh", false).await.unwrap();
        assert!(rig.core.jobs.preparing("fresh").is_none());
        assert!(!calls(rig.dir.path())
            .iter()
            .any(|call| call.starts_with("rm|")));
    }

    #[cfg_attr(windows, ignore = "Unix shell fixture")]
    #[tokio::test]
    async fn removing_a_task_still_being_prepared_is_refused() {
        let rig = rig("  new) sleep 5 ;;");
        rig.core.task_new("fresh", &of("p")).unwrap();
        let error = rig.core.task_remove("fresh", true).await.unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidArgument);
    }

    #[cfg_attr(windows, ignore = "Unix shell fixture")]
    #[tokio::test]
    async fn facts_of_a_preparing_task_come_from_its_job() {
        let rig = rig("  new) echo 'cloning'; sleep 5 ;;");
        rig.core.task_new("fresh", &of("p")).unwrap();
        let facts = rig.core.task_facts("fresh").await.unwrap();
        assert!(!facts.readable);
        assert!(facts.preparing.is_some());
        let other = rig.core.task_facts("nobody").await.unwrap_err();
        assert_eq!(other.code, ErrorCode::BoxFailed);
    }

    #[cfg_attr(windows, ignore = "Unix shell fixture")]
    #[tokio::test]
    async fn missing_box_is_reported_by_reads_and_by_new() {
        let notify: FactsNotifier = Arc::new(|_| {});
        let core = Core::new(BoxCli::at("/nonexistent/box"), notify);
        assert_eq!(core.status().await.unwrap_err().code, ErrorCode::BoxMissing);
        assert_eq!(
            core.task_new("m1", &of("p")).unwrap_err().code,
            ErrorCode::BoxMissing
        );
        assert!(is_environment_error(&core.projects().await.unwrap_err()));
    }
}
