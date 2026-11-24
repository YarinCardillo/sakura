//! The step track of a lane's header: one segment per step the engine reports, done filled, the one in
//! progress half filled, a skipped one dashed, the step's word under each. While a review of the step in
//! progress runs, its word says so.

use crate::l10n::{tr, tr_with, Key};
use crate::theme::{self, lane_page as sizes, Palette};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    div, relative, AnyElement, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, Styled as _,
};
use sakura_core::engine::snapshot::{LaneStep, PipelineStep};

/// How a segment of the track is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentState {
    Done,
    Now,
    Next,
    Skipped,
}

/// One segment: the step's id, its word and where the lane stands on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackSegment {
    pub id: String,
    pub label: SharedString,
    pub state: SegmentState,
}

/// The segments of `steps`, in the engine's order; `in_review` adds ", in review" to the step in progress.
pub fn track_of(steps: &[LaneStep], in_review: bool) -> Vec<TrackSegment> {
    steps
        .iter()
        .map(|step| {
            let state = state_of(&step.state);
            let word = step_words(step);
            let label = match (state, in_review) {
                (SegmentState::Now, true) => tr_with(Key("step.in_review"), &[("step", &word)]),
                // A step turned off says so in its word, so it is not read as one still to come.
                (SegmentState::Skipped, _) => tr_with(Key("step.off"), &[("step", &word)]),
                _ => word,
            };
            TrackSegment {
                id: step.id.clone(),
                label,
                state,
            }
        })
        .collect()
}

/// The segments the header draws: only the steps the lane runs. The engine's whole pipeline says which they
/// are; from an engine that does not list it, the steps it reports less the ones turned off.
pub fn running_track(
    pipeline: &[PipelineStep],
    steps: &[LaneStep],
    in_review: bool,
) -> Vec<TrackSegment> {
    if pipeline.is_empty() {
        let track = track_of(steps, in_review);
        return track
            .into_iter()
            .filter(|segment| segment.state != SegmentState::Skipped)
            .collect();
    }
    pipeline
        .iter()
        .filter(|step| step.is_on)
        .map(pipeline_segment)
        .collect()
}

/// One step of the whole pipeline as a segment; `off` is a skipped one.
pub fn pipeline_segment(step: &PipelineStep) -> TrackSegment {
    let state = match step.state.as_str() {
        "done" => SegmentState::Done,
        // A step the lane waits in is the one it is in.
        "running" | "waiting" => SegmentState::Now,
        "off" => SegmentState::Skipped,
        _ => SegmentState::Next,
    };
    TrackSegment {
        id: step.id.clone(),
        label: pipeline_words(step),
        state,
    }
}

/// The word of a step of the whole pipeline; one this build has no word for is shown as the engine names it.
pub fn pipeline_words(step: &PipelineStep) -> SharedString {
    let key = match step.id.as_str() {
        "plan" => Key("step.plan"),
        "plan_review" => Key("step.plan_review"),
        "build" => Key("step.build"),
        "task_review" => Key("step.task_review"),
        "final_review" => Key("step.final_review"),
        "checks" => Key("step.checks"),
        "evaluator" => Key("step.evaluator"),
        "deliver" => Key("step.deliver"),
        "reporter" => Key("step.reporter"),
        _ => return SharedString::from(step.name.clone()),
    };
    SharedString::from(tr(key))
}

/// How many of the steps run and how many of those are done: the count every page says, from the same steps
/// the track draws, a step turned off in neither.
pub fn counted(steps: &[LaneStep]) -> (u32, u32) {
    let track = track_of(steps, false);
    let running = track
        .iter()
        .filter(|segment| segment.state != SegmentState::Skipped);
    let total = running.clone().count();
    let done = running
        .filter(|segment| segment.state == SegmentState::Done)
        .count();
    (
        u32::try_from(done).unwrap_or(u32::MAX),
        u32::try_from(total).unwrap_or(u32::MAX),
    )
}

