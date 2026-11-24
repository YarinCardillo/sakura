//! The page of one lane: its state, phase and the engine's progress at the top, its roles with the model
//! each runs on, what the engine recorded of it lately, and the controls the engine gives it (change a
//! model or the dial, approve, resume, stop, write to the agent). The page shows the snapshot and sends
//! the person's choice through `sakura cockpit act`; it decides nothing.

mod digest;
mod steps;

use super::agent_chat::{AgentChat, AgentChatEvent};
use super::answers::{lane_key, step_view, Answers, Step};
use super::chrome::{quiet_variant, secondary_variant, tabular, text};
use super::event_words::event_words;
use super::glyph::{glyph_column, placed_state_glyph};
use super::inbox::model as asks;
use super::inbox::InboxPage;
use super::lane_agents::{LaneAgents, LaneAgentsEvent};
use super::lane_composer::{composer_answer, ComposerEvent, LaneComposer};
use super::lane_side::{LaneSide, LaneSideEvent};
use super::lanes;
use super::page_bar::PageControls;
use super::step_track::step_words;
use super::tip::{self, Words};
use super::titled_links::titled_links;
use crate::data::engine_facts::EngineFacts;
use crate::data::source::Source;
use crate::l10n::{say, tr, tr_with, Key};
use crate::theme::{self, lane_page as sizes, Palette, Theme};
use crate::ui::kit;
use gpui::{
    div, AnyElement, App, AppContext as _, ClickEvent, Context, Entity, InteractiveElement as _,
    IntoElement, ParentElement, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement as _, Styled, Subscription, WeakEntity, Window,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_component::{Disableable as _, Icon, Sizable as _};
use gpui_kit_assets::IconName;
use sakura_core::engine::snapshot::{Item, Snapshot, Wait};
use sakura_core::model::StateId;
use sakura_core::said::Said;
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

/// The only kind of address a lane's link may open: the engine takes no other, and the page checks again.
const LINK_SCHEME: &str = "https://";
/// The action that closes an outside lane, the last control of its page.
const ACTION_CLOSE: &str = "d";
const ACTION_CLEAN_UP: &str = "C";

/// The action that writes to the lane's agent: under the conversation of a flow lane.
const ACTION_WRITE: &str = "w";
/// How many characters of a delivered head a lane's page shows.
const DELIVERY_HEAD_CHARS: usize = 7;

/// The word on a verb's button: the Inbox's own, so that one action has one name on the lane's card and in the
/// Inbox ("Done" for a verdict on a check, "Approve" for an approval).
fn verb_words(verb: &asks::Verb) -> SharedString {
    verb.label()
}

/// The words on the buttons of the card of `wait`, in the Inbox's own words: one name per action.
#[cfg(test)]
pub(super) fn ask_button_words(wait: &Wait, lane: Option<&Item>) -> Vec<String> {
    let verbs = asks::verbs(wait, lane);
    let given = verbs.primary.iter().chain(&verbs.secondary);
    given.map(|verb| verb_words(verb).to_string()).collect()
}

/// What the page asks of the window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LanePageEvent {
    /// The person wants to answer an ask in words, which the Inbox is for.
    OpenInbox,
    OpenTrack(String),
    /// What the box holds, unsent, changed: the window keeps it.
    DraftChanged,
}

/// The page of the lane named `lane` (its task's id).
pub struct LanePage {
    lane: String,
    pending_step: Option<(String, bool)>,
    facts: Entity<EngineFacts>,
    answers: Entity<Answers>,
    /// The Inbox, which holds and sends the answers to this lane's asks.
    inbox: WeakEntity<InboxPage>,
    source: Source,
    /// The list of a flow lane's agents, made when its page first shows the lane.
    agents: Option<Entity<LaneAgents>>,
    /// Whether the only run the engine counts as the lane's plan review is the arbiter's: the lane has no plan
    /// review, whatever the engine lists.
    is_plan_review_the_arbiters: bool,
    /// The run name of the agent whose conversation the person opened; none while the overview shows.
    open_agent: Option<String>,
    /// Whether the page is the one the tab shows: a page that is not reads nothing.
    is_shown: bool,
    is_compact: bool,
    /// The conversations the person opened, one per agent, kept while the page lives so that a way back and a
    /// second visit find the box as it was left.
    chats: HashMap<String, Entity<AgentChat>>,
    chat_subscriptions: HashMap<String, Subscription>,
    recent_chats: VecDeque<String>,
    /// How far the overview is scrolled, kept while an agent's conversation is open.
    overview_scroll: ScrollHandle,
    /// What a morning digest wrote, read as Markdown, kept while its text is the same.
    report: Option<(String, Entity<gpui_component::text::TextViewState>)>,
    /// The column at the right: Details, Changes and Files, made when the page first shows the lane.
    side: Option<Entity<LaneSide>>,
    /// Whether the column is shown; remembered by this lane's page.
    is_column_open: bool,
    /// The box the person writes in, made when the page first shows the lane.
    composer: Option<Entity<LaneComposer>>,
    /// Words a last launch left unsent, put in the box when it is made.
    restored_draft: Option<String>,
    /// The same for each agent's box, by the agent's first run, put in that agent's conversation only.
    restored_agent_drafts: HashMap<String, String>,
    /// How far an outside lane's page is scrolled, kept while its tab is on another page.
    scroll: ScrollHandle,
    /// The answer kept under this key is the one to the text the box sent last: when the engine took it, the box empties.
    sent: Option<String>,
    _observers: Vec<Subscription>,
}

impl gpui::EventEmitter<LanePageEvent> for LanePage {}

impl PageControls for LanePage {
    /// A lane's menu: its own actions that need no words from the person (Resume, Stop, and Close for an outside lane).
    fn controls(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let snapshot = self.facts.read(cx).shared_snapshot();
        let item = snapshot.as_ref().and_then(|snapshot| self.item(snapshot));
        let track = snapshot
            .as_deref()
            .zip(item)
            .and_then(|(snapshot, item)| self.track_control(item, snapshot, cx));
        let keys: Vec<&'static str> = item
            .into_iter()
            .flat_map(|item| {
                MENU_ACTIONS
                    .iter()
                    .filter(|key| item.actions.iter().any(|a| a.key == **key))
                    // Stop is for a lane that runs: a stopped or finished one has nothing to stop.
                    .filter(|key| match **key {
                        "s" => lanes::is_open(item),
                        ACTION_CLEAN_UP => item.agent_state == "stopped",
                        _ => true,
                    })
            })
            .copied()
            .collect();
        if keys.is_empty() {
            return track.unwrap_or_else(|| gpui::Empty.into_any_element());
        }
        let page = cx.weak_entity();
        let palette = Theme::palette(cx);
        let is_digest = item.is_some_and(Item::is_digest);
        let is_missing = item.is_some_and(|item| self.is_folder_missing(item, cx));
        let menu = Button::new("lane-more")
            .debug_selector(|| "lane-more".into())
            .custom(quiet_variant(&palette, cx))
            .h(theme::CONTROL_HEIGHT)
            .px(theme::CONTROL_PADDING_X)
            .child(Icon::new(IconName::Ellipsis).with_size(theme::HAND_MARK_SIZE))
            .dropdown_menu(move |menu, _, _| {
                let mut menu = menu.min_w(theme::inbox::MENU_WIDTH);
                for key in &keys {
                    let (page, key) = (page.clone(), *key);
                    let label = match (is_digest, key) {
                        (true, ACTION_CLOSE) => {
                            SharedString::from(tr(Key("lane.action.archive_digest")))
                        }
                        _ => action_label(key),
                    };
                    if is_missing && !is_close_action(key) {
                        let label = format!("{label}, {}", tr(lanes::FOLDER_MISSING));
                        menu = menu.item(
                            PopupMenuItem::new(label)
                                .icon(action_icon(key))
                                .disabled(true),
                        );
                        continue;
                    }
                    menu = menu.item(PopupMenuItem::new(label).icon(action_icon(key)).on_click(
                        move |_: &ClickEvent, window, cx| {
                            drop(page.update(cx, |page, cx| {
                                if key != "R" || !page.offer_resume_note(window, cx) {
                                    page.press_lane_action(key, cx);
                                }
                            }));
                        },
                    ));
                }
                menu
            });
        if let Some(track) = track {
            div()
                .flex()
                .items_center()
                .gap(theme::GAP_SMALL)
                .child(track)
                .child(menu)
                .into_any_element()
        } else {
            menu.into_any_element()
        }
    }
}

