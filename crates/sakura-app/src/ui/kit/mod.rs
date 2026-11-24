//! The pieces the pages share, built only from the design scale (`theme::kit::scale`): one component per kind
//! (button, chip, tag, row and settings row, card, field with its label, stepper, empty state, section header),
//! a choice between a few words, a toast with Undo (Sakura's own sheet, with no close button), the lane state
//! glyphs and the thin progress line. Switch, Slider, Kbd and Dialog come straight from the kit and are not
//! wrapped here.

#[cfg(not(feature = "boxes"))]
mod account_row;
mod agent_row;
mod button;
mod card;
mod chip;
#[cfg(any(test, feature = "snapshot"))]
mod empty_state;
#[cfg(any(test, feature = "snapshot"))]
mod field;
mod filter_button;
mod path_label;
mod progress_line;
mod row;
mod scroll_menu;
mod section_header;
pub mod segments;
mod state_glyph;
#[cfg(not(feature = "boxes"))]
mod stepper;
#[cfg(any(test, feature = "snapshot"))]
mod tag;
mod toast;
mod toast_phase;
mod toggle;

#[cfg(test)]
mod component_tests;
#[cfg(test)]
mod tests;

#[cfg(not(feature = "boxes"))]
pub use account_row::AccountRow;
pub use agent_row::{agent_row, AgentRowFacts, DoingLine};
#[cfg(feature = "snapshot")]
pub use button::button_hovered;
#[cfg(not(feature = "boxes"))]
pub(super) use button::button_icon_disabled;
#[cfg(any(test, feature = "snapshot"))]
pub use button::button_large;
#[cfg(not(feature = "boxes"))]
pub(super) use button::keyboard_ring;
pub use button::{button, button_disabled, button_icon, ButtonKind};
pub use card::card;
pub use chip::menu_chip_at_width;
pub use chip::{arriving, chip, chip_placeholder, menu_chip, menu_chip_on_fill};
#[cfg(any(test, feature = "snapshot"))]
pub use empty_state::empty_state;
#[cfg(any(test, feature = "snapshot"))]
pub use field::field;
pub use filter_button::checked_item;
pub use filter_button::filter_button;
pub use path_label::path_label;
#[cfg(not(feature = "boxes"))]
pub(super) use path_label::{reveal_path, reveal_path_icon};
pub use progress_line::progress_line;
#[cfg(not(feature = "boxes"))]
pub use row::press_row;
#[cfg(any(test, feature = "snapshot"))]
pub use row::settings_row;
pub use row::{command_row, settings_row_acting, settings_row_with, LabelTarget};
#[cfg(any(test, feature = "snapshot", not(feature = "boxes")))]
pub use row::{row, RowKind};
pub use scroll_menu::{scroll_dropdown, ScrollMenu, ScrollMenuItem};
pub use section_header::section_header;
pub use segments::{segment_track, segments, OnPress, Segment};
pub use state_glyph::StateGlyph;
#[cfg(not(feature = "boxes"))]
pub use stepper::{stepper, OnNumber};
#[cfg(any(test, feature = "snapshot"))]
pub use tag::tag;
#[cfg(any(test, feature = "snapshot"))]
pub use toast::toast_surface;
pub use toast::Toast;
pub use toggle::{toggle, toggle_with_label};
