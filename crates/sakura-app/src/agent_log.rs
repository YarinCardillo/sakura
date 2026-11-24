use sakura_acp::thread::{Problem, ProblemKind};
use std::fs::{DirBuilder, OpenOptions};
use std::io::{self, BufRead as _, BufReader, Write as _};
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

const FILE: &str = "agents.log";
const KEPT_LINES: usize = 200;
static WRITER: Mutex<()> = Mutex::new(());

pub fn record(problem: &Problem, cx: &gpui::App) {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return;
    };
    let reason = reason(problem);
    cx.background_executor()
        .spawn(async move {
            let line = format!("{} {reason}", chrono::Local::now().to_rfc3339());
            if let Err(error) = append(&crate::panic_log::log_folder(&home), &line) {
                eprintln!("sakura: could not write agents.log: {error}");
            }
        })
        .detach();
}

fn reason(problem: &Problem) -> &'static str {
    use crate::l10n::{tr, Key};

    // Details can contain arbitrary subprocess output. Only fixed messages are written to disk.
    let detail = problem.detail.as_str();
    if detail.starts_with(tr(Key("master.not_started"))) {
        return "Master spec failed";
    }
    for (key, reason) in [
        ("master.no_codex", "Codex executable was not found"),
        (
            "master.codex.login_no_home",
            "Sakura's Codex home was not found",
        ),
        (
            "master.codex.account_mismatch",
            "Master spec returned a different account",
        ),
    ] {
        if detail == tr(Key(key)) {
            return reason;
        }
    }
    request_reason(problem).unwrap_or_else(|| match problem.kind {
        ProblemKind::NotStarted if detail.contains("os error 2") => {
            "Agent executable was not found"
        }
        ProblemKind::NotStarted if detail.contains("os error 13") => {
            "Permission denied starting the agent"
        }
        ProblemKind::NotStarted => "Could not start the agent process",
        ProblemKind::Died if detail.starts_with("exit code 127") => {
            "Agent executable or interpreter was missing during startup"
        }
        ProblemKind::Died => "Agent disconnected before opening a session",
        ProblemKind::TimedOut => "Timed out opening the agent session",
        ProblemKind::Overrun => "Agent exceeded the connection buffer during startup",
        _ => "Agent refused to open a session",
    })
}

fn request_reason(problem: &Problem) -> Option<&'static str> {
    let (method, _) = problem
        .detail
        .split_once(": ")
        .unwrap_or((&problem.detail, ""));
    match method {
        "initialize" => Some("Agent handshake failed"),
        "config/read" => Some("Codex could not read its configuration"),
        "thread/start" | "thread/resume" => Some("Codex could not open the Master thread"),
        "session/new" | "session/load" => Some("ACP could not open the agent session"),
        _ => None,
    }
}

fn append(folder: &Path, line: &str) -> io::Result<()> {
    let _guard = WRITER.lock().unwrap_or_else(PoisonError::into_inner);
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(folder)?;
    let mut lines = previous_lines(&folder.join(FILE))?;
    lines.push(line.to_string());
    let mut file = tempfile::NamedTempFile::new_in(folder)?;
    for line in &lines[lines.len().saturating_sub(KEPT_LINES)..] {
        writeln!(file, "{line}")?;
    }
    file.persist(folder.join(FILE))?;
    Ok(())
}

fn previous_lines(path: &Path) -> io::Result<Vec<String>> {
    let flags = nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK;
    match OpenOptions::new().read(true).custom_flags(flags).open(path) {
        Ok(file) if file.metadata()?.is_file() => BufReader::new(file).lines().collect(),
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid agents.log",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt as _;

    #[test]
    fn start_failures_keep_only_safe_reasons_in_the_last_200_private_lines() {
        let directory = tempfile::tempdir().unwrap();
        let folder = directory.path().join("logs");
        let problem = Problem {
            kind: ProblemKind::RequestFailed,
            detail: "initialize: server echoed a secret\nanother line".into(),
        };
        for index in 0..205 {
            append(&folder, &format!("{index} {}", reason(&problem))).unwrap();
        }
        let log = folder.join(FILE);
        assert_eq!(
            fs::metadata(&log).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let contents = fs::read_to_string(log).unwrap();
        assert!(contents.starts_with("5 Agent handshake failed\n"));
        assert!(!contents.contains("secret"));
        assert_eq!(contents.lines().count(), 200);
    }
}
