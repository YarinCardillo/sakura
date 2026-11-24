//! The master's earlier conversations: when the person starts fresh, the conversation they leave is kept
//! here by its session, so History can bring it back. The agent keeps the conversation itself; this keeps
//! only which sessions there are and what each began with. One small file, read when the app starts.

use super::latest_write::LatestWrite;
use gpui::BackgroundExecutor;
use serde_json::{json, Value};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// The file the history is kept in, inside the master's folder.
const FILE: &str = "history.json";
/// How many earlier conversations are kept; the oldest go first.
const MAX_KEPT: usize = 30;
/// How many characters of a conversation's first message name it.
const TITLE_CHARS: usize = 60;

/// One earlier conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Past {
    /// The agent's session that holds it.
    pub session: String,
    /// What it began with.
    pub title: String,
    /// The Leeway account it ran on; empty for the host's default.
    pub account: String,
}

impl Past {
    /// An earlier conversation named by its first message: its first line, cut to a short length.
    pub fn new(session: &str, first_message: &str) -> Self {
        let line = first_message.lines().next().unwrap_or_default().trim();
        let title: String = line.chars().take(TITLE_CHARS).collect();
        Self {
            session: session.to_string(),
            title,
            account: String::new(),
        }
    }

    /// The same, said to have run on the account `account`.
    pub fn on_account(mut self, account: &str) -> Self {
        self.account = account.to_string();
        self
    }
}

/// The earlier conversations, newest first. Cheap to clone; clones share what they know.
#[derive(Clone, Default)]
pub struct MasterHistory {
    file: Option<HistoryFile>,
    kept: Rc<RefCell<Vec<Past>>>,
}

#[derive(Clone)]
struct HistoryFile {
    path: PathBuf,
    executor: BackgroundExecutor,
    writer: LatestWrite<(PathBuf, Vec<Past>)>,
}

impl MasterHistory {
    /// The history kept in `folder`, or in memory alone without one. A file that cannot be read is an empty
    /// history: it is said on stderr and written again with the next conversation left.
    pub fn new(folder: Option<&Path>, executor: &BackgroundExecutor) -> Self {
        let file = folder.map(|folder| HistoryFile {
            path: folder.join(FILE),
            executor: executor.clone(),
            writer: LatestWrite::default(),
        });
        let kept = file
            .as_ref()
            .map(|file| read(&file.path))
            .unwrap_or_default();
        Self {
            file,
            kept: Rc::new(RefCell::new(kept)),
        }
    }

    /// The earlier conversations, newest first.
    pub fn list(&self) -> Vec<Past> {
        self.kept.borrow().clone()
    }

    /// Keeps `past` as the newest; one with the same account and session is replaced.
    pub fn add(&self, past: Past) {
        let mut kept = self.kept.borrow_mut();
        kept.retain(|other| other.session != past.session || other.account != past.account);
        kept.insert(0, past);
        kept.truncate(MAX_KEPT);
        self.write(&kept);
    }

    /// Takes the conversation of `session` out: it is the open one again.
    pub fn take_for(&self, account: &str, session: &str) -> Option<Past> {
        let mut kept = self.kept.borrow_mut();
        let at = kept
            .iter()
            .position(|past| past.session == session && past.account == account)?;
        let past = kept.remove(at);
        self.write(&kept);
        Some(past)
    }

    fn write(&self, kept: &[Past]) {
        if let Some(file) = &self.file {
            file.writer
                .submit((file.path.clone(), kept.to_vec()), write, &file.executor);
        }
    }
}

fn write((file, kept): (PathBuf, Vec<Past>)) -> bool {
    let written = file
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(file, entries_json(&kept)));
    if let Err(error) = written {
        eprintln!("sakura: the master's history was not kept: {error}");
        return false;
    }
    true
}

