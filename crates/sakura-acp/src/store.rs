//! Which session belongs to which task: one small JSON file in Sakura's own state directory, so reopening a
//! task continues its conversation. A session lives in the account's own folder inside the box, so the key
//! is the task and the account together. Nothing here is secret: a session id names a transcript, it does
//! not open one. The file also keeps, per session, the words of the refusals said in it: an agent replays
//! a refusal as a plain message, and only what was kept when it was said tells it apart.

use serde_json::{json, Map, Value};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};

/// Held for the whole of a change (read the file, change the map, write it back): two changes at once
/// would each write the map they read and one would be lost. One lock for every store of the process;
/// changes are rare and small.
static CHANGING: Mutex<()> = Mutex::new(());
/// Numbers the temporary files, so no two writes share one.
static WRITES: AtomicU64 = AtomicU64::new(0);

const FILE_NAME: &str = "acp-sessions.json";
const SESSIONS_KEY: &str = "sessions";
const REFUSALS_KEY: &str = "refusals";
/// A refusal's words longer than this are not kept: they come from the agent and are untrusted.
const MAX_REFUSAL_BYTES: usize = 4096;
/// The refusals kept for one session: the oldest leaves when one more is said.
const MAX_REFUSALS: usize = 16;
/// A session id longer than this is not stored: it comes from the agent and is untrusted.
const MAX_SESSION_ID_BYTES: usize = 512;

/// The stored session ids.
#[derive(Debug, Clone)]
pub struct SessionStore {
    path: PathBuf,
}

impl SessionStore {
    /// The store in `state_dir` (the folder that holds the app's preferences).
    pub fn new(state_dir: &Path) -> Self {
        Self {
            path: state_dir.join(FILE_NAME),
        }
    }

    /// The session last opened for `task` with `account`; none when nothing was stored or the file cannot
    /// be read.
    pub fn get(&self, task: &str, account: &str) -> Option<String> {
        let sessions = self.read();
        let id = sessions.get(task)?.get(account)?.as_str()?;
        Some(id.to_string())
    }

    /// Every session stored, as (task, account, session): read once when the app starts, so that asking
    /// for one later reads no file.
    pub fn all(&self) -> Vec<(String, String, String)> {
        let sessions = self.read();
        let of_task = |(task, accounts): (&String, &Value)| {
            let accounts = accounts.as_object().into_iter().flatten();
            let stored = accounts.filter_map(|(account, id)| Some((account, id.as_str()?)));
            let named = stored.map(|(account, id)| (task.clone(), account.clone(), id.to_string()));
            named.collect::<Vec<_>>()
        };
        sessions.iter().flat_map(of_task).collect()
    }

    /// Remembers `session_id` for `task` with `account`.
    pub fn set(&self, task: &str, account: &str, session_id: &str) -> io::Result<()> {
        if session_id.is_empty() || session_id.len() > MAX_SESSION_ID_BYTES {
            let message = "a session id must have between 1 and 512 bytes";
            return Err(io::Error::new(io::ErrorKind::InvalidInput, message));
        }
        let _changing = changing();
        let mut sessions = self.read();
        let accounts = sessions.entry(task).or_insert_with(|| json!({}));
        if let Some(accounts) = accounts.as_object_mut() {
            accounts.insert(account.to_string(), json!(session_id));
        }
        self.write(sessions)
    }

    /// Forgets every session of `task` (the task was removed), with the refusals kept for them.
    pub fn forget(&self, task: &str) -> io::Result<()> {
        let _changing = changing();
        let mut sessions = self.read();
        let Some(accounts) = sessions.remove(task) else {
            return Ok(());
        };
        self.write(sessions)?;
        let mut refusals = self.part(REFUSALS_KEY);
        let of_task = accounts.as_object().into_iter().flatten();
        let forgotten = of_task
            .filter_map(|(_, session)| refusals.remove(session.as_str()?))
            .count();
        match forgotten {
            0 => Ok(()),
            _ => self.write_part(REFUSALS_KEY, refusals),
        }
    }

    /// Every refusal kept, as (session, the words of each of its refusals): read once when the app
    /// starts, as the sessions are.
    pub fn refusals(&self) -> Vec<(String, Vec<String>)> {
        let words = |said: &Value| -> Vec<String> {
            let said = said.as_array().into_iter().flatten();
            said.filter_map(Value::as_str).map(str::to_string).collect()
        };
        let kept = self.part(REFUSALS_KEY);
        let of_session = |(session, said): (&String, &Value)| (session.clone(), words(said));
        kept.iter().map(of_session).collect()
    }

