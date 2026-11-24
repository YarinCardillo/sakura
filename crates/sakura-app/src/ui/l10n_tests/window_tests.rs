//! The tests of the interface's words that open the window: a change of language is drawn at once.

use super::*;
use crate::ui::chrome::text_width;
use crate::ui::desk::Desk;
use crate::ui::interaction_tests::{draw, open_with, wait_for};
use gpui::{Entity, FontWeight, OwnedMenuItem, Pixels, TestAppContext, VisualTestContext};
use std::cell::RefCell;
use std::rc::Rc;

pub(in crate::ui) fn width(cx: &mut VisualTestContext, selector: &'static str) -> f32 {
    f32::from(cx.debug_bounds(selector).expect(selector).size.width)
}

/// How wide `key`'s words are in the language in force, at `size`.
pub(in crate::ui) fn width_of(cx: &mut VisualTestContext, key: Key, size: Pixels) -> f32 {
    cx.update(|window, _| text_width(window, tr(key), size, FontWeight::NORMAL))
}

/// The sidebar's first group and the header's state word show `language`'s words: each is as wide as
/// its words in that language, and the two languages' words are not equally long.
fn assert_drawn_in(cx: &mut VisualTestContext, language: Language) {
    assert_eq!(l10n::language(), language);
    let shown = [
        ("group-needs-you", Key("group.needs_you"), theme::TEXT_LABEL),
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
    crate::ui::interaction_tests::click(cx, "row-sample-accent");
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
    let english = width(cx, "group-needs-you");
    assert_eq!(menu_bar(cx).0, ["Sakura", "File", "Edit", "View", "Window"]);

    choose(cx, LanguageChoice::Is(Language::Italian));
    assert_drawn_in(cx, Language::Italian);
    assert_ne!(width(cx, "group-needs-you"), english, "the words changed");
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
    assert_eq!(width(cx, "group-needs-you"), english, "and back");
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
