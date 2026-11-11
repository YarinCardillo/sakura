//! The conversation of each lavoro: the home of its page. One view per lavoro, made the first time its
//! page is drawn and kept while the app runs, so a tab switch finds it as it was. Looking at a lavoro
//! starts nothing: the agent (`box acp`) is started by the person's first message, or for the lavoro
//! that is shown when a conversation is kept for it from before, which is then shown again. The desk gives the view what only it knows: how the agent is
//! started and with which account, the session kept for the lavoro, the sign-in and the terminal.

use super::centre::CentreView;
use super::chrome::text;
use super::console::rules::ConsoleKind;
use super::desk::Desk;
use super::thread::{
    Ask, ChooseAccount, Host, Launched, Login, Readiness, StoreSession, StoredSession, ThreadView,
    ThreadViewEvent,
};
use crate::data::actions::{Accounts, ActionKey, Actions};
use crate::data::facts::Facts;
use crate::data::projects::Projects;
use crate::l10n::{say, tr, tr_with, Key};
use crate::prefs::Preferences;
use crate::theme::{self, Palette};
use gpui::{
    div, px, AnyElement, App, AppContext as _, Context, Entity, IntoElement as _,
    ParentElement as _, SharedString, Styled as _, Subscription, WeakEntity, Window,
};
use sakura_core::models::{Account, Place, Task};
use sakura_core::{box_words, display};

/// The name a sign-in is started under when the boxes have no account yet: nobody is asked to invent
/// one, and it can be renamed where the accounts are kept.
const FIRST_ACCOUNT: &str = "claude";

/// A lavoro's conversation, and the watch on what it tells the desk.
pub(super) struct Conversation {
    pub(super) view: Entity<ThreadView>,
    _watch: [Subscription; 2],
}

/// What a conversation is started with, as the facts say when it starts.
#[derive(Clone)]
struct Lavoro {
    name: String,
    project: String,
    place: Place,
    workspace: String,
}

/// What the facts say of the lavoro `name` now; nothing while it is not listed.
fn lavoro(facts: &Facts, name: &str) -> Option<Lavoro> {
    let task = &facts.row(name)?.task;
    Some(Lavoro {
        name: task.name.clone(),
        project: task.profile.clone(),
        place: task.place,
        workspace: task.place.workspace(&task.name, task.folder.as_deref()),
    })
}

/// Why `task` has no conversation yet, in one sentence: it is being prepared, its creation failed, or its
/// folder cannot be read.
pub(super) fn unavailable(task: &Task) -> Option<SharedString> {
    match &task.preparing {
        // The line of a creation that failed is `box`'s own: the page says it in Sakura's words.
        Some(preparing) if preparing.failed => Some(creation_failed(&preparing.line)),
        Some(preparing) => {
            let line = display::label(&preparing.line);
            Some(tr_with(Key("start.preparing"), &[("line", &line)]))
        }
        None if !task.readable => Some(tr(Key("changes.unreadable")).into()),
        None => None,
    }
}

/// That a lavoro could not be made, with the reason `line` (what `box` wrote) gives, in Sakura's words.
fn creation_failed(line: &str) -> SharedString {
    could_not_create(&say(&box_words::said(line).said))
}

fn could_not_create(reason: &str) -> SharedString {
    let reason = lowercase_first(reason);
    tr_with(Key("start.creation_failed"), &[("reason", &reason)])
}

/// Where the lavoro `name`, requested with a first message, stands: its creation could not start, it
/// failed, it is still running, or the lavoro is there.
fn readiness(name: &str, facts: &Facts, actions: &Actions) -> Readiness {
    let key = ActionKey::new_task(name);
    if let Some(said) = actions.error(&key) {
        return Readiness::Failed {
            said: could_not_create(&say(said)),
            details: actions.error_details(&key).map(str::to_string),
        };
    }
    let Some(row) = facts.row(name) else {
        return Readiness::Preparing(None);
    };
    match &row.task.preparing {
        Some(preparing) if preparing.failed => Readiness::Failed {
            said: creation_failed(&preparing.line),
            details: Some(preparing.line.clone()),
        },
        Some(preparing) => {
            let line = display::label(&preparing.line);
            Readiness::Preparing((!line.is_empty()).then_some(line))
        }
        None if row.is_pending => Readiness::Preparing(None),
        None => Readiness::Ready,
    }
}

