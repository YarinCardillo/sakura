//! The design scale: the only place a text size, a weight, an icon size, a space, a control height or a radius
//! of the interface is chosen. Screens and components take these names and never a number of their own.
//! Every constant says where it is used. Layout dimensions that are not part of the scale (a window's
//! width, a column's width) are named in `theme.rs` beside the page that owns them.

// The boxes build draws its own pages and takes only part of the scale.
#![cfg_attr(feature = "boxes", allow(dead_code))]

use super::super::CONTROL_HEIGHT;
pub use super::super::WEIGHT_MEDIUM;
use gpui::{px, FontWeight, Pixels, Styled};

/// One level of the type scale: the size, the height of a line and the weight of its words.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextStyle {
    pub size: Pixels,
    pub line_height: Pixels,
    pub weight: FontWeight,
}

/// Sets a [`TextStyle`] on any element in one call, so a size is never given without its line and weight.
pub trait Typeset: Styled + Sized {
    /// The element's text takes `style`.
    fn text_style(self, style: TextStyle) -> Self {
        self.text_size(style.size)
            .line_height(style.line_height)
            .font_weight(style.weight)
    }
}

impl<T: Styled + Sized> Typeset for T {}

/// Regular weight: running text, notes, captions.
pub const WEIGHT_REGULAR: FontWeight = FontWeight::NORMAL;
/// Semibold: titles only. A selected row never changes weight.
pub const WEIGHT_SEMIBOLD: FontWeight = FontWeight::SEMIBOLD;

/// Caption, 11 over 14: key caps, a count beside a name, the smallest meta text.
pub const TEXT_CAPTION: TextStyle = TextStyle {
    size: px(11.),
    line_height: px(14.),
    weight: WEIGHT_REGULAR,
};
/// Footnote, 12 over 16: the sentence under a setting's name, an ask's source line, a column's name.
pub const TEXT_FOOTNOTE: TextStyle = TextStyle {
    size: px(12.),
    line_height: px(16.),
    weight: WEIGHT_REGULAR,
};
/// Body, 13 over 18: rows, fields, messages, every sentence of the interface. The macOS body size.
pub const TEXT_BODY: TextStyle = TextStyle {
    size: px(13.),
    line_height: px(18.),
    weight: WEIGHT_REGULAR,
};
/// Headline, 15 over 20, medium: a card's title, a group's name, a sheet's heading.
pub const TEXT_HEADLINE: TextStyle = TextStyle {
    size: px(15.),
    line_height: px(20.),
    weight: WEIGHT_MEDIUM,
};
/// Title, 17 over 22, semibold: a page's own title (a lane's name, an ask's question).
pub const TEXT_TITLE: TextStyle = TextStyle {
    size: px(17.),
    line_height: px(22.),
    weight: WEIGHT_SEMIBOLD,
};
/// Large title, 22 over 28, semibold: the one big word of a page or a card (the pace).
pub const TEXT_TITLE_LARGE: TextStyle = TextStyle {
    size: px(22.),
    line_height: px(28.),
    weight: WEIGHT_SEMIBOLD,
};
#[cfg(any(test, all(feature = "snapshot", not(feature = "boxes"))))]
/// Every level, smallest first, with its name: the catalogue page draws this list.
pub const TYPE_SCALE: [(&str, TextStyle); 6] = [
    ("TEXT_CAPTION", TEXT_CAPTION),
    ("TEXT_FOOTNOTE", TEXT_FOOTNOTE),
    ("TEXT_BODY", TEXT_BODY),
    ("TEXT_HEADLINE", TEXT_HEADLINE),
    ("TEXT_TITLE", TEXT_TITLE),
    ("TEXT_TITLE_LARGE", TEXT_TITLE_LARGE),
];

/// The one label of every control: buttons, chips, tags, segments, tabs, menu items. The body size, medium.
pub const CONTROL_LABEL: TextStyle = TextStyle {
    size: TEXT_BODY.size,
    line_height: TEXT_BODY.line_height,
    weight: WEIGHT_MEDIUM,
};

/// Small icon, 12: inside a chip or a tag, beside a caption.
pub const ICON_SMALL: Pixels = px(12.);
/// Regular icon, 16: rows, buttons, tabs, menu items, the bars.
pub const ICON_REGULAR: Pixels = px(16.);
/// Large icon, 20: an empty state's mark, a card's lead icon.
pub const ICON_LARGE: Pixels = px(20.);
#[cfg(any(test, feature = "snapshot"))]
/// The three icon sizes, smallest first.
pub const ICON_SIZES: [Pixels; 3] = [ICON_SMALL, ICON_REGULAR, ICON_LARGE];

