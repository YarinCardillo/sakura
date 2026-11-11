//! Terminal colours: the ANSI palette passed in by the app and how a cell colour resolves against it.
//!
//! Colours are `0xRRGGBB` integers so the palette stays plain data. Sakura's reference tokens define no terminal
//! colours, so the defaults are Zed's One Dark and One Light terminal palettes (`assets/themes/one/one.json`),
//! with the cursor and selection taken from the foreground: Sakura has no blue accent.

use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb};

/// Opacity of faint (SGR 2) text, Ghostty's `faint-opacity` default.
pub(crate) const FAINT_OPACITY: f32 = 0.5;

/// First index of the 6x6x6 colour cube.
const CUBE_START: u8 = 16;
/// First index of the grey ramp.
const GREY_START: u8 = 232;
/// The six channel levels of the xterm colour cube.
const CUBE_LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];

/// The colours a terminal draws with. Every value is `0xRRGGBB`.
#[derive(Clone, Debug, PartialEq)]
pub struct TerminalPalette {
    /// Default text colour.
    pub foreground: u32,
    /// Default background; Sakura passes its panel colour here.
    pub background: u32,
    /// Cursor colour (the block's fill).
    pub cursor: u32,
    /// Selection tint, drawn at [`TerminalPalette::selection_alpha`] over the cells.
    pub selection: u32,
    /// Opacity of the selection tint, 0.0 to 1.0.
    pub selection_alpha: f32,
    /// The sixteen ANSI colours: black, red, green, yellow, blue, magenta, cyan, white, then the bright ones.
    pub ansi: [u32; 16],
}

impl TerminalPalette {
    /// Zed's One Dark terminal colours on `background`.
    pub fn one_dark(background: u32) -> Self {
        Self {
            foreground: 0xabb2bf,
            background,
            cursor: 0xabb2bf,
            selection: 0xabb2bf,
            selection_alpha: 0.28,
            ansi: [
                0x282c34, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xabb2bf,
                0x636d83, 0xea858b, 0xaad581, 0xffd885, 0x85c1ff, 0xd398eb, 0x6ed5de, 0xfafafa,
            ],
        }
    }

    /// Zed's One Light terminal colours on `background`.
    pub fn one_light(background: u32) -> Self {
        Self {
            foreground: 0x2a2c33,
            background,
            cursor: 0x2a2c33,
            selection: 0x2a2c33,
            selection_alpha: 0.2,
            ansi: [
                0x000000, 0xde3e35, 0x3f953a, 0xd2b67c, 0x2f5af3, 0x950095, 0x0997b3, 0xbbbbbb,
                0x000000, 0xde3e35, 0x3f953a, 0xd2b67c, 0x2f5af3, 0xa00095, 0x0bbcd6, 0xffffff,
            ],
        }
    }

    /// Resolves a cell colour, preferring colours the program set with OSC 4, 10 or 11.
    pub(crate) fn resolve(&self, color: Color, overrides: &Colors) -> u32 {
        match color {
            Color::Spec(rgb) => pack(rgb),
            Color::Indexed(index) => overrides[index as usize]
                .map(pack)
                .unwrap_or_else(|| self.indexed(index)),
            Color::Named(name) => overrides[name as usize]
                .map(pack)
                .unwrap_or_else(|| self.named(name)),
        }
    }

    /// The colour at `index` of the xterm palette, or a named colour for 256 and above.
    pub(crate) fn by_index(&self, index: usize, overrides: &Colors) -> u32 {
        if let Some(rgb) = overrides[index] {
            return pack(rgb);
        }
        match u8::try_from(index) {
            Ok(index) => self.indexed(index),
            Err(_) => self.named_by_number(index),
        }
    }

