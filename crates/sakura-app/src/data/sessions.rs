//! The session of each task's conversation, kept so that the conversation can be taken up again: in
//! memory while the app runs, and in the folder of the preferences across launches. The file is read
//! once, when the app starts, and written when a session opens, both off the UI thread: asking for a
//! session reads memory only, so a view may ask on every frame.

use sakura_acp::store::SessionStore;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use tokio::runtime::Runtime;
use tokio::sync::watch;

/// The stored sessions, by task and account. Cheap to clone; clones share what they know.
#[derive(Clone)]
pub struct Sessions {
    store: Option<Arc<SessionStore>>,
    /// The folder the sessions are kept in, when they are kept at all.
    #[cfg_attr(feature = "boxes", allow(dead_code))]
    folder: Option<std::path::PathBuf>,
    known: Arc<Mutex<HashMap<(String, String), String>>>,
    /// The words of the refusals said in each session: a replay shows them as refusals again.
    refused: Arc<Mutex<HashMap<String, Vec<String>>>>,
    runtime: Arc<Runtime>,
    /// Whether the file was read into memory: until then `stored` may miss a session that is kept.
    loaded: Arc<watch::Sender<bool>>,
}

impl Sessions {
    /// The sessions kept in `folder` (the folder of the preferences), or in memory alone without one.
    pub fn new(folder: Option<&Path>, runtime: Arc<Runtime>) -> Self {
        let sessions = Self {
            store: folder.map(|folder| Arc::new(SessionStore::new(folder))),
            folder: folder.map(Path::to_path_buf),
            known: Arc::default(),
            refused: Arc::default(),
            runtime,
            loaded: Arc::new(watch::channel(false).0),
        };
        sessions.load();
        sessions
    }

    /// The folder the sessions are kept in: what else the app keeps about its conversations goes beside them.
    #[cfg(not(feature = "boxes"))]
    pub fn folder(&self) -> Option<&Path> {
        self.folder.as_deref()
    }

    /// Whether what was kept is known: the file was read (at once when there is none).
    #[cfg(not(feature = "boxes"))]
    pub fn is_loaded(&self) -> bool {
        *self.loaded.borrow()
    }

    /// Resolves when what was kept is known, at once when it already is.
    #[cfg(not(feature = "boxes"))]
    pub async fn when_loaded(&self) {
        let mut ready = self.loaded.subscribe();
        // The sender lives as long as the sessions, which this call borrows.
        drop(ready.wait_for(|is_loaded| *is_loaded).await);
    }

    /// Sessions whose file has not been read yet, for the tests; `finish_loading` ends the wait.
    #[cfg(all(test, not(feature = "boxes")))]
    pub fn loading(runtime: Arc<Runtime>) -> Self {
        Self {
            store: None,
            folder: None,
            known: Arc::default(),
            refused: Arc::default(),
            runtime,
            loaded: Arc::new(watch::channel(false).0),
        }
    }

    /// Says the file was read, for the tests.
    #[cfg(all(test, not(feature = "boxes")))]
    pub fn finish_loading(&self) {
        self.loaded.send_replace(true);
    }

    /// Reads the file into memory, from the runtime. A session opened meanwhile is the newer one and
    /// stays.
    fn load(&self) {
        let Some(store) = self.store.clone() else {
            self.loaded.send_replace(true);
            return;
        };
        let (known, refused, loaded) = (
            self.known.clone(),
            self.refused.clone(),
            self.loaded.clone(),
        );
        self.runtime.spawn_blocking(move || {
            // The refusals first: a session that can be requested already has its own.
            let kept = store.refusals();
            let mut refused = refused.lock().unwrap_or_else(PoisonError::into_inner);
            for (session, words) in kept {
                refused.entry(session).or_insert(words);
            }
            drop(refused);
            let stored = store.all();
            let mut known = known.lock().unwrap_or_else(PoisonError::into_inner);
            for (task, account, session) in stored {
                known.entry((task, account)).or_insert(session);
            }
            drop(known);
            loaded.send_replace(true);
        });
    }

    /// The tasks that have a session kept, with any account.
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

    /// The words of the refusals said in `session`. Reads memory only.
    pub fn refusals(&self, session: &str) -> Vec<String> {
        let refused = self.refused.lock().unwrap_or_else(PoisonError::into_inner);
        refused.get(session).cloned().unwrap_or_default()
    }

