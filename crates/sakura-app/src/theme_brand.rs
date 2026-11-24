//! The tokens of Sakura's brand in the window: its two colours, the grid the mark is drawn on, the sizes
//! the mark is shown at, the timing of its one motion and the About window. The drawing itself is not
//! here: it is the brand's own file (`assets/brand/cells.json`), read by `ui::brand`.

use gpui::{px, Pixels};
use std::time::Duration;

/// The brand's fuchsia: the mark's colour, and the dark value of the default accent (Sakura Pink).
pub const FUCHSIA: u32 = 0xf400a1;
/// The brand's black: the ground the fuchsia mark stands on, and the ink of whatever stands on fuchsia.
pub const BLACK: u32 = 0x000000;

/// The accent chosen in Settings, with a value for each appearance. The Inbox uses it for attention
/// and the retino uses a muted version as decoration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Accent {
    /// The brand's own.
    #[default]
    SakuraPink,
    InstrumentCyan,
    SignalOrange,
    Chartreuse,
}

/// The accents' values, in the order of [`Accent::ALL`]: on dark, then on light, as RGB hex.
const ACCENT_VALUES: [(u32, u32); 4] = [
    (FUCHSIA, 0xc2007e),
    (0x4bcfe5, 0x087d96),
    (0xff783e, 0xb94b18),
    (0xc2df65, 0x647719),
];

impl Accent {
    /// Every accent, in the order of the row that chooses between them.
    pub const ALL: [Accent; 4] = [
        Accent::SakuraPink,
        Accent::InstrumentCyan,
        Accent::SignalOrange,
        Accent::Chartreuse,
    ];

    fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|accent| *accent == self)
            .unwrap_or_default()
    }

    /// What names the accent in the preferences file and in the window.
    pub fn id(self) -> &'static str {
        match self {
            Accent::SakuraPink => "sakura-pink",
            Accent::InstrumentCyan => "instrument-cyan",
            Accent::SignalOrange => "signal-orange",
            Accent::Chartreuse => "chartreuse",
        }
    }

    /// The accent an id names; the default for none or an id this build does not know.
    pub fn from_id(id: Option<&str>) -> Self {
        Self::ALL
            .into_iter()
            .find(|accent| Some(accent.id()) == id)
            .unwrap_or_default()
    }

    /// The accent's name in the interface.
    #[cfg_attr(feature = "boxes", allow(dead_code))]
    pub fn name(self) -> crate::l10n::Key {
        match self {
            Accent::SakuraPink => crate::l10n::Key("settings_modal.accent.sakura_pink"),
            Accent::InstrumentCyan => crate::l10n::Key("settings_modal.accent.instrument_cyan"),
            Accent::SignalOrange => crate::l10n::Key("settings_modal.accent.signal_orange"),
            Accent::Chartreuse => crate::l10n::Key("settings_modal.accent.chartreuse"),
        }
    }

    /// The accent's value on a dark ground (`is_dark`) or a light one, as RGB hex.
    pub fn rgb(self, is_dark: bool) -> u32 {
        let (dark, light) = ACCENT_VALUES[self.index()];
        if is_dark {
            dark
        } else {
            light
        }
    }
}

/// The ink of whatever stands on the colour `fill` (RGB hex): the brand's black or white, whichever reads
/// better on it.
pub fn ink_on(fill: u32) -> u32 {
    const WHITE: u32 = 0xffffff;
    if contrast_of(fill, BLACK) >= contrast_of(fill, WHITE) {
        BLACK
    } else {
        WHITE
    }
}

/// The relative luminance of an RGB hex colour (WCAG 2).
fn luminance_of(hex: u32) -> f32 {
    let channel = |shift: u32| {
        let value = ((hex >> shift) & 0xff) as f32 / 255.;
        match value <= 0.04045 {
            true => value / 12.92,
            false => ((value + 0.055) / 1.055).powf(2.4),
        }
    };
    0.2126 * channel(16) + 0.7152 * channel(8) + 0.0722 * channel(0)
}

/// The contrast ratio of two RGB hex colours (WCAG 2).
fn contrast_of(one: u32, other: u32) -> f32 {
    let (one, other) = (luminance_of(one), luminance_of(other));
    (one.max(other) + 0.05) / (one.min(other) + 0.05)
}

