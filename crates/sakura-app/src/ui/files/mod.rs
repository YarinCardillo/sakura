//! The "Files" view of the right pane: the whole tree of a task, each level read when its folder is
//! opened, and under it the open file, read only, which follows the agent. Quick open (Cmd+P) lists the
//! task's files by name. Everything is read through `box` (`box tree`, `box file`, `box search`), behind
//! a [`Source`]; nothing is read on a timer.
//!
//! Entry point, one [`FilesPane`] per open task:
//! - [`FilesPane::new`] takes the task's [`Source`] ([`BoxSource`] over the app's `box`) and the folder
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

#[cfg(test)]
mod quick_open_tests;
#[cfg(test)]
mod tests;

pub use pane::{FilesEvent, FilesPane};
pub use quick_open::{QuickOpen, QuickOpenEvent, Target};
pub use source::{on_runtime, BoxSource, Reading, Source};
