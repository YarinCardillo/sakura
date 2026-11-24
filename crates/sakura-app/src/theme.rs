//! Every colour, size, radius and duration of the window, in one set of tokens. No other module writes a colour literal: views
//! read the current [`Palette`] from the [`Theme`] global and sizes from the constants here.

use crate::theme_brand::{ink_on, Accent};
use gpui::{
    point, px, rgb, rgba, App, BoxShadow, FontWeight, Global, Hsla, Pixels, WindowAppearance,
};
#[cfg(feature = "boxes")]
use sakura_terminal::{TerminalPalette, TerminalStyle};
use std::time::Duration;

const RETINO_SATURATION_SCALE: f32 = 0.78;

/// Hex values of the dark tokens (`:root`).
mod dark {
    pub const SURFACE_CANVAS: u32 = 0x0b0b0c;
    pub const SURFACE_PANEL: u32 = 0x000000;
    pub const SURFACE_RAISED: u32 = 0x111112;
    pub const SURFACE_HOVER: u32 = 0x17171a;
    pub const TEXT_PRIMARY: u32 = 0xededef;
    pub const TEXT_SECONDARY: u32 = 0xa1a1a8;
    pub const TEXT_TERTIARY: u32 = 0x9898a0;
    /// `--panel-ring`: white at 7 percent.
    pub const PANEL_RING: u32 = 0xffffff12;
    pub const BORDER: u32 = 0x222226;
    pub const PRIMARY: u32 = 0xd1d1d6;
    pub const PRIMARY_FOREGROUND: u32 = 0x000000;
    pub const DESTRUCTIVE: u32 = 0xff6961;
    pub const SUCCESS: u32 = 0xa8d5a2;
    pub const WARNING: u32 = 0xe5c07b;
    /// The selected row: stronger than every fill under the pointer, so the selection stays seen beneath it.
    pub const SIDEBAR_ACCENT: u32 = 0x2f2f36;
    pub const RING: u32 = 0xd1d1d6;
    /// `--scrollbar-thumb`: white at 24 percent.
    pub const SCROLLBAR_THUMB: u32 = 0xffffff3d;
    /// Selected text: white at 18 percent (no token in `tokens.css`; neutral, never blue).
    pub const SELECTION: u32 = 0xffffff2e;
    pub const SURFACE_INSET: u32 = 0x0d0d0e;
    pub const SHEET_FILL: u32 = 0x1f1f23;
    pub const SHEET_FILL_HOVER: u32 = 0x28282d;
    pub const SILVER_SHEEN_TOP: u32 = 0xfbfbfb;
    pub const SILVER_SHEEN_BOTTOM: u32 = 0xd0d0d0;
    pub const GROUP_SURFACE: u32 = 0x151517;
    pub const SCRIM: u32 = 0x000000b8;
    pub const OVERLAY_SURFACE: u32 = 0x353539;
    pub const OVERLAY_EDGE: u32 = 0x5a5a61;
    pub const TOGGLE_OFF: u32 = 0x505055;
}

/// Hex values of the light tokens (`:root.light`).
mod light {
    pub const SURFACE_CANVAS: u32 = 0xf3f3f3;
    pub const SURFACE_PANEL: u32 = 0xfdfdfd;
    pub const SURFACE_RAISED: u32 = 0xffffff;
    pub const SURFACE_HOVER: u32 = 0xececec;
    pub const TEXT_PRIMARY: u32 = 0x000000;
    pub const TEXT_SECONDARY: u32 = 0x3a3a3a;
    pub const TEXT_TERTIARY: u32 = 0x666666;
    /// `--panel-ring`: black at 6 percent.
    pub const PANEL_RING: u32 = 0x0000000f;
    /// `--panel-edge`: `0 1px 2px` black at 4 percent.
    pub const PANEL_EDGE: u32 = 0x0000000a;
    pub const BORDER: u32 = 0xe3e3e3;
    pub const PRIMARY: u32 = 0x1c1c1e;
    pub const PRIMARY_FOREGROUND: u32 = 0xffffff;
    pub const DESTRUCTIVE: u32 = 0xc62828;
    pub const SUCCESS: u32 = 0x2e7d32;
    pub const WARNING: u32 = 0x8a5a00;
    /// The selected row: stronger than every fill under the pointer, so the selection stays seen beneath it.
    pub const SIDEBAR_ACCENT: u32 = 0xd8d8de;
    pub const RING: u32 = 0x1c1c1e;
    /// `--scrollbar-thumb`: black at 28 percent.
    pub const SCROLLBAR_THUMB: u32 = 0x00000047;
    /// Selected text: black at 12 percent.
    pub const SELECTION: u32 = 0x0000001f;
    pub const SURFACE_INSET: u32 = 0xf5f5f5;
    pub const SHEET_FILL: u32 = 0xececf0;
    pub const SHEET_FILL_HOVER: u32 = 0xe2e2e8;
    pub const GROUP_SURFACE: u32 = 0xf0f0f3;
    pub const SCRIM: u32 = 0x00000066;
    pub const OVERLAY_SURFACE: u32 = 0xf8f8fa;
    pub const OVERLAY_EDGE: u32 = 0xc2c2c9;
    pub const TOGGLE_OFF: u32 = 0x838389;
}

#[cfg(feature = "boxes")]
/// Adapt Zed's One Dark and One Light ANSI palettes to Sakura's status tones and panel colours.
/// Keep both ANSI blues so programs retain the colour they explicitly request.
const TERMINAL_ANSI_DARK: [u32; 16] = [
    0x282c34,
    dark::DESTRUCTIVE,
    dark::SUCCESS,
    dark::WARNING,
    0x61afef,
    0xc678dd,
    0x56b6c2,
    0xabb2bf,
    0x636d83,
    0xff8a84,
    0xc1e3bc,
    0xf0d49e,
    0x85c1ff,
    0xd398eb,
    0x6ed5de,
    dark::TEXT_PRIMARY,
];
#[cfg(feature = "boxes")]
const TERMINAL_ANSI_LIGHT: [u32; 16] = [
    0x000000,
    light::DESTRUCTIVE,
    light::SUCCESS,
    light::WARNING,
    0x2f5af3,
    0x950095,
    0x0997b3,
    0xbbbbbb,
    0x666666,
    0xe53935,
    0x43a047,
    0xa86e00,
    0x4a6ff5,
    0xa00095,
    0x0bbcd6,
    0xffffff,
];
#[cfg(feature = "boxes")]
/// The terminal's selection tint: the text colour at this opacity, like the interface's selection.
const TERMINAL_SELECTION_ALPHA_DARK: f32 = 0.18;
#[cfg(feature = "boxes")]
const TERMINAL_SELECTION_ALPHA_LIGHT: f32 = 0.12;

/// `--hold-fill` and `--hold-fill-ink`: the muted red that fills a hold-to-confirm button while it is held.
const HOLD_FILL: u32 = 0xa8423d;
const HOLD_FILL_INK: u32 = 0xffffff;
/// `--silver-ink`: text on the silver action (dark only; light's action is graphite).
const SILVER_INK: u32 = 0x0b0b0c;

/// One layer of a box shadow token: colour as RGBA hex, offset, blur, spread, and whether it is inset.
struct ShadowLayer {
    rgba: u32,
    y: f32,
    blur: f32,
    spread: f32,
    is_inset: bool,
}

const fn layer(rgba: u32, y: f32, blur: f32, spread: f32, is_inset: bool) -> ShadowLayer {
    ShadowLayer {
        rgba,
        y,
        blur,
        spread,
        is_inset,
    }
}

/// `--silver-relief`, dark: top highlight, bottom shade, close and far shadows. Light has no silver.
const SILVER_RELIEF_DARK: [ShadowLayer; 4] = [
    layer(0xffffffe6, 1., 0., 0., true),
    layer(0x00000014, -1., 0., 0., true),
    layer(0x00000080, 1., 2., 0., false),
    layer(0x00000099, 4., 12., -4., false),
];

/// How the one primary action ("Take") is painted.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ActionFace {
    /// Dark: brushed silver, a sheen from `top` to `bottom` with its relief.
    Silver { top: Hsla, bottom: Hsla },
    /// Light: a flat graphite fill (the light theme's primary).
    Graphite(Hsla),
}

/// The relief of the primary action in `appearance`: the silver's on dark, none on light.
pub fn action_relief(appearance: Appearance) -> Vec<BoxShadow> {
    let layers: &[ShadowLayer] = match appearance {
        Appearance::Dark => &SILVER_RELIEF_DARK,
        Appearance::Light => &[],
    };
    layers
        .iter()
        .map(|layer| BoxShadow {
            color: rgba(layer.rgba).into(),
            offset: point(px(0.), px(layer.y)),
            blur_radius: px(layer.blur),
            spread_radius: px(layer.spread),
            inset: layer.is_inset,
        })
        .collect()
}

/// The resolved colours of one appearance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    pub appearance: Appearance,
    pub canvas: Hsla,
    pub panel: Hsla,
    pub raised: Hsla,
    pub grouped: Hsla,
    pub scrim: Hsla,
    pub overlay_surface: Hsla,
    pub overlay_edge: Hsla,
    pub toggle_off: Hsla,
    pub hover: Hsla,
    pub text: Hsla,
    pub text_secondary: Hsla,
    pub text_tertiary: Hsla,
    pub panel_ring: Hsla,
    pub border: Hsla,
    /// Silver on dark, graphite on light: the one action colour, and the running glyph.
    pub primary: Hsla,
    pub primary_foreground: Hsla,
    pub danger: Hsla,
    pub success: Hsla,
    pub warning: Hsla,
    /// The chosen accent at full strength, used to mark what needs the person.
    pub attention: Hsla,
    /// The ink of whatever stands on the accent: the brand's black or white, whichever reads better.
    pub attention_ink: Hsla,
    pub selected_row: Hsla,
    /// The view switch: its track and its selected segment.
    pub segment_track: Hsla,
    pub segment_selected: Hsla,
    /// `--panel-edge`: the light panel's soft shadow; none on dark.
    pub panel_edge: Option<Hsla>,
    pub ring: Hsla,
    pub scrollbar_thumb: Hsla,
    /// Selected text in diffs and logs.
    pub selection: Hsla,
    /// `--surface-inset`: a well inside the panel (log tails).
    pub inset: Hsla,
    /// `--sheet-fill` and its hover: secondary buttons, filled, never outlined.
    pub fill: Hsla,
    pub fill_hover: Hsla,
    pub hold_fill: Hsla,
    pub hold_ink: Hsla,
    /// The face of the one primary action ("Take") and the ink on it.
    pub action_face: ActionFace,
    pub action_ink: Hsla,
    /// The accent hue at low contrast on the sidebar canvas.
    pub retino_dot: Hsla,
}

impl Palette {
    /// The dark tokens.
    #[cfg_attr(feature = "boxes", allow(dead_code))]
    pub fn dark() -> Self {
        Self::dark_with(Accent::default())
    }

    /// The palette of `appearance` with the accent `accent`.
    pub fn of(appearance: Appearance, accent: Accent) -> Self {
        match appearance {
            Appearance::Dark => Self::dark_with(accent),
            Appearance::Light => Self::light_with(accent),
        }
    }

    fn dark_with(accent: Accent) -> Self {
        use dark::*;
        let attention: Hsla = rgb(accent.rgb(true)).into();
        Self {
            appearance: Appearance::Dark,
            canvas: rgb(SURFACE_CANVAS).into(),
            panel: rgb(SURFACE_PANEL).into(),
            raised: rgb(SURFACE_RAISED).into(),
            grouped: rgb(GROUP_SURFACE).into(),
            scrim: rgba(SCRIM).into(),
            overlay_surface: rgb(OVERLAY_SURFACE).into(),
            overlay_edge: rgb(OVERLAY_EDGE).into(),
            toggle_off: rgb(TOGGLE_OFF).into(),
            hover: rgb(SURFACE_HOVER).into(),
            text: rgb(TEXT_PRIMARY).into(),
            text_secondary: rgb(TEXT_SECONDARY).into(),
            text_tertiary: rgb(TEXT_TERTIARY).into(),
            panel_ring: rgba(PANEL_RING).into(),
            border: rgb(BORDER).into(),
            primary: rgb(PRIMARY).into(),
            primary_foreground: rgb(PRIMARY_FOREGROUND).into(),
            danger: rgb(DESTRUCTIVE).into(),
            success: rgb(SUCCESS).into(),
            warning: rgb(WARNING).into(),
            attention,
            attention_ink: rgb(ink_on(accent.rgb(true))).into(),
            selected_row: rgb(SIDEBAR_ACCENT).into(),
            segment_track: rgb(SHEET_FILL).into(),
            segment_selected: rgb(SHEET_FILL_HOVER).into(),
            panel_edge: None,
            ring: rgb(RING).into(),
            scrollbar_thumb: rgba(SCROLLBAR_THUMB).into(),
            selection: rgba(SELECTION).into(),
            inset: rgb(SURFACE_INSET).into(),
            fill: rgb(SHEET_FILL).into(),
            fill_hover: rgb(SHEET_FILL_HOVER).into(),
            hold_fill: rgb(HOLD_FILL).into(),
            hold_ink: rgb(HOLD_FILL_INK).into(),
            action_face: ActionFace::Silver {
                top: rgb(SILVER_SHEEN_TOP).into(),
                bottom: rgb(SILVER_SHEEN_BOTTOM).into(),
            },
            action_ink: rgb(SILVER_INK).into(),
            retino_dot: retino_dot(
                attention,
                rgb(SURFACE_CANVAS).into(),
                if accent == Accent::InstrumentCyan {
                    4.3
                } else {
                    2.5
                },
            ),
        }
    }

    /// The light tokens.
    #[cfg_attr(feature = "boxes", allow(dead_code))]
    pub fn light() -> Self {
        Self::light_with(Accent::default())
    }

