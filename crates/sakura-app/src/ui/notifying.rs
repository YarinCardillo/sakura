//! The desk's side of notifications and of the Dock badge: what each lavoro is to the person now, read
//! from what the window already knows (its facts, and what its conversation says of its agent), and told
//! to the layer that posts ([`super::notify`]). Nothing here runs on its own: the lavori are told again
//! when the facts change and when the window comes to the front or leaves it, and the model tells the
//! same fact once.

use super::console::rules::is_agent_running;
use super::desk::Desk;
use super::notify::{self, Notifications, Wanted};
use crate::prefs::Preferences;
use gpui::{App, Context, WeakEntity, Window};
use sakura_core::model::Live;
use sakura_core::models::{AgentPhase, Task};
use sakura_core::notify::{Lavoro, Stage};
use std::collections::HashMap;

/// Env var of a launch that must stay out of the person's way: it posts nothing to the system.
const NO_ACTIVATE_VAR: &str = "SAKURA_NO_ACTIVATE";

/// The notifications of one window.
pub(super) struct Notifying {
    layer: Notifications,
    /// Each lavoro's stage as last told and the number of that episode: needing the person again, or
    /// ending again, is a new fact.
    episodes: HashMap<String, (Stage, u64)>,
    /// A launch that takes pictures or must not take the person's attention tells the system nothing.
    is_silent: bool,
}

impl Notifying {
    /// Over the Dock's badge; silent for a snapshot and for a launch that must not activate.
    pub(super) fn new() -> Self {
        let is_silent = crate::snapshot::is_asked() || std::env::var_os(NO_ACTIVATE_VAR).is_some();
        Self {
            layer: Notifications::new(notify::dock_badge()),
            episodes: HashMap::new(),
            is_silent,
        }
    }

    /// What the badge shows now.
    #[cfg(test)]
    pub(super) fn badge(&self) -> Option<&str> {
        self.layer.badge()
    }

    /// The lavori as the model takes them: a stage that differs from the one told last opens a new
    /// episode, so the same need is told once and the next one again.
    fn lavori(&mut self, stages: Vec<(String, Stage)>) -> Vec<Lavoro> {
        let listed = |name: &String| stages.iter().any(|(listed, _)| listed == name);
        self.episodes.retain(|name, _| listed(name));
        let lavoro = |(name, stage): (String, Stage)| {
            let before = self.episodes.get(&name).copied();
            let episode = match before {
                Some((told, episode)) if told == stage => episode,
                Some((_, episode)) => episode + 1,
                None => 0,
            };
            self.episodes.insert(name.clone(), (stage, episode));
            Lavoro {
                name,
                stage,
                fact: episode.to_string(),
            }
        };
        stages.into_iter().map(lavoro).collect()
    }
}

/// Where `task` stands for the person: it waits for them, its agent works, its agent ended, or nothing
/// to say. A conversation's own word comes first; a lavoro whose agent runs in a terminal is read from
/// the facts.
pub(super) fn stage_of(task: &Task, is_waiting: bool, live: Live) -> Stage {
    if is_waiting {
        return Stage::NeedsPerson;
    }
    match live {
        Live::Starting | Live::Working => Stage::Working,
        Live::Answered | Live::Stopped => Stage::Done,
        Live::Unknown if is_agent_running(task) => Stage::Working,
        Live::Unknown => match task.agent.as_ref().map(|agent| &agent.state) {
            Some(AgentPhase::Exited) => Stage::Done,
            _ => Stage::Quiet,
        },
    }
}

impl Desk {
    /// Tells the notifications what the lavori are now and whether Sakura is in front.
    pub(super) fn tell_notifications(&mut self, cx: &mut Context<Self>) {
        let facts = self.facts.read(cx);
        let stage = |row: &crate::data::facts::Row| {
            let name = &row.task.name;
            let stage = stage_of(&row.task, facts.is_waiting(name), facts.live(name));
            (name.clone(), stage)
        };
        let read = facts.rows().iter().filter(|row| !row.is_pending);
        let stages: Vec<(String, Stage)> = read.map(stage).collect();
        let lavori = self.notifying.lavori(stages);
        let preferences = Preferences::try_current(cx).unwrap_or_default();
        let wanted = Wanted {
            needs_person: !preferences.is_needs_person_muted && !self.notifying.is_silent,
            finished: !preferences.is_finished_muted && !self.notifying.is_silent,
        };
        self.notifying.layer.want(wanted);
        let is_in_front = cx.active_window().is_some();
        self.notifying.layer.observe(&lavori, is_in_front, cx);
    }

