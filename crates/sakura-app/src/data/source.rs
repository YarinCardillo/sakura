//! Where facts come from: the real `box` through `sakura-core`, or a fixture file (`SAKURA_FIXTURE`). Every call
//! runs on the tokio runtime and is awaited from GPUI's executor, so the UI thread never waits on `box`.

use super::fixture_changes;
use crate::l10n::Key;
use sakura_acp::connection::{Agent, AgentConfig, AgentEvent, Deadlines};
use sakura_core::box_cli::BoxCli;
use sakura_core::error::{BridgeError, BridgeResult, ErrorCode};
use sakura_core::models::{Account, BoxStatus, Commit, JournalEvent, Machine, Task, TaskSummary};
use sakura_core::patch::Patch;
use sakura_core::projects::{Branch, Branches, Detected, NewLavoro, NewProject, Project};
use sakura_core::said::Said;
use sakura_core::service::Core;
use sakura_core::terminal::{TerminalSink, Terminals};
use sakura_core::watcher::{self, Watcher};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio::runtime::Runtime;
use tokio::sync::mpsc::{unbounded_channel, Receiver, UnboundedReceiver, UnboundedSender};

/// `SAKURA_FIXTURE=1` loads the core's status fixture; `SAKURA_FIXTURE=<path>` loads that file instead.
pub const FIXTURE_VAR: &str = "SAKURA_FIXTURE";
/// The accounts the fixture has, as `box accounts --json` prints them.
pub const FIXTURE_ACCOUNTS_VAR: &str = "SAKURA_FIXTURE_ACCOUNTS";
/// What an action answers in fixture mode.
pub const FIXTURE_REFUSAL: Key = Key("fixture.no_box");
const BUILT_IN_FIXTURE: &str = include_str!("../../../sakura-core/tests/fixtures/status.json");

/// A call in flight on the runtime.
pub type Pending<T> = Pin<Box<dyn Future<Output = BridgeResult<T>> + Send>>;

/// What reaches the facts from outside the window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Signal {
    /// The periodic tick: time passed, nothing is known to have changed.
    Tick,
    /// Something changed: one task (its workspace, its results, an action on it), or `None` for something
    /// not tied to one task (the journal, the queue).
    Changed(Option<String>),
}

/// The signals for the facts.
pub type FactsEvents = UnboundedReceiver<Signal>;

#[derive(Clone)]
enum Kind {
    Live(Core),
    Fixture(Arc<BoxStatus>),
}

/// The facts source and the runtime its calls run on.
#[derive(Clone)]
pub struct Source {
    kind: Kind,
    runtime: Arc<Runtime>,
    /// In tests, actions are recorded and never answer instead of being refused.
    #[cfg(test)]
    recorder: Option<Arc<Recorder>>,
}

/// A machine with nothing to say: light load, no memory pressure.
#[cfg(test)]
pub fn calm_machine() -> Machine {
    Machine {
        cpus: 10,
        load1: 1.5,
        memory_pressure: sakura_core::models::MemoryPressure::Normal,
        swap_used_mb: 0,
        swap_total_mb: 0,
        disk_free_gb: 200.0,
    }
}

/// The actions a test window asked of `box`, in order, and apart from them the facts it read.
#[cfg(test)]
#[derive(Default)]
pub struct Recorder {
    calls: std::sync::Mutex<Vec<String>>,
    reads: std::sync::Mutex<Vec<String>>,
}

#[cfg(test)]
impl Recorder {
    /// The calls so far, as `verb task`.
    pub fn calls(&self) -> Vec<String> {
        self.calls
            .lock()
            .map(|calls| calls.clone())
            .unwrap_or_default()
    }

    /// The reads of names and facts so far, in the order they were asked: `tasks`, `facts <task>`, `status`.
    pub fn reads(&self) -> Vec<String> {
        self.reads
            .lock()
            .map(|reads| reads.clone())
            .unwrap_or_default()
    }
}

