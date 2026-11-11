//! The remove rule: "Elimina lavoro" removes a task at once only when nothing in it would be lost; otherwise
//! a hold confirms it (`box rm --force`) and the reason is written beside the button.

use super::take::is_taken_at_head;
use crate::models::{AgentPhase, Task};
use crate::said::Said;

/// How removing a task is offered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoveRule {
    /// Nothing would be lost: one press removes the task.
    Plain,
    /// Work would be lost: a hold confirms it, and the reason is written beside the button.
    Hold { reason: Said },
    /// The task cannot be removed now.
    Disabled { reason: Said },
}

/// The remove rule for `task`. The first reason that applies is the one written: a running agent, then
/// uncommitted files (or files that could not be read), then commits that were not taken.
pub fn remove_rule(task: &Task) -> RemoveRule {
    if task.preparing.as_ref().is_some_and(|p| !p.failed) {
        return RemoveRule::Disabled {
            reason: Said::new("remove.reason.preparing"),
        };
    }
    match hold_reason(task) {
        Some(reason) => RemoveRule::Hold { reason },
        None => RemoveRule::Plain,
    }
}

fn hold_reason(task: &Task) -> Option<Said> {
    // Only the synthetic entry of a creation has no workspace, so nothing in it can be lost.
    if task.preparing.as_ref().is_some_and(|p| !p.has_workspace) {
        return None;
    }
    let is_agent_running = task
        .agent
        .as_ref()
        .is_some_and(|agent| agent.state == AgentPhase::Running);
    if is_agent_running {
        return Some(Said::new("remove.reason.agent_running"));
    }
    match task.dirty_files {
        None => return Some(Said::new("remove.reason.files_unread")),
        Some(0) => {}
        Some(n) => return Some(Said::new("reason.uncommitted_files").count(u64::from(n))),
    }
    match task.ahead {
        Some(0) => None,
        _ if is_taken_at_head(task) => None,
        Some(n) => Some(Said::new("remove.reason.commits_not_taken").count(u64::from(n))),
        None => Some(Said::new("remove.reason.commits_unread")),
    }
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::{preparing, readable, running, HEAD};
    use super::*;
    use crate::models::TakenState;

    fn hold(reason: Said) -> RemoveRule {
        RemoveRule::Hold { reason }
    }

    fn not_taken(commits: u64) -> Said {
        Said::new("remove.reason.commits_not_taken").count(commits)
    }

    fn uncommitted(files: u64) -> Said {
        Said::new("reason.uncommitted_files").count(files)
    }

    #[test]
    fn a_clean_task_with_nothing_ahead_or_all_taken_is_plain() {
        let mut task = readable("m1");
        task.ahead = Some(0);
        assert_eq!(remove_rule(&task), RemoveRule::Plain);
        let mut task = readable("m1");
        task.taken = Some(TakenState {
            sha: HEAD.into(),
            at: "2026-11-10T07:00:00Z".into(),
        });
        assert_eq!(remove_rule(&task), RemoveRule::Plain);
    }

    #[test]
    fn untaken_commits_need_a_hold_with_their_count() {
        assert_eq!(remove_rule(&readable("m1")), hold(not_taken(3)));
        let mut task = readable("m1");
        task.ahead = Some(1);
        assert_eq!(remove_rule(&task), hold(not_taken(1)));
    }

    #[test]
    fn uncommitted_files_come_before_commits() {
        let mut task = readable("m1");
        task.dirty_files = Some(2);
        assert_eq!(remove_rule(&task), hold(uncommitted(2)));
        task.dirty_files = None;
        assert_eq!(
            remove_rule(&task),
            hold(Said::new("remove.reason.files_unread"))
        );
    }

    #[test]
    fn a_running_agent_comes_first() {
        let mut task = running("m1");
        task.dirty_files = Some(4);
        assert_eq!(
            remove_rule(&task),
            hold(Said::new("remove.reason.agent_running"))
        );
    }

    #[test]
    fn a_failed_creation_whose_workspace_exists_follows_the_normal_rule() {
        let mut task = preparing("new", true);
        if let Some(preparing) = task.preparing.as_mut() {
            preparing.has_workspace = true;
        }
        assert_eq!(remove_rule(&task), hold(not_taken(3)));
        task.dirty_files = Some(1);
        assert_eq!(remove_rule(&task), hold(uncommitted(1)));
        task.ahead = Some(0);
        task.dirty_files = Some(0);
        assert_eq!(remove_rule(&task), RemoveRule::Plain);
    }

    #[test]
    fn a_task_in_preparation_cannot_be_removed_but_a_failed_one_can() {
        assert!(matches!(
            remove_rule(&preparing("new", false)),
            RemoveRule::Disabled { .. }
        ));
        assert_eq!(remove_rule(&preparing("new", true)), RemoveRule::Plain);
    }
}