    fn light_with(accent: Accent) -> Self {
        use light::*;
        let attention: Hsla = rgb(accent.rgb(false)).into();
        Self {
            appearance: Appearance::Light,
            canvas: rgb(SURFACE_CANVAS).into(),
            panel: rgb(SURFACE_PANEL).into(),
            raised: rgb(SURFACE_RAISED).into(),
            grouped: rgb(GROUP_SURFACE).into(),
            scrim: rgba(SCRIM).into(),
            overlay_surface: rgb(OVERLAY_SURFACE).into(),
            overlay_edge: rgb(OVERLAY_EDGE).into(),
            toggle_off: rgb(TOGGLE_OFF).into(),
            hover: rgb(SURFACE_HOVER).into(),
            text: rgb(TEXT_PRIMARY).into(),
            text_secondary: rgb(TEXT_SECONDARY).into(),
            text_tertiary: rgb(TEXT_TERTIARY).into(),
            panel_ring: rgba(PANEL_RING).into(),
            border: rgb(BORDER).into(),
            primary: rgb(PRIMARY).into(),
            primary_foreground: rgb(PRIMARY_FOREGROUND).into(),
            danger: rgb(DESTRUCTIVE).into(),
            success: rgb(SUCCESS).into(),
            warning: rgb(WARNING).into(),
            attention,
            attention_ink: rgb(ink_on(accent.rgb(false))).into(),
            selected_row: rgb(SIDEBAR_ACCENT).into(),
            segment_track: rgb(SURFACE_HOVER).into(),
            segment_selected: rgb(SURFACE_RAISED).into(),
            panel_edge: Some(rgba(PANEL_EDGE).into()),
            ring: rgb(RING).into(),
            scrollbar_thumb: rgba(SCROLLBAR_THUMB).into(),
            selection: rgba(SELECTION).into(),
            inset: rgb(SURFACE_INSET).into(),
            fill: rgb(SHEET_FILL).into(),
            fill_hover: rgb(SHEET_FILL_HOVER).into(),
            hold_fill: rgb(HOLD_FILL).into(),
            hold_ink: rgb(HOLD_FILL_INK).into(),
            action_face: ActionFace::Graphite(rgb(PRIMARY).into()),
            action_ink: rgb(PRIMARY_FOREGROUND).into(),
            retino_dot: retino_dot(attention, rgb(SURFACE_CANVAS).into(), 2.5),
        }
    }
}

fn retino_dot(accent: Hsla, canvas: Hsla, contrast: f32) -> Hsla {
    let end = if canvas.l < 0.5 { 1. } else { 0. };
    let mut dot = Hsla {
        s: accent.s * RETINO_SATURATION_SCALE,
        ..accent
    };
    let (mut low, mut high) = (0., 1.);
    for _ in 0..10 {
        let progress = (low + high) / 2.;
        dot.l = canvas.l + (end - canvas.l) * progress;
        if crate::theme_contrast::contrast(dot, canvas) < contrast {
            low = progress;
        } else {
            high = progress;
        }
    }
    dot
}

/// Dark or light.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Appearance {
    Dark,
    Light,
}

impl Appearance {
    /// What the system window appearance maps to.
    pub fn from_window(appearance: WindowAppearance) -> Self {
        match appearance {
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Appearance::Dark,
            WindowAppearance::Light | WindowAppearance::VibrantLight => Appearance::Light,
        }
    }

    /// `SAKURA_APPEARANCE=dark|light` forces an appearance (for screenshots); anything else follows the system.
    pub fn forced(value: Option<&str>) -> Option<Self> {
        match value {
            Some("dark") => Some(Appearance::Dark),
            Some("light") => Some(Appearance::Light),
            _ => None,
        }
    }

    /// The palette of this appearance.
    #[cfg_attr(feature = "boxes", allow(dead_code))]
    pub fn palette(self) -> Palette {
        match self {
            Appearance::Dark => Palette::dark(),
            Appearance::Light => Palette::light(),
        }
    }
}

#[cfg(feature = "boxes")]
/// The terminal's font, metrics and colours in `appearance`: Geist Mono 13 on the panel, the text token as
/// its foreground and the action colour as its cursor.
pub fn terminal_style(appearance: Appearance) -> TerminalStyle {
    let (text, panel, cursor, ansi, selection_alpha) = match appearance {
        Appearance::Dark => (
            dark::TEXT_PRIMARY,
            dark::SURFACE_PANEL,
            dark::PRIMARY,
            TERMINAL_ANSI_DARK,
            TERMINAL_SELECTION_ALPHA_DARK,
        ),
        Appearance::Light => (
            light::TEXT_PRIMARY,
            light::SURFACE_PANEL,
            light::PRIMARY,
            TERMINAL_ANSI_LIGHT,
            TERMINAL_SELECTION_ALPHA_LIGHT,
        ),
    };
    let palette = TerminalPalette {
        foreground: text,
        background: panel,
        cursor,
        selection: text,
        selection_alpha,
        ansi,
    };
    let font = crate::fonts::terminal_font();
    let mut style = TerminalStyle::new(font.family, palette);
    style.font_fallbacks = font.fallbacks;
    style.font_size = font.size;
    style.padding = TERMINAL_PADDING;
    style.padding_x = f32::from(CONTENT_PADDING);
    // No ring around the terminal: it showed after any return to the window that followed typing
    // (the terminal's only input), which read as a selected button. Tab belongs to the shell here,
    // so there is no keyboard walk a ring would serve.
    style.focus_ring = None;
    style
}

/// The current palette, as a GPUI global.
pub struct Theme {
    pub palette: Palette,
    /// The accent the palette was made with: it stays when the appearance changes.
    pub accent: Accent,
}

impl Global for Theme {}

impl Theme {
    /// Installs or replaces the global for `appearance`, and gives the kit's components (menus, scrollbars)
    /// the same colours and fonts.
    pub fn set(appearance: Appearance, cx: &mut App) {
        let accent = cx
            .try_global::<Theme>()
            .map_or_else(Accent::default, |theme| theme.accent);
        Self::install(appearance, accent, cx);
    }

    /// Changes the accent at once, in the appearance now shown.
    #[cfg(not(feature = "boxes"))]
    pub fn set_accent(accent: Accent, cx: &mut App) {
        let appearance = cx.global::<Theme>().palette.appearance;
        Self::install(appearance, accent, cx);
    }

    pub(crate) fn install(appearance: Appearance, accent: Accent, cx: &mut App) {
        let palette = Palette::of(appearance, accent);
        cx.set_global(Theme { palette, accent });
        sync_kit(&palette, cx);
    }

    /// The current palette.
    pub fn palette(cx: &App) -> Palette {
        cx.global::<Theme>().palette
    }
}

/// The command palette and quick-open offset from the top of the window.
pub const DIALOG_TOP: gpui::Pixels = gpui::px(90.);
pub const DIALOG_SHEET_TOP: gpui::Pixels = gpui::px(116.);

#[cfg(not(feature = "boxes"))]
pub mod tour {
    use gpui::{px, Pixels};

    pub const CARD_WIDTH: Pixels = px(292.);
    pub const WINDOW_MARGIN: Pixels = px(16.);
    pub const TARGET_GAP: Pixels = px(11.);
    pub const ARROW_SIZE: Pixels = px(7.);
}

/// The widths a dialog has: a short question, a sheet with a field, a whole page of settings.
#[cfg(not(feature = "boxes"))]
pub const DIALOG_WIDTH_SMALL: gpui::Pixels = gpui::px(440.);
pub const DIALOG_WIDTH_MEDIUM: gpui::Pixels = gpui::px(600.);
#[cfg(not(feature = "boxes"))]
pub const DIALOG_WIDTH_LARGE: gpui::Pixels = gpui::px(860.);

#[cfg(not(feature = "boxes"))]
pub mod dialog_form {
    use gpui::{px, Pixels};

    pub const MIN_BODY_HEIGHT: Pixels = px(160.);
}

/// The kit's theme in Sakura's colours, type and radii: its popup menus, dialogs, fields, scrollbars and
/// buttons read it, so they are set in the same face at the same size as the rest of the window.
fn sync_kit(palette: &Palette, cx: &mut App) {
    let mode = match palette.appearance {
        Appearance::Dark => gpui_component::ThemeMode::Dark,
        Appearance::Light => gpui_component::ThemeMode::Light,
    };
    gpui_component::Theme::change(mode, None, cx);
    let kit = gpui_component::Theme::global_mut(cx);
    kit.font_family = FONT_UI.into();
    kit.mono_font_family = FONT_MONO.into();
    kit.font_size = TEXT_BODY;
    kit.radius = RADIUS_CONTROL;
    kit.radius_lg = RADIUS_SURFACE;
    let colours = &mut kit.colors;
    colours.background = palette.panel;
    colours.foreground = palette.text;
    colours.muted_foreground = palette.text_tertiary;
    colours.overlay = palette.scrim;
    colours.border = palette.border;
    colours.popover = palette.overlay_surface;
    colours.popover_foreground = palette.text;
    colours.accent = palette.hover;
    colours.accent_foreground = palette.text;
    colours.ring = palette.ring;
    colours.selection = palette.selection;
    colours.scrollbar = gpui::transparent_black();
    colours.scrollbar_thumb = palette.scrollbar_thumb;
    colours.scrollbar_thumb_hover = palette.scrollbar_thumb;
    // The kit's sidebar and settings rows (only the Settings window draws them): the canvas with
    // the sections at rest in the secondary text, the one on screen filled like a selected row.
    colours.sidebar = palette.canvas;
    colours.sidebar_foreground = palette.text_secondary;
    colours.sidebar_accent = palette.selected_row;
    colours.sidebar_accent_foreground = palette.text;
    colours.sidebar_border = palette.border;
    colours.group_box_foreground = palette.text;
    kit.tokens.popover = palette.overlay_surface.into();
    kit.tokens.switch = palette.toggle_off.into();
    kit.tokens.switch_thumb = gpui::white().into();
    kit.tokens.sidebar = palette.canvas.into();
    kit.tokens.sidebar_accent = palette.selected_row.into();
}

/// The interface's family: the Mac's own system font (San Francisco), by the name GPUI gives it. Nothing
/// of the interface is set in another face; its zero is plain, and numbers take tabular figures.
pub const FONT_UI: &str = ".SystemUIFont";
/// The monospaced family (embedded, see `fonts.rs`), only for what is literally code: diffs, terminals,
/// commands, paths and shas. Geist Mono is kept for its plain zero (`MONO_PLAIN_ZERO`): the Mac's own
/// monospaced face slashes it.
pub const FONT_MONO: &str = "Geist Mono";

/// Geist Mono's stylistic set "Non-slashed zero".
pub const MONO_PLAIN_ZERO: &str = "ss09";

/// Window minimum size.
pub const WINDOW_MIN_WIDTH: Pixels = px(1100.);
pub const WINDOW_MIN_HEIGHT: Pixels = px(700.);
/// Window size at first launch.
pub const WINDOW_START_WIDTH: Pixels = px(1440.);
pub const WINDOW_START_HEIGHT: Pixels = px(900.);
// Layout grid.
// One vertical line per column, one horizontal line per row, one size per kind of thing. The grid is
// written once, here. The views take
// every spacing from these tokens, and `ui::grid_tests` fails when an edge, a centre line or a size drifts.
// The numbers the others are derived from:
const STRIP: f32 = 36.;
/// The engine window's title bar: the row of the traffic lights, the arrows and the tabs.
#[cfg(not(feature = "boxes"))]
const TITLE_BAR: f32 = 46.;
const HEADER: f32 = 44.;
const ROW: f32 = 28.;
const RING: f32 = 1.;
const SIDEBAR_EDGE: f32 = 16.;
const CONTENT_EDGE: f32 = 20.;
const CONTROL: f32 = 28.;
const ICON: f32 = 16.;
const SURFACE_RADIUS: f32 = 12.;
#[cfg_attr(feature = "boxes", allow(dead_code))]
const CONTROL_RADIUS: f32 = 6.;
const BODY_TEXT: f32 = 13.;
const STATUS_CONTROL: f32 = 22.;
/// A traffic light's diameter (macOS 26; 13.9 points reference measurement).
const LIGHT: f32 = 14.;
/// The distance between two traffic lights, centre to centre (23.1 measured on the same screenshot).
const LIGHT_PITCH: f32 = 23.;

/// The room over the tabs: no tab touches the window's top edge. Ghostty keeps 5 points over a tab of
/// 28; here it is the small gap of the grid.
const TAB_TOP: f32 = 6.;

/// The window strip: the row of the traffic lights and the tabs, which moves the window. The tabs stand
/// on its bottom edge and leave [`TAB_TOP`] over them; the lights are centred on the tabs' own centre
/// line.
pub const WINDOW_STRIP_HEIGHT: Pixels = px(STRIP);
/// The sidebar's inset: where the traffic lights, the group labels and the row glyphs start, and how far
/// from the sidebar's right edge the trailing figures end.
pub const SIDEBAR_INSET: Pixels = px(SIDEBAR_EDGE);
/// Where the traffic lights sit, from the window's top-left corner: on the sidebar's inset, centred on
/// the line the tabs are centred on.
pub const TRAFFIC_LIGHTS_X: Pixels = px(SIDEBAR_EDGE);
#[cfg(feature = "boxes")]
pub const TRAFFIC_LIGHTS_Y: Pixels = px(TAB_TOP + (STRIP - TAB_TOP - LIGHT) / 2.);
/// In the engine window the lights are centred in the title bar.
#[cfg(not(feature = "boxes"))]
pub const TRAFFIC_LIGHTS_Y: Pixels = px((TITLE_BAR - LIGHT) / 2.);
/// A traffic light's diameter.
#[cfg(test)]
pub const TRAFFIC_LIGHT_SIZE: Pixels = px(LIGHT);
/// The room the three traffic lights take from the window's left edge, with one sidebar inset after the
/// last: where the first tab starts when the sidebar is hidden and the panel reaches under the lights.
pub const TRAFFIC_LIGHTS_ROOM: Pixels = px(SIDEBAR_EDGE + LIGHT + LIGHT_PITCH * 2. + SIDEBAR_EDGE);

