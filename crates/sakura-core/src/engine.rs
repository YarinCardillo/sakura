//! The engine, as the app sees it: the `sakura` command's cockpit face and nothing else. The app reads
//! `sakura cockpit snapshot --json` and acts through `sakura cockpit act --json`; it decides no policy
//! and runs no Git.

pub mod accounts;
pub mod actor;
pub mod agents;
pub mod automations;
pub mod changes;
pub mod cli;
pub mod connectors;
pub mod feed;
pub mod leeway;
pub mod master;
pub mod moment;
pub mod phone;
pub mod presets;
pub mod project_folders;
pub mod resume_note;
pub mod rules;
pub mod scheduled;
pub mod setup;
pub mod snapshot;
pub mod state;
pub mod tmux;
pub mod tracks;
pub mod trust;
pub mod watch;

fn unreadable_answer() -> crate::error::BridgeError {
    crate::error::BridgeError::new(
        crate::error::ErrorCode::EngineFailed,
        crate::said::Said::new("error.engine.unreadable_answer"),
    )
}

#[cfg(test)]
mod tests;
