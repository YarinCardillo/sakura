//! Ending the work of a lavoro without closing it. On this Mac: "Publish" sends the agent the project's
//! publishing request as the person's message (push the branch, open a draft pull request against the
//! base), as a message of its own that leaves what they are writing where it is, and the link of the
//! pull request appears in the header when the agent's answer carries one.
//! In a box: once the work was taken, "Publish" starts a lavoro on this Mac from the branch that was
//! taken, with the same request as its first message. Nothing here closes a lavoro: "Archive" does.

use super::chrome::{control, keep_click, text_width};
use super::desk::Desk;
use super::new_task::Asked;
use super::task_menu::quiet_variant;
use crate::l10n::{say, tr, tr_with, Key};
use crate::prefs::Preferences;
use crate::theme::{self, Theme};
use gpui::{
    div, AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement, Styled as _,
    Window,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::Disableable as _;
use sakura_acp::thread::{EntryKind, Turn};
use sakura_core::model::Live;
use sakura_core::models::{Place, Task};
use sakura_core::publish::{self, PullRequest, Request};

const PUBLISH: Key = Key("publish.action");

/// What is known of the publishing of one lavoro.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Publishing {
    /// The agent was asked to publish and its answer is awaited.
    pending: Option<Pending>,
    /// The pull request its answer named.
    link: Option<PullRequest>,
}

/// A publishing request the agent has not answered yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Pending {
    /// How many entries the conversation had when the request was sent: the answer comes after them.
    entries: usize,
    /// The agent's turn was seen under way: when it is no longer, the answer is whole.
    has_started: bool,
}

/// What the header offers to end the work of `task` with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Offer {
    /// A lavoro on this Mac: its own agent publishes.
    ByItsAgent,
    /// A lavoro in a box whose work was taken: an agent on this Mac publishes the branch taken.
    AfterTake,
    Nothing,
}

/// What `task` can be published by: its own agent on this Mac; for a box, an agent on this Mac once the
/// commit at its head was taken and its branch is known.
pub(super) fn offer(task: &Task) -> Offer {
    let is_ready = task.readable && task.preparing.is_none();
    let is_taken = match (&task.taken, &task.head) {
        (Some(taken), Some(head)) => taken.sha == *head,
        _ => false,
    };
    match task.place {
        _ if !is_ready => Offer::Nothing,
        Place::Mac => Offer::ByItsAgent,
        Place::Box if is_taken && task.branch.is_some() => Offer::AfterTake,
        Place::Box => Offer::Nothing,
    }
}

impl Desk {
    /// The header's actions that end the work of `task`: the pull request's link once the agent named
    /// one, then "Publish".
    pub(super) fn publish_actions(&self, task: &Task, cx: &Context<Self>) -> Vec<AnyElement> {
        let offer = offer(task);
        if offer == Offer::Nothing {
            return Vec::new();
        }
        let link = self
            .publishing
            .get(&task.name)
            .and_then(|kept| kept.link.clone());
        let link = link.map(|link| pull_request_link(&link, cx));
        let button = self.publish_button(task, offer, cx);
        link.into_iter().chain([button]).collect()
    }

    /// The width [`Desk::publish_actions`] takes, for the header's fit.
    pub(super) fn publish_width(&self, task: &Task, window: &Window) -> f32 {
        if offer(task) == Offer::Nothing {
            return 0.0;
        }
        let small = |label: &str| {
            text_width(window, label, theme::TEXT_SMALL, theme::WEIGHT_MEDIUM)
                + f32::from(theme::CONTROL_PADDING_X) * 2.0
                + f32::from(theme::GAP_WIDE)
        };
        let kept = self.publishing.get(&task.name);
        let link = kept.and_then(|kept| kept.link.as_ref());
        small(tr(PUBLISH)) + link.map_or(0.0, |link| small(&link_label(link)))
    }

    /// "Publish": a quiet control of the common height. While the lavoro's own agent cannot be asked it
    /// cannot be pressed, and says why under the pointer.
    fn publish_button(&self, task: &Task, offer: Offer, cx: &Context<Self>) -> AnyElement {
        let why_not = match offer {
            Offer::ByItsAgent => self.why_not_ask(&task.name, cx),
            Offer::AfterTake | Offer::Nothing => None,
        };
        let tip = match (offer, why_not) {
            (Offer::AfterTake, _) => Key("publish.tip.after_take"),
            (_, None) => Key("publish.tip"),
            (_, Some(why_not)) => why_not,
        };
        let name = task.name.clone();
        let quiet = Button::new("header-publish")
            .debug_selector(|| "header-publish".into())
            .custom(quiet_variant(&Theme::palette(cx), cx));
        let button = control(quiet, tr(PUBLISH), &[tr(PUBLISH)])
            .disabled(why_not.is_some())
            .tooltip(tr(tip))
            .on_click(cx.listener(move |desk, _, window, cx| desk.publish(&name, window, cx)));
        keep_click(div().id("header-publish-area").flex_none().child(button)).into_any_element()
    }

