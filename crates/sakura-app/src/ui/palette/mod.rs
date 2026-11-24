//! The command palette, Cmd+K. Built on the kit's command list. In the boxes build it goes to a task, starts a
//! new task, opens a file, shows the views of the right pane, publishes, archives, toggles the panes, closes
//! the tab and sets the language; in the engine build (`engine`) it goes to a page, a lane or a project and
//! runs the actions of the page on screen.
//!
//! Entry point:
//! - [`Palette`], an entity drawn last in the window's root (it lays itself over the whole window).
//! - [`install`] binds Cmd+K and opens the palette for the [`Situation`] the desk describes at that moment
//!   (its tasks, the task and view shown, the panes, the language).
//! - It emits [`PaletteEvent::Asked`] with an [`Ask`] for what only the desk can do; a command on a task
//!   names its task and is emitted only while the window still offers it. Commands the window
//!   already has an action for (new task, the panes, close tab) are dispatched as that action, with its key
//!   shown beside them.
//! - Its sizes are the named constants of [`metrics`].

#[cfg(feature = "boxes")]
pub mod commands;
#[cfg(not(feature = "boxes"))]
pub mod engine;
mod entry;
pub mod metrics;
mod view;

#[cfg(all(test, feature = "boxes"))]
mod tests;

#[cfg(feature = "boxes")]
pub use commands::{
    action_of, groups, icon_of, is_offered, Ask, Entry, Group, PlayState, Run, ShownTask, Situation,
};
#[cfg(not(feature = "boxes"))]
pub use engine::{action_of, groups, icon_of, is_offered, Ask, Entry, Group, Run, Situation};
pub use view::{Palette, PaletteEvent};

use crate::l10n::Key;

/// What the field says before anything is typed.
#[cfg(feature = "boxes")]
pub const PLACEHOLDER: Key = Key("palette.placeholder");
#[cfg(not(feature = "boxes"))]
pub const PLACEHOLDER: Key = Key("palette.placeholder_lanes");

use gpui::{actions, App, Entity, KeyBinding, Window};

actions!(sakura_palette, [TogglePalette, ChooseInNewTab]);

/// Binds Cmd+Shift+P and Cmd+K to `palette` in `window`. `situation` is read each time the key is pressed.
pub fn install(
    palette: &Entity<Palette>,
    situation: impl Fn(&App) -> Situation + 'static,
    window: &Window,
    cx: &mut App,
) {
    // Cmd+Shift+P is the key of the editors the person comes from, and the one the menu shows (the
    // binding made last); Cmd+K stays.
    cx.bind_keys([
        KeyBinding::new("cmd-k", TogglePalette, None),
        KeyBinding::new("cmd-shift-p", TogglePalette, None),
        // Cmd-Return chooses as Return does, and a page opens in a tab of its own.
        // The kit's command list answers in the key context "Command".
        KeyBinding::new("cmd-enter", ChooseInNewTab, Some("Command")),
    ]);
    let situation: std::rc::Rc<view::Now> = std::rc::Rc::new(situation);
    palette.update(cx, |palette, _| palette.watch(situation.clone()));
    let (handle, palette) = (window.window_handle(), palette.downgrade());
    cx.on_action(move |_: &TogglePalette, cx| {
        let Some(palette) = palette.upgrade() else {
            return;
        };
        let situation = situation(cx);
        // The window is being updated while its key dispatches: the palette opens right after.
        cx.defer(move |cx| {
            let toggled = handle.update(cx, |_, window, cx| {
                palette.update(cx, |palette, cx| palette.toggle(&situation, window, cx));
            });
            if let Err(error) = toggled {
                eprintln!("sakura: the window is gone: {error}");
            }
        });
    });
}
