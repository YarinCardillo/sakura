//! The "Files" view of the right pane: the whole tree of a task, each level read when its folder is
//! opened, and under it the open file, read only, which follows the agent. Quick open (Cmd+P) lists the
//! task's files by name. Everything is read behind a [`Source`]: in-process ([`DirectSource`]), and
//! through `box` (`box tree`, `box file`, `box search`) only for a repository the in-process reader does
//! not read; nothing is read on a timer.
//!
//! Entry point, one [`FilesPane`] per open task:
//! - [`FilesPane::new`] takes the task's [`Source`] ([`DirectSource`] over the app's readers) and the folder
//!   its agent works in (the conversation's workspace), which the agent's places are relative to.
//! - The pane asks it to show a place with [`FilesPane::show_location`], for each tool call of the
//!   conversation that names one (the protocol's `locations`). While following is on the file opens at
//!   the line; a place outside the task is refused.
//! - [`FilesPane::task_changed`] reads again what is on screen, when the watcher says the task changed;
//!   [`FilesPane::set_patch`] hands over the task's patch (the one "Changes" already holds) for the
//!   figures of the gutter. [`FilesPane::open`] opens a file the person picked (quick open).
//! - It emits [`FilesEvent`]: following turned on or off (the status bar shows it and its control
//!   calls [`FilesPane::set_following`], which turns it off, or on again at the agent's last place), and "Ask the agent" with the lines selected, for the composer to take.
//!
//! [`QuickOpen`] is one per window, drawn last in the window's root: [`quick_open::install`] binds
//! Cmd+P for the task on screen, and its [`QuickOpenEvent::Picked`] names the task it was opened for
//! with the file, so the file is opened in that task's pane and in no other.

mod file_view;
mod follow;
mod line_index;
mod pane;
pub mod quick_open;
mod source;
mod syntax;
mod tree_rows;

#[cfg(all(test, feature = "boxes"))]
mod quick_open_tests;
#[cfg(all(test, feature = "boxes"))]
mod tests;

#[cfg(not(feature = "boxes"))]
pub use file_view::{FileView, FileViewEvent};
#[cfg(not(feature = "boxes"))]
pub use follow::{Follow, Location};
pub use pane::{FilesEvent, FilesPane};
pub use quick_open::{QuickOpen, QuickOpenEvent, Target};
pub use source::{joined, on_runtime, Reading, Source};
#[cfg(feature = "boxes")]
pub use source::{BoxSource, DirectSource};
