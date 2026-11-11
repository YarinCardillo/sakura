//! What `box` says when it fails, as something to say to the person. `box` ends a failure with one
//! English line, most often starting with `box:`. The lines Sakura knows are named by a key of the window's
//! locale files (the window has one plain sentence for each, in every language), any other by a general
//! key, and the line itself is kept apart as the details, shown only where the person asks for them.
//! This is the one place these lines are read: the bridge, the terminals and the login all tell a failure
//! of `box` through it. Sakura's own reasons for a creation that failed are lines too, with their own
//! prefix, so that a job's last line is always data.

use crate::said::{Key, Said};

/// How `box` starts every line it prints about its own failures.
pub const BOX_PREFIX: &str = "box: ";
/// How Sakura starts a line it writes itself about a creation that failed.
pub const SAKURA_PREFIX: &str = "sakura: ";
/// The lines Sakura writes: `box new` ran past its limit (followed by the seconds), `box` was not found,
/// `box` could not be started or waited for (followed by the system's words).
pub const SAKURA_TIMED_OUT: &str = "box new timed out after ";
pub const SAKURA_BOX_MISSING: &str = "box not found";
pub const SAKURA_BOX_UNUSABLE: &str = "box could not be run: ";
/// What is said of a failure whose line Sakura does not know.
pub const UNKNOWN: Key = Key("box.failure.unknown");

/// A failure of `box` that Sakura knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Known {
    NoConnection,
    SignInNotCompleted,
    StillNotLoggedIn,
    LoginStatusUnreadable,
    AccountNotLoggedIn,
    NoSuchTask,
    TaskExists,
    AgentAlreadyRunning,
    AgentInTerminal,
    ConversationOpen,
    NoAgent,
    ProxyDown,
    WorkspaceUnreadable,
    ChangesUnreadable,
    CommitsUnreadable,
    UncommittedFiles,
    CommitsNotTaken,
    NothingToTake,
    BundleNotVerified,
    DependenciesFailed,
    RunnerStopped,
    RunnerAlreadyRunning,
    VerbNotInProfile,
    UnknownProfile,
    InvalidName,
    NotAFolder,
    NotARepository,
    BareRepository,
    InsideBoxData,
    NotOwned,
    BorrowsObjects,
    HasLinks,
    ConfigIncludes,
    RepositoryRefused,
    UnknownProject,
    NoSuchBranch,
    BranchExists,
    NoAccount,
    NodeMissing,
    AdapterNotInstalled,
}

/// What tells each known failure in the line `box` wrote, most specific first.
const MARKS: [(&str, Known); 49] = [
    ("is not a folder", Known::NotAFolder),
    ("is not inside a git repository", Known::NotARepository),
    ("is a bare repository", Known::BareRepository),
    ("borrows git objects", Known::BorrowsObjects),
    ("symbolic link", Known::HasLinks),
    ("includes other files", Known::ConfigIncludes),
    ("inside the data directory of box", Known::InsideBoxData),
    ("leads into the data directory of box", Known::InsideBoxData),
    ("not owned by you", Known::NotOwned),
    ("cannot be used:", Known::RepositoryRefused),
    ("is refused:", Known::RepositoryRefused),
    ("unknown project", Known::UnknownProject),
    ("invalid project name", Known::InvalidName),
    ("is not a branch of the repository", Known::NoSuchBranch),
    ("already exists in", Known::BranchExists),
    ("name the account of the conversation", Known::NoAccount),
    ("is not an account of the mac", Known::NoAccount),
    ("node and npm are needed", Known::NodeMissing),
    (
        "could not install the claude acp adapter",
        Known::AdapterNotInstalled,
    ),
    ("no connection", Known::NoConnection),
    ("was not completed", Known::SignInNotCompleted),
    ("still not logged in", Known::StillNotLoggedIn),
    (
        "could not read the login status",
        Known::LoginStatusUnreadable,
    ),
    ("has no login yet", Known::AccountNotLoggedIn),
    ("does not exist", Known::NoSuchTask),
    ("already exists", Known::TaskExists),
    ("an agent is already running", Known::AgentAlreadyRunning),
    ("an agent runs in the terminal", Known::AgentInTerminal),
    ("a conversation is open", Known::ConversationOpen),
    ("no agent is running", Known::NoAgent),
    ("is the proxy running", Known::ProxyDown),
    ("could not read the workspace", Known::WorkspaceUnreadable),
    ("could not produce the diff", Known::ChangesUnreadable),
    ("could not read the commits", Known::CommitsUnreadable),
    ("uncommitted file", Known::UncommittedFiles),
    ("commits that were not taken", Known::CommitsNotTaken),
    ("has no commits to take", Known::NothingToTake),
    ("did not verify", Known::BundleNotVerified),
    ("could not bundle", Known::BundleNotVerified),
    ("dependency install failed", Known::DependenciesFailed),
    ("no runner is running", Known::RunnerStopped),
    ("the runner did not start", Known::RunnerStopped),
    ("runner is already running", Known::RunnerAlreadyRunning),
    ("runner is running", Known::RunnerAlreadyRunning),
    ("is not in the profile", Known::VerbNotInProfile),
    ("unknown profile", Known::UnknownProfile),
    ("invalid task name", Known::InvalidName),
    ("invalid account name", Known::InvalidName),
    ("invalid profile name", Known::InvalidName),
];

