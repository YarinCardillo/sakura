//! The Inbox page: every ask the engine holds for the person, across projects. The asks are grouped by lane
//! in a list at the left; the one selected is at the right with what the lane said, where it comes from, what
//! it is about and one row of actions at the bottom. A decision is held for a few seconds with an Undo toast
//! before it is sent (`pending`); the person can read the answers already given under Done.
//!
//! The page decides nothing. It shows the engine's asks and sends the person's answer through
//! `sakura cockpit act`; the engine refuses an answer to a point that moved on, and the refusal is shown
//! under its ask. An action the engine asks to confirm says in words what it does and runs only on the
//! person's confirm.

mod controls;
mod decide;
mod detail;
mod empty;
mod face;
mod keys;
mod list;
pub(super) mod model;
mod offer;
mod pending;
mod proposal;
mod quit;
mod snooze;

#[cfg(test)]
mod tests;

pub(crate) use detail::is_spec_approval;
pub(super) use detail::property_with_label_width;
pub use keys::install_keys;
pub use quit::quit_after_answers;

use super::answers::Answers;
use super::brand::{BrandMark, Ink, MarkSize};
use super::project_add::ProjectAdds;
use crate::data::engine_facts::EngineFacts;
use crate::data::source::Source;
use crate::theme::Theme;
use decide::{Note, ResumeDraft, ResumeKey, SentAnswer};
use gpui::{
    div, AnyElement, AnyWindowHandle, App, AppContext as _, Context, Entity, EventEmitter,
    FocusHandle, InteractiveElement as _, IntoElement, ParentElement as _, Render, ScrollHandle,
    Styled as _, Subscription, Window,
};
use gpui_component::input::InputState;
use gpui_component::text::TextViewState;
use model::Filter;
use pending::HeldActs;
use sakura_core::engine::snapshot::Snapshot;
use std::collections::{HashMap, HashSet};

/// What the page tells the desk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InboxEvent {
    /// A lane was requested: its tab opens. Carries the lane's key.
    OpenLane(String),
    OpenTrack(String, bool),
    /// The Lanes page was requested: what the empty Inbox links to.
    OpenLanes,
    OpenScheduledRun(String),
    StartLane,
}

impl EventEmitter<InboxEvent> for InboxPage {}

/// Which asks the list holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Shown {
    /// The asks that wait for the person.
    Open,
    /// The asks the person answered lately.
    Done,
}

/// The page.
pub struct InboxPage {
    facts: Entity<EngineFacts>,
    source: Source,
    launch_mark: Entity<BrandMark>,
    answers: Entity<Answers>,
    /// Adding a project, offered when there is none; set by the desk.
    adds: Option<Entity<ProjectAdds>>,
    shown: Shown,
    filter: Filter,
    /// The ask selected in the list, by the wait's id (the Done list: by the answer's).
    selected: Option<String>,
    snooze_open: bool,
    snoozes: HashMap<String, snooze::Snooze>,
    held: HeldActs,
    read_marks: HashMap<String, decide::ReadMark>,
    sending: HashMap<String, SentAnswer>,
    reproposals: proposal::ReproposalTimes,
    /// The window the page was last used in, which a confirmation's toast is shown in.
    window: Option<AnyWindowHandle>,
    /// The note or reply being written, when one is.
    note: Option<Note>,
    field: Option<Entity<InputState>>,
    resume_drafts: HashMap<ResumeKey, ResumeDraft>,
    /// The markdown reading of each ask's text, kept while the text is the same.
    bodies: HashMap<String, (String, Entity<TextViewState>)>,
    /// The words of the asks the engine wrote, which no Markdown parser reads, by ask.
    plain_bodies: HashMap<String, String>,
    list_scroll: gpui::ListState,
    is_compact: bool,
    listed: Option<list::Rows>,
    detail_scroll: ScrollHandle,
    focus: FocusHandle,
    _observers: Vec<Subscription>,
}

