//! The Master tab: the conversation with the engine's master. The engine writes how the master starts
//! (`sakura master spec --json`: the adapter, the session options with its cage, the MCP servers) and
//! decides every permission request it makes (`sakura master decide --json`); this page only runs the
//! adapter as told and shows the conversation. The first message starts a new session. A saved session
//! loads when its tab is shown, and a request the engine denies never reaches the person.

use super::answers::{step_view, Answers};
use super::chrome::text;
use super::inbox::model::{Answer, Say};
use super::inbox::InboxPage;
use super::kit::StateGlyph;
use super::kit::{scroll_dropdown, ScrollMenu, ScrollMenuItem};
use super::lanes::glyph_of;
use super::master_accounts::{self, MasterAccount};
use super::master_agent::{self, MasterAgent};
use super::master_binding::{lane_named, proposal_of};
use super::master_picks;
use super::page_bar::PageControls;
use super::thread::{
    Host, LaneAsk, LaneCard, Launched, Login, MasterKit, MasterWorld, Proposal, ProposalAsk,
    Readiness, SharedStart, StartChoice, ThreadView, ThreadViewEvent,
};
use crate::data::engine_facts::EngineFacts;
use crate::data::master_history::{MasterHistory, Past};
use crate::data::sessions::Sessions;
use crate::data::source::Source;
use crate::icons;
use crate::l10n::{say, tr, Key};
use crate::prefs::Preferences;
use crate::theme::{self, Theme};
use gpui::{
    div, AnyElement, App, AppContext as _, Context, Entity, EventEmitter, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, Styled as _, Subscription, WeakEntity,
    Window,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::{Icon, Sizable as _};
use sakura_acp::connection::{
    Agent, AgentConfig, CodexConfig, CodexLogin, Deadlines, LoginEnd, SessionSetup,
};
use sakura_acp::model::Choice;
use sakura_acp::policy::decider::{Decider, Verdict};
use sakura_acp::thread::{Problem, ProblemKind};
use sakura_core::engine::accounts::Readiness as AccountReadiness;
use sakura_core::engine::master::{Decision, GooseMasterRequest, MasterAcp, MasterSpec};
use sakura_core::engine::snapshot::Snapshot;
use sakura_core::engine::tracks::ConversationKey;
use sakura_core::error::{BridgeError, BridgeResult};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::watch;

/// The menu's last entry, which opens the sheet that adds an account.
const ADD_ACCOUNT: Key = Key("account_add.title");

/// The id of the account the master runs on, read when a conversation starts.
type AccountCell = Rc<RefCell<String>>;

/// Whether Sakura's own Codex home holds a login, as the agent menu asks it each time it is drawn.
type LoginCheck = Rc<RefCell<Box<dyn Fn() -> bool>>>;

/// What is said when the engine names no adapter to run the master with.
const NO_ADAPTER: Key = Key("master.no_adapter");
/// What is said, before the engine's own words, when the engine could not start the master.
const NOT_STARTED: Key = Key("master.not_started");
/// The folder inside the preferences' where the master's own files are kept.
const MASTER_FOLDER: &str = "master";
/// The name the Claude master's session is kept under: what the earlier app called its Master tab, so a
/// session kept before still opens. The other agent's is [`MasterAgent::session_name`].
pub(super) const MASTER_SESSION: &str = "@master";

type SpecRead = watch::Receiver<Option<BridgeResult<MasterSpec>>>;
type Preview = Rc<RefCell<Option<MasterPreview>>>;

struct MasterPreview {
    account: Option<String>,
    preset: Option<String>,
    spec: SpecRead,
}

/// What the page asks of the window around it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MasterEvent {
    /// A card of a lane the master started was clicked: show the lane with this key.
    OpenLane(String),
    /// A card of a note the master proposes was asked to open the Inbox.
    OpenInbox,
    ResumeHistory(MasterAgent, Past),
    OpenTrack(String),
}

/// The page of the pinned Master tab.
pub struct MasterPage {
    view: Entity<ThreadView>,
    /// The Leeway account the conversation runs on: its id, empty for the host's own. Fixed for the conversation.
    account: AccountCell,
    /// The engine's facts: the accounts the menu lists.
    facts: Entity<EngineFacts>,
    /// The engine, requested the spec when the Codex login is opened.
    source: Source,
    /// Whether Sakura's Codex home has a login: the menu offers it while it has none.
    #[cfg(test)]
    has_login: LoginCheck,
    /// The Codex login being made in the browser, while there is one; the menu offers to cancel it.
    login: Option<CodexLogin>,
    login_task: Option<gpui::Task<()>>,
    is_login_running: Rc<Cell<bool>>,
    /// The earlier conversations the person left by starting fresh, of each agent; shared by every tab's page.
    histories: [MasterHistory; MasterAgent::ALL.len()],
    /// What tells this page's conversation from the other tabs' in the kept sessions: empty for the first.
    slot: String,
    /// The agent the master is now; the host reads it when it starts the conversation.
    agent: Rc<Cell<MasterAgent>>,
    /// The model and the mode picked in the agent chip before the first message.
    start: SharedStart,
    preview: Preview,
    _preview_task: Option<gpui::Task<()>>,
    /// Whether the conversation kept from before was requested, which happens once, when first shown and
    /// once the kept sessions are known.
    has_asked_kept: bool,
    /// The last list of models each agent gave, which the chip's menu offers; filled from the engine's own
    /// lists while an agent has given none.
    kept_models: KeptModels,
    goose_slot: Rc<RefCell<String>>,
    /// The engine's lists being read.
    _catalogue: Option<gpui::Task<()>>,
    /// Whether the engine was asked since the facts last changed.
    is_catalogue_asked: bool,
    /// Whether the conversation has no account chosen yet, so it starts on the one Settings sets for new
    /// conversations once the engine lists the accounts.
    is_account_pending: bool,
    sessions: Sessions,
    /// The wait for the kept sessions, while there is one.
    _loading: Option<gpui::Task<()>>,
    /// The engine's facts and the person's answers, which the cards follow.
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<MasterEvent> for MasterPage {}

impl MasterPage {
    /// The page over the engine `source`; the master's session is kept in `sessions`, and its cards
    /// follow the engine's `facts` and the person's `answers`.
    pub fn new(
        source: Source,
        sessions: Sessions,
        deps: (Entity<EngineFacts>, Entity<Answers>, Entity<InboxPage>),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let histories = kept_histories(&sessions, cx.background_executor());
        Self::in_slot(
            source,
            sessions,
            deps,
            (histories, String::new()),
            window,
            cx,
        )
    }

