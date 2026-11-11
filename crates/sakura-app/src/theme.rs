//! Every colour, size, radius and duration of the window, in one set of tokens. No other module writes a colour literal: views
//! read the current [`Palette`] from the [`Theme`] global and sizes from the constants here.

use gpui::{
    point, px, rgb, rgba, App, BoxShadow, FontWeight, Global, Hsla, Pixels, WindowAppearance,
};
use sakura_terminal::{TerminalPalette, TerminalStyle};
use std::time::Duration;

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
    pub const SIDEBAR_ACCENT: u32 = 0x111112;
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
    pub const SIDEBAR_ACCENT: u32 = 0xe5e5e5;
    pub const RING: u32 = 0x1c1c1e;
    /// `--scrollbar-thumb`: black at 28 percent.
    pub const SCROLLBAR_THUMB: u32 = 0x00000047;
    /// Selected text: black at 12 percent.
    pub const SELECTION: u32 = 0x0000001f;
    pub const SURFACE_INSET: u32 = 0xf5f5f5;
    pub const SHEET_FILL: u32 = 0xececf0;
    pub const SHEET_FILL_HOVER: u32 = 0xe2e2e8;
}

/// The terminal's ANSI colours. Sakura's tokens define none, so these are Zed's One Dark and One Light terminal
/// palettes with red, green and yellow replaced by Sakura's danger, success and warning tones, and black and white
/// matched to the panel.
///
/// The one exception to "no blue": "no blue" is a rule for the interface, and these are the colours programs
/// ask for. ANSI blue and bright blue stay blue (an `ls` directory, a prompt), because a terminal that shows a
/// different colour than the program requested is a broken terminal. Nothing of the interface reads them.
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
/// The terminal's selection tint: the text colour at this opacity, like the interface's selection.
const TERMINAL_SELECTION_ALPHA_DARK: f32 = 0.18;
const TERMINAL_SELECTION_ALPHA_LIGHT: f32 = 0.12;

/// The one accent, in both appearances, with one meaning only: what needs the person. It is the brand's
/// fuchsia; whatever stands on it is the brand's black (`theme_brand::BLACK`).
const ACCENT: u32 = crate::theme_brand::FUCHSIA;
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
    pub attention: Hsla,
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
}

impl Palette {
    /// The dark tokens.
    pub fn dark() -> Self {
        use dark::*;
        Self {
            appearance: Appearance::Dark,
            canvas: rgb(SURFACE_CANVAS).into(),
            panel: rgb(SURFACE_PANEL).into(),
            raised: rgb(SURFACE_RAISED).into(),
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
            attention: rgb(ACCENT).into(),
            selected_row: rgb(SIDEBAR_ACCENT).into(),
            segment_track: rgb(SURFACE_RAISED).into(),
            segment_selected: rgb(SURFACE_HOVER).into(),
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
        }
    }

    /// The light tokens.
    pub fn light() -> Self {
        use light::*;
        Self {
            appearance: Appearance::Light,
            canvas: rgb(SURFACE_CANVAS).into(),
            panel: rgb(SURFACE_PANEL).into(),
            raised: rgb(SURFACE_RAISED).into(),
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
            attention: rgb(ACCENT).into(),
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
        }
    }
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
    pub fn palette(self) -> Palette {
        match self {
            Appearance::Dark => Palette::dark(),
            Appearance::Light => Palette::light(),
        }
    }
}

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
}

impl Global for Theme {}

impl Theme {
    /// Installs or replaces the global for `appearance`, and gives the kit's components (menus, scrollbars)
    /// the same colours and fonts.
    pub fn set(appearance: Appearance, cx: &mut App) {
        let palette = appearance.palette();
        cx.set_global(Theme { palette });
        sync_kit(&palette, cx);
    }

    /// The current palette.
    pub fn palette(cx: &App) -> Palette {
        cx.global::<Theme>().palette
    }
}