/// The word of a step; a step this build has no word for is shown as the engine names it.
pub fn step_words(step: &LaneStep) -> SharedString {
    let key = match step.id.as_str() {
        "plan" => Key("step.plan"),
        "build" => Key("step.build"),
        "checks" => Key("step.checks"),
        "deliver" => Key("step.deliver"),
        _ => return SharedString::from(step.name.clone()),
    };
    SharedString::from(tr(key))
}

fn state_of(state: &str) -> SegmentState {
    match state {
        "done" => SegmentState::Done,
        "now" => SegmentState::Now,
        "skipped" => SegmentState::Skipped,
        _ => SegmentState::Next,
    }
}

/// Give each step a share of the track proportional to its animated presence.
pub fn step_track_with(segments: &[(TrackSegment, f32)], palette: &Palette) -> AnyElement {
    let columns = segments
        .iter()
        .map(|(segment, presence)| column(segment, *presence, palette));
    div()
        .debug_selector(|| "step-track".into())
        .w_full()
        .max_w(sizes::TRACK_MAX_WIDTH)
        .flex()
        .gap(sizes::TRACK_GAP)
        .children(columns)
        .into_any_element()
}

fn column(segment: &TrackSegment, presence: f32, palette: &Palette) -> AnyElement {
    let ink = match segment.state {
        SegmentState::Done => palette.text_secondary,
        SegmentState::Now => palette.text,
        SegmentState::Next | SegmentState::Skipped => palette.text_tertiary,
    };
    let (selector, label_selector) = (
        format!("step-seg-{}", segment.id),
        format!("step-label-{}", segment.id),
    );
    let label = div()
        .debug_selector(move || label_selector)
        .text_size(sizes::TRACK_LABEL_TEXT)
        .line_height(sizes::TRACK_LABEL_LINE_HEIGHT)
        .text_color(ink)
        .when(segment.state == SegmentState::Now, |label| {
            label.font_weight(theme::WEIGHT_MEDIUM)
        })
        .whitespace_normal()
        .child(segment.label.clone());
    div()
        .min_w_0()
        .max_w_full()
        .flex_basis(sizes::TRACK_SEGMENT_WIDTH * presence)
        .flex_grow(presence)
        .opacity(presence)
        .flex()
        .flex_col()
        .gap(sizes::TRACK_LABEL_GAP)
        .child(bar(segment.state, &selector, palette))
        .child(label)
        .into_any_element()
}