    /// Whether the agent of `task` is in a turn, or waits for the person's answer.
    fn is_agent_busy(&self, task: &str, cx: &gpui::App) -> bool {
        let facts = self.facts.read(cx);
        facts.is_waiting(task) || matches!(facts.live(task), Live::Starting | Live::Working)
    }

    /// Whether the agent of `task` can be given a message now.
    pub(super) fn can_ask_agent(&self, task: &str, cx: &gpui::App) -> bool {
        self.why_not_ask(task, cx).is_none()
    }

    /// Why the agent of `task` cannot be given the publishing request now, in the words of a tooltip:
    /// a turn runs, or its conversation cannot take a message. What the person is writing does not
    /// hold it back: the request is a message of its own and the draft stays in the composer.
    fn why_not_ask(&self, task: &str, cx: &gpui::App) -> Option<Key> {
        if self.is_agent_busy(task, cx) {
            return Some(Key("publish.tip.working"));
        }
        // No conversation yet: its page was never drawn, so nothing runs.
        let conversation = self.conversations.get(task)?.view.read(cx);
        if conversation.thread().turn() != Turn::Idle {
            Some(Key("publish.tip.working"))
        } else if conversation.needs_sign_in() {
            Some(Key("publish.tip.closed"))
        } else {
            None
        }
    }

    /// Publishes the work of `task`: its own agent is asked on this Mac; for a box whose work was
    /// taken, a lavoro on this Mac is made from the branch taken, with the request as its first message.
    pub fn publish(&mut self, task: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.facts.read(cx).row(task) else {
            return;
        };
        let task = row.task.clone();
        match offer(&task) {
            Offer::ByItsAgent => self.ask_agent_to_publish(&task, window, cx),
            Offer::AfterTake => self.publish_from_this_mac(&task, window, cx),
            Offer::Nothing => {}
        }
    }

    /// The request goes to the lavoro's own agent as a message of its own; what the person is writing in
    /// the composer is neither sent with it nor thrown away.
    fn ask_agent_to_publish(&mut self, task: &Task, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_ask_agent(&task.name, cx) {
            // The header was drawn before the agent started a turn: it is drawn again, and says why not.
            return cx.notify();
        }
        let request = request_of(task, cx);
        let conversation = self.conversation(&task.name, window, cx);
        let sent = conversation.update(cx, |conversation, cx| {
            let entries = conversation.thread().entries().len();
            conversation.say(&request, cx).then_some(entries)
        });
        if let Some(entries) = sent {
            let kept = self.publishing.entry(task.name.clone()).or_default();
            kept.pending = Some(Pending {
                entries,
                has_started: false,
            });
            cx.notify();
        }
    }

    /// A lavoro on this Mac from the branch that was taken, asked to publish it.
    fn publish_from_this_mac(&mut self, task: &Task, window: &mut Window, cx: &mut Context<Self>) {
        let Some(branch) = task.branch.clone() else {
            return;
        };
        let asked = Asked {
            text: request_of(task, cx),
            project: task.profile.clone(),
            base: Some(branch),
            place: Place::Mac,
            account: None,
        };
        self.begin_lavoro(asked, window, cx);
    }

    /// The agents that were asked to publish and have answered: the link of the pull request in the
    /// answer, when there is one, is kept for the header. An answer is whole once the turn that was seen
    /// under way is over, or the agent stopped.
    pub(super) fn publishing_follows(&mut self, cx: &mut Context<Self>) {
        let pending = |(task, kept): (&String, &Publishing)| Some((task.clone(), kept.pending?));
        let asked: Vec<(String, Pending)> = self.publishing.iter().filter_map(pending).collect();
        for (task, pending) in asked {
            let is_working = self.is_agent_busy(&task, cx);
            let is_stopped = matches!(self.facts.read(cx).live(&task), Live::Stopped);
            let is_over = !is_working && (pending.has_started || is_stopped);
            let answer = is_over.then(|| self.answer_since(&task, pending.entries, cx));
            let Some(kept) = self.publishing.get_mut(&task) else {
                continue;
            };
            match answer {
                None if is_working => {
                    kept.pending = Some(Pending {
                        has_started: true,
                        ..pending
                    });
                }
                None => {}
                Some(answer) => {
                    kept.pending = None;
                    let link = answer.and_then(|text| publish::pull_request(&text));
                    kept.link = link.or(kept.link.take());
                    cx.notify();
                }
            }
        }
    }

    /// The agent's last answer in the conversation of `task`, among the entries after the first `from`.
    fn answer_since(&self, task: &str, from: usize, cx: &Context<Self>) -> Option<String> {
        let conversation = self.conversations.get(task)?.view.read(cx);
        let entries = conversation.thread().entries().iter().skip(from).rev();
        entries.into_iter().find_map(|entry| match &entry.kind {
            EntryKind::Assistant(message) => Some(message.text.clone()),
            _ => None,
        })
    }

