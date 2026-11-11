//! The changes of a task: the review list beside the agent and in the Changes view, its rows, headers and
//! code lines, and the sentences it shows when there is nothing to list.

mod code_line;
mod fit;
mod header;
pub(in crate::ui) mod line;
pub mod list;
pub mod picker;
mod pointer;
pub mod rows;
mod selection;
mod shape_cache;
pub mod state;
mod wrap;