/// A sentence as it reads after a colon.
fn lowercase_first(sentence: &str) -> String {
    let mut letters = sentence.chars();
    match letters.next() {
        Some(first) => first.to_lowercase().chain(letters).collect(),
        None => String::new(),
    }
}

/// The sentence of a lavoro that has no conversation yet, in the middle of the centre.
pub(super) fn unavailable_view(sentence: SharedString, palette: &Palette) -> AnyElement {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .child(
            text(theme::TEXT_SMALL, palette.text_tertiary)
                .max_w(px(theme::START_COLUMN_WIDTH))
                .child(sentence),
        )
        .into_any_element()
}

impl Desk {
    /// The conversation of the lavoro `name`, made now when its page is drawn for the first time (or
    /// when the lavoro is requested with a first message, before `box` lists it).
    pub(super) fn conversation(
        &mut self,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<ThreadView> {
        if let Some(kept) = self.conversations.get(name) {
            return kept.view.clone();
        }
        let host = self.host_of(name);
        let view = cx.new(|cx| ThreadView::new(host, window, cx));
        let task = name.to_string();
        let watch = cx.subscribe(
            &view,
            move |desk, _, event: &ThreadViewEvent, cx| match *event {
                ThreadViewEvent::WaitsForPerson(waits) => desk.agent_waits(&task, waits, cx),
                ThreadViewEvent::Live(live) => {
                    desk.facts
                        .update(cx, |facts, cx| facts.set_live(&task, live, cx));
                }
            },
        );
        // The agent's tool calls say where it is: the right pane follows it from there.
        let followed = name.to_string();
        let moves = cx.observe(&view, move |desk, _, cx| {
            desk.agent_moved(&followed, cx);
            desk.account_wanted(&followed, cx);
        });
        let conversation = Conversation {
            view: view.clone(),
            _watch: [watch, moves],
        };
        self.conversations.insert(name.to_string(), conversation);
        view
    }

    /// The conversation of `task`, if its page was drawn.
    #[cfg(any(test, feature = "snapshot"))]
    pub fn conversation_of(&self, task: &str) -> Option<Entity<ThreadView>> {
        Some(self.conversations.get(task)?.view.clone())
    }

    /// The agent of `task` asked the person something, or was answered: the lavoro's state says so in
    /// the sidebar and the header, and its tab too (the desk tells the strip whenever the facts move).
    fn agent_waits(&mut self, task: &str, waits: bool, cx: &mut Context<Self>) {
        self.facts
            .update(cx, |facts, cx| facts.set_waiting(task, waits, cx));
    }

    /// Tells the strip the state of every lavoro that has a tab, as the sidebar and the header show it,
    /// and the facts which lavori have a conversation kept from before.
    pub(super) fn tell_states(&mut self, cx: &mut Context<Self>) {
        let kept = self.sessions.tasks();
        self.facts.update(cx, |facts, cx| facts.set_kept(kept, cx));
        let facts = self.facts.read(cx);
        let tabs = self.strip.read(cx).tabs().tabs();
        let states = tabs.iter().filter_map(|tab| {
            let row = facts.row(&tab.task).filter(|row| !row.is_pending)?;
            let (waits, live) = (facts.is_waiting(&tab.task), facts.live(&tab.task));
            let state = sakura_core::model::shown_state_live(&row.task, waits, live);
            Some((tab.task.clone(), state))
        });
        let states = states.collect();
        self.strip
            .update(cx, |strip, cx| strip.set_states(states, cx));
    }

    /// A sign-in ended well: every conversation that was waiting for one starts over.
    pub(super) fn signed_in(&mut self, cx: &mut Context<Self>) {
        for conversation in self.conversations.values() {
            conversation.view.update(cx, |view, cx| {
                if view.needs_sign_in() {
                    view.retry(cx);
                }
            });
        }
    }

    /// What the conversation of the lavoro `name` needs from the desk. Where the lavoro lives and its
    /// folder are read from the facts when they are needed: a conversation can be made before them.
    fn host_of(&self, name: &str) -> Host {
        let (source, known) = (self.source.clone(), self.known());
        let task = name.to_string();
        let launch = move |cx: &App| {
            let Some(lavoro) = lavoro(known.facts.read(cx), &task) else {
                return Launched::Unavailable;
            };
            let account = match known.account_of(&lavoro, cx) {
                Ok(account) => account,
                Err(NoStart::SignInFirst) => return Launched::SignInFirst,
                Err(NoStart::NoAccount) => return Launched::NoAccount,
            };
            match source.agent(&lavoro.name, Some(&account), &lavoro.workspace) {
                Some((agent, events)) => Launched::Started(agent, events),
                None => Launched::Unavailable,
            }
        };
        let (facts, actions, task) = (self.facts.clone(), self.actions.clone(), name.to_string());
        let login = {
            let (known, task) = (self.known(), task.clone());
            move |cx: &App| match lavoro(known.facts.read(cx), &task) {
                Some(lavoro) if lavoro.place == Place::Mac => Login::Mac {
                    account: known.mac_account(&lavoro, cx),
                },
                Some(_) => Login::Box,
                None => Login::Mac { account: None },
            }
        };
        let accounts = {
            let actions = actions.clone();
            move |cx: &App| mac_accounts(actions.read(cx))
        };
        let workspace = {
            let (facts, task) = (facts.clone(), task.clone());
            move |cx: &App| {
                lavoro(facts.read(cx), &task).map_or_else(String::new, |lavoro| lavoro.workspace)
            }
        };
        let (kept, keeping) = (self.sessions.clone(), self.sessions.clone());
        Host {
            launch: Box::new(launch),
            refusals: Box::new(move |session, _| kept.refusals(session)),
            keep_refusal: Box::new(move |session, words, _| keeping.keep_refusal(session, words)),
            stored_session: self.kept_session(name),
            store_session: self.keep_session(name),
            login: Box::new(login),
            accounts: Box::new(accounts),
            choose_account: self.choose_account(name),
            sign_in: self.ask(name, Desk::sign_in),
            open_terminal: self.ask(name, Desk::show_terminal),
            workspace: Box::new(workspace),
            readiness: Box::new(move |cx| readiness(&task, facts.read(cx), actions.read(cx))),
            retry_creation: self.ask(name, Desk::retry_creation),
        }
    }

    /// Reads the session kept for the lavoro `name` with the account it would start with now.
    fn kept_session(&self, name: &str) -> StoredSession {
        let (sessions, known) = (self.sessions.clone(), self.known());
        let name = name.to_string();
        Box::new(move |cx| {
            let lavoro = lavoro(known.facts.read(cx), &name)?;
            let account = known.account_of(&lavoro, cx).ok()?;
            sessions.stored(&lavoro.name, session_owner(lavoro.place, &account))
        })
    }

    /// Keeps the session that opened for the lavoro `name` with the account it started with.
    fn keep_session(&self, name: &str) -> StoreSession {
        let (sessions, known) = (self.sessions.clone(), self.known());
        let name = name.to_string();
        Box::new(move |session, cx| {
            let Some(lavoro) = lavoro(known.facts.read(cx), &name) else {
                return;
            };
            if let Ok(account) = known.account_of(&lavoro, cx) {
                let owner = session_owner(lavoro.place, &account);
                sessions.store(&lavoro.name, owner, session);
            }
        })
    }

    /// What the desk knows that says which account an agent starts with.
    fn known(&self) -> Known {
        Known {
            facts: self.facts.clone(),
            actions: self.actions.clone(),
            projects: self.projects.clone(),
        }
    }

    /// Gives the project of the lavoro `name` the account the person chose for its agent.
    fn choose_account(&self, name: &str) -> ChooseAccount {
        let (desk, task): (WeakEntity<Desk>, String) = (self.me.clone(), name.to_string());
        Box::new(move |account, cx| {
            if let Err(error) = desk.update(cx, |desk, cx| desk.set_account(&task, account, cx)) {
                eprintln!("sakura: the window is gone: {error}");
            }
        })
    }

    /// Tells `box` that the project of `task` uses `account`, and reads the projects again: the
    /// conversation that waited starts when they name it. A refusal is said in the conversation.
    fn set_account(&mut self, task: &str, account: &str, cx: &mut Context<Self>) {
        let Some(lavoro) = lavoro(self.facts.read(cx), task) else {
            return;
        };
        let pending = self
            .source
            .set_project_account(lavoro.project, account.to_string());
        let task = task.to_string();
        cx.spawn(async move |desk, cx| {
            let set = pending.await;
            drop(desk.update(cx, |desk, cx| match set {
                Ok(()) => desk.projects.update(cx, |projects, cx| projects.load(cx)),
                Err(error) => desk.account_refused(&task, error.details.unwrap_or_default(), cx),
            }));
        })
        .detach();
    }

    fn account_refused(&mut self, task: &str, details: String, cx: &mut Context<Self>) {
        if let Some(conversation) = self.conversations.get(task) {
            conversation
                .view
                .update(cx, |view, cx| view.account_refused(details, cx));
        }
    }

    /// The projects were read (again): a conversation that waits for an account reads them when they
    /// were never read, and starts when its project names one now.
    pub(super) fn projects_moved(&mut self, cx: &mut Context<Self>) {
        let waiting: Vec<_> = self
            .conversations
            .values()
            .map(|c| c.view.clone())
            .collect();
        for view in waiting {
            view.update(cx, |view, cx| {
                if view.needs_account() {
                    view.retry(cx);
                }
            });
        }
    }

    /// A conversation moved: one that now waits for an account has the projects read, when they never
    /// were, because the lavoro's project may name it.
    fn account_wanted(&mut self, task: &str, cx: &mut Context<Self>) {
        let needs = |conversation: &Conversation| conversation.view.read(cx).needs_account();
        if self.conversations.get(task).is_some_and(needs) {
            self.projects
                .update(cx, |projects, cx| projects.load_once(cx));
        }
    }

    /// Something the conversation of `task` asks of the desk, on the window.
    fn ask(&self, task: &str, act: fn(&mut Desk, &str, &mut Window, &mut Context<Desk>)) -> Ask {
        let (desk, task): (WeakEntity<Desk>, String) = (self.me.clone(), task.to_string());
        Box::new(move |window, cx| {
            if let Err(error) = desk.update(cx, |desk, cx| act(desk, &task, window, cx)) {
                eprintln!("sakura: the window is gone: {error}");
            }
        })
    }

    /// Opens the sign-in for the agent of `task`, in the agent's place: of the account it starts with
    /// (its login no longer works), else of an account of the boxes that is not signed in, else of a
    /// first one under a name nobody has to invent.
    fn sign_in(&mut self, task: &str, window: &mut Window, cx: &mut Context<Self>) {
        let account = match self.actions.read(cx).accounts() {
            Accounts::Loaded(accounts) => {
                let preferred = Preferences::current(cx).box_account;
                box_account(accounts, preferred.as_deref()).or_else(|| first_of_boxes(accounts))
            }
            Accounts::Loading | Accounts::Failed(_) => None,
        };
        let account = account.unwrap_or_else(|| FIRST_ACCOUNT.to_string());
        self.open_console(task, ConsoleKind::Login(account), window, cx);
        self.show_centre_view(task, CentreView::Agent, cx);
    }

    /// Shows the terminal of `task`, opening one when it has none.
    fn show_terminal(&mut self, task: &str, window: &mut Window, cx: &mut Context<Self>) {
        if !self.has_terminal(task, cx) {
            return self.toggle_terminal(task, window, cx);
        }
        self.show_centre_view(task, CentreView::Terminal, cx);
    }
}

/// Why no agent can be started for a lavoro yet.
enum NoStart {
    /// No account of the boxes is signed in: the person signs in first.
    SignInFirst,
    /// No account is named for a lavoro on the Mac: the person chooses one.
    NoAccount,
}

/// What says which account an agent starts with: the lavori, the accounts and the projects.
#[derive(Clone)]
struct Known {
    facts: Entity<Facts>,
    actions: Entity<Actions>,
    projects: Entity<Projects>,
}

impl Known {
    /// The account `box acp` is given for `lavoro`, always named, so that the agent starts with the
    /// account the window shows. On the Mac: the one its project names, else the only login there is.
    /// In a box: the account chosen last in "New task" while it is signed in, else the first account of
    /// the boxes that is, and a sign-in when there is none (or the accounts are not read yet: "Try
    /// again" asks once more).
    fn account_of(&self, lavoro: &Lavoro, cx: &App) -> Result<String, NoStart> {
        if lavoro.place == Place::Mac {
            return self.mac_account(lavoro, cx).ok_or(NoStart::NoAccount);
        }
        let Accounts::Loaded(accounts) = self.actions.read(cx).accounts() else {
            return Err(NoStart::SignInFirst);
        };
        let preferred = Preferences::current(cx).box_account;
        box_account(accounts, preferred.as_deref()).ok_or(NoStart::SignInFirst)
    }

