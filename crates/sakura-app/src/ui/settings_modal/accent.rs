//! The accent row of "General": four filled swatches, each named under it, the chosen one marked with a ring
//! in the text colour. A swatch is a stop of the Tab walk and takes the keys of a group of radio buttons
//! (Enter or Space chooses it, Left and Right move to the next); nothing takes the keyboard by itself.

use super::group::with_default;
use super::SettingsModal;
use crate::l10n::{tr, Key};
use crate::prefs::Preferences;
use crate::theme::settings_modal as sizes;
use crate::theme::{self, Appearance, Palette, Theme};
use crate::theme_brand::Accent;
use gpui::{
    div, rgb, AnyElement, App, Context, FocusHandle, InteractiveElement as _, IntoElement as _,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Window,
};
use std::rc::Rc;

/// The key names that choose the swatch that has the keyboard.
const CHOOSING_KEYS: [&str; 2] = ["enter", "space"];

impl SettingsModal {
    pub(super) fn accent_row(&self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let chosen = Preferences::current(cx).accent;
        let swatches = Accent::ALL
            .into_iter()
            .enumerate()
            .map(|(nth, accent)| self.swatch(accent, nth, chosen == accent, palette, cx));
        let row = div()
            .flex()
            .items_center()
            .gap(theme::GAP)
            .children(swatches);
        let this = cx.entity().downgrade();
        let reset = Rc::new(move |_: &mut Window, cx: &mut App| {
            choose_accent(Accent::default(), cx);
            drop(this.update(cx, |_, cx| cx.notify()));
        });
        let control = with_default(
            "settings-accent",
            chosen != Accent::default(),
            reset,
            row,
            palette,
        );
        let name = tr(Key("settings_modal.accent.title"));
        super::group::row("settings-row-accent", name, None, control, None, palette)
    }

    fn swatch(
        &self,
        accent: Accent,
        nth: usize,
        is_chosen: bool,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = format!("settings-accent-{}", accent.id());
        let is_dark = palette.appearance == Appearance::Dark;
        let disc = div()
            .size(sizes::SWATCH_DISC)
            .rounded_full()
            .bg(rgb(accent.rgb(is_dark)));
        let ring_colour = if is_chosen {
            palette.text
        } else {
            gpui::transparent_black()
        };
        let mut ring = div()
            .size(sizes::SWATCH_RING)
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .bg(ring_colour);
        if is_chosen {
            let selected = format!("{id}-selected");
            ring = ring.debug_selector(move || selected.clone());
        }
        let handles = self.accent_focus.clone();
        let handle = handles[nth].clone();
        let this = cx.entity().downgrade();
        let (chosen_by_click, chosen_by_key) = (this.clone(), this);
        let focused = palette.hover;
        div()
            .id(SharedString::from(id.clone()))
            .debug_selector(move || id.clone())
            .track_focus(&handle)
            .flex_none()
            .flex()
            .items_center()
            .gap(theme::GAP_SMALL)
            .rounded(theme::RADIUS_CONTROL)
            .focus_visible(move |style| style.bg(focused))
            .child(
                ring.child(
                    div()
                        .size(sizes::SWATCH_RING - sizes::SWATCH_RING_WIDTH * 2.)
                        .rounded_full()
                        .bg(palette.grouped)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(disc),
                ),
            )
            .child(
                div()
                    .text_size(theme::TEXT_LABEL)
                    .text_color(palette.text_secondary)
                    .whitespace_nowrap()
                    .child(tr(accent.name())),
            )
            .on_click(move |_, _, cx| {
                choose_accent(accent, cx);
                drop(chosen_by_click.update(cx, |_, cx| cx.notify()));
            })
            .on_key_down(move |event, window, cx| {
                let key = event.keystroke.key.as_str();
                if CHOOSING_KEYS.contains(&key) {
                    choose_accent(accent, cx);
                    drop(chosen_by_key.update(cx, |_, cx| cx.notify()));
                } else if let Some(next) = neighbour(&handles, nth, key) {
                    window.focus(&next, cx);
                }
            })
            .into_any_element()
    }
}

/// The swatch the arrow `key` moves to from the one at `nth`, when it moves at all.
fn neighbour(handles: &[FocusHandle], nth: usize, key: &str) -> Option<FocusHandle> {
    let to = match key {
        "left" => nth.checked_sub(1)?,
        "right" => nth + 1,
        _ => return None,
    };
    handles.get(to).cloned()
}

/// The accent chosen is kept and in force at once, in every window.
fn choose_accent(accent: Accent, cx: &mut App) {
    Preferences::change(cx, |preferences| preferences.accent = accent);
    Theme::set_accent(accent, cx);
    cx.refresh_windows();
}
