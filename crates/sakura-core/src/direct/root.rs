//! Read task files relative to owned descriptors with `O_NOFOLLOW`.
//! Reopen the root for each operation and match its device and inode.
//! Relative links in task files must stay inside the root and away from its `.git`.
//! Refuse all links in git metadata.

use super::stat::{Identity, Stat};
use crate::error::{BridgeError, BridgeResult, ErrorCode};
use crate::files::{EntryKind, FileBody};
use crate::said::Said;
use rustix::fs::{self, Dir as RawDir, FileType, Mode, OFlags};
use std::ffi::CString;
use std::fs::File;
use std::io::Read;
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};

/// The most a file is read whole for the file view, as `box file` did.
pub const FILE_MAX_BYTES: u64 = 2 * 1024 * 1024;
/// The longest path a caller may name, in bytes.
pub const PATH_MAX_BYTES: usize = 4096;
/// The deepest path a caller may name, in components.
pub const DEPTH_MAX: usize = 64;
/// The longest name of one entry: the file system's own limit.
const NAME_MAX_BYTES: usize = 255;
/// The most links one walk goes through.
const LINKS_MAX: usize = 16;
/// The most entries read of one folder; a longer folder is said to be cut.
const LISTING_MAX: usize = 100_000;
/// The git metadata at the top of a task, never shown or read as a file of it.
const GIT_DIR: &str = ".git";

/// How a root treats a symbolic link met on a walk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Links {
    /// A relative link that stays inside is walked; the top `.git` is hidden.
    FollowInside,
    /// Any link ends the walk (git metadata).
    Refuse,
}

/// A folder opened for reading.
#[derive(Debug, Clone)]
pub struct Root {
    path: PathBuf,
    identity: Identity,
    links: Links,
}

/// One folder reached inside a root, open.
pub struct Dir {
    fd: OwnedFd,
    /// Whether this is the root itself, whose `.git` is hidden for a task.
    is_top: bool,
    links: Links,
}

/// The entries of one folder, by raw name, in no order.
pub struct Listing {
    pub entries: Vec<(String, EntryKind)>,
    pub truncated: bool,
}

impl Root {
    /// The files of a task at `path`: its last component must be a folder and not a link (the
    /// folders above it are the host's own).
    pub fn open(path: &Path) -> BridgeResult<Self> {
        Self::opened(path, None, Links::FollowInside)
    }

    /// As [`Root::open`], and the folder must be the one with `identity` (the one the service made).
    pub fn open_expecting(path: &Path, identity: Identity) -> BridgeResult<Self> {
        Self::opened(path, Some(identity), Links::FollowInside)
    }

    /// Git metadata at `path`: no link anywhere below it is walked, and nothing is hidden.
    pub fn metadata(path: &Path) -> BridgeResult<Self> {
        Self::opened(path, None, Links::Refuse)
    }

    /// Git metadata below an open root, at `name` (one component, never a link).
    pub fn metadata_below(&self, name: &str) -> BridgeResult<Self> {
        let top = self.top()?;
        let dir = top.child_any(name)?.ok_or_else(not_found)?;
        Ok(Self {
            path: self.path.join(name),
            identity: dir.stat()?.identity,
            links: Links::Refuse,
        })
    }

    /// The `.git` file at the top of a task on the Mac, which names its git directory; at most
    /// `limit` bytes, never through a link.
    pub fn git_pointer(&self, limit: u64) -> BridgeResult<Vec<u8>> {
        let (file, stat) = self.top()?.open_plain(GIT_DIR)?;
        if stat.size > limit {
            return Err(too_large());
        }
        let mut bytes = Vec::new();
        file.take(limit)
            .read_to_end(&mut bytes)
            .map_err(io_failure)?;
        Ok(bytes)
    }

    /// The device and inode of the folder.
    pub fn identity(&self) -> Identity {
        self.identity
    }

    /// The path the folder was opened at.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The body of the file at `path`: text, binary, or too large to be read whole.
    pub fn file(&self, path: &str) -> BridgeResult<FileBody> {
        let (dir, name) = self.walk_to_file(path)?;
        match dir.read(&name, FILE_MAX_BYTES)? {
            Some(bytes) => Ok(FileBody::of(bytes)),
            None => Ok(FileBody::TooLarge),
        }
    }