    /// The page of a tab of its own: its conversation is kept under `slot`, and `histories` are the ones the
    /// other tabs' pages share.
    pub fn in_slot(
        source: Source,
        sessions: Sessions,
        (facts, answers, inbox): (Entity<EngineFacts>, Entity<Answers>, Entity<InboxPage>),
        (histories, slot): ([MasterHistory; MasterAgent::ALL.len()], String),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let remembered = Preferences::try_current(cx).map_or(MasterAgent::Claude, |preferences| {
            MasterAgent::remembered(&preferences, None)
        });
        let agent = Rc::new(Cell::new(remembered));
        let remembered_account = Preferences::try_current(cx)
            .map(|preferences| preferences.master_account.clone())
            .unwrap_or_default();
        let is_account_pending = remembered_account.is_empty();
        let account: AccountCell = Rc::new(RefCell::new(remembered_account));
        let start = SharedStart::default();
        if let Some(kept) = Preferences::try_current(cx) {
            start.update(|start| *start = saved_start(&kept, &account.borrow(), remembered));
        }
        let kept_models: KeptModels = Rc::new(RefCell::new(kept_models_of(cx)));
        let preview = Preview::default();
        let goose_slot = Rc::new(RefCell::new(goose_tab_slot(&slot)));
        let login_source = source.clone();
        let is_login_running = Rc::new(Cell::new(false));
        let has_login: LoginCheck =
            Rc::new(RefCell::new(Box::new(master_agent::app_has_codex_login)));
        let mut host = host(
            source,
            sessions.clone(),
            (
                agent.clone(),
                account.clone(),
                start.clone(),
                preview.clone(),
            ),
            (slot.clone(), goose_slot.clone()),
            kept_models.clone(),
        );
        let (login_agent, login_running) = (agent.clone(), is_login_running.clone());
        host.login = Box::new(move |_| match login_agent.get() {
            MasterAgent::Codex => Login::Codex {
                is_waiting: login_running.get(),
            },
            MasterAgent::Claude | MasterAgent::Goose => Login::Mac { account: None },
        });
        let view = cx.new(|cx| ThreadView::new(host, window, cx));
        let world = EngineWorld {
            facts: facts.clone(),
            answers: answers.clone(),
            inbox: inbox.downgrade(),
            page: cx.weak_entity(),
        };
        let menu = {
            let account = account.clone();
            let (agent, start, preview) = (agent.clone(), start.clone(), preview.clone());
            let kept = kept_models.clone();
            let has_login = has_login.clone();
            let is_login_running = is_login_running.clone();
            Rc::new(move |session: &sakura_acp::model::SessionChoices| {
                let kept = kept.borrow();
                let own = master_agent::account_models(&kept, agent.get(), &account.borrow());
                let mut chosen = start.get();
                if agent.get() == MasterAgent::Claude {
                    let preview = preview.borrow();
                    let read = preview.as_ref().map(|preview| preview.spec.borrow());
                    let spec = read
                        .as_deref()
                        .and_then(Option::as_ref)
                        .and_then(|result| result.as_ref().ok());
                    chosen = claude_start(spec, chosen, own);
                }
                let mut menu = master_agent::agent_menu(agent.get(), &chosen, own, session);
                if agent.get() == MasterAgent::Codex && !(has_login.borrow())() {
                    menu.groups
                        .insert(0, master_agent::login_group(is_login_running.get()));
                }
                menu
            })
        };
        let kept_start = start.clone();
        view.update(cx, |view, cx| {
            let kit = MasterKit::new(Rc::new(world)).with_start(start, menu);
            view.with_master(kit, window, cx)
        });
        let (learned_models, learned_agent) = (kept_models.clone(), agent.clone());
        let subscriptions = vec![
            cx.observe(&view, move |page, view, cx| {
                let view = view.read(cx);
                let session = view.session_choices().clone();
                remember_models(&learned_models, learned_agent.get(), &session, cx);
                page.remember_picks(&session, cx);
            }),
            cx.observe(&facts, |page, _, cx| {
                remember_account_names(&page.account_entries(cx), cx);
                page.is_catalogue_asked = false;
                page.ask_for_the_engine_s_models(cx);
                page.refresh_cards(cx);
                page.bind_listed_provider(cx);
                page.start_on_the_defaults_when_due(cx);
            }),
            cx.observe(&answers, |page, _, cx| page.refresh_cards(cx)),
            cx.subscribe(&view, |page, _, event: &ThreadViewEvent, cx| match event {
                ThreadViewEvent::StartFresh => page.start_fresh(cx),
                ThreadViewEvent::ProjectChosen => page.follow_project(cx),
                ThreadViewEvent::CodexLoginRequested => page.open_codex_login(cx),
                ThreadViewEvent::KeptSessionLost(session) => page.forget_lost_session(session),
                _ => {}
            }),
        ];
        Self {
            view,
            account,
            facts,
            source: login_source,
            #[cfg(test)]
            has_login: has_login.clone(),
            login: None,
            login_task: None,
            is_login_running,
            histories,
            slot,
            agent,
            start: kept_start,
            preview,
            _preview_task: None,
            has_asked_kept: false,
            kept_models: kept_models.clone(),
            goose_slot,
            _catalogue: None,
            is_catalogue_asked: false,
            is_account_pending,
            sessions,
            _loading: None,
            _subscriptions: subscriptions,
        }
    }

    /// Leaves the conversation as it is, for History, and starts an empty one: the next message opens a new
    /// session.
    pub fn start_fresh(&mut self, cx: &mut Context<Self>) {
        *self.goose_slot.borrow_mut() = format!("sakura-{}", uuid::Uuid::new_v4());
        self.clear_codex_login();
        self.archive_open(cx);
        self.preview.borrow_mut().take();
        self.forget_open_session();
        self.view.update(cx, |view, cx| view.start_over(cx));
        // After the conversation is put away: putting it away clears what a new one starts with.
        self.start_on_the_defaults(cx);
        cx.notify();
    }

    /// The account, the model and the effort Settings sets for new conversations; the account is the first Claude
    /// account, else the first, while none is set.
    fn start_on_the_defaults(&mut self, cx: &App) {
        let entries = self.account_entries(cx);
        let kept = Preferences::try_current(cx);
        let chosen = kept
            .as_ref()
            .and_then(|kept| kept.new_master_account.clone());
        let engine_default = self
            .facts
            .read(cx)
            .snapshot()
            .and_then(|snapshot| snapshot.engine.as_ref())
            .and_then(|engine| engine.master_account.as_ref());
        let Some(entry) =
            master_accounts::default_with_engine(&entries, chosen.as_deref(), engine_default)
        else {
            return;
        };
        let id = &entry.id;
        if self.agent.get() != entry.agent || *self.account.borrow() != *id {
            self.clear_codex_login();
        }
        self.agent.set(entry.agent);
        *self.account.borrow_mut() = id.clone();
        self.start.update(|start| {
            *start = kept.as_ref().map_or_else(StartChoice::default, |kept| {
                saved_start(kept, id, entry.agent)
            })
        });
        self.is_account_pending = false;
    }

    fn bind_listed_provider(&mut self, cx: &App) {
        if self.view.read(cx).phase() != &super::thread::Phase::Idle
            || self.kept_identity().is_some()
        {
            return;
        }
        let account = self.account.borrow().clone();
        if let Some(entry) = self
            .account_entries(cx)
            .iter()
            .find(|entry| entry.id == account)
        {
            if entry.agent != self.agent.get() {
                self.clear_codex_login();
                self.agent.set(entry.agent);
                self.start.update(|start| *start = StartChoice::default());
            }
        }
    }

    /// A conversation with no account yet starts on the defaults once the accounts are listed, unless it
    /// has begun or has a conversation kept from before under the host's own account.
    fn start_on_the_defaults_when_due(&mut self, cx: &mut Context<Self>) {
        if !self.is_account_pending || !self.sessions.is_loaded() {
            return;
        }
        let has_begun = self.view.read(cx).first_message().is_some();
        let has_kept = self.slot.is_empty()
            && self
                .sessions
                .stored(self.agent.get().session_name(), "")
                .is_some();
        if has_begun || has_kept {
            self.is_account_pending = false;
            return;
        }
        self.start_on_the_defaults(cx);
        cx.notify();
    }

    /// The account of the conversation, for the tests.
    #[cfg(test)]
    pub fn account_for_test(&self) -> String {
        self.account.borrow().clone()
    }

    /// What was picked before the first message, for the tests.
    #[cfg(test)]
    pub fn start_for_test(&self) -> StartChoice {
        self.start.get()
    }

    /// The conversation kept for this page's tab is on the account and the agent it was kept under: they are
    /// not asked of anyone again after a relaunch.
    fn take_up_kept_account(&mut self) {
        if self.kept_identity().is_some() {
            return;
        }
        let Some((task, account)) = self.sessions.find_slot(&self.slot) else {
            return;
        };
        self.clear_codex_login();
        let Some(agent) = MasterAgent::ALL
            .into_iter()
            .find(|agent| agent.session_name() == task)
        else {
            return;
        };
        self.agent.set(agent);
        *self.account.borrow_mut() = account;
        self.is_account_pending = false;
    }

