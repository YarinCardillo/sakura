//! From the sentence to the agent: what the desk does when "New task" asks for a lavoro. The lavoro is
//! named from the sentence, `box new` is started, its tab opens at once with the person's message in the
//! conversation, and the agent is started, with the sentence as its first prompt, when `box` says the
//! lavoro is ready. Also here: what the panel shows when there is no lavoro at all, and the page that adds
//! a project.

use super::centred::centred;
use super::desk::Desk;
use super::new_task::Asked;
use super::panes;
use super::project_setup::SetupEvent;
use crate::data::actions::ActionKey;
use crate::data::projects::Listed;
use crate::l10n::{say, tr, Key};
use crate::prefs::Preferences;
use crate::theme::sheet;
use gpui::{
    div, px, AnyElement, Context, InteractiveElement as _, IntoElement as _, ParentElement as _,
    Styled as _, Window,
};
use sakura_core::models::Place;
use sakura_core::naming;
use sakura_core::projects::NewLavoro;

impl Desk {
    /// Makes the lavoro `asked` for: it is named from the sentence, its creation starts, and its tab
    /// opens now with the sentence as the person's first message.
    pub(super) fn begin_lavoro(
        &mut self,
        asked: Asked,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = self.name_for(&asked.text, cx);
        let account = self.account_to_set(&asked, cx);
        let (project, place) = (asked.project.clone(), asked.place);
        let box_account = asked.account.clone().filter(|_| place == Place::Box);
        Preferences::change(cx, move |kept| {
            kept.last_project = Some(project);
            kept.box_account = box_account.or(kept.box_account.take());
        });
        let lavoro = NewLavoro {
            project: asked.project,
            base: asked.base,
            place: Some(asked.place),
        };
        self.creating
            .insert(name.clone(), (lavoro.clone(), account.clone()));
        let task = name.clone();
        self.actions.update(cx, |actions, cx| {
            actions.new_task(task, lavoro, account, cx)
        });
        let conversation = self.conversation(&name, window, cx);
        conversation.update(cx, |conversation, cx| conversation.begin(asked.text, cx));
        self.select(name, cx);
    }

    /// A name for the lavoro of `sentence` that no lavoro has, listed or requested.
    fn name_for(&self, sentence: &str, cx: &Context<Self>) -> String {
        let facts = self.facts.read(cx);
        let listed = facts.rows().iter().map(|row| row.task.name.as_str());
        let taken: Vec<&str> = listed
            .chain(self.creating.keys().map(String::as_str))
            .collect();
        naming::name_for(sentence, &taken)
    }

    /// The account the project must be told to use before the lavoro is made: the one chosen for a
    /// lavoro on the Mac, when it is not the project's own already. (In a box the account is given when
    /// the agent starts.)
    fn account_to_set(&self, asked: &Asked, cx: &Context<Self>) -> Option<String> {
        if asked.place != Place::Mac {
            return None;
        }
        let projects = self.projects.read(cx);
        let own = projects.project(&asked.project)?.account.as_ref();
        asked.account.clone().filter(|chosen| Some(chosen) != own)
    }

    /// Asks again for the lavoro `task`, whose creation failed.
    pub(super) fn retry_creation(&mut self, task: &str, _: &mut Window, cx: &mut Context<Self>) {
        let Some((lavoro, account)) = self.creating.get(task).cloned() else {
            return;
        };
        let task = task.to_string();
        self.actions.update(cx, |actions, cx| {
            actions.new_task(task, lavoro, account, cx)
        });
    }

    /// Something was heard from `box`: every first message that waits for its lavoro goes on when the
    /// lavoro is ready. A lavoro whose agent started, or that the person removed, waits no more.
    pub(super) fn advance_creations(&mut self, cx: &mut Context<Self>) {
        let waiting: Vec<String> = self.creating.keys().cloned().collect();
        for name in waiting {
            let is_removed = self.actions.read(cx).is_running(&ActionKey::remove(&name));
            let conversation = self.conversations.get(&name).map(|kept| kept.view.clone());
            let Some(conversation) = conversation.filter(|_| !is_removed) else {
                self.creating.remove(&name);
                continue;
            };
            conversation.update(cx, |conversation, cx| conversation.proceed(cx));
            if !conversation.read(cx).is_preparing() {
                self.creating.remove(&name);
            }
        }
    }

    /// Shows the page that adds a project, in the place of the lavoro shown.
    pub(super) fn add_project(&mut self, cx: &mut Context<Self>) {
        self.is_adding_project = true;
        self.setup
            .update(cx, |setup, cx| setup.set_can_cancel(true, cx));
        cx.notify();
    }

    /// The page that adds a project is done: the project added is the one "New task" offers first.
    pub(super) fn project_setup_ended(&mut self, event: &SetupEvent, cx: &mut Context<Self>) {
        self.is_adding_project = false;
        if let SetupEvent::Added(name) = event {
            let name = name.clone();
            Preferences::change(cx, move |kept| kept.last_project = Some(name));
            self.projects.update(cx, |projects, cx| projects.load(cx));
        }
        cx.notify();
    }

    /// The panel when there is no lavoro at all: with no project yet, the page that asks for the folder
    /// of the first one; else the one field of "New task", in place and centred in the panel. A `box`
    /// that has no projects is said plainly.
    pub(super) fn first_page(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        self.projects
            .update(cx, |projects, cx| projects.load_once(cx));
        let projects = self.projects.read(cx);
        let has_project = !projects.usable().is_empty();
        match projects.listed() {
            Listed::Unknown => panes::empty(tr(Key("projects.reading")), cx),
            Listed::Unsupported => panes::no_task(tr(Key("projects.unsupported")), cx),
            Listed::Failed(error) => panes::no_task(say(&error.said).to_string(), cx),
            Listed::Loaded(_) if has_project => {
                let field = div()
                    .debug_selector(|| "first-task".into())
                    .w(px(sheet::WIDTH))
                    .child(self.new_task_in_place(window, cx));
                centred(field).into_any_element()
            }
            Listed::Loaded(_) => {
                self.setup
                    .update(cx, |setup, cx| setup.set_can_cancel(false, cx));
                self.setup.clone().into_any_element()
            }
        }
    }
}
