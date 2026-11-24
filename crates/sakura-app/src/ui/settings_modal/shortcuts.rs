//! "Shortcuts": the keys of the window, read only.

use super::group::group;
use super::SettingsModal;
use crate::l10n::{tr, Key};
use crate::theme::settings_modal as sizes;
use crate::theme::{self, Palette};
use gpui::{
    div, AnyElement, InteractiveElement as _, IntoElement as _, ParentElement as _, Styled as _,
};

/// What a key does, and the key as the Mac's menus write it.
const SHORTCUTS: [(Key, &str); 12] = [
    (Key("settings_modal.shortcuts.settings"), "\u{2318},"),
    (Key("settings_modal.shortcuts.sidebar"), "\u{2318}B"),
    (Key("settings_modal.shortcuts.details"), "\u{2325}\u{2318}B"),
    (Key("settings_modal.shortcuts.new_tab"), "\u{2318}T"),
    (Key("settings_modal.shortcuts.close_tab"), "\u{2318}W"),
    (Key("settings_modal.shortcuts.back"), "\u{2318}["),
    (Key("settings_modal.shortcuts.forward"), "\u{2318}]"),
    (Key("settings_modal.shortcuts.tab"), "\u{2318}1-9"),
    (Key("settings_modal.shortcuts.new_lane"), "\u{2318}N"),
    (Key("settings_modal.shortcuts.palette"), "\u{2318}K"),
    (Key("settings_modal.shortcuts.open_file"), "\u{2318}P"),
    (Key("settings_modal.shortcuts.inbox"), "J K A D R"),
];

impl SettingsModal {
    /// The groups of "Shortcuts".
    pub(super) fn shortcuts(&self, palette: &Palette) -> Vec<AnyElement> {
        let rows = SHORTCUTS.iter().enumerate().map(|(nth, (action, keys))| {
            let id = format!("settings-shortcut-{nth}");
            div()
                .debug_selector(move || id.clone())
                .min_h(sizes::COMPACT_ROW_HEIGHT)
                .px(sizes::ROW_PADDING_X)
                .flex()
                .items_center()
                .justify_between()
                .gap(theme::GAP_WIDE)
                .text_size(sizes::ROW_TEXT)
                .child(div().text_color(palette.text).child(tr(*action)))
                .child(div().text_color(palette.text_secondary).child(*keys))
                .into_any_element()
        });
        vec![group(rows.collect(), palette)]
    }
}
