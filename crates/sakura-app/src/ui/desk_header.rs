//! The desk's side of the task header: what it has room for, the switch between the centre's views, its
//! actions at the right (the branch, which a press copies, "Stop" while an agent started in a terminal
//! runs, "Take" with its hold only for a lavoro in a box with something to take, the overflow last), the notice under it, and the actions the menu runs (stop the agent, open or
//! close a terminal, remove the task with its confirmation). The conversation is started by writing to
//! it: the header has no start action.

use super::chrome::{control, failure_sentence, keep_click, text_width};
use super::console::rules::is_agent_running;
use super::desk::{Desk, Hold};
use super::header_fit::{self, Fit, Widths};
use super::hold::{HoldButton, HoldConfirmed};
use super::page::{self, HeaderFacts};
use super::task_menu::{self, quiet_variant, MenuFacts};
use super::{remove, split, take};
use crate::data::actions::ActionKey;
use crate::data::changes::Scope;
use crate::l10n::{say, tr, tr_with, Key};
use crate::theme::{self, Theme};
use gpui::{
    div, px, AnyElement, AppContext as _, ClipboardItem, Context, Entity, InteractiveElement as _,
    IntoElement, ParentElement, StatefulInteractiveElement as _, Styled, Window,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::tooltip::Tooltip;
use gpui_component::Disableable as _;
use sakura_core::display;
use sakura_core::model::{self, RemoveRule, TakeRule};
use sakura_core::models::{Place, Task};
use std::time::Instant;

/// What the header's stop is called while it can be pressed and while `box` works on it.
const STOP: (Key, Key) = (Key("agent.stop"), Key("agent.stopping"));

impl Desk {
    /// The header row of the selected task, built for [`super::header_view::HeaderView`]; nothing without one.
    pub(super) fn header(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let name = self.shown(cx)?;
        let row = self.facts.read(cx).row(&name)?;
        let (task, is_pending) = (row.task.clone(), row.is_pending);
        let actions = if is_pending {
            Vec::new()
        } else {
            self.header_actions(&task, window, cx)
        };
        let is_waiting = self.facts.read(cx).is_waiting(&task.name);
        let live = self.facts.read(cx).live(&task.name);
        let fit = self.header_fit(&task, (is_pending, is_waiting, live), window, cx);
        let palette = Theme::palette(cx);
        let switch = self.centre_switch(&task.name, &palette, cx);
        let facts = HeaderFacts {
            task: &task,
            is_pending,
            is_waiting,
            live,
            fit,
        };
        Some(page::header(&facts, switch, actions, &palette, cx).into_any_element())
    }

    /// How many files "Modifiche" lists for `task`, once the whole patch of every change was read
    /// (a patch `box` cut short, or the patch of one commit, is not the count of the task's files).
    pub(super) fn listed_files(&self, task: &str, cx: &gpui::App) -> Option<usize> {
        let read = self.changes.read(cx).task(task)?;
        let patch = read.patch()?;
        (*read.scope() == Scope::All && patch.is_complete).then(|| patch.files.len())
    }

    /// The right side of `task`'s header, in order: the branch, the stop of an agent that runs in a
    /// terminal, Play, Publish, Take, the overflow last. One line per action: a new one is one more line.
    fn header_actions(
        &mut self,
        task: &Task,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut actions = Vec::new();
        actions.extend(self.branch_action(task, cx));
        actions.extend(self.agent_action(task, cx));
        actions.extend(self.play_control(task, window, cx));
        actions.extend(self.publish_actions(task, cx));
        actions.extend(self.take_action(task, cx));
        actions.push(self.overflow(task, cx));
        actions
    }

    /// The lavoro's branch, quiet: a press copies it. Under the pointer, the base it starts from.
    fn branch_action(&self, task: &Task, cx: &Context<Self>) -> Option<AnyElement> {
        let branch = task.branch.clone()?;
        let palette = Theme::palette(cx);
        let (fill, ink) = (palette.hover, palette.text);
        let from = page::where_from(task).unwrap_or_default();
        let tip = tr_with(Key("header.branch.tip"), &[("from", from.as_str())]).to_string();
        let chip = div()
            .id("header-branch")
            .debug_selector(|| "header-branch".into())
            .h(theme::STATUS_CONTROL_HEIGHT)
            .max_w(px(theme::HEADER_BRANCH_MAX_WIDTH))
            .min_w_0()
            .px(theme::CONTROL_PADDING_X)
            .flex()
            .items_center()
            .rounded(theme::RADIUS_CONTROL)
            .text_size(theme::TEXT_SMALL)
            .font_weight(theme::WEIGHT_MEDIUM)
            .text_color(palette.text_secondary)
            .cursor_pointer()
            .hover(move |style| style.bg(fill).text_color(ink))
            .child(div().truncate().child(display::label(&branch)))
            .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
            .on_click(move |_, _, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(branch.clone()));
            });
        Some(keep_click(chip).into_any_element())
    }

    /// "Take" for a lavoro in a box with something to take, or the removal waiting for its hold.
    fn take_action(&mut self, task: &Task, cx: &mut Context<Self>) -> Vec<AnyElement> {
        if self.removing.as_deref() == Some(task.name.as_str()) {
            return self.remove_confirmation(task, cx);
        }
        if !has_take(task) {
            return Vec::new();
        }
        let hold = self.take_hold(task, cx);
        let palette = Theme::palette(cx);
        let take = take::button(task, self.actions.read(cx), &hold, &palette, cx);
        take.into_iter().collect()
    }

    /// The overflow: what else can be done with the lavoro.
    fn overflow(&self, task: &Task, cx: &mut Context<Self>) -> AnyElement {
        let facts = MenuFacts {
            is_agent_running: is_agent_running(task),
            is_terminal_open: self.has_terminal(&task.name, cx),
            can_remove: task_menu::can_remove(task),
        };
        task_menu::view(task, facts, self.actions.read(cx), cx)
    }

    /// "Stop" while an agent that was started in a terminal works: a quiet button of the common control
    /// height, with its pending word while `box` answers in the room of its resting one. A conversation
    /// is stopped where it is written.
    fn agent_action(&self, task: &Task, cx: &Context<Self>) -> Option<AnyElement> {
        if !task.readable || task.preparing.is_some() || !is_agent_running(task) {
            return None;
        }
        let is_pending = self
            .actions
            .read(cx)
            .is_running(&ActionKey::stop_agent(&task.name));
        let name = task.name.clone();
        let quiet = Button::new("header-stop")
            .debug_selector(|| "header-stop".into())
            .custom(quiet_variant(&Theme::palette(cx), cx));
        let shown = if is_pending { STOP.1 } else { STOP.0 };
        let button = control(quiet, tr(shown), &[tr(STOP.0), tr(STOP.1)])
            .disabled(is_pending)
            .on_click(cx.listener(move |desk, _, _, cx| desk.stop_agent(name.clone(), cx)));
        let stop = keep_click(div().id(("header-stop", 0usize)).flex_none().child(button));
        Some(stop.into_any_element())
    }

    /// What the header has room for at the width of the page.
    fn header_fit(
        &self,
        task: &Task,
        state: (bool, bool, model::Live),
        window: &Window,
        cx: &gpui::App,
    ) -> Fit {
        let sidebar = self.sidebar_slide.share(Instant::now());
        let available = f32::from(split::body_width(window.viewport_size().width, sidebar));
        let widths = self.header_widths(task, state, window, cx);
        header_fit::fit(available, &widths)
    }

    /// The widths of the header's parts, measured with the fonts they are drawn in.
    /// `state` says whether the task's facts are being read, whether its agent waits for the person and
    /// what its conversation says of its agent.
    fn header_widths(
        &self,
        task: &Task,
        (is_pending, is_waiting, live): (bool, bool, model::Live),
        window: &Window,
        cx: &gpui::App,
    ) -> Widths {
        let small =
            |label: &str| text_width(window, label, theme::TEXT_SMALL, theme::WEIGHT_MEDIUM);
        let gap = f32::from(theme::GAP_WIDE);
        let glyph = f32::from(theme::ICON_SIZE);
        let state_word = if is_pending {
            page::READING
        } else {
            model::shown_state_live(task, is_waiting, live).word
        };
        let actions = self.actions_width(task, is_pending, window);
        let padded = f32::from(theme::CONTROL_PADDING_X) * 2.0;
        let views = self.centre_views_of(&task.name, cx);
        let views: Vec<f32> = views.map(|view| small(view.label()) + padded).collect();
        let between = f32::from(theme::TAB_GAP) * views.len().saturating_sub(1) as f32;
        Widths {
            fixed: f32::from(theme::CONTENT_PADDING) * 2.0
                + glyph
                + f32::from(theme::ROW_GAP)
                + gap * 2.0
                + actions,
            views: views.iter().sum::<f32>() + between,
            state_word: small(tr(state_word)) + gap,
            figures: theme::HEADER_FIGURES_WIDTH + gap,
        }
    }

    /// The width the actions at the right take, from their labels.
    fn actions_width(&self, task: &Task, is_pending: bool, window: &Window) -> f32 {
        if is_pending {
            return 0.0;
        }
        let small = |label: &str| {
            text_width(window, label, theme::TEXT_SMALL, theme::WEIGHT_MEDIUM)
                + f32::from(theme::CONTROL_PADDING_X) * 2.0
                + f32::from(theme::GAP_WIDE)
        };
        let agent = match is_agent_running(task) {
            true => small(tr(STOP.0)),
            false => 0.0,
        };
        let branch = task.branch.as_deref().map_or(0.0, |branch| {
            small(&display::label(branch))
                .min(theme::HEADER_BRANCH_MAX_WIDTH + f32::from(theme::GAP_WIDE))
        });
        let take = match (
            self.removing.as_deref() == Some(task.name.as_str()),
            model::take_rule(task),
        ) {
            (true, _) => small(tr(Key("common.cancel"))) + small(remove::hold_label(false)),
            (false, _) if !has_take(task) => 0.0,
            (false, TakeRule::Plain) => small(tr(Key("take.action"))),
            (false, TakeRule::Hold { .. }) => small(take::hold_label(false)),
            (false, TakeRule::Disabled { .. }) => 0.0,
        };
        // The overflow's icon ends on the inset: only its icon and the bleed before it take room.
        let publish = self.publish_width(task, window);
        let play = self.play_width(&task.name, window);
        let overflow = f32::from(theme::ICON_SIZE + theme::CONTROL_BLEED + theme::GAP_WIDE);
        branch + agent + play + publish + take + overflow
    }

    /// What is said under the header, when there is something to say: why removing would lose work, while
    /// its confirmation is asked; else the failure of the last action on the task, in `box`'s words.
    pub(super) fn page_notice(&self, task: &Task, cx: &Context<Self>) -> Option<AnyElement> {
        let palette = Theme::palette(cx);
        let actions = self.actions.read(cx);
        let failed = [
            ActionKey::remove(&task.name),
            ActionKey::take(&task.name),
            ActionKey::stop_agent(&task.name),
        ];
        let failed = failed.iter().find(|key| actions.error(key).is_some());
        let failure =
            failed.and_then(|key| Some((actions.error(key)?, actions.error_details(key))));
        let is_removing = self.removing.as_deref() == Some(task.name.as_str());
        let (sentence, details, colour) = match (failure, model::remove_rule(task)) {
            (Some((failure, details)), _) => (say(failure).to_string(), details, palette.danger),
            (None, RemoveRule::Hold { reason } | RemoveRule::Disabled { reason })
                if is_removing =>
            {
                (say(&reason).to_string(), None, palette.text_secondary)
            }
            _ => return None,
        };
        // A failure is said in Sakura's words; what `box` wrote is under the pointer.
        let notice = failure_sentence("page-notice", sentence, details)
            .debug_selector(|| "page-notice".into())
            .text_size(theme::TEXT_LABEL)
            .text_color(colour)
            .flex_none()
            .px(theme::CONTENT_PADDING)
            .py(theme::GAP)
            .border_b_1()
            .border_color(palette.border);
        Some(notice.into_any_element())
    }

    /// Stops the agent of `task`.
    pub fn stop_agent(&mut self, task: String, cx: &mut Context<Self>) {
        self.actions
            .update(cx, |actions, cx| actions.stop_agent(task, cx));
    }

    /// "Archive": removes at once when nothing would be lost, else asks for a hold.
    pub fn ask_remove(&mut self, task: &str, cx: &mut Context<Self>) {
        let Some(row) = self.facts.read(cx).row(task) else {
            return;
        };
        match model::remove_rule(&row.task) {
            RemoveRule::Plain => self.remove(task.to_string(), false, cx),
            RemoveRule::Hold { .. } => {
                self.removing = Some(task.to_string());
                cx.notify();
            }
            RemoveRule::Disabled { .. } => {}
        }
    }

    /// Puts the removal's confirmation away.
    pub fn cancel_remove(&mut self, task: &str, cx: &mut Context<Self>) {
        if self.removing.as_deref() == Some(task) {
            self.removing = None;
            cx.notify();
        }
    }

    fn remove(&mut self, task: String, force: bool, cx: &mut Context<Self>) {
        self.actions
            .update(cx, |actions, cx| actions.remove(task, force, cx));
    }

    /// "Cancel" and the hold that removes; the reason is in the notice under the header.
    fn remove_confirmation(&mut self, task: &Task, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let key = ActionKey::remove(&task.name);
        let is_pending = self.actions.read(cx).is_running(&key);
        let hold = self.hold(
            format!("hold-remove-{}", task.name),
            &task.name,
            cx,
            |desk, task, cx| desk.remove(task, true, cx),
        );
        let words = [remove::hold_label(false), remove::hold_label(true)];
        relabel(
            &hold,
            (remove::hold_label(is_pending), words),
            is_pending,
            cx,
        );
        remove::confirm(&task.name, &hold, cx)
    }

    /// The take's hold for `task`, labelled and reset once `box` answered.
    fn take_hold(&mut self, task: &Task, cx: &mut Context<Self>) -> Entity<HoldButton> {
        let is_pending = self
            .actions
            .read(cx)
            .is_running(&ActionKey::take(&task.name));
        let hold = self.hold(
            format!("hold-{}", task.name),
            &task.name,
            cx,
            |desk, task, cx| {
                // The hold came to its end: the trackpad says so, as when a dragged tab lands.
                desk.haptics.alignment();
                desk.take(task, cx)
            },
        );
        let words = [take::hold_label(false), take::hold_label(true)];
        relabel(&hold, (take::hold_label(is_pending), words), is_pending, cx);
        hold
    }

    /// The hold button `id` of `task`, made once; a completed hold calls `on_confirm`.
    fn hold(
        &mut self,
        id: String,
        task: &str,
        cx: &mut Context<Self>,
        on_confirm: fn(&mut Desk, String, &mut Context<Desk>),
    ) -> Entity<HoldButton> {
        if let Some(hold) = self.holds.get(&id) {
            return hold.button.clone();
        }
        let button = cx.new(|_| HoldButton::new(id.clone()));
        let confirmed_task = task.to_string();
        let confirmed = cx.subscribe(&button, move |desk, _, _: &HoldConfirmed, cx| {
            on_confirm(desk, confirmed_task.clone(), cx)
        });
        let hold = Hold {
            task: task.to_string(),
            button: button.clone(),
            _confirmed: confirmed,
        };
        self.holds.insert(id, hold);
        button
    }
}

/// Labels a hold with its word among the two it can show, and readies it for another hold once its
/// action is no longer pending.
fn relabel(
    hold: &Entity<HoldButton>,
    (label, words): (&'static str, [&'static str; 2]),
    is_pending: bool,
    cx: &mut gpui::App,
) {
    hold.update(cx, |hold, cx| {
        hold.set_label(label, words);
        if !is_pending && hold.is_done() {
            hold.reset(cx);
        }
    });
}

/// Whether "Take" belongs to `task`'s header at all: a lavoro in a box is taken into the person's
/// repository; one on the Mac is already a branch of it.
fn has_take(task: &Task) -> bool {
    task.place == Place::Box
}
