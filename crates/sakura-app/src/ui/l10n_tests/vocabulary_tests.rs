//! The interface's vocabulary, kept by tests: the words the person banned are in no value a person
//! reads, in either locale file, and no value holds an em dash, a middle dot or italics markup.
//!
//! Keys are code identifiers (`waiting.approve` is the Inbox's Approve), so only the values count.

use super::{locale, placeholders, ENGLISH, ITALIAN};
use std::collections::BTreeMap;

/// Words the interface never says, in either language: the engine's word for a lane's work is a step,
/// "Waiting" is "Needs you", and a decision is Approve or Decline.
const BANNED_WORDS: [&str; 11] = [
    "waiting", "task", "tasks", "ok", "ko", "accept", "accepts", "accepted", "reject", "rejects",
    "rejected",
];

/// Italian words the interface never says: a unit of work is a passo, and "lavoro" names a task.
const BANNED_ITALIAN_WORDS: [&str; 4] = ["lavoro", "lavori", "compito", "compiti"];

/// Product phrases with a word reserved for a different concept elsewhere.
const ALLOWED_PHRASES: [&str; 3] = ["area di lavoro", "scheduled tasks", "scheduled task"];

/// Keys, or key prefixes ending in a dot, that only the build with the `boxes` feature shows. That build
/// still works in tasks of a box, so its words keep the task; the engine build never reads them.
const BOX_BUILD_ONLY: [&str; 31] = [
    "box.failure.",
    "centre.",
    "changes.unreadable",
    "desk.reading_all",
    "direct.",
    "error.new.",
    "error.task.",
    "error.terminal.",
    "files.invalid.",
    "graph.",
    "group.working",
    "menu.file.new_task",
    "new_task.",
    "page.reading",
    "palette.placeholder",
    "play.unavailable.",
    "projects.unsupported",
    "publish.tip.after_take",
    "remove.reason.",
    "runner.",
    "settings.accounts.mac.description",
    "settings.machine.",
    "settings.project.",
    "settings.storage.",
    "sidebar.",
    "start.creation_failed",
    "state.working",
    "tabs.waiting",
    "take.disabled.",
    "validate.label.",
    "verify.",
];

/// What a person reads of a value: the names of the values it is filled with are not words, and the
/// allowed phrases are left out.
fn read_by_a_person(value: &str) -> String {
    let mut text = value.to_lowercase();
    for name in placeholders(value) {
        text = text.replace(&format!("%{{{name}}}"), " ");
    }
    for phrase in ALLOWED_PHRASES {
        text = text.replace(phrase, " ");
    }
    text
}

/// The banned words a value says, for the language of its file.
fn banned_in(value: &str, italian: bool) -> Vec<String> {
    let text = read_by_a_person(value);
    let words = text.split(|letter: char| !letter.is_alphanumeric());
    let is_banned = |word: &&str| {
        BANNED_WORDS.contains(word) || (italian && BANNED_ITALIAN_WORDS.contains(word))
    };
    words.filter(is_banned).map(str::to_string).collect()
}

/// A `.computer` key is the same text said for a machine that is not a Mac: it is shown where its base key is.
fn is_box_build_only(key: &str) -> bool {
    let key = key.strip_suffix(".computer").unwrap_or(key);
    BOX_BUILD_ONLY
        .iter()
        .any(|only| key == *only || (only.ends_with('.') && key.starts_with(only)))
}

fn engine_build_values(text: &str) -> BTreeMap<String, String> {
    let mut values = locale(text);
    values.retain(|key, _| !is_box_build_only(key));
    values
}

#[test]
fn no_value_the_engine_build_shows_says_a_banned_word_in_either_locale_file() {
    for (italian, text) in [(false, ENGLISH), (true, ITALIAN)] {
        let language = if italian { "Italian" } else { "English" };
        for (key, value) in engine_build_values(text) {
            let mut said = banned_in(&value, italian);
            if key.starts_with("scheduled.") {
                said.retain(|word| !matches!(word.as_str(), "task" | "tasks"));
            }
            assert!(
                said.is_empty(),
                "{key} says {said:?} in {language}: {value}"
            );
        }
    }
}

#[test]
fn a_banned_word_is_found_in_a_value_that_says_it() {
    assert_eq!(banned_in("A task is waiting", false), ["task", "waiting"]);
    assert_eq!(banned_in("Ok, accepted", false), ["ok", "accepted"]);
    assert_eq!(banned_in("Un lavoro in corso", true), ["lavoro"]);
    assert!(banned_in("%{task} wrote its spec", false).is_empty());
    assert!(banned_in("L'area di lavoro è pronta", true).is_empty());
}

#[test]
fn inbox_calls_both_blocked_kinds_blocked() {
    for text in [ENGLISH, ITALIAN] {
        let words = locale(text);
        assert_eq!(words["inbox.kind.blocked"], words["inbox.kind.stuck"]);
    }
}

#[test]
fn program_connector_copy_omits_shell_examples() {
    for text in [ENGLISH, ITALIAN] {
        let words = locale(text);
        for key in ["program_address_hint", "program_missing"] {
            let key = format!("settings_modal.connectors.{key}");
            assert!(!words[&key].contains("~/"), "{key}");
        }
    }
}

#[test]
fn the_tour_uses_the_same_navigation_labels_as_the_pages() {
    for text in [ENGLISH, ITALIAN] {
        let words = locale(text);
        for key in ["tour.lanes", "tour.inbox", "tour.engine"] {
            assert!(words[key].contains("%{page}"), "{key}");
        }
        assert!(words["tour.lanes"].contains("%{new_lane}"));
    }
}

#[test]
fn italian_product_labels_use_the_same_localized_terms() {
    let words = locale(ITALIAN);
    assert!(words["master.project_line"].starts_with(&words["page.project"]));
    assert!(words["new_lane.pace"].starts_with(&words["lane_side.pace"]));
    for key in [
        "lanes_page.column.preset",
        "new_lane.preset",
        "engine_page.preset",
        "engine_page.table.preset",
        "tour.engine_defaults",
    ] {
        assert!(words[key]
            .to_lowercase()
            .contains(&words["lane_side.preset"].to_lowercase()));
    }
    for key in [
        "sakura_pink",
        "instrument_cyan",
        "signal_orange",
        "chartreuse",
    ] {
        let key = format!("settings_modal.accent.{key}");
        assert_ne!(words[&key], locale(ENGLISH)[&key]);
    }
}

#[test]
fn the_names_only_the_box_build_shows_are_still_in_the_locale_files() {
    let english = locale(ENGLISH);
    for only in BOX_BUILD_ONLY {
        let is_named = english
            .keys()
            .any(|key| key == only || (only.ends_with('.') && key.starts_with(only)));
        assert!(is_named, "{only} exempts a key that does not exist");
    }
}

#[test]
fn no_value_holds_an_em_dash_a_middle_dot_or_italics_markup() {
    const FORBIDDEN: [&str; 5] = ["\u{2014}", "\u{b7}", "*", "<i>", "<em>"];
    for (language, text) in [("English", ENGLISH), ("Italian", ITALIAN)] {
        for (key, value) in locale(text) {
            let found = FORBIDDEN.iter().find(|mark| value.contains(**mark));
            assert!(found.is_none(), "{key} holds {found:?} in {language}");
        }
    }
}
