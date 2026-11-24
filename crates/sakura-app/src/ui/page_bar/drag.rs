//! A tab under the pointer, as the strip before the redesign had it: a press that stays put chooses the
//! tab on release; one that moves past the threshold picks the tab up, and it follows the pointer along the
//! bar while the tabs it passes give way on a spring. The release lands it; Esc, a lost window or a change
//! to the tabs put it back. Tabs opened, closed or moved glide from where they stood to their new places.
//! The trackpad taps once each time the tab in the hand crosses into another slot, never on a click.

use super::{PageBar, SplitSide};
use crate::theme::{self, page_bar, tabs::DRAG_THRESHOLD};
use crate::ui::tab_motion::drag::{drop_index, resist_drag, slide_offset, StripBounds, TabBox};
use crate::ui::tab_motion::motion::{Sprung, PRESS_DOWN, PRESS_UP};
use gpui::{Bounds, Context, Pixels, Point};
use sakura_core::tabs::TabId;
use std::collections::HashMap;
use std::time::Instant;

/// A press on a tab that has not moved yet.
#[derive(Debug, Clone)]
struct Press {
    id: TabId,
    from: usize,
    /// Where the pointer was, in window px.
    start: Point<Pixels>,
    active: TabId,
    split_target: Option<TabId>,
    /// The tabs' boxes and the span they may move in, as they stood at the press.
    boxes: Vec<TabBox>,
    bounds: StripBounds,
}

/// A tab in the hand.
#[derive(Debug, Clone, Copy)]
struct Drag {
    id: TabId,
    from: usize,
    /// The place it would land on now.
    to: usize,
    /// How far it is drawn from its slot.
    dx: f32,
    /// How far a passed tab steps aside: this tab's width and the gap.
    step: f32,
}

/// How far a tab is drawn from its slot, on its spring, and when the spring was last advanced.
#[derive(Debug, Clone, Copy)]
struct Offset {
    spring: Sprung,
    at: Instant,
}

/// Where the tabs rest and how far each is drawn from its place.
#[derive(Default)]
pub(super) struct Motion {
    /// Where each tab rests, in window px, as last laid out (before any offset).
    slots: HashMap<TabId, TabBox>,
    /// The tabs' order as last told, to know when it changed.
    order: Vec<TabId>,
    offsets: HashMap<TabId, Offset>,
    /// Where the tabs rested before a change: each glides from there when it is next laid out.
    stood: Option<HashMap<TabId, f32>>,
    press: Option<Press>,
    drag: Option<Drag>,
    /// Where the tab before the one being placed is drawn to, this frame: its right edge.
    previous_right: Option<f32>,
    /// The width the row keeps while tabs glide to their places after a close: the tabs are drawn from where
    /// they stood, so a row that shrank at once would cut the last of them off and hand its room to the +.
    floor: Option<RowFloor>,
    /// How far right of the row's left edge the tabs were drawn, in the last frame that placed them all.
    drawn_extent: f32,
    /// Where the first tab rests and how far right the tabs placed so far this frame are drawn.
    frame: Option<(f32, f32)>,
}

/// The width the row keeps, and whether no frame has been drawn with it yet.
#[derive(Debug, Clone, Copy)]
struct RowFloor {
    width: f32,
    is_fresh: bool,
}

impl Motion {
    /// A bar over tabs in this `order`.
    pub(super) fn new(order: Vec<TabId>) -> Self {
        Self {
            order,
            ..Self::default()
        }
    }
}

impl PageBar {
    /// Whether a tab is pressed or in the hand: the primary button is down on the bar.
    pub(super) fn is_pressing(&self) -> bool {
        self.motion.press.is_some()
    }

    /// Whether a tab is in the hand.
    #[cfg(test)]
    pub(super) fn is_dragging(&self) -> bool {
        self.motion.drag.is_some()
    }

    /// The tab in the hand, which is drawn over the others it passes.
    pub(super) fn held(&self) -> Option<TabId> {
        self.motion.drag.map(|drag| drag.id)
    }

    /// Sets the window space that can receive edge drops, in window coordinates.
    pub fn set_split_drop_bounds(&mut self, bounds: Bounds<Pixels>) {
        self.split_drop_bounds = Some(bounds);
    }

    /// Returns the edge that is previewing a split.
    pub fn split_drop_side(&self) -> Option<SplitSide> {
        self.split_drop_side
    }

