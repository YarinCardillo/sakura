//! "Updates": how the app finds and installs new versions.

use super::controls;
use super::group::{group, row, row_acting};
use super::SettingsModal;
use crate::l10n::{tr, Key};
use crate::theme;
use crate::theme::Palette;
use crate::ui::kit::toggle_with_label;
use crate::updates;
use gpui::{AnyElement, App, Context, IntoElement as _, Styled as _};
use gpui_component::FocusableExt as _;
use std::rc::Rc;

impl SettingsModal {
    /// The groups of "Updates".
    pub(super) fn updates(&self, palette: &Palette, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let this = cx.entity().downgrade();
        let automatic_key = if cfg!(target_os = "macos") {
            Key("updates.automatic_downloads")
        } else {
            Key("updates.automatic_checks")
        };
        let (automatic, automatic_target) = toggle_with_label(
            "settings-updates-automatic",
            updates::is_automatic(cx),
            move |asked, _, cx| {
                updates::set_automatic(asked, cx);
                drop(this.update(cx, |_, cx| cx.notify()));
            },
        );
        let status = updates::status(cx);
        let control = update_control(
            ("settings-updates-check", "settings-updates-restart"),
            Key("updates.check_now"),
            cx,
        );
        let sentence = updates::sentence(&status.state);
        let rows = vec![
            row_acting(
                "settings-row-updates-automatic",
                tr(Key("updates.automatic")),
                Some(tr(automatic_key).into()),
                automatic,
                (None, automatic_target),
                palette,
            ),
            row(
                "settings-row-updates-check",
                tr(Key("updates.check_now")),
                Some(sentence.into()),
                control,
                None,
                palette,
            ),
        ];
        vec![group(rows, palette)]
    }
}

/// The words on the check button: "Checking…" while a check runs, its own words otherwise.
fn check_words(state: &updates::UpdateState, check_label: Key) -> Key {
    match state {
        updates::UpdateState::Checking => Key("updates.checking"),
        _ => check_label,
    }
}

/// The one control of an update: the check, worded `check_label` (named `ids.0`), asking for a check, and held while one runs
/// or none can be made; once an update is ready, "Restart to update" (named `ids.1`) in its place, or a
/// link to its page when only the public page found it.
pub(super) fn update_control(
    ids: (&'static str, &'static str),
    check_label: Key,
    cx: &mut Context<SettingsModal>,
) -> AnyElement {
    let Some(release) = updates::ready_release(cx) else {
        // Held while no check can be made: pressing does nothing and the button is dimmed but still readable
        // (the kit's own disabled look leaves light grey on a near white fill).
        let status = updates::status(cx);
        let can_check = status.can_check;
        let held = if can_check {
            1.
        } else {
            theme::HELD_CONTROL_OPACITY
        };
        // At rest the label has nothing to morph from: it only morphs when a check changes it.
        let state = match status.state {
            updates::UpdateState::Ready => None,
            _ => Some(format!("{:?}", std::mem::discriminant(&status.state))),
        };
        return controls::morphing_button(
            ids.0,
            tr(check_words(&status.state, check_label)),
            state.as_deref(),
            Rc::new(|_, cx: &mut App| {
                if updates::status(cx).can_check {
                    updates::check_now(cx);
                }
            }),
            cx,
        )
        .bg(crate::theme::Theme::palette(cx).fill_hover)
        .opacity(held)
        .focus_ring(false)
        .tab_stop(false)
        .into_any_element();
    };
    let label = match release.url.is_empty() {
        true => Key("updates.restart"),
        false => Key("updates.view_release"),
    };
    controls::button(
        ids.1,
        tr(label),
        Rc::new(|_, cx: &mut App| updates::install_update(cx)),
        cx,
    )
    .bg(crate::theme::Theme::palette(cx).fill_hover)
    .focus_ring(false)
    .tab_stop(false)
    .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::updates::{Release, UpdateState};

    #[test]
    fn the_button_says_checking_while_a_check_runs_and_its_own_words_otherwise() {
        let own = Key("updates.check_now");
        assert_eq!(
            check_words(&UpdateState::Checking, own),
            Key("updates.checking")
        );
        for state in [
            UpdateState::Ready,
            UpdateState::UpToDate,
            UpdateState::Failed,
            UpdateState::Available(Release {
                version: "9.9.9".into(),
                url: String::new(),
            }),
        ] {
            assert_eq!(check_words(&state, own), own, "{state:?}");
        }
    }
}