    /// Keeps `words` as a refusal said in `session_id`. The same words are kept once.
    pub fn add_refusal(&self, session_id: &str, words: &str) -> io::Result<()> {
        if words.is_empty() || words.len() > MAX_REFUSAL_BYTES {
            let message = "the words of a refusal must have between 1 and 4096 bytes";
            return Err(io::Error::new(io::ErrorKind::InvalidInput, message));
        }
        let _changing = changing();
        let mut kept = self.part(REFUSALS_KEY);
        let said = kept.entry(session_id).or_insert_with(|| json!([]));
        if let Some(said) = said.as_array_mut() {
            if !said.iter().any(|kept| kept.as_str() == Some(words)) {
                said.push(json!(words));
            }
            let extra = said.len().saturating_sub(MAX_REFUSALS);
            said.drain(..extra);
        }
        self.write_part(REFUSALS_KEY, kept)
    }

    /// The stored sessions; empty when the file is missing or unreadable.
    fn read(&self) -> Map<String, Value> {
        self.part(SESSIONS_KEY)
    }

    /// One part of the file (the sessions, the refusals); empty when it is missing or unreadable.
    fn part(&self, key: &str) -> Map<String, Value> {
        match self.whole().remove(key) {
            Some(Value::Object(part)) => part,
            _ => Map::new(),
        }
    }

    /// The whole file; empty when it is missing or unreadable.
    fn whole(&self) -> Map<String, Value> {
        let stored = std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok());
        match stored {
            Some(Value::Object(whole)) => whole,
            _ => Map::new(),
        }
    }

    fn write(&self, sessions: Map<String, Value>) -> io::Result<()> {
        self.write_part(SESSIONS_KEY, sessions)
    }

    /// Writes the file with one part replaced, to a temporary file of its own beside the store, and
    /// renames it over, so a crash in the middle leaves the old file whole and no other write, of this
    /// process or another, finds its file renamed away.
    fn write_part(&self, key: &str, part: Map<String, Value>) -> io::Result<()> {
        let mut whole = self.whole();
        whole.insert(key.to_string(), Value::Object(part));
        if let Some(folder) = self.path.parent() {
            std::fs::create_dir_all(folder)?;
        }
        let number = WRITES.fetch_add(1, Ordering::Relaxed);
        let name = format!("json.{}.{number}.tmp", std::process::id());
        let temporary = self.path.with_extension(name);
        let text = Value::Object(whole).to_string();
        let written =
            write_new(&temporary, &text).and_then(|()| std::fs::rename(&temporary, &self.path));
        if written.is_err() {
            std::fs::remove_file(&temporary).ok();
        }
        written
    }
}

/// The lock of a change, also after a panic elsewhere poisoned it: it guards a file, not memory.
fn changing() -> MutexGuard<'static, ()> {
    CHANGING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Writes `text` to `path`, which must not exist yet.
