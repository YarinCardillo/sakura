//! The steps row of a flow lane's header: the bars of the steps the lane runs, which share the width, and the
//! "Steps" menu at the end of the bar that lists every step with where the lane stands on it and a switch for each
//! step the engine lets the person turn on or off. A step that is turned on or off joins or leaves the bar with
//! the shared motion tokens. The engine decides what may be toggled and why not: this shows its word.

use super::LanePage;
use crate::l10n::{tr, Key};
use crate::theme::{self, lane_page as sizes, Palette};
use crate::ui::answers::lane_key;
use crate::ui::engine_acts::SET_LANE_STEP;
use crate::ui::kit::{scroll_dropdown, ScrollMenuItem};
use crate::ui::step_track::{
    pipeline_segment, pipeline_words, running_track, step_track_with, TrackSegment,
};
use gpui::{
    div, AnyElement, App, ClickEvent, Context, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, Styled as _, Window,
};
use gpui_base::motion::{transition, Transition};
use sakura_core::engine::snapshot::{Item, PipelineStep};
use std::collections::BTreeMap;

const STEPS: Key = Key("lane_side.steps");

impl LanePage {
    /// The bars and the Steps menu; with no pipeline from the engine, the bars of the steps it reports and no menu.
    pub(super) fn steps_row(
        &self,
        item: &Item,
        pipeline: &[PipelineStep],
        palette: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let drawn = match pipeline.is_empty() {
            true => running_track(&[], &item.steps, item.in_review)
                .into_iter()
                .map(|segment| (segment, 1.))
                .collect(),
            false => present_steps(pipeline, &item.id, window, cx),
        };
        if drawn.is_empty() && pipeline.is_empty() {
            return None;
        }
        let menu = (!pipeline.is_empty()).then(|| self.steps_menu(pipeline.to_vec(), cx));
        let key = lane_key(&item.id, SET_LANE_STEP.key);
        let is_pending = self.pending_step.is_some()
            && !matches!(
                self.answers.read(cx).step(&key),
                Some(crate::ui::answers::Step::Told {
                    is_problem: true,
                    ..
                })
            );
        let pending = is_pending.then(|| {
            crate::ui::chrome::text(theme::TEXT_LABEL, palette.text_secondary)
                .debug_selector(|| "lane-step-pending".into())
                .child(tr(Key("thread.status.pending")))
        });
        Some(
            div()
                .flex()
                .items_start()
                .gap(sizes::TRACK_MENU_GAP)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .max_w(sizes::TRACK_MAX_WIDTH)
                        .child(step_track_with(&drawn, palette)),
                )
                .children(pending)
                .children(menu)
                .into_any_element(),
        )
    }

    /// The menu of every step: each with its state in words, a switch (a check) where the engine lets it be
    /// toggled and its reason, in words, where it does not.
    fn steps_menu(&self, steps: Vec<PipelineStep>, cx: &mut Context<Self>) -> AnyElement {
        let page = cx.weak_entity();
        let button = crate::ui::kit::menu_chip("lane-steps", tr(STEPS), None, cx)
            .debug_selector(|| "lane-steps".into());
        scroll_dropdown(button, gpui::Anchor::TopLeft, move |menu, _, _| {
            let menu = menu.max_w(sizes::STEPS_MENU_MAX_WIDTH);
            steps
                .iter()
                .fold(menu, |menu, step| menu.item(step_item(step, &page)))
        })
        .into_any_element()
    }

    /// Turns `step` of the lane on or off through the engine's action; a lane the engine lists no such action for
    /// is left as it is.
    pub(super) fn toggle_step(&mut self, step: &str, on: bool, cx: &mut Context<Self>) {
        let snapshot = self.facts.read(cx).shared_snapshot();
        let Some(item) = snapshot.as_ref().and_then(|snapshot| self.item(snapshot)) else {
            return;
        };
        let Some(action) = item
            .actions
            .iter()
            .find(|action| action.key == SET_LANE_STEP.key)
        else {
            return;
        };
        let state = if on { "on" } else { "off" };
        let inputs = BTreeMap::from([
            ("step".to_string(), step.to_string()),
            ("state".to_string(), state.to_string()),
        ]);
        let key = lane_key(&item.id, &action.key);
        if self.answers.read(cx).is_running(&key) {
            return;
        }
        self.pending_step = Some((step.to_string(), on));
        let (action, target) = (action.clone(), item.id.clone());
        self.answers.update(cx, |answers, cx| {
            answers.press(key, action, target, inputs, cx)
        });
    }
}