/// What [`Source::from_environment`] starts: the source, its notifications, and the watcher to keep alive.
pub struct Started {
    pub source: Source,
    pub events: FactsEvents,
    pub watcher: Option<Arc<Watcher>>,
}

impl Source {
    /// The fixture source when `SAKURA_FIXTURE` is set, else `box` with the watcher on `$AGENTBOX_HOME`.
    pub fn from_environment(runtime: Arc<Runtime>) -> Result<Started, String> {
        let (sender, events) = unbounded_channel();
        match std::env::var(FIXTURE_VAR).ok() {
            Some(choice) => {
                let status = Arc::new(read_fixture(&choice)?);
                start_fixture_ticks(sender);
                let source = Source {
                    kind: Kind::Fixture(status),
                    runtime,
                    #[cfg(test)]
                    recorder: None,
                };
                Ok(Started {
                    source,
                    events,
                    watcher: None,
                })
            }
            None => Ok(start_live(runtime, sender, events)),
        }
    }

    /// `box tasks`: the names, read without starting a container. `None` when `box` does not have the
    /// command, and with the fixture, whose status answers at once.
    pub fn tasks(&self) -> Pending<Option<Vec<TaskSummary>>> {
        self.note_read("tasks".into());
        match &self.kind {
            Kind::Live(core) => {
                let core = core.clone();
                self.spawn(async move { core.tasks().await })
            }
            Kind::Fixture(_) => ready(Ok(None)),
        }
    }

    /// `box status`.
    pub fn status(&self) -> Pending<BoxStatus> {
        self.note_read("status".into());
        match &self.kind {
            Kind::Live(core) => {
                let core = core.clone();
                self.spawn(async move { core.status().await })
            }
            Kind::Fixture(status) => ready(Ok(status.as_ref().clone())),
        }
    }

    /// Starts the agent of `task`'s conversation, `box acp <task> [<account>]`, working in `workspace`,
    /// and gives its handle and its events. None on the sample data, which has no `box`, and for names
    /// `box` would not take.
    pub fn agent(
        &self,
        task: &str,
        account: Option<&str>,
        workspace: &str,
    ) -> Option<(Agent, Receiver<AgentEvent>)> {
        self.note_read(format!("acp {task}"));
        let Kind::Live(core) = &self.kind else {
            return None;
        };
        let command = match core.cli().acp(task, account) {
            Ok(command) => command,
            Err(error) => {
                eprintln!("sakura: no agent for {task}: {:?}", error.details);
                return None;
            }
        };
        let config = AgentConfig {
            command: command.argv,
            env: command.env,
            workspace: workspace.into(),
            deadlines: Deadlines::default(),
            max_unread_bytes: sakura_acp::transport::MAX_UNREAD_BYTES,
        };
        Some(Agent::start(config, self.runtime.handle()))
    }

    /// The `box` the window talks to, for the views that read through it (the files of a lavoro, the
    /// graph of a project); none on the sample data, which has no `box`.
    pub fn cli(&self) -> Option<&BoxCli> {
        match &self.kind {
            Kind::Live(core) => Some(core.cli()),
            Kind::Fixture(_) => None,
        }
    }

    /// The runtime the calls run on.
    pub fn runtime(&self) -> &Arc<Runtime> {
        &self.runtime
    }

    /// The row of a task whose creation was just started, known without a `box` call; none in a fixture.
    pub fn task_being_created(&self, task: &str) -> Option<Task> {
        match &self.kind {
            Kind::Live(core) => core.task_being_created(task),
            Kind::Fixture(_) => None,
        }
    }

    /// `box facts <task>`.
    pub fn task_facts(&self, task: String) -> Pending<Task> {
        self.note_read(format!("facts {task}"));
        match &self.kind {
            Kind::Live(core) => {
                let core = core.clone();
                self.spawn(async move { core.task_facts(&task).await })
            }
            Kind::Fixture(status) => ready(fixture_task(status, &task)),
        }
    }