/// The segment: filled when done, partly filled while in progress, dashed when skipped.
fn bar(state: SegmentState, selector: &str, palette: &Palette) -> AnyElement {
    let selector = selector.to_string();
    let track = div()
        .debug_selector(move || selector)
        .h(sizes::TRACK_BAR_HEIGHT)
        .w_full()
        .rounded(sizes::TRACK_BAR_RADIUS)
        .overflow_hidden()
        .flex();
    match state {
        SegmentState::Done => track.bg(palette.text_secondary),
        SegmentState::Now => track.bg(palette.border).child(
            div()
                .h_full()
                .w(relative(sizes::TRACK_NOW_SHARE))
                .bg(palette.text_secondary),
        ),
        SegmentState::Next => track.bg(palette.border),
        SegmentState::Skipped => track
            .border_1()
            .border_dashed()
            .border_color(palette.border),
    }
    .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(id: &str, state: &str) -> LaneStep {
        LaneStep {
            id: id.to_string(),
            name: id.to_uppercase(),
            state: state.to_string(),
            model: None,
            account: None,
            account_label: None,
        }
    }

    fn standard() -> Vec<LaneStep> {
        vec![
            step("plan", "done"),
            step("build", "now"),
            step("checks", "next"),
            step("deliver", "next"),
        ]
    }

    #[test]
    fn a_step_the_lane_waits_in_is_marked_as_the_current_one() {
        let segment = pipeline_segment(&pipeline_step("deliver", "waiting", true));
        assert_eq!(segment.state, SegmentState::Now);
    }

    #[test]
    fn the_track_has_one_segment_per_step_in_the_engines_order_with_its_state() {
        crate::l10n::set_for_test(crate::l10n::Language::English);
        let track = track_of(&standard(), false);
        let seen: Vec<(&str, &str, SegmentState)> = track
            .iter()
            .map(|segment| (segment.id.as_str(), segment.label.as_ref(), segment.state))
            .collect();
        assert_eq!(
            seen,
            [
                ("plan", "Plan", SegmentState::Done),
                ("build", "Build", SegmentState::Now),
                ("checks", "Checks", SegmentState::Next),
                ("deliver", "Deliver", SegmentState::Next),
            ]
        );
    }

    #[test]
    fn the_step_in_progress_says_in_review_while_a_review_runs_and_no_other_does() {
        crate::l10n::set_for_test(crate::l10n::Language::English);
        let track = track_of(&standard(), true);
        assert_eq!(track[1].label, "Build, in review");
        assert_eq!(track[0].label, "Plan");
        assert_eq!(track[2].label, "Checks");
    }

    #[test]
    fn a_skipped_step_is_dashed_and_a_step_the_build_has_no_word_for_keeps_the_engines_name() {
        crate::l10n::set_for_test(crate::l10n::Language::English);
        let steps = [step("plan", "skipped"), step("polish", "now")];
        let track = track_of(&steps, false);
        assert_eq!(track[0].state, SegmentState::Skipped);
        assert_eq!(track[1].label, "POLISH");
    }

    #[test]
    fn a_step_turned_off_says_it_was_turned_off_in_its_label() {
        crate::l10n::set_for_test(crate::l10n::Language::English);
        let track = track_of(&[step("checks", "skipped")], false);
        assert_eq!(track[0].label, "Checks, turned off");
    }

    #[test]
    fn the_count_and_the_track_have_one_source_and_the_step_turned_off_is_in_neither_count() {
        let steps = [
            step("plan", "done"),
            step("build", "now"),
            step("checks", "skipped"),
            step("deliver", "next"),
        ];
        assert_eq!(counted(&steps), (1, 3));
        let drawn_and_counted = track_of(&steps, false)
            .iter()
            .filter(|segment| segment.state != SegmentState::Skipped)
            .count();
        assert_eq!(
            drawn_and_counted, 3,
            "the three that count are the three that run"
        );
    }

    fn pipeline_step(id: &str, state: &str, is_on: bool) -> PipelineStep {
        PipelineStep {
            id: id.to_string(),
            name: id.to_string(),
            state: state.to_string(),
            is_on,
            can_toggle: false,
            reason: String::new(),
            note: String::new(),
        }
    }

    #[test]
    fn the_header_track_draws_only_the_steps_the_lane_runs_from_the_pipeline_or_else_from_the_steps(
    ) {
        crate::l10n::set_for_test(crate::l10n::Language::English);
        let pipeline = [
            pipeline_step("plan", "done", true),
            pipeline_step("final_review", "off", false),
            pipeline_step("build", "running", true),
            pipeline_step("task_review", "next", true),
        ];
        let ids = |segments: Vec<TrackSegment>| -> Vec<String> {
            segments.into_iter().map(|segment| segment.id).collect()
        };
        assert_eq!(
            ids(running_track(&pipeline, &[], false)),
            ["plan", "build", "task_review"]
        );
        let steps = [
            step("plan", "done"),
            step("checks", "skipped"),
            step("deliver", "next"),
        ];
        assert_eq!(ids(running_track(&[], &steps, false)), ["plan", "deliver"]);
        let words: Vec<_> = running_track(&pipeline, &[], false)
            .into_iter()
            .map(|segment| segment.label)
            .collect();
        assert_eq!(words, ["Plan", "Build", "Step review"]);
    }

    #[test]
    fn the_words_exist_in_italian_too() {
        crate::l10n::set_for_test(crate::l10n::Language::Italian);
        let track = track_of(&standard(), true);
        assert_eq!(track[3].label, "Consegna");
        assert_eq!(
            track_of(&[step("checks", "skipped")], false)[0].label,
            "Controlli, disattivato"
        );
        assert_eq!(track[1].label, "Costruzione, in revisione");
    }

    #[test]
    fn a_lane_the_engine_gives_no_steps_has_no_track() {
        assert!(track_of(&[], true).is_empty());
    }
}
