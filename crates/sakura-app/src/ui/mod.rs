//! The window's views.

#[cfg(not(feature = "boxes"))]
pub mod account_add;
#[cfg(all(test, not(feature = "boxes")))]
mod account_add_tests;
mod account_words;
#[cfg(feature = "boxes")]
mod agent_trail;
#[cfg(not(feature = "boxes"))]
mod answers;
#[cfg(all(test, not(feature = "boxes")))]
mod audit_tests;
#[cfg(feature = "boxes")]
pub mod base_branches;
#[cfg(feature = "boxes")]
pub mod blocked;
#[cfg_attr(not(feature = "boxes"), allow(dead_code, unused_imports))]
pub mod brand;
#[cfg_attr(not(feature = "boxes"), allow(dead_code))]
pub mod breath;
#[cfg(feature = "boxes")]
pub mod centre;
#[cfg_attr(not(feature = "boxes"), allow(dead_code))]
pub mod centred;
#[cfg_attr(not(feature = "boxes"), allow(dead_code))]
pub mod changes;
#[cfg(feature = "boxes")]
pub mod chooser;
#[cfg_attr(not(feature = "boxes"), allow(dead_code))]
pub mod chrome;
#[cfg(not(feature = "boxes"))]
mod confirm_words;
#[cfg(all(test, not(feature = "boxes")))]
mod confirm_words_tests;
#[cfg(feature = "boxes")]
pub mod console;
#[cfg(feature = "boxes")]
pub mod conversations;
#[cfg(feature = "boxes")]
pub mod creation;
#[cfg(feature = "boxes")]
pub mod desk;
#[cfg(feature = "boxes")]
pub mod desk_boxes;
#[cfg(feature = "boxes")]
pub mod desk_header;
#[cfg(feature = "boxes")]
pub mod desk_panes;
#[cfg(feature = "boxes")]
pub mod desk_play;
mod dialog_frame;
#[cfg(not(feature = "boxes"))]
mod dialog_guard;
#[cfg(not(feature = "boxes"))]
mod draft_keeper;
#[cfg(not(feature = "boxes"))]
mod engine_acts;
#[cfg(all(test, not(feature = "boxes")))]
mod engine_contract_tests;
#[cfg(not(feature = "boxes"))]
pub mod engine_desk;
#[cfg(all(test, not(feature = "boxes")))]
mod engine_desk_tests;
#[cfg(all(test, not(feature = "boxes")))]
mod engine_fixture_tests;
#[cfg(not(feature = "boxes"))]
mod engine_notifying;
#[cfg(not(feature = "boxes"))]
mod engine_page;
#[cfg(not(feature = "boxes"))]
mod engine_palette;
#[cfg(all(test, not(feature = "boxes")))]
mod engine_palette_tests;
#[cfg(all(test, not(feature = "boxes")))]
mod engine_rig_tests;
#[cfg(not(feature = "boxes"))]
pub mod engine_status_bar;
#[cfg(all(test, not(feature = "boxes")))]
mod engine_status_bar_tests;
#[cfg(not(feature = "boxes"))]
mod engine_values;
mod enter;
#[cfg(not(feature = "boxes"))]
mod event_changes;
#[cfg(not(feature = "boxes"))]
mod event_kinds;
#[cfg(not(feature = "boxes"))]
mod event_notify;
#[cfg(not(feature = "boxes"))]
mod event_words;
#[cfg(all(test, not(feature = "boxes")))]
mod event_words_tests;
#[cfg_attr(not(feature = "boxes"), allow(dead_code, unused_imports))]
pub mod files;
#[cfg(feature = "boxes")]
mod files_side;
#[cfg(not(feature = "boxes"))]
pub mod first_launch;
#[cfg(all(test, not(feature = "boxes")))]
mod first_launch_tests;
#[cfg(feature = "boxes")]
pub mod gitgraph;
#[cfg_attr(not(feature = "boxes"), allow(dead_code))]
pub mod glyph;
#[cfg(feature = "boxes")]
pub mod header_fit;
#[cfg(feature = "boxes")]
pub mod header_view;
#[cfg(not(feature = "boxes"))]
mod held_keys;
#[cfg(feature = "boxes")]
pub mod hold;
mod inert_markdown;
pub mod keyboard;
#[cfg(not(feature = "boxes"))]
mod scheduled;
pub mod tip;
#[cfg(not(feature = "boxes"))]
mod track_page;
// The engine pages that use these arrive task by task; until then they are unused, and the boxes
// build never uses them.
#[cfg(not(feature = "boxes"))]
mod agent_chat;
#[cfg(all(test, not(feature = "boxes")))]
mod agent_chat_tests;
#[cfg(all(test, not(feature = "boxes")))]
mod agent_rig_tests;
#[cfg(not(feature = "boxes"))]
mod agent_words;
#[cfg(not(feature = "boxes"))]
mod ask_findings;
#[cfg(all(feature = "snapshot", not(feature = "boxes")))]
pub mod catalogue;
#[cfg(not(feature = "boxes"))]
mod codex_login;
#[cfg(all(test, not(feature = "boxes")))]
mod dialog_place_tests;
mod drag_scroll;
#[cfg(not(feature = "boxes"))]
pub mod inbox;
#[cfg_attr(feature = "boxes", allow(dead_code, unused_imports))]
pub mod kit;
#[cfg(not(feature = "boxes"))]
mod lane_agents;
#[cfg(all(test, not(feature = "boxes")))]
mod lane_agents_tests;
#[cfg(not(feature = "boxes"))]
mod lane_composer;
#[cfg(not(feature = "boxes"))]
mod lane_paces;
#[cfg(not(feature = "boxes"))]
mod lane_page;
#[cfg(all(test, not(feature = "boxes")))]
mod lane_page_tests;
#[cfg(not(feature = "boxes"))]
mod lane_side;
#[cfg(not(feature = "boxes"))]
mod lane_sidebar;
#[cfg(all(test, not(feature = "boxes")))]
mod lane_sidebar_tests;
#[cfg(all(test, not(feature = "boxes")))]
mod lane_tests;
#[cfg(not(feature = "boxes"))]
pub mod lanes;
#[cfg(not(feature = "boxes"))]
mod lanes_controls;
#[cfg(not(feature = "boxes"))]
mod lanes_page;
#[cfg(all(test, not(feature = "boxes")))]
mod lanes_page_tests;
#[cfg(not(feature = "boxes"))]
pub mod master_accounts;
#[cfg(not(feature = "boxes"))]
pub mod master_agent;
#[cfg(not(feature = "boxes"))]
mod master_binding;
#[cfg(not(feature = "boxes"))]
mod master_page;
#[cfg(all(test, not(feature = "boxes")))]
mod master_page_tests;
#[cfg(not(feature = "boxes"))]
mod master_picks;
#[cfg(not(feature = "boxes"))]
mod new_lane;
#[cfg(all(test, not(feature = "boxes")))]
mod new_lane_tests;
#[cfg(not(feature = "boxes"))]
mod new_lane_view;
#[cfg(feature = "boxes")]
pub mod new_task;
pub mod notify;
#[cfg(feature = "boxes")]
mod notifying;
#[cfg(feature = "boxes")]
pub mod opening;
#[cfg(feature = "boxes")]
pub mod page;
#[cfg(not(feature = "boxes"))]
pub mod page_bar;
#[cfg(not(feature = "boxes"))]
mod page_scroll;
pub mod palette;
#[cfg(feature = "boxes")]
mod palette_side;
#[cfg(test)]
#[cfg(feature = "boxes")]
mod palette_window_tests;
pub mod pane_toggle;
#[cfg(feature = "boxes")]
pub mod panes;
#[cfg(not(feature = "boxes"))]
mod path_text;
#[cfg_attr(not(feature = "boxes"), allow(dead_code))]
pub mod patience;
#[cfg(not(feature = "boxes"))]
mod plain_error;
#[cfg(all(test, not(feature = "boxes")))]
mod plain_error_tests;
#[cfg(feature = "boxes")]
pub mod play;
#[cfg(not(feature = "boxes"))]
pub mod project_add;
#[cfg(all(test, not(feature = "boxes")))]
mod project_add_tests;
#[cfg(not(feature = "boxes"))]
mod project_page;
#[cfg(feature = "boxes")]
pub mod project_setup;
#[cfg(feature = "boxes")]
mod publishing;
mod rem_scope;
#[cfg(feature = "boxes")]
pub mod remove;
#[cfg(feature = "boxes")]
mod restoring;
#[cfg(not(feature = "boxes"))]
mod retino;
#[cfg(all(test, not(feature = "boxes")))]
mod retino_tests;
#[cfg(feature = "boxes")]
pub mod right_pane;
#[cfg(feature = "boxes")]
pub mod row_age;
#[cfg(feature = "boxes")]
pub mod settings;
#[cfg(not(feature = "boxes"))]
pub mod settings_modal;
#[cfg(feature = "boxes")]
pub mod sheet_accounts;
#[cfg(feature = "boxes")]
mod sheet_place;
#[cfg(feature = "boxes")]
pub mod sidebar;
#[cfg(not(feature = "boxes"))]
mod sidebar_pages;
#[cfg(not(feature = "boxes"))]
mod sidebar_projects;
pub mod slide;
pub mod split;
#[cfg(feature = "boxes")]
pub mod status_bar;
#[cfg(not(feature = "boxes"))]
mod step_track;
pub mod tab_motion;
#[cfg(feature = "boxes")]
pub mod tabs;
#[cfg(feature = "boxes")]
pub mod take;
#[cfg(feature = "boxes")]
pub mod task_menu;
#[cfg_attr(not(feature = "boxes"), allow(dead_code, unused_imports))]
pub mod thread;
#[cfg(not(feature = "boxes"))]
mod titled_links;
#[cfg(not(feature = "boxes"))]
pub mod tour;
#[cfg(all(test, not(feature = "boxes")))]
mod tour_tests;
#[cfg_attr(not(feature = "boxes"), allow(dead_code))]
mod update_link;
#[cfg(feature = "boxes")]
pub mod verify;
#[cfg(feature = "boxes")]
pub mod view_row;

