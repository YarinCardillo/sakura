//! The interface's language. Every word the window shows comes from the locale files of this crate
//! (`locales/en.yml`, `locales/it.yml`) through here: a view holds a [`Key`], never the words, and the
//! core hands over things to say ([`Said`]) that are worded here. The mechanism is `rust-i18n`, the one
//! gpui-component uses for its own strings, so one switch changes the kit's built-in words with ours.
//! English is the default.
//!
//! To add a string: add its key to both locale files (named by place and meaning, never by the English
//! words), and show it with [`tr`]. A sentence with a number takes a key with `.one` and `.other` and is
//! shown with [`plural`], or through a [`Said`] with a count; `ui/l10n_tests.rs` fails on a key missing
//! from a file, on a key nothing uses and on a literal in a view that is not a key.

use gpui::SharedString;
use sakura_core::said::{Value, COUNT};
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Mutex;

pub use sakura_core::said::{Key, Said};

/// A language of the interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Language {
    #[default]
    English,
    Italian,
}

impl Language {
    /// Every language, in the order the choice lists them.
    pub const ALL: [Language; 2] = [Language::English, Language::Italian];

    /// The locale's code, as the locale files and the preferences write it.
    pub fn code(self) -> &'static str {
        match self {
            Language::English => "en",
            Language::Italian => "it",
        }
    }

    /// The language of a code; `None` for one the interface does not have.
    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|language| language.code() == code)
    }

    /// The language's own name: the names of languages are not translated.
    pub fn name(self) -> &'static str {
        match self {
            Language::English => "English",
            Language::Italian => "Italiano",
        }
    }

    /// The mark between a number's whole part and its decimals.
    fn decimal_mark(self) -> char {
        match self {
            Language::English => '.',
            Language::Italian => ',',
        }
    }

    /// The mark between groups of three digits.
    fn group_mark(self) -> char {
        match self {
            Language::English => ',',
            Language::Italian => '.',
        }
    }
}

/// What the person chose for the language: the Mac's own, or one of ours.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LanguageChoice {
    /// The first of the Mac's preferred languages that the interface has; English when it has none.
    #[default]
    Automatic,
    Is(Language),
}

impl LanguageChoice {
    /// The choice as the preferences write it; nothing for the automatic one.
    pub fn code(self) -> Option<&'static str> {
        match self {
            LanguageChoice::Automatic => None,
            LanguageChoice::Is(language) => Some(language.code()),
        }
    }

    /// The choice the preferences hold: a code the interface does not have is the automatic one.
    pub fn from_code(code: Option<&str>) -> Self {
        let language = code.and_then(Language::from_code);
        language.map_or(LanguageChoice::Automatic, LanguageChoice::Is)
    }
}

/// Set again each time the language changes, for what keeps words of its own between two drawings (a
/// field's placeholder): it watches this and asks for its words again.
pub struct Spoken;

impl gpui::Global for Spoken {}

/// The language for `choice` on a Mac whose preferred languages are `preferred`, most wanted first
/// (`it-IT`, `en-GB`: the language is what stands before the first hyphen). The automatic choice takes the
/// first one the interface has, and English when it has none of them.
pub fn choose(choice: LanguageChoice, preferred: &[String]) -> Language {
    let of = |tag: &String| {
        let language = tag.split(['-', '_']).next().unwrap_or_default();
        Language::from_code(&language.to_lowercase())
    };
    match choice {
        LanguageChoice::Is(language) => language,
        LanguageChoice::Automatic => preferred.iter().find_map(of).unwrap_or_default(),
    }
}

#[cfg(test)]
thread_local! {
    /// The language of the test running on this thread: tests run side by side in one process, so none
    /// of them may change the language of the others.
    static IN_TEST: std::cell::Cell<Option<Language>> = const { std::cell::Cell::new(None) };
}

/// The language in force.
pub fn language() -> Language {
    #[cfg(test)]
    if let Some(language) = IN_TEST.with(std::cell::Cell::get) {
        return language;
    }
    Language::from_code(&rust_i18n::locale()).unwrap_or_default()
}

/// Sets the language for the test on this thread only.
#[cfg(test)]
pub fn set_for_test(language: Language) {
    IN_TEST.with(|cell| cell.set(Some(language)));
}

/// Sets the language of this process, ours and the kit's. It tells nobody: the window's words follow when
/// it is drawn again and the menu bar when it is rebuilt (`language::set_choice` does both). In a test
/// only the test's own thread changes language.
pub fn apply(language: Language) {
    #[cfg(test)]
    set_for_test(language);
    #[cfg(not(test))]
    rust_i18n::set_locale(language.code());
}