    /// A press on a notification brings the window forward on its lavoro.
    pub(super) fn answer_notifications(window: &Window, cx: &mut Context<Self>) {
        let (desk, handle) = (cx.weak_entity(), window.window_handle());
        cx.on_system_notification_response(move |response, cx| {
            let Some(task) = notify::task_of(&response.tag).map(str::to_string) else {
                return;
            };
            show_lavoro(&desk, task, cx);
            cx.activate(true);
            if let Err(error) = handle.update(cx, |_, window, _| window.activate_window()) {
                eprintln!("sakura: the window of the notification is gone: {error}");
            }
        });
    }
}

/// Shows `task` in the desk, when the lavoro is still listed.
fn show_lavoro(desk: &WeakEntity<Desk>, task: String, cx: &mut App) {
    let shown = desk.update(cx, |desk, cx| {
        if desk.facts.read(cx).has_task(&task) {
            desk.select(task, cx);
        }
    });
    if shown.is_err() {
        eprintln!("sakura: the window of the notification is gone");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sakura_core::models::BoxStatus;

    /// The fixture's lavoro `name`.
    fn task(name: &str) -> Task {
        let status: BoxStatus = serde_json::from_str(include_str!(
            "../../../sakura-core/tests/fixtures/status.json"
        ))
        .expect("the fixture parses");
        let found = status.tasks.into_iter().find(|task| task.name == name);
        found.expect("the fixture's lavoro")
    }

    /// The fixture's lavoro whose agent runs in a terminal, and one whose agent is taken away.
    const RUNNING: &str = "sample-folders";
    const NO_AGENT: &str = "sample-accent";

    #[test]
    fn a_lavoro_that_waits_for_the_person_needs_them_whatever_else_is_true() {
        let task = task(RUNNING);
        assert_eq!(stage_of(&task, true, Live::Working), Stage::NeedsPerson);
        assert_eq!(stage_of(&task, true, Live::Unknown), Stage::NeedsPerson);
    }

    #[test]
    fn a_conversation_says_when_its_agent_works_and_when_it_ended() {
        let mut task = task(NO_AGENT);
        task.agent = None;
        assert_eq!(stage_of(&task, false, Live::Starting), Stage::Working);
        assert_eq!(stage_of(&task, false, Live::Working), Stage::Working);
        assert_eq!(stage_of(&task, false, Live::Answered), Stage::Done);
        assert_eq!(stage_of(&task, false, Live::Stopped), Stage::Done);
        assert_eq!(stage_of(&task, false, Live::Unknown), Stage::Quiet);
    }

    #[test]
    fn an_agent_in_a_terminal_is_read_from_the_facts() {
        let mut task = task(RUNNING);
        assert_eq!(stage_of(&task, false, Live::Unknown), Stage::Working);
        if let Some(agent) = task.agent.as_mut() {
            agent.state = AgentPhase::Exited;
        }
        assert_eq!(stage_of(&task, false, Live::Unknown), Stage::Done);
    }

    #[test]
    fn the_same_stage_is_one_episode_and_the_next_time_is_another() {
        let mut notifying = Notifying {
            layer: Notifications::new(notify::dock_badge()),
            episodes: HashMap::new(),
            is_silent: true,
        };
        let told = |notifying: &mut Notifying, stage| {
            let lavori = notifying.lavori(vec![("m1".to_string(), stage)]);
            lavori[0].fact.clone()
        };
        let first = told(&mut notifying, Stage::NeedsPerson);
        assert_eq!(told(&mut notifying, Stage::NeedsPerson), first);
        told(&mut notifying, Stage::Working);
        assert_ne!(told(&mut notifying, Stage::NeedsPerson), first);
        assert!(notifying.lavori(Vec::new()).is_empty());
        assert!(
            notifying.episodes.is_empty(),
            "a lavoro that is gone is forgotten"
        );
    }
}
