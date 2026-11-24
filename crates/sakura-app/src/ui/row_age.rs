//! Task age labels on their own layer. A tick reuses the sidebar and page unchanged.

use super::chrome::tabular;
use super::unix_now;
use crate::l10n::say;
use crate::theme::{self, Palette};
use gpui::{
    canvas, div, App, Bounds, ContentMask, Context, Div, FontWeight, Global, Hsla,
    InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Render, Styled as _,
    Subscription, Task, WeakEntity, Window,
};
use sakura_core::model::{self, TaskAge, TaskState};
use sakura_core::models::Task as Work;
use std::collections::BTreeMap;
#[cfg(test)]
use std::time::Instant;

/// Fixed room for an age, so crossing a unit boundary never moves the task's name or figures.
pub const AGE_WIDTH: Pixels = gpui::px(64.);

#[derive(Clone, PartialEq)]
struct Mark {
    at: i64,
    bounds: Bounds<Pixels>,
    mask: ContentMask<Pixels>,
    colour: Hsla,
}

#[derive(Default)]
pub struct RowAges {
    marks: BTreeMap<String, Mark>,
    layer: Option<WeakEntity<RowAgeLayer>>,
}

impl Global for RowAges {}

impl RowAges {
    /// Clears the previous sidebar's positions, also when the sidebar is hidden.
    pub fn clear(cx: &mut App) {
        let ages = cx.default_global::<Self>();
        if !ages.marks.is_empty() {
            ages.marks.clear();
            Self::wake(cx);
        }
    }

    fn add(task: String, mark: Mark, cx: &mut App) {
        let ages = cx.default_global::<Self>();
        if ages.marks.get(&task) != Some(&mark) {
            ages.marks.insert(task, mark);
            Self::wake(cx);
        }
    }

    fn wake(cx: &mut App) {
        if let Some(layer) = cx.default_global::<Self>().layer.clone() {
            cx.defer(move |cx| {
                drop(layer.update(cx, |layer, cx| {
                    layer.tick = None;
                    cx.notify();
                }));
            });
        }
    }
}

/// Reserves a row's age slot and records its visible position for the label layer.
pub fn slot(task: &Work, shown: TaskState, palette: &Palette) -> Option<Div> {
    let at = model::task_age(task, shown, unix_now())?.at;
    let (task, colour) = (task.name.clone(), palette.text_tertiary);
    let selector = format!("row-age-{task}");
    let mark = canvas(
        |_, _, _| (),
        move |bounds, (), window, cx| {
            let mask = window.content_mask();
            if bounds.intersects(&mask.bounds) {
                RowAges::add(
                    task.clone(),
                    Mark {
                        at,
                        bounds,
                        mask,
                        colour,
                    },
                    cx,
                );
            }
        },
    )
    .size_full();
    Some(
        div()
            .debug_selector(move || selector)
            .w(AGE_WIDTH)
            .h(theme::ROW_HEIGHT)
            .flex_none()
            .child(mark),
    )
}

/// One timer for the visible row labels, at the earliest next minute or hour boundary.
pub struct RowAgeLayer {
    #[cfg(test)]
    clock: Option<(i64, Instant)>,
    is_visible: bool,
    tick: Option<Task<()>>,
    _visibility: Subscription,
}

impl RowAgeLayer {
    /// Creates the label layer of the window and cancels its timer while fully hidden.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.default_global::<RowAges>().layer = Some(cx.weak_entity());
        let visibility = cx.observe_window_visibility(window, |layer, visibility, _, cx| {
            layer.is_visible = visibility.is_visible();
            layer.tick = None;
            if layer.is_visible {
                cx.notify();
            }
        });
        Self {
            #[cfg(test)]
            clock: None,
            is_visible: window.is_visible(),
            tick: None,
            _visibility: visibility,
        }
    }

    fn now(&self, cx: &App) -> i64 {
        #[cfg(test)]
        if let Some((epoch, started_at)) = self.clock {
            let elapsed = cx
                .background_executor()
                .now()
                .saturating_duration_since(started_at);
            return epoch.saturating_add(elapsed.as_secs().min(i64::MAX as u64) as i64);
        }
        let _ = cx;
        unix_now()
    }

    fn schedule(&mut self, now: i64, cx: &mut Context<Self>) {
        if !self.is_visible || self.tick.is_some() {
            return;
        }
        let delay = cx
            .default_global::<RowAges>()
            .marks
            .values()
            .map(|mark| TaskAge::at(mark.at, now).refresh_in)
            .min();
        let Some(delay) = delay else {
            return;
        };
        self.tick = Some(cx.spawn(async move |layer, cx| {
            cx.background_executor().timer(delay).await;
            drop(layer.update(cx, |layer, cx| {
                layer.tick = None;
                cx.notify();
            }));
        }));
    }

    #[cfg(test)]
    pub(super) fn is_ticking(&self) -> bool {
        self.tick.is_some()
    }

    #[cfg(test)]
    pub(super) fn set_now(&mut self, now: i64, cx: &mut Context<Self>) {
        self.clock = Some((now, cx.background_executor().now()));
        self.tick = None;
        cx.notify();
    }
}

impl Render for RowAgeLayer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.is_visible = window.is_visible();
        if !self.is_visible {
            self.tick = None;
        }
        let now = self.now(cx);
        self.schedule(now, cx);
        let labels = cx
            .default_global::<RowAges>()
            .marks
            .iter()
            .map(|(task, mark)| label(task, mark, now))
            .collect::<Vec<_>>();
        div().absolute().size_full().children(labels)
    }
}

fn label(task: &str, mark: &Mark, now: i64) -> Div {
    let clip = mark.bounds.intersect(&mark.mask.bounds);
    let words = say(&TaskAge::at(mark.at, now).said);
    let selector = format!("row-age-text-{task}-{words}");
    let text = div()
        .debug_selector(move || selector)
        .absolute()
        .left(mark.bounds.left() - clip.left())
        .top(mark.bounds.top() - clip.top())
        .w(mark.bounds.size.width)
        .h(mark.bounds.size.height)
        .flex()
        .items_center()
        .justify_end()
        .text_size(theme::TEXT_LABEL)
        .font_weight(FontWeight::NORMAL)
        .font_family(theme::FONT_UI)
        .font_features(tabular())
        .text_color(mark.colour)
        .child(words);
    div()
        .absolute()
        .left(clip.left())
        .top(clip.top())
        .w(clip.size.width)
        .h(clip.size.height)
        .overflow_hidden()
        .child(text)
}
