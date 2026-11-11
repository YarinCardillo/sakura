//! "Verifiche", a view of the right pane: one row per check, rows not cards, laid out for a narrow pane. A
//! row is the check's name with its action at the right ("Run", or "Cancel" while it is queued or runs
//! and `box` can cancel it), then its state as a mark and its words, then the facts on a quiet line (the
//! short sha the request or the result is for, the duration, the count of tests; a request for a commit
//! that is not HEAD says so). A row opens to the tail of its log, read when opened and shown from its end.

use super::breath::{Breath, Owner};
use super::chrome::{control, failure_sentence, mono, tabular, text};
use super::desk::Desk;
use super::glyph::{glyph, tone_colour, Shape};
use crate::data::actions::{ActionKey, Actions, LogTail};
use crate::data::facts::Facts;
use crate::l10n::{say, tr, tr_with, Key};
use crate::theme::{self, Palette};
use gpui::{
    div, px, AnyElement, Context, Div, InteractiveElement as _, IntoElement, ParentElement,
    ScrollHandle, SharedString, Stateful, StatefulInteractiveElement as _, Styled,
};
use gpui_component::button::{Button, ButtonCustomVariant, ButtonVariants as _};
use gpui_component::Disableable as _;
use sakura_core::display;
use sakura_core::model::{self, VerifyRow, VerifyState};
use sakura_core::models::Task;
use std::time::Instant;

/// The label over the files a green result is held back for, above its log.
const RESERVE_FILES: Key = Key("verify.files_touched");
/// Every word the button of a row can show: it keeps the room of the widest, so the columns before it
/// stay where they are when the word changes.
const ROW_WORDS: [Key; 4] = [
    Key("verify.run"),
    Key("verify.queueing"),
    Key("common.cancel"),
    Key("verify.cancelling"),
];
/// What a running or queued request for a commit other than HEAD says.
const NOT_HEAD: Key = Key("verify.not_head");
const RAN_ON_MAC: Key = Key("verify.ran_on_mac");

/// What the view reads, gathered by the desk.
pub struct VerifyInputs<'a> {
    pub task: &'a Task,
    pub facts: &'a Facts,
    pub actions: &'a Actions,
    pub open_verb: Option<&'a str>,
    pub log_scroll: &'a ScrollHandle,
}

/// The Verifiche view for one task.
pub fn view(inputs: VerifyInputs, palette: &Palette, cx: &Context<Desk>) -> AnyElement {
    let task = inputs.task;
    let rows = model::verify_rows(task, inputs.facts.runner());
    let children = rows.into_iter().flat_map(|row| {
        let is_open = inputs.open_verb == Some(row.verb.as_str());
        let log = is_open.then(|| log_panel(&inputs, &row.verb, palette));
        std::iter::once(verify_row(&inputs, row, palette, cx)).chain(log)
    });
    div()
        .id("verify-list")
        .size_full()
        .overflow_y_scroll()
        .pl(theme::SIDE_PANE_INSET - theme::ROW_PADDING_X)
        .pr(theme::CONTENT_PADDING - theme::ROW_PADDING_X)
        .py(theme::GAP)
        .flex()
        .flex_col()
        .children(summary(task, palette))
        .children(children)
        .into_any_element()
}

/// The first line of the view: the task's branch and the base it starts from, and when taking needs a
/// hold, why (the header says neither).
fn summary(task: &Task, palette: &Palette) -> Option<AnyElement> {
    let from = super::page::where_from(task);
    let reason = match model::take_rule(task) {
        model::TakeRule::Hold { reason } => Some(reason),
        model::TakeRule::Plain | model::TakeRule::Disabled { .. } => None,
    };
    if from.is_none() && reason.is_none() {
        return None;
    }
    let reason = reason.map(|reason| {
        text(theme::TEXT_LABEL, palette.text_secondary)
            .debug_selector(|| "verify-summary-reason".into())
            .child(say(&reason))
    });
    let line = div()
        .debug_selector(|| "verify-summary".into())
        .px(theme::ROW_PADDING_X)
        .pb(theme::GAP)
        .flex()
        .flex_col()
        .gap(theme::GAP_TIGHT)
        // A sentence of the interface about two branches: in its font, as the header writes a branch.
        .children(from.map(|from| text(theme::TEXT_LABEL, palette.text_tertiary).child(from)))
        .children(reason);
    Some(line.into_any_element())
}