/// The status bar at the window's foot: one row, on the canvas, in the place the panel's bottom inset had.
/// Its two toggles are centred on it; the sidebar's sits on the sidebar's inset, the right pane's ends on
/// the panel's right content inset.
pub const STATUS_BAR_HEIGHT: Pixels = px(ROW);
/// The status bar's own control: the small control of macOS, which is also the button of Zed's status bar
/// (22 in a bar of 30), so a toggle's fill under the pointer keeps clear of the window's edge and of the
/// panel above it. Everywhere else a control is [`CONTROL_HEIGHT`].
pub const STATUS_CONTROL_HEIGHT: Pixels = kit::scale::CONTROL_SMALL;
/// How far the fill of a status bar toggle reaches past its icon on each side.
pub const STATUS_CONTROL_BLEED: Pixels = px((STATUS_CONTROL - ICON) / 2.);

pub const SIDEBAR_WIDTH: Pixels = px(236.);
#[cfg(not(feature = "boxes"))]
/// The width of a lane's progress bar where it stands in a row.
pub const LANE_BAR_WIDTH: Pixels = px(72.);
#[cfg(not(feature = "boxes"))]
/// The width of the column of role names on a lane's page.
pub const LANE_ROLE_WIDTH: Pixels = px(132.);
#[cfg(not(feature = "boxes"))]
/// The column of a flow lane's roles, actions and events, beside its conversation.
pub const LANE_SIDE_WIDTH: Pixels = px(300.);
/// The one row pitch: a sidebar row, a group label after the first, a changed file's row, the toolbar of
/// "Changes". Rows start where the header ends, so they share their lines across the window.
pub const ROW_HEIGHT: Pixels = px(ROW);
/// How far the fill of a row (selected, under the pointer) reaches past its content on each side.
pub const ROW_PADDING_X: Pixels = kit::scale::SPACE_S;
/// Between a row's glyph column and its label.
pub const ROW_GAP: Pixels = kit::scale::SPACE_S;
/// The one icon size: the kit's icons, and the column a row keeps for its glyph.
pub const ICON_SIZE: Pixels = kit::scale::ICON_REGULAR;
/// The size of the state marks and the chevrons: icons of the one set (Lucide), at the one icon size.
pub const MARK_SIZE: Pixels = kit::scale::ICON_REGULAR;
/// How far the disc of a filled mark stands inside the mark's box: the set draws its ring with a radius
/// of 10 and a stroke of 2 in a box of 24, so the ring's outer edge is one twenty-fourth of the box in.
pub const MARK_DISC_INSET: Pixels = px(ICON / 24.);
/// The box of the marks the conversation still draws by hand, until it is wired and takes the icon set.
pub const HAND_MARK_SIZE: Pixels = kit::scale::ICON_SMALL;
pub const GLYPH_STROKE: Pixels = px(1.5);
/// The one control height: the view tabs, every button of the header and of the window strip.
pub const CONTROL_HEIGHT: Pixels = px(CONTROL);
pub const CONTROL_PADDING_X: Pixels = kit::scale::SPACE_M;
/// How much of its face the primary button keeps under the pointer.
pub const PRIMARY_HOVER_OPACITY: f32 = 0.88;
/// How much of itself a control keeps while it is held (nothing to press yet): dimmed, still readable.
#[cfg_attr(feature = "boxes", allow(dead_code))]
pub const HELD_CONTROL_OPACITY: f32 = 0.7;
/// How far the fill of an icon button reaches past its icon on each side: the icon sits on the grid line,
/// the fill bleeds past it.
pub const CONTROL_BLEED: Pixels = px((CONTROL - ICON) / 2.);

/// The panel's inset from the window's left edge while the sidebar is hidden. It has none at the right,
/// where it reaches the window's edge, and none at the bottom, where the status bar is.
pub const PANEL_INSET: Pixels = kit::scale::SPACE_S;
pub const PANEL_RING_WIDTH: Pixels = px(RING);
/// `--panel-edge` geometry: 1 px down, 2 px blur.
pub const PANEL_EDGE_OFFSET_Y: Pixels = px(1.);
pub const PANEL_EDGE_BLUR: Pixels = px(2.);
/// The panel's content inset, from the panel's outer edge: the header's first glyph, the terminal's text,
/// the rows of "Changes" and "Checks" start on it; the header's last action, the figures of the
/// changes pane and the toggle in the window strip end on it at the right.
pub const CONTENT_INSET: Pixels = px(CONTENT_EDGE);
/// The same inset as a padding inside the panel's ring.
pub const CONTENT_PADDING: Pixels = px(CONTENT_EDGE - RING);
/// The side pane's inset (the changes pane, the file list): equal to the content inset, so the window's
/// right edge is one line from the strip to the last row.
pub const SIDE_PANE_INSET: Pixels = px(CONTENT_EDGE);
/// The header: from the panel's outer top edge to the bottom of its hairline. The sidebar's first line is
/// as tall, so both are centred on the same line.
pub const HEADER_HEIGHT: Pixels = px(HEADER);
/// The header's box inside the panel's ring.
pub const HEADER_BOX_HEIGHT: Pixels = px(HEADER - RING);
/// The line the header's content and the sidebar's first line are centred on, from the window's top.
#[cfg(any(test, feature = "snapshot"))]
pub const HEADER_CENTRE_Y: Pixels = px(STRIP + HEADER / 2.);

/// The four gaps between things that stand side by side or one over the other.
pub const GAP_TIGHT: Pixels = kit::scale::SPACE_XS;
pub const GAP_SMALL: Pixels = kit::scale::SPACE_S;
pub const GAP: Pixels = kit::scale::SPACE_S;
pub const GAP_WIDE: Pixels = kit::scale::SPACE_M;
/// The room inside a tooltip.
#[cfg(not(feature = "boxes"))]
pub const TIP_PADDING_X: Pixels = kit::scale::SPACE_M;
#[cfg(not(feature = "boxes"))]
pub const TIP_PADDING_Y: Pixels = kit::scale::SPACE_S;
/// How long the first tooltip after a rest takes to fade in (one that follows a tooltip has no fade), and the
/// room between a tooltip and the window's edge.
#[cfg(not(feature = "boxes"))]
pub const TIP_FADE: Duration = Duration::from_millis(120);
#[cfg(not(feature = "boxes"))]
pub const TIP_WINDOW_MARGIN: Pixels = kit::scale::SPACE_S;
#[cfg(not(feature = "boxes"))]
pub const TIP_PATH_WIDTH: Pixels = px(420.);
/// The room a count on a view tab keeps: two tabular figures, so a count going from 9 to 10 moves nothing.
pub const TAB_COUNT_WIDTH: Pixels = px(16.);
/// The room the header's right side keeps for the changed lines when the header is measured.
pub const HEADER_FIGURES_WIDTH: f32 = 84.;
/// The most room the branch takes in the header: a longer one is cut with an ellipsis and is whole under
/// the pointer.
pub const HEADER_BRANCH_MAX_WIDTH: f32 = 220.;
/// The gap between two views of the right pane's row.
pub const TAB_GAP: Pixels = kit::scale::SPACE_XS;
/// The least room a tab of a narrow row keeps at each side of its label.
pub const TAB_PADDING_MIN: Pixels = kit::scale::SPACE_XS;
/// A pane's row of views, the centre's and the right pane's: one row, so that with the toolbar under it the
/// rows of the right pane start two rows under the header, on the lines the sidebar's rows are on. Its
/// views are the control of a bar ([`STATUS_CONTROL_HEIGHT`]), as the toggles of the status bar are.
pub const PANE_VIEWS_HEIGHT: Pixels = px(ROW);

/// Two radii. One for surfaces (the panel, a card, a sheet): 12, the radius of the tab that fuses with
/// the panel. One for controls and everything inside a surface (buttons, fields, row fills, chips): 6,
/// Linear's, so a control one inset from a surface's edge sits concentric with it.
pub const RADIUS_SURFACE: Pixels = kit::scale::RADIUS_SURFACE;
pub const RADIUS_CONTROL: Pixels = kit::scale::RADIUS_CONTROL;
pub const RADIUS_CARD: Pixels = RADIUS_SURFACE;
pub const RADIUS_CHIP: Pixels = RADIUS_CONTROL;

/// Two text sizes, the Mac's own for a window of this kind: 13 for chrome and rows, 12 for what is
/// secondary (labels, figures, notes). Three weights only: regular, medium, semibold.
/// The medium weight. GPUI matches a weight of exactly 500 to the nearest lighter face when a family has
/// none at 500, and the system font's medium face reports a little more: requested 500 it would be drawn
/// regular. Every medium text of the interface asks for this weight.
pub const WEIGHT_MEDIUM: FontWeight = FontWeight(510.);
pub const TEXT_BODY: Pixels = kit::scale::TEXT_BODY.size;
pub const TEXT_SMALL: Pixels = kit::scale::TEXT_BODY.size;
pub const TEXT_LABEL: Pixels = kit::scale::TEXT_FOOTNOTE.size;

/// The Agente view's split: the changes pane's share of the width at first launch, and its limits.
pub const CHANGES_SHARE_DEFAULT: f32 = 0.38;
pub const CHANGES_SHARE_MIN: f32 = 0.25;
pub const CHANGES_SHARE_MAX: f32 = 0.60;
/// The review list: a file's header row, a hunk header, one line of code, and a one-sentence note.
pub const CHANGE_HEADER_HEIGHT: f32 = ROW;
/// The most lines a file's diff has and still starts open in the review list: about what the pane shows
/// at once at the window's usual height, 20 px a line, so it is read at a glance. A longer diff starts
/// closed, its header giving its figures. The conversation folds a diff after as many lines.
pub const DIFF_OPEN_MAX_LINES: usize = 40;
pub const HUNK_HEADER_HEIGHT: f32 = 28.;
pub const DIFF_LINE_HEIGHT: f32 = 20.;
pub const DIFF_NOTE_HEIGHT: f32 = ROW;
/// Code in diffs, in Geist Mono.
pub const DIFF_TEXT: Pixels = kit::scale::TEXT_FOOTNOTE.size;
/// Added and removed lines are tinted at 10 percent of the success and danger tones (Zed's rule), with a
/// bar at the left edge.
pub const DIFF_TINT_ALPHA: f32 = 0.10;
pub const DIFF_BAR_WIDTH: Pixels = px(2.);
/// Space around line numbers and before the code.
pub const DIFF_GUTTER_PADDING: Pixels = kit::scale::SPACE_S;
pub const DIFF_CODE_PADDING: Pixels = kit::scale::SPACE_M;
/// A glyph's advance as a share of the font size, used only if the font cannot be measured.
pub const FALLBACK_ADVANCE: f32 = 0.6;
/// How far a wrapped line's continuation rows are indented under the code column, in columns.
pub const DIFF_HANG_COLUMNS: usize = 2;
/// Spaces a tab takes in a diff.
pub const DIFF_TAB_WIDTH: usize = 4;
/// The toolbar of "Changes" (the commit picker), one row tall.
pub const TOOLBAR_HEIGHT: Pixels = px(ROW);
/// The commit picker's menu.
pub const PICKER_MENU_WIDTH: Pixels = px(420.);
pub const PICKER_MENU_HEIGHT: Pixels = px(360.);

/// The terminal: Ghostty's 13 point text, 8 px from the pane's top and bottom and the content inset from its
/// sides (so its text starts on the line the header starts on). It has no keyboard ring.
pub const TERMINAL_TEXT: f32 = 13.;
/// The sizes the terminal's text can be given in Settings.
pub const TERMINAL_TEXT_SIZES: [u8; 6] = [11, 12, 13, 14, 15, 16];
pub const TERMINAL_PADDING: f32 = 8.;

/// Log tails: line height and the most they take before scrolling.
pub const LOG_LINE_HEIGHT: f32 = 18.;
pub const LOG_MAX_HEIGHT: Pixels = px(320.);

/// The four durations of Sakura's motion. The shortest is also the tick the "Working" breath is redrawn on.
pub const DURATION_INSTANT: Duration = Duration::from_millis(50);
pub const DURATION_FAST: Duration = Duration::from_millis(120);
/// The layout duration: a pane that hides or comes back.
pub const DURATION_BASE: Duration = Duration::from_millis(200);
/// How often a drag-selection held at the edge of a chat scrolls it: once a frame.
pub const DRAG_SCROLL_TICK: Duration = Duration::from_millis(16);
/// The inset that keeps the events of that autoscroll inside the viewport they are meant for.
pub const DRAG_SCROLL_INSET: Pixels = px(1.);
/// The exponent of the ease out of layout movements (a quintic: most of the way in the first third).
pub const EASE_OUT_POWER: i32 = 5;
/// One breath of the "Working" glow.
pub const BREATH: Duration = Duration::from_millis(2400);
/// The interval between retino frames while it drifts.
#[cfg(not(feature = "boxes"))]
pub const RETINO_FRAME: Duration = Duration::from_millis(83);
/// The side of one dot of the retino, on a grid of 4.
#[cfg(not(feature = "boxes"))]
pub const RETINO_DOT_SIZE: Pixels = px(3.);
/// Lowest opacity of the glow during a breath.
pub const BREATH_LOW_OPACITY: f32 = 0.35;

