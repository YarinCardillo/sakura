//! What an error says to the person: the engine's words when it gives them, else a plain sentence naming what
//! failed, and never a command line or a path of this Mac; the raw text stays behind a disclosure.

use super::plain_error::{plain_of, shows_command_or_path};

const RAW: &str = "pull request lookup failed: To get started with GitHub CLI, please run:  gh auth login; see /home/someone/engine-home/measure/flow/add/runs/operations/pull-request-lookup.log";

#[test]
fn a_command_line_and_a_path_of_this_mac_are_recognised() {
    for text in [
        RAW,
        "see /home/someone/x/y.log",
        "run: gh auth login",
        "failed, try git push --force",
        "opened /private/var/folders/ab/cd.txt",
        "at ~/projects/app/x.log",
        "$ cargo test",
    ] {
        assert!(shows_command_or_path(text), "{text}");
    }
    for text in [
        "The pull request lookup failed.",
        "The build took 3 min 20 s and a test failed.",
        "Disk is full (6 GB free of 500 GB).",
        "Read the spec and approve it",
        "",
    ] {
        assert!(!shows_command_or_path(text), "{text}");
    }
}

const REVIEWER: &str = "the code reviewer did not approve the contract flows: e2e/tally_cli.py run_on_file uses tempfile.TemporaryDirectory(ignore_cleanup_errors=True), which requires Python 3.10+; on an older interpreter every line-count row and R1 fail with a TypeError instead of checking the behaviour named, so the flow must work on any Python 3 interpreter (e.g. drop the keyword and handle cleanup errors explicitly); a correct README can make R12 pass when the example is not under a usage heading; the parser must skip fenced code blocks";

#[test]
fn a_reviewer_s_reason_in_plain_english_is_shown_as_it_is_with_nothing_hidden() {
    assert!(!shows_command_or_path(REVIEWER), "make is a verb here");
    let plain = plain_of(REVIEWER, "");
    assert_eq!(plain.words, REVIEWER);
    assert_eq!(plain.raw, None, "nothing to hide behind Details");
}

#[test]
fn a_stack_trace_and_a_run_id_are_not_words_for_a_person() {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    for raw in [
        "Traceback (most recent call last):\n  File \"x.py\", line 3, in <module>\nValueError: no",
        "build-1.2 exited with status 1",
        "thread 'main' panicked at src/lib.rs:10:5",
    ] {
        let plain = plain_of(raw, "");
        assert_ne!(plain.words, raw, "{raw}");
        assert_eq!(plain.raw.as_deref(), Some(raw), "kept behind Details");
    }
}

#[test]
fn the_engine_s_words_win_and_the_raw_text_goes_behind_details() {
    let plain = plain_of(
        RAW,
        "The pull request could not be looked up: GitHub is not signed in.",
    );
    assert_eq!(
        plain.words,
        "The pull request could not be looked up: GitHub is not signed in."
    );
    assert_eq!(plain.raw.as_deref(), Some(RAW));
}

#[test]
fn without_the_engine_s_words_a_raw_error_becomes_a_plain_sentence_naming_what_failed() {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let plain = plain_of(RAW, "");
    assert_eq!(plain.words, "Pull request lookup failed.");
    assert_eq!(
        plain.raw.as_deref(),
        Some(RAW),
        "the raw text is kept for Details"
    );
    assert!(!shows_command_or_path(&plain.words));
}

#[test]
fn an_error_that_names_nothing_gets_a_generic_sentence_and_a_plain_one_stays_as_it_is() {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let odd = plain_of("exit 2: /usr/local/bin/tool --flag", "");
    assert_eq!(
        odd.words,
        "Something went wrong and the engine gave no plain words for it."
    );
    assert!(odd.raw.is_some());
    let plain = plain_of("The reviewer requested changes.", "");
    assert_eq!(plain.words, "The reviewer requested changes.");
    assert_eq!(plain.raw, None, "nothing to hide");
}

#[test]
fn the_sentences_exist_in_italian_too() {
    crate::l10n::set_for_test(crate::l10n::Language::Italian);
    assert_eq!(
        plain_of("exit 2: /usr/local/bin/tool --flag", "").words,
        "Qualcosa è andato storto e il motore non ne ha dato una spiegazione chiara."
    );
    assert_eq!(
        plain_of(RAW, "").words,
        "Pull request lookup failed.",
        "the engine's own phrase, said plainly"
    );
}