/// How dark the veil over what a sheet, quick open or the palette opens over is, in both appearances.
pub const SHEET_DIM: f32 = 0.6;

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
    // What a sheet or a palette opens over steps back: on a black panel the kit's own faint black
    // veil changes nothing, and the page under the sheet competes with it.
    colours.overlay = gpui::hsla(0., 0., 0., SHEET_DIM);
    colours.border = palette.border;
    colours.popover = palette.raised;
    colours.popover_foreground = palette.text;
    colours.accent = palette.hover;
    colours.accent_foreground = palette.text;
    colours.ring = palette.ring;
    colours.selection = palette.selection;
    colours.scrollbar = gpui::transparent_black();
    colours.scrollbar_thumb = palette.scrollbar_thumb;
    colours.scrollbar_thumb_hover = palette.scrollbar_thumb;
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
// ---- Layout grid --------------------------------------------------------------------------------------
// One vertical line per column, one horizontal line per row, one size per kind of thing. The grid is
// written once, here. The views take
// every spacing from these tokens, and `ui::grid_tests` fails when an edge, a centre line or a size drifts.
// The numbers the others are derived from:
const STRIP: f32 = 36.;
const HEADER: f32 = 44.;
const ROW: f32 = 28.;
const RING: f32 = 1.;
const SIDEBAR_EDGE: f32 = 16.;
const CONTENT_EDGE: f32 = 20.;
const CONTROL: f32 = 28.;
const ICON: f32 = 16.;
const SURFACE_RADIUS: f32 = 12.;
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
pub const TRAFFIC_LIGHTS_Y: Pixels = px(TAB_TOP + (STRIP - TAB_TOP - LIGHT) / 2.);
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
pub const STATUS_CONTROL_HEIGHT: Pixels = px(STATUS_CONTROL);
/// How far the fill of a status bar toggle reaches past its icon on each side.
pub const STATUS_CONTROL_BLEED: Pixels = px((STATUS_CONTROL - ICON) / 2.);

pub const SIDEBAR_WIDTH: Pixels = px(244.);
/// The one row pitch: a sidebar row, a group label after the first, a changed file's row, the toolbar of
/// "Changes". Rows start where the header ends, so they share their lines across the window.
pub const ROW_HEIGHT: Pixels = px(ROW);
/// How far the fill of a row (selected, under the pointer) reaches past its content on each side.
pub const ROW_PADDING_X: Pixels = px(8.);
/// Between a row's glyph column and its label.
pub const ROW_GAP: Pixels = px(8.);
/// The one icon size: the kit's icons, and the column a row keeps for its glyph.
pub const ICON_SIZE: Pixels = px(ICON);
/// The size of the state marks and the chevrons: icons of the one set (Lucide), at the one icon size.
pub const MARK_SIZE: Pixels = px(ICON);
/// How far the disc of a filled mark stands inside the mark's box: the set draws its ring with a radius
/// of 10 and a stroke of 2 in a box of 24, so the ring's outer edge is one twenty-fourth of the box in.
pub const MARK_DISC_INSET: Pixels = px(ICON / 24.);
/// The box of the marks the conversation still draws by hand, until it is wired and takes the icon set.
pub const HAND_MARK_SIZE: Pixels = px(12.);
pub const GLYPH_STROKE: Pixels = px(1.5);
/// The one control height: the view tabs, every button of the header and of the window strip.
pub const CONTROL_HEIGHT: Pixels = px(CONTROL);
pub const CONTROL_PADDING_X: Pixels = px(10.);
/// How far the fill of an icon button reaches past its icon on each side: the icon sits on the grid line,
/// the fill bleeds past it.
pub const CONTROL_BLEED: Pixels = px((CONTROL - ICON) / 2.);

/// The panel's inset from the window's left edge while the sidebar is hidden. It has none at the right,
/// where it reaches the window's edge, and none at the bottom, where the status bar is.
pub const PANEL_INSET: Pixels = px(8.);
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
pub const GAP_TIGHT: Pixels = px(4.);
pub const GAP_SMALL: Pixels = px(6.);
pub const GAP: Pixels = px(8.);
pub const GAP_WIDE: Pixels = px(12.);
/// The room a count on a view tab keeps: two tabular figures, so a count going from 9 to 10 moves nothing.
pub const TAB_COUNT_WIDTH: Pixels = px(16.);
/// The room the header's right side keeps for the changed lines when the header is measured.
pub const HEADER_FIGURES_WIDTH: f32 = 84.;
/// The most room the branch takes in the header: a longer one is cut with an ellipsis and is whole under
/// the pointer.
pub const HEADER_BRANCH_MAX_WIDTH: f32 = 220.;
/// The gap between two views of the right pane's row.
pub const TAB_GAP: Pixels = px(2.);
/// The least room a tab of a narrow row keeps at each side of its label.
pub const TAB_PADDING_MIN: Pixels = px(4.);
/// A pane's row of views, the centre's and the right pane's: one row, so that with the toolbar under it the
/// rows of the right pane start two rows under the header, on the lines the sidebar's rows are on. Its
/// views are the control of a bar ([`STATUS_CONTROL_HEIGHT`]), as the toggles of the status bar are.
pub const PANE_VIEWS_HEIGHT: Pixels = px(ROW);

/// Two radii. One for surfaces (the panel, a card, a sheet): 12, the radius of the tab that fuses with
/// the panel. One for controls and everything inside a surface (buttons, fields, row fills, chips): 6,
/// Linear's, so a control one inset from a surface's edge sits concentric with it.
pub const RADIUS_SURFACE: Pixels = px(SURFACE_RADIUS);
pub const RADIUS_CONTROL: Pixels = px(CONTROL_RADIUS);
pub const RADIUS_CARD: Pixels = RADIUS_SURFACE;
pub const RADIUS_CHIP: Pixels = RADIUS_CONTROL;

