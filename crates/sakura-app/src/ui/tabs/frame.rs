//! One frame of the strip: where every tab is drawn right now and what the fused shape looks like. The tabs
//! are laid out for the strip's real width on every frame, each is drawn at its slot plus what its spring
//! or the pointer adds, and the corner follows the active tab's distance from the panel's edge. With no tab
//! open the frame holds no face and the shape is the panel's top alone.

use super::drag::TabBox;
use super::layout::{layout, scroll_to_show};
use super::metrics::{
    CHROME_CLOSABLE, CHROME_PLAIN, PANEL_OVERHANG, STRIP_HEIGHT, TAB_MAX_WIDTH, TITLE_SIZE,
    TOP_INSET, TRAILING_ROOM,
};
use super::morph::morph_progress;
use super::motion::{Sprung, PRESS_DOWN, PRESS_UP};
use super::paint::Prepared;
use super::shape::{Fused, PanelTop, Top};
use super::strip::Strip;
use crate::theme;
use gpui::{font, px, Bounds, Context, Hsla, Pixels, SharedString, TextRun, Window};
use sakura_core::model::TaskState;
use sakura_core::tabs::{shortcut_digit, TabId};
use std::rc::Rc;
use std::time::{Duration, Instant};

/// How one tab is drawn in a frame.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct TabFace {
    pub id: TabId,
    pub title: SharedString,
    pub is_active: bool,
    /// In the hand: it keeps its own look and shows the hover it had when picked up.
    pub is_held: bool,
    /// Its lavoro, and the state the window shows for it (none while the lavoro is not read yet).
    pub task: String,
    pub state: Option<TaskState>,
    /// Whether it can close: another tab remains.
    pub can_close: bool,
    /// Whether it has a close button, shown or not. A tab at its full width keeps the button's room; a
    /// narrowed one gives it to its title and closes from its menu, the middle button or Cmd+W. The tab's
    /// width alone decides, never whether it is selected, so choosing a tab changes nothing inside it.
    pub has_close: bool,
    /// The digit of the shortcut that selects it, if one does.
    pub digit: Option<usize>,
    /// Where it is drawn, from the strip's left edge.
    pub drawn: TabBox,
}

/// What the strip draws in one frame.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Frame {
    pub faces: Vec<TabFace>,
    /// The shape over the panel: the tab shown fused into its top, or the top alone.
    pub top: Top,
    /// The span inactive tabs show in, from the strip's left edge: scrolled tabs are cut at its ends.
    pub view: (f32, f32),
    /// Whether tabs are cut at the view's start and at its end: each cut edge fades into the canvas.
    pub is_cut: (bool, bool),
}

