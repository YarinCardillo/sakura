//! The window's views.

pub mod base_branches;
pub mod blocked;
pub mod brand;
pub mod breath;
pub mod centre;
pub mod centred;
pub mod changes;
pub mod chooser;
pub mod chrome;
pub mod console;
pub mod conversations;
pub mod creation;
pub mod desk;
pub mod desk_header;
pub mod desk_panes;
pub mod desk_play;
pub mod files;
mod files_side;
pub mod gitgraph;
pub mod glyph;
pub mod header_fit;
pub mod header_view;
pub mod hold;
pub mod keyboard;
pub mod new_task;
pub mod notify;
mod notifying;
pub mod opening;
pub mod page;
pub mod palette;
mod palette_side;
#[cfg(test)]
mod palette_window_tests;
pub mod pane_toggle;
pub mod panes;
pub mod play;
pub mod project_setup;
mod publishing;
pub mod remove;
mod restoring;
pub mod right_pane;
pub mod settings;
pub mod sheet_accounts;
pub mod sidebar;
pub mod slide;
pub mod split;
pub mod status_bar;
pub mod tabs;
pub mod take;
pub mod task_menu;
pub mod thread;
pub mod verify;
pub mod view_row;

/// Declares a window test once for every language of the interface, as `name::english` and
/// `name::italian`: a layout must hold whatever the words are.
#[cfg(test)]
macro_rules! in_every_language {
    (fn $name:ident($cx:ident: &mut TestAppContext) $body:block) => {
        mod $name {
            use super::*;

            #[gpui::test]
            fn english($cx: &mut TestAppContext) {
                crate::l10n::set_for_test(crate::l10n::Language::English);
                $body
            }

            #[gpui::test]
            fn italian($cx: &mut TestAppContext) {
                crate::l10n::set_for_test(crate::l10n::Language::Italian);
                $body
            }
        }
    };
}
#[cfg(test)]
mod console_tests;
#[cfg(test)]
mod conversation_tests;
#[cfg(test)]
mod desk_play_tests;
#[cfg(test)]
mod ending_tests;
#[cfg(test)]
mod files_window_tests;
#[cfg(test)]
mod focus_tests;
#[cfg(test)]
mod gesture_tests;
#[cfg(test)]
mod grid_tests;
#[cfg(test)]
mod header_tests;
#[cfg(test)]
mod idle_tests;
#[cfg(test)]
mod interaction_tests;
#[cfg(test)]
mod l10n_tests;
#[cfg(test)]
mod launch_tests;
#[cfg(test)]
mod login_tests;
#[cfg(test)]
mod menu_tests;
#[cfg(test)]
mod new_task_tests;
#[cfg(test)]
mod panes_tests;
#[cfg(test)]
mod project_setup_tests;
#[cfg(test)]
mod real_text;
#[cfg(test)]
mod refresh_tests;
#[cfg(test)]
mod steady_tests;
#[cfg(test)]
mod tabs_window_tests;
#[cfg(test)]
mod wrap_tests;
