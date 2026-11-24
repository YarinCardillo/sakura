//! "About": the name, the version, the licence and where the source is.

use super::controls;
use super::group::{group, row};
use super::SettingsModal;
use crate::l10n::{tr, tr_with, Key};
use crate::theme::Palette;
use gpui::{AnyElement, App, Context};
use std::rc::Rc;

impl SettingsModal {
    /// The groups of "About".
    pub(super) fn about(&self, palette: &Palette, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let version = tr_with(
            Key("brand.about.version"),
            &[("version", crate::updates::CURRENT_VERSION)],
        );
        let license = tr_with(
            Key("brand.about.license"),
            &[("license", env!("CARGO_PKG_LICENSE"))],
        );
        let source = controls::button(
            "settings-about-source",
            tr(Key("settings_modal.about.open_source")),
            Rc::new(|_, cx: &mut App| cx.open_url(env!("CARGO_PKG_REPOSITORY"))),
            cx,
        );
        let rows = vec![
            row(
                "settings-row-about",
                tr(Key("brand.about.title")),
                Some(version),
                gpui::Empty,
                None,
                palette,
            ),
            row(
                "settings-row-license",
                tr(Key("settings_modal.about.license")),
                Some(license),
                gpui::Empty,
                None,
                palette,
            ),
            row(
                "settings-row-source",
                tr(Key("settings_modal.about.source")),
                None,
                source,
                None,
                palette,
            ),
        ];
        vec![group(rows, palette)]
    }
}
