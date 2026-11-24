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

#[cfg_attr(not(feature = "boxes"), allow(unused_imports))]
use crate::l10n::{self, tr, Key, Language, LanguageChoice};
#[cfg_attr(not(feature = "boxes"), allow(unused_imports))]
use crate::theme;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

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
const NOT_INTERFACE: [(&str, &str); 11] = [
    (
        "agent_log.rs",
        "diagnostics written to the agent startup log",
    ),
    (
        "panic_log.rs",
        "a developer log written to a file after a crash, never shown in the window",
    ),
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
        "drive_guard.rs",
        "a refusal printed to stderr for whoever sets the scripted steps' variable",
    ),
    (
        "event_kinds.rs",
        "the words of the engine's ledger lines a sentence is matched on, never shown",
    ),
    (
        "event_changes.rs",
        "the words of the engine's ledger lines a sentence is matched on, never shown",
    ),
    (
        "fixture_changes.rs",
        "the sample patch: invented data in git's format",
    ),
    (
        "login_item.rs",
        "a launch agent: a folder under the home and the property list the system reads",
    ),
];

/// A line with one of these is not interface text: it logs, asserts, names an element, binds a key,
/// declares an action, reads an attribute, matches what `box` printed, notes a call for the tests
/// (`recorded`, `note_read`), names a view for the render counter, or gives the technical details of an
/// internal failure, which are data under the details.
const NOT_SHOWN: [&str; 24] = [
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
    ".key_context(",
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
    "compile_error!",
];

/// Literals that read as words and are not interface text in a language, with why.
const ALLOWED: [(&str, &str); 37] = [
    ("cargo test -p orchard-search", "the catalogue's specimen command"),
    (
        "env CLAUDE_CONFIG_DIR={} {} auth login",
        "the terminal command for a detected login folder",
    ),
    (
        "CODEX_HOME='{home}' '{program}' login",
        "the shell line of the terminal window that runs Codex's own login: a command, not words",
    ),
    (
        "see sakura ",
        "the pointer to a command line of the engine's that is taken out of its text: matched, never shown",
    ),
    (
        "run sakura ",
        "the pointer to a command line of the engine's that is taken out of its text: matched, never shown",
    ),
    (
        "try sakura ",
        "the pointer to a command line of the engine's that is taken out of its text: matched, never shown",
    ),
    (
        "the path cannot be used",
        "the text of an I/O error written to the log, never shown",
    ),
    (
        "tell application Terminalactivatedo script {escaped}end tell",
        "the AppleScript that opens a terminal window: a program, not words",
    ),
    (
        "inbox && !Input",
        "a key binding's context: the Inbox, and not a field written in inside it",
    ),
    ("Expected:", "matches a check template's result line"),
    ("Morning summary", "matches the legacy name of the built-in digest"),
    ("Sakura", "the product's name, the same in every language"),
    (
        "Remove",
        "the engine's own label of its action that removes an account, compared with and never shown",
    ),
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
    (
        "the engine stopped on an error",
        "compared with what the engine wrote in its ledger, never shown",
    ),
    (
        "deferred (night):",
        "compared with what the engine wrote in its ledger, never shown",
    ),
    (
        "defer failed:",
        "compared with what the engine wrote in its ledger, never shown",
    ),
    (
        "start requested",
        "compared with what the engine wrote in its ledger, never shown",
    ),
    (
        "resume requested",
        "compared with what the engine wrote in its ledger, never shown",
    ),
    (
        "stop requested",
        "compared with what the engine wrote in its ledger, never shown",
    ),
    (
        "discard requested",
        "compared with what the engine wrote in its ledger, never shown",
    ),
    (
        " approved",
        "compared with what the engine wrote in its ledger, never shown",
    ),
    (
        " install: exit ",
        "compared with what the engine wrote in its ledger, never shown",
    ),
    (
        "cage denied",
        "compared with what the engine wrote in its ledger, never shown",
    ),
    (
        "opened",
        "compared with what the engine wrote in its ledger, never shown",
    ),
    (
        "ToolSearch",
        "the name of an agent's tool, compared with the title it gave a call, never shown",
    ),
    (
        "closed",
        "compared with what the engine wrote in its ledger, never shown",
    ),
    (
        "answered",
        "compared with what the engine wrote in its ledger, never shown",
    ),
];

/// The words a box follows when it is the place a task works in ("in a box", "nei box"). With none of
/// them before it, "box" would be the command behind the app, which the interface never names.
const BEFORE_A_BOX: [&str; 7] = ["a", "the", "un", "una", "la", "le", "nei"];