    fn prepare_master_spec(&mut self, cx: &mut Context<Self>) {
        if self.agent.get() != MasterAgent::Claude
            || self.view.read(cx).phase() != &super::thread::Phase::Idle
        {
            return;
        }
        let Some(snapshot) = self.facts.read(cx).snapshot() else {
            return;
        };
        let account = master_accounts::engine_argument(&self.account.borrow());
        let preset = snapshot
            .engine
            .as_ref()
            .and_then(|engine| engine.preset.clone());
        if self
            .preview
            .borrow()
            .as_ref()
            .is_some_and(|preview| preview.account == account && preview.preset == preset)
        {
            return;
        }
        let pending = self.source.master_spec(account.clone());
        let (writer, spec) = watch::channel(None);
        *self.preview.borrow_mut() = Some(MasterPreview {
            account,
            preset,
            spec,
        });
        let view = self.view.downgrade();
        self._preview_task = Some(cx.spawn(async move |_, cx| {
            writer.send_replace(Some(pending.await));
            drop(view.update(cx, |_, cx| cx.notify()));
        }));
    }

    /// While an agent has no list of models, asks the engine for the lists its programs have: the chip's menu
    /// offers them before any session has started.
    fn ask_for_the_engine_s_models(&mut self, cx: &mut Context<Self>) {
        let is_missing = MasterAgent::ALL.iter().any(|agent| {
            let kept = self.kept_models.borrow();
            let list = kept.get(agent.code());
            list.is_none_or(|list| list.iter().all(|model| model.efforts.is_empty()))
        });
        if !is_missing || self._catalogue.is_some() || self.is_catalogue_asked {
            return;
        }
        self.is_catalogue_asked = true;
        let pending = self.source.rules();
        self._catalogue = Some(cx.spawn(async move |page, cx| {
            let rules = pending.await;
            drop(page.update(cx, |page, cx| {
                page._catalogue = None;
                if let Ok(rules) = rules {
                    page.take_in_the_engine_s_models(&rules, cx);
                }
            }));
        }));
    }

    fn take_in_the_engine_s_models(
        &mut self,
        rules: &sakura_core::engine::rules::Rules,
        cx: &mut Context<Self>,
    ) {
        master_agent::take_in_catalogue(&mut self.kept_models.borrow_mut(), rules);
        cx.notify();
    }

    /// The earlier conversations, which the pages of every tab share.
    pub fn histories(&self) -> [MasterHistory; MasterAgent::ALL.len()] {
        self.histories.clone()
    }

    /// The tab of this page was closed: its conversation stays whole in History.
    pub fn leave(&self, cx: &App) {
        self.archive_open(cx);
        self.forget_open_session();
    }

    /// The session this page's conversation has is not the one the next message takes up.
    fn forget_open_session(&self) {
        let account = self.account.borrow();
        let key = master_accounts::session_key(&account, &self.slot);
        self.sessions.forget(self.agent.get().session_name(), &key);
    }

    fn forget_lost_session(&self, session: &str) {
        let key = master_accounts::session_key(&self.account.borrow(), &self.slot);
        let task = self.agent.get().session_name();
        if self.sessions.stored(task, &key).as_deref() == Some(session) {
            self.sessions.forget(task, &key);
        }
    }

    /// The accounts the master can run on, flat and in the engine's order, each under its Leeway name.
    pub fn account_entries(&self, cx: &App) -> Vec<MasterAccount> {
        self.facts
            .read(cx)
            .snapshot()
            .map(master_accounts::listed)
            .unwrap_or_default()
    }

    /// Opens the sheet that adds an account, over the page.
    pub fn open_add_account(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (source, facts) = (self.source.clone(), self.facts.clone());
        super::account_add::open(source, facts, window, cx);
    }

    /// The agent the master is now.
    #[cfg(test)]
    pub fn agent(&self) -> MasterAgent {
        self.agent.get()
    }

    /// Opens a new conversation on the account `id`, which runs `agent`, as the menu at the top of the page does:
    /// the conversation that was open stays whole in History, and a conversation's account is never changed.
    /// Choosing the account that has the conversation does nothing.
    pub fn press_account(&mut self, agent: MasterAgent, id: &str, cx: &mut Context<Self>) {
        let entries = self.account_entries(cx);
        if self.current_account(&entries).as_deref() == Some(id) && agent == self.agent.get() {
            return;
        }
        self.clear_codex_login();
        self.archive_open(cx);
        self.forget_open_session();
        if Preferences::try_current(cx).is_some() {
            let id = id.to_string();
            Preferences::change(cx, |preferences| {
                agent.remember(preferences, None);
                preferences.master_account = id;
            });
        }
        self.agent.set(agent);
        *self.goose_slot.borrow_mut() = format!("sakura-{}", uuid::Uuid::new_v4());
        *self.account.borrow_mut() = id.to_string();
        // What the account had open is not taken up: this is a new conversation.
        self.forget_open_session();
        self.preview.borrow_mut().take();
        self.view.update(cx, |view, cx| view.start_over(cx));
        if let Some(kept) = Preferences::try_current(cx) {
            self.start
                .update(|start| *start = saved_start(&kept, id, agent));
        }
        cx.notify();
    }

    /// The id of the account the conversation runs on: the one chosen, or while none is, the first account of
    /// the agent that has it.
    fn current_account(&self, entries: &[MasterAccount]) -> Option<String> {
        let chosen = self.account.borrow().clone();
        if !chosen.is_empty() {
            return Some(chosen);
        }
        let agent = self.agent.get();
        let first = entries.iter().find(|entry| entry.agent == agent);
        first.map(|entry| entry.id.clone())
    }

    /// Decides whether the menu finds a Codex login, for the tests.
    #[cfg(test)]
    pub fn set_login_check_for_test(&self, check: impl Fn() -> bool + 'static) {
        *self.has_login.borrow_mut() = Box::new(check);
    }

    /// The agent for the snapshot drive: the first account that runs it, as the menu's entry would be pressed.
    #[cfg(feature = "snapshot")]
    pub fn set_agent(&mut self, agent: MasterAgent, cx: &mut Context<Self>) {
        let entries = self.account_entries(cx);
        let first = entries.iter().find(|entry| entry.agent == agent);
        let id = first.map(|entry| entry.id.clone()).unwrap_or_default();
        self.press_account(agent, &id, cx);
    }

    /// The login entry of the agent menu was pressed: Codex's own login starts in Sakura's own Codex home, with
    /// the browser opened by the app; pressed while one is being made, it cancels it. Nothing is typed anywhere
    /// and the app never sees a credential.
    fn open_codex_login(&mut self, cx: &mut Context<Self>) {
        if let Some(login) = self.login.as_ref() {
            return login.cancel();
        }
        if self.login_task.is_some() || self.agent.get() != MasterAgent::Codex {
            return;
        }
        self.is_login_running.set(true);
        self.view.update(cx, |_, cx| cx.notify());
        self.login_task = Some(super::codex_login::run(
            self.source
                .master_spec(master_accounts::engine_argument(&self.account.borrow())),
            None,
            |page: &mut Self, login| page.login = Some(login),
            Self::end_codex_login,
            cx,
        ));
    }

    fn clear_codex_login(&mut self) {
        self.login_task = None;
        self.login = None;
        self.is_login_running.set(false);
    }

    /// The login is over: a login in the home starts Codex in the tab that had none; one that did not work is
    /// said in the tab in words.
    fn end_codex_login(&mut self, end: &LoginEnd, cx: &mut Context<Self>) {
        self.clear_codex_login();
        self.view
            .update(cx, |view, cx| match master_agent::login_end_words(end) {
                Some(words) => view.account_refused(words, cx),
                None => view.restart_after_sign_in(cx),
            });
    }

    /// The person chose a project: the conversation's messages are about it. The agent stays the
    /// conversation's own.
    fn follow_project(&mut self, _cx: &mut Context<Self>) {}

    fn history(&self) -> &MasterHistory {
        let at = MasterAgent::ALL
            .iter()
            .position(|agent| *agent == self.agent.get());
        &self.histories[at.unwrap_or_default()]
    }

