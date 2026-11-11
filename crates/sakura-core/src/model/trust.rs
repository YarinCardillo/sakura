//! When a green result cannot be taken as plain green. `box` records with each result whether the commit
//! that was checked changes the files that define the checks, which existing test files it modified or
//! deleted, and for the test verb how many tests ran and how many were skipped against the base's. A commit that rewrites its own checks, or that makes tests go away, passed
//! something other than what the base asks: it is never a plain take and never shows the green glyph.
//! What `box` did not record is not known, and what is not known is not known to be safe: a result without
//! one of these fields (an older `box`, a profile that names no test files, a base never tested) is not
//! plain green either, and the reason says which fact is missing.

use crate::models::{TestCounts, VerifyResult};
use crate::said::{Key, Said};

/// How many of the files that define the checks a reason names before it counts the rest.
const FILES_NAMED: usize = 1;

/// Why a result that passed is not plain green.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Distrust {
    /// The commit changes the files that define the checks.
    ChangesChecks { files: Vec<String> },
    /// `box` could not tell whether it does, and says why.
    ChecksUnknown,
    /// The commit modified or deleted test files that existed at the base.
    TestsTouched {
        modified: u32,
        deleted: u32,
        files: Vec<String>,
    },
    /// Fewer tests ran than the base runs.
    TestsDropped { from: u64, to: u64 },
    /// More tests are skipped than the base skips.
    MoreSkipped { more: u64 },
    /// `box` recorded nothing about something that must be known.
    NotRecorded(Missing),
}

/// What a result says nothing about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Missing {
    /// Whether the commit changes the files that define the checks.
    Checks,
    /// Whether the commit modified or deleted tests that existed at the base.
    TestsTouched,
    /// How many tests ran.
    TestCounts,
    /// How many tests the base runs.
    BaseCounts,
    /// How many tests are skipped, here or at the base.
    Skipped,
}

impl Missing {
    fn reason(self) -> Said {
        Said::new(match self {
            Missing::Checks => "trust.missing.checks",
            Missing::TestsTouched => "trust.missing.tests_touched",
            Missing::TestCounts => "trust.missing.test_counts",
            Missing::BaseCounts => "trust.missing.base_counts",
            Missing::Skipped => "trust.missing.skipped",
        })
    }

    fn reserve(self) -> Reserve {
        match self {
            Missing::Checks => Reserve::ChecksUnknown,
            Missing::TestsTouched => Reserve::TestsUnknown,
            Missing::TestCounts | Missing::BaseCounts | Missing::Skipped => Reserve::CountsUnknown,
        }
    }
}

/// The verb whose results count tests.
const COUNTED_VERB: &str = "test";

impl Distrust {
    /// The reason written beside the take action.
    pub fn reason(&self) -> Said {
        match self {
            Distrust::ChangesChecks { files } => changed_checks(files),
            Distrust::ChecksUnknown => Said::new("trust.checks_unknown"),
            Distrust::TestsTouched {
                modified, deleted, ..
            } => touched_tests(*modified, *deleted),
            Distrust::TestsDropped { from, to } => Said::new("trust.tests_dropped")
                .number("from", *from)
                .number("to", *to),
            Distrust::MoreSkipped { more } => Said::new("trust.more_skipped").count(*more),
            Distrust::NotRecorded(missing) => missing.reason(),
        }
    }

    /// What it is about, for the view of a result: a green that is not plain green.
    pub fn reserve(&self) -> Reserve {
        match self {
            Distrust::ChangesChecks { .. } => Reserve::ChecksChanged,
            Distrust::ChecksUnknown => Reserve::ChecksUnknown,
            Distrust::TestsTouched { .. } => Reserve::TestsTouched,
            Distrust::TestsDropped { .. } => Reserve::FewerTests,
            Distrust::MoreSkipped { .. } => Reserve::MoreSkipped,
            Distrust::NotRecorded(missing) => missing.reserve(),
        }
    }

