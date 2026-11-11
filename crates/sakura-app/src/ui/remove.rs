//! The confirmation of "Archive" when work would be lost: in the header's action area, "Cancel",
//! then a hold ("Archive anyway") that removes with `box rm --force`, with the reason under the pointer on
//! it, as the hold of "Take" has its own. A plain press never removes: only a completed hold does.

use super::chrome::{control, keep_click};
use super::desk::Desk;
use super::hold::HoldButton;
use super::verify::secondary_variant;
use crate::l10n::{tr, Key};
use gpui::{div, AnyElement, Context, Entity, IntoElement, ParentElement, SharedString};
use gpui::{InteractiveElement as _, StatefulInteractiveElement as _};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::tooltip::Tooltip;

const CANCEL: Key = Key("common.cancel");

/// The label of the remove hold: its pending word while `box` removes.
pub fn hold_label(is_pending: bool) -> &'static str {
    if is_pending {
        tr(Key("remove.removing"))
    } else {
        tr(Key("remove.anyway"))
    }
}

/// The confirmation for `task`: "Cancel", then the hold, with `reason` (why work would be lost) under the
/// pointer on it. Nothing else is added to the page: asking moves nothing.
pub fn confirm(
    task: &str,
    reason: Option<SharedString>,
    hold: &Entity<HoldButton>,
    cx: &Context<Desk>,
) -> Vec<AnyElement> {
    let name = task.to_string();
    let cancel = Button::new("remove-cancel")
        .debug_selector(|| "remove-cancel".into())
        .custom(secondary_variant(cx));
    let cancel = control(cancel, tr(CANCEL), &[tr(CANCEL)])
        .on_click(cx.listener(move |desk, _, _, cx| desk.cancel_remove(&name, cx)));
    vec![
        keep_click(div().id("remove-cancel-area").child(cancel)).into_any_element(),
        held(hold, reason),
    ]
}

/// The hold, with the reason it is a hold under the pointer.
fn held(hold: &Entity<HoldButton>, reason: Option<SharedString>) -> AnyElement {
    let held = div()
        .id("remove-hold")
        .debug_selector(|| "remove-hold".into())
        .child(hold.clone());
    match reason {
        Some(reason) => held
            .tooltip(move |window, cx| Tooltip::new(reason.clone()).build(window, cx))
            .into_any_element(),
        None => held.into_any_element(),
    }
}
