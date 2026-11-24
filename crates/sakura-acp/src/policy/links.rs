//! Where a path inside the workspace really leads, on this machine's disk, for an agent on the Mac. There
//! the workspace is a folder of the person's, and the agent (or the repository it checked out) can put a
//! link in it that points anywhere: a path that is inside by its text can read or write outside through it,
//! and so can a file that is a second name (a hard link) of a file outside.
//!
//! The agent's adapter opens the file itself, after the approval, so this is a check made before the
//! open; the guarantee: an approval on the Mac rests on this check until the tool opens the path, which
//! holds because the agent cannot change the disk between the two by itself (its file tools write a
//! file's content and never make a link or a second name, a move and every command reach the person), and
//! a command the person lets run acts as them, with more than any check here could take away.

use super::Reason;
use std::fs::{self, OpenOptions};
use std::io::ErrorKind;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

/// Why `path` (inside `workspace` by its text) goes to the person, judged on disk: it leads out, or it is
/// a file with a second name.
pub(super) fn reason_to_ask(path: &Path, workspace: &Path) -> Option<Reason> {
    let Some(real) = real_inside(path, workspace) else {
        return Some(Reason::OutsideWorkspace);
    };
    file_reason(&real)
}

/// Why the file at `path` goes to the person: it has a second name, or it cannot be opened to tell.
fn file_reason(path: &Path) -> Option<Reason> {
    match has_other_names(path) {
        Ok(false) => None,
        Ok(true) => Some(Reason::LinkedFile),
        Err(_) => Some(Reason::OutsideWorkspace),
    }
}

/// Where `path` really is inside the workspace, relative to the workspace's real folder; nothing when it
/// leads out or cannot be judged.
pub(super) fn real_inside_relative(path: &Path, workspace: &Path) -> Option<PathBuf> {
    let real_workspace = fs::canonicalize(workspace).ok()?;
    let real = real_place(path)?;
    real.strip_prefix(&real_workspace)
        .ok()
        .map(Path::to_path_buf)
}

/// Resolve existing ancestors in filesystem order, so `link/../x` is relative to the link's target.
/// Refuse broken links, `..` after missing components, and paths outside the resolved workspace.
fn real_inside(path: &Path, workspace: &Path) -> Option<PathBuf> {
    let real_workspace = fs::canonicalize(workspace).ok()?;
    real_place(path).filter(|real| real.starts_with(&real_workspace))
}

/// Opens `path` to read, following no link in its last part and never waiting (a pipe opens at once).
pub(super) fn open_confined(path: &Path) -> std::io::Result<fs::File> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
}

/// Whether the file at `real` has more than one name on disk, by the inode its own open finds: a hard
/// link gives a file of the person's a second name inside the workspace, and nothing in the name says
/// so. The open follows no link and does not wait (a pipe); nothing there yet has no other name.
fn has_other_names(real: &Path) -> std::io::Result<bool> {
    let opened = open_confined(real);
    let file = match opened {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    let found = file.metadata()?;
    Ok(found.is_file() && found.nlink() > 1)
}

/// Where `path` is on disk: its deepest part that exists, resolved, with the parts that do not exist
/// yet after it. Nothing when a part is there but cannot be resolved (a link to nowhere, a folder that
/// cannot be read).
fn real_place(path: &Path) -> Option<PathBuf> {
    let mut missing = Vec::new();
    let mut at = path;
    loop {
        match fs::canonicalize(at) {
            Ok(real) => {
                return Some(
                    missing
                        .iter()
                        .rev()
                        .fold(real, |real, part| real.join(part)),
                )
            }
            Err(error) if error.kind() == ErrorKind::NotFound && !is_there(at) => {
                missing.push(at.file_name()?);
                at = at.parent()?;
            }
            Err(_) => return None,
        }
    }
}

/// Whether something, a link to nowhere included, has the name `path`.
fn is_there(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}