    /// The whole of the regular file at `path`, refused over `limit` bytes.
    pub fn read(&self, path: &str, limit: u64) -> BridgeResult<Vec<u8>> {
        let (dir, name) = self.walk_to_file(path)?;
        dir.read(&name, limit)?.ok_or_else(too_large)
    }

    /// The regular file at `path`, open, with its status: for reading parts of it.
    pub fn open_file(&self, path: &str) -> BridgeResult<(File, Stat)> {
        let (dir, name) = self.walk_to_file(path)?;
        dir.open_regular(&name)
    }

    /// The folder at `path` (empty for the root).
    pub fn dir(&self, path: &str) -> BridgeResult<Dir> {
        let (dir, _) = self.walk(path, false)?;
        Ok(dir)
    }

    /// The root folder itself, checked to be the one first opened.
    pub fn top(&self) -> BridgeResult<Dir> {
        let fd = open_last(&self.path)?;
        let dir = Dir {
            fd,
            is_top: true,
            links: self.links,
        };
        if dir.stat()?.identity != self.identity {
            return Err(replaced());
        }
        Ok(dir)
    }

    fn opened(path: &Path, expected: Option<Identity>, links: Links) -> BridgeResult<Self> {
        let fd = open_last(path)?;
        let identity = Stat::of_fd(&fd).map_err(io_failure)?.identity;
        if expected.is_some_and(|expected| expected != identity) {
            return Err(replaced());
        }
        Ok(Self {
            path: path.to_path_buf(),
            identity,
            links,
        })
    }

    fn walk_to_file(&self, path: &str) -> BridgeResult<(Dir, String)> {
        match self.walk(path, true)? {
            (dir, Some(name)) => Ok((dir, name)),
            (_, None) => Err(not_a_file(path)),
        }
    }

    /// Walks `path` from the root, one descriptor at a time. For a file, the folder that holds it and
    /// its name; for a folder, the folder.
    fn walk(&self, path: &str, is_file: bool) -> BridgeResult<(Dir, Option<String>)> {
        let mut pending: Vec<String> = parts(path)?.into_iter().rev().map(str::to_string).collect();
        let mut stack = vec![self.top()?];
        let mut links = 0;
        while let Some(name) = pending.pop() {
            match name.as_str() {
                "" | "." => continue,
                ".." if stack.len() == 1 => return Err(outside()),
                ".." => {
                    stack.pop();
                    continue;
                }
                _ => {}
            }
            let current = stack.last().expect("the root is never popped");
            if current.hides(&name) {
                return Err(outside());
            }
            let kind = current.stat_child(&name).map(|stat| stat.kind);
            match kind.ok_or_else(not_found)? {
                EntryKind::Link => {
                    let target = current.link_target(&name)?;
                    links += 1;
                    if self.links == Links::Refuse || target.starts_with('/') || links > LINKS_MAX {
                        return Err(outside());
                    }
                    pending.extend(target.split('/').rev().map(str::to_string));
                }
                EntryKind::Dir => {
                    let dir = current.child_any(&name)?.ok_or_else(outside)?;
                    stack.push(dir);
                }
                _ if is_file && pending.is_empty() => {
                    let dir = stack.pop().expect("the root is never popped");
                    return Ok((dir, Some(name)));
                }
                _ => return Err(not_found()),
            }
        }
        Ok((stack.pop().expect("the root is never popped"), None))
    }
}

impl Dir {
    /// The entries of this folder, without the top `.git` of a task.
    pub fn list(&self) -> BridgeResult<Listing> {
        let raw = read_dir(&self.fd).map_err(io_failure)?;
        let mut entries = Vec::with_capacity(raw.entries.len());
        for (name, hint) in raw.entries {
            let name = shown_name(name);
            if self.hides(&name) {
                continue;
            }
            let kind = match hint {
                Some(kind) => kind,
                None => self
                    .stat_child(&name)
                    .map_or(EntryKind::Other, |stat| stat.kind),
            };
            entries.push((name, kind));
        }
        Ok(Listing {
            entries,
            truncated: raw.truncated,
        })
    }

