//! The standard macOS menus: the app menu with Settings, the language choice and Quit, View with what
//! the lavoro on screen shows and runs, Edit with the clipboard
//! commands (the kit's own actions, so its text selection and inputs answer them), File with the new task
//! and the tab commands, and the window commands with the two panes that hide. `Minimize`, `Zoom`, `NewTask`
//! the pane toggles and the status bar's terminal are handled by the app for the window (see
//! `ui::desk_panes`). The menu bar is
//! built again when a pane hides or comes back and when the language changes.

use crate::l10n::{tr, Key, Language, LanguageChoice};
use crate::ui::settings::Shortcut;
use crate::ui::tabs;
use gpui::{actions, App, Global, KeyBinding, Keystroke, Menu, MenuItem, OsAction, SystemMenuType};
use gpui_component::input::{Copy, Cut, Paste, Redo, SelectAll, Undo};

actions!(
    sakura,
    [
        Quit,
        Hide,
        HideOthers,
        ShowAll,
        Minimize,
        Zoom,
        ToggleSidebar,
        ToggleChanges,
        SwitchTerminal,
        ToggleFollow,
        NewTask,
        ShowSettings,
        StartPlay,
        StopPlay,
        LanguageAutomatic,
        LanguageEnglish,
        LanguageItalian
    ]
);

/// What starts a new task, on the menu item, the sidebar's button and the dialog's title.
pub const NEW_TASK: Key = Key("menu.file.new_task");

/// Which of the panes that hide are shown: the menu names what a press would do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PanesShown {
    pub sidebar: bool,
    pub changes: bool,
}

/// Installs the menu bar, its key bindings and the app-level handlers.
pub fn install(cx: &mut App) {
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    cx.on_action(|_: &ShowSettings, cx| drop(crate::ui::settings::host::show(cx)));
    let choose = crate::language::set_choice;
    cx.on_action(move |_: &LanguageAutomatic, cx| choose(LanguageChoice::Automatic, cx));
    cx.on_action(move |_: &LanguageEnglish, cx| choose(LanguageChoice::Is(Language::English), cx));
    cx.on_action(move |_: &LanguageItalian, cx| choose(LanguageChoice::Is(Language::Italian), cx));
    cx.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-,", ShowSettings, None),
        KeyBinding::new("cmd-h", Hide, None),
        KeyBinding::new("alt-cmd-h", HideOthers, None),
        KeyBinding::new("cmd-m", Minimize, None),
        KeyBinding::new("cmd-n", NewTask, None),
        KeyBinding::new("cmd-r", StartPlay, None),
        KeyBinding::new("cmd-.", StopPlay, None),
        KeyBinding::new("ctrl-`", SwitchTerminal, None),
        KeyBinding::new("shift-cmd-f", ToggleFollow, None),
        KeyBinding::new("cmd-b", ToggleSidebar, None),
        KeyBinding::new("alt-cmd-b", ToggleChanges, None),
        KeyBinding::new("cmd-x", Cut, None),
        KeyBinding::new("cmd-c", Copy, None),
        KeyBinding::new("cmd-v", Paste, None),
        KeyBinding::new("cmd-a", SelectAll, None),
        KeyBinding::new("cmd-z", Undo, None),
        KeyBinding::new("shift-cmd-z", Redo, None),
    ]);
    let both = PanesShown {
        sidebar: true,
        changes: true,
    };
    show_panes(both, cx);
}

/// The panes the menu bar was last built for: it is built again for them when the language changes.
struct Shown(PanesShown);

impl Global for Shown {}

/// Sets the menu bar for the panes shown now.
pub fn show_panes(panes: PanesShown, cx: &mut App) {
    cx.set_global(Shown(panes));
    rebuild(cx);
}

/// Builds the menu bar again, in the language in force and for the panes it was last built for.
pub fn rebuild(cx: &mut App) {
    let both = PanesShown {
        sidebar: true,
        changes: true,
    };
    let panes = cx.try_global::<Shown>().map_or(both, |shown| shown.0);
    cx.set_menus(menus(panes, cx));
}

/// The menu bar, in the language in force and for the `panes` shown.
fn menus(panes: PanesShown, cx: &App) -> Vec<Menu> {
    let choice = crate::language::choice(cx);
    vec![
        app_menu(choice),
        file_menu(),
        edit_menu(),
        view_menu(),
        window_menu(panes),
    ]
}

/// Every command of the menu bar that has a key, in the menu bar's order: what "Shortcuts" in
/// Settings lists. It is read from the menus and the key bindings themselves, so it cannot say
/// anything they do not do.
pub fn shortcuts(cx: &App) -> Vec<Shortcut> {
    let both = PanesShown {
        sidebar: true,
        changes: true,
    };
    let panes = cx.try_global::<Shown>().map_or(both, |shown| shown.0);
    let mut found = Vec::new();
    for menu in menus(panes, cx) {
        collect_shortcuts(&menu, cx, &mut found);
    }
    found
}

