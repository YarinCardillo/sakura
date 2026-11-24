//! Whether a path names git's metadata. A write there runs nothing now, but git reads it at the person's
//! next command (`core.fsmonitor`, a hook, a pointer to a folder the agent wrote), so on the Mac it
//! reaches the person. Metadata is:
//!
//! - anything under a `.git` folder, and a worktree's `.git` pointer file;
//! - every write in or below a git dir without that name, or one that would make a folder one: git takes
//!   any folder holding HEAD beside objects, refs or a commondir for a repository, and one holding objects
//!   beside refs for the common folder of one;
//! - every write in or below the folder the workspace's `.git` pointer names, and the folder the
//!   `commondir` of that folder, or of a git dir the write is in, names. A pointer elsewhere in the
//!   workspace names a folder git only accepts in the shapes above, which are caught by their shape.
//!
//! A path is checked as the agent wrote it and where it really leads on disk, so a link to the metadata
//! is caught (a hard link to one of its files has a second name and is asked about anyway). Names are
//! compared as the Mac's disks fold them, failing closed: case is folded, the characters HFS+ ignores and
//! combining marks are dropped, and the letters that fold or look like ASCII ones (a Kelvin sign, a long
//! s, ligatures, dotted and dotless i) are read as those.

use super::links;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

/// The name of git's metadata in a working tree.
const GIT: &str = ".git";
/// Beside HEAD, any of these makes a folder a git dir.
const GIT_DIR_STORES: [&str; 3] = ["objects", "refs", "commondir"];
/// Together, these make a folder the common folder of a git dir.
const COMMON_DIR_PARTS: [&str; 2] = ["objects", "refs"];
/// The file of a git dir that names its common folder.
const COMMONDIR: &str = "commondir";
/// The prefix of a `.git` pointer file's line.
const POINTER_PREFIX: &str = "gitdir:";
/// The most of a `.git` pointer or a `commondir` file read.
const POINTER_MAX_BYTES: u64 = 4096;
/// The characters HFS+ ignores in a name, so that `.g\u{200c}it` is `.git` there: the list git checks
/// in `is_hfs_dotgit`.
const IGNORED_IN_NAMES: [char; 16] = [
    '\u{200c}', '\u{200d}', '\u{200e}', '\u{200f}', '\u{202a}', '\u{202b}', '\u{202c}', '\u{202d}',
    '\u{202e}', '\u{206a}', '\u{206b}', '\u{206c}', '\u{206d}', '\u{206e}', '\u{206f}', '\u{feff}',
];

/// Whether `path` (inside `workspace`) names git's metadata by its text or where it leads on disk.
pub(super) fn is_named(path: &Path, workspace: &Path) -> bool {
    let by_text = path.strip_prefix(workspace).unwrap_or(path).to_path_buf();
    let real = links::real_inside_relative(path, workspace);
    let is_metadata = |relative: &PathBuf| {
        has_git_part(relative)
            || writes_a_git_dir(relative, workspace)
            || is_in_named_folder(relative, workspace)
    };
    is_metadata(&by_text) || real.as_ref().is_some_and(is_metadata)
}

fn has_git_part(path: &Path) -> bool {
    names(path).iter().any(|name| is_name(name, GIT))
}

/// Whether a write of `relative` lands in or below a folder git reads as a git dir or a common folder,
/// or makes one.
fn writes_a_git_dir(relative: &Path, workspace: &Path) -> bool {
    let parts = names(relative);
    (0..parts.len()).any(|at| {
        let folder: PathBuf = workspace.join(parts[..at].iter().collect::<PathBuf>());
        would_be_git_dir(&folder, &parts[at])
    })
}

/// Whether `folder` is a git dir or a common folder, or will be once `entry` is written in it.
fn would_be_git_dir(folder: &Path, entry: &str) -> bool {
    let has = |name: &str| is_name(entry, name) || folder.join(name).symlink_metadata().is_ok();
    let is_git_dir = has("HEAD") && GIT_DIR_STORES.iter().any(|store| has(store));
    is_git_dir || COMMON_DIR_PARTS.iter().all(|part| has(part))
}

