//! The command palette, Cmd+K: go to a task, new task, open a file, the views of the right pane, publish,
//! archive, toggle each pane, close the tab, the language. Built on the kit's command list.
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

pub mod commands;
pub mod metrics;
mod view;

#[cfg(test)]
mod tests;

pub use commands::{Ask, PlayState, ShownTask, Situation};
pub use view::{Palette, PaletteEvent};

use gpui::{actions, App, Entity, KeyBinding, Window};

actions!(sakura_palette, [TogglePalette]);

/// Binds Cmd+K to `palette` in `window`. `situation` is read each time the key is pressed.
pub fn install(
    palette: &Entity<Palette>,
    situation: impl Fn(&App) -> Situation + 'static,
    window: &Window,
    cx: &mut App,
) {
    cx.bind_keys([KeyBinding::new("cmd-k", TogglePalette, None)]);
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