/// The mark is drawn on a grid of this many cells a side.
pub const GRID: u32 = 32;
/// The clear space kept on every side of the mark and of the lockup, in cells. It is part of their
/// layout box, so nothing can be set closer.
pub const CLEAR_CELLS: u32 = 4;
/// The lockup (mark, gap, word) is this many cells wide and [`GRID`] cells tall.
pub const LOCKUP_CELLS_WIDE: u32 = 123;

/// The sizes the mark is shown at, in points: whole multiples of [`GRID`], so that a cell is a whole
/// number of points.
pub const MARK_SMALL: u32 = 32;
pub const MARK_MEDIUM: u32 = 64;

/// The mark's one motion: its cells switch on in sixteen steps, in the order of the Bayer matrix the
/// mark is dithered with, one step every [`REVEAL_STEP`], 480 ms in all.
pub const REVEAL_STEPS: u8 = 16;
pub const REVEAL_STEP: Duration = Duration::from_millis(30);

/// The About window: its size, the points one cell of its lockup takes, and the gaps of its column.
pub const ABOUT_WIDTH: Pixels = px(360.);
pub const ABOUT_HEIGHT: Pixels = px(240.);
pub const ABOUT_LOCKUP_CELL: u32 = 2;
pub const ABOUT_GAP: Pixels = px(8.);
pub const ABOUT_LINE_GAP: Pixels = px(4.);