    fn split_drop_for(&self, press: &Press, position: Point<Pixels>) -> Option<(SplitSide, TabId)> {
        let target = press.split_target?;
        if self.tabs.split().is_some()
            || self.tabs.active().id() != press.active
            || self.tabs.tab(target).is_none()
            || self.tabs.tab(press.id).is_none()
        {
            return None;
        }
        let bounds = self.split_drop_bounds?;
        if !bounds.contains(&position) {
            return None;
        }
        if position.x < bounds.left() + page_bar::SPLIT_EDGE_WIDTH {
            Some((SplitSide::Left, target))
        } else if position.x > bounds.right() - page_bar::SPLIT_EDGE_WIDTH {
            Some((SplitSide::Right, target))
        } else {
            None
        }
    }

    fn set_preview(&mut self, preview: Option<SplitSide>, cx: &mut Context<Self>) {
        if self.split_drop_side != preview {
            self.split_drop_side = preview;
            cx.emit(super::PageBarEvent::SplitDropPreviewChanged);
        }
    }

    /// Records the tab and the split target before either can change.
    pub(super) fn press_tab(&mut self, id: TabId, start: Point<Pixels>, cx: &mut Context<Self>) {
        if self.motion.press.is_some() {
            self.cancel_drag(cx);
        }
        if crate::ui::dialog_guard::is_open(cx) {
            return;
        }
        let Some(from) = self.tabs.index_of(id) else {
            return;
        };
        let boxes: Option<Vec<TabBox>> = self
            .tabs
            .tabs()
            .iter()
            .map(|tab| self.motion.slots.get(&tab.id()).copied())
            .collect();
        let Some(boxes) = boxes else {
            return;
        };
        let bounds = StripBounds {
            left: boxes[0].left,
            right: boxes[boxes.len() - 1].right(),
        };
        let active = self.tabs.active().id();
        let split_target = if id == active {
            let tabs = self.tabs.tabs();
            from.checked_sub(1)
                .and_then(|previous| tabs.get(previous))
                .or_else(|| tabs.get(from + 1))
                .map(|tab| tab.id())
        } else {
            Some(active)
        };
        self.motion.press = Some(Press {
            id,
            from,
            start,
            active,
            split_target,
            boxes,
            bounds,
        });
    }

    /// Tracks strip reordering or edge preview after the pointer clears the drag threshold.
    pub(super) fn move_pointer(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        if crate::ui::dialog_guard::is_open(cx) {
            self.cancel_drag(cx);
            return;
        }
        let Some(press) = self.motion.press.clone() else {
            return;
        };
        let delta = position - press.start;
        let dx = f32::from(delta.x);
        let was_dragging = self.motion.drag.is_some();
        if !was_dragging && dx.abs().max(f32::from(delta.y).abs()) < DRAG_THRESHOLD {
            return;
        }
        if self
            .tabs
            .split()
            .is_some_and(|pair| pair.other(press.id).is_some())
        {
            self.cancel_drag(cx);
            return;
        }
        self.update_drag(&press, position, dx, cx);
        if !was_dragging {
            Self::tell_tips(cx, |tips, cx| tips.close(cx));
        }
        cx.notify();
    }

    fn update_drag(
        &mut self,
        press: &Press,
        position: Point<Pixels>,
        dx: f32,
        cx: &mut Context<Self>,
    ) {
        let landed = self.motion.drag.map_or(press.from, |drag| drag.to);
        let is_reordering = Self::is_strip_y(position.y);
        let dx = if is_reordering {
            resist_drag(&press.boxes, press.from, dx, press.bounds)
        } else {
            0.
        };
        let to = if is_reordering {
            let index = drop_index(&press.boxes, press.from, dx, landed);
            self.tabs
                .reorder_target(press.id, index)
                .unwrap_or(press.from)
        } else {
            press.from
        };
        let drag = Drag {
            id: press.id,
            from: press.from,
            to,
            dx,
            step: press.boxes[press.from].width + f32::from(theme::TAB_GAP),
        };
        if is_reordering && drag.to != landed {
            self.haptics.alignment();
        }
        self.stand_aside(&drag);
        self.motion.drag = Some(drag);
        self.set_preview(
            self.split_drop_for(press, position).map(|(side, _)| side),
            cx,
        );
    }

    fn is_strip_y(y: Pixels) -> bool {
        y >= Pixels::ZERO && y < theme::page_bar::BAR_HEIGHT
    }