    /// Whether the agent of `task` was asked to publish and has not answered.
    #[cfg(test)]
    pub fn is_publishing(&self, task: &str) -> bool {
        self.publishing
            .get(task)
            .is_some_and(|kept| kept.pending.is_some())
    }

    /// The pull request the agent of `task` named.
    #[cfg(test)]
    pub fn pull_request_of(&self, task: &str) -> Option<String> {
        let link = self.publishing.get(task)?.link.as_ref()?;
        Some(link.url.clone())
    }
}

/// What the agent is asked: the project's own request when the person wrote one, else the one they wrote
/// for every project, else the default one in the interface's language.
fn request_of(task: &Task, cx: &gpui::App) -> String {
    let own = own_request(&task.profile, cx);
    match publish::request_for(own.as_deref(), &task.base) {
        Request::Own(text) => text,
        Request::Default(said) => say(&said).to_string(),
    }
}

/// The publishing request the person wrote in Settings for `project`; none until they write one, and
/// none where no preferences are kept.
fn own_request(project: &str, cx: &gpui::App) -> Option<String> {
    written_request(&Preferences::try_current(cx)?, project)
}

/// The request written for `project`, else the one written for every project.
fn written_request(preferences: &Preferences, project: &str) -> Option<String> {
    let own = preferences.publish_requests.get(project);
    own.or(preferences.publish_request.as_ref()).cloned()
}

/// "Pull request 12": what the link is called.
fn link_label(link: &PullRequest) -> String {
    let number = link.number.to_string();
    tr_with(Key("publish.pull_request"), &[("number", &number)]).to_string()
}

/// The link of the pull request: a quiet control that opens it in the browser. The address is one the
/// agent wrote: it is opened only as the web address the app's own rule reads in it.
fn pull_request_link(link: &PullRequest, cx: &Context<Desk>) -> AnyElement {
    let (label, url) = (link_label(link), link.url.clone());
    let shown = url.clone();
    let quiet = Button::new("header-pull-request")
        .debug_selector(|| "header-pull-request".into())
        .custom(quiet_variant(&Theme::palette(cx), cx))
        .h(theme::CONTROL_HEIGHT)
        .px(theme::CONTROL_PADDING_X)
        .child(
            div()
                .text_size(theme::TEXT_SMALL)
                .font_weight(theme::WEIGHT_MEDIUM)
                .whitespace_nowrap()
                .child(label),
        )
        .tooltip(shown)
        .on_click(move |_, _, cx| {
            if let Some(address) = sakura_core::links::web_url(&url) {
                cx.open_url(&address);
            }
        });
    keep_click(
        div()
            .id("header-pull-request-area")
            .flex_none()
            .child(quiet),
    )
    .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use sakura_core::models::{BoxStatus, TakenState};

    /// The fixture's lavoro with commits and nothing taken.
    fn task() -> Task {
        let status: BoxStatus = serde_json::from_str(include_str!(
            "../../../sakura-core/tests/fixtures/status.json"
        ))
        .expect("the fixture parses");
        let found = status
            .tasks
            .into_iter()
            .find(|task| task.name == "sample-accent");
        found.expect("the fixture's lavoro")
    }

    #[test]
    fn a_lavoro_on_this_mac_is_published_by_its_own_agent() {
        let mut task = task();
        task.place = Place::Mac;
        assert_eq!(offer(&task), Offer::ByItsAgent);
        task.readable = false;
        assert_eq!(
            offer(&task),
            Offer::Nothing,
            "nothing to publish from a folder not read"
        );
    }

    #[test]
    fn a_lavoro_in_a_box_is_published_only_once_its_head_was_taken() {
        let mut task = task();
        task.place = Place::Box;
        task.taken = None;
        assert_eq!(offer(&task), Offer::Nothing, "Take comes first");
        let head = task.head.clone().expect("a head");
        task.taken = Some(TakenState {
            sha: "0000000000000000000000000000000000000000".into(),
            at: "2026-11-10T06:00:00Z".into(),
        });
        assert_eq!(
            offer(&task),
            Offer::Nothing,
            "an earlier commit was taken, not this one"
        );
        task.taken = Some(TakenState {
            sha: head,
            at: "2026-11-10T07:00:00Z".into(),
        });
        assert_eq!(offer(&task), Offer::AfterTake);
        task.branch = None;
        assert_eq!(offer(&task), Offer::Nothing, "no branch to start from");
    }

    #[test]
    fn the_request_is_the_projects_own_then_the_one_for_every_project_then_none() {
        let mut preferences = Preferences::default();
        assert_eq!(written_request(&preferences, "site"), None, "the default");
        preferences.publish_request = Some("Push and open a draft".into());
        let for_all = written_request(&preferences, "site");
        assert_eq!(for_all.as_deref(), Some("Push and open a draft"));
        let own = "Push to staging".to_string();
        preferences.publish_requests.insert("site".into(), own);
        let own = written_request(&preferences, "site");
        assert_eq!(own.as_deref(), Some("Push to staging"));
        let other = written_request(&preferences, "api");
        assert_eq!(other.as_deref(), Some("Push and open a draft"));
    }
}