    pub(super) fn kept_identity(&self) -> Option<ConversationKey> {
        let agent = self.agent.get();
        let account = self.account.borrow();
        let slot = master_accounts::session_key(&account, &self.slot);
        let session = self.sessions.stored(agent.session_name(), &slot)?;
        Some(master_accounts::conversation_key(agent, &account, session))
    }

    #[cfg(test)]
    pub fn resume(&mut self, session: &str, cx: &mut Context<Self>) {
        if let Some((agent, past)) = self
            .pasts()
            .into_iter()
            .find(|(_, past)| past.session == session)
        {
            cx.emit(MasterEvent::ResumeHistory(agent, past));
        }
    }

    pub(super) fn take_history(&self, agent: MasterAgent, past: &Past) -> Option<Past> {
        let at = MasterAgent::ALL
            .iter()
            .position(|candidate| *candidate == agent)?;
        self.histories[at].take_for(&past.account, &past.session)
    }

    pub(super) fn resume_here(&mut self, agent: MasterAgent, past: &Past, cx: &mut Context<Self>) {
        let Some(past) = self.take_history(agent, past) else {
            return;
        };
        self.archive_open(cx);
        self.forget_open_session();
        self.clear_codex_login();
        self.agent.set(agent);
        *self.account.borrow_mut() = past.account.clone();
        self.has_asked_kept = true;
        self.is_account_pending = false;
        let key = master_accounts::session_key(&past.account, &self.slot);
        self.sessions
            .store(agent.session_name(), &key, &past.session);
        self.preview.borrow_mut().take();
        self.view.update(cx, |view, cx| view.start_over(cx));
        self.restore_picks(&past.session, cx);
        self.view.update(cx, |view, cx| view.load_kept(cx));
        cx.notify();
    }

    fn pasts(&self) -> Vec<(MasterAgent, Past)> {
        let current = self.agent.get();
        let histories = MasterAgent::ALL.into_iter().zip(&self.histories);
        let active = self
            .history()
            .list()
            .into_iter()
            .map(move |past| (current, past));
        active
            .chain(
                histories
                    .filter(|(agent, _)| *agent != current)
                    .flat_map(|(agent, history)| {
                        history.list().into_iter().map(move |past| (agent, past))
                    }),
            )
            .collect()
    }

    /// The earlier conversations, for the tests.
    #[cfg(test)]
    pub fn history_for_test(&self) -> Vec<Past> {
        self.history().list()
    }

    /// Adds an earlier conversation, for the tests.
    #[cfg(test)]
    pub fn add_past_for_test(&self, session: &str, first_message: &str) {
        self.history().add(Past::new(session, first_message));
    }

    fn refresh_cards(&mut self, cx: &mut Context<Self>) {
        self.view.update(cx, |view, cx| view.refresh_cards(cx));
    }

    /// Keeps the open conversation in History, when it has begun and has a session.
    pub(super) fn archive_open(&self, cx: &App) {
        let account = self.account.borrow().clone();
        let key = master_accounts::session_key(&account, &self.slot);
        let session = self.sessions.stored(self.agent.get().session_name(), &key);
        // A conversation that has a session is kept even when its words are not on screen yet (it is still being
        // loaded): it is named "The conversation" until it is opened again.
        let first = self.view.read(cx).first_message().unwrap_or_default();
        if let Some(session) = session {
            self.history()
                .add(Past::new(&session, &first).on_account(&account));
        }
    }

    /// Keeps what the open conversation runs on, as the agent says it, under its session: it resumes on it.
    fn remember_picks(&self, session: &sakura_acp::model::SessionChoices, cx: &mut App) {
        let account = self.account.borrow().clone();
        let key = master_accounts::session_key(&account, &self.slot);
        let stored = self.sessions.stored(self.agent.get().session_name(), &key);
        let picks = master_picks::picks_of(session);
        let Some(stored) = stored.filter(|_| picks != StartChoice::default()) else {
            return;
        };
        self.start.update(|start| {
            start.model = picks.model.clone().or(start.model.take());
            start.effort = picks.effort.clone().or(start.effort.take());
            start.mode = picks.mode.clone().or(start.mode.take());
            if let Some(uses_session_effort) = session.uses_session_effort {
                start.uses_session_effort = uses_session_effort;
            }
        });
        let (stored, mut json) = (
            self.picks_key(&stored),
            master_picks::to_json(&self.start.get()),
        );
        if self.agent.get() == MasterAgent::Goose {
            json["goose_slot"] = self.goose_slot.borrow().as_str().into();
        }
        if Preferences::try_current(cx)
            .is_none_or(|kept| kept.master_picks.get(&stored) == Some(&json))
        {
            return;
        }
        Preferences::change(cx, |preferences| {
            preferences.master_picks.insert(stored, json);
        });
    }

    /// Makes what `session` ran on the choice its next start begins with.
    fn restore_picks(&self, session: &str, cx: &App) {
        let key = self.picks_key(session);
        let picks = Self::kept_picks(&key, cx);
        self.start.update(|start| *start = picks);
        if self.agent.get() == MasterAgent::Goose {
            let stored = Preferences::try_current(cx).and_then(|prefs| {
                prefs
                    .master_picks
                    .get(&key)?
                    .get("goose_slot")?
                    .as_str()
                    .map(str::to_string)
            });
            *self.goose_slot.borrow_mut() = stored.unwrap_or_else(|| goose_tab_slot(&self.slot));
        }
    }

    fn picks_key(&self, session: &str) -> String {
        if self.agent.get() == MasterAgent::Goose {
            format!("goose:{}:{session}", self.account.borrow())
        } else {
            session.to_string()
        }
    }

    /// What the preferences kept for `session`; nothing when they kept nothing.
    fn kept_picks(session: &str, cx: &App) -> StartChoice {
        let kept =
            Preferences::try_current(cx).and_then(|kept| kept.master_picks.get(session).cloned());
        kept.map(|kept| master_picks::from_json(&kept))
            .unwrap_or_default()
    }

    /// What a reopened `session` starts with, for the tests.
    #[cfg(test)]
    pub fn kept_picks_for_test(session: &str, cx: &App) -> StartChoice {
        Self::kept_picks(session, cx)
    }

    /// The session the open conversation is kept under, for the tests.
    #[cfg(test)]
    pub fn stored_session_for_test(&self) -> String {
        let account = self.account.borrow().clone();
        let key = master_accounts::session_key(&account, &self.slot);
        let stored = self.sessions.stored(self.agent.get().session_name(), &key);
        stored.expect("a kept session")
    }

    /// The conversation: the desk fills its composer, and the tests and scripted checks read it.
    pub fn conversation(&self) -> Entity<ThreadView> {
        self.view.clone()
    }
}

impl Render for MasterPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.has_asked_kept && self.sessions.is_loaded() {
            self.has_asked_kept = true;
            self.take_up_kept_account();
            self.bind_listed_provider(cx);
            let account = self.account.borrow().clone();
            let key = master_accounts::session_key(&account, &self.slot);
            let kept = self.sessions.stored(self.agent.get().session_name(), &key);
            if let Some(kept) = kept {
                self.restore_picks(&kept, cx);
            }
            self.view.update(cx, |view, cx| view.load_kept(cx));
        } else if !self.has_asked_kept && self._loading.is_none() {
            // The kept sessions are still being read: no session is known yet, which is not "none kept".
            // The page draws again when they are, and asks then.
            let sessions = self.sessions.clone();
            self._loading = Some(cx.spawn(async move |this, cx| {
                sessions.when_loaded().await;
                drop(this.update(cx, |_, cx| cx.notify()));
            }));
        }
        self.ask_for_the_engine_s_models(cx);
        self.start_on_the_defaults_when_due(cx);
        self.prepare_master_spec(cx);
        self.view.clone()
    }
}