/// Two text sizes, the Mac's own for a window of this kind: 13 for chrome and rows, 12 for what is
/// secondary (labels, figures, notes). Three weights only: regular, medium, semibold.
/// The medium weight. GPUI matches a weight of exactly 500 to the nearest lighter face when a family has
/// none at 500, and the system font's medium face reports a little more: requested 500 it would be drawn
/// regular. Every medium text of the interface asks for this weight.
pub const WEIGHT_MEDIUM: FontWeight = FontWeight(510.);
pub const TEXT_BODY: Pixels = px(BODY_TEXT);
pub const TEXT_SMALL: Pixels = px(13.);
pub const TEXT_LABEL: Pixels = px(12.);

/// The Agente view's split: the changes pane's share of the width at first launch, and its limits.
pub const CHANGES_SHARE_DEFAULT: f32 = 0.38;
pub const CHANGES_SHARE_MIN: f32 = 0.25;
pub const CHANGES_SHARE_MAX: f32 = 0.60;
/// The review list: a file's header row, a hunk header, one line of code, and a one-sentence note.
pub const CHANGE_HEADER_HEIGHT: f32 = ROW;
pub const HUNK_HEADER_HEIGHT: f32 = 28.;
pub const DIFF_LINE_HEIGHT: f32 = 20.;
pub const DIFF_NOTE_HEIGHT: f32 = ROW;
/// Code in diffs, in Geist Mono.
pub const DIFF_TEXT: Pixels = px(12.);
/// Added and removed lines are tinted at 10 percent of the success and danger tones (Zed's rule), with a
/// bar at the left edge.
pub const DIFF_TINT_ALPHA: f32 = 0.10;
pub const DIFF_BAR_WIDTH: Pixels = px(2.);
/// Space around line numbers and before the code.
pub const DIFF_GUTTER_PADDING: Pixels = px(8.);
pub const DIFF_CODE_PADDING: Pixels = px(12.);
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
/// The choices the commit menu holds before it scrolls: those that fit its height.
pub const PICKER_MENU_CHOICES: usize = 14;

/// The terminal: Ghostty's 13 point text, 8 px from the pane's top and bottom and the content inset from its
/// sides (so its text starts on the line the header starts on). It has no keyboard ring.
pub const TERMINAL_TEXT: f32 = 13.;
/// The sizes the terminal's text can be given in Settings.
pub const TERMINAL_TEXT_SIZES: [u8; 6] = [11, 12, 13, 14, 15, 16];
pub const TERMINAL_PADDING: f32 = 8.;

/// Log tails: line height and the most they take before scrolling.
pub const LOG_LINE_HEIGHT: f32 = 18.;
pub const LOG_MAX_HEIGHT: Pixels = px(320.);

/// The four durations of Sakura's motion. The shortest is also the tick the "Al lavoro" breath is redrawn on.
pub const DURATION_INSTANT: Duration = Duration::from_millis(50);
#[allow(dead_code)]
pub const DURATION_FAST: Duration = Duration::from_millis(120);
/// The layout duration: a pane that hides or comes back.
pub const DURATION_BASE: Duration = Duration::from_millis(200);
/// The exponent of the ease out of layout movements (a quintic: most of the way in the first third).
pub const EASE_OUT_POWER: i32 = 5;
#[allow(dead_code)]
pub const DURATION_SLOW: Duration = Duration::from_millis(320);
/// One breath of the "Al lavoro" glow.
pub const BREATH: Duration = Duration::from_millis(2400);
/// Lowest opacity of the glow during a breath.
pub const BREATH_LOW_OPACITY: f32 = 0.35;

// Sizes, shapes and timings of single views, kept here with every other token.
/// Width of the "Start agent" column.
pub const START_COLUMN_WIDTH: f32 = 400.;
/// "Take" while it is pressed.
pub const TAKE_PRESSED_OPACITY: f32 = 0.85;
/// The sheen runs top to bottom.
pub const SHEEN_ANGLE: f32 = 180.;
/// The menu's width.
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
/// The radius of the half disc inside the "Al lavoro" ring.
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
/// How long the press must last.
pub const HOLD: Duration = Duration::from_millis(1200);
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

    /// The list of sections at the window's left.
    pub const SECTIONS_WIDTH: Pixels = px(200.);
    /// A section is one column, no wider than this.
    pub const PAGE_MAX_WIDTH: Pixels = px(640.);
    /// The page's inset from the panel's sides and top: the content inset, twice.
    pub const PAGE_PADDING: Pixels = px(24.);
    /// Between two groups of rows, and between a section's title and its first group.
    pub const GROUP_GAP: Pixels = px(24.);
    /// Between a group's title and its rows.
    pub const GROUP_TITLE_GAP: Pixels = super::GAP;
    /// The section's title.
    pub const PAGE_TITLE: Pixels = px(18.);

    /// One row of a group: a name at the left, its control at the right. As tall as a check's row.
    pub const SETTING_ROW_HEIGHT: Pixels = super::VERIFY_ROW_HEIGHT;
    /// A row's inset inside its group.
    pub const SETTING_ROW_PADDING_X: Pixels = px(12.);
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

