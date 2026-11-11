//! The tree of a lavoro as far as it was read: a level is asked of `box` only when its folder is opened,
//! so a tree of thousands of files costs what the person looks at. The model holds the levels read, the
//! folders open and the "only touched" switch, and says which levels are still to read; reading is the
//! caller's.

use super::{not_a_level, Entry, EntryKind, Level};
use crate::error::BridgeResult;
use std::collections::{BTreeSet, HashMap, HashSet};

/// The most entries the tree holds over every level it read: what it rebuilds its rows from.
const TREE_MAX_ENTRIES: usize = 100_000;

/// What is under a node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Children {
    /// Not a folder.
    None,
    /// A folder whose level was not read yet.
    Unread,
    /// A folder whose level could not be read: it is not asked again until the person opens it again.
    Failed,
    /// A folder's entries, as far as the switch shows them; `truncated` when `box` left some out.
    Read { nodes: Vec<Node>, truncated: bool },
}

/// One entry of the tree with what is under it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    /// The path from the lavoro's root: what `box file` and `box tree --path` take.
    pub path: String,
    pub entry: Entry,
    pub is_open: bool,
    pub children: Children,
}

/// The tree of one lavoro.
#[derive(Debug, Default)]
pub struct Tree {
    levels: HashMap<String, Level>,
    open: BTreeSet<String>,
    /// Folders the person closed while "only touched" opens the changed ones by itself.
    closed: HashSet<String>,
    /// Levels that were requested and could not be read: nothing asks for them again by itself.
    failed: HashSet<String>,
    only_touched: bool,
}

impl Tree {
    /// An empty tree: its root is the first level to read.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether only what differs from the base is shown.
    pub fn is_only_touched(&self) -> bool {
        self.only_touched
    }

    /// Shows everything, or only what differs from the base with its folders open.
    pub fn set_only_touched(&mut self, only_touched: bool) {
        self.only_touched = only_touched;
        self.closed.clear();
    }

    /// Whether the root was read.
    pub fn is_read(&self) -> bool {
        self.is_level_read("")
    }

    /// Whether the level of `dir` was read.
    pub fn is_level_read(&self, dir: &str) -> bool {
        self.levels.contains_key(dir)
    }

    /// Takes the level `box` answered with when requested `dir`. A level that is not that folder's,
    /// that cannot be shown whole (see [`Level::checked`]) or that would make the tree larger than it
    /// may grow is refused, and the tree stays as it was.
    pub fn set_level(&mut self, dir: &str, level: Level) -> BridgeResult<()> {
        let level = level.checked(dir)?;
        let others = self.levels.iter().filter(|(path, _)| path.as_str() != dir);
        let held: usize = others.map(|(_, level)| level.entries.len()).sum();
        if held + level.entries.len() > TREE_MAX_ENTRIES {
            return Err(not_a_level());
        }
        self.failed.remove(dir);
        self.levels.insert(dir.to_string(), level);
        Ok(())
    }

    /// Notes that the level of `dir` could not be read. A level read before stays as it was.
    pub fn set_failed(&mut self, dir: &str) {
        if !self.is_level_read(dir) {
            self.failed.insert(dir.to_string());
        }
    }

    /// Whether the level of `dir` was requested and could not be read.
    pub fn has_failed(&self, dir: &str) -> bool {
        self.failed.contains(dir)
    }

    /// Lets the level of `dir` be asked again: the person requested it.
    pub fn retry(&mut self, dir: &str) {
        self.failed.remove(dir);
    }

    /// Opens or closes the folder at `dir`, as the person asked.
    pub fn set_open(&mut self, dir: &str, is_open: bool) {
        if is_open {
            self.open.insert(dir.to_string());
            self.closed.remove(dir);
            self.retry(dir);
        } else {
            self.open.remove(dir);
            self.closed.insert(dir.to_string());
        }
    }

    /// Opens every folder above `path`, so its row can be shown.
    pub fn reveal(&mut self, path: &str) {
        for (at, _) in path.match_indices('/') {
            self.set_open(&path[..at], true);
        }
    }

    /// The levels shown that were never read: the root, then every open folder under a level that was
    /// read. A level is listed only once its parent's was read, so nothing is read ahead, and never
    /// after it failed, so nothing is asked twice by itself.
    pub fn unread(&self) -> Vec<String> {
        if !self.is_read() {
            return match self.has_failed("") {
                true => Vec::new(),
                false => vec![String::new()],
            };
        }
        let mut unread = Vec::new();
        self.collect_unread("", &mut unread);
        unread
    }

    /// The levels on screen: the root and every open folder that was read. These are read again when
    /// the lavoro changes.
    pub fn shown(&self) -> Vec<String> {
        let mut shown: Vec<String> = self.levels.keys().cloned().collect();
        shown.retain(|dir| dir.is_empty() || self.is_shown_open(dir));
        shown.sort();
        shown
    }

    /// The root's entries with everything read under them.
    pub fn nodes(&self) -> Vec<Node> {
        self.nodes_of("")
    }

    /// Whether the root level was cut short by `box`.
    pub fn is_truncated(&self) -> bool {
        self.levels.get("").is_some_and(|level| level.truncated)
    }

    fn nodes_of(&self, dir: &str) -> Vec<Node> {
        let Some(level) = self.levels.get(dir) else {
            return Vec::new();
        };
        let shown = level
            .entries
            .iter()
            .filter(|entry| !self.only_touched || entry.changed);
        shown.map(|entry| self.node(dir, entry)).collect()
    }

    fn node(&self, dir: &str, entry: &Entry) -> Node {
        let path = join(dir, &entry.name);
        let is_dir = entry.kind == EntryKind::Dir;
        let children = match self.levels.get(&path) {
            _ if !is_dir => Children::None,
            None if self.has_failed(&path) => Children::Failed,
            None => Children::Unread,
            Some(level) => Children::Read {
                nodes: self.nodes_of(&path),
                truncated: level.truncated,
            },
        };
        Node {
            is_open: is_dir && self.is_open(&path, entry),
            path,
            entry: entry.clone(),
            children,
        }
    }

    /// Whether the folder at `path` is open: the person opened it, or the switch shows only what was
    /// touched, it holds something touched and the person did not close it.
    fn is_open(&self, path: &str, entry: &Entry) -> bool {
        let by_switch = self.only_touched && entry.changed && !self.closed.contains(path);
        self.open.contains(path) || by_switch
    }

    /// Whether `dir` is open and so is every folder above it.
    fn is_shown_open(&self, dir: &str) -> bool {
        let Some((parent, name)) = split(dir) else {
            return true;
        };
        let entry = self
            .levels
            .get(parent)
            .and_then(|level| level.entries.iter().find(|entry| entry.name == name));
        entry.is_some_and(|entry| self.is_open(dir, entry)) && self.is_shown_open(parent)
    }

    fn collect_unread(&self, dir: &str, unread: &mut Vec<String>) {
        for node in self.nodes_of(dir) {
            match (&node.children, node.is_open) {
                (Children::Unread, true) => unread.push(node.path),
                (Children::Read { .. }, true) => self.collect_unread(&node.path, unread),
                _ => {}
            }
        }
    }
}

/// `name` inside `dir`.
fn join(dir: &str, name: &str) -> String {
    match dir.is_empty() {
        true => name.to_string(),
        false => format!("{dir}/{name}"),
    }
}

/// The folder a path is in and its own name; `None` for the root.
fn split(path: &str) -> Option<(&str, &str)> {
    if path.is_empty() {
        return None;
    }
    Some(path.rsplit_once('/').unwrap_or(("", path)))
}