impl MasterPage {
    /// What the account chip says: the name given to the account in the app (see [`master_accounts::chip_name`]),
    /// and the program's name only when there is no account at all.
    pub fn account_chip_label(&self, cx: &App) -> Option<String> {
        let entries = self.account_entries(cx);
        let conversation = self.current_account(&entries);
        let kept = Preferences::try_current(cx);
        let saved = kept.as_ref().map(|kept| &kept.master_account_names);
        let new_conversations = kept
            .as_ref()
            .and_then(|kept| kept.new_master_account.as_deref());
        let empty = std::collections::BTreeMap::new();
        let name = master_accounts::chip_name(
            &entries,
            conversation.as_deref().filter(|id| !id.is_empty()),
            saved.unwrap_or(&empty),
            new_conversations,
        );
        // Nothing is named before the engine has answered; once it has and lists no account, the chip is where
        // one is added.
        let is_read = self.facts.read(cx).snapshot().is_some();
        name.or_else(|| is_read.then(|| tr(ADD_ACCOUNT).to_string()))
    }
}

impl MasterPage {
    fn track_control(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let key = self.kept_identity()?;
        let snapshot = self.facts.read(cx).snapshot()?;
        let page = cx.weak_entity();
        let title = self.view.read(cx).first_message().unwrap_or_default();
        let title = Past::new(&key.session, &title).title;
        let title = if title.is_empty() {
            tr(THE_CONVERSATION).into()
        } else {
            title
        };
        super::track_page::membership_control(
            "master-track",
            super::track_page::Membership {
                snapshot,
                current: snapshot.track_of_conversation(&key),
                inputs: super::track_page::conversation_inputs(&key),
                title: Some(title),
                can_link: true,
                can_unlink: true,
                open_track: Rc::new(move |track, cx| {
                    drop(page.update(cx, |_, cx| cx.emit(MasterEvent::OpenTrack(track))));
                }),
            },
            &self.facts,
            &self.source,
            cx,
        )
    }
}

impl PageControls for MasterPage {
    /// At the header's right: the menu with start fresh and the earlier conversations. The agent is chosen in
    /// the composer of an empty conversation.
    fn controls(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let palette = Theme::palette(cx);
        let (page, past) = (cx.entity().downgrade(), self.pasts());
        let menu = Button::new("master-menu")
            .custom(super::chrome::quiet_variant(&palette, cx))
            .h(theme::CONTROL_HEIGHT)
            .px(theme::CONTROL_PADDING_X)
            .child(Icon::new(icons::OVERFLOW).small());
        let menu = scroll_dropdown(menu, gpui::Anchor::TopRight, {
            let page = page.clone();
            move |menu, _, _| conversation_menu(menu, &page, &past)
        });
        div()
            .flex()
            .items_center()
            .gap(theme::GAP)
            .children(self.track_control(cx))
            .child(
                div()
                    .debug_selector(|| "master-menu".to_string())
                    .child(menu),
            )
            .into_any_element()
    }
}

/// "Start fresh", then the earlier conversations, newest first.
fn conversation_menu(
    menu: ScrollMenu,
    page: &WeakEntity<MasterPage>,
    past: &[(MasterAgent, Past)],
) -> ScrollMenu {
    let fresh = page.clone();
    let start = ScrollMenuItem::new(tr(START_FRESH)).on_click(move |_, _, cx| {
        fresh.update(cx, |page, cx| page.start_fresh(cx)).ok();
    });
    let menu = menu.item(start).separator().label(tr(HISTORY)).pin_header();
    if past.is_empty() {
        return menu.item(ScrollMenuItem::new(tr(NO_HISTORY)).disabled(true));
    }
    past.iter().fold(menu, |menu, (agent, past)| {
        let (page, agent, resume) = (page.clone(), *agent, past.clone());
        let title = match past.title.is_empty() {
            true => tr(THE_CONVERSATION).to_string(),
            false => past.title.clone(),
        };
        menu.item(ScrollMenuItem::new(title).on_click(move |_, _, cx| {
            drop(page.update(cx, |_, cx| {
                cx.emit(MasterEvent::ResumeHistory(agent, resume.clone()))
            }));
        }))
    })
}

/// The engine as the master's cards see it: the lanes and waits of its facts, the person's answers, and
/// the window that shows a lane.
struct EngineWorld {
    facts: Entity<EngineFacts>,
    answers: Entity<Answers>,
    /// The Inbox, whose held answer a card's Approve and Decline use.
    inbox: WeakEntity<InboxPage>,
    page: WeakEntity<MasterPage>,
}

impl EngineWorld {
    fn snapshot<'a>(&self, cx: &'a App) -> Option<&'a Snapshot> {
        self.facts.read(cx).snapshot()
    }
}

impl MasterWorld for EngineWorld {
    fn lane(&self, ask: &LaneAsk, cx: &App) -> LaneCard {
        let item = self
            .snapshot(cx)
            .and_then(|snapshot| lane_named(snapshot, &ask.result));
        let Some(item) = item else {
            return LaneCard {
                key: None,
                title: String::new(),
                project: String::new(),
                phase: String::new(),
                progress: None,
                glyph: StateGlyph::NotStarted,
            };
        };
        let progress = item.progress.as_ref().filter(|progress| progress.total > 0);
        LaneCard {
            key: Some(item.lane_key()),
            title: item.name().to_string(),
            project: item.project().to_string(),
            phase: progress
                .map(|progress| progress.phase_id().to_string())
                .unwrap_or_default(),
            progress: progress.map(|progress| (progress.done, progress.total)),
            glyph: glyph_of(item),
        }
    }

    fn open_lane(&self, key: &str, _: &mut Window, cx: &mut App) {
        let key = key.to_string();
        self.page
            .update(cx, |_, cx| cx.emit(MasterEvent::OpenLane(key)))
            .ok();
    }

    fn proposal(&self, ask: &ProposalAsk, cx: &App) -> Proposal {
        match self.snapshot(cx) {
            Some(snapshot) => proposal_of(snapshot, &ask.result),
            None => Proposal::Unbound,
        }
    }

    fn open_inbox(&self, _: &mut Window, cx: &mut App) {
        self.page
            .update(cx, |_, cx| cx.emit(MasterEvent::OpenInbox))
            .ok();
    }

    fn decide(&self, wait: &str, approve: bool, window: &mut Window, cx: &mut App) {
        let say = if approve { Say::Approve } else { Say::Decline };
        let held = self.inbox.update(cx, |inbox, cx| {
            inbox.answer_ask(wait, Answer::Verb(say), window, cx)
        });
        if held.is_err() {
            eprintln!("sakura: the Inbox is gone, the proposal {wait} was not answered");
        }
    }

    fn decision(&self, wait: &str, cx: &App) -> Option<AnyElement> {
        let palette = Theme::palette(cx);
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
        let step = self.answers.read(cx).step(wait)?;
        Some(step_view(&self.answers, wait, step, &palette, cx))
    }

    fn accounts(
        &self,
        cx: &App,
    ) -> Option<(
        SharedString,
        super::thread::agent_menu::ChipGroup,
        super::thread::agent_menu::MenuChoice,
    )> {
        use super::thread::agent_menu::{ChipGroup, MenuChoice};
        let page = self.page.upgrade()?;
        let master = page.read(cx);
        let label = master.account_chip_label(cx)?;
        let accounts = master.account_entries(cx);
        let current = master.current_account(&accounts);
        let choices = accounts
            .into_iter()
            .map(|account| {
                let page = self.page.clone();
                MenuChoice {
                    label: account.label.into(),
                    is_on: current.as_deref() == Some(&account.id),
                    on_pick: Rc::new(move |_, cx| {
                        drop(page.update(cx, |page, cx| {
                            page.press_account(account.agent, &account.id, cx)
                        }));
                    }),
                }
            })
            .collect();
        let page = self.page.clone();
        let add = MenuChoice {
            label: tr(ADD_ACCOUNT).into(),
            is_on: false,
            on_pick: Rc::new(move |window, cx| {
                drop(page.update(cx, |page, cx| page.open_add_account(window, cx)));
            }),
        };
        Some((
            label.into(),
            ChipGroup {
                title: Some(tr(Key("master.menu.account")).into()),
                choices,
            },
            add,
        ))
    }

