//! The confinement policy for permission requests: which ones Sakura answers by itself and which reach the
//! person. Pure: a request and the workspace in, a decision out.
//!
//! The agent runs in a container that is the real boundary (read-only root, no host mounts, a proxy allow
//! list). This policy is a second layer: it decides when to interrupt the person, and it errs toward asking.
//! What the container can already do is allowed once, without the person: reading, searching and editing
//! files inside the workspace, thinking, switching to plan mode, and one simple command of a short list of
//! known programs with arguments that stay inside the workspace (`command`). Everything else goes to the
//! person: fetches, any other command, paths outside the workspace, tools of unknown kind (MCP servers
//! arrive that way), other mode switches, and every request that has no single-use "allow" to choose.
//!
//! The decision reads the call's `kind`, its `locations`, its content and its raw input, whole: a target
//! the raw input names is judged with the locations, and a field the policy has no rule for (a working
//! directory, an environment, a flag it does not know) sends the request to the person. It never reads a
//! title or an option's name, and it never chooses `allow_always`: a remembered permission widens every
//! later automatic approval, so only the person can give one.
//!
//! Paths are compared as text: a symbolic link inside the workspace that points outside it is not seen.

mod command;
mod lexer;
mod subject;

pub use subject::Subject;

use crate::model::{OptionKind, PermissionOption, ToolCallFields, ToolKind};
use crate::paths;
use std::path::Path;

/// What to do with a permission request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Answer with this option, once, without asking the person.
    Allow { option_id: String },
    /// Show the request to the person, with why.
    Ask(Reason),
}

/// Why a request reaches the person.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// The agent offers no "allow once": every allow would be remembered.
    NoSingleUseOption,
    /// The call's kind is unknown or absent, so what it reaches is unknown.
    UnknownKind,
    /// The call names no file or command to check.
    UnknownTarget,
    /// The command is not one the policy can read with certainty as a single known program.
    UnknownCommand,
    /// The call reaches the network or installs something.
    Network,
    /// The call touches a path outside the workspace.
    OutsideWorkspace,
    /// The call changes the agent's mode to something other than plan.
    ModeChange,
    /// The call's raw input has a field the policy has no rule for (a working directory, an environment,
    /// anything it does not know), so what the tool does with it is unknown.
    UnknownField,
}

/// Decides on the request for `tool` (the call's known fields, with the request's own on top) given the
/// agent's `options`, for an agent whose workspace is `workspace` (an absolute path inside the container).
pub fn decide(tool: &ToolCallFields, options: &[PermissionOption], workspace: &Path) -> Decision {
    decide_subject(&Subject::of(tool), options, workspace)
}

/// Decides on a request from what the policy read of it. The decision depends on nothing else, so the
/// subject the person is shown is the whole of what was judged.
pub fn decide_subject(
    subject: &Subject,
    options: &[PermissionOption],
    workspace: &Path,
) -> Decision {
    let allow_once = options
        .iter()
        .find(|option| option.kind == OptionKind::AllowOnce);
    let Some(allow_once) = allow_once else {
        return Decision::Ask(Reason::NoSingleUseOption);
    };
    match reason_to_ask(subject, workspace) {
        Some(reason) => Decision::Ask(reason),
        None => Decision::Allow {
            option_id: allow_once.id.clone(),
        },
    }
}

/// Why the request goes to the person: what its kind and its targets say first, then any field of its raw
/// input that the policy does not read. Only a request it reads whole is answered without the person.
fn reason_to_ask(subject: &Subject, workspace: &Path) -> Option<Reason> {
    let has_unread = !subject.unread.is_empty() || subject.is_unread_cut;
    reason_by_kind(subject, workspace).or(has_unread.then_some(Reason::UnknownField))
}

fn reason_by_kind(subject: &Subject, workspace: &Path) -> Option<Reason> {
    let Some(kind) = subject.kind else {
        return Some(Reason::UnknownKind);
    };
    let outside = || outside_workspace(&subject.paths, workspace);
    match kind {
        ToolKind::Read | ToolKind::Edit | ToolKind::Delete | ToolKind::Move => {
            match subject.paths.is_empty() {
                true => Some(Reason::UnknownTarget),
                false => outside(),
            }
        }
        ToolKind::Search | ToolKind::Think => outside(),
        ToolKind::Execute => outside().or_else(|| command_reason(subject, workspace)),
        ToolKind::SwitchMode => (!subject.is_plan_mode()).then_some(Reason::ModeChange),
        ToolKind::Fetch => Some(Reason::Network),
        ToolKind::Other => Some(Reason::UnknownKind),
    }
}

fn outside_workspace(paths: &[String], workspace: &Path) -> Option<Reason> {
    let is_outside = paths.iter().any(|path| !is_inside(path, workspace));
    is_outside.then_some(Reason::OutsideWorkspace)
}

/// Whether `path` stays inside `workspace`, by the one reading the window also shows paths with.
pub(crate) fn is_inside(path: &str, workspace: &Path) -> bool {
    paths::place(path, workspace).is_inside()
}

fn command_reason(subject: &Subject, workspace: &Path) -> Option<Reason> {
    match subject.command.as_deref() {
        Some(text) => command::reason_to_ask(text, workspace),
        None => Some(Reason::UnknownTarget),
    }
}

#[cfg(test)]
mod tests;