    /// Resolves the release against the current edges and the original source and target.
    pub(super) fn release(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        Self::tell_tips(cx, |tips, _| tips.release());
        let Some(press) = self.motion.press.take() else {
            return;
        };
        let drag = self.motion.drag;
        self.end_drag();
        self.set_preview(None, cx);
        cx.notify();
        if crate::ui::dialog_guard::is_open(cx) {
            return;
        }
        if let Some((side, target)) = drag.and_then(|_| self.split_drop_for(&press, position)) {
            let (left, right) = match side {
                SplitSide::Left => (press.id, target),
                SplitSide::Right => (target, press.id),
            };
            self.create_split(left, right, cx);
        } else if Self::is_strip_y(position.y)
            && position.x >= Pixels::ZERO
            && position.x <= self.strip_width
        {
            self.release_in_strip(press.id, drag, cx);
        }
    }

    fn release_in_strip(&mut self, id: TabId, drag: Option<Drag>, cx: &mut Context<Self>) {
        if let Some(drag) = drag {
            let moved = self.tabs.move_to(drag.id, drag.to);
            self.tell_if(moved, cx);
        } else {
            self.activate(id, cx);
        }
    }

    /// Returns an interrupted gesture to its original strip slot.
    pub(super) fn cancel_drag(&mut self, cx: &mut Context<Self>) {
        Self::tell_tips(cx, |tips, _| tips.release());
        if self.motion.press.is_some() {
            self.end_drag();
            self.set_preview(None, cx);
            cx.notify();
        }
    }

    /// Starts watching `window` for the loss of its active state, once: the release that would land a
    /// tab may never reach a window another app took the front from.
    pub(super) fn watch_window(&mut self, window: &mut gpui::Window, cx: &mut Context<Self>) {
        if self.activation.is_some() {
            return;
        }
        let watch = cx.observe_window_activation(window, |bar, window, cx| {
            if !window.is_window_active() {
                bar.cancel_drag(cx);
            }
        });
        self.activation = Some(watch);
    }

    /// The width the row keeps for the frames the tabs glide in, when the tabs changed: what the tabs of the
    /// last order spanned, which only matters when the new order spans less.
    fn keep_the_row_wide(&mut self) {
        let boxes = self
            .motion
            .order
            .iter()
            .filter_map(|id| self.motion.slots.get(id));
        let (left, right) = boxes.fold((f32::MAX, f32::MIN), |(left, right), slot| {
            (left.min(slot.left), right.max(slot.right()))
        });
        if right > left {
            let width = right - left;
            self.motion.floor = Some(RowFloor {
                width,
                is_fresh: true,
            });
        }
    }

    /// The least width the row has now: none when no tab is on its way.
    pub(super) fn row_floor(&mut self) -> Option<f32> {
        let floor = self.motion.floor?;
        if floor.is_fresh {
            self.motion.floor = Some(RowFloor {
                is_fresh: false,
                ..floor
            });
            return Some(floor.width);
        }
        if !self.is_moving() {
            self.motion.floor = None;
            return None;
        }
        let width = self.motion.drawn_extent;
        self.motion.floor = Some(RowFloor { width, ..floor });
        Some(width)
    }

    /// Tells the bar the tabs changed: a different order glides from where the tabs stood, and a tab in the
    /// hand goes back among the others.
    pub(super) fn note_change(&mut self, cx: &mut Context<Self>) {
        let order: Vec<TabId> = self.tabs.tabs().iter().map(|tab| tab.id()).collect();
        if order != self.motion.order {
            self.keep_the_row_wide();
            let lefts = self.motion.slots.iter().map(|(id, slot)| (*id, slot.left));
            self.motion.stood = Some(lefts.collect());
            self.motion.offsets.retain(|id, _| order.contains(id));
            self.motion.order = order;
        }
        self.end_drag();
        self.set_preview(None, cx);
    }

    /// Records where the tab `id` rests in this layout and returns how far to draw it from there, moving
    /// what moves to `now`: the tab in the hand is under the pointer, the others are on their springs.
    pub(super) fn place(&mut self, id: TabId, slot: TabBox, now: Instant, is_still: bool) -> f32 {
        self.motion.slots.insert(id, slot);
        self.glide_from_where_it_stood(id, slot.left, now, is_still);
        if let Some(drag) = self.motion.drag.filter(|drag| drag.id == id) {
            self.motion.previous_right = None;
            return drag.dx;
        }
        let value = self.spring_value(id, now, is_still);
        let value = self.keep_clear_of_previous(id, slot, value);
        self.measure_drawn_extent(id, slot, value);
        value
    }

