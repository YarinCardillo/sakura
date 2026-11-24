//! What the work tree holds differently from a commit: the tracked files, known from the index when
//! their status has not changed and hashed otherwise, and the files git does not know yet that it does
//! not ignore. No filter, attribute or hook is applied: a file is compared byte for byte.

use super::git::{Index, IndexEntry, Leaf, Mode, Oid};
use super::ignores::{Ignores, IGNORE_FILE, IGNORE_FILE_MAX_BYTES};
use super::root::{Dir, Root};
use super::stat::Stat;
use crate::error::BridgeResult;
use crate::files::EntryKind;
use std::collections::{BTreeMap, HashSet};

/// The largest file hashed to tell whether it changed; a larger one counts as changed.
const HASH_MAX_BYTES: u64 = 16 * 1024 * 1024;
/// The most entries of the work tree looked at for files git does not know.
const WALK_ENTRIES_MAX: usize = 200_000;
/// The deepest folder looked into for them.
const WALK_DEPTH_MAX: usize = 64;

/// A file of the work tree as compared: its mode and, when known, its blob id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Side {
    pub mode: Mode,
    /// `None` for a file git does not know or one too large to hash.
    pub oid: Option<Oid>,
}

/// One path that differs: the commit's leaf and the work tree's side, each absent when that side
/// has no file there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub path: String,
    pub old: Option<Leaf>,
    pub new: Option<Side>,
}

/// The files of the work tree, tracked and not.
pub struct WorkTree {
    pub files: BTreeMap<String, Side>,
}

/// Reads the work tree at `root` with its `index` (whose file has status `index_stat`), looking for at
/// most `untracked_max` files git does not know.
pub fn work_tree(
    root: &Root,
    index: &Index,
    index_stat: Option<Stat>,
    ignores: &Ignores,
    untracked_max: usize,
) -> BridgeResult<WorkTree> {
    let mut files = tracked(root, index, index_stat)?;
    let known: HashSet<&str> = index
        .entries
        .iter()
        .map(|entry| entry.path.as_str())
        .collect();
    let mut walk = Untracked {
        known: &known,
        ignores,
        found: Vec::new(),
        max: untracked_max,
        seen: 0,
    };
    walk.folder(&root.top()?, "", 0)?;
    for (path, mode) in walk.found {
        files.insert(path, Side { mode, oid: None });
    }
    Ok(WorkTree { files })
}

/// The paths where `tree` and the work tree differ, sorted.
pub fn changes(tree: &BTreeMap<String, Leaf>, work: &WorkTree) -> Vec<Change> {
    let paths: std::collections::BTreeSet<&String> = tree.keys().chain(work.files.keys()).collect();
    paths
        .into_iter()
        .filter_map(|path| {
            let (old, new) = (tree.get(path).copied(), work.files.get(path).copied());
            let is_same = matches!((old, new), (Some((mode, oid)), Some(side)) if side.mode == mode && side.oid == Some(oid));
            (!is_same).then(|| Change {
                path: path.clone(),
                old,
                new,
            })
        })
        .collect()
}

/// The tracked files as they are in the work tree, a file missing or replaced by a folder left out.
fn tracked(
    root: &Root,
    index: &Index,
    index_stat: Option<Stat>,
) -> BridgeResult<BTreeMap<String, Side>> {
    let mut files = BTreeMap::new();
    let mut folders = Folders::new(root.top()?);
    let mut previous: Option<&str> = None;
    for entry in &index.entries {
        if previous == Some(entry.path.as_str()) {
            continue;
        }
        previous = Some(entry.path.as_str());
        let (folder, name) = match entry.path.rsplit_once('/') {
            Some((folder, name)) => (folder, name),
            None => ("", entry.path.as_str()),
        };
        let Some(dir) = folders.open(folder)? else {
            continue;
        };
        if let Some(side) = side_of(dir, name, entry, index_stat)? {
            files.insert(entry.path.clone(), side);
        }
    }
    Ok(files)
}

/// The work tree's side of one tracked file, or nothing when it is gone.
fn side_of(
    dir: &Dir,
    name: &str,
    entry: &IndexEntry,
    index_stat: Option<Stat>,
) -> BridgeResult<Option<Side>> {
    let Some(stat) = dir.stat_child(name) else {
        return Ok(None);
    };
    let side = match (stat.kind, entry.mode) {
        (EntryKind::Dir, Mode::Gitlink) => Side {
            mode: Mode::Gitlink,
            oid: Some(entry.oid),
        },
        (EntryKind::File, _) => {
            let mode = if stat.is_executable() {
                Mode::Executable
            } else {
                Mode::File
            };
            let oid = match entry.stage == 0
                && mode == entry.mode
                && is_unchanged(&stat, entry, index_stat)
            {
                true => Some(entry.oid),
                false => dir
                    .read(name, HASH_MAX_BYTES)?
                    .map(|bytes| Oid::of_blob(&bytes)),
            };
            Side { mode, oid }
        }
        (EntryKind::Link, _) => Side {
            mode: Mode::Link,
            oid: Some(Oid::of_blob(&dir.read_link(name)?)),
        },
        _ => return Ok(None),
    };
    Ok(Some(side))
}

