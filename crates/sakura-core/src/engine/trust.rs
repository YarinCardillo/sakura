//! Whether the app may run a program as the engine. The engine is started outside every cage, so what it is
//! must not be something another account, or a process of another user, can replace: the program's real
//! path (every link resolved) is owned by the person who runs the app, and neither it nor any folder above
//! it can be written by others. The group may write only when it is `admin` (gid 80 on a Mac, where it holds
//! the person and root, and Homebrew's folders are group-writable by it), has no nested groups, every name in
//! its member list resolves, and no account but the person and root is in it by member list or by primary
//! group; every lookup that fails refuses. Every folder above is owned by the person or by root, since its
//! owner can replace its entries whatever its mode says; a folder that others can write is allowed only when
//! it is sticky and the entry inside it is the person's own.

use nix::unistd::{Gid, Group, User};
use std::collections::HashMap;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const GROUP_WRITE: u32 = 0o020;
const OTHERS_WRITE: u32 = 0o002;
/// The password file is walked with a call that keeps its place in the process: one walker at a time.
static PASSWD_WALK: Mutex<()> = Mutex::new(());
/// Only the person of an entry can remove or rename it.
const STICKY: u32 = 0o1000;
/// The one group whose write permission is trusted, by name and by number.
const TRUSTED_GROUP: &str = "admin";
const TRUSTED_GID: u32 = 80;
/// The program that reads the directory service, and what it says of a group that has no nested groups.
const DSCL: &str = "/usr/bin/dscl";
const NO_NESTED_GROUPS: &str = "No such key";
/// The owners of a folder above a program that are trusted besides the person: root.
const ROOT: u32 = 0;

/// Why a program is not trusted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Untrusted {
    /// The real path cannot be resolved or read.
    Unreadable(PathBuf),
    /// The real path is owned by someone else.
    NotOwned(PathBuf),
    /// The real path, or a folder above it (`.1`), can be written by others, or by a group with other accounts
    /// in it.
    Writable(PathBuf, PathBuf),
    /// A folder above the real path (`.1`) is owned by an account that is neither the person nor root.
    FolderOwned(PathBuf, PathBuf),
}

/// What the check asks of the system about a group. Every answer that cannot be had is a refusal.
pub trait Groups {
    /// The group's name; none when the system does not know it.
    fn name(&self, gid: u32) -> Option<String>;
    /// Whether the group is the one whose write permission is trusted.
    fn is_trusted(&self, gid: u32, name: &str) -> bool {
        gid == TRUSTED_GID && name == TRUSTED_GROUP
    }
    /// The user ids of the accounts in the group: those its member list names and those whose primary group
    /// it is. None when any part of it cannot be read: a name that does not resolve, an error walking the
    /// accounts.
    fn members(&self, gid: u32) -> Option<Vec<u32>>;
    /// Whether the group has nested groups, whose members the member list does not show. A lookup that fails
    /// says yes.
    fn has_nested(&self, name: &str) -> bool;
}

/// The system's own answers.
#[derive(Default)]
pub(super) struct System {
    groups: std::cell::RefCell<HashMap<u32, Option<Group>>>,
}

impl System {
    fn lookup<T>(&self, gid: u32, select: impl FnOnce(&Group) -> T) -> Option<T> {
        self.groups
            .borrow_mut()
            .entry(gid)
            .or_insert_with(|| Group::from_gid(Gid::from_raw(gid)).ok().flatten())
            .as_ref()
            .map(select)
    }
}

impl Groups for System {
    fn name(&self, gid: u32) -> Option<String> {
        self.lookup(gid, |group| group.name.clone())
    }

    fn members(&self, gid: u32) -> Option<Vec<u32>> {
        let names = self.lookup(gid, |group| group.mem.clone())?;
        let mut uids = Vec::new();
        for name in &names {
            uids.push(User::from_name(name).ok().flatten()?.uid.as_raw());
        }
        uids.extend(primary_members(gid)?);
        Some(uids)
    }

    fn has_nested(&self, name: &str) -> bool {
        let read = std::process::Command::new(DSCL)
            .args([".", "-read", &format!("/Groups/{name}"), "NestedGroups"])
            .output();
        match read {
            Ok(done) => {
                let said = String::from_utf8_lossy(&done.stdout).to_string()
                    + &String::from_utf8_lossy(&done.stderr);
                !said.contains(NO_NESTED_GROUPS)
            }
            Err(_) => true,
        }
    }
}