impl LanePage {
    fn track_control(
        &self,
        item: &Item,
        snapshot: &Snapshot,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        if item.is_outside() || item.agent_state == "cleaned" {
            return None;
        }
        let current = item
            .track
            .as_ref()
            .and_then(|reference| snapshot.track(&reference.id));
        let can_unlink = current.is_some_and(|track| {
            track.lanes.iter().any(|member| {
                !member.did_not_start
                    && member
                        .key
                        .as_ref()
                        .is_some_and(|key| key.as_slice() == item.key.as_slice())
            })
        });
        let page = cx.weak_entity();
        super::track_page::membership_control(
            "lane-track",
            super::track_page::Membership {
                snapshot,
                current,
                inputs: super::track_page::lane_inputs(&item.repo, item.task()),
                title: None,
                can_link: super::track_page::can_link_lane(item, snapshot),
                can_unlink,
                open_track: std::rc::Rc::new(move |track, cx| {
                    drop(page.update(cx, |_, cx| cx.emit(LanePageEvent::OpenTrack(track))));
                }),
            },
            &self.facts,
            &self.source,
            cx,
        )
    }
}

/// The lane actions the "..." menu offers: the ones that carry no words of the person's own (Close is an outside
/// lane's).
const MENU_ACTIONS: [&str; 4] = ["R", "s", ACTION_CLEAN_UP, ACTION_CLOSE];

pub(super) fn is_close_action(key: &str) -> bool {
    matches!(key, ACTION_CLOSE | ACTION_CLEAN_UP)
}

/// The icon a menu entry of a lane action has: what it does, in one glance.
fn action_icon(key: &str) -> IconName {
    match key {
        "R" => IconName::Play,
        ACTION_CLOSE | ACTION_CLEAN_UP => IconName::Inbox,
        _ => IconName::Square,
    }
}

fn action_label(key: &str) -> SharedString {
    let words = match key {
        "R" => Key("waiting.resume"),
        ACTION_CLOSE | ACTION_CLEAN_UP => Key("lane.action.close"),
        _ => Key("waiting.stop"),
    };
    SharedString::from(tr(words))
}

impl LanePage {
    /// The page of `lane` over `facts`; the person's answers are kept by `answers`, and the answers to its
    /// asks go through `inbox`.
    pub fn new(
        lane: String,
        facts: Entity<EngineFacts>,
        (answers, inbox): (Entity<Answers>, WeakEntity<InboxPage>),
        source: Source,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut observers = vec![
            cx.observe(&facts, |page, facts, cx| {
                if let Some(item) = facts
                    .read(cx)
                    .snapshot()
                    .and_then(|snapshot| page.item(snapshot))
                {
                    let applied = page.pending_step.as_ref().is_some_and(|(id, is_on)| {
                        item.pipeline
                            .iter()
                            .any(|step| step.id == *id && step.is_on == *is_on)
                    });
                    if applied {
                        page.pending_step = None;
                    }
                }
                cx.notify();
            }),
            cx.observe(&answers, |page, _, cx| page.empty_composer_if_taken(cx)),
        ];
        if let Some(inbox) = inbox.upgrade() {
            observers.push(cx.observe(&inbox, |_, _, cx| cx.notify()));
        }
        Self {
            lane,
            pending_step: None,
            facts,
            answers,
            inbox,
            source,
            agents: None,
            is_plan_review_the_arbiters: false,
            open_agent: None,
            is_shown: true,
            is_compact: false,
            chats: HashMap::new(),
            chat_subscriptions: HashMap::new(),
            recent_chats: VecDeque::new(),
            overview_scroll: ScrollHandle::new(),
            report: None,
            side: None,
            is_column_open: true,
            composer: None,
            restored_draft: None,
            restored_agent_drafts: HashMap::new(),
            scroll: ScrollHandle::new(),
            sent: None,
            _observers: observers,
        }
    }