#[test]
fn locale_examples_use_public_variables_without_naming_the_service_command() {
    for (language, text) in [("English", ENGLISH), ("Italian", ITALIAN)] {
        for (key, words) in locale(text) {
            let is_word = |c: char| c.is_alphanumeric();
            let said: Vec<&str> = words.split(|c| !is_word(c)).collect();
            // Nor in what the app writes as an example: a branch prefix, a variable of a command.
            let is_unknown_variable = |word: &&str| {
                let name = word.to_ascii_lowercase();
                name.starts_with("sakura_")
                    && ![
                        "sakura_task",
                        "sakura_worktree",
                        "sakura_repo",
                        "sakura_base",
                        "sakura_port",
                        "sakura_ports",
                        "sakura_bin",
                        "sakura_acp_adapter",
                        "sakura_codex",
                    ]
                    .contains(&name.as_str())
            };
            let named: Vec<&str> = words.split(|c: char| !is_word(c) && c != '_').collect();
            assert!(
                !named.iter().any(is_unknown_variable),
                "{key} writes an unknown Sakura variable in {language}: {words}"
            );
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
const NOT_KEYS: [(&str, &str); 2] = [
    ("console.anthropic.com", "a host of the sign-in links"),
    ("agents.log", "the startup diagnostics filename"),
];

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
fn state_actions_and_idle_groups_have_their_own_words() {
    for (text, answer, review, idle, idle_group) in [
        (ENGLISH, "Answer", "Review", "Idle", "Idle"),
        (ITALIAN, "Rispondi", "Da rivedere", "Fermo", "Fermi"),
    ] {
        let words = locale(text);
        assert_eq!(words["state.needs_you"], answer);
        assert_eq!(words["state.to_review"], review);
        assert_eq!(words["state.idle"], idle);
        assert_eq!(words["group.idle"], idle_group);
        assert_ne!(words["state.needs_you"], words["group.needs_you"]);
        assert!(!words.contains_key("group.review"));
        assert!(!words.contains_key("group.not_started"));
        for (state, group) in [
            ("state.preparing", "group.preparing"),
            ("state.working", "group.working"),
        ] {
            assert_eq!(words[state], words[group]);
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

#[path = "l10n_tests/vocabulary_tests.rs"]
mod vocabulary;

#[cfg(feature = "boxes")]
#[path = "l10n_tests/window_tests.rs"]
mod window;
#[cfg(feature = "boxes")]
pub(in crate::ui) use window::{width, width_of};

/// The keys that carry a `%{count}` and read right for any count, as a unit or as a figure after the word.
const COUNT_WITHOUT_FORMS: [&str; 3] = [
    "master.size.mb",
    "master.size.kb",
    "settings_modal.connectors.added",
];

#[test]
fn a_sentence_that_counts_has_a_form_for_one_and_a_form_for_the_rest_in_both_languages() {
    for text in [ENGLISH, ITALIAN] {
        let words = locale(text);
        for (key, said) in &words {
            let family = key.strip_suffix(".other");
            if let Some(family) = family.filter(|_| said.contains("%{count}")) {
                assert!(
                    words.contains_key(&format!("{family}.one")),
                    "{key}: no one"
                );
            }
        }
        for (key, said) in &words {
            let is_form = key.ends_with(".one") || key.ends_with(".other");
            let is_exempt = COUNT_WITHOUT_FORMS.contains(&key.as_str());
            assert!(
                !said.contains("%{count}") || is_form || is_exempt,
                "{key} counts with one form: it reads '1 things'"
            );
        }
    }
}

#[test]
fn counted_words_agree_with_their_number_in_english_and_in_italian() {
    use crate::l10n::{set_for_test, tr_count, Key, Language};
    let hours = Key("engine_page.digest.hours");
    set_for_test(Language::English);
    assert_eq!(tr_count(hours, 1, &[]), "1 hour");
    assert_eq!(tr_count(hours, 24, &[]), "24 hours");
    set_for_test(Language::Italian);
    assert_eq!(tr_count(hours, 1, &[]), "1 ora");
    assert_eq!(tr_count(hours, 12, &[]), "12 ore");
    let batch = Key("inbox.answer.batch");
    set_for_test(Language::English);
    assert_eq!(tr_count(batch, 2, &[]), "2 actions queued");
    set_for_test(Language::Italian);
    assert_eq!(tr_count(batch, 3, &[]), "3 azioni in attesa");
    let approved = Key("events.contract_approved");
    assert_eq!(
        tr_count(approved, 1, &[("by", "Ada")]),
        "1 comando del contratto approvato Ada"
    );
}