fn entries_json(kept: &[Past]) -> String {
    let entry = |past: &Past| json!({ "session": past.session, "title": past.title, "account": past.account });
    Value::Array(kept.iter().map(entry).collect()).to_string()
}

fn read(file: &Path) -> Vec<Past> {
    let bytes = match std::fs::read(file) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(error) => {
            eprintln!("sakura: the master's history was not read: {error}");
            return Vec::new();
        }
    };
    let entries: Value = serde_json::from_slice(&bytes).unwrap_or_else(|error| {
        eprintln!("sakura: the master's history is not readable: {error}");
        Value::Null
    });
    let text = |entry: &Value, field: &str| entry[field].as_str().map(str::to_string);
    let past = |entry: &Value| {
        Some(Past {
            session: text(entry, "session")?,
            title: text(entry, "title")?,
            account: text(entry, "account").unwrap_or_default(),
        })
    };
    entries
        .as_array()
        .map_or_else(Vec::new, |all| all.iter().filter_map(past).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_keeps_the_newest_first_and_a_session_only_once() {
        let history = MasterHistory::default();
        history.add(Past::new("s1", "First question\nmore"));
        history.add(Past::new("s2", "Second"));
        history.add(Past::new("s1", "First question again"));
        let titles: Vec<String> = history.list().into_iter().map(|past| past.title).collect();
        assert_eq!(titles, ["First question again", "Second"]);
    }

    #[test]
    fn history_names_a_conversation_by_its_first_line_cut_short() {
        let long = "x".repeat(200);
        assert_eq!(Past::new("s", &long).title.chars().count(), TITLE_CHARS);
        assert_eq!(Past::new("s", "Line one\nLine two").title, "Line one");
    }

    #[gpui::test]
    fn history_survives_a_launch_and_taking_one_out_removes_it_from_the_file(
        cx: &mut gpui::TestAppContext,
    ) {
        let folder = tempfile::tempdir().expect("a folder");
        let history = MasterHistory::new(Some(&folder.path().join("master")), &cx.executor());
        history.add(Past::new("s1", "One"));
        history.add(Past::new("s2", "Two"));
        cx.run_until_parked();
        let next = MasterHistory::new(Some(&folder.path().join("master")), &cx.executor());
        assert_eq!(next.list().len(), 2);
        assert_eq!(
            next.take_for("", "s1").map(|past| past.title),
            Some("One".into())
        );
        assert_eq!(next.take_for("", "s1"), None);
        cx.run_until_parked();
        let again = MasterHistory::new(Some(&folder.path().join("master")), &cx.executor());
        assert_eq!(again.list(), vec![Past::new("s2", "Two")]);
    }

    #[test]
    fn history_beyond_its_room_forgets_the_oldest() {
        let history = MasterHistory::default();
        for index in 0..MAX_KEPT + 5 {
            history.add(Past::new(&format!("s{index}"), "x"));
        }
        let kept = history.list();
        assert_eq!(kept.len(), MAX_KEPT);
        assert_eq!(kept[0].session, format!("s{}", MAX_KEPT + 4));
    }

    #[gpui::test]
    fn history_file_that_is_not_json_is_an_empty_history(cx: &mut gpui::TestAppContext) {
        let folder = tempfile::tempdir().expect("a folder");
        std::fs::write(folder.path().join(FILE), "not json").expect("the file");
        assert!(MasterHistory::new(Some(folder.path()), &cx.executor())
            .list()
            .is_empty());
    }
}

#[cfg(test)]
#[test]
fn track_history_keeps_colliding_session_tokens_on_each_account() {
    let history = MasterHistory::default();
    history.add(Past::new("same-token", "First").on_account("work"));
    history.add(Past::new("same-token", "Second").on_account("personal"));

    assert_eq!(history.list().len(), 2);
    assert_eq!(
        history.take_for("work", "same-token").unwrap().title,
        "First"
    );
    assert_eq!(
        history.take_for("personal", "same-token").unwrap().title,
        "Second"
    );
}
