//! The agent's process: started from an argument vector in its own process group, its stdio as the
//! connection's lines, the end of its stderr kept for the sentence that says why it died, and stopped,
//! with whatever it left in its process group, only by the id of the child this module started.

use crate::thread::{Problem, ProblemKind};
use crate::transport;
use agent_client_protocol::Lines;
use futures::{Sink, Stream, StreamExt};
use sakura_core::box_cli::{kill_group, sanitize_line};
use std::io;
use std::pin::Pin;
use std::process::{ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::BufReader;
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};

/// The longest line of stderr read; a longer one ends the reading.
const MAX_STDERR_LINE_BYTES: usize = 64 * 1024;

/// The transport handed to the SDK.
pub(super) type AgentLines = Lines<
    Pin<Box<dyn Sink<String, Error = io::Error> + Send>>,
    Pin<Box<dyn Stream<Item = io::Result<String>> + Send>>,
>;

/// A running agent.
pub(super) struct Process {
    child: Child,
    /// The process group the child leads, kept from its start: the child's own id is gone once it has
    /// been waited for, and what it started may still run.
    group: Option<u32>,
    pipes: Option<(ChildStdin, ChildStdout)>,
    /// The last line the agent wrote to stderr.
    last_error_line: Arc<Mutex<Option<String>>>,
}

impl Process {
    /// Starts `command` (program, then arguments) with `env` added to the environment.
    pub(super) fn start(command: &[String], env: &[(String, String)]) -> io::Result<Self> {
        let (program, arguments) = command.split_first().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "the agent's command is empty")
        })?;
        let mut command = Command::new(program);
        command
            .args(arguments)
            .envs(env.iter().map(|(name, value)| (name, value)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command.spawn()?;
        let group = child.id();
        let pipes = child.stdin.take().zip(child.stdout.take());
        let last_error_line = Arc::new(Mutex::new(None));
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(keep_last_line(stderr, last_error_line.clone()));
        }
        Ok(Self {
            child,
            group,
            pipes,
            last_error_line,
        })
    }

    /// The process's stdin and stdout as the connection's lines. Dropping them closes the agent's input.
    /// What the connection has not taken in yet is counted in `unread`.
    pub(super) fn take_lines(&mut self, unread: Arc<transport::Unread>) -> AgentLines {
        let (stdin, stdout) = self.pipes.take().expect("the pipes are taken once");
        let incoming = transport::incoming_counted(
            BufReader::new(stdout),
            transport::MAX_LINE_BYTES,
            unread,
            reaches_a_handler,
        );
        Lines::new(Box::pin(transport::outgoing(stdin)), Box::pin(incoming))
    }

    /// A clean stop: the agent's input is already closed, so it gets `grace` to exit by itself and is then
    /// killed with its process group.
    pub(super) async fn stop(mut self, grace: Duration) {
        self.exit_status(grace).await;
    }

    /// The agent ended by itself (or its connection broke with `error`): why, as a problem.
    pub(super) async fn died(mut self, grace: Duration, error: Option<String>) -> Problem {
        let status = self.exit_status(grace).await;
        let last_line = self
            .last_error_line
            .lock()
            .ok()
            .and_then(|line| line.clone());
        let detail = [status.map(describe), error, last_line];
        Problem {
            kind: ProblemKind::Died,
            detail: detail.into_iter().flatten().collect::<Vec<_>>().join(": "),
        }
    }

    /// Resolves when the process has exited, whether or not its output has reached its end: something
    /// it started may hold that output open.
    pub(super) async fn exited(&mut self) {
        self.child.wait().await.ok();
    }

    /// Waits up to `grace` for the process to exit, then kills its group and waits for it. The group is
    /// killed also when the process exited by itself: a wrapper may leave a worker behind.
    async fn exit_status(&mut self, grace: Duration) -> Option<ExitStatus> {
        let exited = tokio::time::timeout(grace, self.child.wait()).await;
        kill_group(self.group);
        match exited {
            Ok(status) => status.ok(),
            Err(_) => self.child.wait().await.ok(),
        }
    }
}

/// The methods the connection has a handler for: the handler says when it has dealt with the message.
const HANDLED_METHODS: [&str; 2] = ["session/update", "session/request_permission"];

/// The one field of a message that tells whether a handler will deal with it.
#[derive(serde::Deserialize)]
struct Head<'a> {
    #[serde(borrow)]
    method: Option<std::borrow::Cow<'a, str>>,
}

/// Whether `line` is a message one of the connection's handlers will deal with.
fn reaches_a_handler(line: &str) -> bool {
    let head = serde_json::from_str::<Head>(line).ok();
    let method = head.and_then(|head| head.method);
    method.is_some_and(|method| HANDLED_METHODS.contains(&method.as_ref()))
}

/// An exit status as a process reports it, shown under the conversation's sentence beside what the
/// process wrote last: the same in every language of the interface, like that line.
fn describe(status: ExitStatus) -> String {
    match (status.code(), ending_signal(status)) {
        (Some(code), _) => format!("exit code {code}"),
        (None, Some(signal)) => format!("signal {signal}"),
        (None, None) => "ended".to_string(),
    }
}

#[cfg(unix)]
fn ending_signal(status: ExitStatus) -> Option<i32> {
    use std::os::unix::process::ExitStatusExt;
    status.signal()
}

/// Only Unix ends a process with a signal.
#[cfg(not(unix))]
fn ending_signal(_status: ExitStatus) -> Option<i32> {
    None
}

/// Reads stderr to its end, keeping only the last non-empty line, without control characters.
async fn keep_last_line(stderr: ChildStderr, last: Arc<Mutex<Option<String>>>) {
    let lines = transport::incoming(BufReader::new(stderr), MAX_STDERR_LINE_BYTES);
    let mut lines = Box::pin(lines);
    while let Some(Ok(line)) = lines.next().await {
        let line = sanitize_line(line.trim());
        if line.is_empty() {
            continue;
        }
        if let Ok(mut last) = last.lock() {
            *last = Some(line);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_updates_and_permission_requests_reach_a_handler() {
        let update =
            r#"{"jsonrpc":"2.0","method":"session/update","params":{"x":[1,{"method":"no"}]}}"#;
        assert!(reaches_a_handler(update));
        let request =
            r#"{"jsonrpc":"2.0","id":7,"method":"session/request_permission","params":{}}"#;
        assert!(reaches_a_handler(request));
        for line in [
            r#"{"jsonrpc":"2.0","id":3,"result":{}}"#,
            r#"{"jsonrpc":"2.0","method":"_auth/status_update","params":{}}"#,
            "not json",
            "[1,2]",
        ] {
            assert!(!reaches_a_handler(line), "{line}");
        }
    }
}