    /// The lane's own record in `snapshot`.
    fn item<'a>(&self, snapshot: &'a Snapshot) -> Option<&'a Item> {
        snapshot
            .items
            .iter()
            .find(|item| item.lane_key() == self.lane)
    }

    /// Sends `text` through the lane's "write to the agent" action, for the scripted checks.
    #[cfg(feature = "snapshot")]
    pub fn drive_write(&mut self, text: &str, cx: &mut Context<Self>) {
        self.write(text, cx);
    }

    /// Opens the conversation of `run`, for the scripted checks.
    #[cfg(feature = "snapshot")]
    pub fn drive_open_agent(&mut self, run: &str, cx: &mut Context<Self>) {
        self.show_agent(Some(run.to_string()), cx);
    }

    /// How many times the text being written in the open conversation grew, for the scripted checks.
    #[cfg(feature = "snapshot")]
    pub fn drive_chat_growths(&self, cx: &gpui::App) -> Option<usize> {
        let chat = self.chats.get(self.open_agent.as_ref()?)?;
        Some(chat.read(cx).growths())
    }

    /// Sends `text` to the lane through the engine's "write to the agent" action; the box empties when the
    /// engine took it.
    fn write(&mut self, text: &str, cx: &mut Context<Self>) {
        let snapshot = self.facts.read(cx).shared_snapshot();
        let Some(item) = snapshot.as_ref().and_then(|snapshot| self.item(snapshot)) else {
            return;
        };
        if self.is_folder_missing(item, cx) {
            return;
        }
        let Some(action) = item.actions.iter().find(|a| a.key == ACTION_WRITE) else {
            return eprintln!("sakura: the lane has no composer");
        };
        let inputs = BTreeMap::from([("text".to_string(), text.to_string())]);
        let key = lane_key(&item.id, &action.key);
        let (action, target) = (action.clone(), item.id.clone());
        self.sent = Some(key.clone());
        self.answers.update(cx, |answers, cx| {
            answers.press(key, action, target, inputs, cx)
        });
    }

    /// The engine took the text the box sent last: the box empties, and keeps it when the engine refused.
    fn empty_composer_if_taken(&mut self, cx: &mut Context<Self>) {
        let taken = self.sent.as_ref().is_some_and(|key| {
            matches!(
                self.answers.read(cx).step(key),
                Some(Step::Told {
                    is_problem: false,
                    ..
                })
            )
        });
        if let (true, Some(composer)) = (taken, self.composer.clone()) {
            self.sent = None;
            composer.update(cx, |composer, cx| composer.clear(cx));
        }
        cx.notify();
    }

    /// How far the page is scrolled, for the tests.
    #[cfg(test)]
    pub(super) fn scroll_offset(&self) -> gpui::Point<gpui::Pixels> {
        self.scroll.offset()
    }

    /// What the person has typed in the box shown and not sent: the open agent's, else an outside lane's.
    #[cfg(test)]
    pub fn draft(&self, cx: &gpui::App) -> String {
        let open = self
            .open_agent
            .as_ref()
            .and_then(|run| self.chats.get(run))
            .map(|chat| chat.read(cx).draft(cx));
        let outside = self
            .composer
            .as_ref()
            .map(|composer| composer.read(cx).draft(cx));
        open.or(outside)
            .or_else(|| self.restored_draft.clone())
            .unwrap_or_default()
    }

    /// Words a last launch left unsent for each agent: they are in that agent's box once it is made.
    pub fn restore_agent_drafts(&mut self, drafts: Vec<(String, String)>) {
        self.restored_agent_drafts = drafts.into_iter().collect();
    }

    /// Everything unsent on the page: the box of an outside lane (no agent), and the box of each agent whose
    /// conversation was opened, by the agent's first run. A box that is empty is listed too, so that what was
    /// sent is forgotten.
    pub fn drafts(&self, cx: &gpui::App) -> Vec<(Option<String>, String)> {
        let outside = self
            .composer
            .as_ref()
            .map(|composer| composer.read(cx).draft(cx));
        let outside = outside.or_else(|| self.restored_draft.clone());
        let agents = self
            .chats
            .iter()
            .map(|(run, chat)| (Some(run.clone()), chat.read(cx).draft(cx)));
        outside
            .map(|text| (None, text))
            .into_iter()
            .chain(agents)
            .chain(
                self.restored_agent_drafts
                    .iter()
                    .filter(|(run, _)| !self.chats.contains_key(*run))
                    .map(|(run, text)| (Some(run.clone()), text.clone())),
            )
            .collect()
    }

    /// Words a last launch left unsent: they are in the box once it is made.
    pub fn restore_draft(&mut self, text: String) {
        self.restored_draft = Some(text);
    }

    /// The field of the box the person writes in: an outside lane's, or the open agent's.
    #[cfg(test)]
    pub(super) fn composer_field(
        &self,
        cx: &gpui::App,
    ) -> Option<Entity<gpui_component::input::TextareaState>> {
        if let Some(composer) = &self.composer {
            return Some(composer.read(cx).field());
        }
        let chat = self.chats.get(self.open_agent.as_ref()?)?;
        Some(chat.read(cx).composer_field(cx))
    }

    /// The box under the conversation, made when the page first shows the lane.
    fn composer_of(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<LaneComposer> {
        if let Some(composer) = &self.composer {
            return composer.clone();
        }
        let composer = cx.new(|cx| LaneComposer::new(window, cx));
        if let Some(text) = self.restored_draft.take() {
            composer.update(cx, |composer, cx| composer.restore(&text, window, cx));
        }
        self._observers.push(cx.subscribe(
            &composer,
            |page, _, ComposerEvent::Send(text): &ComposerEvent, cx| page.write(text, cx),
        ));
        self._observers
            .push(cx.observe(&composer, |_, _, cx| cx.emit(LanePageEvent::DraftChanged)));
        self.composer = Some(composer.clone());
        composer
    }

    /// The box and, under it, what the engine said to the last message: where it put it, in words.
    fn composer_box(
        &mut self,
        item: &Item,
        window: &mut Window,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        item.actions.iter().find(|a| a.key == ACTION_WRITE)?;
        if self.is_folder_missing(item, cx) {
            return Some(
                text(theme::TEXT_SMALL, palette.text_secondary)
                    .child(tr(lanes::FOLDER_MISSING))
                    .into_any_element(),
            );
        }
        if item.agent_state == "stopped" {
            return Some(self.stopped_box(item, palette, cx));
        }
        let composer = self.composer_of(window, cx);
        composer.update(cx, |composer, cx| composer.set_to(addressee(item), cx));
        let key = lane_key(&item.id, ACTION_WRITE);
        let answered = self.answers.read(cx).step(&key).cloned();
        let answered = answered
            .map(|step| step_view(&self.answers, &key, &composer_answer(&step), palette, cx));
        Some(
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(theme::GAP_TIGHT)
                .child(composer)
                .children(answered)
                .into_any_element(),
        )
    }

    /// What stands where the box would, while the lane is stopped: the words that it is, and Resume when the
    /// engine offers it, since nothing at work could read a message.
    fn stopped_box(
        &mut self,
        item: &Item,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let can_resume = item.actions.iter().any(|a| a.key == "R");
        let resume = can_resume.then(|| {
            super::chrome::control(
                Button::new("lane-resume")
                    .custom(super::chrome::secondary_variant(cx))
                    .bg(crate::theme::Theme::palette(cx).fill),
                tr(Key("waiting.resume")),
                &[tr(Key("waiting.resume"))],
            )
            .debug_selector(|| "lane-resume".into())
            .on_click(cx.listener(|page, _, _, cx| page.press_lane_action("R", cx)))
        });
        div()
            .debug_selector(|| "lane-stopped".into())
            .w_full()
            .max_w(sizes::COMPOSER_MAX_WIDTH)
            .px(sizes::COMPOSER_INNER_LEFT)
            .h(theme::CONTROL_HEIGHT)
            .flex()
            .items_center()
            .justify_between()
            .rounded(sizes::COMPOSER_RADIUS)
            .bg(palette.fill)
            .child(
                text(theme::TEXT_SMALL, palette.text_secondary)
                    .child(tr(Key("lane_page.stopped_note"))),
            )
            .children(resume)
            .into_any_element()
    }

    /// How far the overview is scrolled, for the tests.
    #[cfg(test)]
    pub(super) fn overview_offset(&self) -> gpui::Point<gpui::Pixels> {
        self.overview_scroll.offset()
    }

    /// Scrolls the overview by `by` (negative scrolls down), for the tests.
    #[cfg(test)]
    pub(super) fn scroll_overview(&self, by: gpui::Pixels) {
        self.overview_scroll
            .set_offset(gpui::point(gpui::px(0.), by));
    }

    /// The conversation that is open, for the tests.
    #[cfg(test)]
    pub(super) fn open_chat(&self) -> Option<Entity<AgentChat>> {
        self.chats.get(self.open_agent.as_ref()?).cloned()
    }

    #[cfg(test)]
    pub(super) fn cached_chats(&self) -> usize {
        self.chats.len()
    }

    /// The run name of the agent whose conversation is open; none while the overview shows.
    #[cfg(test)]
    pub(super) fn open_agent(&self) -> Option<String> {
        self.open_agent.clone()
    }
}

impl Render for LanePage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::snapshot::count_render("lane page");
        let palette = Theme::palette(cx);
        let snapshot = self.facts.read(cx).shared_snapshot();
        let page = div()
            .id("lane-page")
            .debug_selector(|| "lane-page".into())
            .size_full()
            .track_scroll(&self.scroll)
            .overflow_y_scroll()
            .flex()
            .flex_col();
        let Some((snapshot, item)) = snapshot
            .as_deref()
            .and_then(|snapshot| self.item(snapshot).map(|item| (snapshot, item)))
        else {
            let gone = text(theme::TEXT_SMALL, palette.text_tertiary)
                .px(theme::CONTENT_PADDING)
                .py(theme::GAP_WIDE)
                .child(tr(Key("lane.gone")));
            return page.child(gone).into_any_element();
        };
        if !item.is_outside() {
            return self.conversation_page(page, (item, snapshot), window, &palette, cx);
        }
        if item.is_digest() {
            return self.digest_page(page, item, snapshot, &palette, cx);
        }
        let events = events_of(item, Some(snapshot));
        let body = div()
            .px(theme::CONTENT_PADDING)
            .py(theme::GAP_WIDE)
            .flex()
            .flex_col()
            .gap(theme::settings::GROUP_GAP)
            .children(unread_view(item, &palette))
            .children(self.roles(item, &palette))
            .children(links_view(item, &palette, cx))
            .children(self.menu_answers(item, &palette, cx))
            .children(events_view(&events, &palette))
            .children(self.composer_box(item, window, &palette, cx));
        page.child(header(item, snapshot, &palette))
            .child(body)
            .into_any_element()
    }
}