    /// The person's login a lavoro on the Mac uses: the one its project names (when the projects were
    /// read), else the only one there is.
    fn mac_account(&self, lavoro: &Lavoro, cx: &App) -> Option<String> {
        let project = self.projects.read(cx).project(&lavoro.project);
        let own = project.and_then(|project| project.account.clone());
        own.or_else(|| only_of_the_mac(self.actions.read(cx)))
    }
}

/// Under whose name a lavoro's session is kept: the account's in a box, where the session lives with
/// the account; none on the Mac, where a lavoro has one conversation whatever its project is told later.
fn session_owner(place: Place, account: &str) -> &str {
    match place {
        Place::Box => account,
        Place::Mac => "",
    }
}

/// The account of the boxes an agent starts with: `preferred` while it is signed in, else the first
/// that is.
pub(super) fn box_account(accounts: &[Account], preferred: Option<&str>) -> Option<String> {
    let usable = |account: &&Account| account.place == Place::Box && account.is_usable();
    let chosen = accounts
        .iter()
        .filter(usable)
        .find(|account| Some(account.name.as_str()) == preferred);
    chosen
        .or_else(|| accounts.iter().find(usable))
        .map(|account| account.name.clone())
}

/// The first account of the boxes, signed in or not.
fn first_of_boxes(accounts: &[Account]) -> Option<String> {
    let of_boxes = |account: &&Account| account.place == Place::Box;
    accounts
        .iter()
        .find(of_boxes)
        .map(|account| account.name.clone())
}

/// The person's logins on the Mac an agent can start with.
fn mac_accounts(actions: &Actions) -> Vec<String> {
    let Accounts::Loaded(accounts) = actions.accounts() else {
        return Vec::new();
    };
    let of_the_mac = |account: &&Account| account.place == Place::Mac && account.is_usable();
    let names = accounts.iter().filter(of_the_mac);
    names.map(|account| account.name.clone()).collect()
}

/// The person's login on the Mac, when there is exactly one.
fn only_of_the_mac(actions: &Actions) -> Option<String> {
    let mut accounts = mac_accounts(actions).into_iter();
    match (accounts.next(), accounts.next()) {
        (Some(only), None) => Some(only),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(name: &str, place: Place, logged_in: bool) -> Account {
        Account {
            name: name.into(),
            place,
            logged_in,
            exists: (place == Place::Mac).then_some(true),
        }
    }

    #[test]
    fn a_lavoro_in_a_box_starts_with_the_first_box_account_that_is_signed_in() {
        let accounts = [
            account("personal", Place::Mac, false),
            account("signed-out", Place::Box, false),
            account("signed-in", Place::Box, true),
        ];
        assert_eq!(box_account(&accounts, None).as_deref(), Some("signed-in"));
        assert_eq!(
            box_account(&accounts[..2], None),
            None,
            "a login of the Mac serves no box"
        );
    }

    #[test]
    fn the_box_account_chosen_last_is_the_one_used_while_it_is_signed_in() {
        let accounts = [
            account("first", Place::Box, true),
            account("chosen", Place::Box, true),
            account("signed-out", Place::Box, false),
        ];
        let of = |preferred| box_account(&accounts, Some(preferred));
        assert_eq!(of("chosen").as_deref(), Some("chosen"));
        assert_eq!(of("signed-out").as_deref(), Some("first"), "not signed in");
        assert_eq!(of("missing").as_deref(), Some("first"), "no longer there");
    }
}
