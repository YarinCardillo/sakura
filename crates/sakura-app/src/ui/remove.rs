//! The confirmation of "Archive" when work would be lost: in the header's action area, "Cancel",
//! then a hold ("Delete anyway") that removes with `box rm --force`; the reason is under the header. A plain press never
//! removes: only a completed hold does.

use super::chrome::{control, keep_click};
use super::desk::Desk;
use super::hold::HoldButton;
use super::verify::secondary_variant;
use crate::l10n::{tr, Key};
use gpui::InteractiveElement as _;
use gpui::{div, AnyElement, Context, Entity, IntoElement, ParentElement};
use gpui_component::button::{Button, ButtonVariants as _};

const CANCEL: Key = Key("common.cancel");

/// The label of the remove hold: its pending word while `box` removes.
pub fn hold_label(is_pending: bool) -> &'static str {
    if is_pending {
        tr(Key("remove.removing"))
    } else {
        tr(Key("remove.anyway"))
    }
}

/// The confirmation for `task`: "Cancel", then the hold. The reason work would be lost is given under the
/// header, not in it.
pub fn confirm(task: &str, hold: &Entity<HoldButton>, cx: &Context<Desk>) -> Vec<AnyElement> {
    let name = task.to_string();
    let cancel = Button::new("remove-cancel")
        .debug_selector(|| "remove-cancel".into())
        .custom(secondary_variant(cx));
    let cancel = control(cancel, tr(CANCEL), &[tr(CANCEL)])
        .on_click(cx.listener(move |desk, _, _, cx| desk.cancel_remove(&name, cx)));
    vec![
        keep_click(div().id("remove-cancel-area").child(cancel)).into_any_element(),
        hold.clone().into_any_element(),
    ]
}
