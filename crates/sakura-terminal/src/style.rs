//! How a terminal looks, passed in by the app: font, size, padding and palette.

use crate::palette::TerminalPalette;
use gpui::{px, Font, FontFallbacks, FontFeatures, FontStyle, FontWeight, Pixels, SharedString};
use std::sync::Arc;

/// Ghostty's default font size on macOS, in points.
pub const DEFAULT_FONT_SIZE: f32 = 13.0;
/// Zed's "standard" terminal line height, as a multiple of the font size.
pub const STANDARD_LINE_HEIGHT: f32 = 1.3;
/// Space between the panel edge and the grid.
pub const DEFAULT_PADDING: f32 = 8.0;

/// OpenType features for terminal text: no ligatures, and Geist Mono's stylistic set 9, its zero without a
/// slash or a dot (other fonts ignore the feature).
fn terminal_font_features() -> FontFeatures {
    FontFeatures(Arc::new(vec![
        ("calt".into(), 0),
        ("liga".into(), 0),
        ("ss09".into(), 1),
    ]))
}

/// The ring drawn inside the view while it holds focus that arrived from the keyboard (Tab), never after a
/// click. Sizes are in points, the colour is `0xRRGGBB`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FocusRing {
    /// Colour of the ring.
    pub colour: u32,
    /// Width of its line.
    pub width: f32,
    /// Distance from the view's edge.
    pub inset: f32,
    /// Corner radius.
    pub radius: f32,
}

/// Font, metrics and colours of a terminal view.
#[derive(Clone, Debug, PartialEq)]
pub struct TerminalStyle {
    /// Font family, for example "Geist Mono". It must be installed or registered with the app.
    pub font_family: SharedString,
    /// The families tried in order for a glyph the font lacks: a Nerd Font, for the icons a shell prints
    /// from the private-use area. `None` leaves the fallback to the system.
    pub font_fallbacks: Option<FontFallbacks>,
    /// Font size in points.
    pub font_size: f32,
    /// Line height as a multiple of the font size.
    pub line_height: f32,
    /// Padding between the view's top and bottom edges and the grid, in points.
    pub padding: f32,
    /// Padding between the view's left and right edges and the grid, in points (an app aligns it with the
    /// text around the terminal).
    pub padding_x: f32,
    /// Colours; `palette.background` also fills the view.
    pub palette: TerminalPalette,
    /// Whether Option sends ESC plus the key (Meta) instead of composing characters. Off by default, as in
    /// Ghostty: on Italian and other layouts Option types `@`, `#` and brackets.
    pub option_as_meta: bool,
    /// The keyboard focus ring; `None` draws no ring.
    pub focus_ring: Option<FocusRing>,
}

impl TerminalStyle {
    /// Ghostty's defaults in `font_family` with `palette`.
    pub fn new(font_family: impl Into<SharedString>, palette: TerminalPalette) -> Self {
        Self {
            font_family: font_family.into(),
            font_fallbacks: None,
            font_size: DEFAULT_FONT_SIZE,
            line_height: STANDARD_LINE_HEIGHT,
            padding: DEFAULT_PADDING,
            padding_x: DEFAULT_PADDING,
            palette,
            option_as_meta: false,
            focus_ring: None,
        }
    }

    /// The regular font.
    pub(crate) fn font(&self) -> Font {
        Font {
            family: self.font_family.clone(),
            features: terminal_font_features(),
            fallbacks: self.font_fallbacks.clone(),
            weight: FontWeight::NORMAL,
            style: FontStyle::Normal,
        }
    }

    /// The font for a run that is bold and or italic.
    pub(crate) fn font_for(&self, bold: bool, italic: bool) -> Font {
        Font {
            weight: if bold {
                FontWeight::BOLD
            } else {
                FontWeight::NORMAL
            },
            style: if italic {
                FontStyle::Italic
            } else {
                FontStyle::Normal
            },
            ..self.font()
        }
    }

    pub(crate) fn font_size_px(&self) -> Pixels {
        px(self.font_size)
    }

    pub(crate) fn padding_px(&self) -> Pixels {
        px(self.padding)
    }

    pub(crate) fn padding_x_px(&self) -> Pixels {
        px(self.padding_x)
    }
}

#[cfg(test)]
mod fallback_tests {
    use super::*;

    #[test]
    fn font_every_weight_and_slant_carries_the_fallback_families() {
        let mut style = TerminalStyle::new("Geist Mono", TerminalPalette::one_dark(0x000000));
        assert_eq!(
            style.font().fallbacks,
            None,
            "none unless the app names them"
        );
        let chain = FontFallbacks::from_fonts(vec!["Symbols Nerd Font Mono".to_string()]);
        style.font_fallbacks = Some(chain.clone());
        assert_eq!(style.font().fallbacks, Some(chain.clone()));
        assert_eq!(style.font_for(true, true).fallbacks, Some(chain));
    }
}
