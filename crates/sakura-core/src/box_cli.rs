//! Runs the `box` binary: explicit path, explicit PATH, an argument vector and never a shell, with a
//! timeout, an output cap and error mapping to `BridgeError`.

use crate::box_words;
use crate::error::{BridgeError, BridgeResult, ErrorCode};
use crate::platform::{self, Launch};
use crate::said::Said;
use crate::validate;
use serde::de::DeserializeOwned;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

/// Environment variable that overrides the location of the `box` binary.
pub const BOX_BIN_VAR: &str = "SAKURA_BOX_BIN";
/// PATH given to every child: apps started from Finder do not get the login PATH.
#[cfg(target_os = "macos")]
pub const CHILD_PATH: &str =
    "/usr/local/bin:/opt/homebrew/bin:/usr/bin:/bin:/Applications/Docker.app/Contents/Resources/bin";
/// PATH given to every child: the system's own folders, whatever started the app.
#[cfg(not(target_os = "macos"))]
pub const CHILD_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";
/// The channel `box` is called through, which `box` asks every caller to name.
const CHANNEL: (&str, &str) = ("BOX_CHANNEL", "sakura");

/// The environment given to every `box` child, whatever starts it (a call, a terminal, an agent's
/// session): the explicit PATH and the channel. This is the one place it is written.
#[cfg(not(windows))]
pub const CHILD_ENV: [(&str, &str); 2] = [("PATH", CHILD_PATH), CHANNEL];
/// The environment given to every `box` child on Windows, where `box` runs under WSL: the channel, and
/// the variable that carries it into WSL. The PATH is WSL's own, and Windows needs its own to find WSL.
#[cfg(windows)]
pub const CHILD_ENV: [(&str, &str); 2] = [("WSLENV", "BOX_CHANNEL"), CHANNEL];

/// How the agent of a conversation is started: an argument vector, never a shell line, and the
/// environment of every `box` child.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpCommand {
    pub argv: Vec<String>,
    pub env: Vec<(String, String)>,
}

/// Whether `text` (what a `box` that failed wrote last) is its usage: it was requested a command it does
/// not have.
pub fn is_usage(text: &str) -> bool {
    text.contains(USAGE_PREFIX)
}

const MAX_STDOUT_BYTES: usize = 16 * 1024 * 1024;
const STDERR_TAIL_BYTES: usize = 64 * 1024;
const MESSAGE_MAX_CHARS: usize = 500;
const READ_CHUNK_BYTES: usize = 16 * 1024;
const DRAIN_GRACE: Duration = Duration::from_secs(2);
const NOT_FOUND_EXIT_CODE: i32 = 3;
/// What `box` does with a command it does not know: its usage on stderr and this exit code.
const USAGE_EXIT_CODE: i32 = 2;
const USAGE_PREFIX: &str = "usage:";
/// `box cancel` when there was nothing queued or running to cancel.
const NOTHING_TO_CANCEL_EXIT_CODE: i32 = 78;
const DOCKER_DOWN_MARKERS: [&str; 4] = [
    "cannot connect to the docker daemon",
    "is the docker daemon running",
    "docker daemon is not running",
    "docker is not running",
];

/// How long a call may take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Limit {
    /// Status-like reads.
    Read,
    /// The task inventory (`box tasks`), which starts no container: the window's first rows wait on it.
    Names,
    /// Workspace facts: `box` starts a sandbox container per task, about ten seconds each.
    Slow,
    /// Diffs, which can be large.
    Diff,
    /// Short actions: stop, remove, queue a verification, start the runner.
    Action,
    /// `take`, which copies the work out of the workspace. `new` has its own limit, see [`Timeouts::new`].
    Take,
    /// The run of the checks that wait (`box runner --once`): it lasts as long as they do.
    Queue,
}

/// The longest a run of the checks that wait may last: several checks, each with the deadline `box` gives
/// a check (45 minutes).
const QUEUE_LIMIT: Duration = Duration::from_secs(6 * 60 * 60);