// Sizes, shapes and timings of single views, kept here with every other token.
/// Width of the "Start agent" column.
pub const START_COLUMN_WIDTH: f32 = 400.;
/// "Take" while it is pressed.
pub const TAKE_PRESSED_OPACITY: f32 = 0.85;
/// The sheen runs top to bottom.
pub const SHEEN_ANGLE: f32 = 180.;
pub const TASK_MENU_WIDTH: f32 = 200.;
/// Width of the column that says why `box` cannot be reached.
pub const BLOCKED_COLUMN_WIDTH: f32 = 440.;
/// Widths of the sidebar skeleton's bars, one per row, while the first facts load.
pub const SKELETON_BAR_WIDTHS: [f32; 3] = [132., 104., 156.];
/// Height of a sidebar skeleton bar while the first facts load.
pub const SKELETON_BAR_HEIGHT: f32 = 10.;
/// Centre of the glyph in its 12 px box.
pub const GLYPH_CENTRE: f32 = 6.0;
/// The ring's radius at the middle of its stroke.
pub const GLYPH_RING_RADIUS: f32 = 5.25;
/// The radius of the half disc inside the "Working" ring.
pub const GLYPH_INNER_RADIUS: f32 = 3.0;
/// The radius of the "Da guardare" dot.
pub const GLYPH_DOT_RADIUS: f32 = 5.0;
/// Dash and gap of the "In preparazione" ring.
pub const GLYPH_DASH: f32 = 2.2;
/// The check mark's stroke.
pub const GLYPH_CHECK: [(f32, f32); 3] = [(3.6, 6.1), (5.3, 7.8), (8.4, 4.4)];
/// The cross's two strokes.
pub const GLYPH_CROSS: [(f32, f32); 4] = [(4.2, 4.2), (7.8, 7.8), (7.8, 4.2), (4.2, 7.8)];
/// The warning mark's bar.
pub const GLYPH_BANG: [(f32, f32); 2] = [(6.0, 3.3), (6.0, 6.3)];
/// The warning mark's dot, and its radius.
pub const GLYPH_BANG_DOT: (f32, f32) = (6.0, 8.4);
pub const GLYPH_BANG_DOT_RADIUS: f32 = 0.9;
/// How long a read runs before its view says it is reading (`ui::patience`): a faster one shows nothing.
pub const SLOW_READ: Duration = Duration::from_millis(150);
/// How long the press must last.
pub const HOLD: Duration = Duration::from_millis(1200);
/// How long typing in a composer must pause before its unsent text is written down.
#[cfg_attr(feature = "boxes", allow(dead_code))]
pub const DRAFT_PAUSE: Duration = Duration::from_millis(400);
/// How long Cmd must be held before the shortcuts show, so that a click or a shortcut with it never flashes them.
#[cfg_attr(feature = "boxes", allow(dead_code))]
pub const HINT_DELAY: Duration = Duration::from_millis(300);
#[cfg(not(feature = "boxes"))]
pub const MODIFIER_POLL_INTERVAL: Duration = Duration::from_millis(83);
/// How long the fill takes to go back after an early release.
pub const HOLD_RELEASE: Duration = Duration::from_millis(200);
/// The first line of a row of "Checks": the check's name and its action.
pub const VERIFY_ROW_HEIGHT: Pixels = px(36.);
/// The line height of a state said in a sentence and of the notes under it.
pub const VERIFY_NOTE_LINE_HEIGHT: f32 = 16.;
/// Width of the column that holds a refusal and its action.
pub const CONSOLE_COLUMN_WIDTH: f32 = 400.;
/// The chevron's stroke, in a 12 px box: pointing right when closed, down when open.
pub const CHEVRON_CLOSED: [(f32, f32); 3] = [(4.5, 2.5), (8.0, 6.0), (4.5, 9.5)];
pub const CHEVRON_OPEN: [(f32, f32); 3] = [(2.5, 4.5), (6.0, 8.0), (9.5, 4.5)];

/// The Settings window, Play and its log (`ui::settings`, `ui::play`).
pub mod settings {
    use gpui::{px, Pixels};

    /// The Settings window when it opens, and the smallest it gets.
    pub const SETTINGS_WIDTH: Pixels = px(880.);
    pub const SETTINGS_HEIGHT: Pixels = px(640.);
    pub const SETTINGS_MIN_WIDTH: Pixels = px(720.);
    pub const SETTINGS_MIN_HEIGHT: Pixels = px(480.);

    /// The list of sections at the window's left, with the search field on top.
    pub const SECTIONS_WIDTH: Pixels = px(220.);
    /// The rem the window is drawn at: the one the kit's settings component is made for. Its rows
    /// then set their text at `SETTING_TEXT` and a section's row is a sidebar row tall.
    pub const KIT_REM: Pixels = px(16.);
    /// The text of a row, its title and the sentence under it: the kit's small text at `KIT_REM`.
    pub const SETTING_TEXT: Pixels = super::kit::scale::TEXT_BODY.size;
    /// The line height of the window's text, against its size: a sentence of two lines reads as one.
    pub const SETTING_LINE_HEIGHT: f32 = 1.35;
    /// How far the kit's divider between the sections and the page takes the pointer on each side
    /// of its line.
    pub const KIT_DIVIDER_REACH: Pixels = px(6.);
    /// Between a group's title and its first row.
    pub const GROUP_TITLE_GAP: Pixels = super::GAP;
    /// Between two groups of rows of a form, and between the form's title and its first group.
    pub const GROUP_GAP: Pixels = super::kit::scale::SPACE_XXL;
    /// The section's title.
    pub const PAGE_TITLE: Pixels = super::kit::scale::TEXT_TITLE.size;

    /// The least height of a row's line: a name at the left, its control at the right.
    pub const SETTING_ROW_HEIGHT: Pixels = super::VERIFY_ROW_HEIGHT;
    /// A text field at the right of a row, and the short one of a number or a verb.
    pub const FIELD_WIDTH: Pixels = px(320.);
    pub const FIELD_SHORT_WIDTH: Pixels = px(96.);
    /// The publishing request: how many lines its field shows at least and at most.
    pub const REQUEST_ROWS_MIN: usize = 3;
    pub const REQUEST_ROWS_MAX: usize = 8;
    /// A menu of branches or accounts.
    pub const CHOICE_MENU_WIDTH: Pixels = px(240.);
    /// The most a menu of many choices (the fonts) takes before it scrolls.
    pub const CHOICE_MENU_HEIGHT: Pixels = px(320.);

    /// The log of a Play: the diff's text size and the log line height, in the monospaced face.
    pub const PLAY_LOG_TEXT: Pixels = super::DIFF_TEXT;
    pub const PLAY_LOG_LINE_HEIGHT: Pixels = px(super::LOG_LINE_HEIGHT);
}

/// Settings as a modal over the window: its size, its list of sections and its grouped rows.
#[cfg(not(feature = "boxes"))]
pub mod settings_modal {
    use gpui::{px, Pixels};

    /// The modal's size and the radius of its corners.
    pub const WIDTH: Pixels = px(860.);
    pub const RADIUS: Pixels = super::kit::scale::RADIUS_SURFACE;
    /// The radius of the list's outer corners: the modal's, one ring narrower, since the list is drawn inside
    /// the modal's ring. A square corner there shows outside the modal's rounded one.
    pub const NAV_RADIUS: Pixels = super::kit::scale::RADIUS_PILL;
    /// The list of sections at the left, its inset and the height of one section's row.
    pub const NAV_WIDTH: Pixels = px(200.);
    pub const NAV_PADDING: Pixels = super::kit::scale::SPACE_S;
    pub const NAV_PADDING_TOP: Pixels = super::kit::scale::SPACE_L;
    pub const NAV_ROW_HEIGHT: Pixels = px(30.);
    pub const NAV_TITLE_TEXT: Pixels = super::kit::scale::TEXT_FOOTNOTE.size;
    /// The section's room: its insets, its title and the space between its groups.
    pub const MAIN_PADDING_X: Pixels = super::kit::scale::SPACE_XXL;
    pub const MAIN_PADDING_Y: Pixels = super::kit::scale::SPACE_XXL;
    pub const TITLE_TEXT: Pixels = super::kit::scale::TEXT_TITLE.size;
    pub const GROUP_GAP: Pixels = super::kit::scale::SPACE_XL;
    /// A group of rows: its radius, the least height of a row, and the room at its sides.
    pub const GROUP_RADIUS: Pixels = super::kit::scale::RADIUS_SURFACE;
    pub const ROW_MIN_HEIGHT: Pixels = super::kit::SETTINGS_ROW_MIN_HEIGHT;
    /// A row that holds one short line and nothing under it (a shortcut): as high as a list row.
    pub const COMPACT_ROW_HEIGHT: Pixels = px(36.);
    pub const ROW_PADDING_X: Pixels = super::kit::scale::SPACE_L;
    pub const ROW_PADDING_Y: Pixels = super::kit::scale::SPACE_S;
    pub const ROW_TEXT: Pixels = super::kit::scale::TEXT_BODY.size;
    /// The sentence under a row's name.
    pub const ROW_NOTE_TEXT: Pixels = super::kit::scale::TEXT_FOOTNOTE.size;
    pub const ROW_NOTE_GAP: Pixels = super::kit::scale::SPACE_XS;
    /// A field in a row, and the room a row's control keeps for itself at most.
    pub const FIELD_WIDTH: Pixels = px(300.);
    /// A field in a form: its label stands above it, and it is as wide as the form needs for a command.
    pub const FORM_FIELD_WIDTH: Pixels = px(420.);
    pub const CONNECTOR_STATUS_WIDTH: Pixels = px(132.);
    pub const CONNECTOR_ACTION_WIDTH: Pixels = px(96.);
    pub const CONNECTOR_FEEDBACK_HEIGHT: Pixels = px(72.);
    pub const CONNECTOR_FOOTER_HEIGHT: Pixels = px(180.);
    pub const SWATCH_RING: Pixels = px(26.);
    pub const SWATCH_RING_WIDTH: Pixels = px(2.);
    pub const SWATCH_DISC: Pixels = px(16.);
    /// The close button: a square, its mark.
    pub const CLOSE_SIZE: Pixels = px(28.);
    pub const CLOSE_ICON: Pixels = super::kit::scale::ICON_REGULAR;
}

/// The Engine page: its cards, the drawn pipeline (a box of 640 by 252 points, its nodes and edges placed
/// on it), the pace meters and the running lanes' table.
#[cfg(not(feature = "boxes"))]
pub mod engine_page {
    use gpui::{px, Pixels};

    /// The page's insets and the space between its cards.
    pub const PAGE_PADDING_X: Pixels = super::kit::scale::SPACE_XXL;
    pub const PAGE_PADDING_TOP: Pixels = super::kit::scale::SPACE_XXL;
    pub const PAGE_PADDING_BOTTOM: Pixels = super::kit::scale::SPACE_XXXL;
    pub const CARD_GAP: Pixels = super::kit::scale::SPACE_L;
    /// A card: its radius, its insets and the space between its parts.
    pub const CARD_TITLE_TEXT: Pixels = super::kit::scale::TEXT_HEADLINE.size;
    pub const CARD_NOTE_TEXT: Pixels = super::kit::scale::TEXT_FOOTNOTE.size;
    /// The small text under a name, in a row or a node.
    pub const SMALL_TEXT: Pixels = super::kit::scale::TEXT_FOOTNOTE.size;
    /// A row of a card: its least height and the room of its text.
    pub const ROW_MIN_HEIGHT: Pixels = px(36.);
    pub const ROW_TEXT: Pixels = super::kit::scale::TEXT_BODY.size;

    /// The drawn pipeline's box.
    pub const SCHEMA_WIDTH: Pixels = px(664.);
    pub const SCHEMA_HEIGHT: Pixels = px(262.);
    /// The room of the rules beside the drawing, at least.
    pub const INSPECTOR_MIN_WIDTH: Pixels = px(240.);
    pub const INSPECTOR_CONTROL_WIDTH: Pixels = px(136.);
    pub const INSPECTOR_GAP: Pixels = super::kit::scale::SPACE_XXL;
    /// A node: its radius (a rounded rectangle, never a pill: a two-line node is taller than it is round), the
    /// padding it keeps on every side between its edge and its text, the sizes of its two lines.
    pub const NODE_RADIUS: Pixels = super::kit::scale::RADIUS_SURFACE;
    pub const NODE_PADDING: Pixels = super::kit::scale::SPACE_M;
    pub const NODE_LABEL_TEXT: Pixels = super::kit::scale::TEXT_BODY.size;
    pub const NODE_REVIEW_LABEL_TEXT: Pixels = super::kit::scale::TEXT_FOOTNOTE.size;
    pub const NODE_SUB_TEXT: Pixels = super::kit::scale::TEXT_CAPTION.size;
    /// The edges: their stroke, the dashes of an unused or guard line, and the arrowhead.
    pub const EDGE_WIDTH: Pixels = px(1.4);
    pub const EDGE_DASH: Pixels = px(3.);
    pub const EDGE_GAP: Pixels = super::kit::scale::SPACE_XS;
    pub const EDGE_OFF_OPACITY: f32 = 0.5;
    pub const ARROW_LENGTH: f32 = 7.;
    pub const ARROW_HALF_WIDTH: f32 = 3.5;
    /// What a node that is off is drawn with.
    pub const NODE_OFF_OPACITY: f32 = 0.55;
    /// The fill of a node that is off: the quiet fill, a little paler, still a surface and never an outline.
    pub const NODE_FLIP_FROM: f32 = 0.4;
    /// The dot on a step whose rules differ from the preset's: its size and its distance from the corner.
    pub const NODE_MARK_SIZE: Pixels = px(5.);
    pub const NODE_MARK_INSET: Pixels = px(6.);

    /// Where each node is in the box: left, top, width, height. The steps share one width, the reviews another.
    pub const START: [f32; 4] = [0., 90., 100., 88.];
    pub const PLAN: [f32; 4] = [124., 90., 100., 88.];
    pub const BUILD: [f32; 4] = [248., 90., 100., 88.];
    pub const CHECKS: [f32; 4] = [440., 90., 100., 88.];
    pub const DELIVER: [f32; 4] = [564., 90., 100., 88.];
    pub const PLAN_REVIEW: [f32; 4] = [122., 0., 104., 64.];
    pub const TASK_REVIEW: [f32; 4] = [246., 0., 104., 64.];
    pub const FINAL_REVIEW: [f32; 4] = [366., 0., 104., 64.];
    pub const GUARD: [f32; 4] = [124., 214., 540., 46.];

    /// The straight edges, from tail to head.
    pub const START_TO_PLAN: [(f32, f32); 2] = [(100., 134.), (122., 134.)];
    pub const PLAN_TO_BUILD: [(f32, f32); 2] = [(224., 134.), (246., 134.)];
    pub const BUILD_TO_CHECKS: [(f32, f32); 2] = [(348., 134.), (438., 134.)];
    pub const CHECKS_TO_DELIVER: [(f32, f32); 2] = [(540., 134.), (562., 134.)];
    /// A review's two arrows: up from its step, and back down.
    pub const PLAN_REVIEW_UP: [(f32, f32); 2] = [(158., 88.), (158., 66.)];
    pub const PLAN_REVIEW_DOWN: [(f32, f32); 2] = [(190., 66.), (190., 88.)];
    pub const TASK_REVIEW_UP: [(f32, f32); 2] = [(282., 88.), (282., 66.)];
    pub const TASK_REVIEW_DOWN: [(f32, f32); 2] = [(314., 66.), (314., 88.)];
    /// The curved edges, as the start, two controls and the end.
    pub const START_BYPASS: [(f32, f32); 4] =
        [(100., 156.), (120., 196.), (228., 196.), (246., 156.)];
    pub const BUILD_TO_FINAL: [(f32, f32); 4] =
        [(338., 88.), (338., 78.), (372., 72.), (384., 66.)];
    pub const FINAL_TO_CHECKS: [(f32, f32); 4] =
        [(452., 66.), (466., 72.), (476., 78.), (480., 88.)];
    /// The guard's dashed ties to the steps it stands under.
    pub const GUARD_TIES: [[(f32, f32); 2]; 4] = [
        [(174., 180.), (174., 212.)],
        [(298., 180.), (298., 212.)],
        [(490., 180.), (490., 212.)],
        [(614., 180.), (614., 212.)],
    ];