    /// Keeps `words` as a refusal said in `session`: at once in memory, and in the file from the runtime.
    pub fn keep_refusal(&self, session: &str, words: &str) {
        if words.is_empty() {
            return;
        }
        {
            let mut refused = self.refused.lock().unwrap_or_else(PoisonError::into_inner);
            let said = refused.entry(session.to_string()).or_default();
            if !said.iter().any(|kept| kept == words) {
                said.push(words.to_string());
            }
        }
        let Some(store) = self.store.clone() else {
            return;
        };
        let (session, words) = (session.to_string(), words.to_string());
        self.runtime.spawn_blocking(move || {
            if let Err(error) = store.add_refusal(&session, &words) {
                eprintln!("sakura: a refusal of the conversation was not kept: {error}");
            }
        });
    }

    /// The task and the account of the conversation kept for the slot of a tab (see `master_accounts::session_key`):
    /// what a conversation needs to be taken up again when its account is not known.
    #[cfg_attr(feature = "boxes", allow(dead_code))]
    pub fn find_slot(&self, slot: &str) -> Option<(String, String)> {
        let known = self.known();
        if slot.is_empty() {
            // Kept under no tab: the account alone, which names none when it holds no `@`.
            let found = known
                .keys()
                .find(|(_, key)| !key.is_empty() && !key.contains('@'));
            return found.map(|(task, key)| (task.clone(), key.clone()));
        }
        let suffix = format!("@{slot}");
        let found = known.keys().find(|(_, key)| key.ends_with(&suffix));
        found.map(|(task, key)| (task.clone(), key[..key.len() - suffix.len()].to_string()))
    }

    /// The session kept for `task` under any account or tab, for the tests.
    #[cfg(test)]
    #[cfg_attr(feature = "boxes", allow(dead_code))]
    pub fn stored_any(&self, task: &str) -> Option<String> {
        let known = self.known();
        let found = known.iter().find(|((kept, _), _)| kept == task);
        found.map(|(_, session)| session.clone())
    }

    /// Forgets the session of `task` with `account` alone.
    #[cfg_attr(feature = "boxes", allow(dead_code))]
    pub fn forget(&self, task: &str, account: &str) {
        let key = (task.to_string(), account.to_string());
        self.known().remove(&key);
        let Some(store) = self.store.clone() else {
            return;
        };
        let (task, account) = (task.to_string(), account.to_string());
        self.runtime.spawn_blocking(move || {
            if let Err(error) = store.forget_account(&task, &account) {
                eprintln!("sakura: the session of {task} was not forgotten: {error}");
            }
        });
    }

    /// Forgets the sessions of every task `exists` no longer accepts.
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
    fn the_conversation_of_a_slot_is_found_with_the_task_and_the_account_it_was_kept_under() {
        let sessions = Sessions::new(None, runtime());
        sessions.store("@master", "personal@t1", "a");
        sessions.store("@master-codex", "codex@t2", "b");
        sessions.store("@master", "personal", "c");
        assert_eq!(
            sessions.find_slot("t2"),
            Some(("@master-codex".to_string(), "codex".to_string()))
        );
        assert_eq!(
            sessions.find_slot("t1").map(|found| found.1),
            Some("personal".into())
        );
        assert_eq!(sessions.find_slot("t3"), None);
    }

    #[test]
    fn the_conversation_kept_under_no_tab_is_found_with_the_account_it_was_kept_under() {
        let sessions = Sessions::new(None, runtime());
        sessions.store("@master", "personal@t1", "a");
        sessions.store("@master", "claude-aa0b", "b");
        assert_eq!(
            sessions.find_slot(""),
            Some(("@master".to_string(), "claude-aa0b".to_string()))
        );
    }

    #[test]
    fn one_account_s_session_is_forgotten_and_the_others_of_the_task_stay() {
        let sessions = Sessions::new(None, runtime());
        sessions.store("m1", "", "a");
        sessions.store("m1", "@2", "b");
        sessions.forget("m1", "@2");
        assert_eq!(sessions.stored("m1", "@2"), None);
        assert_eq!(sessions.stored("m1", "").as_deref(), Some("a"));
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
