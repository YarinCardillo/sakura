//! Runs the `sakura` command's cockpit face: an explicit path, an explicit PATH, an argument vector and
//! never a shell, with a deadline and an output cap, and its answers read into types.

use super::agents::{AgentRun, WriteOutcome};
use super::changes::{ChangesPage, FilePatch};
use super::connectors::ConnectorReceipt;
use super::feed::{AgentFeedPage, FeedPage};
use super::master::{Decided, MasterSpec};
use super::presets::PresetList;
use super::rules::Rules;
use super::snapshot::{Action, Snapshot};
use super::trust::{self, Untrusted};
use super::{leeway, tmux, unreadable_answer};
use crate::error::{BridgeError, BridgeResult, ErrorCode};
use crate::process::{self, limit_said, Captured, Ended};
use crate::said::Said;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::process::Command;

/// Environment variable that names the engine's `sakura` command.
pub const SAKURA_BIN_VAR: &str = "SAKURA_BIN";
/// Environment variable that moves the engine's home, where its state and waits are kept.
pub const ENGINE_HOME_VAR: &str = "SAKURA_ENGINE_HOME";
/// The command's name when it is looked for on the PATH.
const PROGRAM: &str = "sakura";
/// The project disappeared before the engine could run a cockpit action.
pub const PROJECT_FOLDER_MISSING: &str = "error.engine.project_folder_missing";
pub const LANE_FOLDER_MISSING: &str = "error.engine.lane_folder_missing";
/// How long a read of the snapshot may take. Past it the last snapshot is kept and the cause is said.
pub const DEADLINE: Duration = Duration::from_secs(10);
/// How long an action may take: the engine's own limit for one is two minutes, ten for opening a lane.
const ACT_DEADLINE: Duration = Duration::from_secs(11 * 60);
const MAX_STDOUT_BYTES: usize = 16 * 1024 * 1024;
/// The inputs of the cockpit's actions, each an option of `sakura cockpit act` of the same name.
const INPUTS: &[&str] = &[
    "text",
    "model",
    "role",
    "step",
    "name",
    "from",
    "label",
    "percent",
    "plan",
    "since",
    "verdict",
    "note",
    "remember",
    "choice",
    "rule",
    "value",
    "level",
    "on",
    "preset",
    "kind",
    "address",
    "at",
    "lookback_hours",
    "language",
    "account",
    "device",
    "port",
    "folder",
    "provider",
    "state",
    "op",
    "choices",
    "definition",
    "revision",
    "candidate",
    "candidate_revision",
    "until",
    "project",
    "title",
    "goal",
    "summary",
    "status",
    "clear",
    "decision",
    "answer",
    "task",
    "conversation",
    "startop",
    "track",
];

/// The folders of the PATH every engine command runs with, after the person's own `~/.local/bin`: where the
/// engine and the agent programs it starts are installed. An app opened from Finder has almost no PATH of
/// its own, so the app never passes its launch PATH on.
const KNOWN_FOLDERS: [&str; 4] = ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"];
/// The person's own folder for programs, under their home.
const HOME_PROGRAMS: &str = ".local/bin";

/// The PATH of every engine command: `~/.local/bin` of `home` (when it is known), then the known folders.
pub fn known_path(home: Option<&Path>) -> String {
    let own = home.map(|home| home.join(HOME_PROGRAMS).display().to_string());
    let folders = own.into_iter().chain(KNOWN_FOLDERS.map(String::from));
    folders.collect::<Vec<_>>().join(":")
}

fn launch_path() -> String {
    known_path(std::env::var_os("HOME").map(PathBuf::from).as_deref())
}

/// Where the engine can be named: what the app was started with, and what the person set once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Env {
    /// `SAKURA_BIN`; an empty value is none.
    pub sakura_bin: Option<PathBuf>,
    /// The path set in Settings.
    pub configured: Option<PathBuf>,
    /// The folders `sakura` is looked for in, and the PATH of every child.
    pub path: String,
}

impl Env {
    /// The process's own `SAKURA_BIN`, the path from Settings, and the PATH built from known folders (the
    /// app's own launch PATH is not used: from Finder it has almost nothing).
    pub fn from_process(configured: Option<PathBuf>) -> Self {
        Self {
            sakura_bin: std::env::var_os(SAKURA_BIN_VAR).map(PathBuf::from),
            configured,
            path: launch_path(),
        }
    }
}

