//! Trees as lists of files: every file of a commit's tree by path, and the files two trees hold
//! differently, without reading the subtrees they share.

use super::objects::corrupt;
use super::oid::Oid;
use super::parse::{Mode, TreeEntry};
use super::repo::Repo;
use crate::error::BridgeResult;
use std::collections::BTreeMap;

/// The most files one tree is read with.
pub const FILES_MAX: usize = 500_000;
/// The deepest folder read.
const DEPTH_MAX: usize = 64;

/// A file of a tree: its mode and its blob (or a submodule's commit).
pub type Leaf = (Mode, Oid);

/// The files two trees hold differently, by path: the leaf on each side, if any.
pub type Changes = BTreeMap<String, (Option<Leaf>, Option<Leaf>)>;

/// Every file of the tree `oid`, by path.
pub fn files(repo: &Repo, oid: Oid) -> BridgeResult<BTreeMap<String, Leaf>> {
    let mut out = BTreeMap::new();
    collect(repo, oid, "", 0, &mut out)?;
    Ok(out)
}

/// The files whose leaf differs between the trees `old` and `new` (`None` for an empty side), by path:
/// the leaf on each side, nothing on the side that does not have the file.
pub fn changed(repo: &Repo, old: Option<Oid>, new: Option<Oid>) -> BridgeResult<Changes> {
    let mut out = BTreeMap::new();
    compare(repo, old, new, "", 0, &mut out)?;
    Ok(out)
}

fn collect(
    repo: &Repo,
    oid: Oid,
    prefix: &str,
    depth: usize,
    out: &mut BTreeMap<String, Leaf>,
) -> BridgeResult<()> {
    if depth > DEPTH_MAX {
        return Err(corrupt("a tree is too deep"));
    }
    for entry in repo.tree(oid)? {
        let path = format!("{prefix}{}", entry.name);
        match entry.mode {
            Mode::Tree => collect(repo, entry.oid, &format!("{path}/"), depth + 1, out)?,
            mode => {
                if out.len() == FILES_MAX {
                    return Err(corrupt("a tree has too many files"));
                }
                out.insert(path, (mode, entry.oid));
            }
        }
    }
    Ok(())
}

fn compare(
    repo: &Repo,
    old: Option<Oid>,
    new: Option<Oid>,
    prefix: &str,
    depth: usize,
    out: &mut Changes,
) -> BridgeResult<()> {
    if old == new {
        return Ok(());
    }
    if depth > DEPTH_MAX || out.len() >= FILES_MAX {
        return Err(corrupt("a tree is too deep or too large"));
    }
    let entries = |oid: Option<Oid>| -> BridgeResult<BTreeMap<String, TreeEntry>> {
        let listed = match oid {
            Some(oid) => repo.tree(oid)?,
            None => Vec::new(),
        };
        Ok(listed
            .into_iter()
            .map(|entry| (entry.name.clone(), entry))
            .collect())
    };
    let (old, new) = (entries(old)?, entries(new)?);
    let names: std::collections::BTreeSet<&String> = old.keys().chain(new.keys()).collect();
    for name in names {
        let path = format!("{prefix}{name}");
        let (before, after) = (old.get(name), new.get(name));
        let subtree =
            |entry: Option<&TreeEntry>| entry.filter(|e| e.mode == Mode::Tree).map(|e| e.oid);
        let leaf = |entry: Option<&TreeEntry>| {
            entry
                .filter(|e| e.mode != Mode::Tree)
                .map(|e| (e.mode, e.oid))
        };
        if subtree(before).is_some() || subtree(after).is_some() {
            compare(
                repo,
                subtree(before),
                subtree(after),
                &format!("{path}/"),
                depth + 1,
                out,
            )?;
        }
        let (before, after) = (leaf(before), leaf(after));
        if before != after {
            out.insert(path, (before, after));
        }
    }
    Ok(())
}
