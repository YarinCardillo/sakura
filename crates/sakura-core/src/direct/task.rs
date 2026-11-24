//! One task's files and history, read in-process: what `box tree`, `box file`, `box search`,
//! `box diff` and `box log` answered, in the same shapes and with the same limits, for a task on the
//! Mac and a task in a box alike.

use super::git::{self, Leaf, Mode, Oid, Repo};
use super::ignores::Ignores;
use super::patch_text::{self, Content, PatchText, BLOB_MAX_BYTES};
use super::place::TaskRecord;
use super::root::Root;
use super::status::{self, Change, Side};
use super::time::iso;
use crate::error::{BridgeError, BridgeResult};
use crate::files::{self, Entry, EntryKind, FileBody, Level, Matches};
use crate::models::{Commit as LogCommit, Place};
use crate::said::Said;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, PoisonError};

/// The most entries a level lists, as `box tree` did.
const LEVEL_MAX_ENTRIES: usize = 2000;
/// The most file names a search answers, as `box search` did.
const SEARCH_MAX_MATCHES: usize = 200;
/// The most commits ahead of the base listed, as `box log` did.
pub const LOG_MAX_COMMITS: usize = 200;
/// The most files git does not know that one patch shows, as `box diff` did.
const PATCH_MAX_UNTRACKED: usize = 1000;
/// The most files git does not know that the tree and quick open are told of.
const UNTRACKED_MAX: usize = 10_000;
/// The largest `info/exclude` read.
const EXCLUDE_MAX_BYTES: u64 = 1024 * 1024;

/// The files and history of one task.
pub struct TaskReader {
    record: TaskRecord,
    root: Root,
    repo: Repo,
    exclude: Option<Vec<u8>>,
    /// The files of the trees compared with the work tree, by tree: a tree never changes.
    trees: Mutex<HashMap<Oid, Arc<BTreeMap<String, Leaf>>>>,
}

/// The work tree against the commit it left its base at, at one moment.
struct Snapshot {
    changes: Vec<Change>,
    tracked: Vec<String>,
    untracked: Vec<String>,
}

impl TaskReader {
    /// Opens the task the service recorded: its folder, checked as the service checks it, and its
    /// repository.
    pub fn open(record: TaskRecord) -> BridgeResult<Self> {
        let (root, repo) = match record.place {
            Place::Mac => {
                let root = match record.identity {
                    Some(identity) => Root::open_expecting(&record.folder, identity)?,
                    None => Root::open(&record.folder)?,
                };
                let unrecorded =
                    || git::unsupported("the service recorded no git directory for the lavoro");
                let git_dir = record.git_dir.as_deref().ok_or_else(unrecorded)?;
                let repo = Repo::of_lavoro(&root, git_dir)?;
                (root, repo)
            }
            Place::Box => {
                let root = Root::open(&record.folder)?;
                let repo = Repo::of_box(&root)?;
                (root, repo)
            }
        };
        let exclude = repo.exclude(EXCLUDE_MAX_BYTES);
        Ok(Self {
            record,
            root,
            repo,
            exclude,
            trees: Mutex::default(),
        })
    }

    /// What the service recorded of the task.
    pub fn record(&self) -> &TaskRecord {
        &self.record
    }

    /// The repository of the task.
    pub fn repo(&self) -> &Repo {
        &self.repo
    }

    /// The body of one file of the work tree.
    pub fn file(&self, path: &str) -> BridgeResult<FileBody> {
        self.root.file(files::relative_path(path)?)
    }

    /// One folder of the work tree as the Files tree shows it: what is in it but ignored, what changed
    /// against the base with its lines, and the files of the base that are gone from it.
    pub fn level(&self, dir: &str) -> BridgeResult<Level> {
        let dir = files::relative_path(dir)?.trim_matches('/');
        let listing = self.root.dir(dir)?.list()?;
        let snapshot = self.snapshot(UNTRACKED_MAX)?;
        let prefix = if dir.is_empty() {
            String::new()
        } else {
            format!("{dir}/")
        };
        let ignores = Ignores::new(&self.root, self.exclude.as_deref());
        let mut entries: BTreeMap<String, Entry> = listing
            .entries
            .into_iter()
            .filter(|(name, kind)| {
                let path = format!("{prefix}{name}");
                is_tracked(&snapshot.tracked, &path)
                    || !ignores.is_ignored(&path, *kind == EntryKind::Dir)
            })
            .map(|(name, kind)| (name.clone(), plain_entry(name, kind)))
            .collect();
        self.mark_changes(&snapshot, &prefix, &mut entries)?;
        let truncated = listing.truncated || entries.len() > LEVEL_MAX_ENTRIES;
        let entries = entries.into_values().take(LEVEL_MAX_ENTRIES).collect();
        Level {
            path: dir.to_string(),
            entries,
            truncated,
        }
        .checked(dir)
    }

