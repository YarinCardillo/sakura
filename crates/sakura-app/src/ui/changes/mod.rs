//! The changes of a task: the review list beside the agent and in the Changes view, its rows, headers and
//! code lines, and the sentences it shows when there is nothing to list.

mod code_line;
#[cfg(feature = "boxes")]
mod fit;
#[cfg(feature = "boxes")]
mod header;
pub(in crate::ui) mod line;
#[cfg(feature = "boxes")]
pub mod list;
#[cfg(feature = "boxes")]
pub mod picker;
#[cfg(feature = "boxes")]
mod pointer;
#[cfg(feature = "boxes")]
pub mod rows;
#[cfg(feature = "boxes")]
mod selection;
#[cfg(feature = "boxes")]
mod shape_cache;
#[cfg(feature = "boxes")]
pub mod state;
#[cfg(feature = "boxes")]
mod wrap;