/// The words of `key` in the language in force. They are kept for the life of the app once requested (a
/// few hundred short strings in each language), so a view can hold them like any other fixed text; what
/// must follow a change of language is asked again when the view is drawn.
pub fn tr(key: Key) -> &'static str {
    static KEPT: Mutex<Option<HashMap<(Language, &'static str), &'static str>>> = Mutex::new(None);
    let language = language();
    let mut kept = KEPT.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let kept = kept.get_or_insert_with(HashMap::new);
    kept.entry((language, key.0))
        .or_insert_with(|| Box::leak(words(key.0).into_owned().into_boxed_str()))
}

/// The words of `key` with each `%{name}` replaced by its value.
pub fn tr_with(key: Key, values: &[(&str, &str)]) -> SharedString {
    let mut text = words(key.0).into_owned();
    for (name, value) in values {
        text = text.replace(&format!("%{{{name}}}"), value);
    }
    text.into()
}

/// The words of what the core has to say: its sentence in the language in force, numbers written this
/// language's way, the singular or the plural by its count, and every text value shown as a label.
pub fn say(said: &Said) -> SharedString {
    let key = match said.arg(COUNT) {
        Some(Value::Count(count)) => plural_key(said.key.0, *count),
        _ => said.key.0.to_string(),
    };
    let mut text = words(&key).into_owned();
    for (name, value) in &said.args {
        text = text.replace(&format!("%{{{name}}}"), &shown(value));
    }
    text.into()
}

/// A whole number with its groups of three digits marked this language's way: `1,234` or `1.234`.
pub fn grouped(value: u64) -> String {
    let digits = value.to_string();
    let mark = language().group_mark();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(mark);
        }
        out.push(digit);
    }
    out
}

/// Added and removed lines as two figures: `+612` and `−48`, with the real minus sign so that both align
/// in tabular figures.
pub fn signed_lines(insertions: u32, deletions: u32) -> (String, String) {
    const MINUS: char = '\u{2212}';
    (
        format!("+{}", grouped(u64::from(insertions))),
        format!("{MINUS}{}", grouped(u64::from(deletions))),
    )
}

fn plural_key(key: &str, count: u64) -> String {
    let form = if count == 1 { "one" } else { "other" };
    format!("{key}.{form}")
}

/// A value of a sentence as it is written.
fn shown(value: &Value) -> String {
    match value {
        Value::Text(text) => sakura_core::display::label(text),
        Value::Count(count) => grouped(*count),
        Value::Decimal(number) => number.replace('.', &language().decimal_mark().to_string()),
        Value::Said(said) => say(said).to_string(),
    }
}

/// The words of `key`, or the key itself when no locale has it (the tests keep that from happening).
fn words(key: &str) -> Cow<'static, str> {
    words_on(key, sakura_core::platform::System::current())
}

const COMPUTER_KEYS: [(&str, Key); 12] = [
    (
        "box.failure.not_owned",
        Key("box.failure.not_owned.computer"),
    ),
    ("new_task.place.mac", Key("new_task.place.computer")),
    (
        "publish.tip.after_take",
        Key("publish.tip.after_take.computer"),
    ),
    ("settings.accounts.mac", Key("settings.accounts.computer")),
    (
        "settings.accounts.mac.login",
        Key("settings.accounts.computer.login"),
    ),
    (
        "settings.accounts.mac.missing",
        Key("settings.accounts.computer.missing"),
    ),
    (
        "settings.project.place.mac",
        Key("settings.project.place.computer"),
    ),
    ("thread.login_expired", Key("thread.login_expired.computer")),
    (
        "thread.login_expired_hint",
        Key("thread.login_expired_hint.computer"),
    ),
    (
        "thread.login_expired_unnamed",
        Key("thread.login_expired_unnamed.computer"),
    ),
    (
        "thread.no_account_at_all",
        Key("thread.no_account_at_all.computer"),
    ),
    ("verify.ran_on_mac", Key("verify.ran_on_computer")),
];

fn platform_key_on(key: &str, system: sakura_core::platform::System) -> &str {
    if system == sakura_core::platform::System::Mac {
        return key;
    }
    COMPUTER_KEYS
        .iter()
        .find_map(|(original, computer)| (*original == key).then_some(computer.0))
        .unwrap_or(key)
}