/// Whether the file's status is the one the index saw, and the index was written after the file was
/// last changed (a file changed in the same second as the index is hashed: git's "racy" case).
fn is_unchanged(stat: &Stat, entry: &IndexEntry, index_stat: Option<Stat>) -> bool {
    let (seconds, nanos) = stat.modified;
    let is_same_time = seconds as u32 == entry.modified.0
        && (entry.modified.1 == 0 || nanos as u32 == entry.modified.1);
    let is_after = index_stat.is_some_and(|index| index.modified > stat.modified);
    stat.size as u32 == entry.size && is_same_time && is_after
}

/// The folders of the tracked files, opened down from the root and kept open along the current path
/// only: the index lists paths sorted, so a folder is left once and never come back to.
struct Folders {
    stack: Vec<(String, Option<Dir>)>,
}

impl Folders {
    fn new(top: Dir) -> Self {
        Self {
            stack: vec![(String::new(), Some(top))],
        }
    }

    /// The folder at `path`, never through a link; nothing when it is not a folder any more.
    fn open(&mut self, path: &str) -> BridgeResult<Option<&Dir>> {
        while self.stack.len() > 1 && !is_within(path, &self.stack.last().expect("not empty").0) {
            self.stack.pop();
        }
        let parts: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
        while self.stack.len() <= parts.len() {
            let depth = self.stack.len();
            let child = match &self.stack.last().expect("not empty").1 {
                Some(dir) => dir.child(parts[depth - 1])?,
                None => None,
            };
            self.stack.push((parts[..depth].join("/"), child));
        }
        Ok(self.stack.last().expect("not empty").1.as_ref())
    }
}

fn is_within(path: &str, folder: &str) -> bool {
    folder.is_empty()
        || path == folder
        || path
            .strip_prefix(folder)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// The look for files git does not know.
struct Untracked<'a> {
    known: &'a HashSet<&'a str>,
    ignores: &'a Ignores<'a>,
    found: Vec<(String, Mode)>,
    max: usize,
    seen: usize,
}

impl Untracked<'_> {
    fn folder(&mut self, dir: &Dir, prefix: &str, depth: usize) -> BridgeResult<()> {
        let mut listing = dir.list()?.entries;
        listing.sort_by(|a, b| a.0.cmp(&b.0));
        // The folder's own ignore file is read from the folder already open, or known to be absent
        // from its listing, before anything in it is asked about.
        let has_rules = listing.iter().any(|(name, _)| name == IGNORE_FILE);
        let rules = match has_rules {
            true => dir.read(IGNORE_FILE, IGNORE_FILE_MAX_BYTES).ok().flatten(),
            false => None,
        };
        self.ignores
            .learn(prefix.trim_end_matches('/'), rules.as_deref());
        for (name, kind) in listing {
            if self.found.len() >= self.max || self.seen >= WALK_ENTRIES_MAX {
                return Ok(());
            }
            self.seen += 1;
            let path = format!("{prefix}{name}");
            match kind {
                EntryKind::Dir if depth < WALK_DEPTH_MAX => {
                    self.subfolder(dir, &name, &path, depth)?
                }
                EntryKind::File | EntryKind::Link
                    if !self.known.contains(path.as_str())
                        && !self.ignores.is_ignored(&path, false) =>
                {
                    let mode = match kind {
                        EntryKind::Link => Mode::Link,
                        _ if dir
                            .stat_child(&name)
                            .is_some_and(|stat| stat.is_executable()) =>
                        {
                            Mode::Executable
                        }
                        _ => Mode::File,
                    };
                    self.found.push((path, mode));
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// A folder: walked unless it is ignored or holds a repository of its own.
    fn subfolder(&mut self, dir: &Dir, name: &str, path: &str, depth: usize) -> BridgeResult<()> {
        let is_known = self.known.contains(path);
        if is_known || self.ignores.is_ignored(path, true) {
            return Ok(());
        }
        let Some(child) = dir.child(name)? else {
            return Ok(());
        };
        if child.stat_child(".git").is_some() {
            return Ok(());
        }
        self.folder(&child, &format!("{path}/"), depth + 1)
    }
}