    /// The pace's big word, its slider's room and the least width of the left column.
    pub const PACE_WORD_TEXT: Pixels = super::kit::scale::TEXT_TITLE_LARGE.size;
    pub const PACE_LEFT_MIN_WIDTH: Pixels = px(260.);
    pub const PACE_SLIDER_WIDTH: Pixels = px(320.);
    pub const PACE_GAP: Pixels = super::kit::scale::SPACE_XXL;
    /// The account meters: their width at most, the room of a name and of the numbers, the bar's height.
    pub const METER_WIDTH: Pixels = px(440.);
    pub const METER_NAME_WIDTH: Pixels = px(132.);
    pub const METER_NUMBERS_WIDTH: Pixels = px(136.);
    /// The least the bar keeps beside its name and its numbers.
    pub const METER_BAR_MIN_WIDTH: Pixels = px(72.);
    pub const METER_BAR_HEIGHT: Pixels = px(6.);
    pub const METER_ROW_HEIGHT: Pixels = px(46.);
    /// How many accounts show before the list scrolls.
    pub const METER_ROWS_SHOWN: f32 = 3.;
    pub const METER_FORECAST_OPACITY: f32 = 0.35;
    /// The tick that marks the line an account is meant to land at: its width and how far it passes the bar.
    pub const TICK_WIDTH: Pixels = px(2.);
    pub const TICK_OVERSHOOT: Pixels = px(3.);
    pub const TICK_RADIUS: Pixels = px(1.);

    /// The running lanes' table: its row, and the room of its columns.
    pub const TABLE_ROW_HEIGHT: Pixels = px(40.);
    pub const TABLE_HEAD_HEIGHT: Pixels = px(28.);
    pub const TABLE_GLYPH_WIDTH: Pixels = px(18.);
    pub const TABLE_PROJECT_WIDTH: Pixels = px(92.);
    pub const TABLE_PRESET_WIDTH: Pixels = px(84.);
    pub const TABLE_PACE_WIDTH: Pixels = px(190.);

    /// An automation's row: the room of its schedule words, and the digest dialog's width and height.
    pub const WHEN_WIDTH: Pixels = px(300.);
    pub const DIGEST_MAX_HEIGHT: Pixels = px(600.);
    pub const DIGEST_RADIUS: Pixels = super::kit::scale::RADIUS_SURFACE;
    pub const DIGEST_PADDING: Pixels = super::kit::scale::SPACE_XL;
    pub const SAVE_FIELD_WIDTH: Pixels = px(200.);
}

/// The sizes, shapes and timings of the agent conversation (`ui::thread`).
pub mod thread {
    use gpui::{px, rems, Pixels, Rems};
    use std::time::Duration;

    /// How long the person has to finish the Codex login in the browser before it is given up.
    #[cfg_attr(feature = "boxes", allow(dead_code))]
    pub const CODEX_LOGIN_PATIENCE: Duration = Duration::from_secs(10 * 60);

    /// The conversation is one column, no wider than this, centred in a wider pane.
    pub const COLUMN_MAX_WIDTH: Pixels = px(720.);
    /// The column's inset from the pane's sides: the content inset, so text starts on the header's line.
    pub const COLUMN_PADDING_X: Pixels = super::CONTENT_PADDING;
    /// Space above and below the whole transcript.
    pub const TRANSCRIPT_PADDING_Y: Pixels = super::kit::scale::SPACE_L;
    /// Space between two rows of the same turn, and before a new prompt.
    pub const ROW_GAP: Pixels = super::kit::scale::SPACE_M;
    pub const TURN_GAP: Pixels = super::kit::scale::SPACE_XXL;

    /// The space between two parts of one answer: the paragraph gap of the kit's markdown view, so an answer
    /// drawn in parts looks as it does drawn whole.
    pub const ANSWER_PART_GAP: Rems = rems(0.75);

    /// The most bytes one part of an answer holds. Prose is cut into parts at its blank lines long before
    /// this; a block that goes on past it (a file printed whole, one endless line) is cut here so that no part
    /// is laid out again whole while megabytes stream into it.
    pub const ANSWER_PART_MAX_BYTES: usize = 64 * 1024;

    /// Prose: the person's and the agent's messages.
    pub const MESSAGE_LINE_HEIGHT: Pixels = px(20.);
    /// The filled block of the person's message.
    pub const PROMPT_PADDING_X: Pixels = super::kit::scale::SPACE_M;
    pub const PROMPT_PADDING_Y: Pixels = super::kit::scale::SPACE_S;
    /// The master's conversation sets the person's words at the right, in at most this share of the column.
    pub const ASIDE_PROMPT_SHARE: f32 = 0.8;

    /// One-line rows: a thought's header, a tool call, a plan's header, a settled permission.
    pub const LINE_ROW_HEIGHT: Pixels = px(28.);
    /// Space between a row's glyph, its words and its state.
    pub const LINE_ROW_GAP: Pixels = super::kit::scale::SPACE_S;
    /// What sits under a one-line row starts under its words, past the glyph.
    pub const LINE_ROW_INDENT: Pixels = px(24.);
    /// Quiet prose under a row: a thought's text, a plan's steps.
    pub const QUIET_LINE_HEIGHT: Pixels = px(20.);
    /// The pitch of a plan's steps.
    pub const PLAN_ROW_HEIGHT: Pixels = px(26.);

    /// A block of monospaced text (a command's output, a file's lines): its padding and its text's leading.
    pub const BLOCK_PADDING_X: Pixels = super::kit::scale::SPACE_M;
    pub const BLOCK_PADDING_Y: Pixels = super::kit::scale::SPACE_S;
    /// Lines of output shown before the rest is folded behind its line count.
    pub const OUTPUT_FOLDED_LINES: usize = 10;
    /// Lines of a diff shown before the rest is folded behind its line count.
    pub const DIFF_FOLDED_LINES: usize = 40;

    /// The permission card.
    pub const CARD_PADDING: Pixels = super::kit::scale::CARD_PADDING;
    pub const CARD_GAP: Pixels = super::kit::scale::SPACE_M;
    /// Buttons in the conversation: the card's choices, "Riavvia", "Accedi".
    pub const BUTTON_HEIGHT: Pixels = super::CONTROL_HEIGHT;
    pub const BUTTON_PADDING_X: Pixels = super::kit::scale::SPACE_M;
    pub const BUTTON_GAP: Pixels = super::kit::scale::SPACE_S;
    /// A pressed button.
    pub const PRESSED_OPACITY: f32 = super::TAKE_PRESSED_OPACITY;

    /// The composer: a filled box with the radius of a surface.
    pub const COMPOSER_RADIUS: Pixels = super::RADIUS_SURFACE;
    pub const COMPOSER_PADDING: Pixels = CARD_PADDING;
    /// The master's cards: the lane's progress line and its mark.
    pub const CARD_PROGRESS_WIDTH: Pixels = px(80.);
    pub const CARD_GLYPH_SIZE: Pixels = super::kit::scale::ICON_REGULAR;
    /// A round button of the master's composer tools, and the chip of an attached file.
    #[cfg_attr(feature = "boxes", allow(dead_code))]
    pub const TOOL_BUTTON_SIZE: Pixels = px(26.);
    #[cfg_attr(feature = "boxes", allow(dead_code))]
    pub const ATTACHMENT_CHIP_MAX_WIDTH: Pixels = px(220.);
    /// The menu of slash commands over the composer: a row, and the most the menu takes before it cuts.
    #[cfg_attr(feature = "boxes", allow(dead_code))]
    pub const SLASH_ROW_HEIGHT: Pixels = px(28.);
    #[cfg_attr(feature = "boxes", allow(dead_code))]
    pub const SLASH_MENU_HEIGHT: Pixels = px(224.);
    /// The menu of a chip in the master's composer (project, model, mode).
    pub const CHIP_MENU_WIDTH: Pixels = px(220.);
    /// The room an entry of the chip's menu keeps at its left for its check.
    pub const CHIP_CHECK_SLOT: Pixels = px(14.);
    /// What a chip menu leaves of the window's height: the composer below the chip and the bars above the menu.
    pub const CHIP_MENU_CLEARANCE: Pixels = px(240.);
    /// The least height a chip menu is given, so it is never a sliver in a short window.
    pub const CHIP_MENU_LEAST_HEIGHT: Pixels = px(160.);
    /// The least a composer chip is cut to when its row is short of room.
    pub const CHIP_LEAST_WIDTH: Pixels = px(56.);
    pub const COMPOSER_MIN_ROWS: usize = 1;
    pub const COMPOSER_MAX_ROWS: usize = 8;
    /// The round send and stop button, and the mark inside it.
    pub const SEND_BUTTON_SIZE: Pixels = px(28.);
    /// The square of the stop mark, in the 16 px icon box.
    pub const STOP_MARK: [(f32, f32); 4] = [(5.0, 5.0), (11.0, 5.0), (11.0, 11.0), (5.0, 11.0)];
    /// The arrow of the send mark: its shaft and its head.
    pub const SEND_SHAFT: [(f32, f32); 2] = [(8.0, 12.0), (8.0, 4.5)];
    pub const SEND_HEAD: [(f32, f32); 3] = [(4.5, 8.0), (8.0, 4.5), (11.5, 8.0)];
    /// The line above the composer that says the agent is working, with the time it has taken.
    pub const STATUS_LINE_HEIGHT: Pixels = px(24.);

    /// Streamed words reach full colour in the base duration.
    pub const STREAM_FADE: Duration = super::DURATION_BASE;
    /// One sweep of the shimmer on a label that waits: the breath's period, one direction only.
    pub const SHIMMER_SWEEP: Duration = super::BREATH;
    /// How often the elapsed time of a turn is drawn again.
    pub const ELAPSED_TICK: Duration = Duration::from_secs(1);
    /// The agent's events are applied once per frame, at most this many at a time.
    pub const EVENT_FRAME: Duration = Duration::from_millis(16);
    /// How long a stored conversation may take to load before it is said not to have: after the agent's own
    /// deadline for the load, which it should have answered by.
    pub const LOAD_STALL: Duration = Duration::from_secs(150);
    pub const EVENT_BATCH: usize = 512;

    /// The marks of a tool call's kind, as strokes in the 16 px icon box.
    pub const KIND_READ: [&[(f32, f32)]; 3] = [
        &[
            (4.5, 2.5),
            (11.5, 2.5),
            (11.5, 13.5),
            (4.5, 13.5),
            (4.5, 2.5),
        ],
        &[(6.5, 6.0), (9.5, 6.0)],
        &[(6.5, 9.0), (9.5, 9.0)],
    ];
    pub const KIND_EDIT: [&[(f32, f32)]; 2] = [
        &[
            (3.5, 12.5),
            (4.2, 9.8),
            (10.5, 3.5),
            (12.5, 5.5),
            (6.2, 11.8),
            (3.5, 12.5),
        ],
        &[(9.0, 5.0), (11.0, 7.0)],
    ];
    pub const KIND_DELETE: [&[(f32, f32)]; 2] =
        [&[(4.5, 4.5), (11.5, 11.5)], &[(11.5, 4.5), (4.5, 11.5)]];
    pub const KIND_MOVE: [&[(f32, f32)]; 2] = [
        &[(3.0, 8.0), (13.0, 8.0)],
        &[(9.5, 4.5), (13.0, 8.0), (9.5, 11.5)],
    ];
    /// The lens of the search mark (centre, radius) and its handle.
    pub const KIND_SEARCH_LENS: ((f32, f32), f32) = ((7.0, 7.0), 3.75);
    pub const KIND_SEARCH: [&[(f32, f32)]; 1] = [&[(9.8, 9.8), (13.0, 13.0)]];
    pub const KIND_EXECUTE: [&[(f32, f32)]; 2] = [
        &[(3.5, 4.5), (7.5, 8.0), (3.5, 11.5)],
        &[(8.5, 12.0), (12.5, 12.0)],
    ];
    pub const KIND_FETCH: [&[(f32, f32)]; 3] = [
        &[(8.0, 3.0), (8.0, 10.0)],
        &[(5.0, 7.0), (8.0, 10.0), (11.0, 7.0)],
        &[(3.5, 13.0), (12.5, 13.0)],
    ];
    pub const KIND_SWITCH: [&[(f32, f32)]; 4] = [
        &[(3.5, 5.5), (12.5, 5.5)],
        &[(10.0, 3.0), (12.5, 5.5), (10.0, 8.0)],
        &[(12.5, 10.5), (3.5, 10.5)],
        &[(6.0, 8.0), (3.5, 10.5), (6.0, 13.0)],
    ];
    /// The ring of "thinking" and of a tool of unknown kind: centre and radius.
    pub const KIND_RING: ((f32, f32), f32) = ((8.0, 8.0), 3.0);
    /// The bar of a cancelled call, inside its ring, in the 12 px mark box.
    pub const STATE_BAR: [(f32, f32); 2] = [(3.8, 6.0), (8.2, 6.0)];
    /// The stroke of a refused call, across its ring, in the 12 px mark box.
    pub const STATE_SLASH: [(f32, f32); 2] = [(3.2, 8.8), (8.8, 3.2)];
}

/// The tab strip: the corner morph, radius 12 and morph distance 28 px. Its height, radii, ring, icon and title are
/// the window's own.
pub mod tabs {
    use std::time::Duration;

