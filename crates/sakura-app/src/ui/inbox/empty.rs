//! What the page says when there is nothing to list: that nothing needs the person, how many lanes run, the
//! one thing to do on an engine with no project, and the engine's problem in words when it cannot be read.

use super::{InboxEvent, InboxPage, Shown};
use crate::data::engine_facts::Problem;
use crate::l10n::{say, tr, Key};
use crate::theme::{self, inbox as sizes, Palette};
use crate::ui::chrome::{control, secondary_variant, text};
use crate::ui::lanes;
use crate::ui::project_add::Place;
use gpui::Action as _;
use gpui::{
    div, AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _,
};
use gpui_component::button::{Button, ButtonVariants as _};
use sakura_core::engine::snapshot::Snapshot;
use sakura_core::said::Said;

impl InboxPage {
    /// The page with nothing to list. An engine with no project offers adding one.
    pub(super) fn empty_state(
        &mut self,
        snapshot: &Snapshot,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.shown == Shown::Open && self.filter.snoozed {
            return div()
                .size_full()
                .p(sizes::DETAIL_PADDING_X)
                .child(sentence(
                    tr(Key("inbox.snoozed.empty")),
                    palette,
                    "inbox-snoozed-empty",
                ))
                .into_any_element();
        }
        let adds = self.adds.clone().filter(|_| snapshot.projects.is_empty());
        let parts: Vec<AnyElement> = match (self.shown, adds) {
            (Shown::Open, Some(adds)) => {
                let pressed = adds.clone();
                let button = control(
                    Button::new("waiting-add-project")
                        .custom(secondary_variant(cx))
                        .bg(crate::theme::Theme::palette(cx).fill),
                    tr(Key("projects.add")),
                    &[tr(Key("projects.add"))],
                )
                .debug_selector(|| "waiting-add-project".into())
                .on_click(move |_, window, cx| {
                    pressed.update(cx, |adds, cx| adds.choose(Place::Waiting, window, cx))
                });
                let refusal = adds
                    .read(cx)
                    .refusal_in(Place::Waiting)
                    .cloned()
                    .map(|words| {
                        text(theme::TEXT_SMALL, palette.danger)
                            .debug_selector(|| "waiting-add-project-refusal".into())
                            .child(words)
                            .into_any_element()
                    });
                let account = self.account_button(snapshot, cx);
                let mut parts = empty_intro(palette);
                parts.push(
                    div()
                        .flex()
                        .gap(theme::GAP)
                        .children(account)
                        .child(button)
                        .into_any_element(),
                );
                parts.extend(refusal);
                parts
            }
            (Shown::Open, None) => {
                let mut parts = empty_intro(palette);
                parts.extend(self.nothing_parts(snapshot, palette, cx));
                parts.extend(self.account_button(snapshot, cx));
                parts
            }
            (Shown::Done, _) => {
                let selector = match snapshot.decided {
                    None => "inbox-done-absent",
                    Some(_) => "inbox-done-none",
                };
                vec![sentence(&done_words(snapshot), palette, selector)]
            }
        };
        div()
            .debug_selector(|| "inbox-empty".into())
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(theme::GAP)
            .px(theme::CONTENT_PADDING)
            .text_center()
            .children(parts)
            .into_any_element()
    }
}

impl InboxPage {
    fn account_button(&self, snapshot: &Snapshot, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !crate::ui::master_accounts::listed(snapshot).is_empty() {
            return None;
        }
        let (source, facts) = (self.source.clone(), self.facts.clone());
        let button = crate::ui::kit::button(
            "waiting-add-account",
            tr(Key("account_add.title")),
            crate::ui::kit::ButtonKind::Secondary,
            &crate::theme::Theme::palette(cx),
        )
        .on_click(move |_, window, cx| {
            crate::ui::account_add::open(source.clone(), facts.clone(), window, cx);
        });
        Some(button.into_any_element())
    }
}

