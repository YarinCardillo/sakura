//! The rule of the interface's words, kept by tests: every string a person reads comes from the locale
//! files (`locales/en.yml`, `locales/it.yml`), both files hold the same keys with the same values to fill
//! in, the code names no key that is missing and leaves none unused, and a change of language is drawn at
//! once, with no restart.
//!
//! What counts as a string a person reads is decided by a rule narrow enough to be read here:
//! - the literal is in code that ships (not a test file, not under `#[cfg(test)]`, not in a comment);
//! - it reads as words: it holds a word of two letters or more and either a space or a capital first
//!   letter (selectors, `box` arguments, keys and placeholders' names are single lower-case words);
//! - it is not a key of the locale files (`Key("...")`, `Said::new("...")`);
//! - it is not on a line (or the continuation of a line) that writes to stderr, fails a test or an
//!   invariant, names an element, binds a key, or matches what `box` printed ([`NOT_SHOWN`]);
//! - it is not one of the few literals listed in [`ALLOWED`], each with its reason.

use super::chrome::text_width;
use super::desk::Desk;
use super::interaction_tests::{draw, open_with, wait_for};
use crate::l10n::{self, tr, Key, Language, LanguageChoice};
use crate::theme;
use gpui::{Entity, FontWeight, OwnedMenuItem, Pixels, TestAppContext, VisualTestContext};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

const ENGLISH: &str = include_str!("../../locales/en.yml");
const ITALIAN: &str = include_str!("../../locales/it.yml");

/// The crates whose code names keys: the app, and the three that tell it what to say.
const KEY_SOURCES: [&str; 4] = [
    "sakura-app/src",
    "sakura-core/src",
    "sakura-acp/src",
    "sakura-terminal/src",
];

/// Files of the app that ship no interface text, and why.
const NOT_INTERFACE: [(&str, &str); 5] = [
    ("l10n.rs", "the localisation calls themselves"),
    (
        "real_text.rs",
        "test support: the layout tests' text system",
    ),
    (
        "snapshot.rs",
        "snapshot tooling: its words go to stderr and to files",
    ),
    ("drive.rs", "snapshot tooling: names of elements to press"),
    (
        "fixture_changes.rs",
        "the sample patch: invented data in git's format",
    ),
];

/// A line with one of these is not interface text: it logs, asserts, names an element, binds a key,
/// declares an action, reads an attribute, matches what `box` printed, notes a call for the tests
/// (`recorded`, `note_read`), names a view for the render counter, or gives the technical details of an
/// internal failure, which are data under the details.
const NOT_SHOWN: [&str; 22] = [
    "eprintln!",
    "panic!",
    "unreachable!",
    "expect(",
    "assert",
    "debug_selector",
    ".id(",
    "ElementId",
    "Button::new(",
    "KeyBinding::new",
    "Keystroke::parse",
    "actions!",
    "#[",
    "contains(",
    "starts_with(",
    "strip_prefix(",
    "ends_with(",
    "matches!(",
    "recorded(",
    "note_read(",
    "count_render(",
    "internal(",
];

/// Literals that read as words and are not interface text in a language, with why.
const ALLOWED: [(&str, &str); 11] = [
    ("Sakura", "the product's name, the same in every language"),
    ("English", "a language's own name, never translated"),
    ("Italiano", "a language's own name, never translated"),
    ("Geist", "a font family"),
    ("Geist Mono", "a font family"),
    ("Symbols Nerd Font Mono", "a font family"),
    ("Symbols Nerd Font", "a font family"),
    ("Nerd Font", "what the name of such a font family holds"),
    (
        "Library/Application Support/dev.yarin.sakura",
        "a folder on disk",
    ),
    (
        "{minutes} min {:02} s",
        "unit symbols, the same in every language",
    ),
    (
        "(exit {INTERRUPTED_EXIT_CODE})",
        "compared with what `box` printed, never shown",
    ),
];

/// The words a box follows when it is the place a task works in ("in a box", "nei box"). With none of
/// them before it, "box" would be the command behind the app, which the interface never names.
const BEFORE_A_BOX: [&str; 7] = ["a", "the", "un", "una", "la", "le", "nei"];