    pub const STRIP_HEIGHT: f32 = super::STRIP;
    /// The room over a tab, and the height that leaves it: a tab stands on the strip's bottom edge.
    pub const TOP_INSET: f32 = super::TAB_TOP;
    pub const TAB_HEIGHT: f32 = STRIP_HEIGHT - TOP_INSET;
    /// A tab is as wide as its title asks, between these two.
    pub const TAB_MIN_WIDTH: f32 = 88.;
    pub const TAB_MAX_WIDTH: f32 = 224.;
    /// The gap between two tabs, which a passed tab also slides over.
    pub const TAB_GAP: f32 = 4.;
    /// The panel's corner radius, the tab's top corners and its fillets at full size.
    pub const RADIUS: f32 = super::SURFACE_RADIUS;
    /// How far the active tab's left edge sits from the panel's left edge when the corner has fully morphed.
    /// The panel corner and the tab's left fillet (12 px each) share that gap, so at full size they need 24 px.
    pub const MORPH_DISTANCE: f32 = 28.;
    /// The room kept at the strip's end for the active tab's right fillet and the panel's own corner.
    pub const TRAILING_ROOM: f32 = RADIUS * 2.;
    /// How wide the fade is where tabs that do not fit are cut by the strip's ends, and its direction
    /// (in degrees: from the left edge to the right).
    pub const EDGE_FADE: f32 = 24.;
    pub const FADE_TO_RIGHT: f32 = 90.;
    /// The pointer moves this far before a press on a tab becomes a drag.
    pub const DRAG_THRESHOLD: f32 = 4.;
    /// How far back past a neighbour's midpoint a dragged tab must come before a trade is undone.
    pub const SWAP_HYSTERESIS: f32 = 4.;

    /// The panel's ring and the tab's outline.
    pub const RING_WIDTH: f32 = super::RING;
    /// How far the panel's box runs past the strip's right end. The panel reaches the window's right
    /// edge: its right ring and its right corners are drawn this far outside the window, which clips
    /// them, so the panel's top edge and its fill end on the window's edge with no corner of their own.
    pub const PANEL_OVERHANG: f32 = RADIUS + RING_WIDTH;
    /// The hover fill of a tab on the canvas: inset from the strip's top and bottom, with the selected tab's
    /// radius (the surface radius), so it stands beside the selected tab as one of its kind. The inset leaves a
    /// fill at least twice that radius tall: a shorter one is drawn with a smaller radius, as a pill.
    pub const HOVER_INSET_Y: f32 = 3.;
    pub const HOVER_RADIUS: f32 = super::SURFACE_RADIUS;

    /// Inside a tab: 12 px, the 16 px icon, 8 px, the title, then 12 px or the close button's slot.
    pub const PADDING_START: f32 = 12.;
    pub const ICON_SIZE: f32 = super::ICON;
    pub const ICON_GAP: f32 = 8.;
    pub const PADDING_END: f32 = 12.;
    /// The close button, and the 4 px on each side of it.
    pub const CLOSE_SIZE: f32 = 24.;
    pub const CLOSE_MARGIN: f32 = 4.;
    /// The cross inside the close button: the one icon size.
    pub const CLOSE_ICON_SIZE: f32 = ICON_SIZE;
    /// The title: 13 px medium.
    pub const TITLE_SIZE: f32 = super::BODY_TEXT;

    /// How long a pointer rests on a tab before its first tooltip shows; the next tabs show theirs at once.
    pub const TOOLTIP_DELAY: Duration = Duration::from_millis(700);

    /// What a tab is wide besides its title when it can close, and when it cannot.
    pub const CHROME_CLOSABLE: f32 =
        PADDING_START + ICON_SIZE + ICON_GAP + CLOSE_MARGIN + CLOSE_SIZE + CLOSE_MARGIN;
    pub const CHROME_PLAIN: f32 = PADDING_START + ICON_SIZE + ICON_GAP + PADDING_END;

    /// The step a tab's spring is integrated with, in seconds: small against both springs' periods.
    pub const SPRING_STEP: f32 = 0.001;
    /// No frame is longer than this for a spring: a stalled window does not fling a tab.
    pub const SPRING_LONGEST_FRAME: Duration = Duration::from_millis(64);
    /// Closer than this to its target and slower than this, a sprung value is at rest (px, px per second).
    pub const SPRING_REST_DISTANCE: f32 = 0.05;
    pub const SPRING_REST_SPEED: f32 = 1.0;
}

/// A page that scrolls with the keys: how far an arrow key moves it, and how much of a screenful stays in
/// view when a Page key or Space moves it by one.
#[cfg(not(feature = "boxes"))]
pub mod page_scroll {
    use gpui::{px, Pixels};

    pub const LINE: Pixels = px(48.);
    pub const PAGE_OVERLAP: Pixels = px(48.);
}

/// The command palette.
pub mod palette {
    /// The panel: its width, its height at its tallest (the field over a full list) and the least room
    /// kept at its sides. The panel stands where it would be in the middle of the window's height at
    /// its tallest, and keeps that top while the list shrinks.
    pub const WIDTH: f32 = 560.;
    /// The command palette of the engine's window is wider than the file list and stands at a fixed
    /// distance from the window's top, as the approved prototype draws it.
    #[cfg(not(feature = "boxes"))]
    pub const COMMAND_WIDTH: f32 = 620.;
    #[cfg(not(feature = "boxes"))]
    pub const COMMAND_TOP: f32 = 90.;
    /// The engine's palette lists every group while the window has room: up to this high, and with this much
    /// of the window left under it.
    #[cfg(not(feature = "boxes"))]
    pub const COMMAND_LIST_MAX_HEIGHT: f32 = 640.;
    #[cfg(not(feature = "boxes"))]
    pub const COMMAND_BOTTOM_MARGIN: f32 = 40.;
    /// The size of a lane's state mark in front of its name in the list.
    pub const GLYPH_SIZE: f32 = 16.;
    /// The rem the command list is drawn at: the kit's, at which its frame around a line is a whole number of
    /// pixels (at the window's smaller rem it is not, and a line's height would be rounded by the layout).
    pub const LIST_REM: gpui::Pixels = gpui::px(16.);
    /// The room the command list's library puts above and below the content of each line, in rems of
    /// [`LIST_REM`]: its frame pads a line by one and a half quarter-rems at each side.
    pub const LIBRARY_LINE_FRAME_REMS: f32 = 0.375;
    /// That room in rems of the window's own rem, the interface's body size: six pixels, as
    /// `LIBRARY_LINE_FRAME_REMS` of `LIST_REM` (the tests hold the two together).
    #[cfg(test)]
    pub const LINE_FRAME_REMS: f32 = 6. / 13.;
    pub const FULL_HEIGHT: f32 = LIST_MAX_HEIGHT + 28.;
    pub const SIDE_MARGIN: f32 = 16.;
    /// The list never grows past this; longer lists scroll.
    pub const LIST_MAX_HEIGHT: f32 = 360.;
    /// How much the window behind is dimmed while the palette is open.
    pub const SCRIM_ALPHA: f32 = 0.24;
    /// The layer the palette is drawn on: over the window's content, under tooltips.
    pub const LAYER: usize = 100;
    /// The line said when nothing matches: its padding above and below, and its text size.
    pub const EMPTY_PADDING_Y: f32 = 24.;
    pub const EMPTY_TEXT_SIZE: f32 = super::BODY_TEXT;
}

/// "New task": the sheet with the one field.
pub mod sheet {
    /// The sheet's width. It stands at the centre of the panel.
    pub const WIDTH: f32 = 680.;
    /// The field shows this many lines when empty and grows to this many before it scrolls.
    pub const FIELD_MIN_ROWS: usize = 3;
    pub const FIELD_MAX_ROWS: usize = 10;
    /// The width of the list of each thing chosen under the field.
    pub const MENU_WIDTH: f32 = 260.;
}

/// The page that asks for the folder of a project and shows what was recognised in it.
pub mod setup {
    /// The column everything on the page sits in.
    pub const COLUMN_WIDTH: f32 = 520.;
    /// The room of a line's label, so that every value starts on one line.
    pub const LABEL_WIDTH: f32 = 112.;
    /// Between two lines, and between the lines and the buttons.
    pub const LINE_GAP: f32 = 12.;
    pub const SECTION_GAP: f32 = 24.;
    /// The base branch's list.
    pub const BRANCH_WIDTH: f32 = 220.;
}

/// The pieces the pages share (`ui::kit`) and the design scale they are built from.
pub mod kit;

/// The engine window's bottom row: four quiet icon buttons, with no background of its own.
#[cfg(not(feature = "boxes"))]
pub mod engine_bar {
    use gpui::{px, Pixels};

    /// The bottom row's height, the room at its ends and between two buttons (Zed's status bar is the
    /// reference).
    pub const BAR_HEIGHT: Pixels = px(32.);
    pub const BAR_PADDING_X: Pixels = super::kit::scale::SPACE_M;
    pub const BAR_GAP: Pixels = super::kit::scale::SPACE_S;
    /// An icon button is a square of this side, its icon this large: the icon is big enough to read and
    /// the square to hit.
    pub const ICON_BUTTON: Pixels = px(28.);
    pub const ICON_SIZE: Pixels = px(16.);
}

/// The engine window's sidebar: pages, then the projects with their open lanes. It starts right under the
/// title bar and has no footer.
#[cfg(not(feature = "boxes"))]
pub mod sidebar {
    use gpui::{px, Pixels};

    /// The room at each side of the rows: the fill of a row ends where the title bar's arrows end.
    pub const PADDING_X: Pixels = super::kit::scale::SPACE_S;
    /// A page row's height, its label size, and the gap between its icon and its label.
    pub const ROW_HEIGHT: Pixels = px(30.);
    pub const ROW_TEXT: Pixels = super::kit::scale::TEXT_BODY.size;
    pub const ROW_GAP: Pixels = super::kit::scale::SPACE_S;
    /// A page row's count or pace word.
    pub const COUNT_TEXT: Pixels = super::kit::scale::TEXT_FOOTNOTE.size;
    /// A project's chevron, and the heading's button.
    pub const CHEVRON_SIZE: Pixels = super::kit::scale::ICON_SMALL;
    pub const HEADING_HEIGHT: Pixels = px(22.);
    pub const HEADING_BUTTON: Pixels = px(22.);
    /// The room over the heading, and under it.
    pub const HEADING_MARGIN_TOP: Pixels = super::kit::scale::SPACE_L;
    pub const HEADING_MARGIN_BOTTOM: Pixels = super::kit::scale::SPACE_XS;
    /// A lane's row: its height, the room before its mark, the mark's size and the text size.
    pub const LANE_HEIGHT: Pixels = px(28.);
    pub const LANE_PADDING_START: Pixels = super::kit::scale::SPACE_XXXL;
    pub const LANE_MARK_SIZE: Pixels = super::kit::scale::ICON_REGULAR;
    pub const LANE_TEXT: Pixels = super::kit::scale::TEXT_BODY.size;
    /// A lane's progress line.
    pub const LANE_LINE_WIDTH: Pixels = px(34.);
    /// The space between two rows.
    pub const ROW_PITCH_GAP: Pixels = px(1.);
    /// The share of the ink that fills the open page's row.
    pub const SELECTED_SHARE_DARK: f32 = 0.075;
    pub const SELECTED_SHARE_LIGHT: f32 = 0.06;
}

/// The page bar at the top of the page panel: the arrows, the tabs as pills, the page's own controls.
#[cfg(not(feature = "boxes"))]
pub mod page_bar {
    use gpui::{px, Pixels};

    /// The title bar's height, and the padding at its right end (also the page header's).
    pub const BAR_HEIGHT: Pixels = px(super::TITLE_BAR);
    pub const BAR_PADDING_END: Pixels = super::kit::scale::SPACE_M;
    /// The group at the bar's left, as wide as the sidebar, keeps this much after the forward arrow: the
    /// side padding of the sidebar's rows, so the arrow ends where they end.
    pub const GROUP_PADDING_END: Pixels = super::kit::scale::SPACE_S;
    /// The page panel's header row: its height and the room before the breadcrumb.
    pub const HEADER_HEIGHT: Pixels = px(46.);
    pub const HEADER_PADDING_START: Pixels = super::kit::scale::SPACE_XL;
    /// Between the arrows, the row of tabs, the new-tab button and the controls.
    pub const BAR_GAP: Pixels = super::kit::scale::SPACE_S;
    /// A tab's pill: its height, radius, padding, and the gap between its icon and its title.
    pub const PILL_HEIGHT: Pixels = px(30.);
    pub const PILL_RADIUS: Pixels = super::kit::scale::RADIUS_CONTROL;
    pub const PILL_PADDING_X: Pixels = super::kit::scale::SPACE_M;
    pub const PILL_GAP: Pixels = super::kit::scale::SPACE_S;
    /// A compact pill is as wide as its title asks, between these two; the open one reaches the third.
    pub const PILL_MIN_WIDTH: Pixels = px(96.);
    pub const PILL_MAX_WIDTH: Pixels = px(170.);
    pub const OPEN_PILL_MAX_WIDTH: Pixels = px(460.);
    pub const JOINED_PILL_GAP: Pixels = px(2.);
    pub const SPLIT_EDGE_WIDTH: Pixels = px(24.);
    /// The close button that shows on a pill under the pointer.
    pub const CLOSE_SIZE: Pixels = px(18.);
    pub const CLOSE_RADIUS: Pixels = super::kit::scale::RADIUS_SMALL;
    /// A tab's icon: the marks of the lane states and the page icons share it.
    pub const PILL_ICON_SIZE: Pixels = super::kit::scale::ICON_REGULAR;
    /// The widest the engine's problem is said in its popover.
    pub const PROBLEM_POPOVER_WIDTH: Pixels = px(360.);
}

#[cfg(not(feature = "boxes"))]
pub mod page_split {
    use gpui::{px, Pixels};

    // The side columns and their main content fit together above this width.
    pub const NARROW_PANE_WIDTH: Pixels = px(720.);
    pub const NARROW_PANE_SECONDARY_FRACTION: f32 = 0.35;
    pub const DROP_PREVIEW_OPACITY: f32 = 0.65;
}

/// The Lanes page: its rows, their columns and the project tag.
#[cfg(not(feature = "boxes"))]
pub mod lanes_page {
    use gpui::{px, Pixels};