    fn projects(&self, cx: &App) -> Vec<String> {
        let projects = self
            .snapshot(cx)
            .map(|snapshot| snapshot.projects.as_slice());
        projects
            .unwrap_or_default()
            .iter()
            .map(|project| project.name.clone())
            .collect()
    }
}

const START_FRESH: Key = Key("master.menu.start_fresh");
const HISTORY: Key = Key("master.menu.history");
const NO_HISTORY: Key = Key("master.menu.no_history");
const THE_CONVERSATION: Key = Key("master.menu.label");

/// Whether the master's conversation comes back after a restart: the person's choice in Settings, on until
/// they turn it off.
fn is_restored(cx: &gpui::App) -> bool {
    crate::prefs::Preferences::try_current(cx)
        .is_none_or(|preferences| preferences.is_master_restored)
}

/// The earlier conversations kept in the sessions' folder, of each agent.
fn kept_histories(
    sessions: &Sessions,
    executor: &gpui::BackgroundExecutor,
) -> [MasterHistory; MasterAgent::ALL.len()] {
    MasterAgent::ALL.map(|agent| {
        let folder = sessions.folder().map(|folder| folder.join(MASTER_FOLDER));
        let folder = match (folder, agent.history_folder()) {
            (Some(folder), Some(own)) => Some(folder.join(own)),
            (folder, _) => folder,
        };
        MasterHistory::new(folder.as_deref(), executor)
    })
}

/// What the conversation needs from its holder: how to start the master (the agent the person chose when it
/// starts), where its session is kept, and nothing of accounts, sign-in or terminals, which the master has
/// none of.
fn host(
    source: Source,
    sessions: Sessions,
    (agent, account, start, preview): (Rc<Cell<MasterAgent>>, AccountCell, SharedStart, Preview),
    (slot, goose_slot): (String, Rc<RefCell<String>>),
    kept_models: KeptModels,
) -> Host {
    let workspace = Arc::new(Mutex::new(String::new()));
    let (kept, keeping, refused, refusing) = (
        sessions.clone(),
        sessions.clone(),
        sessions.clone(),
        sessions,
    );
    let (kept_agent, keeping_agent) = (agent.clone(), agent.clone());
    let (kept_account, keeping_account) = (account.clone(), account.clone());
    let (kept_slot, keeping_slot) = (slot.clone(), slot.clone());
    let shown_workspace = workspace.clone();
    Host {
        launch: Box::new(move |_| {
            launch(
                &source,
                (&workspace, &preview, &kept_models),
                (agent.get(), account.borrow().clone()),
                start.get(),
                &goose_slot.borrow(),
            )
        }),
        stored_session: Box::new(move |cx| {
            // Turned off in Settings, General: the conversation kept from before is not brought back.
            is_restored(cx)
                .then(|| {
                    let key = master_accounts::session_key(&kept_account.borrow(), &kept_slot);
                    kept.stored(kept_agent.get().session_name(), &key)
                })
                .flatten()
        }),
        store_session: Box::new(move |session, _| {
            let key = master_accounts::session_key(&keeping_account.borrow(), &keeping_slot);
            keeping.store(keeping_agent.get().session_name(), &key, session)
        }),
        refusals: Box::new(move |session, _| refused.refusals(session)),
        keep_refusal: Box::new(move |session, words, _| refusing.keep_refusal(session, words)),
        login: Box::new(|_| Login::Mac { account: None }),
        accounts: Box::new(|_| Vec::new()),
        choose_account: Box::new(|_, _| {}),
        sign_in: Box::new(|_, _| {}),
        open_terminal: Box::new(|_, _| {}),
        workspace: Box::new(move |_| {
            let held = shown_workspace.lock();
            held.unwrap_or_else(PoisonError::into_inner).clone()
        }),
        readiness: Box::new(|_| Readiness::Ready),
        retry_creation: Box::new(|_, _| {}),
    }
}

/// Starts the master on the spec shown in the composer, or reads one when resuming a session.
fn launch(
    source: &Source,
    (workspace, preview, kept_models): (&Arc<Mutex<String>>, &Preview, &KeptModels),
    (agent, account): (MasterAgent, String),
    chosen: StartChoice,
    slot: &str,
) -> Launched {
    let runtime = source.runtime().handle();
    let account = master_accounts::engine_argument(&account);
    let (agent, events) = match agent {
        MasterAgent::Claude => {
            let preview = preview
                .borrow()
                .as_ref()
                .filter(|preview| preview.account == account)
                .map(|preview| preview.spec.clone());
            let models = kept_models
                .borrow()
                .get(agent.code())
                .cloned()
                .unwrap_or_default();
            Agent::start_resolving(
                resolve(
                    source.clone(),
                    workspace.clone(),
                    (account, preview),
                    chosen,
                    models,
                ),
                runtime,
            )
        }
        MasterAgent::Codex => Agent::start_codex_resolving(resolve_codex(
            source.clone(),
            workspace.clone(),
            (account, chosen),
        )),
        MasterAgent::Goose => {
            let Some(account) = account else {
                return Launched::Unavailable;
            };
            let slot = slot.to_string();
            let pending = source.goose_master_spec(GooseMasterRequest {
                account: account.clone(),
                slot: slot.clone(),
                model: chosen.model,
                effort: chosen.effort,
                is_fresh: false,
            });
            let workspace = workspace.clone();
            let message = tr(Key("goose.master_unavailable")).to_string();
            Agent::start_master_resolving(
                async move {
                    let spec = pending.await.map_err(not_started)?;
                    goose_launch(spec.acp, &account, &slot, &workspace, message)
                },
                runtime,
            )
        }
    };
    Launched::Started(agent, events)
}

fn goose_tab_slot(tab: &str) -> String {
    if tab.is_empty() {
        "sakura-main".into()
    } else {
        format!("sakura-tab-{tab}")
    }
}

fn goose_launch(
    launch: Option<MasterAcp>,
    account: &str,
    slot: &str,
    workspace: &Mutex<String>,
    message: String,
) -> Result<MasterAcp, Problem> {
    let unavailable = |detail| Problem {
        kind: ProblemKind::NotStarted,
        detail,
    };
    let mut launch = launch.ok_or_else(|| unavailable(message.clone()))?;
    if launch.readiness != AccountReadiness::Ready {
        let reason = super::account_words::reason_words(launch.reason.as_deref())
            .map_or(message, str::to_string);
        return Err(unavailable(reason));
    }
    let Some(cwd) = launch.cwd.as_ref().filter(|cwd| cwd.is_absolute()) else {
        return Err(unavailable(message));
    };
    if launch.protocol_version != 1
        || launch.harness != "goose"
        || launch.slot != slot
        || !launch
            .argv
            .first()
            .is_some_and(|program| program.starts_with('/'))
        || launch.launch_id.as_ref().is_none_or(String::is_empty)
        || !launch.binding.as_ref().is_some_and(|binding| {
            binding.harness == "goose"
                && binding.account == account
                && binding.context == "master"
                && !binding.storage_namespace.is_empty()
        })
    {
        return Err(unavailable(message));
    }
    *workspace.lock().unwrap_or_else(PoisonError::into_inner) = cwd.display().to_string();
    launch.env.entry("PATH".into()).or_insert_with(|| {
        let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
        sakura_core::engine::cli::known_path(home.as_deref())
    });
    Ok(launch)
}

/// The last list of models each agent gave, by the agent's code.
type KeptModels = Rc<RefCell<std::collections::BTreeMap<String, Vec<sakura_acp::model::Choice>>>>;

/// The lists the preferences kept.
pub(super) fn kept_models_of(
    cx: &App,
) -> std::collections::BTreeMap<String, Vec<sakura_acp::model::Choice>> {
    let kept = Preferences::try_current(cx).map(|kept| kept.master_models.clone());
    let lists = kept.unwrap_or_default().into_iter();
    lists
        .map(|(agent, list)| (agent, master_agent::models_from_json(&list)))
        .collect()
}

