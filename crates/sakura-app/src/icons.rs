//! The window's icons: one set, Lucide, as the kit bundles it. The kit's own components bring the icons
//! they use; the state marks need five more, added here by name so only those are embedded. Everything
//! that is an icon in the window comes from this source, at the one icon size of the theme.
//!
//! Two icons are our own, because Lucide draws strokes only: the toggle of a pane that is open is
//! Lucide's panel icon with the pane's side filled (`assets/icons/panel-*-filled.svg`, the same box,
//! radius and stroke), so the toggle shows its state without a second mark.

use gpui::{AssetSource, Result, SharedString};
use gpui_kit_assets::{IconName, IconNamed};
use std::borrow::Cow;

/// Where our own two icons are served from, beside the set's.
const PANEL_LEFT_FILLED: &str = "icons/panel-left-filled.svg";
const PANEL_RIGHT_FILLED: &str = "icons/panel-right-filled.svg";
const OWN_ICONS: [(&str, &[u8]); 2] = [
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

gpui_kit_assets::icon_assets!(
    MarkIcons,
    [
        Circle,
        CircleDashed,
        CircleDot,
        Contrast,
        CheckCheck,
        SquareTerminal,
        LocateFixed,
        LocateOff
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
/// Two checks: done and taken.
pub const TAKEN: IconName = IconName::CheckCheck;
/// The chevron of a row that opens, closed and open.
pub const CHEVRON_CLOSED: IconName = IconName::ChevronRight;
pub const CHEVRON_OPEN: IconName = IconName::ChevronDown;
/// The trigger of an overflow menu.
pub const OVERFLOW: IconName = IconName::Ellipsis;
/// A lavoro's own terminal, in the status bar.
pub const TERMINAL: IconName = IconName::SquareTerminal;
/// The right pane follows the agent, or was taken by the person.
pub const FOLLOWING: IconName = IconName::LocateFixed;
pub const NOT_FOLLOWING: IconName = IconName::LocateOff;

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
            TAKEN,
            CHEVRON_CLOSED,
            CHEVRON_OPEN,
            OVERFLOW,
            TERMINAL,
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
        let shapes = [
            RING, DASHED, WORKING, NEEDS_YOU, PASSED, FAILED, PROBLEM, TAKEN,
        ];
        for (index, shape) in shapes.iter().enumerate() {
            assert!(!shapes[..index].contains(shape), "{:?}", shape.path());
        }
    }
}