/// Whether `relative` is in a folder git reads as metadata by what a file names: the folder of the
/// workspace's `.git` pointer, and the common folder its `commondir` names or the one named by the
/// `commondir` of a git dir `relative` is in.
fn is_in_named_folder(relative: &Path, workspace: &Path) -> bool {
    let Ok(root) = std::fs::canonicalize(workspace) else {
        return false;
    };
    let real = root.join(relative);
    let pointed = named_folder(&root.join(GIT), POINTER_PREFIX);
    let ancestors = real.ancestors().filter(|folder| folder.starts_with(&root));
    let git_dirs = pointed.iter().map(PathBuf::as_path).chain(ancestors);
    let common = git_dirs.filter_map(|folder| named_folder(&folder.join(COMMONDIR), ""));
    let named: Vec<PathBuf> = pointed.iter().cloned().chain(common).collect();
    named.iter().any(|folder| real.starts_with(folder))
}

/// The real folder the file `file` names after `prefix` (a `.git` pointer's `gitdir:`, or nothing for a
/// `commondir`), relative to the file's folder; read with an open that follows no link and does not
/// wait, from a regular file of at most [`POINTER_MAX_BYTES`].
fn named_folder(file: &Path, prefix: &str) -> Option<PathBuf> {
    let opened = links::open_confined(file).ok()?;
    if !opened.metadata().ok()?.is_file() {
        return None;
    }
    let mut text = String::new();
    opened
        .take(POINTER_MAX_BYTES)
        .read_to_string(&mut text)
        .ok()?;
    let target = text.trim().strip_prefix(prefix)?.trim();
    std::fs::canonicalize(file.parent()?.join(target)).ok()
}

/// The plain parts of `path`, as text.
fn names(path: &Path) -> Vec<String> {
    let parts = path.components().filter_map(|part| match part {
        Component::Normal(name) => Some(name.to_string_lossy().into_owned()),
        _ => None,
    });
    parts.collect()
}

/// Whether the name `name` is `wanted` as the Mac's disks and git compare names, failing closed.
fn is_name(name: &str, wanted: &str) -> bool {
    folded(name) == wanted.to_ascii_lowercase()
}

/// `name` folded for comparison: lower case, without the characters HFS+ ignores or combining marks,
/// and with the letters that fold or look like ASCII ones read as those.
fn folded(name: &str) -> String {
    let mut text = String::with_capacity(name.len());
    for c in name.chars() {
        let is_dropped = IGNORED_IN_NAMES.contains(&c) || COMBINING_MARKS.contains(&c);
        if is_dropped {
            continue;
        }
        match ASCII_FOLDS.iter().find(|(from, _)| *from == c) {
            Some((_, to)) => text.push_str(to),
            None => text.extend(c.to_lowercase()),
        }
    }
    text
}

/// The combining marks, which a folded name drops.
const COMBINING_MARKS: std::ops::RangeInclusive<char> = '\u{300}'..='\u{36f}';
/// The letters that fold to ASCII ones, or are read as them: a Kelvin sign, a long s, sharp s, dotted
/// and dotless i, and the Latin ligatures.
const ASCII_FOLDS: [(char, &str); 12] = [
    ('\u{212a}', "k"),
    ('\u{17f}', "s"),
    ('\u{df}', "ss"),
    ('\u{1e9e}', "ss"),
    ('\u{130}', "i"),
    ('\u{131}', "i"),
    ('\u{fb00}', "ff"),
    ('\u{fb01}', "fi"),
    ('\u{fb02}', "fl"),
    ('\u{fb03}', "ffi"),
    ('\u{fb04}', "ffl"),
    ('\u{fb05}', "st"),
];