/// One call of `sakura cockpit act`: an action the engine offered, the target it applies to and its
/// inputs. `confirm` is the token of a first call's answer, for the second.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Act {
    pub action: Action,
    pub target: String,
    pub inputs: BTreeMap<String, String>,
    pub confirm: Option<String>,
}

impl Act {
    pub fn is_track(&self) -> bool {
        self.action.key.starts_with("track")
            || (self.action.key == "L" && self.inputs.contains_key("track"))
    }
}

/// What the engine answered to an action.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "result", rename_all = "lowercase")]
pub enum ActResult {
    /// The first call of an action that asks: show `summary` verbatim, then call again with `token`.
    Confirm {
        token: String,
        expires: String,
        #[serde(default)]
        summary: String,
        display_args: Option<BTreeMap<String, String>>,
    },
    /// The command ran. A non-zero `exit_code` is a failed command, shown with its `output`.
    Done {
        exit_code: i64,
        #[serde(default)]
        output: String,
        /// Present when the command ran but its row in the action log could not be written.
        audit: Option<String>,
        connector: Option<ConnectorReceipt>,
    },
    /// Nothing ran.
    Refused {
        reason: String,
        reason_code: Option<String>,
        reason_args: Option<BTreeMap<String, serde_json::Value>>,
    },
}

/// Handle on the engine's `sakura` command.
#[derive(Debug, Clone)]
pub struct EngineCli {
    bin: PathBuf,
    path: String,
    /// The helper of the app's own bundle, set as `SAKURA_LEEWAY` for every command; none leaves the
    /// environment as the app got it (a build run from a source tree).
    leeway: Option<PathBuf>,
    /// The tmux of the app's own bundle, set as `SAKURA_TMUX` the same way; none leaves the environment as
    /// the app got it.
    tmux: Option<PathBuf>,
    deadline: Duration,
}

impl EngineCli {
    /// Finds the engine: `SAKURA_BIN`, else the path set in Settings, else `sakura` on the PATH. A place
    /// that is named is the only one looked at: a wrong name is said, not hidden by another engine found
    /// further on. The error names every place that was looked at.
    pub fn locate(env: &Env) -> BridgeResult<Self> {
        let named = [&env.sakura_bin, &env.configured]
            .into_iter()
            .flatten()
            .find(|path| !path.as_os_str().is_empty());
        let candidates = match named {
            Some(path) => vec![path.clone()],
            None => search_path(&env.path),
        };
        let Some(bin) = candidates.iter().find(|path| is_executable(path)) else {
            return Err(missing(&candidates));
        };
        // The engine runs outside every cage: a program that others can replace is refused, by name.
        let real = trust::check(bin, trust::person()).map_err(|why| untrusted(bin, &why))?;
        Ok(Self {
            bin: real,
            path: env.path.clone(),
            leeway: leeway::bundled_helper_of_process(),
            tmux: tmux::bundled_tmux_of_process(),
            deadline: DEADLINE,
        })
    }

    /// The command at `bin`, with the PATH the core gives every child.
    pub fn at(bin: impl Into<PathBuf>) -> Self {
        Self {
            bin: bin.into(),
            path: launch_path(),
            leeway: leeway::bundled_helper_of_process(),
            tmux: tmux::bundled_tmux_of_process(),
            deadline: DEADLINE,
        }
    }

    /// Replaces the deadline of a read.
    pub fn with_deadline(mut self, deadline: Duration) -> Self {
        self.deadline = deadline;
        self
    }

    #[cfg(test)]
    /// Replaces the Leeway helper the commands are told about (the one the process's own bundle holds).
    pub fn with_leeway(mut self, leeway: Option<PathBuf>) -> Self {
        self.leeway = leeway;
        self
    }

    #[cfg(test)]
    /// Replaces the tmux the commands are told about (the one the process's own bundle holds).
    pub fn with_tmux(mut self, tmux: Option<PathBuf>) -> Self {
        self.tmux = tmux;
        self
    }

    /// Path of the command.
    pub fn bin(&self) -> &Path {
        &self.bin
    }

