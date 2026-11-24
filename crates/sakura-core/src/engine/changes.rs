//! What `sakura flow changes <task> --json` says of a lane: the files it changed since its base with their
//! counts, and the patch of one file. The patch and the paths are what an agent wrote: data, shown as they are.

use serde::Deserialize;

/// One file the lane changed.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ChangedFile {
    pub path: String,
    /// `modified`, `added`, `deleted` or `renamed`.
    #[serde(default)]
    pub status: String,
    /// Lines added and removed; none for a binary file.
    #[serde(default)]
    pub added: Option<u64>,
    #[serde(default)]
    pub removed: Option<u64>,
}

/// The files a lane changed against its base, uncommitted edits included.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
pub struct ChangesPage {
    /// Whether the lane's worktree was cleaned up: the list is then empty.
    #[serde(default)]
    pub gone: bool,
    #[serde(default)]
    pub files: Vec<ChangedFile>,
    /// The totals over the files.
    #[serde(default)]
    pub added: Option<u64>,
    #[serde(default)]
    pub removed: Option<u64>,
}

/// The patch of one file of a lane, cut by the engine when very long.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
pub struct FilePatch {
    #[serde(default)]
    pub patch: String,
    #[serde(default)]
    pub truncated: bool,
}