#[cfg(not(feature = "boxes"))]
fn relative_age_words(since: Option<&str>, now: i64) -> Option<gpui::SharedString> {
    use sakura_core::graph::Age;

    let (unit, count) = match relative_age(since?, now)? {
        Age::Now => return Some(crate::l10n::tr(crate::l10n::Key("inbox.age.now")).into()),
        Age::Minutes(count) => ("inbox.age.minute", count),
        Age::Hours(count) => ("inbox.age.hour", count),
        Age::Days(count) => ("inbox.age.day", count),
    };
    Some(crate::l10n::say(
        &sakura_core::said::Said::new(unit).count(count),
    ))
}

fn unix_now() -> i64 {
    let elapsed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
    elapsed.map_or(0, |elapsed| {
        i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX)
    })
}

#[cfg(not(feature = "boxes"))]
fn relative_age(since: &str, now: i64) -> Option<sakura_core::graph::Age> {
    use sakura_core::graph::{parse_time, Age};

    const SECONDS_PER_MINUTE: i64 = 60;
    const MINUTES_PER_HOUR: u64 = 60;
    const MINUTES_PER_DAY: u64 = 24 * MINUTES_PER_HOUR;
    let minute = |seconds: i64| seconds.div_euclid(SECONDS_PER_MINUTE);
    let minutes = minute(now)
        .saturating_sub(minute(parse_time(since)?))
        .max(0) as u64;
    Some(match minutes {
        0 => Age::Now,
        _ if minutes < MINUTES_PER_HOUR => Age::Minutes(minutes),
        _ if minutes < MINUTES_PER_DAY => Age::Hours(minutes / MINUTES_PER_HOUR),
        _ => Age::Days(minutes / MINUTES_PER_DAY),
    })
}

