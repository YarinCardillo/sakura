//! The lines a lane run writes in its phases, as sentences. Each is a tag and a part of the line's words (which the
//! engine's source fixes: `tests/ledger_lines.py` of the engine lists them, and a test of the interface reads that
//! list), so what a line carries besides those words, a task name, a hash, a path, is never shown.

use crate::l10n::{tr, Key};
use crate::ui::lanes::capitalised;
use gpui::SharedString;

/// `(tag, words the line holds in lower case, the sentence)`; the first row that fits wins.
const KINDS: [(&str, &str, Key); 29] = [
    ("plan_review", "revise", Key("events.kind.plan_revised")),
    (
        "plan_review",
        "plan review is off",
        Key("events.kind.plan_review_off"),
    ),
    (
        "contract",
        "wait for the person's approval",
        Key("events.kind.checks_wait"),
    ),
    (
        "contract",
        "cannot be moved",
        Key("events.kind.workspace_not_moved"),
    ),
    (
        "build",
        "build tasks on up to",
        Key("events.kind.build_split"),
    ),
    ("build", "merged", Key("events.kind.task_merged")),
    (
        "build",
        "needs no change",
        Key("events.kind.task_no_change"),
    ),
    ("build", "needs changes", Key("events.kind.task_again")),
    (
        "build",
        "still shows another revision",
        Key("events.kind.workspace_behind"),
    ),
    ("build", "tests.full is empty", Key("events.kind.no_tests")),
    (
        "review",
        "contract flows review",
        Key("events.kind.contract_reviewed"),
    ),
    (
        "review",
        "final review round",
        Key("events.kind.final_revised"),
    ),
    (
        "review",
        "final review sent back",
        Key("events.kind.final_sent_back"),
    ),
    (
        "review",
        "final review still asks",
        Key("events.kind.final_still_asks"),
    ),
    (
        "review",
        "final review: the person accepted",
        Key("events.kind.final_accepted"),
    ),
    ("review", "review round", Key("events.kind.review_round")),
    ("plan", "the plan step is off", Key("events.kind.plan_off")),
    (
        "integration",
        "the engine's head is",
        Key("events.kind.work_put_together"),
    ),
    (
        "integration",
        "overlap notice not posted",
        Key("events.kind.overlap_not_posted"),
    ),
    (
        "pr",
        "overlap check skipped",
        Key("events.kind.overlap_skipped"),
    ),
    (
        "evaluate",
        "not verified by an evaluator",
        Key("events.kind.not_verified"),
    ),
    ("evaluate", "check ", Key("events.kind.check_ran")),
    (
        "calibrate",
        "calibration",
        Key("events.kind.check_calibrated"),
    ),
    (
        "intake",
        "spec changed after approval",
        Key("events.kind.spec_changed"),
    ),
    (
        "report",
        "owner report written",
        Key("events.kind.report_written"),
    ),
    ("questions", "asked by", Key("events.kind.question_asked")),
    ("budget", "ruling:", Key("events.kind.budget_ruling")),
    ("budget", "preflight asks:", Key("events.kind.budget_asks")),
    ("budget", "no usage archive", Key("events.kind.budget_off")),
];

/// Lines of the person's marks, which carry no phase of their own.
const OWNER_KINDS: [(&str, Key); 2] = [
    ("marked", Key("events.kind.row_marked")),
    (
        "put back on the engine's head",
        Key("events.kind.workspace_restored"),
    ),
];

/// The sentence of a line of a lane run this knows by its tag and words, or `None`.
pub fn kind_words(tag: &str, what: &str) -> Option<SharedString> {
    let what = what.to_lowercase();
    let own = KINDS
        .iter()
        .find(|(kind_tag, words, _)| *kind_tag == tag && what.contains(words))
        .map(|(_, _, key)| *key);
    let owner = || {
        OWNER_KINDS
            .iter()
            .find(|(words, _)| tag == "owner" && what.contains(words))
            .map(|(_, key)| *key)
    };
    own.or_else(owner).map(|key| tr(key).into())
}

/// A tag as a phase reads in a sentence: the tags of a run that are not a word as they stand have their own.
pub fn phase_name(tag: &str) -> SharedString {
    match tag {
        "pr" => tr(Key("events.phase.pr")).into(),
        "plan_review" => tr(Key("events.phase.plan_review")).into(),
        "calibrate" => tr(Key("events.phase.calibrate")).into(),
        _ => capitalised(tag).into(),
    }
}
