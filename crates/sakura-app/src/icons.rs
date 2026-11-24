//! The window's icons: one set, Lucide, as the kit bundles it. The kit's own components bring the icons
//! they use; the state marks need five more, added here by name so only those are embedded. Everything
//! that is an icon in the window comes from this source, at the one icon size of the theme.
//!
//! Two icons are our own, because Lucide draws strokes only: the toggle of a pane that is open is
//! Lucide's panel icon with the pane's side filled (`assets/icons/panel-*-filled.svg`, the same box,
//! radius and stroke), so the toggle shows its state without a second mark.
//!
//! The platforms' own marks of the connectors (GitHub, Notion, Linear) are vendored from Simple Icons 13.21.0
//! (https://github.com/simple-icons/simple-icons, CC0 1.0): `assets/icons/simple-*.svg`, unchanged. A feed
//! has Lucide's RSS glyph and a program Lucide's terminal.

use gpui::{AssetSource, Result, SharedString};
use gpui_kit_assets::{IconName, IconNamed};
use std::borrow::Cow;

/// Where our own two icons are served from, beside the set's.
const PANEL_LEFT_FILLED: &str = "icons/panel-left-filled.svg";
const PANEL_RIGHT_FILLED: &str = "icons/panel-right-filled.svg";
const SIMPLE_GITHUB: &str = "icons/simple-github.svg";
const SIMPLE_NOTION: &str = "icons/simple-notion.svg";
const SIMPLE_LINEAR: &str = "icons/simple-linear.svg";
const OWN_ICONS: [(&str, &[u8]); 5] = [
    (
        SIMPLE_GITHUB,
        include_bytes!("../assets/icons/simple-github.svg"),
    ),
    (
        SIMPLE_NOTION,
        include_bytes!("../assets/icons/simple-notion.svg"),
    ),
    (
        SIMPLE_LINEAR,
        include_bytes!("../assets/icons/simple-linear.svg"),
    ),
    (
        PANEL_LEFT_FILLED,
        include_bytes!("../assets/icons/panel-left-filled.svg"),
    ),
    (
        PANEL_RIGHT_FILLED,
        include_bytes!("../assets/icons/panel-right-filled.svg"),
    ),
];

/// The icon of a pane's toggle: the outline while the pane is closed, filled on the pane's side while it
/// is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneIcon {
    Left { is_open: bool },
    Right { is_open: bool },
}

impl IconNamed for PaneIcon {
    fn path(self) -> SharedString {
        match self {
            PaneIcon::Left { is_open: true } => PANEL_LEFT_FILLED.into(),
            PaneIcon::Left { is_open: false } => IconName::PanelLeft.path(),
            PaneIcon::Right { is_open: true } => PANEL_RIGHT_FILLED.into(),
            PaneIcon::Right { is_open: false } => IconName::PanelRight.path(),
        }
    }
}

/// The mark of a connector: its platform's own, the RSS glyph for a feed, a terminal for a program. Only the
/// settings of the engine build draw it.
#[cfg(any(test, not(feature = "boxes")))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectorIcon {
    GitHub,
    Notion,
    Linear,
    Feed,
    Program,
}

#[cfg(any(test, not(feature = "boxes")))]
impl ConnectorIcon {
    /// The mark of the connector kind `kind` as the engine names it (`github`, `notion`, `linear`, `feed`,
    /// `program`); none for a kind that is not known.
    pub fn of_kind(kind: &str) -> Option<Self> {
        match kind {
            "github" => Some(Self::GitHub),
            "notion" => Some(Self::Notion),
            "linear" => Some(Self::Linear),
            "feed" => Some(Self::Feed),
            "program" => Some(Self::Program),
            _ => None,
        }
    }

    /// The mark of a source the digest reads: a `connector` source has its connector's mark, a feed or a
    /// program its own.
    pub fn of_source(kind: &str, connector: Option<&str>) -> Option<Self> {
        match (kind, connector) {
            ("connector", Some(connector)) => Self::of_kind(connector),
            ("connector", None) => None,
            (kind, _) => Self::of_kind(kind),
        }
    }
}

