//! The files of a lane's worktree, read in-process through the core's safe reader (no link is followed out
//! of the folder, every read has a size limit), as the Files view and quick open ask for them. A path outside
//! the worktree is refused before anything is read.

use crate::ui::files::{joined, on_runtime, Reading, Source};
use sakura_core::direct::Root;
use sakura_core::error::{BridgeError, BridgeResult, ErrorCode};
use sakura_core::files::{self, EntryKind, FileBody, Level, Matches};
use sakura_core::said::Said;
use std::collections::VecDeque;
use std::path::Path;
use std::sync::Arc;
use tokio::runtime::Runtime;

/// The most files one search looks at before it says it left some out.
const WALK_LIMIT: usize = 20_000;
/// The most names a search answers.
const MATCHES_LIMIT: usize = 200;
/// Folders that hold what a build made or fetched, never what a person opens by name.
const NOT_WALKED: [&str; 6] = [
    "node_modules",
    "target",
    "dist",
    "build",
    ".venv",
    "__pycache__",
];

/// The files of the worktree at one folder.
#[derive(Clone)]
pub struct WorktreeSource {
    folder: Arc<std::path::PathBuf>,
    runtime: Arc<Runtime>,
}

impl WorktreeSource {
    /// The files below `folder`, read on `runtime`'s blocking threads.
    pub fn new(folder: &Path, runtime: Arc<Runtime>) -> Self {
        Self {
            folder: Arc::new(folder.to_path_buf()),
            runtime,
        }
    }

    fn read<T: Send + 'static>(
        &self,
        read: impl FnOnce(&Root) -> BridgeResult<T> + Send + 'static,
    ) -> Reading<T> {
        let folder = self.folder.clone();
        let runtime = self.runtime.clone();
        on_runtime(&runtime, async move {
            let work = tokio::task::spawn_blocking(move || read(&Root::open(&folder)?));
            work.await.unwrap_or_else(|error| Err(joined(error)))
        })
    }
}

impl Source for WorktreeSource {
    /// A level is never asked of a lane: its Files view shows one file, not a tree.
    fn level(&self, _dir: &str) -> Reading<Level> {
        let said = Said::new("files.invalid.level");
        on_runtime(&self.runtime, async move {
            Err(BridgeError::new(ErrorCode::Unsupported, said))
        })
    }

    fn file(&self, path: &str) -> Reading<FileBody> {
        let path = path.to_string();
        self.read(move |root| root.file(files::relative_path(&path)?))
    }

    fn search(&self, text: &str) -> Reading<Matches> {
        let text = text.to_string();
        self.read(move |root| find(root, &text))
    }
}

/// The files whose path holds `text` (letter case not counting), a match in the file's own name first, then
/// shorter paths first.
fn find(root: &Root, text: &str) -> BridgeResult<Matches> {
    let query = files::query(text)?;
    let wanted = query.to_lowercase();
    let (mut found, mut is_cut) = (Vec::new(), false);
    let (mut seen, mut queue) = (0, VecDeque::from([String::new()]));
    'walk: while let Some(dir) = queue.pop_front() {
        let Ok(listing) = root.dir(&dir).and_then(|dir| dir.list()) else {
            continue;
        };
        for (name, kind) in listing.entries {
            let path = if dir.is_empty() {
                name.clone()
            } else {
                format!("{dir}/{name}")
            };
            match kind {
                EntryKind::Dir if !NOT_WALKED.contains(&name.as_str()) => queue.push_back(path),
                EntryKind::File => {
                    seen += 1;
                    if path.to_lowercase().contains(&wanted) {
                        found.push(path);
                    }
                }
                _ => {}
            }
            if seen >= WALK_LIMIT {
                is_cut = true;
                break 'walk;
            }
        }
    }
    found.sort_by_key(|path| {
        let name = path.rsplit('/').next().unwrap_or(path).to_lowercase();
        (!name.contains(&wanted), path.len(), path.clone())
    });
    let truncated = is_cut || found.len() > MATCHES_LIMIT;
    found.truncate(MATCHES_LIMIT);
    Ok(Matches {
        query: query.to_string(),
        matches: found,
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn worktree() -> (tempfile::TempDir, WorktreeSource) {
        let dir = tempfile::tempdir().expect("a folder");
        for (path, body) in [
            ("src/upload/queue.ts", "export const MAX_TRIES = 5\n"),
            ("src/api/client.ts", "export {}\n"),
            ("node_modules/dep/queue.js", "x"),
            ("README.md", "# Demo\n"),
        ] {
            let at = dir.path().join(path);
            std::fs::create_dir_all(at.parent().expect("a parent")).expect("folders");
            std::fs::write(at, body).expect("a file");
        }
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("a runtime");
        let source = WorktreeSource::new(dir.path(), Arc::new(runtime));
        (dir, source)
    }

    fn block<T>(reading: Reading<T>, source: &WorktreeSource) -> BridgeResult<T> {
        source.runtime.block_on(reading)
    }

    #[test]
    fn a_file_of_the_worktree_is_read_and_a_path_outside_it_never_is() {
        let (_dir, source) = worktree();
        let body = block(source.file("src/upload/queue.ts"), &source).expect("a file");
        assert_eq!(body, FileBody::Text("export const MAX_TRIES = 5\n".into()));
        for outside in ["../x", "/etc/hosts", "-rf", "src/../../x"] {
            assert!(block(source.file(outside), &source).is_err(), "{outside}");
        }
    }

    #[test]
    fn a_link_that_leaves_the_worktree_is_not_followed() {
        let (dir, source) = worktree();
        let secret = tempfile::tempdir().expect("another folder");
        std::fs::write(secret.path().join("secret.txt"), "no").expect("a file");
        std::os::unix::fs::symlink(secret.path(), dir.path().join("out")).expect("a link");
        assert!(block(source.file("out/secret.txt"), &source).is_err());
    }

    #[test]
    fn a_search_finds_names_best_first_and_leaves_what_builds_make_out() {
        let (_dir, source) = worktree();
        let found = block(source.search("queue"), &source).expect("a search");
        assert_eq!(found.matches, ["src/upload/queue.ts"]);
        let found = block(source.search("CLIENT"), &source).expect("a search");
        assert_eq!(found.matches, ["src/api/client.ts"]);
        assert!(block(source.search("-flag"), &source).is_err());
    }
}