impl LanePage {
    /// The page of a flow lane: the header that never scrolls, then the overview (the goal, the asks and the
    /// lane's agents), or the conversation of the agent the person opened, and its roles, the other controls
    /// the engine gives it and what it recorded lately in a column beside it.
    fn conversation_page(
        &mut self,
        page: gpui::Stateful<gpui::Div>,
        (item, snapshot): (&Item, &Snapshot),
        window: &mut Window,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let is_identity = super::held_keys::HeldKeys::current(cx).is_identity_shown();
        let item = crate::ui::lane_agents::as_run(item, self.is_plan_review_the_arbiters);
        let pipeline = item.current_pipeline().into_owned();
        let steps = self.steps_row(&item, &pipeline, palette, window, cx);
        let header = flow_header(&item, Some(snapshot), is_identity, steps, palette);
        let body = match self.open_agent.clone() {
            Some(run) => self.chat_of(&run, &item, window, cx).into_any_element(),
            None => self.overview(&item, Some(snapshot), palette, cx),
        };
        let centre = div()
            .debug_selector(|| "lane-centre".into())
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex()
            .flex_col()
            .child(header)
            .child(body);
        let centre = centre.into_any_element();
        let panes = if self.is_column_open {
            let side = self.side_of(&item, pipeline, window, cx);
            super::split::columns(centre, side.into_any_element(), self.is_compact, palette)
                .into_any_element()
        } else {
            centre
        };
        page.overflow_hidden().child(panes).into_any_element()
    }

    /// What stands under the header while no agent is open: the goal, the asks that wait for the person, the
    /// lane's agents and, once it delivered, how. It scrolls; the person writes to an agent from its own page.
    fn overview(
        &mut self,
        item: &Item,
        snapshot: Option<&Snapshot>,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let agents = self.agents_of(item, cx);
        let asks = self.asks(item, snapshot, palette, cx);
        // The words of the sections and of the rows stand on the line of the title above, so the fill of a
        // row reaches past it by the row's own room.
        let column = div()
            .w_full()
            .max_w(sizes::FEED_MAX_WIDTH)
            .mx(-crate::theme::kit::scale::ROW_PADDING)
            .flex()
            .flex_col()
            .gap(sizes::FEED_GAP)
            .children(goal_view(item, palette))
            .children(asks)
            .child(agents)
            .children(delivery_view(item, palette, cx).map(|delivered| {
                div()
                    .px(crate::theme::kit::scale::ROW_PADDING)
                    .child(delivered)
            }));
        div()
            .id("lane-overview")
            .debug_selector(|| "lane-overview".into())
            .flex_1()
            .min_h_0()
            .track_scroll(&self.overview_scroll)
            .overflow_y_scroll()
            .px(sizes::FEED_PADDING_X)
            .py(sizes::HEADER_PADDING_BOTTOM)
            .child(column)
            .into_any_element()
    }