/// Adds the commands of `menu` that have a key to `found`, those of its submenus under their own name.
fn collect_shortcuts(menu: &Menu, cx: &App, found: &mut Vec<Shortcut>) {
    for item in &menu.items {
        match item {
            MenuItem::Submenu(inner) => collect_shortcuts(inner, cx, found),
            MenuItem::Action { name, action, .. } => {
                let keymap = cx.key_bindings();
                let keymap = keymap.borrow();
                let binding = keymap.bindings_for_action(action.as_ref()).last();
                let keys = binding.map(|binding| {
                    let strokes = binding.keystrokes().iter();
                    let strokes: Vec<String> = strokes.map(|key| keys_of(key.inner())).collect();
                    strokes.join(" ")
                });
                if let Some(keys) = keys {
                    found.push(Shortcut {
                        menu: menu.name.clone(),
                        action: name.clone(),
                        keys: keys.into(),
                    });
                }
            }
            MenuItem::Separator | MenuItem::SystemMenu(_) => {}
        }
    }
}

/// A key as the Mac's menus write it: the modifiers in the Mac's order, then the key.
fn keys_of(stroke: &Keystroke) -> String {
    let modifiers = [
        (stroke.modifiers.control, '⌃'),
        (stroke.modifiers.alt, '⌥'),
        (stroke.modifiers.shift, '⇧'),
        (stroke.modifiers.platform, '⌘'),
    ];
    let mut keys: String = modifiers
        .into_iter()
        .filter_map(|(is_held, sign)| is_held.then_some(sign))
        .collect();
    keys.push_str(&stroke.key.to_uppercase());
    keys
}

/// What the sidebar's toggle would do, in the menu and in its tooltip.
pub fn sidebar_label(is_shown: bool) -> Key {
    if is_shown {
        Key("menu.window.hide_sidebar")
    } else {
        Key("menu.window.show_sidebar")
    }
}

/// What the right pane's toggle would do, in the menu and in its tooltip.
pub fn changes_label(is_shown: bool) -> Key {
    if is_shown {
        Key("menu.window.hide_pane")
    } else {
        Key("menu.window.show_pane")
    }
}

fn app_menu(choice: LanguageChoice) -> Menu {
    Menu::new(crate::window::APP_NAME).items([
        MenuItem::action(
            tr(crate::ui::brand::about::TITLE),
            crate::ui::brand::about::About,
        ),
        MenuItem::separator(),
        MenuItem::action(tr(Key("menu.app.settings")), ShowSettings),
        MenuItem::submenu(language_menu(choice)),
        MenuItem::separator(),
        MenuItem::os_submenu(tr(Key("menu.app.services")), SystemMenuType::Services),
        MenuItem::separator(),
        MenuItem::action(tr(Key("menu.app.hide")), Hide),
        MenuItem::action(tr(Key("menu.app.hide_others")), HideOthers),
        MenuItem::action(tr(Key("menu.app.show_all")), ShowAll),
        MenuItem::separator(),
        MenuItem::action(tr(Key("menu.app.quit")), Quit),
    ])
}

/// The language choice, the one in force checked. The names of languages are their own, in every
/// language. The same choice is in Settings, under "Appearance".
fn language_menu(choice: LanguageChoice) -> Menu {
    let is = |language| choice == LanguageChoice::Is(language);
    Menu::new(tr(Key("menu.app.language"))).items([
        MenuItem::action(tr(Key("language.automatic")), LanguageAutomatic)
            .checked(choice == LanguageChoice::Automatic),
        MenuItem::action(Language::English.name(), LanguageEnglish).checked(is(Language::English)),
        MenuItem::action(Language::Italian.name(), LanguageItalian).checked(is(Language::Italian)),
    ])
}

fn file_menu() -> Menu {
    Menu::new(tr(Key("menu.file"))).items([
        MenuItem::action(tr(NEW_TASK), NewTask),
        MenuItem::action(
            tr(Key("menu.file.open_file")),
            crate::ui::files::quick_open::ToggleQuickOpen,
        ),
        MenuItem::action(
            tr(Key("menu.file.commands")),
            crate::ui::palette::TogglePalette,
        ),
        MenuItem::separator(),
        MenuItem::action(tr(tabs::CLOSE_TAB), tabs::keys::CloseTab),
    ])
}

fn edit_menu() -> Menu {
    Menu::new(tr(Key("menu.edit"))).items([
        MenuItem::os_action(tr(Key("menu.edit.undo")), Undo, OsAction::Undo),
        MenuItem::os_action(tr(Key("menu.edit.redo")), Redo, OsAction::Redo),
        MenuItem::separator(),
        MenuItem::os_action(tr(Key("menu.edit.cut")), Cut, OsAction::Cut),
        MenuItem::os_action(tr(Key("menu.edit.copy")), Copy, OsAction::Copy),
        MenuItem::os_action(tr(Key("menu.edit.paste")), Paste, OsAction::Paste),
        MenuItem::os_action(
            tr(Key("menu.edit.select_all")),
            SelectAll,
            OsAction::SelectAll,
        ),
    ])
}

