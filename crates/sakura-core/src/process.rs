//! What every command runner of the core shares: ending a process group, one line of a process's last
//! words, and running a command to its end with a deadline, an output cap and the output of both pipes.
//! The runners ([`crate::box_cli`], [`crate::engine::cli`]) map what happened to their own errors.

use crate::error::BridgeError;
use crate::platform;
use crate::said::Said;
use std::process::{ExitStatus, Stdio};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

/// The longest a line of a process's words is kept, in characters.
const MESSAGE_MAX_CHARS: usize = 500;
/// What stderr keeps: its last bytes, so a chatty process never blocks.
const STDERR_TAIL_BYTES: usize = 64 * 1024;
const READ_CHUNK_BYTES: usize = 16 * 1024;
/// How long the readers get once the child has exited.
const DRAIN_GRACE: Duration = Duration::from_secs(2);

pub(crate) const USAGE_PREFIX: &str = "usage:";

/// Recognizes a CLI usage error even when the process wrapper prefixes it with an exit status.
pub fn is_usage(text: &str) -> bool {
    text.contains(USAGE_PREFIX)
}

/// PATH given to every child: apps started from Finder do not get the login PATH.
#[cfg(target_os = "macos")]
pub const CHILD_PATH: &str =
    "/usr/local/bin:/opt/homebrew/bin:/usr/bin:/bin:/Applications/Docker.app/Contents/Resources/bin";
/// PATH given to every child: the system's own folders, whatever started the app.
#[cfg(not(target_os = "macos"))]
pub const CHILD_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

/// A limit to say: whole minutes from one minute up, else seconds.
pub fn limit_said(limit: Duration) -> Said {
    let seconds = limit.as_secs();
    match (seconds / 60, seconds % 60) {
        (minutes, 0) if minutes >= 1 => Said::new("limit.minutes").count(minutes),
        _ => Said::new("limit.seconds").count(seconds),
    }
}

/// A failure inside Sakura itself: the general sentence, with what the system said as the details.
pub fn internal(details: String) -> BridgeError {
    BridgeError::internal(Said::new("error.internal")).with_details(Some(details))
}

/// What a command wrote and how it ended.
pub struct Captured {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub status: ExitStatus,
}

/// Why a command did not run to its end.
#[derive(Debug)]
pub enum Ended {
    /// The program could not be started.
    Spawn(std::io::Error),
    /// The deadline passed; the process group was killed.
    TimedOut,
    /// Stdout went past its cap; the process group was killed.
    TooMuchOutput,
    /// Waiting for the child failed.
    Wait(std::io::Error),
}

/// Runs `command` with piped output, its own process group (on Unix) and no stdin, telling `on_start` the
/// process once it runs. Past `limit` the whole group, or on Windows the whole tree, is killed.
pub async fn capture(
    command: &mut Command,
    limit: Duration,
    max_stdout: usize,
    on_start: impl FnOnce(Option<u32>) + Send,
) -> Result<Captured, Ended> {
    capture_with_input(command, &[], limit, max_stdout, on_start).await
}

/// Like [`capture`], with `input` written to the command's stdin (closed after it; no stdin at all when
/// `input` is empty). The deadline covers the writing too: a command that never reads is killed at it.
pub async fn capture_with_input(
    command: &mut Command,
    input: &[u8],
    limit: Duration,
    max_stdout: usize,
    on_start: impl FnOnce(Option<u32>) + Send,
) -> Result<Captured, Ended> {
    capture_interruptibly(
        command,
        input,
        limit,
        max_stdout,
        on_start,
        std::future::pending(),
    )
    .await
}

/// Captures a child and sends it SIGINT on request so it can unwind its own children.
pub async fn capture_interruptibly(
    command: &mut Command,
    input: &[u8],
    limit: Duration,
    max_stdout: usize,
    on_start: impl FnOnce(Option<u32>) + Send,
    interrupt: impl std::future::Future<Output = ()> + Send,
) -> Result<Captured, Ended> {
    let stdin = if input.is_empty() {
        Stdio::null()
    } else {
        Stdio::piped()
    };
    command
        .stdin(stdin)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command.spawn().map_err(Ended::Spawn)?;
    let pid = child.id();
    on_start(pid);
    let (stdout, stderr) = (child.stdout.take(), child.stderr.take());
    let writing = child.stdin.take();
    let work = async {
        let (collected, ()) = tokio::join!(
            collect(child, stdout, stderr, max_stdout, pid, interrupt),
            write_input(writing, input)
        );
        collected
    };
    tokio::time::timeout(limit, work).await.unwrap_or_else(|_| {
        kill_group(pid);
        Err(Ended::TimedOut)
    })
}

/// Writes `input` to a child's stdin and closes it. A child that stops reading early (it answered from
/// what it had) is not an error here: its own end says how it went.
async fn write_input(stdin: Option<tokio::process::ChildStdin>, input: &[u8]) {
    use tokio::io::AsyncWriteExt;
    let Some(mut stdin) = stdin else { return };
    stdin.write_all(input).await.ok();
    stdin.shutdown().await.ok();
}

#[cfg(all(feature = "boxes", unix))]
/// Asks a process started with its own process group to end (TERM): the program then ends what it runs itself.
pub fn ask_to_end(pid: Option<u32>) {
    let Some(process) = pid
        .and_then(|pid| i32::try_from(pid).ok())
        .filter(|pid| *pid > 1)
        .and_then(rustix::process::Pid::from_raw)
    else {
        return;
    };
    let _ = rustix::process::kill_process(process, rustix::process::Signal::TERM);
}

#[cfg(all(feature = "boxes", not(unix)))]
/// Ends the WSL launcher and its children on Windows, where POSIX signals are unavailable.
pub fn ask_to_end(pid: Option<u32>) {
    kill_group(pid);
}

/// Kills a process group started with its own process group, or on Windows the process and its tree.
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

async fn collect(
    mut child: tokio::process::Child,
    stdout: Option<impl AsyncRead + Unpin>,
    stderr: Option<impl AsyncRead + Unpin>,
    max_stdout: usize,
    pid: Option<u32>,
    interrupt: impl std::future::Future<Output = ()>,
) -> Result<Captured, Ended> {
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let (status, exceeded) = {
        let readers = async {
            let (is_over, ()) = tokio::join!(
                read_capped(stdout, &mut out, max_stdout, pid),
                read_tail(stderr, &mut err)
            );
            is_over
        };
        wait_and_drain(&mut child, readers, interrupt).await
    };
    if exceeded {
        return Err(Ended::TooMuchOutput);
    }
    let status = status.map_err(Ended::Wait)?;
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
    interrupt: impl std::future::Future<Output = ()>,
) -> (std::io::Result<ExitStatus>, bool) {
    let wait = wait_interruptibly(child, interrupt);
    tokio::pin!(readers, wait);
    tokio::select! {
        is_over = &mut readers => (wait.await, is_over),
        status = &mut wait => {
            let drained = tokio::time::timeout(DRAIN_GRACE, &mut readers).await;
            (status, drained.unwrap_or(false))
        }
    }
}

async fn wait_interruptibly(
    child: &mut tokio::process::Child,
    interrupt: impl std::future::Future<Output = ()>,
) -> std::io::Result<ExitStatus> {
    tokio::select! {
        status = child.wait() => status,
        () = interrupt => {
            if let Some(pid) = child.id()
                .and_then(|id| i32::try_from(id).ok())
                .and_then(rustix::process::Pid::from_raw)
            {
                let _ = rustix::process::kill_process(pid, rustix::process::Signal::INT);
            }
            child.wait().await
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