/// Timeout values per [`Limit`]. Production values by default; tests shorten them.
#[derive(Debug, Clone, Copy)]
pub struct Timeouts {
    pub read: Duration,
    pub names: Duration,
    pub slow: Duration,
    pub diff: Duration,
    pub action: Duration,
    pub take: Duration,
    /// `box new`, run as a job: cloning and installing can take long, but never forever.
    pub new: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            read: Duration::from_secs(10),
            names: Duration::from_secs(3),
            slow: Duration::from_secs(60),
            diff: Duration::from_secs(30),
            action: Duration::from_secs(120),
            take: Duration::from_secs(10 * 60),
            new: Duration::from_secs(45 * 60),
        }
    }
}

impl Timeouts {
    fn for_limit(&self, limit: Limit) -> Duration {
        match limit {
            Limit::Read => self.read,
            Limit::Names => self.names,
            Limit::Slow => self.slow,
            Limit::Diff => self.diff,
            Limit::Action => self.action,
            Limit::Take => self.take,
            Limit::Queue => QUEUE_LIMIT,
        }
    }
}

/// A limit to say: whole minutes from one minute up, else seconds.
pub fn limit_said(limit: Duration) -> Said {
    let seconds = limit.as_secs();
    match (seconds / 60, seconds % 60) {
        (minutes, 0) if minutes >= 1 => Said::new("limit.minutes").count(minutes),
        _ => Said::new("limit.seconds").count(seconds),
    }
}

/// Handle on the `box` binary.
#[derive(Debug, Clone)]
pub struct BoxCli {
    bin: PathBuf,
    timeouts: Timeouts,
    max_stdout: usize,
}

struct Captured {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    status: ExitStatus,
}

impl BoxCli {
    /// Uses `SAKURA_BOX_BIN` when set, else the `box` of the service carried with the app.
    pub fn from_environment() -> Self {
        let bin = std::env::var_os(BOX_BIN_VAR)
            .map(PathBuf::from)
            .unwrap_or_else(platform::default_box);
        Self::at(bin)
    }

    /// Uses the binary at `bin`.
    pub fn at(bin: impl Into<PathBuf>) -> Self {
        Self {
            bin: bin.into(),
            timeouts: Timeouts::default(),
            max_stdout: MAX_STDOUT_BYTES,
        }
    }

    /// Replaces the timeouts.
    pub fn with_timeouts(mut self, timeouts: Timeouts) -> Self {
        self.timeouts = timeouts;
        self
    }

    /// Replaces the stdout cap.
    pub fn with_max_stdout(mut self, max_stdout: usize) -> Self {
        self.max_stdout = max_stdout;
        self
    }

    /// How long `box new` may run.
    pub fn new_limit(&self) -> Duration {
        self.timeouts.new
    }

    /// Path of the binary.
    pub fn bin(&self) -> &Path {
        &self.bin
    }

    /// Fails with `box-missing` unless the binary exists and is executable.
    pub fn check(&self) -> BridgeResult<()> {
        if platform::is_runnable(&self.bin) {
            Ok(())
        } else {
            let path = self.bin.display().to_string();
            Err(BridgeError::new(
                ErrorCode::BoxMissing,
                Said::new("error.box.missing_at").text("path", path),
            ))
        }
    }

    /// Runs `box <args>` and returns stdout.
    pub async fn run_bytes(&self, args: &[&str], limit: Limit) -> BridgeResult<Vec<u8>> {
        self.check()?;
        let captured = self.capture(args, self.timeouts.for_limit(limit)).await?;
        if captured.status.success() {
            Ok(captured.stdout)
        } else {
            Err(failure(captured.status.code(), &captured.stderr))
        }
    }

    /// Runs `box <args>` to its end, telling `on_start` the process it started, and returns the last line
    /// it wrote on stderr when it succeeded: `box` warns there about what it left undone.
    pub async fn run_warned(
        &self,
        args: &[&str],
        limit: Limit,
        on_start: impl FnOnce(Option<u32>) + Send,
    ) -> BridgeResult<Option<String>> {
        self.check()?;
        let limit = self.timeouts.for_limit(limit);
        let captured = self.capture_started(args, limit, on_start).await?;
        if captured.status.success() {
            Ok(last_line(&String::from_utf8_lossy(&captured.stderr)))
        } else {
            Err(failure(captured.status.code(), &captured.stderr))
        }
    }

