//! Where the window's facts come from: the engine's `sakura` command, through `sakura-core`. Every call runs
//! on the tokio runtime and is awaited from GPUI's executor, so the UI thread never waits on the engine.
//! This is the only source of the default build; the `boxes` feature brings back the one of `box`.

use super::engine_facts::{EngineRead, Pending};
use sakura_core::engine::agents::{AgentRun, WriteOutcome};
use sakura_core::engine::changes::{ChangesPage, FilePatch};
use sakura_core::engine::cli::{Act, ActResult, EngineCli, Env};
use sakura_core::engine::feed::AgentFeedPage;
use sakura_core::engine::master::{Decided, MasterSpec};
use sakura_core::engine::rules::Rules;
use sakura_core::engine::snapshot::Snapshot;
use sakura_core::engine::watch::{self, EngineWatcher};
use sakura_core::error::{BridgeError, BridgeResult};
use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::runtime::Runtime;
use tokio::sync::watch::Receiver;

/// Where facts can come from.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// `sakura cockpit snapshot --json` and `sakura cockpit act --json`.
    Engine,
}

/// The engine and the runtime its calls run on.
#[derive(Clone)]
pub struct Source {
    /// The command, or why it could not be found: every read then says it again, so the person is told
    /// at every look and the window keeps what it had.
    cli: Arc<Mutex<Result<EngineCli, BridgeError>>>,
    runtime: Arc<Runtime>,
}

/// What [`Source::from_environment`] starts: the source, the changes under the engine's home and the
/// watcher to keep alive.
pub struct Started {
    pub source: Source,
    pub changes: Receiver<()>,
    pub watcher: Option<EngineWatcher>,
}

impl Source {
    /// Every origin this build has.
    #[cfg(test)]
    pub const ORIGINS: [Origin; 1] = [Origin::Engine];

    /// The engine named by `SAKURA_BIN`, else by `configured` (the path set in Settings), else on the
    /// PATH, with a watcher on the engine's home.
    pub fn from_environment(runtime: Arc<Runtime>, configured: Option<PathBuf>) -> Started {
        let (sender, changes) = tokio::sync::watch::channel(());
        let on_change = Arc::new(move || {
            let _ = sender.send(());
        });
        let watcher = EngineWatcher::start(&watch::home(), on_change);
        let cli = Self::locate(configured);
        Started {
            source: Source::new(cli, runtime),
            changes,
            watcher,
        }
    }

    /// A source over a given command, for the tests.
    #[cfg(test)]
    pub fn over(cli: EngineCli, runtime: Arc<Runtime>) -> Self {
        Self::new(Ok(cli), runtime)
    }

    fn new(cli: Result<EngineCli, BridgeError>, runtime: Arc<Runtime>) -> Self {
        Self {
            cli: Arc::new(Mutex::new(cli)),
            runtime,
        }
    }

    /// Validates the configured engine path, honoring the environment override.
    pub fn locate(configured: Option<PathBuf>) -> BridgeResult<EngineCli> {
        EngineCli::locate(&Env::from_process(configured))
    }

    /// Applies a completed discovery, including a failure, to future requests.
    pub fn replace_command(&self, found: BridgeResult<EngineCli>) -> BridgeResult<()> {
        let answer = found.as_ref().map(|_| ()).map_err(Clone::clone);
        *self
            .cli
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = found;
        answer
    }

    #[cfg(test)]
    fn relocate_in(&self, env: &Env) -> BridgeResult<()> {
        self.replace_command(EngineCli::locate(env))
    }

    /// Where the command is found when no path is written in Settings (`SAKURA_BIN` first, else the PATH):
    /// what an empty path field means. None when none is found.
    pub fn default_command(&self) -> Option<PathBuf> {
        let found = Self::locate(None);
        found.ok().map(|cli| cli.bin().to_path_buf())
    }

    /// Where the command now in use is; none when no command was found.
    pub fn command(&self) -> Option<PathBuf> {
        self.current().ok().map(|cli| cli.bin().to_path_buf())
    }

    /// The command now in use, or why there is none.
    fn current(&self) -> Result<EngineCli, BridgeError> {
        let held = self
            .cli
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        held.clone()
    }

    /// Reads the engine's setup detection on the runtime.
    pub fn setup_detect(&self) -> Pending<sakura_core::engine::setup::SetupDetection> {
        match self.current() {
            Ok(cli) => self.spawn(async move { cli.setup_detect().await }),
            Err(error) => ready(Err(error)),
        }
    }

    /// Resolves the name returned by setup into the Master account id.
    pub async fn setup_account_id(&self, name: String) -> BridgeResult<Option<String>> {
        let snapshot = self.snapshot().await?;
        Ok(snapshot
            .accounts
            .into_iter()
            .find(|account| !account.is_hidden() && account.name() == name)
            .map(|account| account.id))
    }