/// The segments of the steps that run or have only just stopped running, each with how present it is: a step that
/// is turned on grows into the bar and one that is turned off shrinks out of it, over the base duration.
fn present_steps(
    pipeline: &[PipelineStep],
    lane: &str,
    window: &mut Window,
    cx: &mut App,
) -> Vec<(TrackSegment, f32)> {
    pipeline
        .iter()
        .filter_map(|step| {
            let target = if step.is_on { 1. } else { 0. };
            let id = SharedString::from(format!("step-presence-{lane}-{}", step.id));
            let presence: f32 = transition(
                id,
                target,
                Transition::new(theme::DURATION_BASE),
                window,
                cx,
            );
            (presence > 0.).then(|| (pipeline_segment(step), presence))
        })
        .collect()
}

/// One row of the menu: the step's word and where the lane stands on it, with a check while it runs; one the
/// engine does not let be toggled is dimmed and says why under its name.
fn step_item(step: &PipelineStep, page: &gpui::WeakEntity<LanePage>) -> ScrollMenuItem {
    let shown = step.clone();
    let row = ScrollMenuItem::element(move |_, cx| step_row(&shown, cx)).checked(step.is_on);
    match step.can_toggle {
        false => row.disabled(true),
        true => {
            let (page, id, turn_on) = (page.clone(), step.id.clone(), !step.is_on);
            row.on_click(move |_: &ClickEvent, _, cx| {
                drop(page.update(cx, |page, cx| page.toggle_step(&id, turn_on, cx)));
            })
        }
    }
}

fn step_row(step: &PipelineStep, cx: &App) -> AnyElement {
    let palette = theme::Theme::palette(cx);
    let top = div()
        .flex()
        .items_center()
        .justify_between()
        .gap(theme::GAP_WIDE)
        .child(div().flex_1().min_w_0().child(pipeline_words(step)))
        .child(
            div()
                .flex_none()
                .text_color(palette.text_tertiary)
                .child(menu_state_words(&step.state)),
        );
    let why = (!step.can_toggle && !step.reason.is_empty()).then(|| {
        div()
            .text_size(theme::TEXT_SMALL)
            .text_color(palette.text_tertiary)
            .child(tr(Key("lane_steps.cannot_change")))
    });
    let note = (step.state == "waiting" && !step.note.is_empty()).then(|| {
        let selector = format!("steps-note-{}", step.id);
        div()
            .debug_selector(move || selector.clone())
            .text_size(theme::TEXT_SMALL)
            .text_color(palette.text_tertiary)
            .child(tr(Key("lane_steps.waiting_for_action")))
    });
    let selector = format!("steps-row-{}", step.id);
    div()
        .debug_selector(move || selector.clone())
        .flex()
        .flex_col()
        .w_full()
        .child(top)
        .children(why)
        .children(note)
        .into_any_element()
}

/// What the menu says of a step: the engine's `waiting` is the lane needing the person; any other state is said as
/// it is. A step that runs between two tasks of the lane is still running, so the lane's own state is not read.
fn menu_state_words(state: &str) -> SharedString {
    match state {
        "waiting" => tr(Key("lane_steps.state.waiting")).into(),
        _ => state_words(state),
    }
}

fn state_words(state: &str) -> SharedString {
    let key = match state {
        "done" => Key("lane_steps.state.done"),
        "running" => Key("lane_steps.state.running"),
        "off" => Key("lane_steps.state.off"),
        _ => Key("lane_steps.state.next"),
    };
    tr(key).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_step_that_runs_is_never_said_to_need_the_person_unless_the_engine_says_waiting() {
        crate::l10n::set_for_test(crate::l10n::Language::English);
        // Between two build tasks the lane is not working, and the step still runs.
        assert_eq!(menu_state_words("running"), "Running");
        assert_eq!(menu_state_words("running"), "Running");
        assert_eq!(menu_state_words("done"), "Done");
        // The engine's own word for a step the lane needs the person in (its commit e9b22d6).
        assert_eq!(menu_state_words("waiting"), "Needs you");
        assert_eq!(menu_state_words("waiting"), "Needs you");
    }
}