fn words_on(key: &str, system: sakura_core::platform::System) -> Cow<'static, str> {
    let key = platform_key_on(key, system);
    let found = crate::_rust_i18n_try_translate(language().code(), key);
    found.unwrap_or_else(|| Cow::Owned(key.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_labels_name_the_computer_on_linux_and_windows_and_keep_mac_on_macos() {
        use sakura_core::platform::System;
        let keys = [
            "box.failure.not_owned",
            "new_task.place.mac",
            "thread.login_expired",
            "thread.login_expired_hint",
            "thread.login_expired_unnamed",
            "thread.no_account_at_all",
            "verify.ran_on_mac",
            "settings.accounts.mac",
            "settings.accounts.mac.login",
            "settings.accounts.mac.missing",
            "settings.project.place.mac",
            "publish.tip.after_take",
        ];
        for language in Language::ALL {
            in_language(language, || {
                for key in keys {
                    assert!(words_on(key, System::Mac).contains("Mac"));
                    for system in [System::Linux, System::Windows] {
                        let text = words_on(key, system);
                        assert!(!text.contains("Mac"), "{key}: {text}");
                        assert_ne!(text, platform_key_on(key, system));
                    }
                }
            });
        }
        in_language(Language::English, || {
            assert_eq!(
                words_on("new_task.place.mac", System::Linux),
                "On this computer"
            );
        });
        in_language(Language::Italian, || {
            assert_eq!(
                words_on("new_task.place.mac", System::Windows),
                "Su questo computer"
            );
        });
    }

    fn in_language<T>(language: Language, read: impl FnOnce() -> T) -> T {
        set_for_test(language);
        read()
    }

    #[test]
    fn english_is_the_language_when_none_was_chosen() {
        assert_eq!(Language::default(), Language::English);
        assert_eq!(Language::from_code("it"), Some(Language::Italian));
        assert_eq!(Language::from_code("fr"), None);
        assert_eq!(Language::Italian.name(), "Italiano", "never translated");
    }

    #[test]
    fn the_automatic_choice_takes_the_first_preferred_language_the_interface_has() {
        let on = |preferred: &[&str]| {
            let preferred: Vec<String> = preferred.iter().map(|tag| tag.to_string()).collect();
            choose(LanguageChoice::Automatic, &preferred)
        };
        assert_eq!(on(&["it-IT", "en-US"]), Language::Italian);
        assert_eq!(on(&["en-GB", "it-IT"]), Language::English);
        assert_eq!(on(&["fr-FR", "it-IT"]), Language::Italian);
        assert_eq!(on(&["fr-FR"]), Language::English);
        assert_eq!(on(&[]), Language::English);
        assert_eq!(on(&["IT"]), Language::Italian, "a bare code, in any case");
    }

    #[test]
    fn an_explicit_choice_overrides_the_mac_s_languages() {
        let italian_mac = ["it-IT".to_string()];
        let english = LanguageChoice::Is(Language::English);
        assert_eq!(choose(english, &italian_mac), Language::English);
        let italian = LanguageChoice::Is(Language::Italian);
        assert_eq!(choose(italian, &["en-US".to_string()]), Language::Italian);
        assert_eq!(LanguageChoice::from_code(None), LanguageChoice::Automatic);
        assert_eq!(LanguageChoice::from_code(Some("it")), italian);
        assert_eq!(
            LanguageChoice::from_code(Some("fr")),
            LanguageChoice::Automatic
        );
        assert_eq!(italian.code(), Some("it"));
        assert_eq!(LanguageChoice::Automatic.code(), None);
    }

    #[test]
    fn a_key_is_said_in_the_language_in_force() {
        let key = Key("common.cancel");
        assert_eq!(in_language(Language::English, || tr(key)), "Cancel");
        assert_eq!(in_language(Language::Italian, || tr(key)), "Annulla");
    }

    #[test]
    fn one_thing_and_several_have_their_own_words() {
        let waiting = |count| say(&Said::new("runner.waiting").count(count));
        set_for_test(Language::Italian);
        assert_eq!(waiting(1).as_ref(), "1 verifica in attesa");
        assert_eq!(waiting(3).as_ref(), "3 verifiche in attesa");
        set_for_test(Language::English);
        assert_eq!(waiting(1).as_ref(), "1 check waiting");
        assert_eq!(waiting(3).as_ref(), "3 checks waiting");
        let tests = |count| say(&Said::new("verify.tests").count(count));
        assert_eq!(tests(1).as_ref(), "1 test");
        assert_eq!(tests(7880).as_ref(), "7,880 tests");
    }

    #[test]
    fn numbers_are_written_the_language_s_way() {
        let load = Said::new("machine.load")
            .tenths("load", 10.44)
            .number("cores", 10);
        set_for_test(Language::Italian);
        assert_eq!(say(&load).as_ref(), "Carico 10,4 su 10 core");
        assert_eq!(grouped(1_234_567), "1.234.567");
        assert_eq!(
            signed_lines(1612, 48),
            ("+1.612".into(), "\u{2212}48".into())
        );
        set_for_test(Language::English);
        assert_eq!(say(&load).as_ref(), "Load 10.4 on 10 cores");
        assert_eq!(grouped(1_234_567), "1,234,567");
    }

    #[test]
    fn a_sentence_inside_another_and_a_name_that_hides_are_said_whole() {
        set_for_test(Language::Italian);
        let limit = Said::new("limit.minutes").count(45);
        let late = Said::new("error.box.timeout").said("limit", limit);
        assert_eq!(
            say(&late).as_ref(),
            "Il servizio non ha risposto entro 45 minuti."
        );
        let hostile = Said::new("trust.changes_checks.only").text("file", "a\u{202e}b\nc.ts");
        assert_eq!(
            say(&hostile).as_ref(),
            "Questo commit cambia i controlli: a\u{fffd}b\u{fffd}c.ts"
        );
    }
}