    /// The files the reason is about, in full, for whoever asks.
    pub fn files(&self) -> &[String] {
        match self {
            Distrust::ChangesChecks { files } | Distrust::TestsTouched { files, .. } => files,
            Distrust::ChecksUnknown
            | Distrust::TestsDropped { .. }
            | Distrust::MoreSkipped { .. }
            | Distrust::NotRecorded(_) => &[],
        }
    }
}

/// What a green result is held back for, as the Checks row says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Reserve {
    ChecksChanged,
    ChecksUnknown,
    TestsTouched,
    FewerTests,
    MoreSkipped,
    /// Nothing recorded about the tests that existed at the base.
    TestsUnknown,
    /// No count of the tests, or none of the base's, to compare.
    CountsUnknown,
}

impl Reserve {
    /// The key of the words of the row, in place of the word of a plain pass.
    pub fn word(self) -> Key {
        Key(match self {
            Reserve::ChecksChanged => "reserve.checks_changed",
            Reserve::ChecksUnknown => "reserve.checks_unknown",
            Reserve::TestsTouched => "reserve.tests_touched",
            Reserve::FewerTests => "reserve.fewer_tests",
            Reserve::MoreSkipped => "reserve.more_skipped",
            Reserve::TestsUnknown => "reserve.tests_unknown",
            Reserve::CountsUnknown => "reserve.counts_unknown",
        })
    }
}

/// That the commit changes the checks, with the first file and how many more there are. The name is the
/// agent's own: the window shows it as a label.
fn changed_checks(files: &[String]) -> Said {
    let Some(first) = files.first() else {
        return Said::new("trust.changes_checks.none");
    };
    match files.len() - FILES_NAMED {
        0 => Said::new("trust.changes_checks.only").text("file", first),
        more => Said::new("trust.changes_checks.more")
            .text("file", first)
            .count(more as u64),
    }
}

/// Why the result of `verb`, which passed, is not plain green: what `box` recorded against it, or else
/// the first thing that must be known and was not recorded. `None` only when everything that must be
/// known is recorded and says the checks and the tests are the base's.
pub fn distrust(verb: &str, result: &VerifyResult) -> Option<Distrust> {
    recorded(result).or_else(|| missing(verb, result).map(Distrust::NotRecorded))
}

/// What `box` recorded against `result`.
fn recorded(result: &VerifyResult) -> Option<Distrust> {
    if result.changes_checks == Some(true) {
        let files = result.checks_touched.clone().unwrap_or_default();
        let is_unknown = files.is_empty() && result.checks_reason.is_some();
        return Some(if is_unknown {
            Distrust::ChecksUnknown
        } else {
            Distrust::ChangesChecks { files }
        });
    }
    if let Some(touched) = result.tests_touched.as_ref() {
        if touched.modified + touched.deleted > 0 {
            return Some(Distrust::TestsTouched {
                modified: touched.modified,
                deleted: touched.deleted,
                files: touched.files.clone(),
            });
        }
    }
    counts_distrust(result.tests.as_ref()?)
}

/// The first fact the result of `verb` should carry and does not. Counts belong to the test verb only.
fn missing(verb: &str, result: &VerifyResult) -> Option<Missing> {
    if result.changes_checks.is_none() {
        return Some(Missing::Checks);
    }
    if result.tests_touched.is_none() {
        return Some(Missing::TestsTouched);
    }
    if verb != COUNTED_VERB {
        return None;
    }
    let Some(tests) = result.tests.as_ref() else {
        return Some(Missing::TestCounts);
    };
    if tests.base.is_none() {
        return Some(Missing::BaseCounts);
    }
    (tests.skipped.is_none() || tests.base_skipped.is_none()).then_some(Missing::Skipped)
}