impl Strip {
    /// Lays the strip out in `bounds` and moves everything that moves to now.
    pub(super) fn frame(
        &mut self,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Frame {
        let now = Instant::now();
        let elapsed = self.last_frame.map_or(Duration::ZERO, |last| now - last);
        self.last_frame = Some(now);
        let is_still = cx.reduce_motion();
        let width = f32::from(bounds.size.width);
        let naturals = self.naturals(window);
        self.lay_out(&naturals, width);
        self.glide(is_still);
        self.advance(elapsed, is_still);
        self.origin = bounds.origin;
        let faces = self.faces(&naturals);
        let top = self.top(&faces, bounds);
        self.prepare(&top);
        if self.is_moving() {
            window.request_animation_frame();
        }
        Frame {
            faces,
            top,
            view: (self.leading_inset, width),
            is_cut: (self.scroll > 0., self.scroll < self.furthest),
        }
    }

    /// The width that shows each tab's whole title, its icon and its close button.
    fn naturals(&self, window: &Window) -> Vec<f32> {
        let chrome = self.chrome();
        let titles = self.tabs.tabs().iter().map(|tab| title_of(&tab.task));
        titles
            .map(|title| title_width(&title, window) + chrome)
            .collect()
    }

    fn chrome(&self) -> f32 {
        if self.tabs.len() > 1 {
            CHROME_CLOSABLE
        } else {
            CHROME_PLAIN
        }
    }

    /// The tabs' slots for a strip `width` wide, scrolled so the active tab shows (a drag never scrolls).
    fn lay_out(&mut self, naturals: &[f32], width: f32) {
        let (start, end) = (self.leading_inset, width - TRAILING_ROOM);
        let boxes = layout(naturals, start, end - start);
        let content_end = boxes.last().map_or(start, TabBox::right);
        self.furthest = (content_end - end).max(0.);
        let shown = self.tabs.active_index().map(|index| boxes[index]);
        let is_free = self.press.is_none() && std::mem::take(&mut self.is_showing_active);
        if let Some(shown) = shown.filter(|_| is_free) {
            self.scroll = scroll_to_show(self.scroll, shown, start, end, content_end);
        }
        self.scroll = self.scroll.clamp(0., self.furthest);
        let ids = self.tabs.tabs().iter().map(|tab| tab.id);
        self.slots = ids
            .zip(boxes)
            .map(|(id, slot)| {
                let left = slot.left - self.scroll;
                (id, TabBox { left, ..slot })
            })
            .collect();
    }

    /// After a change to the tabs, each is drawn where it stood and travels to its new slot.
    fn glide(&mut self, is_still: bool) {
        let Some(stood) = self.glide_from.take() else {
            return;
        };
        if is_still {
            return self.offsets.clear();
        }
        for (id, slot) in &self.slots {
            let Some(before) = stood.get(id) else {
                continue;
            };
            let offset = self
                .offsets
                .entry(*id)
                .or_insert_with(|| Sprung::at_rest(0.));
            offset.displace(before - slot.left);
        }
    }

    /// Lets `elapsed` pass on every tab's spring: quick while a tab is in the hand, soft otherwise.
    fn advance(&mut self, elapsed: Duration, is_still: bool) {
        let spring = if self.drag.is_some() {
            PRESS_DOWN
        } else {
            PRESS_UP
        };
        for offset in self.offsets.values_mut() {
            if is_still {
                offset.jump_to(offset.target());
            } else {
                offset.advance(spring, elapsed);
            }
        }
        self.offsets
            .retain(|_, offset| !offset.is_at_rest() || offset.value != 0.);
    }

    /// How far the tab `id` is drawn from its slot.
    fn offset_of(&self, id: TabId) -> f32 {
        match self.drag {
            Some(drag) if drag.id == id => drag.dx,
            _ => self.offsets.get(&id).map_or(0., |offset| offset.value),
        }
    }

    fn faces(&self, naturals: &[f32]) -> Vec<TabFace> {
        let (count, active) = (self.tabs.len(), self.tabs.active_id());
        let tabs = self.tabs.tabs().iter().zip(&self.slots).zip(naturals);
        tabs.enumerate()
            .map(|(index, ((tab, (_, slot)), natural))| TabFace {
                id: tab.id,
                title: title_of(&tab.task),
                is_active: Some(tab.id) == active,
                is_held: self.drag.is_some_and(|drag| drag.id == tab.id),
                task: tab.task.clone(),
                state: self.states.get(&tab.task).copied(),
                can_close: count > 1,
                has_close: count > 1 && slot.width + 0.5 >= natural.min(TAB_MAX_WIDTH),
                digit: shortcut_digit(index, count),
                drawn: TabBox {
                    left: slot.left + self.offset_of(tab.id),
                    width: slot.width,
                },
            })
            .collect()
    }

    /// The shape over the panel: the active tab as drawn fused into its top, with the corner at what the
    /// tab's distance gives; with no tab open, the panel's top alone, both corners round.
    fn top(&mut self, faces: &[TabFace], bounds: Bounds<Pixels>) -> Top {
        let (left, top) = (f32::from(bounds.left()), f32::from(bounds.top()));
        // The panel runs past the strip's right end, the window's edge: see `PANEL_OVERHANG`.
        let right = f32::from(bounds.right()) + PANEL_OVERHANG;
        let panel_top = top + STRIP_HEIGHT;
        let Some(active) = self.tabs.active_index().map(|index| faces[index].drawn) else {
            self.corner = 1.;
            return Top::Plain(PanelTop {
                left,
                right,
                top: panel_top,
            });
        };
        self.corner = morph_progress(active.left);
        Top::Fused(Fused {
            panel_left: left,
            panel_right: right,
            panel_top,
            tab_left: left + active.left,
            tab_right: left + active.right(),
            tab_top: top + TOP_INSET,
            morph: self.corner,
        })
    }

    /// Builds the paths of `top` unless the ones kept are already its.
    fn prepare(&mut self, top: &Top) {
        let is_kept = self.prepared.as_ref().is_some_and(|kept| kept.is_for(top));
        if !is_kept {
            self.prepared = Some(Rc::new(Prepared::of(top)));
            self.shapes_built += 1;
        }
    }

    /// Whether another frame is due: a tab on its spring.
    pub(super) fn is_moving(&self) -> bool {
        self.offsets.values().any(|offset| !offset.is_at_rest())
    }
}

/// A tab's title: the name of its lavoro.
fn title_of(task: &str) -> SharedString {
    SharedString::from(task.to_string())
}

/// The width of `title` at the tab's size and weight, rounded up so the last letter is never cut.
fn title_width(title: &SharedString, window: &Window) -> f32 {
    let mut face = font(theme::FONT_UI);
    face.weight = theme::WEIGHT_MEDIUM;
    let run = TextRun {
        len: title.len(),
        font: face,
        color: Hsla::default(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let line = window
        .text_system()
        .shape_line(title.clone(), px(TITLE_SIZE), &[run], None);
    f32::from(line.width).ceil() + 1.
}
