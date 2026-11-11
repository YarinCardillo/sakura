//! The session of each lavoro's conversation, kept so that the conversation can be taken up again: in
//! memory while the app runs, and in the folder of the preferences across launches. The file is read
//! once, when the app starts, and written when a session opens, both off the UI thread: asking for a
//! session reads memory only, so a view may ask on every frame.

use sakura_acp::store::SessionStore;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use tokio::runtime::Runtime;

/// The stored sessions, by lavoro and account. Cheap to clone; clones share what they know.
#[derive(Clone)]
pub struct Sessions {
    store: Option<Arc<SessionStore>>,
    known: Arc<Mutex<HashMap<(String, String), String>>>,
    runtime: Arc<Runtime>,
}

impl Sessions {
    /// The sessions kept in `folder` (the folder of the preferences), or in memory alone without one.
    pub fn new(folder: Option<&Path>, runtime: Arc<Runtime>) -> Self {
        let sessions = Self {
            store: folder.map(|folder| Arc::new(SessionStore::new(folder))),
            known: Arc::default(),
            runtime,
        };
        sessions.load();
        sessions
    }

    /// Reads the file into memory, from the runtime. A session opened meanwhile is the newer one and
    /// stays.
    fn load(&self) {
        let Some(store) = self.store.clone() else {
            return;
        };
        let known = self.known.clone();
        self.runtime.spawn_blocking(move || {
            let stored = store.all();
            let mut known = known.lock().unwrap_or_else(PoisonError::into_inner);
            for (task, account, session) in stored {
                known.entry((task, account)).or_insert(session);
            }
        });
    }

    /// The lavori that have a session kept, with any account.
    pub fn tasks(&self) -> std::collections::HashSet<String> {
        self.known().keys().map(|(task, _)| task.clone()).collect()
    }

    /// The session last opened for `task` with `account`, if one is kept.
    pub fn stored(&self, task: &str, account: &str) -> Option<String> {
        let key = (task.to_string(), account.to_string());
        self.known().get(&key).cloned()
    }

    /// Keeps `session` for `task` with `account`: at once in memory, and in the file from the runtime.
    pub fn store(&self, task: &str, account: &str, session: &str) {
        let key = (task.to_string(), account.to_string());
        self.known().insert(key, session.to_string());
        let Some(store) = self.store.clone() else {
            return;
        };
        let (task, account, session) = (task.to_string(), account.to_string(), session.to_string());
        self.runtime.spawn_blocking(move || {
            if let Err(error) = store.set(&task, &account, &session) {
                eprintln!("sakura: the session of {task} was not kept: {error}");
            }
        });
    }

    /// Forgets the sessions of every lavoro `exists` no longer accepts.
    pub fn retain(&self, exists: impl Fn(&str) -> bool) {
        let gone: Vec<String> = {
            let mut known = self.known();
            let gone = known.keys().filter(|(task, _)| !exists(task));
            let gone: Vec<String> = gone.map(|(task, _)| task.clone()).collect();
            known.retain(|(task, _), _| exists(task));
            gone
        };
        let Some(store) = self.store.clone() else {
            return;
        };
        for task in gone {
            let store = store.clone();
            self.runtime.spawn_blocking(move || {
                if let Err(error) = store.forget(&task) {
                    eprintln!("sakura: the session of {task} was not forgotten: {error}");
                }
            });
        }
    }

    fn known(&self) -> std::sync::MutexGuard<'_, HashMap<(String, String), String>> {
        self.known.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runtime() -> Arc<Runtime> {
        Arc::new(Runtime::new().expect("a runtime"))
    }

    #[test]
    fn a_session_is_known_at_once_and_read_from_the_file_by_another_launch() {
        let folder = tempfile::tempdir().expect("a folder");
        let runtime = runtime();
        let sessions = Sessions::new(Some(folder.path()), runtime.clone());
        assert_eq!(sessions.stored("m1", "personal"), None);
        sessions.store("m1", "personal", "session-7");
        assert_eq!(
            sessions.stored("m1", "personal").as_deref(),
            Some("session-7")
        );
        assert_eq!(sessions.stored("m1", "other"), None, "kept per account");
        // The file is written and read from the runtime: another launch knows the session once both
        // are done.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let is_read = || {
            let next = Sessions::new(Some(folder.path()), runtime.clone());
            std::thread::sleep(std::time::Duration::from_millis(50));
            next.stored("m1", "personal").as_deref() == Some("session-7")
        };
        while !is_read() && std::time::Instant::now() < deadline {}
        assert!(is_read(), "the next launch reads the session from the file");
    }

    #[test]
    fn the_sessions_of_a_lavoro_that_is_gone_are_forgotten() {
        let sessions = Sessions::new(None, runtime());
        sessions.store("m1", "", "a");
        sessions.store("gone", "", "b");
        sessions.retain(|task| task == "m1");
        assert_eq!(sessions.stored("m1", "").as_deref(), Some("a"));
        assert_eq!(sessions.stored("gone", ""), None);
    }
}