impl InboxPage {
    /// The page over `facts`; the person's answers are kept by `answers`, and an answer that must go at once
    /// (the window closes) is sent through `source`.
    pub fn new(
        facts: Entity<EngineFacts>,
        answers: Entity<Answers>,
        source: Source,
        cx: &mut Context<Self>,
    ) -> Self {
        let observers = vec![
            cx.observe(&facts, |page, _, cx| {
                if let Some(snapshot) = page.facts.read(cx).snapshot() {
                    page.reproposals.reconcile(snapshot);
                    page.retire_resume_drafts(snapshot);
                }
                page.forget_resolved_bodies(cx);
                page.forget_acted_on_answers(cx);
                page.reconcile_read_marks(cx);
                page.reconcile_snoozes(cx);
                cx.notify();
            }),
            cx.observe(&answers, |page, _, cx| {
                page.settle_read_marks(cx);
                page.settle_snoozes(cx);
                page.bring_back_answered(cx);
            }),
            cx.subscribe(
                &facts,
                |page, _, _: &crate::data::engine_facts::AgesChanged, cx| {
                    page.expire_snoozes(cx);
                    if page.shown == Shown::Open
                        && page.snapshot(cx).is_some_and(|snapshot| {
                            snapshot
                                .waits
                                .iter()
                                .any(|wait| wait.snoozed_until.is_some())
                        })
                    {
                        cx.notify();
                    }
                },
            ),
            cx.on_release(|page, cx| drop(page.flush_held(cx))),
            cx.on_app_quit(|page, cx| {
                let sent = page.flush_held(cx);
                async move {
                    for pending in sent {
                        drop(pending.await);
                    }
                }
            }),
        ];
        quit::register(cx.weak_entity(), cx);
        let mut reproposals = proposal::ReproposalTimes::default();
        if let Some(snapshot) = facts.read(cx).snapshot() {
            reproposals.reconcile(snapshot);
        }
        Self {
            facts,
            source: source.clone(),
            launch_mark: cx.new(|_| BrandMark::revealing(MarkSize::MEDIUM, Ink::Fuchsia)),
            answers,
            adds: None,
            shown: Shown::Open,
            filter: Filter::default(),
            selected: None,
            snooze_open: false,
            snoozes: HashMap::new(),
            held: HeldActs::new(source),
            read_marks: HashMap::new(),
            sending: HashMap::new(),
            reproposals,
            window: None,
            note: None,
            field: None,
            resume_drafts: HashMap::new(),
            bodies: HashMap::new(),
            plain_bodies: HashMap::new(),
            list_scroll: gpui::ListState::new(0, gpui::ListAlignment::Top, crate::theme::GAP_WIDE),
            listed: None,
            is_compact: false,
            detail_scroll: ScrollHandle::new(),
            focus: cx.focus_handle(),
            _observers: observers,
        }
    }

    /// The text of the ask `id` as shown, for the tests.
    #[cfg(test)]
    pub(super) fn body_text(&self, id: &str) -> Option<String> {
        let read = self.bodies.get(id).map(|(text, _)| text.clone());
        read.or_else(|| self.plain_bodies.get(id).cloned())
    }

    /// Forgets the markdown of the asks the engine no longer lists, so a window left open does not keep every
    /// ask it ever showed.
    fn forget_resolved_bodies(&mut self, cx: &App) {
        let Some(snapshot) = self.facts.read(cx).snapshot() else {
            return;
        };
        let listed: HashSet<&str> = snapshot.waits.iter().map(|wait| wait.id.as_str()).collect();
        let decided: HashSet<_> = snapshot
            .decided
            .iter()
            .flatten()
            .map(model::decided_key)
            .collect();
        self.bodies
            .retain(|id, _| listed.contains(id.as_str()) || decided.contains(id));
        self.plain_bodies
            .retain(|id, _| listed.contains(id.as_str()) || decided.contains(id));
    }

    /// Sends every held answer that may go now, for the quit that waits for them.
    fn flush_held(
        &mut self,
        cx: &App,
    ) -> Vec<crate::data::engine_facts::Pending<sakura_core::engine::cli::ActResult>> {
        for ask in self.held.asks() {
            if self
                .held
                .act_of(&ask)
                .is_some_and(|act| self.held_destination_missing(&act, cx))
            {
                self.held.take(&ask);
            }
        }
        self.held.flush()
    }

    /// Offers adding a project through `adds` while the engine has none: what an empty page is for.
    pub fn offer_adding(&mut self, adds: Entity<ProjectAdds>, cx: &mut Context<Self>) {
        self._observers
            .push(cx.observe(&adds, |_, _, cx| cx.notify()));
        self.adds = Some(adds);
    }

    /// The answers the page sends and shows.
    #[cfg(test)]
    fn answers(&self) -> &Entity<Answers> {
        &self.answers
    }

