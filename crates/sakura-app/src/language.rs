//! Which language the window speaks, and when it changes. The choice is a preference: automatic (the
//! default), or one of the interface's languages. Automatic follows the Mac's ordered list of preferred
//! languages (`NSLocale.preferredLanguages`, which also carries the language chosen for this app alone in
//! System Settings), read at launch and again whenever the window becomes the active one, so a change
//! made there applies without a restart. A change of language is applied at once: the words of the
//! window, of the menu bar and of the kit's own components.

use crate::l10n::{self, choose, Language, LanguageChoice};
use crate::prefs::Preferences;
use gpui::{App, Global};
use std::rc::Rc;

/// Reads the Mac's preferred languages, most wanted first.
type Preferred = dyn Fn() -> Vec<String>;

/// Where the preferred languages are read from: the system, or a list a test gives.
struct SystemLanguages(Rc<Preferred>);

impl Global for SystemLanguages {}

/// A language requested this launch alone (a snapshot, a check): it stands in for the preference until
/// another is chosen, and is never saved.
struct ForThisLaunch(Language);

impl Global for ForThisLaunch {}

/// The choice in force: the language requested this launch, else the preference (the automatic one
/// before any preferences are installed).
pub fn choice(cx: &App) -> LanguageChoice {
    if let Some(asked) = cx.try_global::<ForThisLaunch>() {
        return LanguageChoice::Is(asked.0);
    }
    let preferences = Preferences::try_current(cx);
    preferences.map_or(LanguageChoice::Automatic, |preferences| {
        preferences.language
    })
}

/// Sets the language before the first window is built: the one `asked` for this launch, else the one
/// the `preference` gives on this Mac, whose list is read from the system.
pub fn start(preference: LanguageChoice, asked: Option<Language>, cx: &mut App) {
    cx.set_global(SystemLanguages(Rc::new(preferred_by_the_system)));
    if let Some(asked) = asked {
        cx.set_global(ForThisLaunch(asked));
    }
    let choice = asked.map_or(preference, LanguageChoice::Is);
    l10n::apply(choose(choice, &preferred(cx)));
}

/// Takes the Mac's preferred languages from `read` from now on (a test's own list).
#[cfg(test)]
pub fn read_preferred_with(read: impl Fn() -> Vec<String> + 'static, cx: &mut App) {
    cx.set_global(SystemLanguages(Rc::new(read)));
}

/// Changes the choice: it is kept for the next launch, and the language it gives is in force at once.
/// The one function the language choice calls, wherever it is shown.
pub fn set_choice(choice: LanguageChoice, cx: &mut App) {
    if cx.has_global::<ForThisLaunch>() {
        cx.remove_global::<ForThisLaunch>();
    }
    Preferences::change(cx, |preferences| preferences.language = choice);
    speak(choose(choice, &preferred(cx)), cx);
    // The menu checks the choice, which changed even when the language did not.
    crate::menus::rebuild(cx);
}

/// The window became the active one: with the automatic choice, the Mac's list is read again and the
/// language follows it. An explicit choice is left alone.
pub fn follow_the_system(cx: &mut App) {
    if choice(cx) == LanguageChoice::Automatic {
        speak(choose(LanguageChoice::Automatic, &preferred(cx)), cx);
    }
}

/// Puts `language` in force when it is not already: every window is drawn again and the menu bar built
/// again.
fn speak(language: Language, cx: &mut App) {
    if l10n::language() == language {
        return;
    }
    l10n::apply(language);
    cx.set_global(l10n::Spoken);
    crate::menus::rebuild(cx);
    cx.refresh_windows();
}

fn preferred(cx: &App) -> Vec<String> {
    cx.try_global::<SystemLanguages>()
        .map_or_else(Vec::new, |read| (read.0)())
}