/// The sha a row shows and whether it is HEAD. A request `box` accepted and no runner state shows yet was
/// queued for HEAD (`box verify` asks for HEAD), so it shows HEAD, never the last result's sha.
pub fn shown_sha(
    row: &VerifyRow,
    shown: VerifyState,
    head: Option<&str>,
) -> (Option<String>, bool) {
    let is_local_request =
        matches!(row.state, VerifyState::Result(_)) && !matches!(shown, VerifyState::Result(_));
    match head {
        Some(head) if is_local_request => (Some(model::short_sha(head)), true),
        _ => (row.sha.clone(), row.is_head),
    }
}

/// A request `box` accepted shows as queued until a runner state read after it arrives.
pub fn shown_state(
    state: VerifyState,
    queued_at: Option<Instant>,
    status_started_at: Option<Instant>,
) -> VerifyState {
    let is_newer = |queued: Instant| status_started_at.is_none_or(|read| queued > read);
    match state {
        VerifyState::Result(_) if queued_at.is_some_and(is_newer) => {
            VerifyState::InCoda { position: 0 }
        }
        other => other,
    }
}

/// One check, laid out for a pane that can be narrow: its name with its action at the right, under it the
/// state as a mark and its words (a sentence wraps, whole), then the facts of the result on one quiet
/// line, then what the commit did to existing tests and a failure of the action, each on a line of its
/// own. Nothing is ever beside the action but the name, so no note moves or crowds it.
fn verify_row(
    inputs: &VerifyInputs,
    row: VerifyRow,
    palette: &Palette,
    cx: &Context<Desk>,
) -> AnyElement {
    let task = inputs.task.name.clone();
    let queued_at = inputs.actions.queued_at(&task, &row.verb);
    let state = shown_state(row.state, queued_at, inputs.facts.status_started_at());
    let key = ActionKey::Verify {
        task,
        verb: row.verb.clone(),
    };
    let cancel_key = ActionKey::cancel(&inputs.task.name, &row.verb);
    let failed = [&key, &cancel_key]
        .into_iter()
        .find(|key| inputs.actions.error(key).is_some());
    let error = failed.and_then(|key| {
        let sentence = inputs.actions.error(key)?;
        let details = inputs.actions.error_details(key);
        Some(error_line(&row.verb, (&say(sentence), details), palette))
    });
    let name = div()
        .h(theme::VERIFY_ROW_HEIGHT)
        .flex_none()
        .flex()
        .items_center()
        .gap(theme::GAP_WIDE)
        .child(verb_cell(&row.verb, palette))
        .child(div().flex_1().min_w_0())
        .child(
            div()
                .flex_none()
                .child(row_button(inputs, &row.verb, state, cx)),
        );
    row_frame(&row.verb, palette, cx)
        .child(name)
        .child(state_line(state, palette, Breath::of(Owner::Page, cx)))
        .children(facts_line(inputs, &row, state, palette))
        .children(touched_note(&row, palette))
        .children(place_note(&row, palette))
        .children(error)
        .into_any_element()
}

/// A row that opens and closes its log on a click, brightening on hover only.
fn row_frame(verb: &str, palette: &Palette, cx: &Context<Desk>) -> Stateful<Div> {
    let (hover, toggle_verb) = (palette.hover, verb.to_string());
    let selector = format!("verify-{verb}");
    div()
        .id(SharedString::from(format!("verify-{verb}")))
        .debug_selector(move || selector)
        .min_h(theme::VERIFY_ROW_HEIGHT)
        .flex_none()
        .px(theme::ROW_PADDING_X)
        .pb(theme::GAP)
        .flex()
        .flex_col()
        .rounded(theme::RADIUS_CONTROL)
        .hover(move |style| style.bg(hover))
        .on_click(cx.listener(move |desk, _, _, cx| desk.toggle_log(toggle_verb.clone(), cx)))
}

fn verb_cell(verb: &str, palette: &Palette) -> impl IntoElement {
    text(theme::TEXT_BODY, palette.text)
        .min_w_0()
        .truncate()
        .font_weight(theme::WEIGHT_MEDIUM)
        .child(verb.to_string())
}