    /// Runs `sakura cockpit snapshot --json` and reads it. Within [`DEADLINE`]; refuses another API
    /// version than the app knows.
    pub async fn snapshot(&self) -> BridgeResult<Snapshot> {
        let args = [
            "cockpit".to_string(),
            "snapshot".to_string(),
            "--json".to_string(),
        ];
        let captured = self.run(&args, self.deadline).await?;
        if !captured.status.success() {
            return Err(failed(captured.status.code()));
        }
        Snapshot::parse(&captured.stdout)
    }

    /// Reads one bounded page of saved run history.
    pub async fn scheduled_history(
        &self,
        id: &str,
        before: Option<u64>,
    ) -> BridgeResult<super::scheduled::HistoryPage> {
        self.scheduled_read("history", id, "before", before).await
    }

    /// Reads one bounded page of a saved conversation, including an expiry response.
    pub async fn scheduled_conversation(
        &self,
        id: &str,
        after: Option<u64>,
    ) -> BridgeResult<super::scheduled::ConversationPage> {
        self.scheduled_read("conversation", id, "after", after)
            .await
    }

    async fn scheduled_read<T: serde::de::DeserializeOwned>(
        &self,
        command: &str,
        id: &str,
        cursor_name: &str,
        cursor: Option<u64>,
    ) -> BridgeResult<T> {
        let mut args = vec!["scheduled".into(), command.into(), "--json".into()];
        if let Some(cursor) = cursor {
            args.extend(option(cursor_name, &cursor.to_string()));
        }
        args.extend(["--".into(), id.into()]);
        let captured = self.run(&args, self.deadline).await?;
        match serde_json::from_slice(&captured.stdout) {
            Ok(page) => Ok(page),
            Err(_) if !captured.status.success() => Err(failed(captured.status.code())),
            Err(_) => Err(unreadable_answer()),
        }
    }

    /// Runs `sakura cockpit act ... --json`. Inputs are checked against the action's `needs` and
    /// `optional` before anything is started. The engine's three answers, a refusal included, come back
    /// as an [`ActResult`]; an engine that failed without one is an error.
    pub async fn act(&self, act: &Act) -> BridgeResult<ActResult> {
        let result = self.act_with(&act_arguments(act)?).await?;
        if let ActResult::Refused { reason, .. } = &result {
            if reason.starts_with("no longer waiting: the engine has moved on")
                && is_project_folder_missing(&act.target)
            {
                return Err(BridgeError::internal(Said::new(PROJECT_FOLDER_MISSING)));
            }
        }
        Ok(result)
    }

    /// Runs `sakura cockpit act add-project <folder> --json`: the engine registers the Git repository of
    /// `folder`, which must be an absolute path, and answers with the project or the reason it refused.
    pub async fn add_project(&self, folder: &str) -> BridgeResult<ActResult> {
        if !folder.starts_with('/') {
            return Err(invalid("folder"));
        }
        let mut args: Vec<String> = ["cockpit", "act", "add-project"].map(String::from).into();
        args.push(folder.to_string());
        args.extend(option("source", APP_SOURCE));
        args.push("--json".to_string());
        self.act_with(&args).await
    }

    async fn act_with(&self, args: &[String]) -> BridgeResult<ActResult> {
        let captured = self.run(args, ACT_DEADLINE.max(self.deadline)).await?;
        match serde_json::from_slice::<ActResult>(&captured.stdout) {
            Ok(result) => Ok(result),
            Err(_) if !captured.status.success() => Err(failed(captured.status.code())),
            Err(_) => Err(unreadable_answer()),
        }
    }

    /// Runs `sakura flow feed <task> --json` in the folder of the lane's repository and reads the page of
    /// the feed that follows the cursor `since` (the start of the feed for an empty one).
    pub async fn feed(&self, repo: &Path, task: &str, since: &str) -> BridgeResult<FeedPage> {
        check_task_and_folder(repo, task)?;
        let mut args: Vec<String> = ["flow", "feed", "--json"].map(String::from).into();
        if !since.is_empty() {
            args.push(format!("--since={since}"));
        }
        args.extend(["--".to_string(), task.to_string()]);
        let captured = self.run_in(&args, &[], Some(repo), self.deadline).await?;
        if !captured.status.success() {
            return Err(failed(captured.status.code()));
        }
        serde_json::from_slice(&captured.stdout).map_err(|_| unreadable_answer())
    }