    /// The column of the lane, made when the page first shows it, told where the agent is.
    fn side_of(
        &mut self,
        item: &Item,
        pipeline: Vec<sakura_core::engine::snapshot::PipelineStep>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<LaneSide> {
        let side = match &self.side {
            Some(side) => side.clone(),
            None => {
                let (facts, answers, source) = (
                    self.facts.clone(),
                    self.answers.clone(),
                    self.source.clone(),
                );
                let side = cx.new(|cx| {
                    let mut side = LaneSide::new(item, facts, answers, source, window, cx);
                    if self.is_compact {
                        side.set_compact(true, cx);
                    }
                    side
                });
                self._observers
                    .push(cx.subscribe(&side, |_, _, _: &LaneSideEvent, cx| {
                        cx.emit(LanePageEvent::OpenInbox)
                    }));
                self.side = Some(side.clone());
                side
            }
        };
        side.update(cx, |side, cx| side.set_pipeline(pipeline, cx));
        // Files follows the agent the person has open; the overview has none to follow.
        let place = self
            .open_agent
            .as_ref()
            .and_then(|run| self.chats.get(run))
            .and_then(|chat| chat.update(cx, |chat, cx| chat.place(item.worktree.as_deref(), cx)));
        side.update(cx, |side, cx| side.agent_at(place, cx));
        side
    }

    /// Whether the page has a column at all: a flow lane does, an outside lane's page does not.
    pub fn has_column(&self, cx: &gpui::App) -> bool {
        let snapshot = self.facts.read(cx).snapshot();
        let item = snapshot.and_then(|snapshot| self.item(snapshot));
        item.is_some_and(|item| !item.is_outside())
    }

    /// Whether the column at the right is shown.
    pub fn is_column_open(&self) -> bool {
        self.is_column_open
    }

    /// Hides the column, or brings it back.
    pub fn toggle_column(&mut self, cx: &mut Context<Self>) {
        self.is_column_open = !self.is_column_open;
        cx.notify();
    }

    /// Switches the lane to `preset` from its next step.
    #[cfg(test)]
    pub fn switch_preset(&mut self, preset: &str, cx: &mut Context<Self>) {
        if let Some(side) = self.side.clone() {
            side.update(cx, |side, cx| side.switch_preset(preset, cx));
        }
    }

    /// Shows Files with `path` open, as quick open picked it.
    pub fn open_file(&mut self, path: &str, cx: &mut Context<Self>) {
        self.is_column_open = true;
        if let Some(side) = self.side.clone() {
            side.update(cx, |side, cx| side.open_file(path, cx));
        }
        cx.notify();
    }

    #[cfg(test)]
    pub(super) fn open_file_path(&self, cx: &gpui::App) -> Option<String> {
        self.side.as_ref()?.read(cx).open_file_path(cx)
    }

    /// What quick open reads files from: the lane's folder, when it has one.
    pub fn files_source(&self, cx: &gpui::App) -> Option<std::rc::Rc<dyn super::files::Source>> {
        self.side.as_ref()?.read(cx).source(cx)
    }

    /// Presses the lane action `key` (Resume, Stop) as its menu item does.
    pub fn press_lane_action(&mut self, key: &str, cx: &mut Context<Self>) {
        let snapshot = self.facts.read(cx).shared_snapshot();
        let Some(item) = snapshot.as_ref().and_then(|snapshot| self.item(snapshot)) else {
            return;
        };
        let is_cleanup = key == ACTION_CLEAN_UP && item.agent_state == "stopped";
        if !(key == ACTION_CLOSE || is_cleanup) && self.is_folder_missing(item, cx) {
            return;
        }
        let Some(action) = item.actions.iter().find(|action| action.key == key) else {
            return;
        };
        let (answer_key, target) = (lane_key(&item.id, key), item.id.clone());
        let mut action = action.clone();
        if is_cleanup {
            action.label = action_label(key).to_string();
        }
        self.answers.update(cx, |answers, cx| {
            answers.press(answer_key, action, target, BTreeMap::new(), cx)
        });
    }

    fn is_folder_missing(&self, item: &Item, cx: &App) -> bool {
        !item.is_outside()
            && self
                .facts
                .read(cx)
                .snapshot()
                .is_some_and(|snapshot| snapshot.is_project_folder_missing(&item.repo))
    }

    /// The conversation of the agent `run`, made when the person first opens it.
    fn chat_of(
        &mut self,
        run: &str,
        item: &Item,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<AgentChat> {
        self.keep_recent_chat(run, cx);
        if let Some(chat) = self.chats.get(run) {
            return chat.clone();
        }
        let agents = self.agents_of(item, cx);
        let (source, repo) = (self.source.clone(), item.repo.clone().into());
        let place = (source, repo, item.task().to_string());
        let origin = run.to_string();
        let chat = cx.new(|cx| AgentChat::new(place, origin, agents, window, cx));
        if let Some(text) = self.restored_agent_drafts.remove(run) {
            chat.update(cx, |chat, cx| chat.restore(&text, window, cx));
        }
        let subscription = cx.subscribe(&chat, |page, _, event: &AgentChatEvent, cx| match event {
            AgentChatEvent::Back => {
                page.close_agent(cx);
            }
            AgentChatEvent::DraftChanged => cx.emit(LanePageEvent::DraftChanged),
        });
        self.chat_subscriptions
            .insert(run.to_string(), subscription);
        self.chats.insert(run.to_string(), chat.clone());
        self.sync_activity(cx);
        chat
    }

    fn keep_recent_chat(&mut self, run: &str, cx: &gpui::App) {
        const MAX_CACHED_CHATS: usize = 8;
        if self.recent_chats.back().is_none_or(|last| last != run) {
            self.recent_chats.retain(|one| one != run);
            self.recent_chats.push_back(run.to_string());
        }
        if self.recent_chats.len() <= MAX_CACHED_CHATS {
            return;
        }
        let Some(index) = self.recent_chats.iter().position(|id| {
            id != run
                && self
                    .chats
                    .get(id)
                    .is_some_and(|chat| !chat.read(cx).is_sending())
        }) else {
            return;
        };
        let old = self.recent_chats.remove(index).expect("an entry at index");
        if let Some(chat) = self.chats.remove(&old) {
            let draft = chat.read(cx).draft(cx);
            if !draft.is_empty() {
                self.restored_agent_drafts.insert(old.clone(), draft);
            }
        }
        self.chat_subscriptions.remove(&old);
    }

    /// Opens the conversation of `run` as a press on its row does, for the tests.
    #[cfg(test)]
    pub(super) fn open_agent_for_test(&mut self, run: &str, cx: &mut Context<Self>) {
        self.show_agent(Some(run.to_string()), cx);
    }

    /// Shows the conversation of `run`, or the overview for none, and lets what reads follow what is seen.
    fn show_agent(&mut self, run: Option<String>, cx: &mut Context<Self>) {
        self.open_agent = run;
        self.sync_activity(cx);
        cx.notify();
    }

    pub(super) fn is_compact(&self) -> bool {
        self.is_compact
    }

    pub(super) fn set_compact(&mut self, is_compact: bool, cx: &mut Context<Self>) {
        self.is_compact = is_compact;
        if let Some(side) = &self.side {
            side.update(cx, |side, cx| side.set_compact(is_compact, cx));
        }
        cx.notify();
    }

    #[cfg(test)]
    pub(super) fn is_shown_for_test(&self) -> bool {
        self.is_shown
    }

    /// Tells the page whether the tab shows it. Hidden, its list and its conversations read nothing.
    pub fn set_shown(&mut self, is_shown: bool, cx: &mut Context<Self>) {
        if self.is_shown != is_shown {
            self.is_shown = is_shown;
            self.sync_activity(cx);
        }
    }

    /// The list reads while the page is shown; a conversation reads while it is the one shown on a page that is.
    fn sync_activity(&mut self, cx: &mut Context<Self>) {
        let shown = self.is_shown;
        if let Some(agents) = &self.agents {
            agents.update(cx, |agents, cx| agents.set_active(shown, cx));
        }
        for (run, chat) in &self.chats {
            let is_open = shown && self.open_agent.as_deref() == Some(run.as_str());
            chat.update(cx, |chat, cx| chat.set_active(is_open, cx));
        }
    }

    /// Closes the open agent's conversation and shows the overview, as it was left. False when none was open.
    pub fn close_agent(&mut self, cx: &mut Context<Self>) -> bool {
        let was_open = self.open_agent.is_some();
        self.show_agent(None, cx);
        was_open
    }

    /// The list of `item`'s agents, made when its page first shows it.
    fn agents_of(&mut self, item: &Item, cx: &mut Context<Self>) -> Entity<LaneAgents> {
        if let Some(agents) = &self.agents {
            return agents.clone();
        }
        let (source, facts) = (self.source.clone(), self.facts.clone());
        let (repo, task) = (item.repo.clone().into(), item.task().to_string());
        let agents = cx.new(|cx| LaneAgents::new(source, repo, task, &facts, cx));
        self._observers
            .push(cx.observe(&agents, |page, agents, cx| {
                let runs = agents.read(cx).runs();
                let is_arbiters =
                    sakura_core::engine::agents::is_plan_review_only_the_arbiters(runs);
                if page.is_plan_review_the_arbiters != is_arbiters {
                    page.is_plan_review_the_arbiters = is_arbiters;
                    cx.notify();
                }
            }));
        self._observers.push(
            cx.subscribe(&agents, |page, _, event: &LaneAgentsEvent, cx| {
                let LaneAgentsEvent::Open(run) = event;
                page.show_agent(Some(run.clone()), cx);
            }),
        );
        self.agents = Some(agents.clone());
        agents
    }
}

/// The role that gets the person's message now: the reviewer while one runs, else the role of the step in
/// progress; none when the engine reports neither.
fn addressee(item: &Item) -> Option<SharedString> {
    // A lane that ended has no role at work to write to.
    if !lanes::is_open(item) || item.agent_state == "blocked" {
        return None;
    }
    let review = item
        .reviews
        .iter()
        .find(|review| review.is_on && review.state == "now");
    let role = match (
        review.map(|review| review.id.as_str()),
        item.steps.iter().find(|step| step.state == "now"),
    ) {
        (Some("plan_review"), _) => "plan_reviewer",
        (Some(_), _) => "code_reviewer",
        (None, Some(step)) => match step.id.as_str() {
            "plan" => "planner",
            "build" => "builder",
            "checks" => "evaluator",
            _ => "reporter",
        },
        (None, None) => return None,
    };
    Some(lanes::role_words(role))
}

/// The header row: the lane's state mark and word (an outside lane's title and who it is from), its phase,
/// and what the engine computed of its progress.
fn header(item: &Item, snapshot: &Snapshot, palette: &Palette) -> AnyElement {
    let state = lanes::shown_state(item, Some(snapshot));
    let phase = item
        .progress
        .as_ref()
        .map(|progress| lanes::phase_word(progress.phase_id()));
    let words = item
        .progress
        .as_ref()
        .filter(|progress| progress.total > 0)
        .map(lanes::done_words);
    div()
        .debug_selector(|| "lane-header".into())
        .h(theme::HEADER_BOX_HEIGHT)
        .flex_none()
        .px(theme::CONTENT_PADDING)
        .flex()
        .items_center()
        .gap(theme::GAP_WIDE)
        .border_b_1()
        .border_color(palette.border)
        .child(glyph_column(
            placed_state_glyph("lane-header", state, palette, super::breath::Breath::Still),
            "lane-header-glyph".into(),
        ))
        .child(match item.is_outside() {
            true => title_view(item, palette),
            false => text(theme::TEXT_SMALL, palette.text_secondary)
                .child(tr(state.word))
                .into_any_element(),
        })
        .children(phase.map(|phase| {
            text(theme::TEXT_SMALL, palette.text_secondary)
                .debug_selector(|| "lane-phase".into())
                .child(phase)
        }))
        .child(div().flex_1())
        .children(words.map(|words| {
            text(theme::TEXT_LABEL, palette.text_secondary)
                .font_features(tabular())
                .child(words)
        }))
        .child(lanes::progress(item, "lane-header", false, palette))
        .into_any_element()
}

/// What the lane was asked to do, as its opener wrote it: the first thing the overview says; none from an
/// engine that does not report one.
fn goal_view(item: &Item, palette: &Palette) -> Option<AnyElement> {
    let goal = item
        .goal
        .as_deref()
        .filter(|goal| !goal.trim().is_empty())?;
    let header = kit::section_header("lane-goal-header", tr(Key("agents.goal")), None, palette);
    let words = text(theme::TEXT_BODY, palette.text)
        .debug_selector(|| "lane-goal".into())
        .px(crate::theme::kit::scale::ROW_PADDING)
        .child(super::thread::shown(goal));
    Some(
        div()
            .flex()
            .flex_col()
            .child(header)
            .child(words)
            .into_any_element(),
    )
}

/// The header of a flow lane's page, which never scrolls: its name, its state in a sentence with the mark,
/// and the steps with where the lane stands on them.
fn flow_header(
    item: &Item,
    snapshot: Option<&Snapshot>,
    is_identity: bool,
    steps: Option<AnyElement>,
    palette: &Palette,
) -> AnyElement {
    let state = lanes::shown_state(item, snapshot);
    let sentence = state_sentence(item, snapshot);
    let title = div()
        .flex()
        .items_center()
        .gap(theme::GAP_WIDE)
        .mb(sizes::TITLE_GAP_BELOW)
        .min_w_0()
        .child(
            text(sizes::TITLE_TEXT, palette.text)
                .debug_selector(|| "lane-title".into())
                .line_height(sizes::TITLE_LINE_HEIGHT)
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .whitespace_nowrap()
                .child(lanes::lane_words(item, is_identity)),
        )
        .child(
            div()
                .debug_selector(|| "lane-state".into())
                .min_w_0()
                .flex()
                .items_center()
                .gap(theme::GAP_SMALL)
                .text_size(sizes::STATE_TEXT)
                .text_color(palette.text_secondary)
                .child(glyph_column(
                    placed_state_glyph("lane-header", state, palette, super::breath::Breath::Still),
                    "lane-header-glyph".into(),
                ))
                .child(div().truncate().child(sentence)),
        );
    div()
        .debug_selector(|| "lane-header".into())
        .flex_none()
        .px(sizes::HEADER_PADDING_X)
        .pt(sizes::HEADER_PADDING_TOP)
        .pb(sizes::HEADER_PADDING_BOTTOM)
        .border_b_1()
        .border_color(palette.border)
        .child(title)
        .children(steps)
        .into_any_element()
}

/// The lane's state in a sentence: what needs the person, the step that runs, or that it finished or stopped.
pub(super) fn state_sentence(item: &Item, snapshot: Option<&Snapshot>) -> SharedString {
    let state = lanes::shown_state(item, snapshot);
    let ask = snapshot.and_then(|snapshot| open_asks(item, snapshot).into_iter().next());
    let ask = ask.map(ask_words).or_else(|| gate_words(item));
    match (item.agent_state.as_str(), state.id) {
        (_, StateId::NeedsYou | StateId::Failed) if ask.is_some() => tr_with(
            Key("lane_page.state.needs_you"),
            &[("ask", &ask.unwrap_or_default())],
        ),
        (_, StateId::NeedsYou) => tr(Key("lane_page.state.needs_you_plain")).into(),
        (_, StateId::Working) => running_words(item),
        ("delivered", _) => tr(Key("lane_page.state.finished")).into(),
        ("stopped", _) => tr(Key("lane_page.state.stopped")).into(),
        _ => tr(state.word).into(),
    }
}

/// What an ask says in the header, in plain words: for a spec to approve what to do with it, else the
/// engine's own title.
fn ask_words(wait: &Wait) -> String {
    match super::inbox::is_spec_approval(wait) {
        true => tr(Key("lane_page.ask.spec")).to_string(),
        false => super::inbox::model::ask_title_words(wait),
    }
}

/// What a lane at a gate waits for when the snapshot lists no ask for it yet, from the gate its next command
/// names (`sakura flow approve <task> <gate> ...`): the header never says "Needs you" and leaves the ask out.
fn gate_words(item: &Item) -> Option<String> {
    let mut words = item.next_command.split_whitespace();
    let gate = words
        .by_ref()
        .skip_while(|word| *word != "approve")
        .nth(2)?;
    let key = match gate {
        "spec" => Key("lane_page.ask.spec"),
        "checks" => Key("lane_page.ask.checks"),
        "budget" | "budget-wait" => Key("lane_page.ask.budget"),
        "overlap" => Key("lane_page.ask.overlap"),
        _ => return None,
    };
    Some(tr(key).to_string())
}

/// "Running: Build": the step in progress, or the phase when the engine reports no steps.
pub(super) fn running_words(item: &Item) -> SharedString {
    // The step the bar is on (the lane's whole pipeline) first; the older list of steps only when the engine
    // gives no pipeline: the header and the bar are one source.
    let pipeline = item.current_pipeline();
    let on_the_bar = pipeline.iter().find(|step| step.state == "running");
    let step = item.steps.iter().find(|step| step.state == "now");
    let word = match (on_the_bar, step, item.progress.as_ref()) {
        (Some(running), _, _) => Some(super::step_track::pipeline_words(running)),
        (None, Some(step), _) => Some(step_words(step)),
        (None, None, Some(progress)) => Some(lanes::phase_word(progress.phase_id())),
        (None, None, None) => None,
    };
    match word {
        Some(word) => tr_with(Key("lane_page.state.running"), &[("step", &word)]),
        None => tr(Key("lane_page.state.running_plain")).into(),
    }
}

/// The asks of `item` that wait for the person, oldest first.
fn open_asks<'a>(item: &Item, snapshot: &'a Snapshot) -> Vec<&'a Wait> {
    let now = super::unix_now();
    snapshot
        .waits_for(item)
        .filter(|wait| !wait.is_snoozed_at(now))
        .collect()
}