/// The state: its mark in its tone and its words beside it. A sentence ("Verde, ma il commit cambia i
/// controlli") takes as many lines as the pane's width asks, whole.
fn state_line(state: VerifyState, palette: &Palette, breath: Breath) -> impl IntoElement {
    text(theme::TEXT_SMALL, tone_colour(state.tone(), palette))
        .debug_selector(|| "verify-state".into())
        .line_height(px(theme::VERIFY_NOTE_LINE_HEIGHT))
        .flex()
        .items_start()
        .gap(theme::GAP_SMALL)
        .child(
            div()
                .debug_selector(|| "verify-glyph".into())
                .flex_none()
                .child(glyph(
                    Shape::of_verify(state),
                    state.tone(),
                    palette,
                    breath,
                )),
        )
        .child(div().min_w_0().child(tr(state.word())))
}

/// The facts of the result or of the request, quiet, on one line that wraps: the short sha, the duration
/// or the place in the queue, the count of tests, and that the commit is not HEAD when it is not.
fn facts_line(
    inputs: &VerifyInputs,
    row: &VerifyRow,
    state: VerifyState,
    palette: &Palette,
) -> Option<impl IntoElement> {
    let (sha, is_head) = shown_sha(row, state, inputs.task.head.as_deref());
    let is_request = !matches!(state, VerifyState::Result(_));
    let has_note = is_request && sha.is_some() && !is_head;
    let figure = match state {
        VerifyState::InCoda { position } if position > 0 => {
            let position = position.to_string();
            Some(tr_with(Key("verify.position"), &[("position", &position)]).to_string())
        }
        _ => row.seconds.map(model::duration),
    };
    if sha.is_none() && figure.is_none() && row.tests.is_none() {
        return None;
    }
    let selector = format!("verify-tests-{}", row.verb);
    let count = row.tests.clone().map(|count| {
        div()
            .debug_selector(move || selector)
            .whitespace_nowrap()
            .child(say(&count))
    });
    Some(
        text(theme::TEXT_LABEL, palette.text_tertiary)
            .pl(theme::ICON_SIZE + theme::GAP_SMALL)
            .pt(theme::GAP_TIGHT)
            .flex()
            .flex_wrap()
            .gap_x(theme::GAP_WIDE)
            .font_features(tabular())
            .children(sha.map(|sha| mono().child(sha)))
            .children(figure)
            .children(count)
            .children(has_note.then_some(tr(NOT_HEAD))),
    )
}

/// What the commit did to the tests that existed at the base, when it touched any: a line of its own that
/// wraps, starting under the state's words.
fn touched_note(row: &VerifyRow, palette: &Palette) -> Option<impl IntoElement> {
    let sentence = row.tests_touched.clone()?;
    Some(
        text(theme::TEXT_LABEL, palette.text_tertiary)
            .debug_selector(|| "verify-tests-touched".into())
            .pl(theme::ICON_SIZE + theme::GAP_SMALL)
            .pt(theme::GAP_TIGHT)
            .line_height(px(theme::VERIFY_NOTE_LINE_HEIGHT))
            .font_features(tabular())
            .child(say(&sentence)),
    )
}

/// Where the last result ran, when that was the task's own folder on the Mac: one plain sentence under
/// the result, since such a check is not isolated from the agent.
fn place_note(row: &VerifyRow, palette: &Palette) -> Option<impl IntoElement> {
    row.ran_on_mac.then(|| {
        text(theme::TEXT_LABEL, palette.text_tertiary)
            .debug_selector(|| "verify-ran-on-mac".into())
            .pl(theme::ICON_SIZE + theme::GAP_SMALL)
            .pt(theme::GAP_TIGHT)
            .line_height(px(theme::VERIFY_NOTE_LINE_HEIGHT))
            .child(tr(RAN_ON_MAC))
    })
}