    /// Runs `sakura flow sessions <task> --json` in the folder of the lane's repository and reads the lane's
    /// agents, one per run, in the order the engine lists them (by start).
    pub async fn agents(&self, repo: &Path, task: &str) -> BridgeResult<Vec<AgentRun>> {
        check_task_and_folder(repo, task)?;
        let args: Vec<String> = ["flow", "sessions", "--json", "--", task]
            .map(String::from)
            .into();
        self.read_in(&args, repo).await
    }

    /// Runs `sakura flow feed <task> --run <run> --json`: one agent's conversation after the cursor `since`
    /// (its start when none), with the text it is writing now. A run that could be read as an option is
    /// refused before anything starts.
    pub async fn agent_feed(
        &self,
        repo: &Path,
        task: &str,
        run: &str,
        since: Option<&str>,
    ) -> BridgeResult<AgentFeedPage> {
        check_task_and_folder(repo, task)?;
        check_run(run)?;
        let mut args: Vec<String> = ["flow", "feed", "--json"].map(String::from).into();
        args.push(format!("--run={run}"));
        if let Some(since) = since.filter(|since| !since.is_empty()) {
            args.push(format!("--since={since}"));
        }
        args.extend(["--".to_string(), task.to_string()]);
        self.read_in(&args, repo).await
    }

    /// Runs `sakura flow steer <task> --run <run> --actor app:<login> <text> --json`: the text goes to that agent alone, live,
    /// queued or as a resume of a finished run, as the engine decides, and it says which in words. The text
    /// follows `--`, so it is never read as an option.
    pub async fn write_to_agent(
        &self,
        repo: &Path,
        task: &str,
        run: &str,
        text: &str,
    ) -> BridgeResult<WriteOutcome> {
        check_task_and_folder(repo, task)?;
        check_run(run)?;
        if text.trim().is_empty() {
            return Err(invalid("text"));
        }
        let mut args: Vec<String> = ["flow", "steer"].map(String::from).into();
        args.push(format!("--run={run}"));
        // The ledger names who asked, as it does for every other action of the window.
        args.push(format!("--actor={}", super::actor::app_actor()));
        args.extend(["--json", "--", task, text].map(String::from));
        self.read_in(&args, repo).await
    }

    /// Runs `sakura engine rules --json` and reads the rules of how a lane runs.
    pub async fn rules(&self) -> BridgeResult<Rules> {
        let args = ["engine", "rules", "--json"].map(String::from);
        let captured = self.run(&args, self.deadline).await?;
        if !captured.status.success() {
            return Err(failed(captured.status.code()));
        }
        serde_json::from_slice(&captured.stdout).map_err(|_| unreadable_answer())
    }

    /// Runs `sakura cockpit phone --json` and reads the setup of the phone page. It changes nothing, in
    /// Tailscale or anywhere else.
    pub async fn phone(&self) -> BridgeResult<super::phone::PhoneSetup> {
        let args = ["cockpit", "phone", "--json"].map(String::from);
        let captured = self.run(&args, self.deadline).await?;
        if !captured.status.success() {
            return Err(failed(captured.status.code()));
        }
        super::phone::PhoneSetup::parse(&captured.stdout).map_err(|_| unreadable_answer())
    }

    /// Detects supported programs and login folders only when the person requests a search.
    pub async fn setup_detect(&self) -> BridgeResult<super::setup::SetupDetection> {
        let args = ["setup", "detect", "--json"].map(String::from);
        let captured = self.run(&args, self.deadline).await?;
        if !captured.status.success() {
            return Err(failed(captured.status.code()));
        }
        serde_json::from_slice(&captured.stdout).map_err(|_| unreadable_answer())
    }

    /// Runs `sakura preset list --json` and reads the presets that can be chosen, each with its name in words.
    pub async fn presets(&self) -> BridgeResult<PresetList> {
        let args = ["preset", "list", "--json"].map(String::from);
        let captured = self.run(&args, self.deadline).await?;
        if !captured.status.success() {
            return Err(failed(captured.status.code()));
        }
        PresetList::parse(&captured.stdout).map_err(|_| unreadable_answer())
    }

