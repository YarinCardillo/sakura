//! The take rule: when "Prendi" is a plain press, when it needs a hold, and when it is not offered.

use super::result::{result_view, ResultView};
use super::trust::{distrust, Distrust};
use super::REQUIRED_VERBS;
use crate::models::Task;
use crate::said::Said;

/// How the "Prendi" action is offered for a task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TakeRule {
    /// Everything is verified on HEAD: one press takes the work.
    Plain,
    /// Taking is possible but unverified: a hold confirms it, and the reason is written beside the button.
    Hold { reason: Said },
    /// There is nothing that could be taken.
    Disabled { reason: Said },
}

/// The take rule for `task`. Plain only when there are commits ahead, no uncommitted file, and every verb
/// in [`REQUIRED_VERBS`] has a passed result for HEAD. The first reason that applies is the one written.
pub fn take_rule(task: &Task) -> TakeRule {
    if let Some(reason) = disabled_reason(task) {
        return TakeRule::Disabled { reason };
    }
    match hold_cause(task) {
        Some(cause) => TakeRule::Hold {
            reason: cause.reason(),
        },
        None => TakeRule::Plain,
    }
}

fn disabled_reason(task: &Task) -> Option<Said> {
    let reason = if task.preparing.is_some() {
        "take.disabled.preparing"
    } else if !task.readable {
        "take.disabled.unreadable"
    } else if task.ahead.unwrap_or(0) == 0 {
        "take.disabled.nothing"
    } else if is_taken_at_head(task) {
        "take.disabled.taken"
    } else {
        return None;
    };
    Some(Said::new(reason))
}

/// Why taking needs a hold: something said in a sentence, or a green result that is not plain green.
enum HoldCause {
    Sentence(Said),
    Distrust(Distrust),
}

impl HoldCause {
    fn reason(&self) -> Said {
        match self {
            HoldCause::Sentence(sentence) => sentence.clone(),
            HoldCause::Distrust(distrust) => distrust.reason(),
        }
    }
}

/// The first cause that applies: uncommitted files, then a required verb that did not pass on HEAD, then
/// a required verb that passed but cannot be trusted as plain green.
fn hold_cause(task: &Task) -> Option<HoldCause> {
    let sentence = |said: Said| Some(HoldCause::Sentence(said));
    match task.dirty_files {
        None => return sentence(Said::new("take.reason.files_unread")),
        Some(0) => {}
        Some(n) => return sentence(Said::new("reason.uncommitted_files").count(u64::from(n))),
    }
    let not_passed = REQUIRED_VERBS
        .iter()
        .find_map(|verb| verb_reason(verb, result_view(task, verb)));
    if let Some(reason) = not_passed {
        return sentence(reason);
    }
    let distrusted = REQUIRED_VERBS
        .iter()
        .find_map(|verb| distrust(verb, task.results.get(*verb)?));
    distrusted.map(HoldCause::Distrust)
}

fn verb_reason(verb: &str, view: ResultView) -> Option<Said> {
    let key = match view {
        ResultView::Passato | ResultView::VerdeConRiserva(_) => return None,
        ResultView::Fallito => "take.reason.check.failed",
        ResultView::Vecchio => "take.reason.check.old",
        ResultView::MaiEseguito => "take.reason.check.never",
        ResultView::Annullata => "take.reason.check.cancelled",
    };
    Some(Said::new(key).text("verb", verb))
}

/// The files the reason of a hold is about, in full, when it is about files: for whoever asks to see them.
pub fn take_details(task: &Task) -> Vec<String> {
    match hold_cause(task) {
        Some(HoldCause::Distrust(distrust)) if disabled_reason(task).is_none() => {
            distrust.files().to_vec()
        }
        _ => Vec::new(),
    }
}

