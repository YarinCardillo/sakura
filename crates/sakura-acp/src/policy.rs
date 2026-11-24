//! The confinement policy for permission requests: which ones Sakura answers by itself and which reach the
//! person. A request and the workspace in, a decision out; the one thing read besides is where a path
//! inside the workspace leads on disk (`links`).
//!
//! An agent in a box runs in a container that is the real boundary (read-only root, no host mounts, a proxy
//! allow list). There this policy is a second layer: it decides when to interrupt the person, and it errs
//! toward asking. What the container can already do is allowed once, without the person: reading,
//! searching and editing files inside the workspace, thinking, switching to plan mode, and one simple
//! command of a short list of known programs with arguments that stay inside the workspace (`command`).
//! Everything else goes to the person: fetches, any other command, paths outside the workspace, tools of
//! unknown kind (MCP servers arrive that way), other mode switches, and every request that has no
//! single-use "allow" to choose.
//!
//! An agent on the Mac ([`Runs::OnMac`]) runs as the person, and no sandbox stands around its commands:
//! what a known program does there depends on the workspace (a package script, a Makefile, a test suite,
//! a path through a link), so every command it asks for reaches the person, and so does a move, which
//! can change where other paths lead. Its other file tools keep the rules above, judged on disk
//! (`links`), and what cannot be judged there goes to the person.
//!
//! The decision reads the call's `kind`, its `locations`, its content and its raw input, whole: a target
//! the raw input names is judged with the locations, and a field the policy has no rule for (a working
//! directory, an environment, a flag it does not know) sends the request to the person. It never reads a
//! title or an option's name, and it never chooses `allow_always`: a remembered permission widens every
//! later automatic approval, so only the person can give one.
//!
//! Paths are compared as text, then, on the Mac, by where they really lead on its disk: a path through a
//! link inside the workspace that points outside it is outside, and so is one that cannot be resolved. In
//! a box, whose disk is not this one, they are compared as text only, and so are a command's arguments.

mod command;
pub mod decider;
mod git_metadata;
mod lexer;
mod links;
pub(crate) mod master_command;
mod subject;

pub use subject::Subject;

use crate::model::{OptionKind, PermissionOption, ToolCallFields, ToolKind};
use crate::paths::{self, Place};
use std::path::Path;

/// Where the agent runs, which decides what an approval without the person can rest on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Runs {
    /// In its box, which is the boundary: paths and commands are judged by their text.
    InBox,
    /// On the Mac, as the person, with nothing else between it and their files.
    OnMac,
}

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
    /// The call runs a command on the Mac, outside any sandbox.
    CommandOnMac,
    /// The call moves files on the Mac, which can change where other paths lead after they were judged.
    MoveOnMac,
    /// The call touches a file on the Mac that has another name on disk (a hard link), which can be a
    /// file outside the workspace.
    LinkedFile,
    /// The call writes git's metadata on the Mac (its config, a hook, a worktree's `.git` pointer), which
    /// can make a later git command of the person's run something.
    GitMetadata,
    /// The engine, which decides for its master, has no rule for the call.
    EngineAsks,
}

/// Decides on the request for `tool` (the call's known fields, with the request's own on top) given the
/// agent's `options`, for an agent that `runs` with its workspace at `workspace` (an absolute path as the
/// agent sees it).
pub fn decide(
    tool: &ToolCallFields,
    options: &[PermissionOption],
    workspace: &Path,
    runs: Runs,
) -> Decision {
    decide_subject(&Subject::of(tool), options, workspace, runs)
}

/// Decides on a request from what the policy read of it. The decision depends on nothing else, so the
/// subject the person is shown is the whole of what was judged.
pub fn decide_subject(
    subject: &Subject,
    options: &[PermissionOption],
    workspace: &Path,
    runs: Runs,
) -> Decision {
    let allow_once = options
        .iter()
        .find(|option| option.kind == OptionKind::AllowOnce);
    let Some(allow_once) = allow_once else {
        return Decision::Ask(Reason::NoSingleUseOption);
    };
    match reason_to_ask(subject, workspace, runs) {
        Some(reason) => Decision::Ask(reason),
        None => Decision::Allow {
            option_id: allow_once.id.clone(),
        },
    }
}

/// Why the request goes to the person: what its kind and its targets say first, then any field of its raw
/// input that the policy does not read. Only a request it reads whole is answered without the person.
fn reason_to_ask(subject: &Subject, workspace: &Path, runs: Runs) -> Option<Reason> {
    let has_unread = !subject.unread.is_empty() || subject.is_unread_cut;
    reason_by_kind(subject, workspace, runs).or(has_unread.then_some(Reason::UnknownField))
}

fn reason_by_kind(subject: &Subject, workspace: &Path, runs: Runs) -> Option<Reason> {
    let Some(kind) = subject.kind else {
        return Some(Reason::UnknownKind);
    };
    let outside = || outside_workspace(&subject.paths, workspace, runs);
    match kind {
        ToolKind::Move if runs == Runs::OnMac => Some(Reason::MoveOnMac),
        ToolKind::Read | ToolKind::Edit | ToolKind::Delete | ToolKind::Move => {
            match subject.paths.is_empty() {
                true => Some(Reason::UnknownTarget),
                false => outside().or_else(|| git_metadata_reason(kind, subject, workspace, runs)),
            }
        }
        ToolKind::Search | ToolKind::Think => outside(),
        ToolKind::Execute => outside()
            .or_else(|| command_reason(subject, workspace))
            .or((runs == Runs::OnMac).then_some(Reason::CommandOnMac)),
        ToolKind::SwitchMode => (!subject.is_plan_mode()).then_some(Reason::ModeChange),
        ToolKind::Fetch => Some(Reason::Network),
        ToolKind::Other => Some(Reason::UnknownKind),
    }
}

/// Why one of `paths` goes to the person: it is outside the workspace by its text or, on the Mac, on
/// disk, or it names a file of more than one name there. Paths outside by their text are said first.
fn outside_workspace(paths: &[String], workspace: &Path, runs: Runs) -> Option<Reason> {
    let mut inside = Vec::with_capacity(paths.len());
    for path in paths {
        match paths::place(path, workspace) {
            Place::Inside(_) => inside.push(path),
            Place::Outside(_) => return Some(Reason::OutsideWorkspace),
        }
    }
    if runs == Runs::InBox {
        return None;
    }
    let mut on_disk = inside
        .into_iter()
        .filter_map(|path| links::reason_to_ask(&workspace.join(path), workspace));
    on_disk.next()
}

/// Why a write on the Mac goes to the person by where it writes: git's metadata.
fn git_metadata_reason(
    kind: ToolKind,
    subject: &Subject,
    workspace: &Path,
    runs: Runs,
) -> Option<Reason> {
    let is_write = matches!(kind, ToolKind::Edit | ToolKind::Delete | ToolKind::Move);
    let is_metadata = |path: &String| git_metadata::is_named(&workspace.join(path), workspace);
    let is_asked = runs == Runs::OnMac && is_write && subject.paths.iter().any(is_metadata);
    is_asked.then_some(Reason::GitMetadata)
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