/// Keeps the list of models a started session of `agent` offers, as the last the agent gave: the menu of
/// an empty conversation shows it until the next session says it again.
fn remember_models(
    kept: &KeptModels,
    agent: MasterAgent,
    session: &sakura_acp::model::SessionChoices,
    cx: &mut App,
) {
    let Some(models) = session
        .models
        .as_ref()
        .filter(|models| !models.choices.is_empty())
    else {
        return;
    };
    if kept.borrow().get(agent.code()) == Some(&models.choices) {
        return;
    }
    kept.borrow_mut()
        .insert(agent.code().to_string(), models.choices.clone());
    if Preferences::try_current(cx).is_some() {
        let json = master_agent::models_to_json(&models.choices);
        Preferences::change(cx, |preferences| {
            preferences
                .master_models
                .insert(agent.code().to_string(), json);
        });
    }
}

/// Keeps the names the engine gives the accounts, so the account chip can say them while the accounts are read
/// again. Nothing is kept while the engine lists none.
fn remember_account_names(entries: &[MasterAccount], cx: &mut App) {
    if entries.is_empty() || Preferences::try_current(cx).is_none() {
        return;
    }
    let names = entries
        .iter()
        .map(|entry| (entry.id.clone(), entry.label.clone()))
        .collect();
    Preferences::change(cx, |preferences| preferences.master_account_names = names);
}

async fn resolve(
    source: Source,
    workspace: Arc<Mutex<String>>,
    (account, preview): (Option<String>, Option<SpecRead>),
    chosen: StartChoice,
    models: Vec<Choice>,
) -> Result<AgentConfig, Problem> {
    let prepared = match preview {
        Some(mut preview) => preview
            .wait_for(Option::is_some)
            .await
            .ok()
            .and_then(|spec| spec.clone()),
        None => None,
    };
    let spec = match prepared {
        Some(Ok(spec)) => spec,
        _ => source.master_spec(account).await.map_err(not_started)?,
    };
    *workspace.lock().unwrap_or_else(PoisonError::into_inner) = spec.cwd.display().to_string();
    let start = claude_start(Some(&spec), chosen, &models);
    config_of(&spec, master_decider(source), start)
}

fn saved_start(kept: &Preferences, account: &str, agent: MasterAgent) -> StartChoice {
    let applies = match kept.new_master_account.as_deref() {
        Some(chosen) => chosen == account,
        None => agent == MasterAgent::Claude,
    };
    if applies {
        return StartChoice {
            model: kept.new_master_model.clone(),
            effort: kept.new_master_effort.clone(),
            ..StartChoice::default()
        };
    }
    StartChoice::default()
}

fn claude_start(
    spec: Option<&MasterSpec>,
    mut chosen: StartChoice,
    models: &[Choice],
) -> StartChoice {
    if chosen.mode.is_none() {
        chosen.mode = spec.map(|spec| spec.start_mode.clone());
    }
    if chosen.model.is_none() {
        chosen.model = spec.and_then(MasterSpec::model).map(str::to_string);
        if chosen.effort.is_none() && !chosen.uses_session_effort {
            chosen.effort = spec.and_then(MasterSpec::effort).map(str::to_string);
        }
    }
    if chosen.effort.is_none() && !chosen.uses_session_effort {
        chosen.effort = chosen
            .model
            .as_deref()
            .and_then(|id| master_agent::current_model(models, Some(id)))
            .and_then(|model| model.default_effort.clone());
    }
    chosen.effort = chosen.effort.filter(|effort| !effort.is_empty());
    chosen
}

/// Asks the engine how to start the master and starts Codex from it, as [`resolve`] does for Claude: Codex's
/// program is found by `SAKURA_CODEX`, the master's PATH or the app's own, and the engine decides what it asks.
async fn resolve_codex(
    source: Source,
    workspace: Arc<Mutex<String>>,
    (account, chosen): (Option<String>, StartChoice),
) -> Result<CodexConfig, Problem> {
    let spec = source
        .master_spec(account.clone())
        .await
        .map_err(not_started)?;
    validate_codex_account(&spec, account.as_deref())?;
    let mut held = workspace.lock().unwrap_or_else(PoisonError::into_inner);
    *held = spec.cwd.display().to_string();
    drop(held);
    let named = std::env::var(master_agent::CODEX_VARIABLE).ok();
    let app_path = std::env::var("PATH").ok();
    let program = master_agent::find_codex(&spec, named.as_deref(), app_path.as_deref());
    let config = master_agent::codex_config_of(&spec, program.as_deref(), master_decider(source))?;
    Ok(CodexConfig {
        model: chosen.model.filter(|model| !model.is_empty()),
        effort: chosen.effort,
        collaboration: chosen.mode.filter(|mode| mode == "plan"),
        ..config
    })
}

fn validate_codex_account(spec: &MasterSpec, requested: Option<&str>) -> Result<(), Problem> {
    let wrong_provider = !spec.account_provider.is_empty() && spec.account_provider != "codex";
    let wrong_account = !spec.account.is_empty() && requested.is_some_and(|id| id != spec.account);
    if wrong_provider || wrong_account {
        return Err(Problem {
            kind: ProblemKind::NotStarted,
            detail: tr(Key("master.codex.account_mismatch")).to_string(),
        });
    }
    Ok(())
}

/// How the adapter is started from the engine's spec: its command, its environment without the markers
/// of the app's own surroundings, the session options and MCP servers as the engine wrote them, the mode
/// set before the first prompt, and the engine as the one who answers its permission requests.
fn config_of(
    spec: &MasterSpec,
    decider: Decider,
    start: StartChoice,
) -> Result<AgentConfig, Problem> {
    if spec.argv.is_empty() {
        let detail = match spec.adapter_problem.is_empty() {
            true => tr(NO_ADAPTER).to_string(),
            false => spec.adapter_problem.clone(),
        };
        return Err(Problem {
            kind: ProblemKind::NotStarted,
            detail,
        });
    }
    let mode = start.mode.as_deref().unwrap_or(&spec.start_mode);
    let meta = claude_meta(spec, &start, mode);
    let mode = mode.to_string();
    Ok(AgentConfig {
        command: spec.argv.clone(),
        env: env_of(spec),
        workspace: spec.cwd.clone(),
        deadlines: Deadlines::default(),
        max_unread_bytes: sakura_acp::transport::MAX_UNREAD_BYTES,
        asks_beyond_workspace: false,
        session: SessionSetup {
            meta: Some(meta),
            mcp_servers: spec.mcp_servers.clone(),
            start_mode: Some(mode),
            start_model: start.model,
            start_effort: start.effort,
            requires_start_choices: true,
            closes_sessions: spec.close_sessions,
            env_allow: Some(spec.env_allow.clone()),
            unset_env: spec.unset_env.clone(),
            decider: Some(decider),
        },
    })
}

fn claude_meta(spec: &MasterSpec, start: &StartChoice, mode: &str) -> serde_json::Value {
    let mut meta = spec.session_meta();
    let effort = start.effort.as_deref().filter(|value| *value != "default");
    let launch_effort = effort.filter(|_| !start.uses_session_effort);
    let options = &mut meta["claudeCode"]["options"];
    if options.is_null() {
        *options = serde_json::json!({});
    }
    if let Some(fields) = options.as_object_mut() {
        for (key, value) in [("model", start.model.as_deref()), ("effort", launch_effort)] {
            match value {
                Some(value) => {
                    fields.insert(key.to_string(), value.into());
                }
                None => {
                    fields.remove(key);
                }
            }
        }
        // The adapter owns the SDK's permissionMode and forwards extraArgs to the subprocess.
        options["extraArgs"]["permission-mode"] = mode.into();
        options["allowDangerouslySkipPermissions"] = false.into();
        let settings = &mut options["settings"];
        if settings.is_null() || settings.is_object() {
            settings["permissions"]["defaultMode"] = mode.into();
            if let Some(effort) = effort {
                settings["effortLevel"] = effort.into();
            }
        }
    }
    meta
}