/// Declares a window test once for every language of the interface, as `name::english` and
/// `name::italian`: a layout must hold whatever the words are.
#[cfg(all(test, feature = "boxes"))]
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
#[cfg(all(test, not(feature = "boxes")))]
mod composer_enter_tests;
#[cfg(test)]
mod composer_look_tests;
#[cfg(test)]
#[cfg(feature = "boxes")]
mod console_tests;
#[cfg(test)]
#[cfg(feature = "boxes")]
mod conversation_tests;
#[cfg(all(test, not(feature = "boxes")))]
mod desk_drafts_tests;
#[cfg(test)]
#[cfg(feature = "boxes")]
mod desk_play_tests;
#[cfg(all(test, not(feature = "boxes")))]
mod draft_keeper_tests;
#[cfg(test)]
#[cfg(feature = "boxes")]
mod ending_tests;
#[cfg(test)]
mod enter_tests;
#[cfg(test)]
#[cfg(feature = "boxes")]
mod files_window_tests;
#[cfg(test)]
#[cfg(feature = "boxes")]
mod focus_tests;
#[cfg(test)]
mod fuchsia_tests;
#[cfg(test)]
#[cfg(feature = "boxes")]
mod gesture_tests;
#[cfg(test)]
#[cfg(feature = "boxes")]
mod grid_tests;
#[cfg(test)]
#[cfg(feature = "boxes")]
mod header_tests;
#[cfg(all(test, not(feature = "boxes")))]
mod held_keys_tests;
#[cfg(test)]
#[cfg(feature = "boxes")]
mod idle_tests;
#[cfg(test)]
#[cfg(feature = "boxes")]
mod interaction_tests;
#[cfg(test)]
mod l10n_tests;
#[cfg(test)]
#[cfg(feature = "boxes")]
mod launch_tests;
#[cfg(test)]
#[cfg(feature = "boxes")]
mod login_tests;
#[cfg(test)]
#[cfg(feature = "boxes")]
mod menu_tests;
#[cfg(test)]
#[cfg(feature = "boxes")]
mod new_task_tests;
#[cfg(test)]
#[cfg(feature = "boxes")]
mod panes_tests;
#[cfg(test)]
#[cfg(feature = "boxes")]
mod project_setup_tests;
#[cfg(test)]
mod real_text;
#[cfg(test)]
#[cfg(feature = "boxes")]
mod refresh_tests;
#[cfg(test)]
#[cfg(feature = "boxes")]
mod row_age_tests;
#[cfg(test)]
#[cfg(feature = "boxes")]
mod steady_tests;
#[cfg(all(test, not(feature = "boxes")))]
mod tab_focus_tests;
#[cfg(test)]
#[cfg(feature = "boxes")]
mod tabs_window_tests;
#[cfg(all(test, not(feature = "boxes")))]
mod view_state_tests;
#[cfg(test)]
#[cfg(feature = "boxes")]
mod wrap_tests;