/// Why the last press on the row's action failed, in Sakura's words, on a line of its own; what `box` wrote
/// is under the pointer.
fn error_line(
    verb: &str,
    (sentence, details): (&str, Option<&str>),
    palette: &Palette,
) -> impl IntoElement {
    let id = SharedString::from(format!("verify-error-{verb}"));
    failure_sentence(id, sentence.to_string(), details)
        .text_size(theme::TEXT_LABEL)
        .text_color(palette.danger)
        .pl(theme::ICON_SIZE + theme::GAP_SMALL)
        .pt(theme::GAP_TIGHT)
}

/// "Cancel" where "Run" is while the check is queued or runs and `box` can cancel it, and while a
/// cancel is under way; else "Run".
fn row_button(
    inputs: &VerifyInputs,
    verb: &str,
    state: VerifyState,
    cx: &Context<Desk>,
) -> AnyElement {
    let is_waiting = matches!(state, VerifyState::InCorso | VerifyState::InCoda { .. });
    let key = ActionKey::cancel(&inputs.task.name, verb);
    let is_cancelling = inputs.actions.is_running(&key);
    if is_cancelling || (is_waiting && inputs.actions.can_cancel()) {
        cancel_button(&inputs.task.name, verb, is_cancelling, cx).into_any_element()
    } else {
        run_button(inputs, verb, state, cx).into_any_element()
    }
}

/// "Cancel": its pending word at once, until the runner's state read after it arrives.
fn cancel_button(
    task: &str,
    verb: &str,
    is_cancelling: bool,
    cx: &Context<Desk>,
) -> impl IntoElement {
    let (task, verb) = (task.to_string(), verb.to_string());
    let selector = format!("cancel-{verb}");
    let button = Button::new(SharedString::from(format!("cancel-{verb}")))
        .debug_selector(move || selector)
        .custom(secondary_variant(cx));
    let shown = if is_cancelling {
        tr(Key("verify.cancelling"))
    } else {
        tr(Key("common.cancel"))
    };
    control(button, shown, &ROW_WORDS.map(tr))
        .disabled(is_cancelling)
        .on_click(cx.listener(move |desk, _, _, cx| {
            cx.stop_propagation();
            desk.cancel_verify(task.clone(), verb.clone(), cx);
        }))
}

/// "Run": pending word at once, off while the runner already has the verb.
fn run_button(
    inputs: &VerifyInputs,
    verb: &str,
    state: VerifyState,
    cx: &Context<Desk>,
) -> impl IntoElement {
    let task = inputs.task.name.clone();
    let key = ActionKey::Verify {
        task: task.clone(),
        verb: verb.to_string(),
    };
    let is_pending = inputs.actions.is_running(&key);
    let is_waiting = matches!(state, VerifyState::InCorso | VerifyState::InCoda { .. });
    let verb = verb.to_string();
    let selector = format!("run-{verb}");
    let button = Button::new(SharedString::from(format!("run-{verb}")))
        .debug_selector(move || selector)
        .custom(secondary_variant(cx));
    let shown = if is_pending {
        tr(Key("verify.queueing"))
    } else {
        tr(Key("verify.run"))
    };
    control(button, shown, &ROW_WORDS.map(tr))
        .disabled(is_waiting || !inputs.task.readable)
        .on_click(cx.listener(move |desk, _, _, cx| {
            cx.stop_propagation();
            desk.run_verify(task.clone(), verb.clone(), cx);
        }))
}

/// The secondary button: a filled box a step above its surface, never an outline.
pub fn secondary_variant(cx: &gpui::App) -> ButtonCustomVariant {
    let palette = crate::theme::Theme::palette(cx);
    ButtonCustomVariant::new(cx)
        .color(palette.fill)
        .foreground(palette.text)
        .hover(palette.fill_hover)
        .active(palette.fill_hover)
}