    /// A lane's row: its height, side padding, the room between its parts and its text.
    pub const ROW_HEIGHT: Pixels = px(38.);
    pub const ROW_PADDING_X: Pixels = super::kit::scale::SPACE_M;
    pub const ROW_GAP: Pixels = super::kit::scale::SPACE_M;
    pub const ROW_TEXT: Pixels = super::kit::scale::TEXT_BODY.size;
    /// The columns after the title: project tag, step with its line, preset and age.
    pub const TAG_WIDTH: Pixels = px(96.);
    pub const STEP_WIDTH: Pixels = px(156.);
    pub const PRESET_WIDTH: Pixels = px(72.);
    pub const AGE_WIDTH: Pixels = px(88.);
    /// The column the state mark stands in, at the row's start.
    pub const GLYPH_COLUMN: Pixels = px(18.);
    pub const LINE_WIDTH: Pixels = px(36.);
    /// A group's header band: its height and the room above it.
    pub const GROUP_HEIGHT: Pixels = px(32.);
    pub const GROUP_GAP: Pixels = super::kit::scale::SPACE_S;
    /// The state mark in a row and in a header.
    pub const GLYPH_SIZE: Pixels = super::kit::scale::ICON_REGULAR;
    /// The room under the last group.
    pub const LIST_PADDING_BOTTOM: Pixels = super::kit::scale::SPACE_XL;
    /// The row of column names over the list.
    pub const COLUMNS_HEIGHT: Pixels = px(28.);
}

/// The page of a morning digest, read as a report.
#[cfg(not(feature = "boxes"))]
pub mod digest_page {
    use gpui::{px, Pixels};

    /// How wide a line of the report runs, and the room above and below it.
    pub const READING_WIDTH: Pixels = px(700.);
    pub const PADDING_Y: Pixels = super::kit::scale::SPACE_XXL;
    /// The gap between the report, the asks and the links; the asks' rows; a section's name over its rows.
    pub const SECTION_GAP: Pixels = super::kit::scale::SPACE_XXL;
    pub const ROW_GAP: Pixels = super::kit::scale::SPACE_XS;
    pub const HEADING_GAP: Pixels = super::kit::scale::SPACE_S;
    /// An ask's row: its height and side padding.
    pub const ROW_HEIGHT: Pixels = px(36.);
    pub const ROW_PADDING_X: Pixels = super::kit::scale::SPACE_M;
    /// The report's text.
    pub const TEXT: Pixels = super::kit::scale::TEXT_BODY.size;
}

/// The New lane sheet: a title, what the lane should do, and the chips under them.
#[cfg(not(feature = "boxes"))]
pub mod new_lane {
    use gpui::{px, Pixels};

    /// The sheet's corner radius.
    pub const RADIUS: Pixels = super::kit::scale::RADIUS_SURFACE;
    /// The sheet's height: the description takes what the title, the header and the footer leave.
    pub const HEIGHT: Pixels = px(292.);
    /// The space around the header line, the title and the description.
    pub const PADDING_X: Pixels = super::kit::scale::SPACE_XL;
    pub const HEADER_PADDING_TOP: Pixels = super::kit::scale::SPACE_L;
    pub const TITLE_PADDING_TOP: Pixels = super::kit::scale::SPACE_M;
    /// The title's and the description's text.
    pub const TITLE_TEXT: Pixels = super::kit::scale::TEXT_TITLE.size;
    pub const DESCRIPTION_TEXT: Pixels = super::kit::scale::TEXT_BODY.size;
    /// The description shows this many lines when empty and grows to this many before it scrolls.
    pub const DESCRIPTION_MIN_ROWS: usize = 1;
    pub const DESCRIPTION_MAX_ROWS: usize = 12;
    /// The footer: the space above its line, and its padding.
    pub const FOOTER_MARGIN_TOP: Pixels = super::kit::scale::SPACE_M;
    pub const FOOTER_PADDING_X: Pixels = super::kit::scale::SPACE_L;
    pub const FOOTER_PADDING_Y: Pixels = super::kit::scale::SPACE_M;
    /// Its padding, and the gap between its lines.
    pub const NO_PROJECT_PADDING: Pixels = super::kit::scale::SPACE_XL;
    pub const NO_PROJECT_GAP: Pixels = super::kit::scale::SPACE_S;
}

/// The Inbox page: the list of asks at the left, the selected ask at the right.
#[cfg(not(feature = "boxes"))]
pub mod inbox {
    use gpui::{px, Pixels};

    /// The list's width, and the room around its rows.
    pub const LIST_WIDTH: Pixels = px(384.);
    pub const LIST_PADDING: Pixels = super::kit::scale::SPACE_S;
    pub const LIST_PADDING_BOTTOM: Pixels = super::kit::scale::SPACE_L;
    /// A group's band: a soft header over the asks of one lane.
    pub const BAND_HEIGHT: Pixels = px(32.);
    pub const BAND_GAP_ABOVE: Pixels = super::kit::scale::SPACE_S;
    pub const BAND_PADDING_X: Pixels = super::kit::scale::SPACE_S;
    pub const BAND_TEXT: Pixels = super::kit::scale::TEXT_FOOTNOTE.size;
    pub const BAND_MARK_SIZE: Pixels = super::kit::scale::ICON_REGULAR;
    /// An ask's row: its padding, the room of its unread dot and the dot itself.
    pub const ROW_RADIUS: Pixels = super::kit::scale::RADIUS_CONTROL;
    pub const ROW_PADDING_X: Pixels = super::kit::scale::SPACE_S;
    pub const AGE_WIDTH: Pixels = super::lanes_page::AGE_WIDTH;
    pub const ROW_PADDING_TOP: Pixels = super::kit::scale::SPACE_S;
    pub const ROW_PADDING_BOTTOM: Pixels = super::kit::scale::SPACE_S;
    pub const ROW_GAP_ABOVE: Pixels = super::kit::scale::SPACE_XS;
    pub const ROW_LINE_GAP: Pixels = super::kit::scale::SPACE_XS;
    pub const UNREAD_COLUMN: Pixels = px(10.);
    pub const UNREAD_DOT: Pixels = px(6.);
    /// The selected ask: the room around what it says, and how wide a line of it may run.
    pub const DETAIL_PADDING_X: Pixels = super::kit::scale::SPACE_XXXL;
    pub const DETAIL_PADDING_TOP: Pixels = super::kit::scale::SPACE_XXL;
    pub const DETAIL_PADDING_BOTTOM: Pixels = super::kit::scale::SPACE_XL;
    pub const TITLE_TEXT: Pixels = super::kit::scale::TEXT_TITLE.size;
    pub const TITLE_MAX_WIDTH: Pixels = px(560.);
    pub const DETAIL_COLUMN_WIDTH: Pixels = px(280.);
    pub const PROPERTIES_WIDTH: Pixels = px(200.);
    pub const SOURCE_TEXT: Pixels = super::kit::scale::TEXT_FOOTNOTE.size;
    pub const SOURCE_GAP_BELOW: Pixels = super::kit::scale::SPACE_XL;
    pub const BODY_TEXT: Pixels = super::kit::scale::TEXT_BODY.size;
    pub const BODY_MAX_WIDTH: Pixels = px(580.);
    pub const CONTEXT_GAP_ABOVE: Pixels = super::kit::scale::SPACE_XL;
    pub const CONTEXT_PADDING_X: Pixels = super::kit::scale::SPACE_L;
    pub const CONTEXT_PADDING_Y: Pixels = super::kit::scale::SPACE_M;
    pub const CONTEXT_RADIUS: Pixels = super::kit::scale::RADIUS_CONTROL;
    pub const CONTEXT_BAR_WIDTH: Pixels = px(120.);
    /// The row of actions at the bottom of the selected ask.
    pub const ACTIONS_PADDING_TOP: Pixels = super::kit::scale::SPACE_M;
    pub const ACTIONS_PADDING_BOTTOM: Pixels = super::kit::scale::SPACE_L;
    pub const ACTIONS_GAP: Pixels = super::kit::scale::SPACE_S;
    /// The note field: a filled box, never outlined.
    pub const NOTE_RADIUS: Pixels = super::kit::scale::RADIUS_CONTROL;
    pub const NOTE_PADDING_X: Pixels = super::kit::scale::SPACE_M;
    /// The widest the menu of the filter and of Open and Done is.
    pub const MENU_WIDTH: Pixels = px(200.);
    /// The gap between the parts of an ask's source line.
    pub const SOURCE_GAP: Pixels = super::kit::scale::SPACE_M;
    /// How long an answer is held under the Undo toast before it is sent.
    pub const HOLD: std::time::Duration = std::time::Duration::from_secs(6);
    /// How long the calls that check a question and confirm it are given, at the end of a hold and at the
    /// window's close or the app's quit.
    pub const CONFIRM_BOUND: std::time::Duration = std::time::Duration::from_secs(15);
}

/// The project page: the column its labels (Folder, Branch) stand in.
#[cfg(not(feature = "boxes"))]
pub mod project_page {
    use gpui::{px, Pixels};
    pub const LABEL_COLUMN: Pixels = px(72.);
}

/// The lane page: the header that never scrolls, the conversation under it and the column at its right.
#[cfg(not(feature = "boxes"))]
pub mod lane_page {
    use gpui::{px, Pixels};

    /// The header: the room around the lane's name, its state and its steps.
    pub const HEADER_PADDING_X: Pixels = px(40.);
    pub const HEADER_PADDING_TOP: Pixels = super::kit::scale::SPACE_L;
    pub const HEADER_PADDING_BOTTOM: Pixels = super::kit::scale::SPACE_L;
    pub const TITLE_TEXT: Pixels = super::kit::scale::TEXT_TITLE.size;
    /// The prototype sets no line height, so a line is about 1.2 times its text: the title's and the track's.
    pub const TITLE_LINE_HEIGHT: Pixels = px(21.);
    pub const TRACK_LABEL_LINE_HEIGHT: Pixels = super::kit::scale::TEXT_CAPTION.line_height;
    pub const STATE_TEXT: Pixels = super::kit::scale::TEXT_FOOTNOTE.size;
    pub const TITLE_GAP_BELOW: Pixels = super::kit::scale::SPACE_L;
    /// The step track: its segments' height and pitch, and the words under them.
    pub const TRACK_MAX_WIDTH: Pixels = px(620.);
    pub const TRACK_SEGMENT_WIDTH: Pixels = px(68.);
    pub const STEPS_MENU_MAX_WIDTH: Pixels = px(380.);
    /// The room between the step bars and the Steps menu, and the width of that menu.
    pub const TRACK_MENU_GAP: Pixels = super::kit::scale::SPACE_L;
    pub const TRACK_BAR_HEIGHT: Pixels = px(4.);
    pub const TRACK_BAR_RADIUS: Pixels = px(2.);
    pub const TRACK_GAP: Pixels = super::kit::scale::SPACE_XS;
    pub const TRACK_LABEL_GAP: Pixels = super::kit::scale::SPACE_S;
    pub const TRACK_LABEL_TEXT: Pixels = super::kit::scale::TEXT_FOOTNOTE.size;
    /// How much of the segment in progress is filled.
    pub const TRACK_NOW_SHARE: f32 = 0.55;
    /// The conversation: its room, how wide it runs and the gap between messages.
    pub const FEED_PADDING_X: Pixels = px(40.);
    /// The conversation's column is as wide as the box under it, so the two stand on one line at any width.
    pub const FEED_MAX_WIDTH: Pixels = COMPOSER_MAX_WIDTH;
    pub const FEED_GAP: Pixels = super::kit::scale::SPACE_XL;
    /// The fade at the top of the conversation, where text passes under the header.
    pub const TOP_FADE_HEIGHT: Pixels = px(14.);
    pub const AVATAR_SIZE: Pixels = px(22.);
    pub const AVATAR_TEXT: Pixels = super::kit::scale::TEXT_CAPTION.size;
    pub const AVATAR_GAP: Pixels = super::kit::scale::SPACE_M;
    pub const MESSAGE_TEXT: Pixels = super::kit::scale::TEXT_BODY.size;
    pub const WHO_GAP_BELOW: Pixels = super::kit::scale::SPACE_XS;
    /// A message folds to this many lines, counting a long line as it wraps at `FOLD_COLUMNS` characters.
    pub const FOLD_LINES: usize = 12;
    pub const FOLD_COLUMNS: usize = 90;
    pub const MORE_GAP_ABOVE: Pixels = super::kit::scale::SPACE_XS;
    /// A result card, an ask under a message and the one-line notices.
    pub const CARD_GAP: Pixels = super::kit::scale::SPACE_S;
    pub const NOTICE_TEXT: Pixels = super::kit::scale::TEXT_FOOTNOTE.size;
    /// The composer under the conversation.
    pub const COMPOSER_PADDING_X: Pixels = px(40.);
    pub const COMPOSER_PADDING_TOP: Pixels = super::kit::scale::SPACE_M;
    pub const COMPOSER_PADDING_BOTTOM: Pixels = super::kit::scale::SPACE_L;
    /// The box the person writes in: how wide it runs, its corners and the room inside it.
    pub const COMPOSER_MAX_WIDTH: Pixels = px(720.);
    pub const COMPOSER_RADIUS: Pixels = super::kit::scale::RADIUS_SURFACE;
    pub const COMPOSER_INNER_LEFT: Pixels = px(14.);
    pub const COMPOSER_INNER_TOP: Pixels = px(12.);
    pub const COMPOSER_INNER_RIGHT: Pixels = px(12.);
    pub const COMPOSER_INNER_BOTTOM: Pixels = px(10.);
    pub const COMPOSER_GAP: Pixels = super::kit::scale::SPACE_M;
    /// How often the list of a lane's agents is read again while its page is shown, and how often the
    /// elapsed time of a tool that runs is drawn again.
    pub const AGENTS_TICK: std::time::Duration = std::time::Duration::from_secs(3);
    pub const AGENTS_CLOCK_TICK: std::time::Duration = std::time::Duration::from_secs(1);
    /// How often one agent's conversation is read again while the agent works, and while it does not.
    pub const AGENT_CHAT_LIVE_TICK: std::time::Duration = std::time::Duration::from_millis(500);
    pub const AGENT_CHAT_IDLE_TICK: std::time::Duration = std::time::Duration::from_secs(3);
    pub const COMPOSER_MIN_ROWS: usize = 1;
    pub const COMPOSER_MAX_ROWS: usize = 8;
}