/// 4: between an icon and its text, between two lines of one block, inside a key cap.
pub const SPACE_XS: Pixels = px(4.);
/// 8: the room a chip keeps at each side, a row's fill past its content, between two controls.
pub const SPACE_S: Pixels = px(8.);
/// 12: the padding of a row, between a card's blocks, between a name and its control.
pub const SPACE_M: Pixels = px(12.);
/// 16: a card's padding, the sidebar's inset, between two groups of controls.
pub const SPACE_L: Pixels = px(16.);
/// 20: a page's side inset, a sheet's padding.
pub const SPACE_XL: Pixels = px(20.);
/// 24: between two sections of a page.
pub const SPACE_XXL: Pixels = px(24.);
/// 32: the room around an empty state, between a page's title and its content.
pub const SPACE_XXXL: Pixels = px(32.);
#[cfg(any(test, feature = "snapshot"))]
/// Every step on the 4 pt grid, smallest first.
pub const SPACING_STEPS: [Pixels; 7] = [
    SPACE_XS, SPACE_S, SPACE_M, SPACE_L, SPACE_XL, SPACE_XXL, SPACE_XXXL,
];

/// Small control, 22: a chip, a tag, a status-bar button, a key.
pub const CONTROL_SMALL: Pixels = px(22.);
/// Regular control, 28: a button, a segment, a tab, a menu item, a field, a row of a list.
pub const CONTROL_REGULAR: Pixels = CONTROL_HEIGHT;
/// Large control, 36: the one primary action of a sheet or a card, a composer's send.
pub const CONTROL_LARGE: Pixels = px(36.);
#[cfg(any(test, feature = "snapshot"))]
/// The three control heights, smallest first.
pub const CONTROL_HEIGHTS: [Pixels; 3] = [CONTROL_SMALL, CONTROL_REGULAR, CONTROL_LARGE];

/// 4: a key cap, a progress line's end, a mark inside a control.
pub const RADIUS_SMALL: Pixels = px(4.);
/// 6: buttons, fields, segments, menu items, row fills.
pub const RADIUS_CONTROL: Pixels = px(6.);
/// 12: panels, cards, sheets, toasts, menus.
pub const RADIUS_SURFACE: Pixels = px(12.);
/// 11, half of a chip: chips, pills and tags, round at both ends.
pub const RADIUS_PILL: Pixels = px(11.);
#[cfg(any(test, feature = "snapshot"))]
/// The four radii.
pub const RADII: [Pixels; 4] = [RADIUS_SMALL, RADIUS_CONTROL, RADIUS_PILL, RADIUS_SURFACE];

/// The chip rule, for every chip, pill and tag: the small control's height, the same room at both ends of the
/// row, the label centred on the height, and one gap between an icon and its text.
pub const CHIP_HEIGHT: Pixels = CONTROL_SMALL;
/// The room before the first mark or word of a chip.
pub const CHIP_PADDING_START: Pixels = SPACE_S;
/// The room after the last mark or word of a chip: equal to the room before.
pub const CHIP_PADDING_END: Pixels = SPACE_S;
pub const CHIP_ICON: Pixels = ICON_SMALL;
/// The gap between an icon and its text, in a chip and in every control with an icon.
pub const ICON_TEXT_GAP: Pixels = SPACE_XS;
pub const CHIP_RADIUS: Pixels = RADIUS_PILL;

/// The room inside a card, on all four sides, and the gap between its blocks.
pub const CARD_PADDING: Pixels = SPACE_L;
/// The gap between the blocks of a card.
pub const CARD_GAP: Pixels = SPACE_M;
/// The room a row keeps at each side of its content, and between the parts of its line.
pub const ROW_PADDING: Pixels = SPACE_M;
/// The room a settings row keeps at each side, a little wider than a list row's.
pub const SETTINGS_ROW_PADDING: Pixels = SPACE_L;
/// The dot of a tag or a status: a mark smaller than the small icon.
pub const DOT_SIZE: Pixels = px(6.);
/// The gap between a field's label and its control, and between its control and its hint.
pub const FIELD_GAP: Pixels = SPACE_XS;