#[test]
fn no_value_names_the_command_behind_it() {
    for (language, text) in [("English", ENGLISH), ("Italian", ITALIAN)] {
        for (key, words) in locale(text) {
            let is_word = |c: char| c.is_alphanumeric();
            let said: Vec<&str> = words.split(|c| !is_word(c)).collect();
            let boxes = said.iter().enumerate().filter(|(_, word)| **word == "box");
            for (nth, _) in boxes {
                let before = nth.checked_sub(1).map(|nth| said[nth].to_lowercase());
                let is_a_place = before.is_some_and(|word| BEFORE_A_BOX.contains(&word.as_str()));
                let is_a_command = said.get(nth + 1) == Some(&"command");
                assert!(
                    is_a_place && !is_a_command,
                    "{key} names box as a command in {language}: {words}"
                );
            }
        }
    }
}

#[test]
fn installation_guidance_names_the_public_product() {
    for text in [ENGLISH, ITALIAN] {
        let words = locale(text);
        assert!(words["blocked.install_before"].contains("Sakura"));
    }
}

/// Literals shaped like a key in a family of the locale files that are not keys, with why.
const NOT_KEYS: [(&str, &str); 1] = [("console.anthropic.com", "a host of the sign-in links")];

/// The keys and values of a locale file: one `"key": "value"` a line.
fn locale(text: &str) -> BTreeMap<String, String> {
    let entries = text.lines().filter(|line| line.starts_with('"'));
    entries
        .map(|line| {
            let entry: BTreeMap<String, String> =
                serde_json::from_str(&format!("{{{line}}}")).expect("a locale line");
            entry.into_iter().next().expect("one entry a line")
        })
        .collect()
}

/// The names of the values a sentence is filled with: `%{name}`.
fn placeholders(value: &str) -> BTreeSet<String> {
    let parts = value.split("%{").skip(1);
    parts
        .filter_map(|part| part.split_once('}'))
        .map(|(name, _)| name.to_string())
        .collect()
}

/// Every `.rs` file at or under `path` that ships: no test file.
fn shipped(path: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(path) else {
        return Vec::new();
    };
    let is_test = |path: &Path| {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        name == "tests.rs" || name.ends_with("_tests.rs") || name == "fixtures.rs"
    };
    entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .flat_map(|path| match path.is_dir() {
            true => shipped(&path),
            false if path.extension().is_some_and(|e| e == "rs") && !is_test(&path) => vec![path],
            false => Vec::new(),
        })
        .collect()
}

/// The code of a source that ships: what stands before its test module.
fn shipped_code(path: &Path) -> String {
    let source = std::fs::read_to_string(path).expect("a source file");
    let code = source.split("#[cfg(test)]\nmod ").next();
    code.unwrap_or_default().to_string()
}

/// The string literals of one line of code, each with the code before it on the line; what follows a
/// `//` is left out.
fn literals(line: &str) -> Vec<(&str, String)> {
    let mut found = Vec::new();
    let mut letters = line.char_indices().peekable();
    let mut before = ' ';
    while let Some((at, letter)) = letters.next() {
        if letter == '/' && letters.peek().is_some_and(|(_, next)| *next == '/') {
            break;
        }
        if letter == '"' && before != '\'' {
            let mut text = String::new();
            while let Some((_, inside)) = letters.next() {
                match inside {
                    '\\' => drop(letters.next()),
                    '"' => break,
                    other => text.push(other),
                }
            }
            found.push((&line[..at], text));
        }
        before = letter;
    }
    found
}

/// Whether the literal after `code` is a key of the locale files.
fn is_key_call(code: &str) -> bool {
    code.ends_with("Key(") || code.ends_with("Said::new(")
}

/// Whether `text` has the shape of a key: lower-case words joined by dots.
fn is_key_shaped(text: &str) -> bool {
    let is_word = |word: &str| {
        word.starts_with(|c: char| c.is_ascii_lowercase())
            && word
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    };
    text.contains('.') && text.split('.').all(is_word)
}

