//! A tab under the pointer: a press that stays put chooses the tab on release; one that moves past the
//! threshold picks the tab up, and it follows the pointer along the strip while the tabs it passes step
//! aside. The release commits the new place; Esc, a lost window or a change to the tabs put the tab back.
//! A dragged tab keeps its own look, and the pointer stays the arrow throughout.

use super::drag::{drop_index, resist_drag, slide_offset, StripBounds, TabBox};
use super::metrics::{DRAG_THRESHOLD, TAB_GAP, TRAILING_ROOM};
use super::motion::Sprung;
use super::strip::{Strip, StripEvent};
use gpui::Context;
use sakura_core::tabs::TabId;

/// A press on a tab that has not moved yet.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Press {
    pub id: TabId,
    pub from: usize,
    /// Where the pointer was, in window px.
    pub start_x: f32,
    /// The tabs' boxes and the span they may move in, as they stood at the press.
    pub boxes: Vec<TabBox>,
    pub bounds: StripBounds,
}

/// A tab in the hand.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Drag {
    pub id: TabId,
    pub from: usize,
    /// The place it would land on now.
    pub to: usize,
    /// How far it is drawn from its slot.
    pub dx: f32,
    /// How far a passed tab steps aside: this tab's width and the gap.
    pub step: f32,
}

impl Strip {
    /// The primary button went down on the tab `id` with the pointer at `x` (window px); `width` is the
    /// strip's own.
    pub(super) fn press_tab(&mut self, id: TabId, x: f32, width: f32, cx: &mut Context<Self>) {
        self.end_drag();
        let Some(from) = self.tabs.index_of(id) else {
            return;
        };
        let boxes: Vec<TabBox> = self.slots.iter().map(|(_, slot)| *slot).collect();
        let Some(first) = boxes.first() else {
            return;
        };
        let bounds = StripBounds {
            left: first.left.max(self.leading_inset),
            right: width - TRAILING_ROOM,
        };
        self.press = Some(Press {
            id,
            from,
            start_x: x,
            boxes,
            bounds,
        });
        self.tip_owner = None;
        self.tips.update(cx, |tips, cx| tips.hide(cx));
    }

    /// The pointer is at `x` (window px) with the button still down.
    pub(super) fn move_pointer(&mut self, x: f32, cx: &mut Context<Self>) {
        let Some(press) = self.press.clone() else {
            return;
        };
        let dx = x - press.start_x;
        if self.drag.is_none() && dx.abs() < DRAG_THRESHOLD {
            return;
        }
        let landed = self.drag.map_or(press.from, |drag| drag.to);
        // The tab stops flush at the strip's ends: where it lands is told from where it is drawn, so a
        // pointer that goes on past the end crosses no slot the tab itself does not reach.
        let dx = resist_drag(&press.boxes, press.from, dx, press.bounds);
        let drag = Drag {
            id: press.id,
            from: press.from,
            to: drop_index(&press.boxes, press.from, dx, landed),
            dx,
            step: press.boxes[press.from].width + TAB_GAP,
        };
        if drag.to != landed {
            // The tab crossed into another slot: one light tap, as when something snaps into place.
            self.haptics.alignment();
        }
        self.stand_aside(&drag);
        self.drag = Some(drag);
        cx.notify();
    }

    /// The button came up: a tab in the hand lands on its place, a press that never moved chooses its tab.
    pub(super) fn release(&mut self, cx: &mut Context<Self>) {
        let Some(press) = self.press.clone() else {
            return;
        };
        let Some(drag) = self.drag else {
            self.press = None;
            return self.activate(press.id, cx);
        };
        let stood = self.slot_lefts();
        self.end_drag();
        if self.tabs.move_to(drag.id, drag.to) {
            self.glide_from = Some(stood);
            cx.emit(StripEvent::Changed);
        }
        cx.notify();
    }

    /// Esc, or the window lost the pointer: the tab goes back to where it was picked up.
    pub(super) fn cancel_drag(&mut self, cx: &mut Context<Self>) {
        if self.press.is_some() {
            self.end_drag();
            cx.notify();
        }
    }

    /// The tabs `drag` passes head one step aside; the others head home.
    fn stand_aside(&mut self, drag: &Drag) {
        for (index, tab) in self.tabs.tabs().iter().enumerate() {
            if tab.id == drag.id {
                continue;
            }
            let aside = slide_offset(index, drag.from, drag.to, drag.step);
            self.offsets
                .entry(tab.id)
                .or_insert_with(|| Sprung::at_rest(0.))
                .send_to(aside);
        }
    }
}