impl Known {
    /// The key of the sentence for the person.
    pub fn key(self) -> Key {
        Key(match self {
            Known::NoConnection => "box.failure.no_connection",
            Known::SignInNotCompleted => "box.failure.sign_in_not_completed",
            Known::StillNotLoggedIn => "box.failure.still_not_logged_in",
            Known::LoginStatusUnreadable => "box.failure.login_status_unreadable",
            Known::AccountNotLoggedIn => "box.failure.account_not_logged_in",
            Known::NoSuchTask => "box.failure.no_such_task",
            Known::TaskExists => "box.failure.task_exists",
            Known::AgentAlreadyRunning => "box.failure.agent_already_running",
            Known::AgentInTerminal => "box.failure.agent_in_terminal",
            Known::ConversationOpen => "box.failure.conversation_open",
            Known::NoAgent => "box.failure.no_agent",
            Known::ProxyDown => "box.failure.proxy_down",
            Known::WorkspaceUnreadable => "box.failure.workspace_unreadable",
            Known::ChangesUnreadable => "box.failure.changes_unreadable",
            Known::CommitsUnreadable => "box.failure.commits_unreadable",
            Known::UncommittedFiles => "box.failure.uncommitted_files",
            Known::CommitsNotTaken => "box.failure.commits_not_taken",
            Known::NothingToTake => "box.failure.nothing_to_take",
            Known::BundleNotVerified => "box.failure.bundle_not_verified",
            Known::DependenciesFailed => "box.failure.dependencies_failed",
            Known::RunnerStopped => "box.failure.runner_stopped",
            Known::RunnerAlreadyRunning => "box.failure.runner_already_running",
            Known::VerbNotInProfile => "box.failure.verb_not_in_profile",
            Known::UnknownProfile => "box.failure.unknown_profile",
            Known::InvalidName => "box.failure.invalid_name",
            Known::NotAFolder => "box.failure.not_a_folder",
            Known::NotARepository => "box.failure.not_a_repository",
            Known::BareRepository => "box.failure.bare_repository",
            Known::InsideBoxData => "box.failure.inside_box_data",
            Known::NotOwned => "box.failure.not_owned",
            Known::BorrowsObjects => "box.failure.borrows_objects",
            Known::HasLinks => "box.failure.has_links",
            Known::ConfigIncludes => "box.failure.config_includes",
            Known::RepositoryRefused => "box.failure.repository_refused",
            Known::UnknownProject => "box.failure.unknown_project",
            Known::NoSuchBranch => "box.failure.no_such_branch",
            Known::BranchExists => "box.failure.branch_exists",
            Known::NoAccount => "box.failure.no_account",
            Known::NodeMissing => "box.failure.node_missing",
            Known::AdapterNotInstalled => "box.failure.adapter_not_installed",
        })
    }
}

/// A failure as the window says it: what to say, and the line that was written, for the details.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub said: Said,
    pub details: Option<String>,
}

/// The reason in a line of `box`, without its prefix; `None` for a line that is not one of its own.
pub fn reason(line: &str) -> Option<&str> {
    let reason = line.strip_prefix(BOX_PREFIX)?.trim();
    (!reason.is_empty()).then_some(reason)
}

/// The failure Sakura knows in `reason`, if it knows it.
pub fn known(reason: &str) -> Option<Known> {
    let lowered = reason.to_lowercase();
    MARKS
        .iter()
        .find(|(mark, _)| lowered.contains(mark))
        .map(|(_, known)| *known)
}

/// What to say of the last line a failed `box` printed (its own, or of a program it ran), or of the line
/// Sakura wrote about a creation: the known sentence or the general one, with the line kept as the details.
pub fn said(line: &str) -> Failure {
    let said = match line.strip_prefix(SAKURA_PREFIX) {
        Some(own) => said_of_sakura(own),
        None => Said::from(known(reason(line).unwrap_or(line)).map_or(UNKNOWN, Known::key)),
    };
    Failure {
        said,
        details: Some(line.to_string()),
    }
}