/// The keys the shipped code names: the literals given to `Key` and `Said::new`, and every literal
/// shaped like a key in a family the locale files have (keys are also handed over as plain literals).
fn keys_in_code(families: &BTreeSet<&str>) -> BTreeSet<String> {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let files = KEY_SOURCES
        .iter()
        .flat_map(|source| shipped(&crates.join(source)));
    let mut keys = BTreeSet::new();
    for file in files {
        let code = shipped_code(&file);
        let lines = code
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"));
        for (before, text) in lines.flat_map(literals) {
            let family = text.split('.').next().unwrap_or_default();
            let is_named_key = is_key_shaped(&text)
                && families.contains(family)
                && !NOT_KEYS.iter().any(|(literal, _)| *literal == text);
            if is_key_call(before) || is_named_key {
                keys.insert(text);
            }
        }
    }
    keys
}

/// Whether `text` reads as words for a person: a word of two letters or more outside its braces, and a
/// space or a capital first letter.
fn reads_as_words(text: &str) -> bool {
    let mut shown = String::new();
    let mut depth = 0;
    for letter in text.chars() {
        match letter {
            '{' => depth += 1,
            '}' if depth > 0 => depth -= 1,
            other if depth == 0 => shown.push(other),
            _ => {}
        }
    }
    let mut words = shown.split(|c: char| !c.is_alphabetic());
    let has_word = words.any(|word| word.chars().count() >= 2);
    let mut first = text.trim().chars();
    let is_capitalised = first.next().is_some_and(char::is_uppercase)
        && first.next().is_some_and(char::is_lowercase);
    has_word && (text.trim().contains(' ') || is_capitalised)
}

/// The literals of one source that read as words and are neither keys nor allowed, as "file:line: text".
fn words_outside_the_locales(path: &Path) -> Vec<String> {
    let code = shipped_code(path);
    let is_shown = |line: &str| !NOT_SHOWN.iter().any(|mark| line.contains(mark));
    let mut found = Vec::new();
    let mut previous = "";
    for (number, line) in code.lines().enumerate() {
        // A literal on its own line belongs to the call the line before opened.
        let is_continuation = line.trim_start().starts_with('"');
        let is_comment = line.trim_start().starts_with("//");
        if !is_comment && is_shown(line) && (!is_continuation || is_shown(previous)) {
            for (before, text) in literals(line) {
                let is_allowed = ALLOWED.iter().any(|(literal, _)| *literal == text);
                if reads_as_words(&text) && !is_key_call(before) && !is_allowed {
                    found.push(format!("{}:{}: {text}", path.display(), number + 1));
                }
            }
        }
        if !line.trim().is_empty() {
            previous = line;
        }
    }
    found
}

#[test]
fn both_locales_hold_the_same_keys_with_words_and_the_same_values_to_fill_in() {
    let (english, italian) = (locale(ENGLISH), locale(ITALIAN));
    assert!(english.len() > 300, "the English words were read");
    let keys = |locale: &BTreeMap<String, String>| locale.keys().cloned().collect::<Vec<_>>();
    assert_eq!(keys(&english), keys(&italian), "one set of keys");
    for (key, words) in &english {
        let other = &italian[key];
        assert!(!words.trim().is_empty(), "{key} says nothing in English");
        assert!(!other.trim().is_empty(), "{key} says nothing in Italian");
        assert_eq!(placeholders(words), placeholders(other), "{key}");
        assert!(!words.contains('!') && !other.contains('!'), "{key} shouts");
        let named = format!("{words} {other}").to_lowercase();
        assert!(
            !named.contains("agentbox"),
            "{key} names the tool behind box"
        );
    }
}

#[test]
fn a_state_reads_as_the_group_the_sidebar_lists_it_under() {
    for text in [ENGLISH, ITALIAN] {
        let words = locale(text);
        for (state, group) in [
            ("state.idle", "group.idle"),
            ("state.working", "group.working"),
            ("state.to_review", "group.review"),
        ] {
            assert_eq!(words[state], words[group], "{state} and {group}");
        }
    }
}

