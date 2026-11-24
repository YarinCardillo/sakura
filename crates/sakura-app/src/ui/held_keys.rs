//! The modifier keys held down, as the window shows them: Cmd held a moment shows the shortcut of what has
//! one (the tabs' numbers), and Option held shows the technical name of what is named in plain words (a
//! lane's branch). A pure state machine, read through a global by whatever draws, and driven by the
//! window's modifier events.

use crate::theme::{HINT_DELAY, MODIFIER_POLL_INTERVAL};
use gpui::{App, Global, Modifiers, Task, Window};

/// What the state machine asks its driver to do after a change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// Nothing.
    None,
    /// Cmd went down alone: call [`HeldKeys::delay_over`] with this token after the pause.
    WaitForHints(u64),
}

/// The modifier keys held now.
#[derive(Debug, Default, Clone)]
pub struct HeldKeys {
    is_command_alone: bool,
    are_hints_shown: bool,
    is_option_alone: bool,
    /// Which hold of Cmd a pause belongs to: a pause of an earlier hold does nothing.
    token: u64,
    /// The keys last taken from the window.
    seen: Modifiers,
}

impl Global for HeldKeys {}

impl HeldKeys {
    /// The state after the keys `modifiers` are held. Cmd alone starts a wait; Cmd with any other key is a
    /// shortcut and shows nothing.
    pub fn change(&mut self, modifiers: Modifiers) -> Effect {
        let others = modifiers.shift || modifiers.alt || modifiers.control || modifiers.function;
        let was_command = self.is_command_alone;
        self.is_command_alone = modifiers.platform && !others;
        self.is_option_alone =
            modifiers.alt && !(modifiers.platform || modifiers.shift || modifiers.control);
        if !self.is_command_alone {
            self.are_hints_shown = false;
            return Effect::None;
        }
        if was_command {
            return Effect::None;
        }
        self.token += 1;
        Effect::WaitForHints(self.token)
    }

    /// The pause of hold `token` is over: the hints show when Cmd is still held. Whether anything changed.
    pub fn delay_over(&mut self, token: u64) -> bool {
        let is_due = token == self.token && self.is_command_alone && !self.are_hints_shown;
        self.are_hints_shown |= is_due;
        is_due
    }

    /// Whether the shortcuts show.
    pub fn are_hints_shown(&self) -> bool {
        self.are_hints_shown
    }

    /// Whether the technical names show.
    pub fn is_identity_shown(&self) -> bool {
        self.is_option_alone
    }

    /// The keys held now, as the window shows them; nothing before any was held.
    pub fn current(cx: &App) -> Self {
        cx.try_global::<Self>().cloned().unwrap_or_default()
    }
}

/// GPUI dispatches modifier events through the focus path, which can be empty on a lane page.
pub fn watch(window: &Window, cx: &App) -> Task<()> {
    window.spawn(cx, async |cx| loop {
        cx.background_executor().timer(MODIFIER_POLL_INTERVAL).await;
        let result = cx.update(|window, cx| {
            if window.is_window_active() && sync(window.modifiers(), cx) {
                window.refresh();
            }
        });
        if result.is_err() {
            return;
        }
    })
}

pub fn follow(window: &Window, cx: &mut App) -> bool {
    sync(window.modifiers(), cx)
}

fn sync(modifiers: Modifiers, cx: &mut App) -> bool {
    let mut held = HeldKeys::current(cx);
    if held.seen == modifiers {
        return false;
    }
    held.seen = modifiers;
    let effect = held.change(modifiers);
    cx.set_global(held);
    if let Effect::WaitForHints(token) = effect {
        cx.spawn(async move |cx| {
            cx.background_executor().timer(HINT_DELAY).await;
            cx.update(|cx| show_hints(token, cx));
        })
        .detach();
    }
    true
}

fn show_hints(token: u64, cx: &mut App) {
    let mut held = HeldKeys::current(cx);
    if held.delay_over(token) {
        cx.set_global(held);
        cx.refresh_windows();
    }
}