/// An outside lane's title, and under it whom the lane is from.
fn title_view(item: &Item, palette: &Palette) -> AnyElement {
    div()
        .min_w_0()
        .flex()
        .flex_col()
        .child(
            text(theme::TEXT_SMALL, palette.text)
                .debug_selector(|| "lane-title".into())
                .truncate()
                .child(lanes::lane_words(item, false)),
        )
        .child(
            text(theme::TEXT_LABEL, palette.text_tertiary)
                .debug_selector(|| "lane-from".into())
                .truncate()
                .child(lanes::from_words(item.agent(), item.project())),
        )
        .into_any_element()
}

/// "3 not read yet": how many of the person's answers the lane's agent has not read, while there are any.
fn unread_view(item: &Item, palette: &Palette) -> Option<AnyElement> {
    let count = u64::from(item.unread);
    (count > 0).then(|| {
        let words = say(&Said::new("lane.unread").count(count));
        text(theme::TEXT_SMALL, palette.text_secondary)
            .debug_selector(|| "lane-unread".into())
            .child(words)
            .into_any_element()
    })
}

/// How a delivery reached where it is: a pull request, or a push, to any remote, one on this Mac included.
pub(super) fn delivery_words(kind: &str) -> Key {
    match kind {
        "pull_request" => Key("lane.delivery.pull_request"),
        _ => Key("lane.delivery.push"),
    }
}