    /// The files whose path holds `text` (letter case not counting), tracked or not and not ignored: a
    /// match in the file's own name first, shorter paths first.
    pub fn search(&self, text: &str) -> BridgeResult<Matches> {
        let query = files::query(text)?;
        if query.trim().is_empty() {
            return Err(BridgeError::invalid(Said::new("files.invalid.query")));
        }
        let wanted = query.to_lowercase();
        let snapshot = self.snapshot(UNTRACKED_MAX)?;
        let mut found: Vec<&String> = snapshot
            .tracked
            .iter()
            .chain(&snapshot.untracked)
            .filter(|path| path.to_lowercase().contains(&wanted))
            .collect();
        found.sort_by_key(|path| {
            let name = path.rsplit('/').next().unwrap_or(path).to_lowercase();
            (!name.contains(&wanted), path.len(), path.to_string())
        });
        found.dedup();
        Ok(Matches {
            query: query.to_string(),
            truncated: found.len() > SEARCH_MAX_MATCHES,
            matches: found
                .into_iter()
                .take(SEARCH_MAX_MATCHES)
                .cloned()
                .collect(),
        })
    }

    /// The patch of the work tree against the commit it left its base at, every file git does not know
    /// yet as a new file.
    pub fn working_patch(&self) -> BridgeResult<String> {
        let snapshot = self.snapshot(PATCH_MAX_UNTRACKED)?;
        let mut patch = PatchText::default();
        for change in &snapshot.changes {
            if patch.is_full() {
                break;
            }
            let old = change.old.map(|(mode, oid)| (mode, self.blob(mode, oid)));
            let new = match change.new {
                Some(side) => Some((side.mode, self.work_content(&change.path, side)?)),
                None => None,
            };
            patch.file(&change.path, old, new);
        }
        Ok(patch.finish())
    }

    /// The patch of one commit ahead of the base against its first parent.
    pub fn commit_patch(&self, sha: &str) -> BridgeResult<String> {
        let oid = Oid::from_hex(sha).ok_or_else(|| not_a_commit(sha))?;
        let listed = self.ahead()?;
        let commit = listed
            .iter()
            .find(|(id, _)| *id == oid)
            .map(|(_, c)| c.clone());
        let commit = commit.ok_or_else(|| not_a_commit(sha))?;
        let parent = match commit.parents.first() {
            Some(parent) => Some(self.repo.commit(*parent)?.tree),
            None => None,
        };
        let mut patch = PatchText::default();
        for (path, (old, new)) in git::changed(&self.repo, parent, Some(commit.tree))? {
            if patch.is_full() {
                break;
            }
            let old = old.map(|(mode, oid)| (mode, self.blob(mode, oid)));
            let new = new.map(|(mode, oid)| (mode, self.blob(mode, oid)));
            patch.file(&path, old, new);
        }
        Ok(patch.finish())
    }

    /// The commits ahead of the base, newest first, with what each changed against its first parent.
    pub fn log(&self) -> BridgeResult<Vec<LogCommit>> {
        self.ahead()?
            .into_iter()
            .map(|(oid, commit)| {
                let parent = match commit.parents.first() {
                    Some(parent) => Some(self.repo.commit(*parent)?.tree),
                    None => None,
                };
                let changed = git::changed(&self.repo, parent, Some(commit.tree))?;
                let (insertions, deletions) =
                    changed.values().fold((0u32, 0u32), |(a, d), (old, new)| {
                        let (added, removed) = self.counts(*old, *new);
                        (a.saturating_add(added), d.saturating_add(removed))
                    });
                Ok(LogCommit {
                    sha: oid.hex(),
                    subject: commit.subject,
                    time: iso(commit.time),
                    files_changed: changed.len() as u32,
                    insertions,
                    deletions,
                })
            })
            .collect()
    }

    /// The commits ahead of the base, newest first, at most [`LOG_MAX_COMMITS`].
    pub fn ahead(&self) -> BridgeResult<Vec<(Oid, git::Commit)>> {
        match self.repo.head()? {
            Some(head) => git::ahead(&self.repo, Some(self.record.base), head, LOG_MAX_COMMITS),
            None => Ok(Vec::new()),
        }
    }

    /// Reads the work tree against the commit HEAD left the base at (the recorded base when they share
    /// none).
    fn snapshot(&self, untracked_max: usize) -> BridgeResult<Snapshot> {
        let base = match self.repo.head()? {
            Some(head) => {
                git::merge_base(&self.repo, self.record.base, head)?.unwrap_or(self.record.base)
            }
            None => self.record.base,
        };
        let tree = self.tree_files(base)?;
        let (index, index_stat) = self.repo.index()?;
        let ignores = Ignores::new(&self.root, self.exclude.as_deref());
        let work = status::work_tree(&self.root, &index, index_stat, &ignores, untracked_max)?;
        let tracked: Vec<String> = index
            .entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect();
        let untracked = work
            .files
            .iter()
            .filter(|(path, side)| side.oid.is_none() && tracked.binary_search(path).is_err())
            .map(|(path, _)| path.clone())
            .collect();
        Ok(Snapshot {
            changes: status::changes(&tree, &work),
            tracked,
            untracked,
        })
    }

