//! Git read in-process from the repository's own files, with no git started and nothing the repository
//! configures ever run: no hooks, no filters, no textconv, no fsmonitor, no pager, no alternates. A
//! library was weighed first: `gix` reads by path and so would follow a link the agent planted in its
//! `.git`, its status runs the clean filters `.gitattributes` names, and its object store resolves
//! alternates. Reading the few formats needed (loose and packed objects, refs, the index) through the
//! confined root is smaller than wrapping it, and every byte goes through the same walk as a file.

mod history;
mod index;
mod objects;
mod oid;
mod parse;
mod repo;
mod trees;

pub use history::{ahead, latest, merge_base};
pub(crate) use index::unsupported;
pub use index::{Index, IndexEntry};
pub use oid::{Oid, EMPTY_TREE};
pub use parse::{Commit, Mode, TreeEntry};
pub use repo::Repo;
pub use trees::{changed, files, Leaf};

#[cfg(test)]
pub(crate) use objects::apply_delta_for_test;