    /// Runs `box <args>` and returns stdout as text (invalid UTF-8 is replaced).
    pub async fn run_text(&self, args: &[&str], limit: Limit) -> BridgeResult<String> {
        let bytes = self.run_bytes(args, limit).await?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    /// Runs `box <args>` and parses stdout as one JSON document.
    pub async fn run_json<T: DeserializeOwned>(
        &self,
        args: &[&str],
        limit: Limit,
    ) -> BridgeResult<T> {
        let bytes = self.run_bytes(args, limit).await?;
        serde_json::from_slice(&bytes).map_err(|error| {
            BridgeError::failed(Said::new("error.box.unreadable_answer"))
                .with_details(Some(error.to_string()))
        })
    }

    /// The program, arguments and environment of the conversation's agent for `task`: `box acp <task>`,
    /// with the account when one is named (a lavoro in a box needs it; one on the Mac takes its
    /// project's when none is). The names are checked here; this is the one place the command is written.
    pub fn acp(&self, task: &str, account: Option<&str>) -> BridgeResult<AcpCommand> {
        let launch = Launch::of(&self.bin);
        let mut argv = vec![launch.program.to_string_lossy().into_owned()];
        argv.extend(launch.leading);
        argv.extend(["acp".to_string(), validate::task(task)?.to_string()]);
        if let Some(account) = account {
            argv.push(validate::account(account)?.to_string());
        }
        let env = CHILD_ENV
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()));
        Ok(AcpCommand {
            argv,
            env: env.collect(),
        })
    }

    /// A command with the environment of every `box` child ([`CHILD_ENV`]), no stdin and its own process
    /// group. The caller sets the pipes.
    pub fn command(&self, args: &[&str]) -> Command {
        let launch = Launch::of(&self.bin);
        let mut command = Command::new(&launch.program);
        command
            .args(&launch.leading)
            .args(args)
            .envs(CHILD_ENV)
            .stdin(Stdio::null());
        #[cfg(unix)]
        command.process_group(0);
        command
    }

    async fn capture(&self, args: &[&str], limit: Duration) -> BridgeResult<Captured> {
        self.capture_started(args, limit, |_| {}).await
    }

    /// Runs `box <args>` and collects what it writes; `on_start` is told the process once it runs.
    async fn capture_started(
        &self,
        args: &[&str],
        limit: Duration,
        on_start: impl FnOnce(Option<u32>) + Send,
    ) -> BridgeResult<Captured> {
        let mut child = self
            .command(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(spawn_error)?;
        let pid = child.id();
        on_start(pid);
        let (stdout, stderr) = (child.stdout.take(), child.stderr.take());
        let work = collect(child, stdout, stderr, self.max_stdout, pid);
        tokio::time::timeout(limit, work).await.unwrap_or_else(|_| {
            kill_group(pid);
            let said = Said::new("error.box.timeout").said("limit", limit_said(limit));
            Err(BridgeError::failed(said))
        })
    }
}

/// Asks a process started by [`BoxCli::command`] to end (TERM): `box` then ends what it runs itself.
#[cfg(unix)]
pub fn ask_to_end(pid: Option<u32>) {
    let Some(process) = pid
        .and_then(|pid| i32::try_from(pid).ok())
        .filter(|pid| *pid > 1)
    else {
        return;
    };
    // SAFETY: kill takes plain integers and has no memory effects. The id is a child we spawned, and ids
    // 0 and 1 are excluded above.
    unsafe {
        libc::kill(process, libc::SIGTERM);
    }
}

/// Ends the WSL launcher and its children on Windows, where POSIX signals are unavailable.
#[cfg(not(unix))]
pub fn ask_to_end(pid: Option<u32>) {
    kill_group(pid);
}