    /// `box journal`, only events since `since` when given. The fixture has no journal.
    pub fn journal(&self, since: Option<String>) -> Pending<Vec<JournalEvent>> {
        match &self.kind {
            Kind::Live(core) => {
                let core = core.clone();
                self.spawn(async move { core.journal(since.as_deref()).await })
            }
            Kind::Fixture(_) => ready(Ok(Vec::new())),
        }
    }

    /// `box diff <task> [--commit <sha>]`, parsed into files off the UI thread.
    pub fn patch(&self, task: String, commit: Option<String>) -> Pending<Patch> {
        match &self.kind {
            Kind::Live(core) => {
                let core = core.clone();
                self.spawn(async move {
                    let text = core.task_diff(&task, commit.as_deref()).await?;
                    Ok(Patch::parse(&text))
                })
            }
            Kind::Fixture(_) => {
                let text = fixture_changes::patch(&task, commit.as_deref());
                self.spawn(async move { text.map(|text| Patch::parse(&text)) })
            }
        }
    }

    /// `box log <task> --json`: the commits ahead of the base, newest first.
    pub fn commits(&self, task: String) -> Pending<Vec<Commit>> {
        match &self.kind {
            Kind::Live(core) => {
                let core = core.clone();
                self.spawn(async move { core.task_log(&task).await })
            }
            Kind::Fixture(_) => ready(fixture_changes::log(&task)),
        }
    }

    /// `box result-log <task> <verb>`: the tail of the last result's log.
    pub fn result_log(&self, task: String, verb: String) -> Pending<String> {
        self.live(move |core| async move { core.result_log(&task, &verb).await })
    }

    /// `box verify <task> <verb> --no-wait`.
    pub fn verify(&self, task: String, verb: String) -> Pending<()> {
        if let Some(pending) = self.recorded(format!("verify {task} {verb}")) {
            return pending;
        }
        self.live(move |core| async move { core.verify_request(&task, &verb).await })
    }

    /// `box cancel <task> <verb>`, answering whether there was a queued or running check to cancel.
    pub fn cancel(&self, task: String, verb: String) -> Pending<bool> {
        if let Some(pending) = self.recorded(format!("cancel {task} {verb}")) {
            return pending;
        }
        self.live(move |core| async move { core.verify_cancel(&task, &verb).await })
    }

    /// Whether `box` can cancel a check at all. The fixture offers it like every other action: a press
    /// answers that `box` is not called.
    pub fn can_cancel(&self) -> Pending<bool> {
        match &self.kind {
            Kind::Live(core) => {
                let core = core.clone();
                self.spawn(async move { core.can_cancel().await })
            }
            Kind::Fixture(_) => ready(Ok(true)),
        }
    }

    /// `box projects --json`: the projects a lavoro can be made in; `None` on a `box` that has none of
    /// the verbs (an older one). The sample data has one project, the one its lavori name.
    pub fn projects(&self) -> Pending<Option<Vec<Project>>> {
        self.note_read("projects".to_string());
        match &self.kind {
            Kind::Live(core) => {
                let core = core.clone();
                self.spawn(async move { core.projects().await })
            }
            Kind::Fixture(status) => ready(Ok(Some(fixture_projects(status)))),
        }
    }

    /// `box project detect <folder> --json`: what `box` recognises in the folder the person chose.
    pub fn project_detect(&self, folder: String) -> Pending<Detected> {
        if let Some(pending) = self.recorded(format!("project detect {folder}")) {
            return pending;
        }
        self.live(move |core| async move { core.project_detect(&folder).await })
    }

    /// `box project add`: adds the project as the person confirmed it.
    pub fn project_add(&self, project: NewProject) -> Pending<()> {
        if let Some(pending) = self.recorded(format!("project add {}", project.name)) {
            return pending;
        }
        self.live(move |core| async move { core.project_add(&project).await })
    }

    /// `box branches <project> --json`: the branches a lavoro of the project can start from. The sample
    /// data has none to offer beside the project's own.
    pub fn branches(&self, project: String) -> Pending<Branches> {
        self.note_read(format!("branches {project}"));
        match &self.kind {
            Kind::Live(core) => {
                let core = core.clone();
                self.spawn(async move { core.branches(&project).await })
            }
            Kind::Fixture(status) => ready(Ok(fixture_branches(status, &project))),
        }
    }