/// Where Sakura's source is.
pub const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Palette;
    use crate::theme_contrast::{contrast, GRAPHIC, TEXT};
    use gpui::{rgb, Hsla};

    #[test]
    fn the_default_accent_is_the_brand_s_fuchsia_on_dark_and_its_deeper_value_on_light() {
        assert_eq!(Palette::dark().attention, Hsla::from(rgb(FUCHSIA)));
        assert_eq!(
            Palette::light().attention,
            Hsla::from(rgb(Accent::SakuraPink.rgb(false)))
        );
    }

    #[test]
    fn black_on_fuchsia_and_fuchsia_on_the_dark_panel_read_as_text() {
        let (fuchsia, black) = (Hsla::from(rgb(FUCHSIA)), Hsla::from(rgb(BLACK)));
        assert!(contrast(black, fuchsia) >= TEXT);
        let dark = Palette::dark();
        for ground in [dark.panel, dark.canvas, dark.raised, dark.hover] {
            assert!(contrast(fuchsia, ground) >= TEXT, "{ground:?}");
        }
    }

    /// The four accents with the dark and the light value the person chose, as RGB hex.
    const OWNER_S_ACCENTS: [(Accent, u32, u32); 4] = [
        (Accent::SakuraPink, 0xf400a1, 0xc2007e),
        (Accent::InstrumentCyan, 0x4bcfe5, 0x087d96),
        (Accent::SignalOrange, 0xff783e, 0xb94b18),
        (Accent::Chartreuse, 0xc2df65, 0x647719),
    ];

    #[test]
    fn the_four_accents_hold_the_owner_s_values_and_sakura_pink_is_the_default() {
        assert_eq!(Accent::default(), Accent::SakuraPink);
        for (accent, dark, light) in OWNER_S_ACCENTS {
            assert_eq!(accent.rgb(true), dark, "{accent:?} on dark");
            assert_eq!(accent.rgb(false), light, "{accent:?} on light");
        }
        assert_eq!(
            Accent::ALL.map(|accent| accent.rgb(true)),
            OWNER_S_ACCENTS.map(|a| a.1)
        );
    }

    #[test]
    fn an_accent_is_saved_by_its_id_and_an_unknown_id_is_the_default() {
        for accent in Accent::ALL {
            assert_eq!(Accent::from_id(Some(accent.id())), accent);
        }
        assert_eq!(Accent::from_id(Some("magenta")), Accent::SakuraPink);
        assert_eq!(Accent::from_id(None), Accent::SakuraPink);
    }

    #[test]
    fn retino_dots_are_more_chromatic_and_stay_quieter_than_text() {
        use crate::theme::Appearance;
        let chroma = |color: Hsla| (1. - (2. * color.l - 1.).abs()) * color.s;

        for (appearance, old_lightness) in [(Appearance::Dark, 0.122), (Appearance::Light, 0.873)] {
            for accent in Accent::ALL {
                let palette = Palette::of(appearance, accent);
                let dot = palette.retino_dot;
                let old_dot = Hsla {
                    l: old_lightness,
                    s: palette.attention.s * 0.22,
                    ..palette.attention
                };
                let label = format!("{appearance:?} {accent:?}");
                assert_eq!(dot.h, old_dot.h, "{label}");
                assert!(chroma(dot) > 2. * chroma(old_dot), "{label}");
                assert_eq!(
                    dot.l > old_lightness,
                    appearance == Appearance::Dark,
                    "{label}"
                );
                assert_ne!(dot.l, old_lightness, "{label}");
                assert!(
                    contrast(dot, palette.canvas) < contrast(palette.text_tertiary, palette.canvas),
                    "{label}"
                );
            }
        }
    }

    #[test]
    fn the_palette_follows_the_chosen_accent() {
        use crate::theme::Appearance;
        for (accent, dark, light) in OWNER_S_ACCENTS {
            let on_dark = Palette::of(Appearance::Dark, accent);
            let on_light = Palette::of(Appearance::Light, accent);
            assert_eq!(on_dark.attention, Hsla::from(rgb(dark)));
            assert_eq!(on_light.attention, Hsla::from(rgb(light)));
        }
    }

    #[test]
    fn the_ink_on_every_accent_fill_reads_as_text_in_both_themes() {
        use crate::theme::Appearance;
        for accent in Accent::ALL {
            for appearance in [Appearance::Dark, Appearance::Light] {
                let palette = Palette::of(appearance, accent);
                let ratio = contrast(palette.attention_ink, palette.attention);
                assert!(ratio >= TEXT, "{accent:?} {appearance:?}: {ratio}");
            }
        }
    }

    #[test]
    fn every_accent_reads_as_text_on_the_surfaces_text_stands_on_and_as_a_mark_on_the_rest() {
        use crate::theme::Appearance;
        for accent in Accent::ALL {
            for appearance in [Appearance::Dark, Appearance::Light] {
                let palette = Palette::of(appearance, accent);
                for ground in [palette.panel, palette.raised] {
                    let ratio = contrast(palette.attention, ground);
                    assert!(
                        ratio >= TEXT,
                        "{accent:?} {appearance:?} {ground:?}: {ratio}"
                    );
                }
                for ground in [palette.canvas, palette.hover] {
                    let ratio = contrast(palette.attention, ground);
                    assert!(
                        ratio >= GRAPHIC,
                        "{accent:?} {appearance:?} {ground:?}: {ratio}"
                    );
                }
            }
        }
    }

    #[test]
    fn no_code_names_an_accent_value_outside_the_table() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let values: Vec<String> = OWNER_S_ACCENTS
            .iter()
            .flat_map(|(_, dark, light)| [format!("{dark:06x}"), format!("{light:06x}")])
            .collect();
        let mut offenders = Vec::new();
        let mut folders = vec![src];
        while let Some(folder) = folders.pop() {
            for entry in std::fs::read_dir(folder).expect("a folder") {
                let path = entry.expect("an entry").path();
                if path.is_dir() {
                    folders.push(path);
                    continue;
                }
                let is_table = path
                    .file_name()
                    .is_some_and(|name| name == "theme_brand.rs");
                if is_table || path.extension().is_none_or(|ext| ext != "rs") {
                    continue;
                }
                let text = std::fs::read_to_string(&path)
                    .expect("a source")
                    .to_lowercase();
                offenders.extend(
                    values
                        .iter()
                        .filter(|value| text.contains(value.as_str()))
                        .map(|value| format!("{}: {value}", path.display())),
                );
            }
        }
        assert!(
            offenders.is_empty(),
            "literal accents outside the table: {offenders:#?}"
        );
    }

    #[test]
    fn the_mark_s_sizes_are_whole_multiples_of_the_grid() {
        for points in [MARK_SMALL, MARK_MEDIUM] {
            assert_eq!(points % GRID, 0, "{points}");
        }
        assert_eq!(
            REVEAL_STEP * u32::from(REVEAL_STEPS),
            Duration::from_millis(480)
        );
    }
}