/// The two forms of a choice between a few words. The inline form hugs its words (a pace, a preset, a
/// theme); the full-width form spans its container in equal parts (the switch of a column's views).
pub mod segments {
    use super::kit::scale;
    use gpui::{px, Pixels};

    /// One shape for both forms: a track of the control radius around parts of the small control's height, their
    /// words in the one control label.
    pub const TRACK_RADIUS: Pixels = scale::RADIUS_CONTROL;
    pub const TRACK_PADDING: Pixels = scale::SPACE_XS;
    pub const PART_HEIGHT: Pixels = scale::CONTROL_SMALL;
    pub const PART_RADIUS: Pixels = scale::RADIUS_SMALL;
    /// The gap between two parts of the inline form, and the room of a word at each side.
    pub const INLINE_GAP: Pixels = px(1.);
    pub const INLINE_PART_PADDING_X: Pixels = scale::SPACE_S;
}

/// The column at the right of the lane page: Details, Changes and Files.
#[cfg(not(feature = "boxes"))]
pub mod lane_side {
    use gpui::{px, Pixels};

    pub const WIDTH: Pixels = px(316.);
    pub const SWITCH_MARGIN_X: Pixels = super::kit::scale::SPACE_M;
    pub const SWITCH_MARGIN_TOP: Pixels = super::kit::scale::SPACE_M;
    pub const SWITCH_MARGIN_BOTTOM: Pixels = super::kit::scale::SPACE_S;
    pub const BODY_PADDING_X: Pixels = super::kit::scale::SPACE_L;
    pub const BODY_PADDING_BOTTOM: Pixels = super::kit::scale::SPACE_L;
    /// The properties of Details: the width of the names, and the height of one row.
    pub const PROP_NAME_WIDTH: Pixels = px(92.);
    pub const PROP_HEIGHT: Pixels = px(30.);
    /// The room of the state word at the right of a step's row ("Done", "Now", "Skipped").
    pub const STATE_WIDTH: Pixels = px(52.);
    pub const SUBHEAD_GAP_ABOVE: Pixels = super::kit::scale::SPACE_L;
    pub const SUBHEAD_GAP_BELOW: Pixels = super::kit::scale::SPACE_S;
    /// A file of Changes, and a line of a diff.
    pub const FILE_ROW_HEIGHT: Pixels = px(30.);
    pub const FILE_ROW_RADIUS: Pixels = super::kit::scale::RADIUS_CONTROL;
    pub const FILE_TEXT: Pixels = super::kit::scale::TEXT_CAPTION.size;
    pub const DIFF_RADIUS: Pixels = super::kit::scale::RADIUS_CONTROL;
    /// The most files Changes lists before it says how many more there are.
    pub const FILES_LISTED: usize = 12;
    /// The switch of Files: its row's height.
    pub const FOLLOW_HEIGHT: Pixels = px(30.);
    /// An ask under Details, and the menu of presets.
    pub const ASK_RADIUS: Pixels = super::kit::scale::RADIUS_CONTROL;
    pub const ASK_PADDING_X: Pixels = super::kit::scale::SPACE_M;
    pub const ASK_PADDING_Y: Pixels = super::kit::scale::SPACE_M;
    pub const PRESET_MENU_WIDTH: Pixels = px(180.);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_line_frame_in_window_rems_is_the_library_frame_at_the_list_rem() {
        let in_window_rems = f32::from(TEXT_BODY) * palette::LINE_FRAME_REMS;
        let in_list_rems = f32::from(palette::LIST_REM) * palette::LIBRARY_LINE_FRAME_REMS;
        assert_eq!(in_window_rems, in_list_rems);
        assert_eq!(in_list_rems.fract(), 0., "a whole number of pixels");
    }

    /// Every `.rs` file under `dir`, recursively.
    fn sources(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
        let entries = std::fs::read_dir(dir).expect("a source folder");
        entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .flat_map(|path| match path.is_dir() {
                true => sources(&path),
                false => vec![path]
                    .into_iter()
                    .filter(|p| p.extension().is_some_and(|e| e == "rs"))
                    .collect(),
            })
            .collect()
    }

    /// Whether `path` holds only tests: `x_tests.rs` beside a view, or the `tests.rs` of a module's folder.
    fn is_test_file(path: &std::path::Path) -> bool {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        name == "tests.rs" || name.ends_with("_tests.rs")
    }

    #[test]
    fn interface_modules_define_no_sizes_or_durations_of_their_own() {
        let ui = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui");
        let kinds = [": f32 =", ": Pixels =", ": Duration =", ": [(f32, f32)"];
        let offenders: Vec<String> = sources(&ui)
            .into_iter()
            .filter(|path| !is_test_file(path))
            .flat_map(|path| {
                let text = std::fs::read_to_string(&path).expect("a source file");
                let body = text
                    .split("#[cfg(test)]")
                    .next()
                    .unwrap_or_default()
                    .to_string();
                body.lines()
                    .filter(|line| {
                        line.trim_start().starts_with("const ") || line.contains(" const ")
                    })
                    .filter(|line| kinds.iter().any(|kind| line.contains(kind)))
                    .map(|line| format!("{}: {}", path.display(), line.trim()))
                    .collect::<Vec<_>>()
            })
            .collect();
        assert!(
            offenders.is_empty(),
            "sizes belong in theme.rs: {offenders:#?}"
        );
    }

    /// Whether `line` sets a spacing with a number written in the view: a helper named after a step of
    /// the spacing scale (`gap_3`, `px_2`, `pt_1`), or a pixel literal other than zero.
    fn has_literal_spacing(line: &str) -> bool {
        const HELPERS: [&str; 17] = [
            ".gap_", ".p_", ".px_", ".py_", ".pt_", ".pb_", ".pl_", ".pr_", ".m_", ".mx_", ".my_",
            ".mt_", ".mb_", ".ml_", ".mr_", ".w_", ".h_",
        ];
        let is_step = |rest: &str| {
            let step: String = rest.chars().take_while(|c| *c != '(').collect();
            step.starts_with(|c: char| c.is_ascii_digit()) && step != "0"
        };
        let has_helper = HELPERS.iter().any(|helper| {
            line.match_indices(helper)
                .any(|(at, _)| is_step(&line[at + helper.len()..]))
        });
        let has_pixels = line.match_indices("px(").any(|(at, _)| {
            let value = &line[at + "px(".len()..];
            value.starts_with(|c: char| c.is_ascii_digit()) && !value.starts_with("0.)")
        });
        has_helper || has_pixels
    }

    #[test]
    fn the_views_take_every_spacing_from_the_tokens() {
        assert!(has_literal_spacing("    .gap_3()"));
        assert!(has_literal_spacing("    .w(px(12.))"));
        assert!(!has_literal_spacing("    .top_0().left_0().w(px(0.))"));
        assert!(!has_literal_spacing(
            "    .gap(theme::GAP_WIDE).px(theme::ROW_PADDING_X)"
        ));
        let ui = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui");
        let offenders: Vec<String> = sources(&ui)
            .into_iter()
            .filter(|path| !is_test_file(path))
            .flat_map(|path| {
                let text = std::fs::read_to_string(&path).expect("a source file");
                let body = text.split("#[cfg(test)]").next().unwrap_or_default();
                body.lines()
                    .filter(|line| !line.trim_start().starts_with("//"))
                    .filter(|line| has_literal_spacing(line))
                    .map(|line| format!("{}: {}", path.display(), line.trim()))
                    .collect::<Vec<_>>()
            })
            .collect();
        assert!(
            offenders.is_empty(),
            "spacing belongs to the grid of theme.rs: {offenders:#?}"
        );
    }

    #[test]
    fn panel_ring_keeps_the_token_alpha() {
        let ring: Hsla = rgba(dark::PANEL_RING).into();
        assert!((ring.a - 0.07).abs() < 0.005);
        let ring: Hsla = rgba(light::PANEL_RING).into();
        assert!((ring.a - 0.06).abs() < 0.005);
    }

    #[test]
    fn forced_appearance_reads_only_dark_and_light() {
        assert_eq!(Appearance::forced(Some("light")), Some(Appearance::Light));
        assert_eq!(Appearance::forced(Some("dark")), Some(Appearance::Dark));
        assert_eq!(Appearance::forced(Some("blue")), None);
        assert_eq!(Appearance::forced(None), None);
    }

    #[test]
    fn light_primary_action_is_a_graphite_fill_with_light_ink() {
        let palette = Palette::light();
        assert_eq!(
            palette.action_face,
            ActionFace::Graphite(rgb(light::PRIMARY).into())
        );
        assert_eq!(
            palette.action_ink,
            Hsla::from(rgb(light::PRIMARY_FOREGROUND))
        );
        assert!(palette.action_ink.l > 0.9);
    }

    #[test]
    fn dark_primary_action_keeps_the_silver_relief() {
        let palette = Palette::dark();
        assert!(matches!(palette.action_face, ActionFace::Silver { .. }));
        assert!(palette.action_ink.l < 0.1);
        assert!(!action_relief(Appearance::Dark).is_empty());
        assert!(action_relief(Appearance::Light).is_empty());
    }

    /// The files that draw a link, where the hand is right: the click takes the person to another page.
    const LINK_FILES: [&str; 3] = ["titled_links.rs", "about.rs", "detail.rs"];

    #[test]
    fn only_links_show_the_hand_cursor_and_rows_buttons_and_pills_keep_the_arrow() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders = Vec::new();
        for path in sources(&root)
            .into_iter()
            .filter(|path| !is_test_file(path))
        {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("");
            let text = std::fs::read_to_string(&path).expect("a source file");
            let body = text.split("#[cfg(test)]").next().unwrap_or_default();
            if body.contains("cursor_pointer") && !LINK_FILES.contains(&name) {
                offenders.push(path.display().to_string());
            }
        }
        assert!(
            offenders.is_empty(),
            "the hand outside a link: {offenders:?}"
        );
    }

    /// How far a fill stands from the ground it sits on (the panel), in lightness.
    fn lift(fill: Hsla, ground: Hsla) -> f32 {
        (fill.l - ground.l).abs()
    }

    #[test]
    fn a_selected_row_stands_further_from_the_panel_than_any_fill_under_the_pointer() {
        for palette in [Palette::dark(), Palette::light()] {
            let selected = lift(palette.selected_row, palette.panel);
            for hover in [palette.hover, palette.fill] {
                assert!(
                    selected > lift(hover, palette.panel) + 0.02,
                    "{:?}: selection {selected} against hover {hover:?}",
                    palette.appearance
                );
            }
        }
    }

    #[test]
    fn the_words_on_a_selected_row_read_as_text() {
        for palette in [Palette::dark(), Palette::light()] {
            let ratio =
                crate::theme_contrast::contrast(palette.text_secondary, palette.selected_row);
            assert!(
                ratio >= crate::theme_contrast::TEXT,
                "{:?} {ratio}",
                palette.appearance
            );
        }
    }

    fn is_blue(colour: Hsla) -> bool {
        (0.5..0.75).contains(&colour.h) && colour.s > 0.2
    }

    /// Every colour of the interface: every field of the palette, destructured without `..` so a new token
    /// cannot be added without being checked.
    fn interface_colours(palette: Palette) -> Vec<Hsla> {
        let Palette {
            appearance: _,
            canvas,
            panel,
            raised,
            grouped,
            scrim,
            overlay_surface,
            overlay_edge,
            toggle_off,
            hover,
            text,
            text_secondary,
            text_tertiary,
            panel_ring,
            border,
            primary,
            primary_foreground,
            danger,
            success,
            warning,
            attention,
            attention_ink,
            selected_row,
            segment_track,
            segment_selected,
            panel_edge,
            ring,
            scrollbar_thumb,
            selection,
            inset,
            fill,
            fill_hover,
            hold_fill,
            hold_ink,
            action_face,
            action_ink,
            retino_dot,
        } = palette;
        let face = match action_face {
            ActionFace::Silver { top, bottom } => vec![top, bottom],
            ActionFace::Graphite(fill) => vec![fill],
        };
        let mut colours = vec![
            canvas,
            panel,
            raised,
            grouped,
            scrim,
            overlay_surface,
            overlay_edge,
            toggle_off,
            hover,
            text,
            text_secondary,
            text_tertiary,
            panel_ring,
            border,
            primary,
            primary_foreground,
            danger,
            success,
            warning,
            attention,
            attention_ink,
            selected_row,
            segment_track,
            segment_selected,
            ring,
            scrollbar_thumb,
            selection,
            inset,
            fill,
            fill_hover,
            hold_fill,
            hold_ink,
            action_ink,
            retino_dot,
        ];
        colours.extend(panel_edge);
        colours.extend(face);
        colours
    }

    #[test]
    fn no_colour_of_the_interface_is_blue() {
        for palette in [Palette::dark(), Palette::light()] {
            for colour in interface_colours(palette) {
                assert!(!is_blue(colour), "{colour:?}");
            }
        }
    }

    #[cfg(feature = "boxes")]
    #[test]
    fn the_terminal_palette_is_the_one_exception_and_shows_the_blue_programs_ask_for() {
        const ANSI_BLUE: usize = 4;
        const ANSI_BRIGHT_BLUE: usize = 12;
        for appearance in [Appearance::Dark, Appearance::Light] {
            let ansi = terminal_style(appearance).palette.ansi;
            for index in [ANSI_BLUE, ANSI_BRIGHT_BLUE] {
                assert!(
                    is_blue(rgb(ansi[index]).into()),
                    "{appearance:?} ANSI {index}"
                );
            }
            let palette = match appearance {
                Appearance::Dark => Palette::dark(),
                Appearance::Light => Palette::light(),
            };
            let interface = interface_colours(palette);
            let leaked = ansi
                .iter()
                .map(|&value| Hsla::from(rgb(value)))
                .filter(|c| is_blue(*c));
            assert!(leaked.into_iter().all(|blue| !interface.contains(&blue)));
        }
    }
}
