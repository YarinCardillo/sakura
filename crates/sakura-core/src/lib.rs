//! Sakura core without any UI toolkit: the engine's cockpit runner and its snapshot, the tab strip's model
//! and what the person is told while the app is behind. With the `boxes` feature it also holds the
//! bridge to the `box` CLI, machine facts, a file watcher and terminal sessions. Events
//! leave through plain callbacks.

#[cfg(feature = "boxes")]
pub mod box_cli;
#[cfg(feature = "boxes")]
pub mod box_words;
pub mod direct;
pub mod display;
pub mod drafts;
pub mod engine;
pub mod error;
pub mod files;
pub mod graph;
#[cfg(feature = "boxes")]
pub mod jobs;
pub mod links;
#[cfg(feature = "boxes")]
pub mod machine;
pub mod model;
pub mod models;
pub mod naming;
pub mod notify;
pub mod patch;
pub mod platform;
#[cfg(feature = "boxes")]
pub mod play;
pub mod process;
#[cfg(feature = "boxes")]
pub mod projects;
#[cfg(feature = "boxes")]
pub mod publish;
pub mod said;
#[cfg(feature = "boxes")]
pub mod service;
#[cfg(feature = "boxes")]
pub mod settings;
pub mod single_flight;
pub mod tabs;
#[cfg(feature = "boxes")]
pub mod terminal;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
pub mod validate;
#[cfg(feature = "boxes")]
pub mod watcher;