impl InboxPage {
    /// What an empty Inbox says: that nothing needs the person, then what the lanes are doing (how many run,
    /// or how many are stopped) with the way to them.
    fn nothing_parts(
        &mut self,
        snapshot: &Snapshot,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut parts = Vec::new();
        let running = lanes::running_count(&snapshot.items);
        let now = crate::ui::unix_now();
        let blocked = lanes::open_lanes(&snapshot.items)
            .filter(|item| {
                item.agent_state == "blocked"
                    && !lanes::is_waiting_on_snoozed_asks(item, snapshot, now)
            })
            .count();
        let stopped = snapshot
            .items
            .iter()
            .filter(|item| !item.is_digest() && item.agent_state == "stopped")
            .count();
        for (count, key, selector) in [
            (running, "inbox.running", "inbox-running"),
            (blocked, "inbox.blocked", "inbox-blocked"),
            (stopped, "inbox.stopped", "inbox-stopped"),
        ] {
            if count > 0 {
                parts.push(sentence(
                    &say(&Said::new(key).count(count as u64)),
                    palette,
                    selector,
                ));
            }
        }
        if !snapshot.projects.is_empty() {
            parts.push(
                crate::ui::kit::button(
                    "inbox-start-lane",
                    tr(Key("inbox.start_lane")),
                    crate::ui::kit::ButtonKind::Secondary,
                    palette,
                )
                .on_click(cx.listener(|_, _, _, cx| cx.emit(InboxEvent::StartLane)))
                .into_any_element(),
            );
        }
        if running + blocked + stopped > 0 {
            parts.push(see_lanes(palette, cx));
        }
        parts
    }
}

/// The link under the sentence that opens the Lanes page.
fn see_lanes(palette: &Palette, cx: &mut Context<InboxPage>) -> AnyElement {
    let hover = palette.text;
    div()
        .id("inbox-see-lanes")
        .debug_selector(|| "inbox-see-lanes".into())
        .text_size(theme::TEXT_SMALL)
        .text_color(palette.text_secondary)
        .hover(move |style| style.text_color(hover))
        .on_click(cx.listener(|_, _, _, cx| cx.emit(InboxEvent::OpenLanes)))
        .child(tr(Key("inbox.see_lanes")))
        .into_any_element()
}

/// What the Done list says when it has nothing: that the engine does not keep answers, or that none was given.
fn done_words(snapshot: &Snapshot) -> String {
    match snapshot.decided {
        None => tr(Key("inbox.done.absent")).to_string(),
        Some(_) => tr(Key("inbox.done.none")).to_string(),
    }
}

fn empty_intro(palette: &Palette) -> Vec<AnyElement> {
    vec![
        headline(tr(Key("inbox.nothing")), palette),
        sentence(tr(Key("inbox.empty_intro")), palette, "inbox-empty-intro"),
    ]
}

fn headline(words: &str, palette: &Palette) -> AnyElement {
    text(theme::TEXT_BODY, palette.text)
        .debug_selector(|| "inbox-nothing".into())
        .child(words.to_string())
        .into_any_element()
}

fn sentence(words: &str, palette: &Palette, selector: &'static str) -> AnyElement {
    text(theme::TEXT_SMALL, palette.text_tertiary)
        .debug_selector(move || selector.into())
        .child(words.to_string())
        .into_any_element()
}

/// What is said when the engine cannot be read: its problem in words, and a button that opens Settings,
/// where the engine's path is set.
pub(super) fn problem_notice(
    problem: &Problem,
    palette: &Palette,
    cx: &mut Context<InboxPage>,
) -> AnyElement {
    let button = control(
        Button::new("waiting-engine-settings")
            .custom(secondary_variant(cx))
            .bg(crate::theme::Theme::palette(cx).fill),
        tr(Key("waiting.open_settings")),
        &[tr(Key("waiting.open_settings"))],
    )
    .debug_selector(|| "waiting-engine-settings".into())
    .on_click(|_, window, cx| window.dispatch_action(crate::menus::ShowSettings.boxed_clone(), cx));
    div()
        .debug_selector(|| "waiting-engine-problem".into())
        .flex_none()
        .px(theme::CONTENT_PADDING)
        .py(theme::GAP_WIDE)
        .flex()
        .flex_col()
        .gap(theme::GAP)
        .child(text(theme::TEXT_SMALL, palette.warning).child(problem.text()))
        .child(div().flex().child(button))
        .into_any_element()
}
