//! The strip's tooltips: a tab's full title and its shortcut, below the tab, after the delay for the first
//! and at once while the pointer moves on to the next tab (one delay group, the kit's overlay with the
//! strip as its only user). Never while a tab is pressed or dragged.

use super::metrics::TOOLTIP_DELAY;
use super::strip::Strip;
use gpui::{
    div, App, Bounds, Context, InteractiveElement as _, IntoElement as _, Keystroke,
    ParentElement as _, Pixels, SharedString, Window,
};
use gpui_base::{Placement, TooltipOverlay, TooltipRequest};
use gpui_component::kbd::Kbd;
use gpui_component::tooltip::Tooltip;
use sakura_core::tabs::TabId;

/// The overlay: a tooltip appears and changes place without animation.
pub(super) fn overlay() -> TooltipOverlay {
    TooltipOverlay::new().render_with(|view, _, _, _| {
        div()
            .debug_selector(|| "tab-tip".into())
            .child(view)
            .into_any_element()
    })
}

/// The keystroke that selects the tab with `digit`, as the tooltip shows it.
pub(super) fn shortcut(digit: usize) -> Option<Keystroke> {
    Keystroke::parse(match digit {
        1 => "cmd-1",
        2 => "cmd-2",
        3 => "cmd-3",
        4 => "cmd-4",
        5 => "cmd-5",
        6 => "cmd-6",
        7 => "cmd-7",
        8 => "cmd-8",
        9 => "cmd-9",
        _ => return None,
    })
    .ok()
}

impl Strip {
    /// The pointer came onto or left the tab `id`, drawn at `bounds`. The tooltip belongs to the tab the
    /// pointer came onto last: a leave from any other tab is not its business, in whatever order the two
    /// tabs hear of the pointer.
    pub(super) fn hover_tab(
        &mut self,
        (id, is_over): (TabId, bool),
        tip: (SharedString, Option<usize>),
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let is_busy = self.press.is_some() || self.drag.is_some();
        if !is_over && self.tip_owner.is_some_and(|owner| owner != id) {
            return;
        }
        self.tip_owner = (is_over && !is_busy).then_some(id);
        self.tips.update(cx, |tips, cx| {
            if !is_over || is_busy {
                return tips.request_hide(window, cx);
            }
            let (title, digit) = tip;
            let request = TooltipRequest::new(bounds, move |window, cx| {
                build(title.clone(), digit, window, cx)
            });
            let request = request
                .with_placement(Placement::Bottom)
                .with_show_delay(TOOLTIP_DELAY);
            tips.request_show(request, window, cx);
        });
    }
}

fn build(
    title: SharedString,
    digit: Option<usize>,
    window: &mut Window,
    cx: &mut App,
) -> gpui::AnyView {
    let key = digit.and_then(shortcut).map(Kbd::new);
    Tooltip::new(title).key_binding(key).build(window, cx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tab_digit_has_its_command_key() {
        for digit in 1..=9 {
            let Some(stroke) = shortcut(digit) else {
                panic!("no shortcut for {digit}");
            };
            assert!(stroke.modifiers.platform);
            assert_eq!(stroke.key, digit.to_string());
        }
        assert!(shortcut(0).is_none());
        assert!(shortcut(10).is_none());
    }
}