/// "Delivered": what a flow lane delivered, from the snapshot: how, the pull request when there is one (its
/// address opens in the default browser), and the branch and the head. The head is a short hash on its own,
/// never cut; only the branch is, with its whole name in a tooltip.
pub(super) fn delivery_view(item: &Item, palette: &Palette, cx: &App) -> Option<AnyElement> {
    let delivery = item.delivery.as_ref()?;
    let head: String = delivery.head.chars().take(DELIVERY_HEAD_CHARS).collect();
    let mut rows = vec![text(theme::TEXT_SMALL, palette.text_secondary)
        .child(tr(delivery_words(&delivery.kind)))
        .into_any_element()];
    if delivery.url.starts_with(LINK_SCHEME) {
        rows.push(pull_request_link(&delivery.url, palette));
    }
    rows.push(branch_and_head(&delivery.branch, head, palette, cx));
    let column = div()
        .debug_selector(|| "lane-delivery".into())
        .flex()
        .flex_col()
        .gap(theme::GAP_TIGHT)
        .children(rows);
    Some(section(
        Key("lane.delivered"),
        "delivery",
        vec![column.into_any_element()],
        palette,
    ))
}

/// The pull request's address, one line, a press opens it.
fn pull_request_link(url: &str, palette: &Palette) -> AnyElement {
    let address = url.to_string();
    super::chrome::mono()
        .id("lane-delivery-link")
        .debug_selector(|| "lane-delivery-place".into())
        .text_size(theme::TEXT_SMALL)
        .text_color(palette.text)
        .truncate()
        .on_click(move |_, _, cx| cx.open_url(&address))
        .child(url.to_string())
        .into_any_element()
}

/// The branch, which gives way when the room is short and says its whole name in a tooltip, and the head
/// beside it, which never does.
fn branch_and_head(branch: &str, head: String, palette: &Palette, cx: &App) -> AnyElement {
    let named = (!branch.is_empty()).then(|| {
        let whole = SharedString::from(branch.to_string());
        let name = super::chrome::mono()
            .id("lane-delivery-branch")
            .debug_selector(|| "lane-delivery-branch".into())
            .min_w_0()
            .truncate()
            .text_size(theme::TEXT_SMALL)
            .text_color(palette.text);
        tip::bind(name, "lane-delivery-branch", Words::plain(whole), cx).child(branch.to_string())
    });
    div()
        .flex()
        .items_center()
        .gap(theme::GAP)
        .children(named)
        .child(
            super::chrome::mono()
                .debug_selector(|| "lane-delivery-head".into())
                .flex_none()
                .text_size(theme::TEXT_SMALL)
                .text_color(palette.text_secondary)
                .child(head),
        )
        .into_any_element()
}

/// The reason and the locations that can be revealed on a blocked card.
pub(super) fn ask_reason(wait: &Wait) -> Option<(String, String)> {
    if wait.kind != super::inbox::model::KIND_BLOCKED {
        return None;
    }
    let plain = super::inbox::model::plain_reason(wait);
    let words = super::inbox::model::plain_text(&plain.words);
    let paths: BTreeSet<_> = [words.as_str(), plain.raw.as_deref().unwrap_or_default()]
        .into_iter()
        .flat_map(|text| {
            super::path_text::path_ranges(text)
                .into_iter()
                .map(move |range| &text[range])
        })
        .filter(|path| path.starts_with('/') || path.starts_with("~/"))
        .collect();
    let locations = paths.into_iter().collect::<Vec<_>>().join("\n");
    let words = if super::plain_error::shows_command_or_path(&words) {
        tr(Key("error.plain.generic")).to_string()
    } else if words == wait.title {
        String::new()
    } else {
        words
    };
    (!(words.is_empty() && locations.is_empty())).then_some((words, locations))
}

/// "Links": the addresses an outside agent gave its lane, each opened in the default browser.
fn links_view(item: &Item, palette: &Palette, cx: &App) -> Option<AnyElement> {
    let links = titled_links("link", &item.links, palette, cx)?;
    Some(section(Key("lane.links"), "links", vec![links], palette))
}

impl LanePage {
    /// The asks of the lane that wait for the person, between the conversation and the composer, each with
    /// the verbs the engine gives it: the same answers as the Inbox's, sent at once.
    fn asks(
        &mut self,
        item: &Item,
        snapshot: Option<&Snapshot>,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let waits = open_asks(item, snapshot?);
        let mut cards: Vec<AnyElement> = waits
            .into_iter()
            .map(|wait| self.ask_card(wait, item, palette, cx))
            .collect();
        cards.extend(self.menu_answers(item, palette, cx));
        if cards.is_empty() {
            return None;
        }
        Some(
            div()
                .debug_selector(|| "lane-asks".into())
                .flex_none()
                .px(crate::theme::kit::scale::ROW_PADDING)
                .flex()
                .flex_col()
                .gap(theme::GAP)
                .children(cards)
                .into_any_element(),
        )
    }

    fn offer_resume_note(&self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(snapshot) = self.facts.read(cx).shared_snapshot() else {
            return false;
        };
        let Some(item) = self.item(&snapshot) else {
            return false;
        };
        let ask = open_asks(item, &snapshot).into_iter().find(|wait| {
            asks::verbs(wait, Some(item))
                .primary
                .is_some_and(|verb| verb.say == asks::Say::Resume && verb.takes_note)
        });
        let Some(ask) = ask else {
            return false;
        };
        self.inbox
            .update(cx, |inbox, cx| {
                inbox.answer_ask(&ask.id, asks::Answer::Verb(asks::Say::Resume), window, cx);
            })
            .is_ok()
    }

    /// What the engine said to a Resume or a Stop pressed from the menu: its question to confirm, or its word.
    fn menu_answers(&self, item: &Item, palette: &Palette, cx: &App) -> Vec<AnyElement> {
        MENU_ACTIONS
            .iter()
            .filter_map(|key| {
                let key = lane_key(&item.id, key);
                let step = self.answers.read(cx).step(&key)?;
                (!matches!(step, Step::PreparingStop))
                    .then(|| step_view(&self.answers, &key, step, palette, cx))
            })
            .collect()
    }