    /// `sakura cockpit act ... --json`.
    pub fn act(&self, act: Act) -> Pending<ActResult> {
        match self.current() {
            Ok(cli) => self.spawn(async move { cli.act(&act).await }),
            Err(error) => ready(Err(error)),
        }
    }

    /// `sakura cockpit act ... --json`, given up after `bound`.
    pub fn act_within(&self, act: Act, bound: Duration) -> Pending<ActResult> {
        match self.current() {
            Ok(cli) => self.spawn(within(bound, async move { cli.act(&act).await })),
            Err(error) => ready(Err(error)),
        }
    }

    /// An act whose question the person was shown, with the token the engine gave for it in `act.confirm`:
    /// the question is asked again, and only when the engine still asks `summary` is the act run with the
    /// token shown. When the question changed, nothing runs and the answer is the engine's new question, with
    /// its own token. Both calls run on the runtime, so they go on when the caller drops the future, and the
    /// exchange is given up after `bound`.
    pub fn act_confirmed(&self, act: Act, summary: String, bound: Duration) -> Pending<ActResult> {
        match self.current() {
            Ok(cli) => {
                self.spawn(
                    async move { within(bound, confirm_if_unchanged(&cli, act, &summary)).await },
                )
            }
            Err(error) => ready(Err(error)),
        }
    }

    /// `sakura cockpit act add-project <folder> --json`: the engine registers the Git repository of `folder`.
    pub fn add_project(&self, folder: String) -> Pending<ActResult> {
        match self.current() {
            Ok(cli) => self.spawn(async move { cli.add_project(&folder).await }),
            Err(error) => ready(Err(error)),
        }
    }

    /// `sakura engine rules --json`: the rules of how a lane runs.
    pub fn rules(&self) -> Pending<Rules> {
        match self.current() {
            Ok(cli) => self.spawn(async move { cli.rules().await }),
            Err(error) => ready(Err(error)),
        }
    }

    /// `sakura cockpit phone --json`: the setup of the phone page.
    pub fn phone(&self) -> Pending<sakura_core::engine::phone::PhoneSetup> {
        match self.current() {
            Ok(cli) => self.spawn(async move { cli.phone().await }),
            Err(error) => ready(Err(error)),
        }
    }

    pub fn goose_setup_program(&self) -> Result<String, BridgeError> {
        Ok(self.current()?.bin().display().to_string())
    }

    /// `sakura master spec --json`: the engine writes the master's files and says how to start it.
    pub fn master_spec(&self, account: Option<String>) -> Pending<MasterSpec> {
        match self.current() {
            Ok(cli) => self.spawn(async move { cli.master_spec(account.as_deref()).await }),
            Err(error) => ready(Err(error)),
        }
    }

    pub fn goose_master_spec(
        &self,
        request: sakura_core::engine::master::GooseMasterRequest,
    ) -> Pending<MasterSpec> {
        match self.current() {
            Ok(cli) => self.spawn(async move { cli.goose_master_spec(&request).await }),
            Err(error) => ready(Err(error)),
        }
    }

    /// `sakura master decide --json`: the engine's decision on one permission request of the master.
    pub fn master_decide(&self, request: serde_json::Value) -> Pending<Decided> {
        match self.current() {
            Ok(cli) => self.spawn(async move { cli.master_decide(&request).await }),
            Err(error) => ready(Err(error)),
        }
    }

    /// `sakura flow sessions <task> --json` in `repo`: the lane's agents, one per run.
    pub fn agents(&self, repo: PathBuf, task: String) -> Pending<Vec<AgentRun>> {
        match self.current() {
            Ok(cli) => self.spawn(async move { cli.agents(&repo, &task).await }),
            Err(error) => ready(Err(error)),
        }
    }

    /// `sakura flow feed <task> --run <run> --json` in `repo`: what one agent's conversation holds after the
    /// cursor `since`, and the text it is writing now.
    pub fn agent_feed(
        &self,
        repo: PathBuf,
        task: String,
        run: String,
        since: Option<String>,
    ) -> Pending<AgentFeedPage> {
        match self.current() {
            Ok(cli) => self
                .spawn(async move { cli.agent_feed(&repo, &task, &run, since.as_deref()).await }),
            Err(error) => ready(Err(error)),
        }
    }

    /// `sakura flow steer <task> --run <run> <text> --json` in `repo`: a message to one agent alone.
    pub fn write_to_agent(
        &self,
        repo: PathBuf,
        task: String,
        run: String,
        text: String,
    ) -> Pending<WriteOutcome> {
        match self.current() {
            Ok(cli) => {
                self.spawn(async move { cli.write_to_agent(&repo, &task, &run, &text).await })
            }
            Err(error) => ready(Err(error)),
        }
    }

