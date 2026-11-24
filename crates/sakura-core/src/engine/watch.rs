//! Tells the app that something changed under the engine's home, so it reads the snapshot again. Only
//! paths are observed: no file is opened and no symlink followed. The reader debounces; every event
//! that is not a mere access is passed on at once.

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The folder of the engine's state: `SAKURA_ENGINE_HOME`, else the engine's default under the user's
/// Application Support.
pub fn home() -> PathBuf {
    if let Some(named) = std::env::var_os(super::cli::ENGINE_HOME_VAR).filter(|v| !v.is_empty()) {
        return PathBuf::from(named);
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join("Library/Application Support/Sakura/engine")
}

/// The files and folder under the engine's home that the rules it prints come from: the person's changes, the
/// pace, the engine's config, the preset chosen, and the folder of the presets saved.
const RULE_FILE_STEMS: [&str; 3] = ["rules", "pace", "config"];
const RULE_FILE_SUFFIX: &str = ".toml";
const CHOSEN_PRESET: &str = "chosen-preset";
const PRESETS_FOLDER: &str = "presets";

/// What the rule inputs under `root` look like now: for each, its last change and size, none when it is not
/// there. Two stamps that differ mean the rules the engine prints may have changed. Only paths are stat-ed.
pub fn rules_stamp(root: &Path) -> Vec<Option<(std::time::SystemTime, u64)>> {
    let files = RULE_FILE_STEMS.map(|stem| format!("{stem}{RULE_FILE_SUFFIX}"));
    let names = files
        .iter()
        .map(String::as_str)
        .chain([CHOSEN_PRESET, PRESETS_FOLDER]);
    names
        .map(|name| {
            let info = std::fs::symlink_metadata(root.join(name)).ok()?;
            Some((info.modified().ok()?, info.len()))
        })
        .collect()
}

/// A live file watcher; dropping it stops it.
pub struct EngineWatcher {
    _watcher: RecommendedWatcher,
}

impl EngineWatcher {
    /// Watches `root` and calls `on_change` for every change under it. `None` when `root` cannot be
    /// watched (it does not exist yet): the reader's timer then still reads the snapshot.
    pub fn start(root: &Path, on_change: Arc<dyn Fn() + Send + Sync>) -> Option<Self> {
        let root = root.canonicalize().ok()?;
        let mut watcher = notify::recommended_watcher(
            move |result: notify::Result<notify::Event>| match result {
                Ok(event) if matches!(event.kind, EventKind::Access(_)) => {}
                Ok(_) => on_change(),
                Err(error) => {
                    eprintln!("sakura: engine watcher error: {error}");
                    on_change();
                }
            },
        )
        .map_err(|error| eprintln!("sakura: cannot watch the engine home: {error}"))
        .ok()?;
        watcher
            .watch(&root, RecursiveMode::Recursive)
            .map_err(|error| eprintln!("sakura: cannot watch {}: {error}", root.display()))
            .ok()?;
        Some(Self { _watcher: watcher })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;
    use std::time::Duration;

    #[test]
    fn a_file_written_under_the_watched_folder_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let (sender, received) = channel();
        let on_change: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            let _ = sender.send(());
        });
        let _watcher = EngineWatcher::start(dir.path(), on_change).expect("the folder exists");
        std::fs::write(dir.path().join("state.json"), "{}").unwrap();
        assert!(received.recv_timeout(Duration::from_secs(60)).is_ok());
    }

    #[test]
    fn the_rules_stamp_moves_with_a_rule_input_and_not_with_other_files() {
        let dir = tempfile::tempdir().unwrap();
        let before = rules_stamp(dir.path());
        std::fs::write(dir.path().join("ledger.log"), "x").unwrap();
        assert_eq!(rules_stamp(dir.path()), before, "a log is not a rule");
        std::fs::write(dir.path().join("rules.toml"), "lanes = 3\n").unwrap();
        let written = rules_stamp(dir.path());
        assert_ne!(written, before);
        std::fs::write(dir.path().join("rules.toml"), "lanes = 4, more\n").unwrap();
        assert_ne!(rules_stamp(dir.path()), written, "the size moves it too");
    }

    #[test]
    fn a_folder_that_does_not_exist_is_not_watched() {
        let dir = tempfile::tempdir().unwrap();
        let on_change: Arc<dyn Fn() + Send + Sync> = Arc::new(|| {});
        assert!(EngineWatcher::start(&dir.path().join("none"), on_change).is_none());
    }
}