fn write_new(path: &Path, text: &str) -> io::Result<()> {
    use std::io::Write as _;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, SessionStore) {
        let folder = tempfile::tempdir().expect("a temporary folder");
        let store = SessionStore::new(&folder.path().join("state"));
        (folder, store)
    }

    #[test]
    fn get_before_anything_is_stored_is_none() {
        let (_folder, store) = store();
        assert_eq!(store.get("task", "personal"), None);
    }

    #[test]
    fn set_then_get_returns_the_session_per_task_and_account() {
        let (_folder, store) = store();
        store.set("folders", "personal", "sess-1").unwrap();
        store.set("folders", "work", "sess-2").unwrap();
        store.set("other", "personal", "sess-3").unwrap();
        assert_eq!(store.get("folders", "personal").as_deref(), Some("sess-1"));
        assert_eq!(store.get("folders", "work").as_deref(), Some("sess-2"));
        assert_eq!(store.get("other", "personal").as_deref(), Some("sess-3"));
        assert_eq!(store.get("other", "work"), None);
    }

    #[test]
    fn all_lists_every_session_with_its_task_and_account() {
        let (_folder, store) = store();
        assert!(store.all().is_empty());
        store.set("m1", "personal", "a").unwrap();
        store.set("m1", "", "b").unwrap();
        store.set("m2", "personal", "c").unwrap();
        let mut all = store.all();
        all.sort();
        let listed = |task: &str, account: &str, id: &str| {
            (task.to_string(), account.to_string(), id.to_string())
        };
        assert_eq!(
            all,
            [
                listed("m1", "", "b"),
                listed("m1", "personal", "a"),
                listed("m2", "personal", "c"),
            ]
        );
    }

    #[test]
    fn a_refusal_is_kept_with_its_session_beside_the_sessions_and_leaves_with_its_task() {
        let (folder, store) = store();
        store.set("folders", "personal", "s1").unwrap();
        store.add_refusal("s1", "API Error: no").unwrap();
        store.add_refusal("s1", "API Error: no").unwrap();
        store.add_refusal("s2", "API Error: never").unwrap();
        store.set("headings", "personal", "s2").unwrap();
        let reopened = SessionStore::new(&folder.path().join("state"));
        assert_eq!(reopened.get("folders", "personal").as_deref(), Some("s1"));
        let mut kept = reopened.refusals();
        kept.sort();
        let expected = [
            ("s1".to_string(), vec!["API Error: no".to_string()]),
            ("s2".to_string(), vec!["API Error: never".to_string()]),
        ];
        assert_eq!(kept, expected, "the same words once, each session its own");
        store.forget("folders").unwrap();
        assert_eq!(store.refusals(), [expected[1].clone()]);
        assert_eq!(store.get("headings", "personal").as_deref(), Some("s2"));
        assert!(store.add_refusal("s2", "").is_err());
        assert!(store.add_refusal("s2", &"x".repeat(4097)).is_err());
    }

    #[test]
    fn set_again_replaces_the_session_and_survives_a_new_store_on_the_same_folder() {
        let (folder, store) = store();
        store.set("folders", "personal", "old").unwrap();
        store.set("folders", "personal", "new").unwrap();
        let reopened = SessionStore::new(&folder.path().join("state"));
        assert_eq!(reopened.get("folders", "personal").as_deref(), Some("new"));
    }

    #[test]
    fn forget_removes_every_account_of_the_task_only() {
        let (_folder, store) = store();
        store.set("folders", "personal", "a").unwrap();
        store.set("folders", "work", "b").unwrap();
        store.set("other", "personal", "c").unwrap();
        store.forget("folders").unwrap();
        store.forget("mai-esistito").unwrap();
        assert_eq!(store.get("folders", "personal"), None);
        assert_eq!(store.get("folders", "work"), None);
        assert_eq!(store.get("other", "personal").as_deref(), Some("c"));
    }

    #[test]
    fn unreadable_file_reads_as_empty_and_is_replaced_by_the_next_set() {
        let (_folder, store) = store();
        std::fs::create_dir_all(store.path.parent().unwrap()).unwrap();
        std::fs::write(&store.path, "not json {").unwrap();
        assert_eq!(store.get("folders", "personal"), None);
        store.set("folders", "personal", "a").unwrap();
        assert_eq!(store.get("folders", "personal").as_deref(), Some("a"));
    }

    #[test]
    fn set_refuses_an_empty_or_oversized_session_id() {
        let (_folder, store) = store();
        assert!(store.set("folders", "personal", "").is_err());
        let long = "x".repeat(MAX_SESSION_ID_BYTES + 1);
        assert!(store.set("folders", "personal", &long).is_err());
        assert_eq!(store.get("folders", "personal"), None);
    }

    #[test]
    fn names_with_quotes_and_newlines_are_stored_as_data() {
        let (_folder, store) = store();
        store.set("a\"b\n", "acc\\ount", "s\"1").unwrap();
        assert_eq!(store.get("a\"b\n", "acc\\ount").as_deref(), Some("s\"1"));
    }

    #[test]
    fn sets_from_many_threads_at_once_all_succeed_and_none_is_lost() {
        const THREADS: usize = 12;
        const SETS: usize = 30;
        let (_folder, store) = store();
        let writers: Vec<_> = (0..THREADS)
            .map(|thread| {
                let store = store.clone();
                std::thread::spawn(move || {
                    (0..SETS)
                        .map(|set| store.set(&format!("lavoro-{thread}-{set}"), "personal", "s"))
                        .filter(|result| result.is_err())
                        .count()
                })
            })
            .collect();
        let failed: usize = writers
            .into_iter()
            .map(|writer| writer.join().unwrap())
            .sum();
        assert_eq!(failed, 0, "a write failed");
        let kept = (0..THREADS)
            .flat_map(|thread| (0..SETS).map(move |set| format!("lavoro-{thread}-{set}")))
            .filter(|task| store.get(task, "personal").is_some())
            .count();
        assert_eq!(kept, THREADS * SETS, "a session that was set is gone");
    }

    #[test]
    fn forgets_and_sets_at_once_keep_every_other_task() {
        let (_folder, store) = store();
        for task in 0..40 {
            store.set(&format!("old-{task}"), "personal", "s").unwrap();
        }
        let forgetter = {
            let store = store.clone();
            std::thread::spawn(move || {
                for task in 0..40 {
                    store.forget(&format!("old-{task}")).unwrap();
                }
            })
        };
        for task in 0..40 {
            store.set(&format!("new-{task}"), "personal", "s").unwrap();
        }
        forgetter.join().unwrap();
        let has = |task: String| store.get(&task, "personal").is_some();
        assert!(
            (0..40).all(|task| has(format!("new-{task}"))),
            "a set was lost"
        );
        assert!(
            !(0..40).any(|task| has(format!("old-{task}"))),
            "a forgotten task came back"
        );
        let folder = store.path.parent().unwrap();
        let left: Vec<_> = std::fs::read_dir(folder).unwrap().flatten().collect();
        assert_eq!(left.len(), 1, "no temporary file is left: {left:?}");
    }
}
