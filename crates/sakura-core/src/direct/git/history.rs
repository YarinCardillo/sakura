//! Walks of the commit graph, newest first as `git log` lists them: the commits a branch has that
//! another has not, the commit two branches meet at, and the latest commits of a branch. Every walk
//! reads at most [`VISITS_MAX`] commits, however the agent shaped its history.

use super::oid::Oid;
use super::parse::Commit;
use super::repo::Repo;
use crate::error::BridgeResult;
use std::collections::{BinaryHeap, HashMap};

/// The most commits one walk reads.
pub const VISITS_MAX: usize = 50_000;

const FROM_HEAD: u8 = 1;
const FROM_BASE: u8 = 2;

/// The commits reachable from `head` and not from `base`, newest first, at most `limit`.
pub fn ahead(
    repo: &Repo,
    base: Option<Oid>,
    head: Oid,
    limit: usize,
) -> BridgeResult<Vec<(Oid, Commit)>> {
    let mut walk = Walk::new(repo);
    walk.push(head, FROM_HEAD)?;
    if let Some(base) = base {
        walk.push(base, FROM_BASE)?;
    }
    let mut found = Vec::new();
    while found.len() < limit && walk.has_interesting() {
        let Some((oid, flags, commit)) = walk.pop()? else {
            break;
        };
        let carried = if flags & FROM_BASE != 0 {
            FROM_BASE
        } else {
            FROM_HEAD
        };
        for parent in commit.parents.clone() {
            walk.push(parent, carried)?;
        }
        if flags & FROM_BASE == 0 {
            found.push((oid, commit));
        }
    }
    Ok(found)
}

/// The newest commit both `one` and `other` reach; nothing when they share none within the walk.
pub fn merge_base(repo: &Repo, one: Oid, other: Oid) -> BridgeResult<Option<Oid>> {
    if one == other {
        return Ok(Some(one));
    }
    let mut walk = Walk::new(repo);
    walk.push(one, FROM_HEAD)?;
    walk.push(other, FROM_BASE)?;
    while let Some((oid, flags, commit)) = walk.pop()? {
        if flags & (FROM_HEAD | FROM_BASE) == FROM_HEAD | FROM_BASE {
            return Ok(Some(oid));
        }
        for parent in commit.parents.clone() {
            walk.push(parent, flags)?;
        }
    }
    Ok(None)
}

/// The latest `limit` commits reachable from `tip`, newest first.
pub fn latest(repo: &Repo, tip: Oid, limit: usize) -> BridgeResult<Vec<(Oid, Commit)>> {
    ahead(repo, None, tip, limit)
}

/// A walk by commit time: each commit read once, its flags the union of the sides that reach it.
struct Walk<'r> {
    repo: &'r Repo,
    queue: BinaryHeap<(i64, u64, Oid)>,
    flags: HashMap<Oid, u8>,
    commits: HashMap<Oid, Commit>,
    done: std::collections::HashSet<Oid>,
    order: u64,
}

impl<'r> Walk<'r> {
    fn new(repo: &'r Repo) -> Self {
        Self {
            repo,
            queue: BinaryHeap::new(),
            flags: HashMap::new(),
            commits: HashMap::new(),
            done: std::collections::HashSet::new(),
            order: u64::MAX,
        }
    }

    fn push(&mut self, oid: Oid, flags: u8) -> BridgeResult<()> {
        let known = self.flags.entry(oid).or_insert(0);
        let is_new = *known == 0;
        *known |= flags;
        if !is_new || self.done.contains(&oid) || self.commits.len() >= VISITS_MAX {
            return Ok(());
        }
        let commit = self.repo.commit(oid)?;
        self.queue.push((commit.time, self.order, oid));
        self.order -= 1;
        self.commits.insert(oid, commit);
        Ok(())
    }

    fn pop(&mut self) -> BridgeResult<Option<(Oid, u8, Commit)>> {
        let Some((_, _, oid)) = self.queue.pop() else {
            return Ok(None);
        };
        self.done.insert(oid);
        let flags = self.flags.get(&oid).copied().unwrap_or_default();
        let commit = self
            .commits
            .get(&oid)
            .cloned()
            .expect("a queued commit was read");
        Ok(Some((oid, flags, commit)))
    }

    /// Whether anything still queued is reached from the head side only.
    fn has_interesting(&self) -> bool {
        self.queue.iter().any(|(_, _, oid)| {
            self.flags
                .get(oid)
                .is_some_and(|flags| flags & FROM_BASE == 0)
        })
    }
}
