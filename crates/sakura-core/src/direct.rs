//! Sakura's own reader of a task's files, git history and conversations: in-process, with no `box`, no
//! shell and no git started for a read. A task in a box is a folder of this Mac written by an agent that
//! is not trusted, so everything here holds against that agent: no symbolic link is followed out of
//! the task's folder, every read has a size limit, and nothing the workspace configures (hooks, filters,
//! alternates, a `.git` that points elsewhere) is ever run or followed.
//!
//! Entry point: [`Readers`], one per data folder of the service, which opens a [`TaskReader`] per task
//! and draws the graph of a project.

#[cfg(feature = "boxes")]
pub mod git;
#[cfg(feature = "boxes")]
mod ignores;
#[cfg(feature = "boxes")]
mod patch_text;
#[cfg(feature = "boxes")]
mod place;
#[cfg(feature = "boxes")]
mod readers;
mod root;
mod stat;
#[cfg(feature = "boxes")]
mod status;
#[cfg(feature = "boxes")]
mod task;
#[cfg(feature = "boxes")]
mod time;

#[cfg(feature = "boxes")]
pub use place::{ProjectRecord, TaskRecord};
#[cfg(feature = "boxes")]
pub use readers::Readers;
pub use root::{Dir, Listing, Root, FILE_MAX_BYTES};
pub use stat::{Identity, Stat};
#[cfg(feature = "boxes")]
pub use task::TaskReader;

#[cfg(all(feature = "boxes", test))]
mod git_tests;
#[cfg(test)]
mod tests;