    /// What `name` is, without following it; nothing when it does not exist.
    pub fn stat_child(&self, name: &str) -> Option<Stat> {
        let name = component(name).ok()?;
        Stat::at(&self.fd, &name)
    }

    /// The folder `name` below this one, never through a link; nothing when it is not a folder.
    pub fn child(&self, name: &str) -> BridgeResult<Option<Dir>> {
        if self.hides(name) {
            return Ok(None);
        }
        match self.child_any(name) {
            Ok(dir) => Ok(dir),
            Err(error) if error.code == ErrorCode::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// The whole of the regular file `name`, never through a link; nothing when it is over `limit`.
    pub fn read(&self, name: &str, limit: u64) -> BridgeResult<Option<Vec<u8>>> {
        let (file, stat) = self.open_regular(name)?;
        let size = stat.size;
        if size > limit {
            return Ok(None);
        }
        let mut bytes = Vec::with_capacity(usize::try_from(size).unwrap_or(0));
        file.take(limit + 1)
            .read_to_end(&mut bytes)
            .map_err(io_failure)?;
        Ok((bytes.len() as u64 <= limit).then_some(bytes))
    }

    /// Where the link `name` points, as written.
    pub fn read_link(&self, name: &str) -> BridgeResult<Vec<u8>> {
        let name = component(name)?;
        let mut buffer = vec![0u8; PATH_MAX_BYTES + 1];
        let length = fs::readlinkat_raw(&self.fd, name.as_c_str(), buffer.as_mut_slice())
            .map_err(|_| not_found())?;
        if length > PATH_MAX_BYTES {
            return Err(outside());
        }
        buffer.truncate(length);
        Ok(buffer)
    }

    /// The device, inode, size and times of this folder.
    pub fn stat(&self) -> BridgeResult<Stat> {
        Stat::of_fd(&self.fd).map_err(io_failure)
    }

    fn hides(&self, name: &str) -> bool {
        self.is_top && self.links == Links::FollowInside && name == GIT_DIR
    }

    /// The folder `name`, or nothing when `name` is a link; an error when it is neither.
    fn child_any(&self, name: &str) -> BridgeResult<Option<Dir>> {
        match open_at(&self.fd, name, true) {
            Ok(fd) => Ok(Some(Dir {
                fd,
                is_top: false,
                links: self.links,
            })),
            Err(_)
                if self
                    .stat_child(name)
                    .is_some_and(|s| s.kind == EntryKind::Link) =>
            {
                Ok(None)
            }
            Err(_) => Err(not_found()),
        }
    }

    fn link_target(&self, name: &str) -> BridgeResult<String> {
        String::from_utf8(self.read_link(name)?).map_err(|_| outside())
    }

    /// The regular file `name`, open, with its status; never through a link, never waiting on a pipe.
    pub fn open_regular(&self, name: &str) -> BridgeResult<(File, Stat)> {
        if self.hides(name) {
            return Err(outside());
        }
        self.open_plain(name)
    }

    fn open_plain(&self, name: &str) -> BridgeResult<(File, Stat)> {
        let fd = open_at(&self.fd, name, false).map_err(|_| {
            match self.stat_child(name).map(|stat| stat.kind) {
                Some(EntryKind::Link) => outside(),
                _ => not_found(),
            }
        })?;
        let stat = Stat::of_fd(&fd).map_err(io_failure)?;
        if stat.kind != EntryKind::File {
            return Err(not_a_file(name));
        }
        Ok((File::from(fd), stat))
    }
}

/// The components of a path a caller gave: relative, without `..`, within the bounds.
fn parts(path: &str) -> BridgeResult<Vec<&str>> {
    let parts: Vec<&str> = path
        .split('/')
        .filter(|part| !matches!(*part, "" | "."))
        .collect();
    let is_plain = path.len() <= PATH_MAX_BYTES
        && parts.len() <= DEPTH_MAX
        && !path.starts_with('/')
        && !path.contains('\0')
        && parts
            .iter()
            .all(|part| *part != ".." && part.len() <= NAME_MAX_BYTES);
    match is_plain {
        true => Ok(parts),
        false => Err(BridgeError::invalid(
            Said::new("files.invalid.path").text("value", path),
        )),
    }
}

/// One entry's name as the system takes it: never empty, never a way out or back (`..`, `.`), never
/// more than one component.
fn component(name: &str) -> BridgeResult<CString> {
    let is_one =
        !matches!(name, "" | "." | "..") && name.len() <= NAME_MAX_BYTES && !name.contains('/');
    match is_one {
        true => CString::new(name).map_err(|_| outside()),
        false => Err(outside()),
    }
}

/// Opens the folder at the last component of `path` below its parent, never through a link.
fn open_last(path: &Path) -> BridgeResult<OwnedFd> {
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
        return Err(not_found());
    };
    let parent = fs::open(
        parent,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|_| not_found())?;
    let name = name.to_str().ok_or_else(not_found)?;
    open_at(&parent, name, true).map_err(|_| {
        let stat = component(name)
            .ok()
            .and_then(|name| Stat::at(&parent, &name));
        match stat.is_some_and(|stat| stat.kind == EntryKind::Link) {
            true => outside(),
            false => not_found(),
        }
    })
}

/// `openat` of one component, never following a link and never waiting on a pipe.
fn open_at(dir: &OwnedFd, name: &str, directory: bool) -> std::io::Result<OwnedFd> {
    let name =
        component(name).map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    let mut flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
    if directory {
        flags |= OFlags::DIRECTORY;
    }
    Ok(fs::openat(dir, name.as_c_str(), flags, Mode::empty())?)
}

struct RawListing {
    entries: Vec<(Vec<u8>, Option<EntryKind>)>,
    truncated: bool,
}

/// The names in an open folder with the kind the folder says, at most [`LISTING_MAX`].
fn read_dir(fd: &OwnedFd) -> std::io::Result<RawListing> {
    let mut listing = RawListing {
        entries: Vec::new(),
        truncated: false,
    };
    for entry in RawDir::read_from(fd)? {
        let entry = entry?;
        let name = entry.file_name().to_bytes();
        if name == b"." || name == b".." {
            continue;
        }
        if listing.entries.len() == LISTING_MAX {
            listing.truncated = true;
            break;
        }
        listing
            .entries
            .push((name.to_vec(), kind_hint(entry.file_type())));
    }
    Ok(listing)
}

/// A name as text: one that is not UTF-8 is shown with replacement marks, kept within the length a
/// name can have (it cannot be opened by that name, and is only shown).
fn shown_name(name: Vec<u8>) -> String {
    match String::from_utf8(name) {
        Ok(name) => name,
        Err(error) => {
            let mut shown = String::from_utf8_lossy(error.as_bytes()).into_owned();
            while shown.len() > NAME_MAX_BYTES {
                shown.pop();
            }
            shown
        }
    }
}

fn kind_hint(kind: FileType) -> Option<EntryKind> {
    match kind {
        FileType::RegularFile => Some(EntryKind::File),
        FileType::Directory => Some(EntryKind::Dir),
        FileType::Symlink => Some(EntryKind::Link),
        FileType::Unknown => None,
        _ => Some(EntryKind::Other),
    }
}

pub(crate) fn outside() -> BridgeError {
    BridgeError::invalid(Said::new("direct.outside"))
}

pub(crate) fn not_found() -> BridgeError {
    BridgeError::new(ErrorCode::NotFound, Said::new("direct.not_found"))
}

fn not_a_file(path: &str) -> BridgeError {
    BridgeError::new(ErrorCode::NotFound, Said::new("direct.not_a_file"))
        .with_details(Some(path.into()))
}

pub(crate) fn too_large() -> BridgeError {
    BridgeError::failed(Said::new("direct.too_large"))
}

fn replaced() -> BridgeError {
    BridgeError::new(ErrorCode::NotFound, Said::new("direct.replaced"))
}

pub(crate) fn io_failure(error: std::io::Error) -> BridgeError {
    BridgeError::failed(Said::new("direct.unreadable")).with_details(Some(error.to_string()))
}