    /// Starts `box new <task> <project>` (with the base and the place the person chose) as a job of the
    /// core and answers at once: the task is listed as being created from then on. With `account`, the
    /// project is first told to use it (`box project set <project> --account`), so that the lavoro is
    /// made with it.
    pub fn new_task(
        &self,
        task: String,
        lavoro: NewLavoro,
        account: Option<String>,
    ) -> Pending<()> {
        if let Some(pending) = self.recorded(format!("new {task} {}", lavoro.project)) {
            return pending;
        }
        self.live(move |core| async move {
            if let Some(account) = account {
                core.project_set_account(&lavoro.project, &account).await?;
            }
            core.task_new(&task, &lavoro)
        })
    }

    /// `box project set <project> --account <account>`: the account the project's agents start with.
    pub fn set_project_account(&self, project: String, account: String) -> Pending<()> {
        if let Some(pending) = self.recorded(format!("project set {project} --account {account}")) {
            return pending;
        }
        self.live(move |core| async move { core.project_set_account(&project, &account).await })
    }

    /// `box take <task>`, answering the taken sha.
    pub fn take(&self, task: String) -> Pending<String> {
        if let Some(pending) = self.recorded(format!("take {task}")) {
            return pending;
        }
        self.live(move |core| async move { core.task_take(&task).await })
    }

    /// `box stop <task>`.
    pub fn stop_agent(&self, task: String) -> Pending<()> {
        if let Some(pending) = self.recorded(format!("stop {task}")) {
            return pending;
        }
        self.live(move |core| async move { core.agent_stop(&task).await })
    }

    /// `box rm <task>`, with `--force` when `force`.
    pub fn remove_task(&self, task: String, force: bool) -> Pending<()> {
        let flag = if force { " --force" } else { "" };
        if let Some(pending) = self.recorded(format!("rm {task}{flag}")) {
            return pending;
        }
        self.live(move |core| async move { core.task_remove(&task, force).await })
    }

    /// `box accounts --json`. The fixture has the accounts of `SAKURA_FIXTURE_ACCOUNTS` (JSON, as `box` prints
    /// them), none by default: today's state of a machine where no account has logged in.
    pub fn accounts(&self) -> Pending<Vec<Account>> {
        match &self.kind {
            Kind::Live(core) => {
                let core = core.clone();
                self.spawn(async move { core.accounts().await })
            }
            Kind::Fixture(_) => ready(fixture_accounts(
                std::env::var(FIXTURE_ACCOUNTS_VAR).ok().as_deref(),
            )),
        }
    }

    /// The pty sessions of `box attach`, `box shell` and `box login`, reporting to `sink`. The fixture has no
    /// `box` to run them with, and test windows open none.
    pub fn terminals(&self, sink: TerminalSink) -> Option<Terminals> {
        match &self.kind {
            Kind::Live(core) => Some(Terminals::new(core.cli().clone(), home_dir(), sink)),
            Kind::Fixture(_) => None,
        }
    }

    /// Machine facts from macOS (sysctl), read off the UI thread.
    pub fn machine(&self) -> Pending<Machine> {
        // A test window is not about the Mac it runs on: its machine is calm, whatever the load of
        // the test run is.
        #[cfg(test)]
        if self.recorder.is_some() {
            return Box::pin(std::future::ready(Ok(calm_machine())));
        }
        let home = watcher::agentbox_home();
        let handle = self.runtime.spawn_blocking(move || {
            let existing = if home.exists() {
                home
            } else {
                std::path::PathBuf::from("/")
            };
            sakura_core::machine::read(&existing)
        });
        Box::pin(async move { handle.await.unwrap_or_else(|error| Err(joined(error))) })
    }