    /// One ask: what it says and one button for each answer the engine allows.
    fn ask_card(
        &mut self,
        wait: &Wait,
        item: &Item,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let verbs = asks::verbs(wait, Some(item));
        let is_missing = self.is_folder_missing(item, cx);
        let mut buttons: Vec<AnyElement> = Vec::new();
        let is_held = self.is_held(&wait.id, cx);
        let given = verbs
            .primary
            .iter()
            .chain(&verbs.secondary)
            .filter(|_| !is_held);
        for (position, verb) in given.enumerate() {
            let id = format!("ask-{}-{position}", wait.id);
            let is_disabled = is_missing && asks::is_folder_action(wait, &verb.action.key);
            buttons.push(self.verb_button(id, &wait.id, verb, is_disabled, cx));
        }
        if is_missing && !is_held {
            for (_, label) in verbs.missing_blocked_actions(wait) {
                let id: SharedString = format!("ask-{}-{}", wait.id, buttons.len()).into();
                buttons.push(
                    Button::new(id.clone())
                        .debug_selector(move || id.to_string())
                        .custom(secondary_variant(cx))
                        .child(tr(label))
                        .disabled(true)
                        .tooltip(tr(Key(sakura_core::engine::cli::PROJECT_FOLDER_MISSING)))
                        .into_any_element(),
                );
            }
        }
        if verbs.reply.is_some() && !is_held {
            for (position, choice) in verbs.choices.iter().enumerate() {
                let id = format!("ask-{}-choice-{position}", wait.id);
                buttons.push(self.choice_button(id, &wait.id, choice, cx));
            }
            buttons.push(self.reply_button(&wait.id, cx));
        }
        let note = self
            .inbox
            .update(cx, |inbox, cx| {
                inbox.resume_note_row(&wait.id, is_missing, palette, cx)
            })
            .ok()
            .flatten();
        let answered = self.decision(&wait.id, palette, cx);
        let selector = format!("lane-ask-{}", wait.id);
        kit::card(selector, palette)
            .child(
                text(theme::TEXT_SMALL, palette.text)
                    .child(asks::kind_words(wait.original_kind()))
                    .font_weight(theme::WEIGHT_MEDIUM),
            )
            .child(
                text(theme::TEXT_SMALL, palette.text)
                    .child(super::inbox::model::ask_title_words(wait)),
            )
            .children(note)
            .children(is_missing.then(|| {
                text(theme::TEXT_SMALL, palette.text_secondary)
                    .child(tr(Key(sakura_core::engine::cli::PROJECT_FOLDER_MISSING)))
            }))
            .children(ask_reason(wait).map(|(reason, locations)| {
                div()
                    .children((!reason.is_empty()).then(|| {
                        text(theme::TEXT_SMALL, palette.text_secondary)
                            .debug_selector(|| "lane-ask-reason".into())
                            .child(crate::ui::thread::shown(&reason))
                    }))
                    .children((!locations.is_empty()).then(|| {
                        super::plain_error::details_view(
                            "lane-ask-details",
                            &wait.id,
                            &locations,
                            &self.answers,
                            palette,
                            cx,
                        )
                    }))
            }))
            .children(crate::ui::ask_findings::findings_view(
                "lane-ask-findings",
                &wait.id,
                &wait.findings,
                palette,
            ))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(theme::GAP_SMALL)
                    .children(buttons),
            )
            .children(answered)
            .into_any_element()
    }

    /// Whether the Inbox holds an answer to the ask `wait` for Undo: the card then offers no other.
    fn is_held(&self, wait: &str, cx: &App) -> bool {
        let held = self.inbox.read_with(cx, |inbox, _| inbox.held_words(wait));
        held.is_ok_and(|words| words.is_some())
    }

    /// What the answer to the ask `wait` says: the words of the hold while the Inbox holds it for Undo, else
    /// what the engine told of it.
    fn decision(&self, wait: &str, palette: &Palette, cx: &App) -> Option<AnyElement> {
        let held = self
            .inbox
            .read_with(cx, |inbox, _| inbox.held_words(wait))
            .ok()
            .flatten();
        if let Some(words) = held {
            return Some(
                text(theme::TEXT_SMALL, palette.text_secondary)
                    .child(words)
                    .into_any_element(),
            );
        }
        let step = self.answers.read(cx).step(wait)?.clone();
        Some(step_view(&self.answers, wait, &step, palette, cx))
    }

    /// A button that answers the ask `wait` with `say` through the Inbox, which holds the answer under its
    /// Undo toast and sends it after: the same path as the Inbox's own buttons and the master's proposals.
    fn verb_button(
        &self,
        id: String,
        wait: &str,
        verb: &asks::Verb,
        is_disabled: bool,
        cx: &App,
    ) -> AnyElement {
        let (selector, say) = (id.clone(), verb.say);
        let (inbox, wait) = (self.inbox.clone(), wait.to_string());
        let button = Button::new(SharedString::from(id))
            .debug_selector(move || selector)
            .custom(secondary_variant(cx))
            .bg(crate::theme::Theme::palette(cx).fill)
            .child(verb_words(verb))
            .disabled(is_disabled);
        if is_disabled {
            return button
                .tooltip(tr(Key(sakura_core::engine::cli::PROJECT_FOLDER_MISSING)))
                .into_any_element();
        }
        button
            .on_click(move |_, window, cx| {
                let held = inbox.update(cx, |inbox, cx| {
                    inbox.answer_ask(&wait, asks::Answer::Verb(say), window, cx)
                });
                if held.is_err() {
                    eprintln!("sakura: the Inbox is gone, the ask {wait} was not answered");
                }
            })
            .into_any_element()
    }

    /// A button that answers the question `wait` with `choice` through the Inbox: held under its Undo toast
    /// and sent after, like every other answer.
    fn choice_button(&self, id: String, wait: &str, choice: &str, cx: &App) -> AnyElement {
        let selector = id.clone();
        let (inbox, wait, chosen) = (self.inbox.clone(), wait.to_string(), choice.to_string());
        Button::new(SharedString::from(id))
            .debug_selector(move || selector)
            .custom(secondary_variant(cx))
            .bg(crate::theme::Theme::palette(cx).fill)
            .child(SharedString::from(choice.to_string()))
            .on_click(move |_, window, cx| {
                let answer = asks::Answer::Choice(chosen.clone());
                let held =
                    inbox.update(cx, |inbox, cx| inbox.answer_ask(&wait, answer, window, cx));
                if held.is_err() {
                    eprintln!("sakura: the Inbox is gone, the ask {wait} was not answered");
                }
            })
            .into_any_element()
    }

    /// "Reply": answering in words is what the Inbox is for.
    fn reply_button(&self, ask: &str, cx: &mut Context<Self>) -> AnyElement {
        let selector = format!("ask-{ask}-reply");
        Button::new(SharedString::from(selector.clone()))
            .debug_selector(move || selector)
            .custom(secondary_variant(cx))
            .bg(crate::theme::Theme::palette(cx).fill)
            .child(tr(Key("inbox.reply")))
            .on_click(cx.listener(|_, _, _, cx| cx.emit(LanePageEvent::OpenInbox)))
            .into_any_element()
    }

    /// "Roles": each role of the lane and the model it runs on; an outside lane has none.
    fn roles(&self, item: &Item, palette: &Palette) -> Option<AnyElement> {
        if item.models.is_empty() {
            return None;
        }
        let rows = item.models.iter().map(|(role, model)| {
            let selector = format!("role-{role}");
            div()
                .debug_selector(move || selector)
                .h(theme::ROW_HEIGHT)
                .flex()
                .items_center()
                .gap(theme::GAP_WIDE)
                .child(
                    div()
                        .w(theme::LANE_ROLE_WIDTH)
                        .flex_none()
                        .text_color(palette.text_secondary)
                        .child(lanes::role_words(role)),
                )
                .child(div().flex_1().min_w_0().truncate().child(model.clone()))
                .into_any_element()
        });
        Some(section(Key("lane.roles"), "roles", rows.collect(), palette))
    }
}

/// A titled list.
fn section(
    words: Key,
    selector: &'static str,
    rows: Vec<AnyElement>,
    palette: &Palette,
) -> AnyElement {
    div()
        .debug_selector(move || format!("section-{selector}"))
        .flex()
        .flex_col()
        .child(
            div()
                .pb(theme::GAP_SMALL)
                .text_size(theme::TEXT_LABEL)
                .text_color(palette.text_tertiary)
                .child(tr(words)),
        )
        .children(rows)
        .into_any_element()
}

/// The lines the engine recorded of this lane, oldest first, as sentences.
pub(super) fn events_of(item: &Item, snapshot: Option<&Snapshot>) -> Vec<SharedString> {
    let scope = match item.is_outside() {
        true => format!("outside/{}", item.task()),
        false => format!("{}/{}", item.project(), item.task()),
    };
    let events = snapshot.into_iter().flat_map(|snapshot| &snapshot.events);
    events
        .filter(|event| event.scope == scope)
        .map(|event| event_words(&event.text))
        .collect()
}

/// "Recently": what the engine recorded of the lane, when it recorded anything.
fn events_view(events: &[SharedString], palette: &Palette) -> Option<AnyElement> {
    if events.is_empty() {
        return None;
    }
    let rows = events.iter().map(|line| {
        text(theme::TEXT_SMALL, palette.text_secondary)
            .py(theme::GAP_TIGHT)
            .child(line.clone())
            .into_any_element()
    });
    Some(section(
        Key("lane.events"),
        "events",
        rows.collect(),
        palette,
    ))
}