/// The adapter's environment: the engine's own, with the PATH the engine built from absolute folders
/// (an app opened from Finder has none of its own), which it must not lack.
pub(super) fn env_of(spec: &MasterSpec) -> Vec<(String, String)> {
    let mut env = spec.env.clone();
    if !spec.path.is_empty() {
        env.insert("PATH".to_string(), spec.path.clone());
    }
    env.into_iter().collect()
}

/// The engine as the one who decides: each request is put to `sakura master decide`. A request the engine
/// cannot answer is denied: nothing the engine has not allowed or asked about runs.
pub fn master_decider(source: Source) -> Decider {
    Decider::new(move |request| {
        let asking = source.master_decide(request);
        Box::pin(async move {
            match asking.await {
                Ok(decided) => verdict_of(decided.decision),
                Err(error) => {
                    eprintln!(
                        "sakura: the engine did not decide a request of the master: {error:?}"
                    );
                    Verdict::Deny
                }
            }
        })
    })
}

fn verdict_of(decision: Decision) -> Verdict {
    match decision {
        Decision::Allow => Verdict::Allow,
        Decision::Deny => Verdict::Deny,
        Decision::Ask => Verdict::Ask,
    }
}

/// The localized reason a Master session could not start.
fn not_started(error: BridgeError) -> Problem {
    let said = say(&error.said);
    Problem {
        kind: ProblemKind::NotStarted,
        detail: format!("{} {said}", tr(NOT_STARTED)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_start_refusal_omits_raw_engine_diagnostics() {
        let error = BridgeError {
            code: sakura_core::error::ErrorCode::EngineFailed,
            said: sakura_core::said::Said::new("error.engine.unreadable_answer"),
            details: Some("/fixture/private-program --internal".into()),
        };
        let message = not_started(error);
        assert!(!message.detail.contains("/fixture/private-program"));
    }

    fn default_config(spec: &MasterSpec) -> Result<AgentConfig, Problem> {
        config_of(
            spec,
            decider(),
            claude_start(Some(spec), StartChoice::default(), &[]),
        )
    }

    fn spec(argv: &[&str]) -> MasterSpec {
        MasterSpec {
            acp: None,
            argv: argv.iter().map(|word| word.to_string()).collect(),
            env: [("SAKURA_ENGINE_HOME".to_string(), "/engine".to_string())].into(),
            cwd: "/engine/master".into(),
            options: serde_json::json!({ "model": "haiku" }),
            mcp_servers: vec![serde_json::json!({ "name": "sakura-master" })],
            start_mode: "default".to_string(),
            close_sessions: true,
            unset_env: vec!["CLAUDECODE".to_string()],
            env_allow: vec!["PATH".to_string(), "HOME".to_string()],
            node: String::new(),
            adapter: String::new(),
            adapter_problem: String::new(),
            path: String::new(),
            account: String::new(),
            account_provider: String::new(),
            account_folder: String::new(),
            codex_home: String::new(),
            system_prompt_append: "You are the master.".to_string(),
        }
    }

    fn decider() -> Decider {
        Decider::new(|_| Box::pin(async { Verdict::Ask }))
    }

    #[test]
    fn the_adapter_uses_the_engine_spec_and_the_master_start_mode() {
        let config = default_config(&spec(&["/opt/acp"])).expect("a configuration");
        assert_eq!(config.command, ["/opt/acp"]);
        assert_eq!(
            config.env,
            [("SAKURA_ENGINE_HOME".to_string(), "/engine".to_string())]
        );
        assert_eq!(config.workspace, std::path::Path::new("/engine/master"));
        assert!(
            !config.asks_beyond_workspace,
            "the engine decides, not the policy"
        );
        let session = config.session;
        assert_eq!(
            session.meta,
            Some(serde_json::json!({
                "claudeCode": { "options": {
                    "model": "haiku",
                    "allowDangerouslySkipPermissions": false,
                    "extraArgs": { "permission-mode": "default" },
                    "settings": { "permissions": { "defaultMode": "default" } },
                } },
                "systemPrompt": { "append": "You are the master." },
            }))
        );
        assert_eq!(session.mcp_servers.len(), 1);
        assert_eq!(session.start_mode.as_deref(), Some("default"));
        assert_eq!(
            session.start_model.as_deref(),
            Some("haiku"),
            "the spec's model is the one the session is put on, over the adapter's own first model"
        );
        assert!(session.closes_sessions);
        assert_eq!(session.unset_env, ["CLAUDECODE"]);
        assert_eq!(
            session.env_allow,
            Some(vec!["PATH".to_string(), "HOME".to_string()]),
            "the adapter inherits those names only"
        );
        assert!(session.decider.is_some());
    }

    #[test]
    fn the_adapter_starts_from_the_engine_s_absolute_interpreter_and_adapter_and_its_path() {
        let mut named = spec(&["/opt/homebrew/bin/node", "/opt/acp/claude-agent-acp"]);
        named.path = "/opt/homebrew/bin:/usr/bin".to_string();
        let config = default_config(&named).expect("a configuration");
        assert_eq!(
            config.command,
            ["/opt/homebrew/bin/node", "/opt/acp/claude-agent-acp"],
            "the interpreter and the adapter are started by absolute path, as the spec says"
        );
        let path = config.env.iter().find(|(name, _)| name == "PATH");
        assert_eq!(
            path.map(|(_, value)| value.as_str()),
            Some("/opt/homebrew/bin:/usr/bin")
        );
        assert!(config
            .env
            .iter()
            .any(|(name, _)| name == "SAKURA_ENGINE_HOME"));
    }

    #[test]
    fn a_spec_with_no_adapter_is_a_problem_that_says_what_to_name() {
        crate::l10n::set_for_test(crate::l10n::Language::English);
        let problem = default_config(&spec(&[])).expect_err("no adapter");
        assert_eq!(problem.kind, ProblemKind::NotStarted);
        assert!(problem.detail.contains("SAKURA_ACP_ADAPTER"), "{problem:?}");
    }

    #[test]
    fn a_spec_with_no_adapter_says_it_in_the_engine_s_words_when_it_has_them() {
        let real: MasterSpec = serde_json::from_str(include_str!(
            "../../../sakura-core/tests/fixtures/engine-real-master-spec.json"
        ))
        .expect("the engine's own spec");
        let problem = default_config(&real).expect_err("no adapter");
        assert_eq!(problem.kind, ProblemKind::NotStarted);
        assert_eq!(problem.detail, real.adapter_problem);
    }

    #[test]
    fn the_engine_s_three_decisions_are_the_three_verdicts() {
        assert_eq!(verdict_of(Decision::Allow), Verdict::Allow);
        assert_eq!(verdict_of(Decision::Deny), Verdict::Deny);
        assert_eq!(verdict_of(Decision::Ask), Verdict::Ask);
    }

    #[gpui::test]
    fn a_kept_conversation_comes_back_only_while_restoring_it_is_on(cx: &mut gpui::TestAppContext) {
        use crate::prefs::Preferences;
        let rig = crate::ui::engine_rig_tests::open(cx, "{}", "{}", "{}");
        cx.update(|cx| Preferences::default().install(None, cx));
        rig.sessions.store(MASTER_SESSION, "", "kept");
        let agent = Rc::new(Cell::new(MasterAgent::Claude));
        let host = host(
            rig.source.clone(),
            rig.sessions.clone(),
            (
                agent,
                Rc::new(RefCell::new(String::new())),
                SharedStart::default(),
                Preview::default(),
            ),
            (String::new(), Rc::new(RefCell::new("fixture".into()))),
            KeptModels::default(),
        );
        let stored = |cx: &mut gpui::TestAppContext| cx.update(|cx| (host.stored_session)(cx));
        assert_eq!(stored(cx).as_deref(), Some("kept"));
        cx.update(|cx| Preferences::change(cx, |prefs| prefs.is_master_restored = false));
        assert_eq!(
            stored(cx),
            None,
            "restoring is off: the kept session stays unread"
        );
    }
}
