//! One repository opened for reading: where its git directory and its common directory are, its refs
//! and its objects. Three kinds are opened, each from facts the service recorded, never by looking for
//! a `.git` upwards:
//! - a task in a box: the `.git` folder at the top of its workspace, written by the agent. It must be
//!   a folder (a `.git` file pointing elsewhere is refused) and nothing it names outside is followed.
//! - a task on the Mac: the git directory the service recorded when it made the worktree, which the
//!   worktree's `.git` file must still name exactly, as the service checked (`checked_lavoro`).
//! - the person's repository of a project, for the base branch the Git view draws.

use super::index::{self, Index};
use super::objects::{corrupt, Kind, Object, Store};
use super::oid::Oid;
use super::parse::{self, Commit, TreeEntry};
use crate::direct::root::{not_found, outside, Root};
use crate::direct::Stat;
use crate::error::{BridgeResult, ErrorCode};
use std::path::Path;

/// The largest commit or tag read.
const COMMIT_MAX_BYTES: usize = 1024 * 1024;
/// The largest tree read.
const TREE_MAX_BYTES: usize = 16 * 1024 * 1024;
/// The largest index read.
const INDEX_MAX_BYTES: u64 = 256 * 1024 * 1024;
/// The largest `packed-refs` read.
const PACKED_REFS_MAX_BYTES: u64 = 32 * 1024 * 1024;
/// The largest loose ref or pointer file read.
const REF_MAX_BYTES: u64 = 4096;
/// The most symbolic refs followed from one name.
const SYMBOLIC_MAX: usize = 5;

/// A repository opened for reading.
pub struct Repo {
    /// Where `HEAD` and the index are.
    git: Root,
    /// Where the refs and the objects are.
    common: Root,
    store: Store,
}

impl Repo {
    /// The repository of a task in a box: the `.git` folder of its workspace.
    pub fn of_box(workspace: &Root) -> BridgeResult<Self> {
        let git = workspace
            .metadata_below(".git")
            .map_err(|_| corrupt("the workspace has no .git folder"))?;
        Self::at(git.clone(), git)
    }

    /// The repository of a task on the Mac: `recorded` is the git directory the service made it with.
    pub fn of_lavoro(worktree: &Root, recorded: &Path) -> BridgeResult<Self> {
        let pointer = worktree.git_pointer(REF_MAX_BYTES)?;
        let expected = format!("gitdir: {}", recorded.display());
        let line = std::str::from_utf8(&pointer).map_err(|_| outside())?;
        if line.strip_suffix('\n').unwrap_or(line) != expected {
            return Err(corrupt(
                "the worktree's .git does not name the git directory the service recorded",
            ));
        }
        let worktrees = recorded
            .parent()
            .filter(|parent| parent.file_name().is_some_and(|name| name == "worktrees"));
        let common = worktrees
            .and_then(Path::parent)
            .ok_or_else(|| corrupt("the recorded git directory is not a worktree's"))?;
        let git = Root::metadata(recorded)?;
        let common = Root::metadata(common)?;
        let named =
            String::from_utf8(git.read("commondir", REF_MAX_BYTES)?).map_err(|_| outside())?;
        let named = Root::metadata(&lexical(&recorded.join(named.trim_end_matches('\n'))))?;
        if named.identity() != common.identity() {
            return Err(corrupt(
                "the worktree's git directory belongs to another repository",
            ));
        }
        Self::at(git, common)
    }

    /// The person's repository at `folder`, whose `.git` must be a folder.
    pub fn of_repository(folder: &Path) -> BridgeResult<Self> {
        let git = Root::metadata(&folder.join(".git"))
            .map_err(|_| index::unsupported("the repository's .git is not a folder"))?;
        Self::at(git.clone(), git)
    }

    fn at(git: Root, common: Root) -> BridgeResult<Self> {
        let store = Store::open(common.metadata_below("objects")?)?;
        Ok(Self { git, common, store })
    }

    /// The commit `HEAD` names; nothing on a branch with no commit yet.
    pub fn head(&self) -> BridgeResult<Option<Oid>> {
        self.resolve_in(&self.git, "HEAD", 0)
    }

    /// The branch `HEAD` is on (`main` for `refs/heads/main`); nothing when it is detached.
    pub fn head_branch(&self) -> BridgeResult<Option<String>> {
        let text = self.ref_file(&self.git, "HEAD")?.ok_or_else(not_found)?;
        Ok(text.strip_prefix("ref: refs/heads/").map(str::to_string))
    }

    /// The commit the ref `name` (`refs/heads/main`) names; nothing when there is no such ref.
    pub fn resolve(&self, name: &str) -> BridgeResult<Option<Oid>> {
        self.resolve_in(&self.common, name, 0)
    }

    /// The commit `oid`; an annotated tag is followed to its commit.
    pub fn commit(&self, oid: Oid) -> BridgeResult<Commit> {
        let object = self.object(oid, COMMIT_MAX_BYTES)?;
        match object.kind {
            Kind::Commit => parse::commit(&object.data),
            Kind::Tag => self.commit(tag_target(&object.data)?),
            _ => Err(corrupt(
                "an id that should name a commit names another object",
            )),
        }
    }

