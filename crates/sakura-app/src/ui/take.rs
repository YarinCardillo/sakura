//! "Take" in the task header, as the take rule says: a plain press on the primary face (brushed silver on
//! dark, graphite on light) when everything is verified on HEAD; a hold ("Take without checks") when it is
//! not, with the reason under the pointer on the button and in "Verifiche"; and nothing at all when there is
//! nothing to take. The pending word shows at once, the outcome when `box` answers.

use super::chrome::{keep_click, steady_label};
use super::desk::Desk;
use super::hold::HoldButton;
use crate::data::actions::{ActionKey, Actions};
use crate::l10n::{say, tr, Key};
use crate::theme::{self, ActionFace, Palette};
use gpui::{
    div, linear_color_stop, linear_gradient, AnyElement, Context, Div, Entity,
    InteractiveElement as _, IntoElement, ParentElement, Stateful, StatefulInteractiveElement as _,
    Styled,
};
use gpui_component::tooltip::Tooltip;
use sakura_core::display;
use sakura_core::model::{self, TakeRule};
use sakura_core::models::Task;

/// The plain take's word, and its word while `box` takes.
const TAKE: (Key, Key) = (Key("take.action"), Key("take.taking"));

/// "Take" for `task`, or nothing when there is nothing to take; `hold` is the task's hold button.
pub fn button(
    task: &Task,
    actions: &Actions,
    hold: &Entity<HoldButton>,
    palette: &Palette,
    cx: &Context<Desk>,
) -> Option<AnyElement> {
    let is_pending = actions.is_running(&ActionKey::take(&task.name));
    match model::take_rule(task) {
        TakeRule::Plain => Some(primary(task, is_pending, palette, cx)),
        TakeRule::Hold { reason } => Some(held(hold, whole_reason(task, &say(&reason)))),
        TakeRule::Disabled { .. } => None,
    }
}

/// The reason a take needs a hold, with the files it is about on the lines after it.
pub fn whole_reason(task: &Task, reason: &str) -> String {
    let files = model::take_details(task);
    std::iter::once(reason.to_string())
        .chain(files.iter().map(|file| display::label(file)))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The label of the hold button: its pending word while `box` takes.
pub fn hold_label(is_pending: bool) -> &'static str {
    if is_pending {
        tr(TAKE.1)
    } else {
        tr(Key("take.unverified"))
    }
}

/// The hold button, with the reason it is a hold under the pointer.
fn held(hold: &Entity<HoldButton>, reason: String) -> AnyElement {
    div()
        .id("take-hold")
        .debug_selector(|| "take-hold".into())
        .tooltip(move |window, cx| Tooltip::new(reason.clone()).build(window, cx))
        .child(hold.clone())
        .into_any_element()
}

/// The one filled action ("Take").
fn primary(task: &Task, is_pending: bool, palette: &Palette, cx: &Context<Desk>) -> AnyElement {
    let word = if is_pending { TAKE.1 } else { TAKE.0 };
    let button = action_face(palette).child(steady_label(tr(word), &[tr(TAKE.0), tr(TAKE.1)]));
    let name = task.name.clone();
    keep_click(button)
        .active(|style| style.opacity(theme::TAKE_PRESSED_OPACITY))
        .on_click(cx.listener(move |desk, _, _, cx| desk.take(name.clone(), cx)))
        .into_any_element()
}

/// The face of the primary action: brushed silver with its relief on dark, a graphite fill on light.
fn action_face(palette: &Palette) -> Stateful<Div> {
    let face = div()
        .id("take")
        .debug_selector(|| "take".into())
        .h(theme::CONTROL_HEIGHT)
        .px(theme::CONTROL_PADDING_X)
        .flex()
        .items_center()
        .rounded(theme::RADIUS_CONTROL)
        .shadow(theme::action_relief(palette.appearance))
        .text_color(palette.action_ink)
        .text_size(theme::TEXT_SMALL)
        .font_weight(theme::WEIGHT_MEDIUM);
    match palette.action_face {
        ActionFace::Silver { top, bottom } => face.bg(linear_gradient(
            theme::SHEEN_ANGLE,
            linear_color_stop(top, 0.),
            linear_color_stop(bottom, 1.),
        )),
        ActionFace::Graphite(fill) => face.bg(fill),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sakura_core::models::BoxStatus;

    /// The fixture's verified task, whose test result now says the commit changes `file`.
    fn changing(file: &str) -> Task {
        let status: BoxStatus = serde_json::from_str(include_str!(
            "../../../sakura-core/tests/fixtures/status.json"
        ))
        .expect("the fixture parses");
        let mut task = status
            .tasks
            .into_iter()
            .find(|task| task.name == "sample-accent")
            .expect("the verified task");
        let result = task.results.get_mut("test").expect("a test result");
        result.changes_checks = Some(true);
        result.checks_touched = Some(vec![file.to_string()]);
        task
    }

    #[test]
    fn the_files_under_a_reason_are_one_line_each_whatever_their_names_hold() {
        let task = changing("vitest.config.ts\nNessun controllo cambiato\u{202e}");
        assert_eq!(
            whole_reason(&task, "Motivo"),
            "Motivo\nvitest.config.ts\u{fffd}Nessun controllo cambiato\u{fffd}"
        );
    }
}
