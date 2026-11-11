//! Sakura desk core without any UI toolkit: a validated bridge to the `box` CLI, machine facts, the usage
//! reader, a file watcher and terminal sessions. Nothing here depends on Tauri, so a different shell can
//! reuse it unchanged. Events leave through plain callbacks.

pub mod box_cli;
pub mod box_words;
pub mod display;
pub mod error;
pub mod files;
pub mod graph;
pub mod jobs;
pub mod links;
pub mod machine;
pub mod model;
pub mod models;
pub mod naming;
pub mod notify;
pub mod patch;
pub mod platform;
pub mod play;
pub mod projects;
pub mod publish;
pub mod said;
pub mod service;
pub mod settings;
pub mod single_flight;
pub mod tabs;
pub mod terminal;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
pub mod usage;
pub mod validate;
pub mod watcher;