/// The sizes, shapes and timings of the agent conversation (`ui::thread`).
pub mod thread {
    use gpui::{px, rems, Pixels, Rems};
    use std::time::Duration;

    /// The conversation is one column, no wider than this, centred in a wider pane.
    pub const COLUMN_MAX_WIDTH: Pixels = px(760.);
    /// The column's inset from the pane's sides: the content inset, so text starts on the header's line.
    pub const COLUMN_PADDING_X: Pixels = super::CONTENT_PADDING;
    /// Space above and below the whole transcript.
    pub const TRANSCRIPT_PADDING_Y: Pixels = px(16.);
    /// Space between two rows of the same turn, and before a new prompt.
    pub const ROW_GAP: Pixels = px(10.);
    pub const TURN_GAP: Pixels = px(24.);

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
    pub const PROMPT_PADDING_X: Pixels = px(12.);
    pub const PROMPT_PADDING_Y: Pixels = px(8.);

    /// One-line rows: a thought's header, a tool call, a plan's header, a settled permission.
    pub const LINE_ROW_HEIGHT: Pixels = px(28.);
    /// Space between a row's glyph, its words and its state.
    pub const LINE_ROW_GAP: Pixels = px(8.);
    /// What sits under a one-line row starts under its words, past the glyph.
    pub const LINE_ROW_INDENT: Pixels = px(24.);
    /// Quiet prose under a row: a thought's text, a plan's steps.
    pub const QUIET_LINE_HEIGHT: Pixels = px(20.);
    /// The pitch of a plan's steps.
    pub const PLAN_ROW_HEIGHT: Pixels = px(26.);

    /// A block of monospaced text (a command's output, a file's lines): its padding and its text's leading.
    pub const BLOCK_PADDING_X: Pixels = px(12.);
    pub const BLOCK_PADDING_Y: Pixels = px(8.);
    /// Lines of output shown before the rest is folded behind its line count.
    pub const OUTPUT_FOLDED_LINES: usize = 10;
    /// Lines of a diff shown before the rest is folded behind its line count.
    pub const DIFF_FOLDED_LINES: usize = 40;

    /// The permission card.
    pub const CARD_PADDING: Pixels = px(12.);
    pub const CARD_GAP: Pixels = px(10.);
    /// Buttons in the conversation: the card's choices, "Riavvia", "Accedi".
    pub const BUTTON_HEIGHT: Pixels = super::CONTROL_HEIGHT;
    pub const BUTTON_PADDING_X: Pixels = px(12.);
    pub const BUTTON_GAP: Pixels = px(8.);
    /// A pressed button.
    pub const PRESSED_OPACITY: f32 = super::TAKE_PRESSED_OPACITY;

    /// The composer: a filled box with the radius of a surface.
    pub const COMPOSER_RADIUS: Pixels = super::RADIUS_SURFACE;
    pub const COMPOSER_PADDING: Pixels = px(8.);
    pub const COMPOSER_MIN_ROWS: usize = 1;
    pub const COMPOSER_MAX_ROWS: usize = 8;
    /// The round send and stop button, and the mark inside it.
    pub const SEND_BUTTON_SIZE: Pixels = px(28.);
    pub const SEND_BUTTON_DISABLED_OPACITY: f32 = 0.4;
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

    /// The strip's height.
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
    /// The hover fill of a tab on the canvas: inset from the strip's top and bottom, with the theme's control
    /// radius.
    pub const HOVER_INSET_Y: f32 = 4.;
    pub const HOVER_RADIUS: f32 = super::CONTROL_RADIUS;

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

/// The command palette.
pub mod palette {
    /// The panel: its width, how far it stands from the window's top, and the least room kept at its sides.
    pub const WIDTH: f32 = 560.;
    pub const TOP: f32 = 96.;
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

#[cfg(test)]
mod tests {
    use super::*;

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
        } = palette;
        let face = match action_face {
            ActionFace::Silver { top, bottom } => vec![top, bottom],
            ActionFace::Graphite(fill) => vec![fill],
        };
        let mut colours = vec![
            canvas,
            panel,
            raised,
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