/// What the counts of a test result say against the base's: fewer tests ran, or more are skipped.
fn counts_distrust(tests: &TestCounts) -> Option<Distrust> {
    if let Some(base) = tests.base.filter(|base| tests.ran < *base) {
        return Some(Distrust::TestsDropped {
            from: base,
            to: tests.ran,
        });
    }
    let more = tests.skipped?.checked_sub(tests.base_skipped?)?;
    (more > 0).then_some(Distrust::MoreSkipped { more })
}

/// What `result`'s commit did to the tests that existed at the base; `None` when it left them alone or
/// `box` did not record it.
pub fn touched_sentence(result: &VerifyResult) -> Option<Said> {
    let touched = result.tests_touched.as_ref()?;
    let is_touched = touched.modified + touched.deleted > 0;
    is_touched.then(|| touched_tests(touched.modified, touched.deleted))
}

/// That the commit modifies so many existing tests, deletes so many, or both. The count that decides the
/// plural is the first figure said.
fn touched_tests(modified: u32, deleted: u32) -> Said {
    let (modified, deleted) = (u64::from(modified), u64::from(deleted));
    match (modified, deleted) {
        (modified, 0) => Said::new("trust.tests_modified").count(modified),
        (0, deleted) => Said::new("trust.tests_deleted").count(deleted),
        (modified, deleted) => Said::new("trust.tests_modified_deleted")
            .count(modified)
            .number("deleted", deleted),
    }
}