    /// The ask selected now.
    #[cfg(test)]
    fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    /// The facts as the page draws them: its own copy, since drawing asks entities for controls.
    pub(super) fn snapshot(&self, cx: &App) -> Option<std::rc::Rc<Snapshot>> {
        let mut snapshot = self.facts.read(cx).shared_snapshot()?;
        self.reproposals.apply(&mut snapshot);
        if self.snoozes.values().any(|snooze| !snooze.is_pending) {
            self.merge_snoozes(std::rc::Rc::make_mut(&mut snapshot));
        }
        Some(snapshot)
    }

    /// Shows the answered asks, for the scripted checks.
    #[cfg(feature = "snapshot")]
    pub fn drive_done(&mut self, cx: &mut Context<Self>) {
        self.show(Shown::Done, cx);
    }

    pub(in crate::ui) fn is_compact(&self) -> bool {
        self.is_compact
    }

    pub(in crate::ui) fn set_compact(&mut self, is_compact: bool, cx: &mut Context<Self>) {
        self.is_compact = is_compact;
        cx.notify();
    }

    /// Lets go of the keyboard when the page is not the one shown: it took it to have its keys, and nothing
    /// else holds it until the person acts on the page that is shown.
    pub fn keep_focus_only_while_shown(
        &mut self,
        is_shown: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !is_shown && self.focus.is_focused(window) {
            window.blur(cx);
            cx.notify();
        }
    }

    #[cfg(test)]
    pub(in crate::ui) fn selection_and_scroll_for_test(
        &self,
    ) -> (Option<&str>, gpui::Point<gpui::Pixels>) {
        (self.selected.as_deref(), self.detail_scroll.offset())
    }

    #[cfg(test)]
    pub(in crate::ui) fn set_detail_scroll_for_test(&self, offset: gpui::Point<gpui::Pixels>) {
        self.detail_scroll.set_offset(offset);
    }

    /// Makes `ask` the selected one.
    pub(super) fn select(&mut self, ask: String, cx: &mut Context<Self>) {
        if self.selected.as_deref() != Some(ask.as_str()) {
            self.note = None;
            self.snooze_open = false;
            self.detail_scroll
                .set_offset(gpui::point(gpui::px(0.), gpui::px(0.)));
        }
        self.selected = Some(ask);
        cx.notify();
    }

    /// Keeps the selection on an ask the list holds: the first one when it holds none of the selected.
    fn reconcile(&mut self, ids: &[String]) {
        let is_listed = self.selected.as_ref().is_some_and(|id| ids.contains(id));
        if !is_listed {
            self.selected = ids.first().cloned();
            self.note = None;
            self.snooze_open = false;
        }
    }
}

impl Render for InboxPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::snapshot::count_render("inbox");
        let palette = Theme::palette(cx);
        let problem = self.facts.read(cx).problem().cloned();
        let notice = problem.map(|problem| empty::problem_notice(&problem, &palette, cx));
        let body = self.body(cx);
        div()
            .id("inbox")
            .debug_selector(|| "inbox".into())
            .key_context(keys::CONTEXT)
            .track_focus(&self.focus)
            .tab_index(0)
            .on_key_down(cx.listener(Self::answer_key))
            .size_full()
            .flex()
            .flex_col()
            .on_action(cx.listener(Self::next))
            .on_action(cx.listener(Self::previous))
            .children(notice)
            .child(div().flex_1().min_h_0().flex().child(body))
    }
}

impl InboxPage {
    /// What is under the problem notice: the list and the selected ask, or what an empty page says.
    fn body(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let palette = Theme::palette(cx);
        let Some(snapshot) = self.snapshot(cx) else {
            return div()
                .debug_selector(|| "inbox-launch-brand".into())
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(self.launch_mark.clone())
                .into_any_element();
        };
        let ids = self.ids(&snapshot, cx);
        self.reconcile(&ids);
        if ids.is_empty() {
            return div()
                .size_full()
                .relative()
                .child(self.empty_state(&snapshot, &palette, cx))
                .into_any_element();
        }
        let list = self.list(&palette, cx);
        let detail = self.detail(&snapshot, &palette, cx);
        super::split::columns(
            list,
            div()
                .relative()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .overflow_hidden()
                .child(detail)
                .into_any_element(),
            self.is_compact,
            &palette,
        )
        .into_any_element()
    }
}