#[cfg(any(test, not(feature = "boxes")))]
impl IconNamed for ConnectorIcon {
    fn path(self) -> SharedString {
        match self {
            Self::GitHub => SIMPLE_GITHUB.into(),
            Self::Notion => SIMPLE_NOTION.into(),
            Self::Linear => SIMPLE_LINEAR.into(),
            Self::Feed => IconName::Rss.path(),
            Self::Program => IconName::Terminal.path(),
        }
    }
}

gpui_kit_assets::icon_assets!(
    MarkIcons,
    [
        Circle,
        CircleDashed,
        CircleDot,
        Contrast,
        FolderPlus,
        MessageCircle,
        SquareTerminal,
        LocateFixed,
        LocateOff,
        Layers,
        Gauge,
        CircleArrowDown,
        ListFilter,
        Rss,
        Terminal
    ]
);

/// The kit's icons and the state marks'.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some((_, bytes)) = OWN_ICONS.iter().find(|(own, _)| *own == path) {
            return Ok(Some(Cow::Borrowed(bytes)));
        }
        match MarkIcons.load(path)? {
            Some(bytes) => Ok(Some(bytes)),
            None => gpui_kit_assets::Assets.load(path),
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = gpui_kit_assets::Assets.list(path)?;
        paths.extend(MarkIcons.list(path)?);
        let own = OWN_ICONS.iter().map(|(own, _)| *own);
        paths.extend(own.filter(|own| own.starts_with(path)).map(Into::into));
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}

