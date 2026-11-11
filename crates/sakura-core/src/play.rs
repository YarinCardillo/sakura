//! Play: the run command of a task's project, started and stopped with `box play <task>
//! start|stop|status --json`, and its output, read with `box play <task> log`. A task in a box has
//! no Play yet (`box` ends with 81): that is an answer, not a failure. Every reply is checked where
//! it is parsed; the log is kept within bounds and without the codes terminals colour text with.

use crate::box_cli::{BoxCli, Limit};
use crate::error::{BridgeError, BridgeResult, ErrorCode};
use crate::graph::parse_time;
use crate::models::Place;
use crate::said::Said;
use crate::validate;
use serde::Deserialize;
use std::collections::HashSet;
use std::iter::Peekable;
use std::str::Chars;

/// The most ports a task holds: `box`'s own limit.
pub const PORTS_MAX: usize = 100;
/// The most lines of a log that are kept: what `box play <task> log` prints.
pub const LOG_MAX_LINES: usize = 400;
/// The longest line of a log that is kept, in characters; the rest of a longer one is let go.
pub const LINE_MAX_CHARS: usize = 2000;
/// What `box` writes for a task in a box (exit 81), lowercase; the line is the error's details.
const NOT_IN_BOX_MARK: &str = "play in a box is not built yet";
/// A tab of the output, in the log.
const TAB: &str = "    ";
/// The longest colour code looked for after its escape character was taken away.
const CODE_MAX_CHARS: usize = 16;

/// Whether the command runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Run {
    Stopped,
    /// `pid` is the process group of the command; `since` when it started, in seconds since the epoch.
    Running {
        pid: u32,
        since: Option<i64>,
    },
}

/// The Play of one task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Play {
    /// The ports reserved for the task: none before its first start.
    pub ports: Vec<u16>,
    pub run: Run,
}

impl Play {
    pub fn is_running(&self) -> bool {
        matches!(self.run, Run::Running { .. })
    }

    /// The port the command is told to listen on (`SAKURA_PORT`).
    pub fn first_port(&self) -> Option<u16> {
        self.ports.first().copied()
    }
}

/// What is asked of `box play`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Start,
    Stop,
    Status,
}

impl Action {
    fn word(self) -> &'static str {
        match self {
            Action::Start => "start",
            Action::Stop => "stop",
            Action::Status => "status",
        }
    }

    fn limit(self) -> Limit {
        match self {
            Action::Start | Action::Stop => Limit::Action,
            Action::Status => Limit::Read,
        }
    }
}

/// What `box play` answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    State(Play),
    /// The task is in a box: Play is not available there yet.
    NotInBox,
}

/// Whether a task can play at all, known before `box` is asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Availability {
    Ready,
    /// The task is in a box.
    InBox,
    /// Its project names no run command.
    NoRunCommand,
}

impl Availability {
    /// From where the task lives and its project's run command.
    pub fn of(place: Place, run: Option<&str>) -> Self {
        let has_command = run.is_some_and(|command| !command.trim().is_empty());
        match place {
            Place::Box => Availability::InBox,
            Place::Mac if has_command => Availability::Ready,
            Place::Mac => Availability::NoRunCommand,
        }
    }
}

#[derive(Deserialize)]
struct Printed {
    task: String,
    running: bool,
    pid: Option<i64>,
    ports: Vec<u64>,
    since: Option<String>,
}

impl Printed {
    fn ports(&self) -> Option<Vec<u16>> {
        let port = |port: &u64| u16::try_from(*port).ok().filter(|port| *port != 0);
        let ports: Vec<u16> = self.ports.iter().map(port).collect::<Option<_>>()?;
        let unique: HashSet<u16> = ports.iter().copied().collect();
        (ports.len() <= PORTS_MAX && unique.len() == ports.len()).then_some(ports)
    }

    fn run(&self) -> Option<Run> {
        match (self.running, self.pid, &self.since) {
            (false, None, None) => Some(Run::Stopped),
            (true, Some(pid), since) => {
                let pid = u32::try_from(pid).ok().filter(|pid| *pid > 0)?;
                let since = match since {
                    Some(time) => Some(parse_time(time)?),
                    None => None,
                };
                Some(Run::Running { pid, since })
            }
            _ => None,
        }
    }
}