    /// In a test window with a recorder: notes `call`. On the fixture it then answers never, so the
    /// action stays running; on a fake `box` the action runs.
    #[cfg(test)]
    fn recorded<T: Send + 'static>(&self, call: String) -> Option<Pending<T>> {
        let recorder = self.recorder.as_ref()?;
        if let Ok(mut calls) = recorder.calls.lock() {
            calls.push(call);
        }
        match self.kind {
            Kind::Live(_) => None,
            Kind::Fixture(_) => Some(Box::pin(std::future::pending())),
        }
    }

    #[cfg(not(test))]
    fn recorded<T>(&self, _: String) -> Option<Pending<T>> {
        None
    }

    /// In a test window with a recorder: notes a read of names or facts, in the order it was asked.
    #[cfg(test)]
    fn note_read(&self, read: String) {
        if let Some(mut reads) = self.recorder.as_ref().and_then(|r| r.reads.lock().ok()) {
            reads.push(read);
        }
    }

    #[cfg(not(test))]
    fn note_read(&self, _: String) {}

    /// The built-in fixture for a test window: no ticks, actions recorded by `recorder`.
    #[cfg(test)]
    pub fn for_test(runtime: Arc<Runtime>, recorder: Arc<Recorder>) -> Started {
        let (_, events) = unbounded_channel();
        let status = Arc::new(read_fixture("1").expect("the built-in fixture parses"));
        let source = Source {
            kind: Kind::Fixture(status),
            runtime,
            recorder: Some(recorder),
        };
        Started {
            source,
            events,
            watcher: None,
        }
    }

    /// A test window on the `box` script at `script`: real reads and actions, no watcher, no ticks.
    /// `recorder` notes them.
    #[cfg(test)]
    pub fn live_for_test(
        runtime: Arc<Runtime>,
        script: std::path::PathBuf,
        recorder: Arc<Recorder>,
    ) -> Started {
        let (sender, events) = unbounded_channel();
        let notify = Arc::new(move |task| drop(sender.send(Signal::Changed(task))));
        let source = Source {
            // Limits no busy machine reaches: these tests are about what the window does, not about
            // how long `box` may take.
            kind: Kind::Live(Core::new(
                BoxCli::at(script).with_timeouts(sakura_core::test_support::patient()),
                notify,
            )),
            runtime,
            recorder: Some(recorder),
        };
        Started {
            source,
            events,
            watcher: None,
        }
    }

    /// Runs `call` against `box`; the fixture has no `box` to act on and says so.
    fn live<T, F, Fut>(&self, call: F) -> Pending<T>
    where
        T: Send + 'static,
        F: FnOnce(Core) -> Fut,
        Fut: Future<Output = BridgeResult<T>> + Send + 'static,
    {
        match &self.kind {
            Kind::Live(core) => self.spawn(call(core.clone())),
            Kind::Fixture(_) => ready(Err(BridgeError::invalid(FIXTURE_REFUSAL))),
        }
    }

    fn spawn<T: Send + 'static>(
        &self,
        work: impl Future<Output = BridgeResult<T>> + Send + 'static,
    ) -> Pending<T> {
        let handle = self.runtime.spawn(work);
        Box::pin(async move { handle.await.unwrap_or_else(|error| Err(joined(error))) })
    }
}

fn start_live(
    runtime: Arc<Runtime>,
    sender: UnboundedSender<Signal>,
    events: FactsEvents,
) -> Started {
    let from_core = sender.clone();
    let core = Core::new(
        BoxCli::from_environment(),
        Arc::new(move |task| drop(from_core.send(Signal::Changed(task)))),
    );
    let ticks = sender.clone();
    let on_tick: watcher::TickSink = Arc::new(move || drop(ticks.send(Signal::Tick)));
    let sink: watcher::FactsSink = Arc::new(move |task| drop(sender.send(Signal::Changed(task))));
    let watcher = Watcher::start(
        watcher::agentbox_home(),
        (sink, on_tick),
        watcher::DEBOUNCE,
        watcher::TICK,
    );
    let source = Source {
        kind: Kind::Live(core),
        runtime,
        #[cfg(test)]
        recorder: None,
    };
    Started {
        source,
        events,
        watcher: Some(watcher),
    }
}