/// Kills a process group started by [`BoxCli::command`].
pub fn kill_group(pid: Option<u32>) {
    if let Some(pid) = pid {
        platform::kill_tree(pid);
    }
}

/// Last non-empty line of `text`, stripped of control characters and truncated.
pub fn last_line(text: &str) -> Option<String> {
    let line = text
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())?;
    Some(sanitize_line(line))
}

/// Strips control characters and truncates, for text that comes from a workspace or a process.
pub fn sanitize_line(line: &str) -> String {
    line.chars()
        .filter(|c| !c.is_control())
        .take(MESSAGE_MAX_CHARS)
        .collect()
}

fn spawn_error(error: std::io::Error) -> BridgeError {
    spawn_error_on(platform::System::current(), error)
}

fn spawn_error_on(system: platform::System, error: std::io::Error) -> BridgeError {
    match error.kind() {
        std::io::ErrorKind::NotFound => {
            let key = match system {
                platform::System::Windows => "error.box.wsl_missing",
                _ => "error.box.missing",
            };
            BridgeError::new(ErrorCode::BoxMissing, Said::new(key))
        }
        _ => internal(format!("box could not be started: {error}")),
    }
}

/// What is said of a `box` that failed without printing a line.
fn ended_without_a_word(code: Option<i32>) -> Said {
    match u64::try_from(code.unwrap_or(-1)) {
        Ok(code) => Said::new("error.box.ended_with_code").number("code", code),
        Err(_) => Said::new("error.box.interrupted"),
    }
}

/// A failure inside Sakura itself: the general sentence, with what the system said as the details.
pub fn internal(details: String) -> BridgeError {
    BridgeError::internal(Said::new("error.internal")).with_details(Some(details))
}

/// Maps a failed `box` run to a [`BridgeError`] from its exit code and stderr.
pub fn failure(code: Option<i32>, stderr: &[u8]) -> BridgeError {
    let text = String::from_utf8_lossy(stderr);
    let lowered = text.to_lowercase();
    if DOCKER_DOWN_MARKERS
        .iter()
        .any(|marker| lowered.contains(marker))
    {
        return BridgeError::new(ErrorCode::DockerDown, Said::new("error.docker.down"));
    }
    if code == Some(USAGE_EXIT_CODE) && lowered.trim_start().starts_with(USAGE_PREFIX) {
        return BridgeError::new(ErrorCode::Unsupported, Said::new("error.box.unsupported"));
    }
    // The last line `box` printed is English: the person gets Sakura's sentence for it, and the line stays
    // as the details.
    let told = last_line(&text).map(|line| box_words::said(&line));
    let (said, details) = match told {
        Some(failure) => (failure.said, failure.details),
        None => (ended_without_a_word(code), None),
    };
    let kind = match code {
        Some(NOT_FOUND_EXIT_CODE) => ErrorCode::NotFound,
        Some(NOTHING_TO_CANCEL_EXIT_CODE) => ErrorCode::NothingToCancel,
        _ => ErrorCode::BoxFailed,
    };
    BridgeError::new(kind, said).with_details(details)
}

async fn collect(
    mut child: tokio::process::Child,
    stdout: Option<impl AsyncRead + Unpin>,
    stderr: Option<impl AsyncRead + Unpin>,
    max_stdout: usize,
    pid: Option<u32>,
) -> BridgeResult<Captured> {
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let (status, exceeded) = {
        let readers = async {
            let (is_over, ()) = tokio::join!(
                read_capped(stdout, &mut out, max_stdout, pid),
                read_tail(stderr, &mut err)
            );
            is_over
        };
        wait_and_drain(&mut child, readers).await
    };
    if exceeded {
        return Err(BridgeError::failed(Said::new("error.box.too_much_output")));
    }
    let status = status.map_err(|error| internal(format!("waiting for box failed: {error}")))?;
    Ok(Captured {
        stdout: out,
        stderr: err,
        status,
    })
}