/// The count of a test result as the Checks row shows it: so many tests, and so many skipped when
/// some were.
pub fn tests_word(result: &VerifyResult) -> Option<Said> {
    let tests = result.tests.as_ref()?;
    let ran = Said::new("verify.tests").count(tests.ran);
    Some(match tests.skipped.unwrap_or(0) {
        0 => ran,
        skipped => Said::new("verify.tests_skipped")
            .said("tests", ran)
            .count(skipped),
    })
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::{passed_result, readable, unrecorded_result};
    use super::*;
    use crate::models::{TestCounts, TestsTouched};

    fn passed() -> VerifyResult {
        let mut task = readable("m1");
        passed_result(&mut task, "test");
        task.results.remove("test").unwrap()
    }

    fn counts(ran: u64, base: Option<u64>) -> Option<TestCounts> {
        Some(TestCounts {
            ran,
            skipped: Some(0),
            base,
            base_skipped: Some(0),
        })
    }

    /// What the row says of 7876 tests with `count` skipped.
    fn skipped(count: u64) -> Said {
        Said::new("verify.tests_skipped")
            .said("tests", Said::new("verify.tests").count(7876))
            .count(count)
    }

    /// A passed result as an older `box` writes it: nothing recorded.
    fn unrecorded() -> VerifyResult {
        let mut task = readable("m1");
        unrecorded_result(&mut task, "test");
        task.results.remove("test").unwrap()
    }

    #[test]
    fn a_result_without_the_fields_is_not_known_to_be_safe() {
        let distrust = distrust("test", &unrecorded()).unwrap();
        assert_eq!(distrust, Distrust::NotRecorded(Missing::Checks));
        assert_eq!(distrust.reserve().word(), Key("reserve.checks_unknown"));
        assert!(distrust.files().is_empty());
        assert_eq!(tests_word(&unrecorded()), None);
    }

    #[test]
    fn a_commit_that_leaves_the_checks_alone_and_runs_every_test_is_trusted() {
        let mut result = passed();
        assert_eq!(distrust("test", &result), None);
        result.tests = counts(7900, Some(7880));
        assert_eq!(
            distrust("test", &result),
            None,
            "more tests than the base is fine"
        );
        result.tests = counts(300, None);
        assert_eq!(
            distrust("test", &result),
            Some(Distrust::NotRecorded(Missing::BaseCounts)),
            "an unknown base is not a base that agrees"
        );
        assert_eq!(
            distrust("rules", &result),
            None,
            "other verbs count no tests"
        );
    }

    #[test]
    fn a_commit_that_changes_the_checks_is_named_with_its_files() {
        let mut result = passed();
        result.changes_checks = Some(true);
        result.checks_touched = Some(vec!["vitest.config.ts".into(), "package.json".into()]);
        let distrust = distrust("test", &result).unwrap();
        assert_eq!(
            distrust.reason(),
            Said::new("trust.changes_checks.more")
                .text("file", "vitest.config.ts")
                .count(1)
        );
        assert_eq!(distrust.files().len(), 2);
        result.checks_touched = Some(vec!["vitest.config.ts".into()]);
        assert_eq!(
            super::distrust("test", &result).unwrap().reason(),
            Said::new("trust.changes_checks.only").text("file", "vitest.config.ts")
        );
    }

    #[test]
    fn a_file_name_that_hides_or_breaks_the_line_is_shown_with_marks_in_the_reason() {
        let mut result = passed();
        result.changes_checks = Some(true);
        result.checks_touched = Some(vec!["vitest\u{202e}.config.ts\nTutto bene".into()]);
        let distrust = distrust("test", &result).unwrap();
        assert_eq!(
            distrust.reason(),
            Said::new("trust.changes_checks.only")
                .text("file", "vitest\u{202e}.config.ts\nTutto bene"),
            "the name is kept as written: the window shows it as a label"
        );
        assert_eq!(
            distrust.files(),
            ["vitest\u{202e}.config.ts\nTutto bene"],
            "the name itself is kept as written"
        );
    }

    #[test]
    fn checks_box_could_not_compare_are_unknown_not_changed() {
        let mut result = passed();
        result.changes_checks = Some(true);
        result.checks_touched = Some(Vec::new());
        result.checks_reason = Some("could not be computed".into());
        let distrust = distrust("test", &result).unwrap();
        assert_eq!(distrust, Distrust::ChecksUnknown);
        assert_eq!(distrust.reason(), Said::new("trust.checks_unknown"));
    }

    #[test]
    fn fewer_tests_than_the_base_is_said_with_both_counts() {
        let mut result = passed();
        result.changes_checks = Some(false);
        result.tests = counts(300, Some(7880));
        let distrust = distrust("test", &result).unwrap();
        assert_eq!(
            distrust,
            Distrust::TestsDropped {
                from: 7880,
                to: 300
            }
        );
        assert_eq!(
            distrust.reason(),
            Said::new("trust.tests_dropped")
                .number("from", 7880)
                .number("to", 300)
        );
    }

    #[test]
    fn changed_checks_are_said_before_a_drop_in_tests() {
        let mut result = passed();
        result.changes_checks = Some(true);
        result.checks_touched = Some(vec!["package.json".into()]);
        result.tests = counts(300, Some(7880));
        assert!(matches!(
            distrust("test", &result),
            Some(Distrust::ChangesChecks { .. })
        ));
    }

    fn touched(modified: u32, deleted: u32) -> VerifyResult {
        let mut result = passed();
        result.changes_checks = Some(false);
        result.tests_touched = Some(TestsTouched {
            modified,
            deleted,
            files: vec!["src/a.spec.ts".into(), "src/b.spec.ts".into()],
        });
        result
    }

    #[test]
    fn existing_tests_modified_or_deleted_are_said_with_their_counts() {
        let sentence = |modified, deleted| touched_sentence(&touched(modified, deleted));
        assert_eq!(
            sentence(3, 1),
            Some(
                Said::new("trust.tests_modified_deleted")
                    .count(3)
                    .number("deleted", 1)
            )
        );
        assert_eq!(
            sentence(1, 0),
            Some(Said::new("trust.tests_modified").count(1))
        );
        assert_eq!(
            sentence(0, 2),
            Some(Said::new("trust.tests_deleted").count(2))
        );
        assert_eq!(
            sentence(1, 4),
            Some(
                Said::new("trust.tests_modified_deleted")
                    .count(1)
                    .number("deleted", 4)
            )
        );
        assert_eq!(sentence(0, 0), None);
        assert_eq!(
            touched_sentence(&passed()),
            None,
            "not recorded, nothing said"
        );
    }

    #[test]
    fn a_commit_that_touches_existing_tests_is_distrusted_with_its_files() {
        let distrust = distrust("test", &touched(3, 1)).unwrap();
        assert_eq!(
            distrust.reason(),
            Said::new("trust.tests_modified_deleted")
                .count(3)
                .number("deleted", 1)
        );
        assert_eq!(distrust.files(), ["src/a.spec.ts", "src/b.spec.ts"]);
        assert_eq!(distrust.reserve().word(), Key("reserve.tests_touched"));
        assert_eq!(super::distrust("test", &touched(0, 0)), None);
    }

    #[test]
    fn touched_tests_are_said_after_changed_checks_and_before_a_drop() {
        let mut result = touched(0, 2);
        result.tests = counts(300, Some(7880));
        assert!(matches!(
            distrust("test", &result),
            Some(Distrust::TestsTouched { .. })
        ));
        result.changes_checks = Some(true);
        assert!(matches!(
            distrust("test", &result),
            Some(Distrust::ChangesChecks { .. })
        ));
    }

    fn skipping(skipped: Option<u64>, base_skipped: Option<u64>) -> VerifyResult {
        let mut result = passed();
        result.changes_checks = Some(false);
        result.tests = Some(TestCounts {
            ran: 7876,
            skipped,
            base: Some(7876),
            base_skipped,
        });
        result
    }

    #[test]
    fn more_skipped_tests_than_the_base_is_said_with_how_many_more() {
        let distrust = distrust("test", &skipping(Some(4), Some(0))).unwrap();
        assert_eq!(distrust, Distrust::MoreSkipped { more: 4 });
        assert_eq!(distrust.reason(), Said::new("trust.more_skipped").count(4));
        assert_eq!(distrust.reserve().word(), Key("reserve.more_skipped"));
        let one = super::distrust("test", &skipping(Some(3), Some(2))).unwrap();
        assert_eq!(one.reason(), Said::new("trust.more_skipped").count(1));
    }

    #[test]
    fn as_many_skipped_tests_as_the_base_says_nothing_and_an_unknown_count_is_missing() {
        assert_eq!(distrust("test", &skipping(Some(2), Some(2))), None);
        assert_eq!(distrust("test", &skipping(Some(1), Some(2))), None);
        let missing = Some(Distrust::NotRecorded(Missing::Skipped));
        assert_eq!(
            distrust("test", &skipping(Some(4), None)),
            missing,
            "no base to compare"
        );
        assert_eq!(
            distrust("test", &skipping(None, Some(0))),
            missing,
            "not recorded"
        );
    }

    #[test]
    fn a_drop_in_tests_is_said_before_more_skipped_ones() {
        let mut result = skipping(Some(4), Some(0));
        if let Some(tests) = result.tests.as_mut() {
            tests.ran = 300;
        }
        assert!(matches!(
            distrust("test", &result),
            Some(Distrust::TestsDropped { .. })
        ));
    }

    #[test]
    fn the_count_says_the_skipped_tests_when_there_are_any() {
        assert_eq!(tests_word(&skipping(Some(4), Some(0))), Some(skipped(4)));
        assert_eq!(tests_word(&skipping(Some(1), None)), Some(skipped(1)));
        assert_eq!(
            tests_word(&skipping(Some(0), Some(0))),
            Some(Said::new("verify.tests").count(7876))
        );
        assert_eq!(
            tests_word(&skipping(None, None)),
            Some(Said::new("verify.tests").count(7876))
        );
    }

    #[test]
    fn the_count_of_a_test_result_is_said_in_plain_figures() {
        let mut result = passed();
        result.tests = counts(7880, Some(7880));
        assert_eq!(
            tests_word(&result),
            Some(Said::new("verify.tests").count(7880))
        );
    }
}
