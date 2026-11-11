//! The files of a lavoro, read through `box`: one directory level (`box tree`), the bytes of one file
//! (`box file`) and file names for quick open (`box search`). The shapes are the ones `box` documents in
//! its `docs/interface.md` ("Reading"). Everything read here was written by an agent: it is shown, never
//! run, and every value that reaches an argument vector is checked first.

mod marks;
mod reference;
mod tree;

pub use marks::{gutter_marks, GutterMark, MarkKind};
pub use reference::{workspace_relative, Reference};
pub use tree::{Children, Node, Tree};

use crate::box_cli::{BoxCli, Limit};
use crate::error::{BridgeError, BridgeResult, ErrorCode};
use crate::said::Said;
use crate::validate;
use serde::Deserialize;

/// The longest text `box search` takes, in characters.
const QUERY_MAX_CHARS: usize = 200;
/// What `box file` writes when a file is over its limit (exit 82); the line is kept as the error's details.
const TOO_LARGE_MARK: &str = "larger than 2 MiB";
/// How much of a file is looked at to tell text from binary.
const SNIFF_BYTES: usize = 8 * 1024;
/// The most entries `box tree` lists for one level.
const LEVEL_MAX_ENTRIES: usize = 2000;
/// The deepest folder the tree reads, in folders under the root.
const TREE_MAX_DEPTH: usize = 64;
/// The longest name an entry can have, in bytes: the file system's own limit.
const NAME_MAX_BYTES: usize = 255;
/// The most added or removed lines an entry is said to have: more than any file a tree shows, and
/// small enough that two of them add up within the number they are kept in.
const COUNT_MAX: u32 = 1_000_000_000;

/// What an entry of a directory is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EntryKind {
    File,
    Dir,
    /// A symbolic link, never followed by the tree.
    Link,
    /// A file of the base that is gone from this level.
    Deleted,
    /// Anything else (a socket, a device), and any kind a newer `box` may add.
    #[serde(other)]
    Other,
}

/// One entry of a directory level.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Entry {
    pub name: String,
    pub kind: EntryKind,
    /// Whether it differs from the base; for a folder, whether anything below it does.
    #[serde(default)]
    pub changed: bool,
    /// Added and removed lines, for files `box` could count.
    #[serde(default)]
    pub added: Option<u32>,
    #[serde(default)]
    pub removed: Option<u32>,
}

/// One directory level of a lavoro.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Level {
    /// The directory, relative to the lavoro; empty for its root.
    pub path: String,
    pub entries: Vec<Entry>,
    /// Whether `box` left entries out because the level is too long.
    #[serde(default)]
    pub truncated: bool,
}

impl Level {
    /// The level when it is the one requested (`asked`, empty for the root) and can be shown whole: no
    /// more entries than `box` lists, each with a name of its own that is one path component and
    /// counts of lines a file can have, no deeper than the tree goes. Anything else is refused whole: a level is never shown in part.
    pub fn checked(self, asked: &str) -> BridgeResult<Self> {
        let depth = asked.split('/').filter(|part| !part.is_empty()).count();
        let mut names = std::collections::HashSet::new();
        let has_own_names = self
            .entries
            .iter()
            .all(|entry| is_component(&entry.name) && names.insert(entry.name.as_str()));
        let counts = self
            .entries
            .iter()
            .flat_map(|entry| [entry.added, entry.removed]);
        let has_sane_counts = counts.flatten().all(|count| count <= COUNT_MAX);
        let is_whole = self.path == asked
            && depth <= TREE_MAX_DEPTH
            && self.entries.len() <= LEVEL_MAX_ENTRIES
            && has_own_names
            && has_sane_counts;
        match is_whole {
            true => Ok(self),
            false => Err(not_a_level()),
        }
    }
}

/// Whether `name` is the name of one entry of a folder: not empty, not a way out or back, without a
/// separator, no longer than a file system allows.
fn is_component(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= NAME_MAX_BYTES
        && !matches!(name, "." | "..")
        && !name.contains(['/', '\0'])
}