/// The real path of `program` when it is trusted for the user `owner`, else why not.
pub fn check(program: &Path, owner: u32) -> Result<PathBuf, Untrusted> {
    check_with(program, owner, &[ROOT])
}

/// Like [`check`], with `others` as the owners of the folders above that are trusted besides `owner`.
pub fn check_with(program: &Path, owner: u32, others: &[u32]) -> Result<PathBuf, Untrusted> {
    check_with_groups(program, owner, others, &System::default())
}

/// Like [`check_with`], with `groups` answering what the system says of a group.
pub fn check_with_groups(
    program: &Path,
    owner: u32,
    others: &[u32],
    groups: &dyn Groups,
) -> Result<PathBuf, Untrusted> {
    let real = program
        .canonicalize()
        .map_err(|_| Untrusted::Unreadable(program.to_path_buf()))?;
    let own = std::fs::metadata(&real).map_err(|_| Untrusted::Unreadable(real.clone()))?;
    if own.uid() != owner {
        return Err(Untrusted::NotOwned(real));
    }
    let mut group_trust = HashMap::new();
    let mut is_group_ours = |meta: &std::fs::Metadata| {
        *group_trust
            .entry(meta.gid())
            .or_insert_with(|| is_trusted_group(groups, meta.gid(), owner, others))
    };
    if own.mode() & OTHERS_WRITE != 0 || (own.mode() & GROUP_WRITE != 0 && !is_group_ours(&own)) {
        return Err(Untrusted::Writable(real.clone(), real));
    }
    let mut entry = real.clone();
    for folder in real.ancestors().skip(1) {
        let meta =
            std::fs::metadata(folder).map_err(|_| Untrusted::Unreadable(folder.to_path_buf()))?;
        let is_trusted_owner = meta.uid() == owner || others.contains(&meta.uid());
        if !is_trusted_owner {
            return Err(Untrusted::FolderOwned(real.clone(), folder.to_path_buf()));
        }
        // A sticky folder lets nobody but the person of an entry remove or replace it, whoever else can write.
        let is_protected = is_own_in_sticky(&meta, &entry, owner);
        let others_write = meta.mode() & OTHERS_WRITE != 0;
        let group_writes = meta.mode() & GROUP_WRITE != 0 && !is_group_ours(&meta);
        if !is_protected && (others_write || group_writes) {
            return Err(Untrusted::Writable(real.clone(), folder.to_path_buf()));
        }
        entry = folder.to_path_buf();
    }
    Ok(real)
}

/// Whether a group that may write is one the person can trust: `admin`, with no nested groups, whose every
/// member can be named, and in which there is no account but the person and the `others` (root).
fn is_trusted_group(groups: &dyn Groups, gid: u32, owner: u32, others: &[u32]) -> bool {
    let Some(name) = groups.name(gid) else {
        return false;
    };
    if !groups.is_trusted(gid, &name) || groups.has_nested(&name) {
        return false;
    }
    let Some(members) = groups.members(gid) else {
        return false;
    };
    members
        .iter()
        .all(|uid| *uid == owner || others.contains(uid))
}

/// Whether a folder is sticky and holds `entry`, which `owner` owns: nobody else can
/// remove or rename it.
fn is_own_in_sticky(folder: &std::fs::Metadata, entry: &Path, owner: u32) -> bool {
    let is_own = std::fs::metadata(entry).is_ok_and(|meta| meta.uid() == owner);
    folder.mode() & STICKY != 0 && is_own
}

/// The user ids of the accounts whose primary group is `gid`; none when walking the accounts ended in an
/// error and not at their end.
fn primary_members(gid: u32) -> Option<Vec<u32>> {
    let _walking = PASSWD_WALK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut uids = Vec::new();
    // SAFETY: The lock serializes passwd enumeration. Each record is copied before getpwent
    // can replace it, and endpwent closes the enumeration before releasing the lock.
    unsafe {
        libc::setpwent();
        let is_whole = loop {
            errno::set_errno(errno::Errno(0));
            let entry = libc::getpwent();
            if entry.is_null() {
                break errno::errno().0 == 0;
            }
            if (*entry).pw_gid == gid {
                uids.push((*entry).pw_uid);
            }
        };
        libc::endpwent();
        is_whole.then_some(uids)
    }
}

/// The user the app runs as.
pub fn person() -> u32 {
    rustix::process::geteuid().as_raw()
}
