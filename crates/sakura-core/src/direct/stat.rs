//! What the reader needs to know of an entry without opening it: its kind, its mode, its size and the
//! times and identity the git index compares.

use crate::files::EntryKind;
use rustix::fs::{self, AtFlags, FileType};
use std::ffi::CStr;
use std::os::fd::OwnedFd;

/// Which file a folder is, by device and inode: two paths name the same folder when these agree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Identity {
    pub device: u64,
    pub inode: u64,
}

impl Identity {
    /// `"<device>:<inode>"`, as the service records a task's folder.
    pub fn parse(text: &str) -> Option<Self> {
        let (device, inode) = text.trim().split_once(':')?;
        Some(Self {
            device: device.parse().ok()?,
            inode: inode.parse().ok()?,
        })
    }
}

/// The status of one entry, as `lstat` gives it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stat {
    pub kind: EntryKind,
    /// The permission bits and the type, as the system gives them.
    pub mode: u32,
    pub size: u64,
    pub modified: (i64, i64),
    pub changed: (i64, i64),
    pub identity: Identity,
}

impl Stat {
    /// The status of an open descriptor.
    pub fn of_fd(fd: &OwnedFd) -> std::io::Result<Self> {
        Ok(Self::from_raw(&fs::fstat(fd)?))
    }

    /// The status of `name` below the folder `dir`, the link itself for a link; nothing when absent.
    pub fn at(dir: &OwnedFd, name: &CStr) -> Option<Self> {
        let raw = fs::statat(dir, name, AtFlags::SYMLINK_NOFOLLOW).ok()?;
        Some(Self::from_raw(&raw))
    }

    /// Whether the person may run it.
    pub fn is_executable(&self) -> bool {
        self.mode & 0o100 != 0
    }

    #[allow(clippy::unnecessary_cast, clippy::useless_conversion)]
    fn from_raw(raw: &fs::Stat) -> Self {
        let mode = u32::from(raw.st_mode);
        let kind = match FileType::from_raw_mode(raw.st_mode) {
            FileType::RegularFile => EntryKind::File,
            FileType::Directory => EntryKind::Dir,
            FileType::Symlink => EntryKind::Link,
            _ => EntryKind::Other,
        };
        Self {
            kind,
            mode,
            size: u64::try_from(raw.st_size).unwrap_or(0),
            modified: (raw.st_mtime as i64, raw.st_mtime_nsec as i64),
            changed: (raw.st_ctime as i64, raw.st_ctime_nsec as i64),
            identity: Identity {
                device: raw.st_dev as u64,
                inode: raw.st_ino as u64,
            },
        }
    }
}