/// Waits for the child and for its output readers. Output held open by a detached grandchild must not
/// hang the call, so once the child has exited the readers get only a short grace period.
async fn wait_and_drain(
    child: &mut tokio::process::Child,
    readers: impl std::future::Future<Output = bool>,
) -> (std::io::Result<ExitStatus>, bool) {
    tokio::pin!(readers);
    tokio::select! {
        is_over = &mut readers => (child.wait().await, is_over),
        status = child.wait() => {
            let drained = tokio::time::timeout(DRAIN_GRACE, &mut readers).await;
            (status, drained.unwrap_or(false))
        }
    }
}

/// Reads into `buffer` up to `cap` bytes. Past the cap it kills the process group and reports `true`.
async fn read_capped(
    reader: Option<impl AsyncRead + Unpin>,
    buffer: &mut Vec<u8>,
    cap: usize,
    pid: Option<u32>,
) -> bool {
    let Some(mut reader) = reader else {
        return false;
    };
    let mut chunk = vec![0u8; READ_CHUNK_BYTES];
    while let Ok(read) = reader.read(&mut chunk).await {
        if read == 0 {
            return false;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if buffer.len() > cap {
            kill_group(pid);
            return true;
        }
    }
    false
}

/// Reads everything but keeps only the last `STDERR_TAIL_BYTES`, so a chatty process never blocks.
async fn read_tail(reader: Option<impl AsyncRead + Unpin>, buffer: &mut Vec<u8>) {
    let Some(mut reader) = reader else { return };
    let mut chunk = vec![0u8; READ_CHUNK_BYTES];
    while let Ok(read) = reader.read(&mut chunk).await {
        if read == 0 {
            return;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if buffer.len() > STDERR_TAIL_BYTES {
            buffer.drain(..buffer.len() - STDERR_TAIL_BYTES);
        }
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use crate::test_support::write_script;

    /// Writes an executable `box` script with the given shell body into `dir`. Its limits are
    /// patient ones: a test that is not about a limit must not fail because the machine was busy.
    pub fn fake_box(dir: &Path, body: &str) -> BoxCli {
        let path = dir.join("box");
        write_script(&path, &format!("#!/bin/sh\n{body}\n"));
        BoxCli::at(path).with_timeouts(crate::test_support::patient())
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::fake_box;
    use super::*;
    use std::time::Instant;

    #[test]
    fn missing_wsl_has_its_own_installation_message() {
        let error = spawn_error_on(
            platform::System::Windows,
            std::io::Error::from(std::io::ErrorKind::NotFound),
        );
        assert_eq!(error.code, ErrorCode::BoxMissing);
        assert_eq!(error.said, Said::new("error.box.wsl_missing"));
        let unix = spawn_error_on(
            platform::System::Linux,
            std::io::Error::from(std::io::ErrorKind::NotFound),
        );
        assert_eq!(unix.said, Said::new("error.box.missing"));
    }

    #[cfg(windows)]
    #[test]
    fn the_agent_command_uses_wsl_and_carries_its_channel_without_replacing_path() {
        let cli = BoxCli::at(r"C:\Tools\Sakura\service\bin\box");
        let named = cli.acp("m1", Some("personal")).unwrap();
        assert_eq!(
            named.argv,
            [
                "wsl.exe",
                "--exec",
                "bash",
                "/mnt/c/Tools/Sakura/service/bin/box",
                "acp",
                "m1",
                "personal"
            ]
        );
        assert!(named.env.contains(&("WSLENV".into(), "BOX_CHANNEL".into())));
        assert!(named.env.contains(&("BOX_CHANNEL".into(), "sakura".into())));
        assert!(!named.env.iter().any(|(key, _)| key == "PATH"));
        assert!(cli.acp("m1; rm -rf /", None).is_err());
        assert!(cli.acp("m1", Some("--flag")).is_err());
    }

    fn short_timeouts() -> Timeouts {
        Timeouts {
            read: Duration::from_millis(400),
            names: Duration::from_millis(400),
            slow: Duration::from_millis(400),
            diff: Duration::from_millis(400),
            action: Duration::from_millis(400),
            take: Duration::from_secs(10),
            new: Duration::from_millis(400),
        }
    }

    #[cfg_attr(windows, ignore = "Unix shell fixture")]
    #[tokio::test]
    async fn run_text_returns_stdout_and_passes_arguments_as_a_vector() {
        let dir = tempfile::tempdir().unwrap();
        let cli = fake_box(dir.path(), r#"for a in "$@"; do echo "[$a]"; done"#);
        let out = cli
            .run_text(&["a b", "$(id)", ";ls"], Limit::Read)
            .await
            .unwrap();
        assert_eq!(out, "[a b]\n[$(id)]\n[;ls]\n");
    }

    #[cfg_attr(windows, ignore = "Unix shell fixture")]
    #[tokio::test]
    async fn child_gets_the_explicit_path() {
        let dir = tempfile::tempdir().unwrap();
        let cli = fake_box(dir.path(), r#"echo "$PATH""#);
        let out = cli.run_text(&[], Limit::Read).await.unwrap();
        assert_eq!(out.trim(), CHILD_PATH);
    }

    #[cfg(unix)]
    #[test]
    fn the_agent_s_command_is_box_acp_with_checked_names_and_the_child_environment() {
        let cli = BoxCli::at("/opt/box");
        let alone = cli.acp("m1", None).unwrap();
        assert_eq!(alone.argv, ["/opt/box", "acp", "m1"]);
        let named = cli.acp("m1", Some("personal")).unwrap();
        assert_eq!(named.argv, ["/opt/box", "acp", "m1", "personal"]);
        let channel = ("BOX_CHANNEL".to_string(), "sakura".to_string());
        assert!(named.env.contains(&channel), "{:?}", named.env);
        assert!(named.env.iter().any(|(name, _)| name == "PATH"));
        assert!(
            cli.acp("m1; rm -rf /", None).is_err(),
            "a name is never a shell word"
        );
        assert!(cli.acp("m1", Some("--flag")).is_err());
    }

    #[test]
    fn the_usage_of_box_is_told_from_what_any_other_failure_wrote() {
        assert!(is_usage("exit code 2: usage: box <command>"));
        assert!(is_usage("usage: box <command>"));
        assert!(!is_usage("box: a conversation is open for task 'm1'"));
        assert!(!is_usage(""));
    }

    #[cfg_attr(windows, ignore = "Unix shell fixture")]
    #[tokio::test]
    async fn every_call_says_it_comes_from_sakura() {
        let dir = tempfile::tempdir().unwrap();
        let cli = fake_box(dir.path(), r#"echo "$BOX_CHANNEL""#);
        let out = cli.run_text(&["status"], Limit::Read).await.unwrap();
        assert_eq!(out.trim(), "sakura");
    }

    #[cfg_attr(windows, ignore = "Unix shell fixture")]
    #[tokio::test]
    async fn failure_is_said_in_italian_and_keeps_the_last_stderr_line_as_details() {
        let dir = tempfile::tempdir().unwrap();
        let cli = fake_box(
            dir.path(),
            "echo first >&2\necho 'workspace is locked' >&2\nexit 1",
        );
        let error = cli.run_text(&["x"], Limit::Read).await.unwrap_err();
        assert_eq!(error.code, ErrorCode::BoxFailed);
        assert_eq!(
            error.said,
            Said::from(box_words::UNKNOWN),
            "a line Sakura does not know"
        );
        assert_eq!(error.details.as_deref(), Some("workspace is locked"));
    }

    #[cfg_attr(windows, ignore = "Unix shell fixture")]
    #[tokio::test]
    async fn a_failure_box_names_is_its_own_sentence() {
        let dir = tempfile::tempdir().unwrap();
        let body = r#"echo "box: task 'm9' does not exist (box new m9)" >&2; exit 1"#;
        let cli = fake_box(dir.path(), body);
        let error = cli
            .run_text(&["facts", "m9"], Limit::Read)
            .await
            .unwrap_err();
        assert_eq!(error.said, Said::new("box.failure.no_such_task"));
        let details = error.details.expect("the line box wrote");
        assert!(
            details.starts_with("box: task 'm9' does not exist"),
            "{details}"
        );
    }

    #[cfg_attr(windows, ignore = "Unix shell fixture")]
    #[tokio::test]
    async fn exit_code_3_maps_to_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let cli = fake_box(dir.path(), "echo 'no result' >&2\nexit 3");
        let error = cli.run_text(&[], Limit::Read).await.unwrap_err();
        assert_eq!(error.code, ErrorCode::NotFound);
        assert_eq!(error.details.as_deref(), Some("no result"));
    }

    #[cfg_attr(windows, ignore = "Unix shell fixture")]
    #[tokio::test]
    async fn a_command_box_does_not_know_maps_to_unsupported() {
        let dir = tempfile::tempdir().unwrap();
        let body = "echo 'usage: box <command>' >&2\necho '  up   start' >&2\nexit 2";
        let cli = fake_box(dir.path(), body);
        let error = cli.run_text(&["tasks"], Limit::Names).await.unwrap_err();
        assert_eq!(error.code, ErrorCode::Unsupported);
        let other = fake_box(dir.path(), "echo 'box: bad flag' >&2\nexit 2");
        let error = other.run_text(&["x"], Limit::Read).await.unwrap_err();
        assert_eq!(error.code, ErrorCode::BoxFailed);
    }

    #[cfg_attr(windows, ignore = "Unix shell fixture")]
    #[tokio::test]
    async fn docker_not_running_maps_to_docker_down() {
        let dir = tempfile::tempdir().unwrap();
        let body = "echo 'Cannot connect to the Docker daemon at unix:///x. Is the docker daemon running?' >&2\nexit 1";
        let cli = fake_box(dir.path(), body);
        let error = cli.run_text(&[], Limit::Read).await.unwrap_err();
        assert_eq!(error.code, ErrorCode::DockerDown);
    }

    #[cfg_attr(windows, ignore = "Unix shell fixture")]
    #[tokio::test]
    async fn silent_failure_still_has_a_message() {
        let dir = tempfile::tempdir().unwrap();
        let cli = fake_box(dir.path(), "exit 7");
        let error = cli.run_text(&[], Limit::Read).await.unwrap_err();
        let ended = Said::new("error.box.ended_with_code").number("code", 7);
        assert_eq!(error.said, ended);
    }

    #[cfg_attr(windows, ignore = "Unix shell fixture")]
    #[tokio::test]
    async fn slow_call_times_out_and_kills_its_children() {
        let dir = tempfile::tempdir().unwrap();
        let cli = fake_box(dir.path(), "sleep 30\necho late").with_timeouts(short_timeouts());
        let started = Instant::now();
        let error = cli.run_text(&[], Limit::Read).await.unwrap_err();
        assert_eq!(error.code, ErrorCode::BoxFailed);
        assert_eq!(error.said.key.0, "error.box.timeout");
        // The child sleeps thirty seconds: the call is back long before that, on a busy machine too.
        assert!(started.elapsed() < Duration::from_secs(20));
    }

    #[cfg_attr(windows, ignore = "Unix shell fixture")]
    #[tokio::test]
    async fn take_has_its_own_longer_limit() {
        let dir = tempfile::tempdir().unwrap();
        let cli = fake_box(dir.path(), "sleep 1\necho done").with_timeouts(short_timeouts());
        let out = cli.run_text(&[], Limit::Take).await.unwrap();
        assert_eq!(out.trim(), "done");
    }

    #[cfg_attr(windows, ignore = "Unix shell fixture")]
    #[tokio::test]
    async fn a_take_past_its_limit_is_killed_and_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let timeouts = Timeouts {
            take: Duration::from_secs(2),
            ..short_timeouts()
        };
        let cli = fake_box(dir.path(), "sleep 30").with_timeouts(timeouts);
        let started = Instant::now();
        let error = cli
            .run_text(&["take", "m1"], Limit::Take)
            .await
            .unwrap_err();
        let limit = Said::new("limit.seconds").count(2);
        assert_eq!(
            error.said,
            Said::new("error.box.timeout").said("limit", limit)
        );
        // The child sleeps thirty seconds: the call is back long before that, on a busy machine too.
        assert!(started.elapsed() < Duration::from_secs(20));
    }

    #[test]
    fn production_limits_for_take_and_new_are_ten_and_forty_five_minutes() {
        let limits = Timeouts::default();
        let minutes = |count| Said::new("limit.minutes").count(count);
        assert_eq!(limit_said(limits.take), minutes(10));
        assert_eq!(limit_said(limits.new), minutes(45));
        assert_eq!(limit_said(Duration::from_secs(60)), minutes(1));
        assert_eq!(
            limit_said(Duration::from_secs(10)),
            Said::new("limit.seconds").count(10)
        );
    }

    #[cfg_attr(windows, ignore = "Unix shell fixture")]
    #[tokio::test]
    async fn garbage_json_is_an_error_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        let cli = fake_box(dir.path(), "echo 'not json'");
        let error = cli
            .run_json::<Vec<String>>(&[], Limit::Read)
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::BoxFailed);
        assert_eq!(error.said, Said::new("error.box.unreadable_answer"));
        assert!(error.details.is_some(), "what the parser said is kept");
    }

    #[cfg_attr(windows, ignore = "Unix shell fixture")]
    #[tokio::test]
    async fn oversized_output_is_rejected_and_the_process_killed() {
        let dir = tempfile::tempdir().unwrap();
        let cli = fake_box(dir.path(), "yes | head -c 5000000").with_max_stdout(1024 * 1024);
        let error = cli.run_bytes(&[], Limit::Read).await.unwrap_err();
        assert_eq!(error.said, Said::new("error.box.too_much_output"));
    }

    #[cfg_attr(windows, ignore = "Unix shell fixture")]
    #[tokio::test]
    async fn chatty_stderr_does_not_block_and_keeps_the_tail() {
        let dir = tempfile::tempdir().unwrap();
        let body = "yes noise | head -c 300000 >&2\necho 'the end' >&2\nexit 1";
        let cli = fake_box(dir.path(), body);
        let error = cli.run_text(&[], Limit::Read).await.unwrap_err();
        assert_eq!(error.details.as_deref(), Some("the end"));
    }

    #[tokio::test]
    async fn missing_binary_is_box_missing_on_every_call() {
        let cli = BoxCli::at("/nonexistent/dir/box");
        assert_eq!(cli.check().unwrap_err().code, ErrorCode::BoxMissing);
        let error = cli.run_text(&[], Limit::Read).await.unwrap_err();
        assert_eq!(error.code, ErrorCode::BoxMissing);
    }

    #[cfg_attr(windows, ignore = "Unix shell fixture")]
    #[tokio::test]
    async fn non_executable_file_is_box_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("box");
        std::fs::write(&path, "#!/bin/sh\n").unwrap();
        let error = BoxCli::at(path).check().unwrap_err();
        assert_eq!(error.code, ErrorCode::BoxMissing);
    }

    #[cfg_attr(windows, ignore = "Unix shell fixture")]
    #[tokio::test]
    async fn output_held_open_by_a_detached_grandchild_does_not_hang() {
        let dir = tempfile::tempdir().unwrap();
        let cli = fake_box(dir.path(), "(sleep 20 &)\necho started");
        let started = Instant::now();
        let out = cli.run_text(&[], Limit::Read).await.unwrap();
        assert_eq!(out.trim(), "started");
        // The grandchild holds the pipe for twenty seconds: the call does not wait for it.
        assert!(started.elapsed() < Duration::from_secs(15));
    }

    #[test]
    fn last_line_skips_blank_lines_and_strips_control_characters() {
        assert_eq!(
            last_line("a\n\x1b[31mb\x1b[0m  \n\n").as_deref(),
            Some("[31mb[0m")
        );
        assert_eq!(last_line("  \n"), None);
    }
}