    /// Adds the tab `id`, drawn `value` from `slot`, to the extent the tabs are drawn over this frame.
    fn measure_drawn_extent(&mut self, id: TabId, slot: TabBox, value: f32) {
        if self.tabs.index_of(id) == Some(0) {
            self.motion.frame = Some((slot.left, f32::MIN));
        }
        let Some((left, right)) = self.motion.frame else {
            return;
        };
        let right = right.max(slot.right() + value);
        self.motion.frame = Some((left, right));
        if self.tabs.index_of(id) == self.tabs.len().checked_sub(1) {
            self.motion.drawn_extent = right - left;
        }
    }

    /// Where the spring of the tab `id` has it now, moved to `now`.
    fn spring_value(&mut self, id: TabId, now: Instant, is_still: bool) -> f32 {
        let spring = if self.motion.drag.is_some() {
            PRESS_DOWN
        } else {
            PRESS_UP
        };
        let Some(offset) = self.motion.offsets.get_mut(&id) else {
            return 0.;
        };
        let elapsed = now.saturating_duration_since(offset.at);
        offset.at = now;
        if is_still {
            let target = offset.spring.target();
            offset.spring.jump_to(target);
        } else {
            offset.spring.advance(spring, elapsed);
        }
        let value = offset.spring.value;
        if offset.spring.is_at_rest() && value == 0. {
            self.motion.offsets.remove(&id);
        }
        value
    }

    /// `value`, raised when the tab would be drawn over the one before it. A tab glides from where it stood
    /// but is drawn at the width of its new slot (the pills of a full row grow when one closes, the open
    /// one is wider than the rest), so the springs of two neighbours alone cannot keep them apart: a
    /// tab never goes left of the right edge of the one before it. While a tab is in the hand the tabs
    /// give way to it and may be drawn under it, so nothing is held then.
    fn keep_clear_of_previous(&mut self, id: TabId, slot: TabBox, value: f32) -> f32 {
        let is_first = self.tabs.index_of(id) == Some(0);
        let previous = self.motion.previous_right.filter(|_| !is_first);
        let gap = if self.tabs.split().is_some_and(|pair| pair.right == id) {
            page_bar::JOINED_PILL_GAP
        } else {
            theme::TAB_GAP
        };
        let least = previous.map_or(f32::MIN, |right| right + f32::from(gap) - slot.left);
        let value = if self.motion.drag.is_some() {
            value
        } else {
            value.max(least)
        };
        self.motion.previous_right = Some(slot.right() + value);
        value
    }

    /// Whether another frame is due: a tab on its spring.
    pub(super) fn is_moving(&self) -> bool {
        self.motion
            .offsets
            .values()
            .any(|offset| !offset.spring.is_at_rest())
    }

    /// After a change, a tab is drawn where it stood and travels to its new slot.
    fn glide_from_where_it_stood(&mut self, id: TabId, left: f32, now: Instant, is_still: bool) {
        let Some(before) = self
            .motion
            .stood
            .as_mut()
            .and_then(|stood| stood.remove(&id))
        else {
            return;
        };
        if is_still || before == left {
            return;
        }
        let offset = self.motion.offsets.entry(id).or_insert(Offset {
            spring: Sprung::at_rest(0.),
            at: now,
        });
        offset.spring.displace(before - left);
    }

    /// The tabs `drag` passes head one step aside; the others head home.
    fn stand_aside(&mut self, drag: &Drag) {
        let now = Instant::now();
        let ids: Vec<TabId> = self.tabs.tabs().iter().map(|tab| tab.id()).collect();
        for (index, id) in ids.into_iter().enumerate().filter(|(_, id)| *id != drag.id) {
            let aside = slide_offset(index, drag.from, drag.to, drag.step);
            let offset = self.motion.offsets.entry(id).or_insert(Offset {
                spring: Sprung::at_rest(0.),
                at: now,
            });
            offset.spring.send_to(aside);
        }
    }

    /// The tab in the hand, if any, goes back among the others: it and the tabs that stood aside head home.
    fn end_drag(&mut self) {
        self.motion.press = None;
        if let Some(drag) = self.motion.drag.take() {
            let mut held = Sprung::at_rest(drag.dx);
            held.send_to(0.);
            let held = Offset {
                spring: held,
                at: Instant::now(),
            };
            self.motion.offsets.insert(drag.id, held);
        }
        for offset in self.motion.offsets.values_mut() {
            offset.spring.send_to(0.);
        }
    }
}
