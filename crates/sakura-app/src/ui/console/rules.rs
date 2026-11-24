//! What a console's end means, as plain rules: whether `box` refused to start it, whether it closes normally,
//! and when an agent's console has outlived its agent.

use super::login::{login_failure, LoginFailure};
use sakura_core::box_words;
use sakura_core::models::{AgentPhase, Task, TerminalKind};
use std::time::Instant;

/// Whether `task` has an agent container that runs: an agent started in a terminal (`box agent`).
pub fn is_agent_running(task: &Task) -> bool {
    task.agent
        .as_ref()
        .is_some_and(|agent| agent.state == AgentPhase::Running)
}

/// What a console in a task's pane runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsoleKind {
    /// `box attach <task>`: the task's agent.
    Agent,
    /// `box shell <task>`: a shell in a one-off container of the task.
    Shell,
    /// `box login <account>`: the account's login, shown in the pane where it was requested.
    Login(String),
    /// `box up`: the boxes made ready on this machine, shown where a sign-in would be, before what
    /// needs a box goes on.
    BoxUp,
}

/// Which of a task's two places a console takes: each task has one console of the agent (its
/// terminal, or the login requested there) and, beside it, one terminal of its own. Opening one never
/// touches the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Place {
    Agent,
    Terminal,
}

impl ConsoleKind {
    /// The place it takes among the task's consoles.
    pub fn place(&self) -> Place {
        match self {
            ConsoleKind::Agent | ConsoleKind::Login(_) | ConsoleKind::BoxUp => Place::Agent,
            ConsoleKind::Shell => Place::Terminal,
        }
    }

    /// The `sakura-core` terminal kind that runs it.
    pub fn terminal_kind(&self) -> TerminalKind {
        match self {
            ConsoleKind::Agent => TerminalKind::Agent,
            ConsoleKind::Shell => TerminalKind::Shell,
            ConsoleKind::Login(_) => TerminalKind::Login,
            ConsoleKind::BoxUp => TerminalKind::Up,
        }
    }

    /// The account of a login.
    pub fn account(&self) -> Option<&str> {
        match self {
            ConsoleKind::Login(account) => Some(account),
            ConsoleKind::Agent | ConsoleKind::Shell | ConsoleKind::BoxUp => None,
        }
    }
}

/// What happens to a console once its process exited.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// It is put away and the task says its terminal ended: a shell that ended, with whatever code.
    Close(Option<i32>),
    /// A login that ended well: the account is read again before anything is said.
    LoginCheck,
    /// A login that failed, and why.
    LoginFailed(LoginFailure),
    /// It stays on screen with what it printed, and says it ended.
    Ended(Option<i32>),
    /// `box` refused: the line it wrote replaces the terminal, said in Sakura's words with the line itself
    /// as the details.
    Failed(String),
    /// `box up` ended well: the boxes can be used, and what waited for them goes on.
    BoxesReady,
    /// `box up` found no Docker to talk to: one line says what to do.
    DockerDown,
}

/// The outcome of a console of `kind` whose process exited with `code` after printing `last_line` last.
pub fn outcome(kind: &ConsoleKind, code: Option<i32>, last_line: Option<&str>) -> Outcome {
    if let ConsoleKind::Login(_) = kind {
        return match code {
            Some(0) => Outcome::LoginCheck,
            _ => Outcome::LoginFailed(login_failure(code, last_line)),
        };
    }
    if *kind == ConsoleKind::BoxUp {
        return boxes_made(code, last_line);
    }
    let refusal = last_line
        .filter(|_| code != Some(0))
        .filter(|line| box_words::reason(line).is_some());
    if let Some(line) = refusal {
        return Outcome::Failed(line.to_string());
    }
    match kind {
        ConsoleKind::Shell => Outcome::Close(code),
        ConsoleKind::Agent | ConsoleKind::Login(_) | ConsoleKind::BoxUp => Outcome::Ended(code),
    }
}

/// The end of `box up`: ready, Docker not there, or a failure whose last lines stay on screen.
fn boxes_made(code: Option<i32>, last_line: Option<&str>) -> Outcome {
    let reason = last_line.and_then(box_words::reason);
    let known = reason.and_then(box_words::known);
    match (code, known) {
        (Some(0), _) => Outcome::BoxesReady,
        (_, Some(box_words::Known::DockerUnreachable)) => Outcome::DockerDown,
        _ => Outcome::Ended(code),
    }
}

