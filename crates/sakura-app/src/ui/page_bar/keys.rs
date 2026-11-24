//! The bar's keys: Cmd+1 to Cmd+8 select a tab by its place, Cmd+9 the last one, Cmd+W closes the tab
//! shown while another remains, Cmd+T opens a new tab with a new, empty Master conversation (the + of the bar does the same), Cmd+[ and Cmd+] go back and forward in
//! the tab shown. Bound once for the app and answered wherever the keyboard is, like the window's other
//! commands.

use super::PageBar;
use gpui::{actions, App, Entity, KeyBinding};

actions!(
    sakura_tabs,
    [
        SelectTab1,
        SelectTab2,
        SelectTab3,
        SelectTab4,
        SelectTab5,
        SelectTab6,
        SelectTab7,
        SelectTab8,
        SelectLastTab,
        CloseTab,
        NewTab,
        GoBack,
        GoForward,
        FocusOtherPane
    ]
);

/// The digit of the key that shows the last tab, whatever its place.
const LAST_TAB_DIGIT: usize = 9;

/// Binds the keys and sends them to `bar`.
pub fn install(bar: &Entity<PageBar>, cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-1", SelectTab1, None),
        KeyBinding::new("cmd-2", SelectTab2, None),
        KeyBinding::new("cmd-3", SelectTab3, None),
        KeyBinding::new("cmd-4", SelectTab4, None),
        KeyBinding::new("cmd-5", SelectTab5, None),
        KeyBinding::new("cmd-6", SelectTab6, None),
        KeyBinding::new("cmd-7", SelectTab7, None),
        KeyBinding::new("cmd-8", SelectTab8, None),
        KeyBinding::new("cmd-9", SelectLastTab, None),
        KeyBinding::new("cmd-w", CloseTab, None),
        KeyBinding::new("cmd-t", NewTab, None),
        KeyBinding::new("cmd-[", GoBack, None),
        KeyBinding::new("cmd-]", GoForward, None),
        KeyBinding::new("alt-cmd-j", FocusOtherPane, None),
    ]);
    select::<SelectTab1>(bar, 1, cx);
    select::<SelectTab2>(bar, 2, cx);
    select::<SelectTab3>(bar, 3, cx);
    select::<SelectTab4>(bar, 4, cx);
    select::<SelectTab5>(bar, 5, cx);
    select::<SelectTab6>(bar, 6, cx);
    select::<SelectTab7>(bar, 7, cx);
    select::<SelectTab8>(bar, 8, cx);
    select::<SelectLastTab>(bar, LAST_TAB_DIGIT, cx);
    act::<CloseTab>(
        bar,
        |bar, cx| {
            bar.close_active(cx);
        },
        cx,
    );
    act::<NewTab>(
        bar,
        |bar, cx| {
            bar.open_new_conversation(cx);
        },
        cx,
    );
    let weak = bar.downgrade();
    cx.on_action(move |_: &GoBack, cx| {
        if crate::ui::dialog_guard::is_open(cx) {
            return;
        }
        // The page's own way back first (an agent's conversation closes), then the tab's history.
        let inner = weak.read_with(cx, |bar, _| bar.inner_back.clone());
        if inner.is_ok_and(|inner| inner.is_some_and(|inner| inner(cx))) {
            return;
        }
        drop(weak.update(cx, |bar, cx| bar.back(cx)));
    });
    act::<GoForward>(
        bar,
        |bar, cx| {
            bar.forward(cx);
        },
        cx,
    );
}

/// The action `A` selects the tab `digit` names.
fn select<A: gpui::Action>(bar: &Entity<PageBar>, digit: usize, cx: &mut App) {
    act::<A>(
        bar,
        move |bar, cx| {
            bar.activate_digit(digit, cx);
        },
        cx,
    );
}

/// The action `A` runs `run` on the bar.
fn act<A: gpui::Action>(
    bar: &Entity<PageBar>,
    run: impl Fn(&mut PageBar, &mut gpui::Context<PageBar>) + 'static,
    cx: &mut App,
) {
    let bar = bar.downgrade();
    cx.on_action(move |_: &A, cx| {
        // The tabs under Settings (or another dialog) take no key until it is closed.
        if !crate::ui::dialog_guard::is_open(cx) {
            drop(bar.update(cx, |bar, cx| run(bar, cx)));
        }
    });
}