/// What to say of a line Sakura wrote itself.
fn said_of_sakura(line: &str) -> Said {
    let timed_out = line.strip_prefix(SAKURA_TIMED_OUT);
    let seconds = timed_out.and_then(|rest| rest.trim_end_matches(" s").parse::<u64>().ok());
    match seconds {
        Some(seconds) => {
            let limit = crate::box_cli::limit_said(std::time::Duration::from_secs(seconds));
            Said::new("error.new.timeout").said("limit", limit)
        }
        None if line.starts_with(SAKURA_BOX_MISSING) => Said::new("error.box.missing"),
        None => Said::new("error.internal"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_known_line_names_its_sentence_and_the_line_is_kept_as_details() {
        let line = "box: task 'm9' does not exist (box new m9)";
        let failure = said(line);
        assert_eq!(failure.said, Said::new("box.failure.no_such_task"));
        assert_eq!(failure.details.as_deref(), Some(line));
    }

    #[test]
    fn an_unknown_line_is_the_general_sentence_never_the_line_itself() {
        let failure = said("box: fd 7: Bad file descriptor");
        assert_eq!(failure.said, Said::from(UNKNOWN));
        assert_eq!(
            failure.details.as_deref(),
            Some("box: fd 7: Bad file descriptor")
        );
        let foreign = super::said("Error response from daemon: no such container");
        assert_eq!(foreign.said, Said::from(UNKNOWN));
    }

    #[test]
    fn the_lines_box_writes_are_each_told_apart() {
        let lines = [
            ("an agent is already running for task 'm1' (box stop m1, or box attach m1)", Known::AgentAlreadyRunning),
            ("an agent runs in the terminal for task 'm1'; stop it first (box stop m1)", Known::AgentInTerminal),
            ("a conversation is open for task 'm1' (box acp); close it first (box stop m1)", Known::ConversationOpen),
            ("no agent is running for task 'm1'", Known::NoAgent),
            ("account 'personal' has no login yet (box login personal)", Known::AccountNotLoggedIn),
            ("could not attach the proxy to agentbox-net-m1 (is the proxy running? box up)", Known::ProxyDown),
            ("task 'm1' already exists", Known::TaskExists),
            ("task 'm1' has 3 uncommitted file(s); commit them or use --force", Known::UncommittedFiles),
            ("task 'm1' has commits that were not taken (box take m1); use --force to discard them", Known::CommitsNotTaken),
            ("task 'm1' has no commits to take", Known::NothingToTake),
            ("the bundle of task 'm1' did not verify", Known::BundleNotVerified),
            ("dependency install failed; the workspace is kept, remove it with: box rm m1 --force", Known::DependenciesFailed),
            ("no runner is running", Known::RunnerStopped),
            ("a runner is already running (pid 4242)", Known::RunnerAlreadyRunning),
            ("verb 'lint' is not in the profile of task 'm1'", Known::VerbNotInProfile),
            ("unknown profile 'x' (profiles/x.conf not found)", Known::UnknownProfile),
            ("invalid task name 'A B' (lowercase letters, digits and single hyphens)", Known::InvalidName),
            ("could not produce the diff of task 'm1'", Known::ChangesUnreadable),
        ];
        for (line, expected) in lines {
            assert_eq!(known(line), Some(expected), "{line}");
        }
        assert_eq!(known("invalid --since time"), None, "not a name");
    }

    #[test]
    fn every_known_failure_has_a_key_of_its_own() {
        let keys: std::collections::HashSet<Key> =
            MARKS.iter().map(|(_, known)| known.key()).collect();
        let kinds: std::collections::HashSet<Known> = MARKS.iter().map(|(_, k)| *k).collect();
        assert_eq!(keys.len(), kinds.len());
        assert!(keys.iter().all(|key| key.0.starts_with("box.failure.")));
    }

    #[test]
    fn a_line_sakura_wrote_about_a_creation_says_what_it_was() {
        let timed_out = said("sakura: box new timed out after 2700 s");
        let limit = Said::new("limit.minutes").count(45);
        assert_eq!(
            timed_out.said,
            Said::new("error.new.timeout").said("limit", limit)
        );
        assert_eq!(
            said("sakura: box not found").said,
            Said::new("error.box.missing")
        );
        let unusable = said("sakura: box could not be run: Permission denied");
        assert_eq!(unusable.said, Said::new("error.internal"));
        assert_eq!(
            unusable.details.as_deref(),
            Some("sakura: box could not be run: Permission denied")
        );
    }

    #[test]
    fn a_line_that_is_not_box_s_own_has_no_reason() {
        assert_eq!(
            reason("box: no runner is running"),
            Some("no runner is running")
        );
        assert_eq!(reason("box: "), None);
        assert_eq!(reason("Error: something else"), None);
    }
}