/// `[NSLocale preferredLanguages]` as strings (`it-IT`, `en-GB`); empty when Foundation gives none.
#[cfg(target_os = "macos")]
fn preferred_by_the_system() -> Vec<String> {
    use std::ffi::{c_char, c_void, CStr};

    /// More languages than anyone lists; a bound on what is read.
    const MOST: usize = 64;

    #[link(name = "objc")]
    extern "C" {
        fn objc_getClass(name: *const c_char) -> *mut c_void;
        fn sel_registerName(name: *const c_char) -> *mut c_void;
        fn objc_msgSend();
    }
    type Get = unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void;
    type Count = unsafe extern "C" fn(*mut c_void, *mut c_void) -> usize;
    type At = unsafe extern "C" fn(*mut c_void, *mut c_void, usize) -> *mut c_void;
    type Utf8 = unsafe extern "C" fn(*mut c_void, *mut c_void) -> *const c_char;
    // SAFETY: the class and the selectors are looked up by their documented names and every object is
    // checked for null; `objc_msgSend` is called through the exact C signature of each method (a class
    // getter returning an NSArray, `count`, `objectAtIndex:` within the count, `UTF8String` of an
    // NSString, whose bytes are copied before anything else runs), on the main thread.
    unsafe {
        let class = objc_getClass(c"NSLocale".as_ptr());
        if class.is_null() {
            return Vec::new();
        }
        let send = objc_msgSend as unsafe extern "C" fn();
        let get: Get = std::mem::transmute(send);
        let list = get(class, sel_registerName(c"preferredLanguages".as_ptr()));
        if list.is_null() {
            return Vec::new();
        }
        let count: Count = std::mem::transmute(send);
        let at: At = std::mem::transmute(send);
        let utf8: Utf8 = std::mem::transmute(send);
        let total = count(list, sel_registerName(c"count".as_ptr())).min(MOST);
        let tag = |index: usize| {
            let string = at(list, sel_registerName(c"objectAtIndex:".as_ptr()), index);
            let bytes = (!string.is_null())
                .then(|| utf8(string, sel_registerName(c"UTF8String".as_ptr())))
                .filter(|bytes| !bytes.is_null())?;
            Some(CStr::from_ptr(bytes).to_string_lossy().into_owned())
        };
        (0..total).filter_map(tag).collect()
    }
}

/// The language of the session's locale (`LC_ALL`, `LC_MESSAGES`, `LANG`) as a tag: `it_IT.UTF-8` is
/// `it-IT`. Empty when none is set or it is the plain `C` locale.
#[cfg(not(target_os = "macos"))]
fn preferred_by_the_system() -> Vec<String> {
    let named = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|name| std::env::var(name).ok())
        .find(|value| !value.is_empty());
    named
        .as_deref()
        .and_then(tag_of_locale)
        .into_iter()
        .collect()
}

/// A POSIX locale name as a language tag, without its encoding and modifier.
#[cfg(any(test, not(target_os = "macos")))]
fn tag_of_locale(locale: &str) -> Option<String> {
    let name = locale.split(['.', '@']).next().unwrap_or_default();
    let is_language = name.len() >= 2
        && name != "POSIX"
        && name.chars().all(|c| c.is_ascii_alphabetic() || c == '_');
    is_language.then(|| name.replace('_', "-"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn a_language_asked_for_one_launch_stands_until_another_is_chosen_and_is_not_saved(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            l10n::set_for_test(Language::English);
            Preferences::default().install(None, cx);
            start(LanguageChoice::Automatic, Some(Language::Italian), cx);
            read_preferred_with(|| vec!["en-US".to_string()], cx);
            assert_eq!(l10n::language(), Language::Italian);
            follow_the_system(cx);
            assert_eq!(
                l10n::language(),
                Language::Italian,
                "the Mac's list is not asked"
            );
            assert_eq!(choice(cx), LanguageChoice::Is(Language::Italian));
            let saved = Preferences::current(cx).language;
            assert_eq!(
                saved,
                LanguageChoice::Automatic,
                "the preference is untouched"
            );
            set_choice(LanguageChoice::Automatic, cx);
            assert_eq!(
                l10n::language(),
                Language::English,
                "the choice made in the app wins"
            );
            assert_eq!(choice(cx), LanguageChoice::Automatic);
        });
    }

    #[test]
    fn a_locale_name_becomes_a_language_tag() {
        assert_eq!(tag_of_locale("it_IT.UTF-8").as_deref(), Some("it-IT"));
        assert_eq!(tag_of_locale("en_GB@euro").as_deref(), Some("en-GB"));
        assert_eq!(tag_of_locale("de").as_deref(), Some("de"));
        assert_eq!(tag_of_locale("C"), None);
        assert_eq!(tag_of_locale("POSIX"), None);
        assert_eq!(tag_of_locale(""), None);
    }

    #[test]
    fn the_mac_gives_a_list_of_language_tags_or_none() {
        // Whatever this Mac prefers: every entry is a tag, and reading it twice gives the same.
        let preferred = preferred_by_the_system();
        assert!(preferred
            .iter()
            .all(|tag| !tag.is_empty() && tag.len() < 32));
        assert_eq!(preferred, preferred_by_the_system());
    }
}