/// What the lavoro on screen shows and runs: its agent or its terminal, whether the right pane follows
/// the agent, and its Play. Each acts on the lavoro on screen and does nothing without one.
fn view_menu() -> Menu {
    Menu::new(tr(Key("menu.view"))).items([
        MenuItem::action(tr(Key("menu.view.terminal")), SwitchTerminal),
        MenuItem::action(tr(Key("menu.view.follow")), ToggleFollow),
        MenuItem::separator(),
        MenuItem::action(tr(Key("menu.view.play_start")), StartPlay),
        MenuItem::action(tr(Key("menu.view.play_stop")), StopPlay),
    ])
}

/// One window only, so the menu holds the window commands and no window list.
fn window_menu(panes: PanesShown) -> Menu {
    Menu::new(tr(Key("menu.window"))).items([
        MenuItem::action(tr(Key("menu.window.minimize")), Minimize),
        MenuItem::action(tr(Key("menu.window.zoom")), Zoom),
        MenuItem::separator(),
        MenuItem::action(tr(sidebar_label(panes.sidebar)), ToggleSidebar),
        MenuItem::action(tr(changes_label(panes.changes)), ToggleChanges),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_labels_say_what_a_press_would_do() {
        assert_ne!(sidebar_label(true), sidebar_label(false));
        assert_ne!(changes_label(true), changes_label(false));
        crate::l10n::set_for_test(Language::Italian);
        assert_eq!(tr(sidebar_label(true)), "Nascondi barra laterale");
        assert_eq!(tr(changes_label(false)), "Mostra modifiche");
        crate::l10n::set_for_test(Language::English);
        assert_eq!(tr(sidebar_label(false)), "Show sidebar");
    }

    #[test]
    fn the_app_menu_opens_with_about_under_the_product_s_name() {
        crate::l10n::set_for_test(Language::English);
        let menu = app_menu(LanguageChoice::Automatic);
        assert_eq!(menu.name.as_ref(), "Sakura");
        let is_about = matches!(&menu.items[0],
            MenuItem::Action { name, .. } if name.as_ref() == "About Sakura");
        assert!(is_about, "About is the first command of the app menu");
        assert!(matches!(menu.items[1], MenuItem::Separator));
    }

    #[test]
    fn a_key_is_written_as_the_mac_s_menus_write_it() {
        let written = |keys: &'static str| keys_of(&Keystroke::parse(keys).expect("a key"));
        assert_eq!(written("cmd-n"), "⌘N");
        assert_eq!(written("alt-cmd-b"), "⌥⌘B");
        assert_eq!(written("shift-cmd-z"), "⇧⌘Z");
        assert_eq!(written("cmd-,"), "⌘,");
        assert_eq!(written("ctrl-alt-shift-cmd-k"), "⌃⌥⇧⌘K");
    }

    #[gpui::test]
    fn the_shortcuts_are_the_menu_bar_s_commands_that_have_a_key(cx: &mut gpui::TestAppContext) {
        crate::l10n::set_for_test(Language::English);
        let found = cx.update(|cx| {
            install(cx);
            shortcuts(cx)
        });
        let of = |action: &str| {
            let shortcut = found.iter().find(|shortcut| shortcut.action == action);
            shortcut.map(|shortcut| (shortcut.menu.to_string(), shortcut.keys.to_string()))
        };
        assert_eq!(of("Settings…"), Some(("Sakura".into(), "⌘,".into())));
        assert_eq!(of("Quit Sakura"), Some(("Sakura".into(), "⌘Q".into())));
        assert_eq!(of("New task"), Some(("File".into(), "⌘N".into())));
        assert_eq!(of("Hide sidebar"), Some(("Window".into(), "⌘B".into())));
        assert_eq!(of("Hide changes"), Some(("Window".into(), "⌥⌘B".into())));
        assert_eq!(of("Zoom"), None, "a command with no key is not a shortcut");
        assert_eq!(of("English"), None);
        let menus: Vec<String> = found.iter().map(|s| s.menu.to_string()).collect();
        let mut in_order = menus.clone();
        in_order.dedup();
        assert_eq!(of("Start Play"), Some(("View".into(), "⌘R".into())));
        assert_eq!(of("Stop Play"), Some(("View".into(), "⌘.".into())));
        assert_eq!(
            in_order,
            ["Sakura", "File", "Edit", "View", "Window"],
            "{menus:?}"
        );
    }

    #[test]
    fn the_language_menu_checks_the_choice_in_force_and_names_languages_by_their_own_names() {
        crate::l10n::set_for_test(Language::English);
        let menu = language_menu(LanguageChoice::Is(Language::Italian));
        let checked: Vec<bool> = menu.items.iter().map(MenuItem::is_checked).collect();
        assert_eq!(checked, [false, false, true]);
        let automatic = language_menu(LanguageChoice::Automatic);
        assert!(automatic.items[0].is_checked());
    }
}