    fn indexed(&self, index: u8) -> u32 {
        if index < CUBE_START {
            return self.ansi[usize::from(index)];
        }
        if index < GREY_START {
            let cube = index - CUBE_START;
            let channel = |step: u8| u32::from(CUBE_LEVELS[usize::from(step % 6)]);
            return channel(cube / 36) << 16 | channel(cube / 6) << 8 | channel(cube);
        }
        let grey = 8 + 10 * u32::from(index - GREY_START);
        grey << 16 | grey << 8 | grey
    }

    fn named(&self, name: NamedColor) -> u32 {
        let number = name as usize;
        if number < 16 {
            return self.ansi[number];
        }
        self.named_by_number(number)
    }

    fn named_by_number(&self, number: usize) -> u32 {
        const BACKGROUND: usize = NamedColor::Background as usize;
        const CURSOR: usize = NamedColor::Cursor as usize;
        const DIM_BLACK: usize = NamedColor::DimBlack as usize;
        const DIM_WHITE: usize = NamedColor::DimWhite as usize;
        match number {
            BACKGROUND => self.background,
            CURSOR => self.cursor,
            DIM_BLACK..=DIM_WHITE => self.ansi[number - DIM_BLACK],
            // Foreground, bright foreground and dim foreground: faint text is drawn with opacity instead.
            _ => self.foreground,
        }
    }
}

/// Packs an alacritty colour into `0xRRGGBB`.
pub(crate) fn pack(rgb: Rgb) -> u32 {
    u32::from(rgb.r) << 16 | u32::from(rgb.g) << 8 | u32::from(rgb.b)
}

/// Unpacks `0xRRGGBB` into an alacritty colour.
pub(crate) fn unpack(color: u32) -> Rgb {
    let [_, r, g, b] = color.to_be_bytes();
    Rgb { r, g, b }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn palette() -> TerminalPalette {
        TerminalPalette::one_dark(0x000000)
    }

    #[test]
    fn palette_indexed_the16_basic_colours_come_from_the_ansi_table() {
        let colors = Colors::default();
        assert_eq!(palette().resolve(Color::Indexed(1), &colors), 0xe06c75);
        assert_eq!(
            palette().resolve(Color::Named(NamedColor::BrightBlue), &colors),
            0x85c1ff
        );
    }

    #[test]
    fn palette_indexed_cube_and_grey_ramp_follow_xterm() {
        let colors = Colors::default();
        assert_eq!(palette().resolve(Color::Indexed(16), &colors), 0x000000);
        assert_eq!(palette().resolve(Color::Indexed(196), &colors), 0xff0000);
        assert_eq!(palette().resolve(Color::Indexed(21), &colors), 0x0000ff);
        assert_eq!(palette().resolve(Color::Indexed(231), &colors), 0xffffff);
        assert_eq!(palette().resolve(Color::Indexed(232), &colors), 0x080808);
        assert_eq!(palette().resolve(Color::Indexed(255), &colors), 0xeeeeee);
    }

    #[test]
    fn palette_named_defaults_use_foreground_and_passed_background() {
        let colors = Colors::default();
        let palette = TerminalPalette::one_light(0xfdfdfd);
        assert_eq!(
            palette.resolve(Color::Named(NamedColor::Background), &colors),
            0xfdfdfd
        );
        assert_eq!(
            palette.resolve(Color::Named(NamedColor::Foreground), &colors),
            0x2a2c33
        );
    }

    #[test]
    fn palette_overrides_from_osc_win_over_the_palette() {
        let mut colors = Colors::default();
        colors[1] = Some(Rgb { r: 1, g: 2, b: 3 });
        assert_eq!(palette().resolve(Color::Indexed(1), &colors), 0x010203);
        assert_eq!(palette().by_index(1, &colors), 0x010203);
    }

    #[test]
    fn palette_true_colour_passes_through() {
        let colors = Colors::default();
        let spec = Color::Spec(Rgb {
            r: 0x12,
            g: 0x34,
            b: 0x56,
        });
        assert_eq!(palette().resolve(spec, &colors), 0x123456);
        assert_eq!(pack(unpack(0x123456)), 0x123456);
    }
}