/// The state of `task` in a reply of `box play <task> start|stop|status --json`.
pub fn parse(task: &str, bytes: &[u8]) -> BridgeResult<Play> {
    let invalid = || BridgeError::failed(Said::new("play.invalid.state"));
    let printed: Printed = serde_json::from_slice(bytes)
        .map_err(|error| invalid().with_details(Some(error.to_string())))?;
    let state = printed.ports().zip(printed.run());
    let state = state.filter(|_| printed.task == task);
    state
        .map(|(ports, run)| Play { ports, run })
        .ok_or_else(invalid)
}

/// The address a port of a Play is opened at: on this Mac, never anywhere else.
pub fn url(port: u16) -> String {
    format!("http://localhost:{port}")
}

/// `box play <task> start|stop|status --json`.
pub async fn play(cli: &BoxCli, task: &str, action: Action) -> BridgeResult<Outcome> {
    let task = validate::task(task)?;
    let args = ["play", task, action.word(), "--json"];
    match cli.run_bytes(&args, action.limit()).await {
        Ok(bytes) => parse(task, &bytes).map(Outcome::State),
        Err(error) if is_not_in_box(&error) => Ok(Outcome::NotInBox),
        Err(error) => Err(error),
    }
}

/// `box play <task> log`: the newest lines of the output. Before the first output there is none.
pub async fn read_log(cli: &BoxCli, task: &str) -> BridgeResult<Log> {
    let task = validate::task(task)?;
    match cli.run_text(&["play", task, "log"], Limit::Read).await {
        Ok(text) => Ok(Log::of(&text)),
        Err(error) if error.code == ErrorCode::NotFound => Ok(Log::default()),
        Err(error) => Err(error),
    }
}

fn is_not_in_box(error: &BridgeError) -> bool {
    let line = error.details.as_deref().unwrap_or_default();
    error.code == ErrorCode::BoxFailed && line.to_lowercase().contains(NOT_IN_BOX_MARK)
}

/// The output of a Play as it is shown: its newest lines, plain text.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Log {
    lines: Vec<String>,
    is_cut: bool,
}

impl Log {
    /// The log in `text`: the newest [`LOG_MAX_LINES`] lines, each at most [`LINE_MAX_CHARS`]
    /// characters, without colour codes and control characters.
    pub fn of(text: &str) -> Self {
        let all: Vec<&str> = text.lines().collect();
        let skipped = all.len().saturating_sub(LOG_MAX_LINES);
        let mut is_cut = skipped > 0;
        let lines = all[skipped..].iter().map(|line| {
            let clean = clean(line);
            let kept: String = clean.chars().take(LINE_MAX_CHARS).collect();
            is_cut |= kept.len() < clean.len();
            kept
        });
        let lines = lines.collect();
        Self { lines, is_cut }
    }

    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    /// Whether older lines or the end of a long line were let go.
    pub fn is_cut(&self) -> bool {
        self.is_cut
    }

    /// The whole log as one text, for a view that selects across lines.
    pub fn text(&self) -> String {
        self.lines.join("\n")
    }
}

/// `line` without what a terminal would act on: colour codes (with their escape character, or what
/// `box` leaves of them when it takes that character away) and control characters; a tab is spaces.
fn clean(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => skip_escape(&mut chars),
            '[' if is_colour_code(chars.clone()) => skip_through(&mut chars, |c| c == 'm'),
            '\t' => out.push_str(TAB),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// Skips the rest of an escape sequence: a control sequence up to its final letter, or one character.
fn skip_escape(chars: &mut Peekable<Chars>) {
    if let Some('[') = chars.next() {
        skip_through(chars, |c| ('@'..='~').contains(&c));
    }
}

fn skip_through(chars: &mut Peekable<Chars>, is_last: impl Fn(char) -> bool) {
    for c in chars.by_ref() {
        if is_last(c) {
            break;
        }
    }
}

/// Whether what follows a `[` is a colour code: numbers separated by semicolons, then `m`.
fn is_colour_code(rest: Peekable<Chars>) -> bool {
    let code: String = rest
        .take(CODE_MAX_CHARS)
        .take_while(|c| *c != 'm')
        .collect();
    let has_end = code.chars().count() < CODE_MAX_CHARS;
    let is_numbers = !code.is_empty()
        && code
            .split(';')
            .all(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()));
    has_end && is_numbers
}

#[cfg(test)]
mod tests;