    /// The entries of the tree `oid`.
    pub fn tree(&self, oid: Oid) -> BridgeResult<Vec<TreeEntry>> {
        if oid == super::oid::EMPTY_TREE {
            return Ok(Vec::new());
        }
        let object = self.object(oid, TREE_MAX_BYTES)?;
        match object.kind {
            Kind::Tree => parse::tree(&object.data),
            _ => Err(corrupt(
                "an id that should name a tree names another object",
            )),
        }
    }

    /// The content of the blob `oid`; nothing when it is over `limit` bytes.
    pub fn blob(&self, oid: Oid, limit: usize) -> BridgeResult<Option<std::sync::Arc<Vec<u8>>>> {
        match self.store.read(oid, limit) {
            Ok(Some(object)) if object.kind == Kind::Blob => Ok(Some(object.data)),
            Ok(Some(_)) => Err(corrupt(
                "an id that should name a file names another object",
            )),
            Ok(None) => Err(corrupt("a file's object is missing")),
            Err(error) if is_too_large(&error) => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// The repository's own excludes (`info/exclude`), when it has them.
    pub fn exclude(&self, limit: u64) -> Option<Vec<u8>> {
        self.common.read("info/exclude", limit).ok()
    }

    /// The index and the status of its file; an empty index when there is none.
    pub fn index(&self) -> BridgeResult<(Index, Option<Stat>)> {
        let (file, stat) = match self.git.open_file("index") {
            Ok(opened) => opened,
            Err(error) if error.code == ErrorCode::NotFound => return Ok((Index::default(), None)),
            Err(error) => return Err(error),
        };
        if stat.size > INDEX_MAX_BYTES {
            return Err(corrupt("the index is too large"));
        }
        let mut data = Vec::with_capacity(stat.size as usize);
        use std::io::Read;
        file.take(INDEX_MAX_BYTES)
            .read_to_end(&mut data)
            .map_err(crate::direct::root::io_failure)?;
        Ok((index::parse(&data)?, Some(stat)))
    }

    fn object(&self, oid: Oid, limit: usize) -> BridgeResult<Object> {
        self.store
            .read(oid, limit)?
            .ok_or_else(|| corrupt("an object is missing"))
    }

    fn resolve_in(&self, root: &Root, name: &str, depth: usize) -> BridgeResult<Option<Oid>> {
        if depth > SYMBOLIC_MAX {
            return Err(corrupt("symbolic refs point at each other"));
        }
        let text = match self.ref_file(root, name)? {
            Some(text) => text,
            None => return self.packed(name),
        };
        match text.strip_prefix("ref: ") {
            Some(target) if is_ref_name(target) => self.resolve_in(&self.common, target, depth + 1),
            Some(_) => Err(corrupt("a symbolic ref names something that is not a ref")),
            None => Oid::from_hex(&text)
                .map(Some)
                .ok_or_else(|| corrupt("a ref holds no id")),
        }
    }

    /// The text of a loose ref, its line ending removed; nothing when there is none.
    fn ref_file(&self, root: &Root, name: &str) -> BridgeResult<Option<String>> {
        if name != "HEAD" && !is_ref_name(name) {
            return Err(corrupt("not a ref name"));
        }
        match root.read(name, REF_MAX_BYTES) {
            Ok(bytes) => {
                let text = String::from_utf8(bytes).map_err(|_| corrupt("a ref is not text"))?;
                Ok(Some(text.trim_end().to_string()))
            }
            Err(error) if error.code == ErrorCode::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn packed(&self, name: &str) -> BridgeResult<Option<Oid>> {
        let bytes = match self.common.read("packed-refs", PACKED_REFS_MAX_BYTES) {
            Ok(bytes) => bytes,
            Err(error) if error.code == ErrorCode::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let text = String::from_utf8_lossy(&bytes);
        let found = text
            .lines()
            .filter(|line| !line.starts_with(['#', '^']))
            .filter_map(|line| line.split_once(' '))
            .find(|(_, ref_name)| *ref_name == name);
        Ok(found.and_then(|(hex, _)| Oid::from_hex(hex)))
    }
}

/// `path` with its `.` and `..` components resolved by name: for paths the service recorded.
fn lexical(path: &Path) -> std::path::PathBuf {
    let mut out = std::path::PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// Whether `name` is a ref this reader follows: under `refs/`, plain components only.
fn is_ref_name(name: &str) -> bool {
    let is_plain = |part: &str| {
        !part.is_empty()
            && !part.starts_with('.')
            && !part.ends_with(".lock")
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '+' | '@'))
    };
    name.len() <= 255
        && name.starts_with("refs/")
        && !name.contains("..")
        && name.split('/').all(is_plain)
}

fn tag_target(data: &[u8]) -> BridgeResult<Oid> {
    let text = String::from_utf8_lossy(data);
    let hex = text
        .lines()
        .next()
        .and_then(|line| line.strip_prefix("object "));
    hex.and_then(Oid::from_hex)
        .ok_or_else(|| corrupt("a tag names no object"))
}

pub(crate) fn is_too_large(error: &crate::error::BridgeError) -> bool {
    error.said.key.0 == "direct.too_large"
}
