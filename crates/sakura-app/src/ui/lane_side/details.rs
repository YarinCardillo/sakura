//! Details: what the engine says of the lane (its project, branch, preset and pace), the steps it runs with
//! the model and, when the person has more than one account, the account of each, and the asks that wait.
//! A row the engine gives no fact for is not shown.

use super::{LaneSide, LaneSideEvent};
use crate::l10n::{tr, Key};
use crate::theme::{self, lane_side as sizes, Palette};
use crate::ui::answers::{lane_key, step_view, Step};
use crate::ui::chrome::{mono, secondary_variant, text};
use crate::ui::inbox::model::kind_words;
use crate::ui::lane_paces::{pace_words, pending_note, LANE_PACES};
use crate::ui::lanes::capitalised;
use crate::ui::step_track::{pipeline_words, step_words};
use crate::ui::tip::{self, Words};
use gpui::{
    div, AnyElement, ClickEvent, Context, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use sakura_core::engine::presets::PresetChoice;
use sakura_core::engine::snapshot::{Item, LaneReview, LaneStep, PipelineStep, Snapshot, Wait};
use std::collections::BTreeMap;

impl LaneSide {
    /// The Details view of `item`.
    pub(super) fn details(
        &mut self,
        item: &Item,
        snapshot: Option<&Snapshot>,
        palette: &Palette,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut rows = vec![prop(
            Key("lane_side.project"),
            project_value(item, palette),
            "detail-project",
            palette,
        )];
        if let Some(name) = crate::ui::lanes::branch(item) {
            let whole = SharedString::from(name.to_string());
            let branch = mono()
                .id("detail-branch-text")
                .debug_selector(|| "detail-branch-text".into())
                .min_w_0()
                .max_w_full()
                .flex()
                .flex_wrap()
                .text_size(theme::TEXT_LABEL)
                .children(
                    name.split_inclusive(['/', '-'])
                        .map(|part| div().max_w_full().child(part.to_string())),
                );
            let branch = tip::bind(branch, "detail-branch-text", Words::plain(whole), cx);
            rows.push(prop(
                Key("lane_side.branch"),
                branch.into_any_element(),
                "detail-branch",
                palette,
            ));
        }
        if let Some(preset) = &item.preset {
            let shown = self.queued(item, ACTION_PRESET, cx).unwrap_or(preset);
            let value = self.preset_value(item, shown, snapshot, cx);
            let row = prop(Key("lane_side.preset"), value, "detail-preset", palette);
            rows.push(self.answered(row, (item, ACTION_PRESET), palette, cx));
        }
        if let Some(pace) = &item.pace {
            let value = self.pace_value(item, pace, palette, cx);
            let row = prop(Key("lane_side.pace"), value, "detail-pace", palette);
            rows.push(self.answered(row, (item, ACTION_PACE), palette, cx));
        }
        let mut body = div()
            .id("lane-side-details")
            .size_full()
            .track_scroll(&self.details_scroll)
            .overflow_y_scroll()
            .px(sizes::BODY_PADDING_X)
            .pb(sizes::BODY_PADDING_BOTTOM)
            .flex()
            .flex_col()
            .child(div().flex().flex_col().children(rows));
        if let Some(delivered) = crate::ui::lane_page::delivery_view(item, palette, cx) {
            body = body.child(div().mt(sizes::SUBHEAD_GAP_ABOVE).child(delivered));
        }
        if let Some(steps) = steps_view(item, snapshot, &self.pipeline, palette) {
            body = body
                .child(subhead(Key("lane_side.steps"), palette))
                .child(steps);
        }
        let waits = snapshot
            .map(|snapshot| asks_of(item, snapshot))
            .unwrap_or_default();
        if !waits.is_empty() {
            body = body
                .child(subhead(Key("lane_side.asks"), palette))
                .children(
                    waits
                        .into_iter()
                        .map(|wait| self.ask_row(wait, palette, cx)),
                );
        }
        body.into_any_element()
    }

    /// The lane's preset: a menu of presets when the engine lets the person switch it, else the word.
    fn preset_value(
        &self,
        item: &Item,
        preset: &str,
        snapshot: Option<&Snapshot>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let name = match item.preset.as_deref() == Some(preset) {
            true => item.preset_name().unwrap_or_default(),
            false => preset_words(snapshot, preset),
        };
        if !item
            .actions
            .iter()
            .any(|action| action.key == ACTION_PRESET)
        {
            return div().child(name).into_any_element();
        }
        let side = cx.weak_entity();
        let current = preset.to_string();
        let choices = presets_offered(snapshot, preset);
        preset_chip(name, cx)
            .dropdown_menu(move |menu, _, _| {
                let mut menu = menu.min_w(sizes::PRESET_MENU_WIDTH);
                for choice in &choices {
                    menu = menu.item(preset_item(choice, &current, &side));
                }
                menu
            })
            .into_any_element()
    }

    /// Pace: a menu of the five paces that sends the lane's own, or its word when the engine offers no control.
    fn pace_value(
        &self,
        item: &Item,
        pace: &str,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if !item.actions.iter().any(|action| action.key == ACTION_PACE) {
            return text(theme::TEXT_SMALL, palette.text)
                .child(pace_words(pace))
                .into_any_element();
        }
        let side = cx.weak_entity();
        let current = pace.to_string();
        crate::ui::kit::menu_chip("lane-pace", pace_words(pace), None, cx)
            .debug_selector(|| "lane-pace".into())
            .dropdown_menu(move |menu, _, _| {
                LANE_PACES.iter().fold(menu, |menu, (level, word)| {
                    let (side, level) = (side.clone(), level.to_string());
                    let item = PopupMenuItem::new(tr(*word))
                        .checked(level == current)
                        .on_click(move |_: &ClickEvent, _, cx| {
                            let inputs = BTreeMap::from([("level".to_string(), level.clone())]);
                            drop(side.update(cx, |side, cx| side.send(ACTION_PACE, inputs, cx)));
                        });
                    menu.item(item)
                })
            })
            .into_any_element()
    }

    /// One ask that waits on the person, with the way to answer it in the Inbox.
    fn ask_row(&self, wait: &Wait, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let button = Button::new(SharedString::from(format!("detail-ask-{}", wait.id)))
            .debug_selector(|| "detail-ask-open".into())
            .custom(secondary_variant(cx))
            .bg(crate::theme::Theme::palette(cx).fill)
            .child(tr(Key("lane_side.answer_in_inbox")))
            .on_click(cx.listener(|_, _: &ClickEvent, _, cx| cx.emit(LaneSideEvent::OpenInbox)));
        div()
            .px(sizes::ASK_PADDING_X)
            .py(sizes::ASK_PADDING_Y)
            .mb(theme::GAP)
            .rounded(sizes::ASK_RADIUS)
            .bg(palette.raised)
            .flex()
            .flex_col()
            .gap(theme::GAP)
            .child(
                text(theme::TEXT_SMALL, palette.text)
                    .child(crate::ui::inbox::model::ask_title_words(wait)),
            )
            .child(
                text(theme::TEXT_LABEL, palette.text_tertiary)
                    .child(kind_words(wait.original_kind())),
            )
            .child(div().flex().child(button))
            .into_any_element()
    }

    /// Switches the lane to `preset`: the engine applies it from the next step, and the row says so.
    pub fn switch_preset(&mut self, preset: &str, cx: &mut Context<Self>) {
        let inputs = BTreeMap::from([("preset".to_string(), preset.to_string())]);
        self.send(ACTION_PRESET, inputs, cx);
    }

    /// `row` with what the engine said to the last change of `action` under it: that it is sending, that it
    /// applies at the next step, or why it was refused.
    fn answered(
        &self,
        row: AnyElement,
        (item, action): (&Item, &'static str),
        palette: &Palette,
        cx: &gpui::App,
    ) -> AnyElement {
        let key = lane_key(&item.id, action);
        let step = self.answers.read(cx).step(&key);
        let line = match step {
            Some(Step::Told {
                is_problem: true, ..
            }) => step.map(|step| step_view(&self.answers, &key, step, palette, cx)),
            _ if action == ACTION_PACE && item.pace_pending => Some(
                div()
                    .pb(theme::GAP_SMALL)
                    .child(pending_note("detail-pace-queued".into(), palette))
                    .into_any_element(),
            ),
            Some(Step::Told {
                is_problem: false, ..
            }) if action == ACTION_PACE => None,
            Some(Step::Told {
                is_problem: false, ..
            }) => self.queued(item, action, cx).map(|_| {
                let selector = format!("detail-{}-queued", action_name(action));
                text(theme::TEXT_LABEL, palette.text_tertiary)
                    .debug_selector(move || selector)
                    .pb(theme::GAP_SMALL)
                    .child(tr(Key("lane_side.applies_next")))
                    .into_any_element()
            }),
            Some(step) => Some(step_view(&self.answers, &key, step, palette, cx)),
            None => None,
        };
        div()
            .flex()
            .flex_col()
            .child(row)
            .children(line)
            .into_any_element()
    }
}

/// The name a row's selectors carry for its action.
fn action_name(action: &str) -> &'static str {
    match action {
        ACTION_PRESET => "preset",
        _ => "pace",
    }
}

/// The chip that opens the menu of presets: the preset's name and the chevron after it (the kit's menu chip).
fn preset_chip(name: String, cx: &gpui::App) -> Button {
    crate::ui::kit::menu_chip("lane-preset", name, None, cx)
}

/// The id of the step that runs the project's tests.
const CHECKS_STEP: &str = "checks";

/// The engine's action that switches a lane's preset, and the one that sets its lane pace.
pub(super) const ACTION_PRESET: &str = "S";
pub(super) const ACTION_PACE: &str = "q";

/// Whether the window keeps the person's choice of the action `key` until the engine applies it: the preset.
/// A pace is never kept here: the engine says the one chosen at once, and whether it waits for the next step.
pub(super) fn is_queued_kind(key: &str) -> bool {
    key == ACTION_PRESET
}

fn preset_item(
    choice: &PresetChoice,
    current: &str,
    side: &gpui::WeakEntity<LaneSide>,
) -> PopupMenuItem {
    let (side, picked) = (side.clone(), choice.name.clone());
    PopupMenuItem::new(choice.words())
        .checked(choice.name == current)
        .on_click(move |_: &ClickEvent, _, cx| {
            drop(side.update(cx, |side, cx| side.switch_preset(&picked, cx)));
        })
}

/// The presets the menu lists: the ones the engine lists, the preset the lane runs as and every preset another
/// lane runs as (one the person saved shows while a lane runs as it).
fn presets_offered(snapshot: Option<&Snapshot>, current: &str) -> Vec<PresetChoice> {
    let in_use = snapshot.into_iter().flat_map(|snapshot| &snapshot.items);
    let mut names: Vec<String> = snapshot
        .map(crate::ui::lanes::presets_to_pick)
        .unwrap_or_default()
        .into_iter()
        .map(|preset| preset.name)
        .collect();
    for name in in_use
        .filter_map(|item| item.preset.clone())
        .chain([current.to_string()])
    {
        if !names.contains(&name) {
            names.push(name);
        }
    }
    let named = |name: String| {
        let words = preset_words(snapshot, &name);
        PresetChoice::named(&name, Some(&words))
    };
    names.into_iter().map(named).collect()
}

/// The preset `name` in words: the engine's label when the snapshot has one, else the name with a capital.
fn preset_words(snapshot: Option<&Snapshot>, name: &str) -> String {
    match snapshot {
        Some(snapshot) => crate::ui::lanes::preset_words(snapshot, name),
        None => capitalised(name),
    }
}

/// "Claude Opus": a model as the engine writes it (`claude:opus`), the provider and the name in words.
pub(super) fn model_words(id: &str) -> String {
    let Some((provider, name)) = id.split_once(':') else {
        return id.to_string();
    };
    let is_plain = name.chars().all(|c| c.is_ascii_alphabetic());
    let name = if is_plain {
        capitalised(name)
    } else {
        name.to_string()
    };
    format!("{} {name}", capitalised(provider))
}

fn project_value(item: &Item, palette: &Palette) -> AnyElement {
    text(theme::TEXT_SMALL, palette.text)
        .child(item.project().to_string())
        .into_any_element()
}

/// A property of the lane: its name at the left and its value beside it.
fn prop(name: Key, value: AnyElement, selector: &'static str, palette: &Palette) -> AnyElement {
    div()
        .debug_selector(move || selector.into())
        .min_h(sizes::PROP_HEIGHT)
        .flex()
        .items_center()
        .child(
            text(theme::TEXT_SMALL, palette.text_tertiary)
                .w(sizes::PROP_NAME_WIDTH)
                .flex_none()
                .child(tr(name)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .text_size(theme::TEXT_SMALL)
                .child(value),
        )
        .into_any_element()
}

fn subhead(words: Key, palette: &Palette) -> AnyElement {
    text(theme::TEXT_LABEL, palette.text_tertiary)
        .mt(sizes::SUBHEAD_GAP_ABOVE)
        .mb(sizes::SUBHEAD_GAP_BELOW)
        .child(tr(words))
        .into_any_element()
}

fn steps_view(
    item: &Item,
    snapshot: Option<&Snapshot>,
    pipeline: &[PipelineStep],
    palette: &Palette,
) -> Option<AnyElement> {
    let has_ended = matches!(
        item.agent_state.as_str(),
        "stopped" | "delivered" | "cleaned"
    );
    let rows: Vec<_> = if pipeline.is_empty() {
        item.steps
            .iter()
            .map(|step| step_row(step, has_ended, snapshot, None, palette))
            .chain(
                item.reviews
                    .iter()
                    .filter(|review| review.is_on)
                    .map(|review| review_row(review, has_ended, snapshot, palette)),
            )
            .collect()
    } else {
        pipeline
            .iter()
            .filter_map(|step| pipeline_row(item, step, has_ended, snapshot, palette))
            .collect()
    };
    (!rows.is_empty()).then(|| {
        div()
            .debug_selector(|| "detail-steps".into())
            .flex()
            .flex_col()
            .children(rows)
            .into_any_element()
    })
}

fn pipeline_row(
    item: &Item,
    listed: &PipelineStep,
    has_ended: bool,
    snapshot: Option<&Snapshot>,
    palette: &Palette,
) -> Option<AnyElement> {
    let state = detail_state(&listed.state);
    if let Some(review) = item.reviews.iter().find(|review| review.id == listed.id) {
        let mut review = review.clone();
        state.clone_into(&mut review.state);
        return listed
            .is_on
            .then(|| review_row(&review, has_ended, snapshot, palette));
    }
    let step = item.steps.iter().find(|step| step.id == listed.id);
    if !listed.is_on && step.is_none() {
        return None;
    }
    let mut step = step.cloned().unwrap_or_else(|| LaneStep {
        id: listed.id.clone(),
        name: pipeline_words(listed).to_string(),
        state: String::new(),
        model: None,
        account: None,
        account_label: None,
    });
    state.clone_into(&mut step.state);
    let note = (!listed.note.is_empty()).then(|| listed.note.clone());
    Some(step_row(&step, has_ended, snapshot, note, palette))
}

fn step_row(
    step: &LaneStep,
    has_ended: bool,
    snapshot: Option<&Snapshot>,
    note: Option<String>,
    palette: &Palette,
) -> AnyElement {
    let shown = row(
        &format!("step-{}", step.id),
        step_words(step),
        (&step_model(step), step_account(step, snapshot)),
        &state_shown(&step.state, has_ended),
        palette,
    );
    let Some(note) = note.filter(|_| step.state == "waiting") else {
        return shown;
    };
    let selector = format!("step-note-{}", step.id);
    div()
        .child(shown)
        .child(
            text(theme::TEXT_SMALL, palette.text_secondary)
                .debug_selector(move || selector)
                .pb(theme::GAP_TIGHT)
                .child(note),
        )
        .into_any_element()
}

/// What the Checks step runs, as the Engine page draws it: the project's tests, whatever role the engine
/// lists for the step. Any other step runs the model the engine names.
pub(super) fn step_model(step: &LaneStep) -> Option<String> {
    match step.id.as_str() {
        CHECKS_STEP => Some(tr(Key("engine_page.run.checks")).to_string()),
        _ => step.model.clone(),
    }
}

/// The account a step runs on, when the engine names one; the project's tests run on none.
pub(super) fn step_account(step: &LaneStep, snapshot: Option<&Snapshot>) -> Option<String> {
    let id = step.account.as_deref().filter(|_| step.id != CHECKS_STEP)?;
    Some(
        step.account_label
            .clone()
            .unwrap_or_else(|| account_name(snapshot, id)),
    )
}

fn review_row(
    review: &LaneReview,
    has_ended: bool,
    snapshot: Option<&Snapshot>,
    palette: &Palette,
) -> AnyElement {
    let key = match review.id.as_str() {
        "plan_review" => Key("lane_side.review.plan_review"),
        "task_review" => Key("lane_side.review.task_review"),
        _ => Key("lane_side.review.final_review"),
    };
    let name = SharedString::from(tr(key));
    let account = review.account.as_deref().map(|id| {
        let named = review.account_label.clone();
        named.unwrap_or_else(|| account_name(snapshot, id))
    });
    row(
        &format!("review-{}", review.id),
        name,
        (&review.model, account),
        &state_shown(&review.state, has_ended),
        palette,
    )
}

/// What the account `id` is called: the label the person gave it, never its id when the snapshot lists a label.
pub(super) fn account_name(snapshot: Option<&Snapshot>, id: &str) -> String {
    let listed = snapshot.and_then(|snapshot| snapshot.accounts.iter().find(|a| a.id == id));
    listed.map_or_else(|| id.to_string(), |account| account.name().to_string())
}

/// One row of Steps, in four columns that each keep their own room: the name, the model, the account when
/// there is one (the one that gives way, with an ellipsis), and how far it is. `id` is `<kind>-<name>`
/// (`step-plan`, `review-task_review`); the selectors are `<kind>-row-<name>` and the same with `model`,
/// `account` and `state` for its columns.
fn row(
    id: &str,
    name: SharedString,
    (model, account): (&Option<String>, Option<String>),
    state: &str,
    palette: &Palette,
) -> AnyElement {
    let (kind, which) = id.split_once('-').unwrap_or((id, ""));
    let selector = format!("{kind}-row-{which}");
    let column = |column: &str| format!("{kind}-{column}-{which}");
    let (model_id, account_id, state_id) = (column("model"), column("account"), column("state"));
    // A step that is off runs nothing, so it names no model.
    let (model, account) = match state {
        "skipped" => (&None, None),
        _ => (model, account),
    };
    // The account wraps onto a second line instead of being cut short.
    let account = account.map(|account| {
        text(theme::TEXT_SMALL, palette.text_secondary)
            .debug_selector(move || account_id)
            .min_w_0()
            .max_w_full()
            .overflow_hidden()
            .child(account)
    });
    div()
        .debug_selector(move || selector)
        .min_h(sizes::PROP_HEIGHT)
        .py(theme::GAP_TIGHT)
        .flex()
        .items_center()
        .child(
            text(theme::TEXT_SMALL, palette.text_tertiary)
                .w(sizes::PROP_NAME_WIDTH)
                .flex_none()
                .child(name),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_wrap()
                .gap_x(theme::GAP)
                .text_size(theme::TEXT_SMALL)
                .child(
                    div()
                        .debug_selector(move || model_id)
                        .flex_none()
                        .whitespace_nowrap()
                        .child(model.as_deref().map(model_words).unwrap_or_default()),
                )
                .children(account),
        )
        .child(
            text(theme::TEXT_LABEL, palette.text_tertiary)
                .debug_selector(move || state_id)
                .w(sizes::STATE_WIDTH)
                .flex_none()
                .flex()
                .justify_end()
                .child(state_words(state)),
        )
        .into_any_element()
}

/// The state a row shows: a lane that ended is not "now" at any step.
fn state_shown(state: &str, has_ended: bool) -> String {
    match (state, has_ended) {
        ("now", true) => String::new(),
        _ => state.to_string(),
    }
}

fn state_words(state: &str) -> SharedString {
    let key = match state {
        "done" => Key("lane_side.state.done"),
        "now" => Key("lane_side.state.now"),
        "skipped" => Key("lane_side.state.skipped"),
        "next" => Key("lane_steps.state.next"),
        // A step the lane waits in needs the person.
        "waiting" => Key("lane_steps.state.waiting"),
        _ => return SharedString::default(),
    };
    SharedString::from(tr(key))
}

/// The asks of `item` that wait for the person.
fn asks_of<'a>(item: &Item, snapshot: &'a Snapshot) -> Vec<&'a Wait> {
    let now = crate::ui::unix_now();
    snapshot
        .waits_for(item)
        .filter(|wait| !wait.is_snoozed_at(now))
        .collect()
}

/// The state a step of the pipeline says in the words of Details (`done`, `now`, `next`, `skipped`).
fn detail_state(pipeline_state: &str) -> &'static str {
    match pipeline_state {
        "done" => "done",
        "running" => "now",
        "waiting" => "waiting",
        "off" => "skipped",
        _ => "next",
    }
}
