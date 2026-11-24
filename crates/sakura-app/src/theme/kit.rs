//! The pieces the pages share (`ui::kit`): sizes and timings only. The design scale they are built from is
//! in [`scale`].

// The boxes build draws its own pages and takes only part of the scale.
#![cfg_attr(feature = "boxes", allow(dead_code))]

pub mod scale;

use gpui::{px, Pixels};
use std::time::Duration;

/// A chip: the scale's chip rule (small control height, equal padding, pill radius).
pub const CHIP_HEIGHT: Pixels = scale::CHIP_HEIGHT;
pub const CHIP_RADIUS: Pixels = scale::CHIP_RADIUS;
/// The room a chip keeps while it has no name to say yet: a quiet block the name takes the place of.
pub const CHIP_PLACEHOLDER_WIDTH: Pixels = px(88.);
/// The least height of a settings row: a name over its sentence, with the room of a control.
pub const SETTINGS_ROW_MIN_HEIGHT: Pixels = px(64.);
/// The widest an empty state's sentence runs, and the room of a stepper's number.
pub const EMPTY_STATE_MAX_WIDTH: Pixels = px(360.);
pub const STEPPER_NUMBER_WIDTH: Pixels = scale::SPACE_XXL;
/// The thickness of a lane's progress line.
pub const PROGRESS_LINE_HEIGHT: Pixels = px(3.);
/// How long a toast with Undo stays, one without, and one that says something went wrong.
pub const TOAST_WITH_UNDO: Duration = Duration::from_secs(6);
pub const TOAST_WITHOUT_UNDO: Duration = Duration::from_secs(4);
pub const TOAST_ERROR: Duration = Duration::from_secs(8);
/// How often a toast looks at the pointer and the clock.
pub const TOAST_TICK: Duration = Duration::from_millis(50);
/// A toast's sheet: its radius, the space between its text and its button, and the padding around them
/// (less at the button's side, which has its own padding).
pub const TOAST_RADIUS: Pixels = px(10.);
pub const TOAST_GAP: Pixels = px(14.);
pub const TOAST_PADDING_Y: Pixels = px(9.);
pub const TOAST_PADDING_START: Pixels = px(14.);
pub const TOAST_PADDING_END: Pixels = px(10.);
pub const TOAST_BOTTOM: Pixels = px(20.);
#[cfg(not(feature = "boxes"))]
pub const TOAST_DOCK_HEIGHT: Pixels = px(96.);
/// How faint a button is while what it does is on its way.
pub const DISABLED_OPACITY: f32 = 0.5;
/// The width of the ring around a toast's sheet.
pub const TOAST_RING_WIDTH: Pixels = px(1.);
/// How far a toast rises while it appears, and sinks while it leaves.
pub const TOAST_RISE: Pixels = px(6.);
/// Its drop shadow: offset, blur and the darkness of the black.
pub const TOAST_SHADOW_OFFSET_Y: Pixels = px(2.);
pub const TOAST_SHADOW_BLUR: Pixels = px(8.);
pub const TOAST_SHADOW_DARKNESS: f32 = 0.12;
/// How far a drag or a sideways scroll across a toast must travel to send it away.
pub const TOAST_SWIPE_DISTANCE: Pixels = px(48.);
pub const TOAST_BUTTON_PADDING_X: Pixels = px(8.);
pub const TOAST_BUTTON_PADDING_Y: Pixels = px(4.);
/// The grid a lane's state glyph is drawn on, and its parts on that grid: the ring's stroke and outer
/// diameter, the discs, the half disc's radius, the dash and the check.
pub const GLYPH_GRID: f32 = 14.;
pub const GLYPH_STROKE: f32 = 1.5;
pub const GLYPH_RING_DIAMETER: f32 = 12.;
pub const GLYPH_NEEDS_YOU_DIAMETER: f32 = 10.;
pub const GLYPH_FINISHED_DIAMETER: f32 = 11.;
pub const GLYPH_HALF_RADIUS: f32 = 3.5;
pub const GLYPH_DASH_WIDTH: f32 = 5.;
pub const GLYPH_CHECK_SIZE: f32 = 8.;

#[cfg(test)]
mod scale_tests;

/// The rooms of the catalogue page's specimens (snapshot builds): named layout sizes, not part of the scale.
#[cfg(all(feature = "snapshot", not(feature = "boxes")))]
pub mod catalogue {
    use gpui::{px, Pixels};

    /// The room a specimen of a long label is given, by kind.
    pub const BUTTON_LONG_ROOM: Pixels = px(180.);
    pub const CHIP_LONG_ROOM: Pixels = px(160.);
    pub const TAG_LONG_ROOM: Pixels = px(120.);
    /// A field, and the full width segments.
    pub const FIELD_WIDTH: Pixels = px(320.);
    /// A row, a settings row and a card.
    pub const ROW_WIDTH: Pixels = px(300.);
    /// An agent row: wide enough for its two sides and the words of what the agent does.
    pub const AGENT_ROW_WIDTH: Pixels = px(560.);
    /// A conversation with an agent: the width of its column.
    pub const CONVERSATION_WIDTH: Pixels = px(560.);
    pub const SETTINGS_ROW_WIDTH: Pixels = px(460.);
    pub const CARD_WIDTH: Pixels = px(360.);
    /// The card that holds the empty state.
    pub const EMPTY_CARD_HEIGHT: Pixels = px(200.);
    /// The samples of a spacing step, a control height and a radius.
    pub const STEP_BAR_HEIGHT: Pixels = px(12.);
    pub const HEIGHT_BLOCK_WIDTH: Pixels = px(96.);
    pub const RADIUS_BLOCK: Pixels = px(56.);
}

/// The agent row reserves this much for the role before moving the state below it.
pub const AGENT_ROW_ROLE_WIDTH: Pixels = px(144.);
pub const AGENT_ROW_DOING_MAX_WIDTH: Pixels = px(360.);

pub mod dropdown {
    use gpui::{px, Pixels};

    pub const PADDING: Pixels = px(4.);
    pub const ROW_GAP: Pixels = px(2.);
    pub const ITEM_HEIGHT: Pixels = px(26.);
    pub const CHECK_SIZE: Pixels = px(12.);
    pub const MIN_WIDTH: Pixels = px(128.);
    pub const MAX_WIDTH: Pixels = px(500.);
    pub const MAX_HEIGHT: Pixels = px(450.);
}

#[cfg(not(feature = "boxes"))]
pub const SETUP_LOGIN_ACTION_WIDTH: Pixels = px(132.);