    /// Runs `sakura flow changes <task> --json` in the folder of the lane's repository: the files the lane
    /// changed. A lane whose worktree was cleaned up answers an empty list with `gone` set.
    pub async fn changes(&self, repo: &Path, task: &str) -> BridgeResult<ChangesPage> {
        let args = changes_arguments(repo, task, None)?;
        self.read_in(&args, repo).await
    }

    /// The patch of `path` (relative to the lane's worktree) of the lane `task`. A path that leaves the
    /// worktree, or could be read as an option, is refused before anything starts.
    pub async fn changes_file(
        &self,
        repo: &Path,
        task: &str,
        path: &str,
    ) -> BridgeResult<FilePatch> {
        let args = changes_arguments(repo, task, Some(path))?;
        self.read_in(&args, repo).await
    }

    async fn read_in<T: serde::de::DeserializeOwned>(
        &self,
        args: &[String],
        repo: &Path,
    ) -> BridgeResult<T> {
        let captured = self.run_in(args, &[], Some(repo), self.deadline).await?;
        if !captured.status.success() {
            return Err(failed(captured.status.code()));
        }
        serde_json::from_slice(&captured.stdout).map_err(|_| unreadable_answer())
    }

    /// Runs `sakura master spec --json` (`--account ID` for a master on one Leeway account), which writes the master's files first, and reads the launch spec
    /// it prints. A refusal of the engine (it prints its reason) is an error that carries the reason.
    pub async fn master_spec(&self, account: Option<&str>) -> BridgeResult<MasterSpec> {
        let mut args: Vec<String> = ["master", "spec", "--json"].map(String::from).into();
        if let Some(account) = account.filter(|account| !account.is_empty()) {
            args.extend(["--account".to_string(), account.to_string()]);
        }
        self.read_master_spec(&args).await
    }

    /// Prepares only the selected Goose account's operator launch.
    pub async fn goose_master_spec(
        &self,
        request: &super::master::GooseMasterRequest,
    ) -> BridgeResult<MasterSpec> {
        let mut args = vec![
            "master".into(),
            "spec".into(),
            "--json".into(),
            "--harness=goose".into(),
            format!("--account={}", request.account),
            format!("--slot={}", request.slot),
        ];
        if let Some(model) = &request.model {
            args.push(format!("--model={model}"));
        }
        if let Some(effort) = &request.effort {
            args.push(format!("--effort={effort}"));
        }
        if request.is_fresh {
            args.push("--start-fresh".into());
        }
        self.read_master_spec(&args).await
    }

    async fn read_master_spec(&self, args: &[String]) -> BridgeResult<MasterSpec> {
        let captured = self.run(args, self.deadline).await?;
        if !captured.status.success() {
            return Err(failed(captured.status.code()));
        }
        serde_json::from_slice(&captured.stdout).map_err(|_| unreadable_answer())
    }

    /// Runs `sakura master decide --json` with one permission request on its stdin and reads the engine's
    /// decision. Every answer of the engine exits 0, a refusal to run (inside a cage, for one) does not.
    pub async fn master_decide(&self, request: &serde_json::Value) -> BridgeResult<Decided> {
        let args = ["master", "decide", "--json"].map(String::from);
        let input = request.to_string();
        let captured = self
            .run_in(&args, input.as_bytes(), None, self.deadline)
            .await?;
        if !captured.status.success() {
            return Err(failed(captured.status.code()));
        }
        serde_json::from_slice(&captured.stdout).map_err(|_| unreadable_answer())
    }

    async fn run(&self, args: &[String], limit: Duration) -> BridgeResult<Captured> {
        self.run_in(args, &[], None, limit).await
    }