/// What is said of a level that cannot be shown.
pub(crate) fn not_a_level() -> BridgeError {
    BridgeError::failed(Said::new("files.invalid.level"))
}

/// File names matching a text, best first.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Matches {
    pub query: String,
    pub matches: Vec<String>,
    #[serde(default)]
    pub truncated: bool,
}

/// What a file holds, as the file view shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileBody {
    Text(String),
    /// Not text: its bytes are not shown.
    Binary,
    /// Over the limit of `box file`: it was not read.
    TooLarge,
}

impl FileBody {
    /// The body of `bytes`: text when they are UTF-8 with no NUL near the start, else binary.
    pub fn of(bytes: Vec<u8>) -> Self {
        let sniffed = &bytes[..bytes.len().min(SNIFF_BYTES)];
        if sniffed.contains(&0) {
            return FileBody::Binary;
        }
        String::from_utf8(bytes).map_or(FileBody::Binary, FileBody::Text)
    }
}

/// A path inside a lavoro as `box` takes it: relative, without `..`, never read as a flag. Empty is the
/// root, which only a directory may be.
pub fn relative_path(value: &str) -> BridgeResult<&str> {
    let climbs = value.split('/').any(|part| part == "..");
    let is_plain = !value.starts_with(['/', '-']) && !value.contains('\0') && !climbs;
    if is_plain {
        Ok(value)
    } else {
        Err(not_a_path(value))
    }
}

/// The text of a search: 1 to 200 characters, never read as a flag.
pub fn query(value: &str) -> BridgeResult<&str> {
    let length = value.chars().count();
    let is_plain =
        (1..=QUERY_MAX_CHARS).contains(&length) && !value.starts_with('-') && !value.contains('\0');
    if is_plain {
        Ok(value)
    } else {
        Err(BridgeError::invalid(Said::new("files.invalid.query")))
    }
}

/// `box tree <task> [--path <dir>] --json`: the level of `dir` (empty for the root).
pub async fn read_level(cli: &BoxCli, task: &str, dir: &str) -> BridgeResult<Level> {
    let (task, dir) = (validate::task(task)?, relative_path(dir)?);
    let mut args = vec!["tree", task];
    if !dir.is_empty() {
        args.extend(["--path", dir]);
    }
    args.push("--json");
    let level: Level = cli.run_json(&args, Limit::Read).await?;
    level.checked(dir)
}

/// `box file <task> <path>`: the body of one file. A file over the limit is an answer, not a failure.
pub async fn read_file(cli: &BoxCli, task: &str, path: &str) -> BridgeResult<FileBody> {
    let (task, path) = (validate::task(task)?, relative_path(path)?);
    if path.is_empty() {
        return Err(not_a_path(path));
    }
    match cli.run_bytes(&["file", task, path], Limit::Read).await {
        Ok(bytes) => Ok(FileBody::of(bytes)),
        Err(error) if is_too_large(&error) => Ok(FileBody::TooLarge),
        Err(error) => Err(error),
    }
}

/// `box search <task> <text> --json`: the files whose path holds `text`.
pub async fn search(cli: &BoxCli, task: &str, text: &str) -> BridgeResult<Matches> {
    let (task, text) = (validate::task(task)?, query(text)?);
    cli.run_json(&["search", task, text, "--json"], Limit::Read)
        .await
}

fn not_a_path(value: &str) -> BridgeError {
    BridgeError::invalid(Said::new("files.invalid.path").text("value", value))
}

/// Whether `error` is `box file` refusing a file over its limit.
fn is_too_large(error: &BridgeError) -> bool {
    let line = error.details.as_deref().unwrap_or_default();
    error.code == ErrorCode::BoxFailed && line.contains(TOO_LARGE_MARK)
}

#[cfg(test)]
mod tests;