    /// `sakura flow changes <task> --json` in `repo`: the files the lane changed.
    pub fn changes(&self, repo: PathBuf, task: String) -> Pending<ChangesPage> {
        match self.current() {
            Ok(cli) => self.spawn(async move { cli.changes(&repo, &task).await }),
            Err(error) => ready(Err(error)),
        }
    }

    /// The patch of `path` of the lane `task`, read in `repo`.
    pub fn changes_file(&self, repo: PathBuf, task: String, path: String) -> Pending<FilePatch> {
        match self.current() {
            Ok(cli) => self.spawn(async move { cli.changes_file(&repo, &task, &path).await }),
            Err(error) => ready(Err(error)),
        }
    }

    pub fn scheduled_history(
        &self,
        id: String,
        before: Option<u64>,
    ) -> Pending<sakura_core::engine::scheduled::HistoryPage> {
        match self.current() {
            Ok(cli) => self.spawn(async move { cli.scheduled_history(&id, before).await }),
            Err(error) => ready(Err(error)),
        }
    }

    pub fn scheduled_conversation(
        &self,
        id: String,
        after: Option<u64>,
    ) -> Pending<sakura_core::engine::scheduled::ConversationPage> {
        match self.current() {
            Ok(cli) => self.spawn(async move { cli.scheduled_conversation(&id, after).await }),
            Err(error) => ready(Err(error)),
        }
    }

    /// The runtime the calls run on.
    pub fn runtime(&self) -> &Arc<Runtime> {
        &self.runtime
    }

    fn spawn<T: Send + 'static>(
        &self,
        work: impl Future<Output = BridgeResult<T>> + Send + 'static,
    ) -> Pending<T> {
        let handle = self.runtime.spawn(work);
        Box::pin(async move { handle.await.unwrap_or_else(|error| Err(joined(error))) })
    }
}

impl EngineRead for Source {
    fn snapshot(&self) -> Pending<Snapshot> {
        match self.current() {
            Ok(cli) => self.spawn(async move { cli.snapshot().await }),
            Err(error) => ready(Err(error)),
        }
    }
}

/// What `exchange` comes to, or the error that it was given up after `bound`.
async fn within<T>(
    bound: Duration,
    exchange: impl Future<Output = BridgeResult<T>>,
) -> BridgeResult<T> {
    match tokio::time::timeout(bound, exchange).await {
        Ok(answered) => answered,
        Err(_) => Err(sakura_core::process::internal(format!(
            "no answer in {bound:?}"
        ))),
    }
}

/// Asks again what `act` (carrying the token that was shown) was asked: when the engine still asks
/// `summary`, the answer to `act` with that token; otherwise the engine's new question, unconfirmed.
async fn confirm_if_unchanged(cli: &EngineCli, act: Act, summary: &str) -> BridgeResult<ActResult> {
    let asking = Act {
        confirm: None,
        ..act.clone()
    };
    match cli.act(&asking).await? {
        ActResult::Confirm { summary: now, .. } if now == summary => cli.act(&act).await,
        other => Ok(other),
    }
}

fn ready<T: Send + 'static>(value: BridgeResult<T>) -> Pending<T> {
    Box::pin(std::future::ready(value))
}

fn joined(error: tokio::task::JoinError) -> BridgeError {
    sakura_core::process::internal(format!("a read was interrupted: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_build_has_no_box_source() {
        assert_eq!(Source::ORIGINS, [Origin::Engine]);
    }

    fn script(dir: &std::path::Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        sakura_core::test_support::write_script(&path, "#!/bin/sh\n");
        path
    }

    fn env(configured: Option<PathBuf>) -> Env {
        Env {
            sakura_bin: None,
            configured,
            path: String::new(),
        }
    }

    #[test]
    fn relocate_with_a_saved_path_makes_that_command_the_one_in_use() {
        let dir = tempfile::tempdir().expect("a folder");
        let (first, second) = (script(dir.path(), "first"), script(dir.path(), "second"));
        let runtime = Arc::new(Runtime::new().expect("a runtime"));
        let source = Source::over(EngineCli::at(first), runtime);
        assert_eq!(source.relocate_in(&env(Some(second.clone()))), Ok(()));
        assert_eq!(
            source.current().expect("a command").bin(),
            second.canonicalize().expect("a real path"),
            "the engine is run by its real path, the one that was checked"
        );
    }

    #[test]
    fn relocate_with_a_path_that_is_no_command_says_so_and_every_read_says_it_too() {
        let dir = tempfile::tempdir().expect("a folder");
        let runtime = Arc::new(Runtime::new().expect("a runtime"));
        let source = Source::over(EngineCli::at(script(dir.path(), "first")), runtime);
        let nowhere = dir.path().join("nowhere");
        let said = source
            .relocate_in(&env(Some(nowhere)))
            .expect_err("no command there");
        assert_eq!(source.current().expect_err("still none"), said);
    }
}
