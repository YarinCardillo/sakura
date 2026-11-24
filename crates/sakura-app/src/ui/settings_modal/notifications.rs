//! "Notifications": what the system is told of while Sakura is in the background.

use super::group::{group, row_acting, with_default};
use super::SettingsModal;
use crate::l10n::{tr, Key};
use crate::prefs::Preferences;
use crate::theme::Palette;
use crate::ui::kit::toggle_with_label;
use gpui::{AnyElement, App, Context, Window};
use std::rc::Rc;

impl SettingsModal {
    /// The groups of "Notifications".
    pub(super) fn notifications(
        &self,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let preferences = Preferences::current(cx);
        let this = cx.entity().downgrade();
        let (needs_reset, finished_reset) = (this.clone(), this.clone());
        let needs = {
            let this = this.clone();
            toggle_with_label(
                "settings-notify-needs",
                !preferences.is_needs_person_muted,
                move |asked, _, cx| {
                    Preferences::change(cx, |prefs| prefs.is_needs_person_muted = !asked);
                    drop(this.update(cx, |_, cx| cx.notify()));
                },
            )
        };
        let finished = toggle_with_label(
            "settings-notify-finished",
            !preferences.is_finished_muted,
            move |asked, _, cx| {
                Preferences::change(cx, |prefs| prefs.is_finished_muted = !asked);
                drop(this.update(cx, |_, cx| cx.notify()));
            },
        );
        let needs_default = Rc::new(move |_: &mut Window, cx: &mut App| {
            Preferences::change(cx, |prefs| prefs.is_needs_person_muted = false);
            drop(needs_reset.update(cx, |_, cx| cx.notify()));
        });
        let finished_default = Rc::new(move |_: &mut Window, cx: &mut App| {
            Preferences::change(cx, |prefs| prefs.is_finished_muted = false);
            drop(finished_reset.update(cx, |_, cx| cx.notify()));
        });
        let (needs, needs_target) = needs;
        let (finished, finished_target) = finished;
        let needs = with_default(
            "settings-notify-needs",
            preferences.is_needs_person_muted,
            needs_default,
            needs,
            palette,
        );
        let finished = with_default(
            "settings-notify-finished",
            preferences.is_finished_muted,
            finished_default,
            finished,
            palette,
        );
        let rows = vec![
            row_acting(
                "settings-row-notify-needs",
                tr(Key("settings.notify.needs_you")),
                Some(tr(Key("settings.notify.needs_you.description")).into()),
                needs,
                (None, needs_target),
                palette,
            ),
            row_acting(
                "settings-row-notify-finished",
                tr(Key("settings.notify.finished")),
                Some(tr(Key("settings.notify.finished.description")).into()),
                finished,
                (None, finished_target),
                palette,
            ),
        ];
        vec![group(rows, palette)]
    }
}