    /// Runs the command with `input` on its stdin, in `folder` when one is given.
    async fn run_in(
        &self,
        args: &[String],
        input: &[u8],
        folder: Option<&Path>,
        limit: Duration,
    ) -> BridgeResult<Captured> {
        let mut command = Command::new(&self.bin);
        command.args(args).env("PATH", &self.path);
        if let Some(leeway) = &self.leeway {
            command.env(leeway::LEEWAY_VAR, leeway);
        }
        if let Some(tmux) = &self.tmux {
            command.env(tmux::TMUX_VAR, tmux);
        }
        if let Some(folder) = folder {
            command.current_dir(folder);
        }
        let ended =
            process::capture_with_input(&mut command, input, limit, MAX_STDOUT_BYTES, |_| {}).await;
        ended.map_err(|ended| match ended {
            Ended::Spawn(error) if error.kind() == std::io::ErrorKind::NotFound => {
                missing(std::slice::from_ref(&self.bin))
            }
            Ended::Spawn(_) | Ended::Wait(_) => BridgeError::internal(Said::new("error.internal")),
            Ended::TimedOut => {
                let said = Said::new("error.engine.timeout").said("limit", limit_said(limit));
                BridgeError::new(ErrorCode::EngineFailed, said)
            }
            Ended::TooMuchOutput => BridgeError::new(
                ErrorCode::EngineFailed,
                Said::new("error.engine.too_much_output"),
            ),
        })
    }
}

fn is_project_folder_missing(target: &str) -> bool {
    let Ok(key) = serde_json::from_str::<Vec<String>>(target) else {
        return false;
    };
    let [kind, folder, _, ..] = key.as_slice() else {
        return false;
    };
    let folder = Path::new(folder);
    matches!(kind.as_str(), "flow" | "wait")
        && folder.is_absolute()
        && matches!(folder.try_exists(), Ok(false))
}

/// Who acts, as the engine records it: the person at this window, not at a terminal.
const APP_SOURCE: &str = "app";

fn act_arguments(act: &Act) -> BridgeResult<Vec<String>> {
    check_words(act)?;
    check_inputs(&act.action, &act.inputs)?;
    let mut args: Vec<String> = [
        "cockpit",
        "act",
        act.action.key.as_str(),
        act.target.as_str(),
    ]
    .map(String::from)
    .into();
    for (name, value) in act.inputs.iter().filter(|(_, value)| !value.is_empty()) {
        if act.is_track() {
            args.push(format!("--{name}={value}"));
        } else {
            args.extend(option(name, value));
        }
    }
    if let Some(token) = &act.confirm {
        args.extend(option("confirm", token));
    }
    args.extend(option("source", APP_SOURCE));
    args.push("--json".to_string());
    Ok(args)
}

/// An option and its value; a value that starts with a dash is joined with an equals sign, which
/// the option parser never reads as an option of its own.
fn option(name: &str, value: &str) -> Vec<String> {
    if value.starts_with('-') {
        vec![format!("--{name}={value}")]
    } else {
        vec![format!("--{name}"), value.to_string()]
    }
}

/// The arguments of `flow changes` for `task`, and for the patch of `path` when one is named. The task and
/// the path are checked before the engine starts.
fn changes_arguments(repo: &Path, task: &str, path: Option<&str>) -> BridgeResult<Vec<String>> {
    check_task_and_folder(repo, task)?;
    let mut args: Vec<String> = ["flow", "changes", "--json"].map(String::from).into();
    if let Some(path) = path {
        if !is_inside_worktree(path) {
            return Err(invalid("path"));
        }
        args.push(format!("--file={path}"));
    }
    args.extend(["--".to_string(), task.to_string()]);
    Ok(args)
}

/// Whether `path` names something below the worktree: relative, no `..` and nothing that reads as an option.
fn is_inside_worktree(path: &str) -> bool {
    let is_plain = !path.is_empty() && !path.starts_with(['/', '-']);
    is_plain && path.split('/').all(|part| part != "..")
}

/// The key is one action key and the target one of the snapshot's ids (compact JSON): neither can be
/// read by the option parser as an option.
fn check_words(act: &Act) -> BridgeResult<()> {
    let is_key =
        !act.action.key.is_empty() && act.action.key.chars().all(|c| c.is_ascii_alphanumeric());
    if !is_key {
        return Err(invalid("key"));
    }
    if !act.target.starts_with('[') {
        return Err(invalid("target"));
    }
    Ok(())
}