/// A ring: nothing current.
pub const RING: IconName = IconName::Circle;
/// A dashed ring: not started yet, or waiting its turn.
pub const DASHED: IconName = IconName::CircleDashed;
/// A ring with its inner half: work in progress. The half is the part that breathes.
pub const WORKING: IconName = IconName::Contrast;
/// A ring with a dot: needs the person.
pub const NEEDS_YOU: IconName = IconName::CircleDot;
/// A ring with a check: passed.
pub const PASSED: IconName = IconName::CircleCheck;
/// A ring with a cross: failed.
pub const FAILED: IconName = IconName::CircleX;
/// A ring with an exclamation mark: something is wrong with the thing itself.
pub const PROBLEM: IconName = IconName::CircleAlert;
/// The chevron of a row that opens, closed and open.
pub const CHEVRON_CLOSED: IconName = IconName::ChevronRight;
pub const CHEVRON_OPEN: IconName = IconName::ChevronDown;
/// The trigger of an overflow menu.
pub const OVERFLOW: IconName = IconName::Ellipsis;
/// A task's own terminal, in the status bar.
#[cfg(any(test, feature = "boxes"))]
pub const TERMINAL: IconName = IconName::SquareTerminal;
/// The right pane follows the agent, or was taken by the person.
#[cfg(feature = "boxes")]
pub const FOLLOWING: IconName = IconName::LocateFixed;
#[cfg(feature = "boxes")]
pub const NOT_FOLLOWING: IconName = IconName::LocateOff;
/// Settings, in the status bar.
pub const SETTINGS: IconName = IconName::Settings;
/// The search field of Settings.
#[cfg(any(test, feature = "boxes"))]
pub const SEARCH: IconName = IconName::Search;
/// Back to the list a page was opened from.
pub const BACK: IconName = IconName::ChevronLeft;
/// The tab of the Waiting page: what waits for the person, an inbox.
pub const WAITING_PAGE: IconName = IconName::Inbox;
/// The tab of the Master's page: a conversation.
pub const MASTER_PAGE: IconName = IconName::MessageCircle;
/// The tab of the Lanes page: the stack of lanes.
#[cfg(not(feature = "boxes"))]
pub const LANES_PAGE: IconName = IconName::Layers;
/// The tab of the Engine page: its gauge.
#[cfg(not(feature = "boxes"))]
pub const ENGINE_PAGE: IconName = IconName::Gauge;
/// A project's page: a plain folder.
#[cfg(not(feature = "boxes"))]
pub const PROJECT_PAGE: IconName = IconName::Folder;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_mark_and_every_kit_icon_is_in_the_bundle() {
        let marks = [
            RING,
            DASHED,
            WORKING,
            NEEDS_YOU,
            PASSED,
            FAILED,
            PROBLEM,
            CHEVRON_CLOSED,
            CHEVRON_OPEN,
            OVERFLOW,
            TERMINAL,
            SETTINGS,
            SEARCH,
            BACK,
            WAITING_PAGE,
            MASTER_PAGE,
            #[cfg(not(feature = "boxes"))]
            LANES_PAGE,
            #[cfg(not(feature = "boxes"))]
            ENGINE_PAGE,
        ];
        for mark in marks {
            let path = mark.path();
            let bytes = Assets.load(&path).expect("the source answers");
            assert!(bytes.is_some_and(|svg| !svg.is_empty()), "{path}");
        }
        let listed = Assets.list("icons/").expect("the list");
        assert!(listed.len() > 100, "the kit's icons and the marks'");
    }

    #[test]
    fn every_connector_has_a_mark_in_the_bundle() {
        for kind in ["github", "notion", "linear", "feed", "program"] {
            let icon = ConnectorIcon::of_kind(kind).expect(kind);
            let path = icon.path();
            let bytes = Assets.load(&path).expect("the source answers");
            assert!(bytes.is_some_and(|svg| !svg.is_empty()), "{path}");
        }
    }

    #[test]
    fn a_connector_kind_that_is_not_known_has_no_mark() {
        assert_eq!(ConnectorIcon::of_kind("jira"), None);
        assert_eq!(ConnectorIcon::of_source("connector", None), None);
    }

    #[test]
    fn a_digest_source_has_its_connector_s_mark_or_its_own_kind_s() {
        assert_eq!(
            ConnectorIcon::of_source("connector", Some("notion")),
            Some(ConnectorIcon::Notion)
        );
        assert_eq!(
            ConnectorIcon::of_source("feed", None),
            Some(ConnectorIcon::Feed)
        );
        assert_eq!(
            ConnectorIcon::of_source("program", None),
            Some(ConnectorIcon::Program)
        );
    }

    #[test]
    fn a_pane_s_toggle_is_the_outline_when_closed_and_our_filled_icon_when_open() {
        let icons = [
            (PaneIcon::Left { is_open: false }, "icons/panel-left.svg"),
            (PaneIcon::Left { is_open: true }, PANEL_LEFT_FILLED),
            (PaneIcon::Right { is_open: false }, "icons/panel-right.svg"),
            (PaneIcon::Right { is_open: true }, PANEL_RIGHT_FILLED),
        ];
        for (icon, path) in icons {
            assert_eq!(icon.path().as_ref(), path);
            let bytes = Assets.load(path).expect("the source answers");
            let svg = bytes.expect("the icon is in the bundle");
            let text = std::str::from_utf8(&svg).expect("an svg");
            let is_filled = text.contains("fill=\"currentColor\"");
            let is_open = matches!(
                icon,
                PaneIcon::Left { is_open: true } | PaneIcon::Right { is_open: true }
            );
            assert_eq!(is_filled, is_open, "{path}");
            assert!(text.contains("rx=\"2\""), "the set's own box: {path}");
        }
    }

    #[test]
    fn every_state_has_a_shape_of_its_own() {
        let shapes = [RING, DASHED, WORKING, NEEDS_YOU, PASSED, FAILED, PROBLEM];
        for (index, shape) in shapes.iter().enumerate() {
            assert!(!shapes[..index].contains(shape), "{:?}", shape.path());
        }
    }
}