/// The fixture has no watcher; a plain tick keeps machine facts and ages moving.
fn start_fixture_ticks(sender: UnboundedSender<Signal>) {
    std::thread::spawn(move || loop {
        std::thread::sleep(watcher::TICK);
        if sender.send(Signal::Tick).is_err() {
            return;
        }
    });
}

fn read_fixture(choice: &str) -> Result<BoxStatus, String> {
    let text = if choice == "1" {
        BUILT_IN_FIXTURE.to_string()
    } else {
        std::fs::read_to_string(choice).map_err(|error| format!("{choice}: {error}"))?
    };
    serde_json::from_str(&text).map_err(|error| format!("{choice}: {error}"))
}

fn fixture_accounts(json: Option<&str>) -> BridgeResult<Vec<Account>> {
    let Some(json) = json else {
        return Ok(Vec::new());
    };
    serde_json::from_str(json).map_err(|error| {
        BridgeError::invalid(
            Said::new("fixture.accounts_unreadable").text("variable", FIXTURE_ACCOUNTS_VAR),
        )
        .with_details(Some(error.to_string()))
    })
}

/// The projects of the sample data: one per project its lavori name, in a box, starting from the base
/// of its first lavoro.
fn fixture_projects(status: &BoxStatus) -> Vec<Project> {
    let mut projects: Vec<Project> = Vec::new();
    for task in &status.tasks {
        if projects.iter().all(|project| project.name != task.profile) {
            projects.push(Project {
                name: task.profile.clone(),
                base: Some(task.base.clone()),
                place: task.place,
                account: None,
                run: None,
                error: None,
            });
        }
    }
    projects
}

/// The branches of a project of the sample data: its base.
fn fixture_branches(status: &BoxStatus, project: &str) -> Branches {
    let base = fixture_projects(status)
        .into_iter()
        .find(|listed| listed.name == project)
        .and_then(|listed| listed.base)
        .unwrap_or_default();
    Branches {
        branches: vec![Branch { name: base.clone() }],
        default: base,
    }
}

fn fixture_task(status: &BoxStatus, name: &str) -> BridgeResult<Task> {
    status
        .tasks
        .iter()
        .find(|task| task.name == name)
        .cloned()
        .ok_or_else(|| BridgeError::new(ErrorCode::NotFound, Said::new("box.failure.no_such_task")))
}

/// The directory `box` runs in for terminals: the home directory, as from a login shell.
fn home_dir() -> std::path::PathBuf {
    std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("/"))
}

fn ready<T: Send + 'static>(value: BridgeResult<T>) -> Pending<T> {
    Box::pin(std::future::ready(value))
}

fn joined(error: tokio::task::JoinError) -> BridgeError {
    sakura_core::box_cli::internal(format!("a read was interrupted: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_accounts_are_none_by_default_or_read_as_box_prints_them() {
        assert!(fixture_accounts(None).unwrap().is_empty());
        let accounts = fixture_accounts(Some(r#"[{"name":"personal","loggedIn":true}]"#)).unwrap();
        assert_eq!(accounts.len(), 1);
        assert!(accounts[0].logged_in);
        assert!(fixture_accounts(Some("nope")).is_err());
    }

    #[test]
    fn built_in_fixture_parses() {
        let status = read_fixture("1").unwrap();
        assert_eq!(status.tasks.len(), 4);
    }

    #[test]
    fn fixture_task_finds_by_name_or_reports_not_found() {
        let status = read_fixture("1").unwrap();
        assert_eq!(
            fixture_task(&status, "headings-26").unwrap().name,
            "headings-26"
        );
        let missing = fixture_task(&status, "nessuno").unwrap_err();
        assert_eq!(missing.code, ErrorCode::NotFound);
    }

    #[test]
    fn unreadable_fixture_path_is_an_error_with_the_path() {
        let error = read_fixture("/nonexistent/status.json").unwrap_err();
        assert!(error.starts_with("/nonexistent/status.json"));
    }
}