fn log_panel(inputs: &VerifyInputs, verb: &str, palette: &Palette) -> AnyElement {
    let body: AnyElement = match inputs.actions.log(&inputs.task.name, verb) {
        None | Some(LogTail::Loading) => quiet(tr(Key("verify.log.reading")), palette),
        Some(LogTail::Missing) => quiet(tr(Key("verify.log.none")), palette),
        Some(LogTail::Failed(error)) => quiet_owned(say(error).to_string(), palette),
        Some(LogTail::Loaded(log)) => mono()
            .text_size(theme::TEXT_LABEL)
            .line_height(px(theme::LOG_LINE_HEIGHT))
            .text_color(palette.text_secondary)
            .whitespace_nowrap()
            .child(log.clone())
            .into_any_element(),
    };
    div()
        .id(SharedString::from(format!("log-{verb}")))
        .mx(theme::ROW_PADDING_X)
        .mb(theme::ROW_GAP)
        .max_h(theme::LOG_MAX_HEIGHT)
        .overflow_scroll()
        .track_scroll(inputs.log_scroll)
        .p(theme::GAP_WIDE)
        .rounded(theme::RADIUS_CONTROL)
        .bg(palette.inset)
        .children(reserve_files(inputs.task, verb, palette))
        .child(body)
        .into_any_element()
}

/// Above the log of a green result that is not plain green: the files it is held back for, in full.
fn reserve_files(task: &Task, verb: &str, palette: &Palette) -> Option<AnyElement> {
    let distrust = model::distrust(verb, task.results.get(verb)?)?;
    if distrust.files().is_empty() {
        return None;
    }
    let files = distrust
        .files()
        .iter()
        .map(|file| div().child(display::label(file)));
    let list = mono()
        .text_size(theme::TEXT_LABEL)
        .line_height(px(theme::LOG_LINE_HEIGHT))
        .text_color(palette.text_secondary)
        .children(files);
    let block = div()
        .debug_selector(|| "reserve-files".into())
        .pb(theme::GAP_WIDE)
        .flex()
        .flex_col()
        .gap(theme::GAP_TIGHT)
        .child(text(theme::TEXT_LABEL, palette.text_tertiary).child(tr(RESERVE_FILES)))
        .child(list);
    Some(block.into_any_element())
}

fn quiet(sentence: &'static str, palette: &Palette) -> AnyElement {
    text(theme::TEXT_SMALL, palette.text_tertiary)
        .child(sentence)
        .into_any_element()
}

fn quiet_owned(sentence: String, palette: &Palette) -> AnyElement {
    text(theme::TEXT_SMALL, palette.danger)
        .child(sentence)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use sakura_core::model::ResultView;
    use std::time::Duration;

    #[test]
    fn an_accepted_request_shows_queued_until_a_newer_runner_read() {
        let old = VerifyState::Result(ResultView::Passato);
        let read = Instant::now();
        let queued = read + Duration::from_secs(1);
        assert_eq!(
            shown_state(old, Some(queued), Some(read)),
            VerifyState::InCoda { position: 0 }
        );
        assert_eq!(
            shown_state(old, Some(queued), None),
            VerifyState::InCoda { position: 0 }
        );
        let newer_read = queued + Duration::from_secs(1);
        assert_eq!(shown_state(old, Some(queued), Some(newer_read)), old);
        assert_eq!(shown_state(old, None, Some(read)), old);
    }

    fn result_row(sha: &str, is_head: bool) -> VerifyRow {
        VerifyRow {
            verb: "test".into(),
            state: VerifyState::Result(ResultView::Vecchio),
            sha: Some(sha.into()),
            seconds: Some(38.0),
            is_head,
            tests: None,
            tests_touched: None,
            ran_on_mac: false,
        }
    }

    #[test]
    fn a_local_request_shows_head_not_the_last_results_sha() {
        let row = result_row("1111111", false);
        let head = "2222222222222222222222222222222222222222";
        let queued = VerifyState::InCoda { position: 0 };
        assert_eq!(
            shown_sha(&row, queued, Some(head)),
            (Some("2222222".into()), true)
        );
        assert_eq!(
            shown_sha(&row, row.state, Some(head)),
            (Some("1111111".into()), false)
        );
    }

    #[test]
    fn a_runner_request_keeps_its_own_sha() {
        let mut row = result_row("3333333", false);
        row.state = VerifyState::InCorso;
        let head = "2222222222222222222222222222222222222222";
        assert_eq!(
            shown_sha(&row, VerifyState::InCorso, Some(head)),
            (Some("3333333".into()), false)
        );
    }

    #[test]
    fn the_runner_state_wins_over_a_local_request() {
        let running = VerifyState::InCorso;
        let now = Instant::now();
        assert_eq!(shown_state(running, Some(now), None), running);
    }
}