    fn tree_files(&self, commit: Oid) -> BridgeResult<Arc<BTreeMap<String, Leaf>>> {
        let tree = self.repo.commit(commit)?.tree;
        if let Some(files) = self
            .trees
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&tree)
        {
            return Ok(files.clone());
        }
        let files = Arc::new(git::files(&self.repo, tree)?);
        let mut trees = self.trees.lock().unwrap_or_else(PoisonError::into_inner);
        trees.clear();
        trees.insert(tree, files.clone());
        Ok(files)
    }

    /// Marks what changed in the level at `prefix`, and adds the files of the base gone from it.
    fn mark_changes(
        &self,
        snapshot: &Snapshot,
        prefix: &str,
        entries: &mut BTreeMap<String, Entry>,
    ) -> BridgeResult<()> {
        for change in snapshot
            .changes
            .iter()
            .filter(|change| change.path.starts_with(prefix))
        {
            let rest = &change.path[prefix.len()..];
            let (name, is_itself) = match rest.split_once('/') {
                Some((name, _)) => (name, false),
                None => (rest, true),
            };
            if is_itself && change.new.is_none() && !entries.contains_key(name) {
                entries.insert(
                    name.to_string(),
                    plain_entry(name.to_string(), EntryKind::Deleted),
                );
            }
            let Some(entry) = entries.get_mut(name) else {
                continue;
            };
            entry.changed = true;
            if is_itself && entry.kind != EntryKind::Dir {
                let new = match change.new {
                    Some(side) => Some((side.mode, self.work_content(&change.path, side)?)),
                    None => None,
                };
                let old = change.old.map(|(mode, oid)| (mode, self.blob(mode, oid)));
                let empty = || Content::Bytes(Arc::default());
                let counts = patch_text::line_counts(
                    &old.map_or_else(empty, |(_, content)| content),
                    &new.map_or_else(empty, |(_, content)| content),
                );
                (entry.added, entry.removed) = counts.unzip();
            }
        }
        Ok(())
    }

    fn counts(&self, old: Option<Leaf>, new: Option<Leaf>) -> (u32, u32) {
        let content = |leaf: Option<Leaf>| {
            leaf.map_or(Content::Bytes(Arc::default()), |(mode, oid)| {
                self.blob(mode, oid)
            })
        };
        patch_text::line_counts(&content(old), &content(new)).unwrap_or((0, 0))
    }

    /// A blob of the repository as a side of a patch.
    fn blob(&self, mode: Mode, oid: Oid) -> Content {
        if mode == Mode::Gitlink {
            return Content::Bytes(Arc::new(format!("Subproject commit {oid}\n").into_bytes()));
        }
        match self.repo.blob(oid, BLOB_MAX_BYTES) {
            Ok(Some(bytes)) => Content::Bytes(bytes),
            Ok(None) | Err(_) => Content::TooLarge,
        }
    }

    /// A file of the work tree as a side of a patch: a link's target, a file's bytes.
    fn work_content(&self, path: &str, side: Side) -> BridgeResult<Content> {
        let (folder, name) = path.rsplit_once('/').unwrap_or(("", path));
        let bytes = match side.mode {
            Mode::Gitlink => {
                return Ok(self.blob(Mode::Gitlink, side.oid.unwrap_or(git::EMPTY_TREE)))
            }
            Mode::Link => self.root.dir(folder)?.read_link(name)?,
            _ => match self.root.dir(folder)?.read(name, BLOB_MAX_BYTES as u64)? {
                Some(bytes) => bytes,
                None => return Ok(Content::TooLarge),
            },
        };
        Ok(Content::Bytes(Arc::new(bytes)))
    }
}

fn plain_entry(name: String, kind: EntryKind) -> Entry {
    Entry {
        name,
        kind,
        changed: false,
        added: None,
        removed: None,
    }
}

/// Whether `path` is a tracked file or a folder that holds one (`tracked` sorted).
fn is_tracked(tracked: &[String], path: &str) -> bool {
    let folder = format!("{path}/");
    let first_below = tracked.partition_point(|known| known.as_str() < folder.as_str());
    tracked
        .binary_search_by(|known| known.as_str().cmp(path))
        .is_ok()
        || tracked
            .get(first_below)
            .is_some_and(|known| known.starts_with(&folder))
}

fn not_a_commit(sha: &str) -> BridgeError {
    BridgeError::invalid(Said::new("error.invalid.commit").text("value", sha))
}