#[test]
fn the_code_names_no_key_the_locales_lack_and_the_locales_hold_no_key_the_code_never_names() {
    let english = locale(ENGLISH);
    let families: BTreeSet<&str> = english
        .keys()
        .filter_map(|key| key.split('.').next())
        .collect();
    let named = keys_in_code(&families);
    assert!(
        named.len() > 250,
        "the code's keys were found: {}",
        named.len()
    );
    // A key said with a count has two entries, `key.one` and `key.other`.
    let counted = |key: &str| ["one", "other"].map(|form| format!("{key}.{form}"));
    let is_in_locales = |key: &String| {
        english.contains_key(key) || counted(key).iter().all(|form| english.contains_key(form))
    };
    let missing: Vec<&String> = named.iter().filter(|key| !is_in_locales(key)).collect();
    assert!(missing.is_empty(), "keys with no words: {missing:#?}");
    let is_named = |key: &&String| {
        let base = key.rsplit_once('.').map(|(base, _)| base.to_string());
        named.contains(*key) || base.is_some_and(|base| named.contains(&base))
    };
    let unused: Vec<&String> = english.keys().filter(|key| !is_named(key)).collect();
    assert!(unused.is_empty(), "words no code asks for: {unused:#?}");
}

#[test]
fn no_string_a_person_reads_stands_outside_the_locale_files() {
    assert!(reads_as_words("Avvia agente") && reads_as_words("in corso"));
    assert!(reads_as_words("Start agent") && reads_as_words("{branch} from {base}"));
    assert!(!reads_as_words("toggle-sidebar") && !reads_as_words("new"));
    assert!(!reads_as_words("SAKURA_LANGUAGE") && !reads_as_words("{} {}"));
    assert!(is_key_shaped("sidebar.new_task.tooltip") && !is_key_shaped("New task"));
    let found = literals(r#"x(Key("a.b"), 'a', "due \"tre\"") // "no""#);
    assert_eq!(found[0], ("x(Key(", "a.b".to_string()));
    assert!(is_key_call(found[0].0) && !is_key_call(found[1].0));
    assert_eq!(found[1].1, "due tre");
    assert_eq!(found.len(), 2, "what follows `//` is a comment");
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let is_interface = |path: &PathBuf| {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        !NOT_INTERFACE.iter().any(|(file, _)| *file == name)
    };
    let files: Vec<PathBuf> = shipped(&source).into_iter().filter(is_interface).collect();
    assert!(files.len() > 80, "the sources were found: {}", files.len());
    let outside: Vec<String> = files
        .iter()
        .flat_map(|file| words_outside_the_locales(file))
        .collect();
    assert!(
        outside.is_empty(),
        "words outside the locale files: {outside:#?}"
    );
}

pub(super) fn width(cx: &mut VisualTestContext, selector: &'static str) -> f32 {
    f32::from(cx.debug_bounds(selector).expect(selector).size.width)
}

/// How wide `key`'s words are in the language in force, at `size`.
pub(super) fn width_of(cx: &mut VisualTestContext, key: Key, size: Pixels) -> f32 {
    cx.update(|window, _| text_width(window, tr(key), size, FontWeight::NORMAL))
}

/// The sidebar's first group and the header's state word show `language`'s words: each is as wide as
/// its words in that language, and the two languages' words are not equally long.
fn assert_drawn_in(cx: &mut VisualTestContext, language: Language) {
    assert_eq!(l10n::language(), language);
    let shown = [
        ("group-review", Key("group.review"), theme::TEXT_LABEL),
        ("header-state", Key("state.verified"), theme::TEXT_SMALL),
    ];
    for (selector, key, size) in shown {
        let (drawn, words) = (width(cx, selector), width_of(cx, key, size));
        assert!(
            (drawn - words).abs() < 1.0,
            "{selector} is {drawn} wide, its words in {} are {words}",
            language.name()
        );
    }
}

/// The names of the menus of the menu bar, and of the language choice with the one checked.
fn menu_bar(cx: &mut VisualTestContext) -> (Vec<String>, Vec<(String, bool)>) {
    let menus = cx.update(|_, cx| cx.get_menus()).unwrap_or_default();
    let names = menus.iter().map(|menu| menu.name.to_string()).collect();
    let submenu = |item: &OwnedMenuItem| match item {
        OwnedMenuItem::Submenu(languages) => Some(languages.items.clone()),
        _ => None,
    };
    let languages = menus
        .first()
        .and_then(|menu| menu.items.iter().find_map(submenu));
    let choices = languages.unwrap_or_default().into_iter();
    let choices = choices.filter_map(|item| match item {
        OwnedMenuItem::Action { name, checked, .. } => Some((name, checked)),
        _ => None,
    });
    (names, choices.collect())
}

fn open_on_a_verified_task(cx: &mut TestAppContext) -> (&mut VisualTestContext, Entity<Desk>) {
    l10n::set_for_test(Language::English);
    let (cx, desk, _) = open_with(cx, None);
    wait_for(cx, "the rows", |cx| {
        cx.debug_bounds("row-sample-accent").is_some()
    });
    super::interaction_tests::click(cx, "row-sample-accent");
    wait_for(cx, "the header", |cx| {
        cx.debug_bounds("header-state").is_some()
    });
    (cx, desk)
}

/// What the empty composer of the conversation says: words a field keeps between two drawings.
fn composer_placeholder(cx: &mut VisualTestContext, desk: &Entity<Desk>) -> String {
    desk.read_with(cx, |desk, cx| {
        let conversation = desk.conversation_of("sample-accent");
        conversation
            .expect("the conversation")
            .read(cx)
            .placeholder(cx)
    })
}

fn choose(cx: &mut VisualTestContext, choice: LanguageChoice) {
    cx.update(|_, cx| crate::language::set_choice(choice, cx));
    draw(cx);
}

#[gpui::test]
fn choosing_a_language_draws_the_sidebar_the_header_and_the_menu_bar_again_in_it_at_once(
    cx: &mut TestAppContext,
) {
    let (cx, desk) = open_on_a_verified_task(cx);
    assert_drawn_in(cx, Language::English);
    assert_eq!(composer_placeholder(cx, &desk), "Message the agent");
    let english = width(cx, "group-review");
    assert_eq!(menu_bar(cx).0, ["Sakura", "File", "Edit", "View", "Window"]);

    choose(cx, LanguageChoice::Is(Language::Italian));
    assert_drawn_in(cx, Language::Italian);
    assert_ne!(width(cx, "group-review"), english, "the words changed");
    assert_eq!(composer_placeholder(cx, &desk), "Scrivi all'agente");
    let (menus, choices) = menu_bar(cx);
    assert_eq!(menus, ["Sakura", "File", "Modifica", "Vista", "Finestra"]);
    let named = |name: &str, checked| (name.to_string(), checked);
    assert_eq!(
        choices,
        [
            named("Automatica", false),
            named("English", false),
            named("Italiano", true)
        ],
        "the three choices, languages by their own names, the one in force checked"
    );
    let kept = cx.update(|_, cx| crate::prefs::Preferences::current(cx).language);
    assert_eq!(kept, LanguageChoice::Is(Language::Italian));

    choose(cx, LanguageChoice::Is(Language::English));
    assert_drawn_in(cx, Language::English);
    assert_eq!(width(cx, "group-review"), english, "and back");
}

/// The window stops being the active one and becomes it again.
fn come_back(cx: &mut VisualTestContext) {
    cx.deactivate_window();
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
    draw(cx);
}

#[gpui::test]
fn the_automatic_choice_follows_the_mac_s_languages_when_the_window_becomes_active_and_an_explicit_one_does_not(
    cx: &mut TestAppContext,
) {
    let (cx, _) = open_on_a_verified_task(cx);
    let preferred = Rc::new(RefCell::new(vec!["en-GB".to_string()]));
    let read = preferred.clone();
    cx.update(|_, cx| crate::language::read_preferred_with(move || read.borrow().clone(), cx));
    come_back(cx);
    assert_drawn_in(cx, Language::English);
    assert_eq!(menu_bar(cx).1[0], ("Automatic".to_string(), true));

    // The language is changed in System Settings while Sakura is not the active app.
    *preferred.borrow_mut() = vec!["fr-FR".to_string(), "it-IT".to_string()];
    assert_eq!(
        l10n::language(),
        Language::English,
        "not before it is active"
    );
    come_back(cx);
    assert_drawn_in(cx, Language::Italian);
    assert_eq!(menu_bar(cx).0[4], "Finestra");

    choose(cx, LanguageChoice::Is(Language::English));
    come_back(cx);
    assert_drawn_in(cx, Language::English);

    // Back to the automatic choice: the Mac's list is in force at once.
    choose(cx, LanguageChoice::Automatic);
    assert_drawn_in(cx, Language::Italian);
    *preferred.borrow_mut() = vec!["de-DE".to_string()];
    come_back(cx);
    assert_drawn_in(cx, Language::English);
}