fn check_inputs(action: &Action, inputs: &BTreeMap<String, String>) -> BridgeResult<()> {
    for name in &action.needs {
        let is_given = inputs.get(name).is_some_and(|value| !value.is_empty());
        if !is_given {
            return Err(BridgeError::invalid(
                Said::new("error.engine.input_missing").text("name", name),
            ));
        }
    }
    if inputs
        .get("definition")
        .is_some_and(|value| value.len() > 32_768)
    {
        return Err(BridgeError::invalid(Said::new(
            "scheduled.reason.invalid_definition",
        )));
    }
    if matches!(action.key.as_str(), "connectoradd" | "connectoredit")
        && inputs
            .get("choices")
            .is_some_and(|value| value.len() > 70_000 || value.chars().any(char::is_control))
    {
        return Err(BridgeError::invalid(Said::new(
            "connectors.reason.invalid_choices",
        )));
    }
    for name in inputs.keys() {
        let is_taken = action.needs.contains(name) || action.optional.contains(name);
        if !is_taken || !INPUTS.contains(&name.as_str()) {
            return Err(invalid("input"));
        }
    }
    Ok(())
}

/// The folder of a lane's project is not there. The lane and the project are named, never the folder's
/// place on this Mac.
fn lane_folder_missing(repo: &Path, task: &str) -> BridgeError {
    let project = repo
        .file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
    let said = Said::new(LANE_FOLDER_MISSING)
        .text("lane", task)
        .text("project", project);
    BridgeError::internal(said)
}

fn check_task_and_folder(repo: &Path, task: &str) -> BridgeResult<()> {
    crate::validate::task(task).map_err(|_| invalid("task"))?;
    if !repo.is_dir() {
        return Err(lane_folder_missing(repo, task));
    }
    Ok(())
}

fn check_run(run: &str) -> BridgeResult<()> {
    if run.is_empty() || run.starts_with('-') {
        return Err(invalid("run"));
    }
    Ok(())
}

fn invalid(what: &'static str) -> BridgeError {
    let said = Said::new("error.engine.input_refused").text("name", what);
    BridgeError::invalid(said)
}

fn search_path(path: &str) -> Vec<PathBuf> {
    let path = OsString::from(path);
    let folders = std::env::split_paths(&path);
    folders
        .filter(|folder| !folder.as_os_str().is_empty())
        .map(|folder| folder.join(PROGRAM))
        .collect()
}

fn is_executable(path: &Path) -> bool {
    crate::platform::is_runnable(path)
}

/// A program the app will not run as the engine: which one, and why, in the interface's words.
fn untrusted(program: &Path, why: &Untrusted) -> BridgeError {
    let said = match why {
        Untrusted::NotOwned(real) => Said::new("error.engine.untrusted_owner")
            .text("path", program.display().to_string())
            .text("real", real.display().to_string()),
        Untrusted::FolderOwned(real, folder) => Said::new("error.engine.untrusted_folder")
            .text("path", program.display().to_string())
            .text("real", real.display().to_string())
            .text("folder", folder.display().to_string()),
        Untrusted::Writable(real, folder) => Said::new("error.engine.untrusted_writable")
            .text("path", program.display().to_string())
            .text("real", real.display().to_string())
            .text("folder", folder.display().to_string()),
        Untrusted::Unreadable(place) => Said::new("error.engine.untrusted_unreadable")
            .text("path", program.display().to_string())
            .text("real", place.display().to_string()),
    };
    BridgeError::new(ErrorCode::EngineMissing, said)
}

fn missing(places: &[PathBuf]) -> BridgeError {
    let places: Vec<String> = places
        .iter()
        .map(|place| place.display().to_string())
        .collect();
    let said = Said::new("error.engine.missing").text("places", places.join(", "));
    BridgeError::new(ErrorCode::EngineMissing, said)
}

fn failed(code: Option<i32>) -> BridgeError {
    let said = match u64::try_from(code.unwrap_or(-1)) {
        Ok(code) => Said::new("error.engine.failed").number("code", code),
        Err(_) => Said::new("error.engine.interrupted"),
    };
    BridgeError::new(ErrorCode::EngineFailed, said)
}

#[cfg(test)]
mod tests {
    use super::INPUTS;

    /// `sakura cockpit act --help` of the engine, as captured by `tests/fixtures/make-engine-real.py`.
    const ACT_HELP: &str = include_str!("../../tests/fixtures/engine-real-act-help.txt");

    #[test]
    fn every_input_the_app_may_pass_is_an_option_of_the_real_engine_command() {
        for name in INPUTS {
            assert!(
                ACT_HELP.contains(&format!("[--{name} ")),
                "the engine's `cockpit act` has no --{name}"
            );
        }
    }
}