/// Whether an agent console that ended at `ended_at` can go: facts read after its end say no agent runs.
/// A console still running, or one ended after the last read, stays.
pub fn is_agent_gone(
    kind: &ConsoleKind,
    ended_at: Option<Instant>,
    fetched_at: Instant,
    is_running: bool,
) -> bool {
    *kind == ConsoleKind::Agent && !is_running && ended_at.is_some_and(|at| fetched_at > at)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::l10n::Key;
    use std::time::Duration;

    #[test]
    fn a_refusal_from_box_fails_with_its_sentence() {
        let line = Some("box: no agent is running for task 'm2'");
        assert_eq!(
            outcome(&ConsoleKind::Agent, Some(3), line),
            Outcome::Failed("box: no agent is running for task 'm2'".into())
        );
    }

    #[test]
    fn a_login_that_ends_well_is_checked_and_one_that_fails_says_why() {
        let login = ConsoleKind::Login("personal".into());
        assert_eq!(outcome(&login, Some(0), None), Outcome::LoginCheck);
        assert_eq!(
            outcome(&login, Some(1), Some("box: docker is not running")),
            Outcome::LoginFailed(LoginFailure::Other(Key("login.failed").into()))
        );
        assert_eq!(
            outcome(&login, Some(130), None),
            Outcome::LoginFailed(LoginFailure::Cancelled)
        );
    }

    #[test]
    fn boxes_made_ready_go_on_and_a_failure_keeps_its_last_lines() {
        let up = ConsoleKind::BoxUp;
        assert_eq!(outcome(&up, Some(0), None), Outcome::BoxesReady);
        assert_eq!(
            outcome(
                &up,
                Some(1),
                Some("ERROR: failed to solve: network unreachable")
            ),
            Outcome::Ended(Some(1)),
            "what it printed stays on screen"
        );
        let docker = "box: Docker is not reachable. Start Docker Desktop and retry.";
        assert_eq!(outcome(&up, Some(1), Some(docker)), Outcome::DockerDown);
        assert_eq!(up.place(), Place::Agent, "where the sign-in shows");
        assert_eq!(up.terminal_kind(), TerminalKind::Up);
    }

    #[test]
    fn a_program_line_that_is_not_from_box_is_an_end_not_a_refusal() {
        let line = Some("Error: something in the agent");
        assert_eq!(
            outcome(&ConsoleKind::Agent, Some(1), line),
            Outcome::Ended(Some(1))
        );
        assert_eq!(
            outcome(&ConsoleKind::Agent, Some(0), Some("box: a warning")),
            Outcome::Ended(Some(0))
        );
    }

    #[test]
    fn a_shell_that_ends_is_put_away_whatever_its_code() {
        assert_eq!(
            outcome(&ConsoleKind::Shell, Some(0), Some("exit")),
            Outcome::Close(Some(0))
        );
        assert_eq!(
            outcome(&ConsoleKind::Shell, Some(130), None),
            Outcome::Close(Some(130))
        );
        assert_eq!(
            outcome(&ConsoleKind::Shell, None, None),
            Outcome::Close(None)
        );
    }

    #[test]
    fn a_shell_has_its_own_place_beside_the_agent_and_a_login_takes_the_agent_s() {
        assert_eq!(ConsoleKind::Shell.place(), Place::Terminal);
        assert_eq!(ConsoleKind::Agent.place(), Place::Agent);
        assert_eq!(ConsoleKind::Login("personal".into()).place(), Place::Agent);
    }

    #[test]
    fn an_agent_console_goes_only_when_facts_after_its_end_say_no_agent_runs() {
        let ended = Instant::now();
        let later = ended + Duration::from_secs(1);
        let earlier = ended - Duration::from_secs(1);
        let agent = ConsoleKind::Agent;
        assert!(is_agent_gone(&agent, Some(ended), later, false));
        assert!(!is_agent_gone(&agent, Some(ended), later, true));
        assert!(!is_agent_gone(&agent, Some(ended), earlier, false));
        assert!(!is_agent_gone(&agent, None, later, false));
        assert!(!is_agent_gone(
            &ConsoleKind::Shell,
            Some(ended),
            later,
            false
        ));
    }
}
