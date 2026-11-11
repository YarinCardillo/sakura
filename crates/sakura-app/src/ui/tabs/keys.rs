//! The strip's keys: Cmd+1 to Cmd+8 select a tab by its place, Cmd+9 the last one, Cmd+W closes the tab
//! shown while another remains. Bound once for the app and answered wherever the keyboard is, like the
//! window's other commands.

use super::strip::Strip;
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
        CloseTab
    ]
);

/// Binds the keys and sends them to `strip`.
pub fn install(strip: &Entity<Strip>, cx: &mut App) {
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
    ]);
    select::<SelectTab1>(strip, 1, cx);
    select::<SelectTab2>(strip, 2, cx);
    select::<SelectTab3>(strip, 3, cx);
    select::<SelectTab4>(strip, 4, cx);
    select::<SelectTab5>(strip, 5, cx);
    select::<SelectTab6>(strip, 6, cx);
    select::<SelectTab7>(strip, 7, cx);
    select::<SelectTab8>(strip, 8, cx);
    select::<SelectLastTab>(strip, sakura_core::tabs::LAST_TAB_DIGIT, cx);
    let strip = strip.downgrade();
    cx.on_action(move |_: &CloseTab, cx| {
        drop(strip.update(cx, |strip, cx| strip.close_active(cx)));
    });
}

/// The action `A` selects the tab `digit` names.
fn select<A: gpui::Action>(strip: &Entity<Strip>, digit: usize, cx: &mut App) {
    let strip = strip.downgrade();
    cx.on_action(move |_: &A, cx| {
        drop(strip.update(cx, |strip, cx| strip.select_position(digit, cx)));
    });
}