/// Whether the task's HEAD is the commit that was taken last.
pub(super) fn is_taken_at_head(task: &Task) -> bool {
    match (&task.taken, &task.head) {
        (Some(taken), Some(head)) => taken.sha == *head,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::{
        cancelled_result, failed_result, passed_result, preparing, readable, unrecorded_result,
        HEAD,
    };
    use super::*;
    use crate::models::{TakenState, TestCounts};

    fn verified(name: &str) -> Task {
        let mut task = readable(name);
        passed_result(&mut task, "test");
        passed_result(&mut task, "rules");
        task
    }

    fn reason(rule: TakeRule) -> Option<Said> {
        match rule {
            TakeRule::Plain => None,
            TakeRule::Hold { reason } | TakeRule::Disabled { reason } => Some(reason),
        }
    }

    /// The reason of a required check that did not pass on this commit.
    fn check(how: &str, verb: &str) -> Said {
        let key = match how {
            "failed" => "take.reason.check.failed",
            "old" => "take.reason.check.old",
            "never" => "take.reason.check.never",
            _ => "take.reason.check.cancelled",
        };
        Said::new(key).text("verb", verb)
    }

    #[test]
    fn passed_results_that_say_nothing_about_the_checks_need_a_hold() {
        let mut task = readable("m1");
        unrecorded_result(&mut task, "test");
        unrecorded_result(&mut task, "rules");
        assert_eq!(
            take_rule(&task),
            TakeRule::Hold {
                reason: Said::new("trust.missing.checks")
            },
            "what is not known is not known to be safe"
        );
    }

    #[test]
    fn verified_clean_commits_ahead_is_plain() {
        assert_eq!(take_rule(&verified("m1")), TakeRule::Plain);
    }

    #[test]
    fn nothing_ahead_is_disabled() {
        let mut task = verified("m1");
        task.ahead = Some(0);
        assert_eq!(
            take_rule(&task),
            TakeRule::Disabled {
                reason: Said::new("take.disabled.nothing")
            }
        );
    }

    #[test]
    fn unreadable_and_preparing_are_disabled() {
        let mut task = verified("m1");
        task.readable = false;
        assert!(matches!(take_rule(&task), TakeRule::Disabled { .. }));
        assert!(matches!(
            take_rule(&preparing("new", false)),
            TakeRule::Disabled { .. }
        ));
    }

    #[test]
    fn already_taken_at_head_is_disabled() {
        let mut task = verified("m1");
        task.taken = Some(TakenState {
            sha: HEAD.into(),
            at: "2026-11-10T07:00:00Z".into(),
        });
        assert_eq!(
            reason(take_rule(&task)),
            Some(Said::new("take.disabled.taken"))
        );
    }

    #[test]
    fn uncommitted_files_need_a_hold_with_their_count() {
        let mut task = verified("m1");
        task.dirty_files = Some(2);
        assert_eq!(
            take_rule(&task),
            TakeRule::Hold {
                reason: Said::new("reason.uncommitted_files").count(2)
            }
        );
        task.dirty_files = Some(1);
        assert_eq!(
            reason(take_rule(&task)),
            Some(Said::new("reason.uncommitted_files").count(1))
        );
    }

    #[test]
    fn missing_required_result_needs_a_hold() {
        let mut task = readable("m1");
        passed_result(&mut task, "test");
        assert_eq!(reason(take_rule(&task)), Some(check("never", "rules")));
    }

    #[test]
    fn failed_required_result_needs_a_hold() {
        let mut task = verified("m1");
        failed_result(&mut task, "test");
        assert_eq!(reason(take_rule(&task)), Some(check("failed", "test")));
    }

    #[test]
    fn a_cancelled_required_result_never_satisfies_the_take_rule() {
        let mut task = verified("m1");
        cancelled_result(&mut task, "rules");
        if let Some(result) = task.results.get_mut("rules") {
            result.exit_code = 0;
        }
        assert_eq!(
            take_rule(&task),
            TakeRule::Hold {
                reason: check("cancelled", "rules")
            }
        );
    }

    fn result<'a>(task: &'a mut Task, verb: &str) -> &'a mut crate::models::VerifyResult {
        task.results.get_mut(verb).expect("a result")
    }

    #[test]
    fn a_verified_commit_that_changes_the_checks_is_never_a_plain_take() {
        let mut task = verified("m1");
        let rules = result(&mut task, "rules");
        rules.changes_checks = Some(true);
        rules.checks_touched = Some(vec!["eslint.config.js".into(), "package.json".into()]);
        assert_eq!(
            take_rule(&task),
            TakeRule::Hold {
                reason: Said::new("trust.changes_checks.more")
                    .text("file", "eslint.config.js")
                    .count(1)
            }
        );
        assert_eq!(
            take_details(&task),
            vec!["eslint.config.js", "package.json"]
        );
    }

    #[test]
    fn a_verified_commit_that_runs_fewer_tests_than_the_base_is_never_a_plain_take() {
        let mut task = verified("m1");
        let test = result(&mut task, "test");
        test.changes_checks = Some(false);
        test.tests = Some(TestCounts {
            ran: 300,
            base: Some(7880),
            ..Default::default()
        });
        assert_eq!(
            reason(take_rule(&task)),
            Some(
                Said::new("trust.tests_dropped")
                    .number("from", 7880)
                    .number("to", 300)
            )
        );
        assert!(take_details(&task).is_empty());
    }

    #[test]
    fn results_that_say_the_checks_are_untouched_and_the_tests_all_ran_are_a_plain_take() {
        let mut task = verified("m1");
        for verb in ["test", "rules"] {
            let result = result(&mut task, verb);
            result.changes_checks = Some(false);
            result.checks_touched = Some(Vec::new());
        }
        result(&mut task, "test").tests = Some(TestCounts {
            ran: 7880,
            skipped: Some(0),
            base: Some(7880),
            base_skipped: Some(0),
        });
        assert_eq!(take_rule(&task), TakeRule::Plain);
        assert!(take_details(&task).is_empty());
    }

    #[test]
    fn each_fact_box_did_not_record_is_a_hold_that_names_it() {
        let held = |spoil: &dyn Fn(&mut Task)| {
            let mut task = verified("m1");
            spoil(&mut task);
            reason(take_rule(&task))
        };
        assert_eq!(
            held(&|task| result(task, "rules").tests_touched = None),
            Some(Said::new("trust.missing.tests_touched"))
        );
        assert_eq!(
            held(&|task| result(task, "test").tests = None),
            Some(Said::new("trust.missing.test_counts"))
        );
        let no_base = |task: &mut Task| {
            if let Some(tests) = result(task, "test").tests.as_mut() {
                tests.base = None;
            }
        };
        assert_eq!(held(&no_base), Some(Said::new("trust.missing.base_counts")));
        let no_base_skipped = |task: &mut Task| {
            if let Some(tests) = result(task, "test").tests.as_mut() {
                tests.base_skipped = None;
            }
        };
        assert_eq!(
            held(&no_base_skipped),
            Some(Said::new("trust.missing.skipped"))
        );
        assert_eq!(
            held(&|task| result(task, "rules").tests = None),
            None,
            "only the test verb counts tests"
        );
    }

    #[test]
    fn a_verified_commit_that_touches_existing_tests_is_never_a_plain_take() {
        let mut task = verified("m1");
        let test = result(&mut task, "test");
        test.changes_checks = Some(false);
        test.tests_touched = Some(crate::models::TestsTouched {
            modified: 3,
            deleted: 1,
            files: vec!["src/a.spec.ts".into()],
        });
        assert_eq!(
            take_rule(&task),
            TakeRule::Hold {
                reason: Said::new("trust.tests_modified_deleted")
                    .count(3)
                    .number("deleted", 1)
            }
        );
        assert_eq!(take_details(&task), vec!["src/a.spec.ts"]);
    }

    #[test]
    fn a_verified_commit_that_skips_more_tests_than_the_base_is_never_a_plain_take() {
        let mut task = verified("m1");
        let test = result(&mut task, "test");
        test.changes_checks = Some(false);
        test.tests = Some(TestCounts {
            ran: 7876,
            skipped: Some(4),
            base: Some(7876),
            base_skipped: Some(0),
        });
        assert_eq!(
            take_rule(&task),
            TakeRule::Hold {
                reason: Said::new("trust.more_skipped").count(4)
            }
        );
    }

    #[test]
    fn a_failed_verb_is_said_before_any_distrust_of_another() {
        let mut task = verified("m1");
        result(&mut task, "test").changes_checks = Some(true);
        failed_result(&mut task, "rules");
        assert_eq!(reason(take_rule(&task)), Some(check("failed", "rules")));
    }

    #[test]
    fn green_result_for_an_older_commit_needs_a_hold() {
        let mut task = verified("m1");
        if let Some(result) = task.results.get_mut("test") {
            result.is_head = false;
        }
        assert_eq!(reason(take_rule(&task)), Some(check("old", "test")));
    }

    #[test]
    fn unknown_dirty_count_is_never_plain() {
        let mut task = verified("m1");
        task.dirty_files = None;
        assert!(matches!(take_rule(&task), TakeRule::Hold { .. }));
    }
}
