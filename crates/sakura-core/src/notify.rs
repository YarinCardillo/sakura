//! What to tell the person while Sakura is not in front, and what the Dock badge counts. Pure: from
//! the tasks as they were and as they are, and whether Sakura is in front, it decides the
//! notifications (a task starts to need the person; an agent finished) and the count (the tasks
//! that need the person). The same fact is never told twice, and nothing is told while Sakura is in
//! front: the person is looking at it. The platform layer shows what is decided here.

use std::collections::{HashMap, HashSet};

/// The most tasks one observation takes in.
pub const LAVORI_MAX: usize = 10_000;
/// The most notifications one observation gives: more facts at once are counted by the badge only.
pub const NOTES_MAX: usize = 8;
/// The largest count the badge writes out.
const BADGE_MAX: u32 = 99;

/// Where a task stands, for the person.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Stage {
    /// It waits for the person: a permission, a question.
    NeedsPerson,
    /// Its agent is at work.
    Working,
    /// Its agent ended.
    Done,
    /// Anything else.
    Quiet,
}

/// One task as an observation sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lavoro {
    pub name: String,
    pub stage: Stage,
    /// What names the fact behind the stage (the request waited on, the commit the agent ended
    /// at): the same stage with the same fact is the same fact, however often it is seen.
    pub fact: String,
}

/// What a notification is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NoteKind {
    NeedsPerson,
    Finished,
}

/// One notification to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub kind: NoteKind,
    pub task: String,
}

/// What one observation decides.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Decision {
    pub notes: Vec<Note>,
    /// The tasks that need the person.
    pub badge: u32,
}

/// Remembers the tasks as they were and what was already told.
#[derive(Debug, Default)]
pub struct Notifier {
    /// The stage of each task at the last observation; none before the first.
    before: Option<HashMap<String, Stage>>,
    /// The last fact told (or seen in front) of each task that is still there.
    told: HashMap<String, (NoteKind, String)>,
}

impl Notifier {
    /// Takes in the tasks as they are now. The first observation tells nothing: what is there when
    /// Sakura starts is not news.
    pub fn observe(&mut self, lavori: &[Lavoro], is_in_front: bool) -> Decision {
        let mut names = HashSet::new();
        let lavori = lavori.iter().take(LAVORI_MAX);
        let lavori: Vec<&Lavoro> = lavori
            .filter(|lavoro| names.insert(lavoro.name.as_str()))
            .collect();
        let needing = lavori
            .iter()
            .filter(|lavoro| lavoro.stage == Stage::NeedsPerson);
        let badge = u32::try_from(needing.count()).unwrap_or(u32::MAX);
        let mut notes = Vec::new();
        for lavoro in &lavori {
            let is_new = self.is_news(lavoro);
            let is_launch = self.before.is_none();
            if is_new && !is_launch && !is_in_front && notes.len() < NOTES_MAX {
                notes.push(Note {
                    kind: kind_of(lavoro.stage).unwrap_or(NoteKind::Finished),
                    task: lavoro.name.clone(),
                });
            }
        }
        self.told.retain(|name, _| names.contains(name.as_str()));
        let stages = lavori
            .iter()
            .map(|lavoro| (lavoro.name.clone(), lavoro.stage));
        self.before = Some(stages.collect());
        Decision { notes, badge }
    }

    /// Whether `lavoro` shows a fact not told yet, which is told (or seen) from now on.
    fn is_news(&mut self, lavoro: &Lavoro) -> bool {
        let before = self
            .before
            .as_ref()
            .and_then(|before| before.get(&lavoro.name));
        let was_at_work = matches!(before, Some(Stage::Working | Stage::NeedsPerson));
        let kind = match lavoro.stage {
            Stage::NeedsPerson => NoteKind::NeedsPerson,
            Stage::Done if was_at_work || self.before.is_none() => NoteKind::Finished,
            _ => return false,
        };
        let fact = (kind, lavoro.fact.clone());
        let is_told = self.told.get(&lavoro.name) == Some(&fact);
        self.told.insert(lavoro.name.clone(), fact);
        !is_told
    }

    #[cfg(test)]
    fn told_count(&self) -> usize {
        self.told.len()
    }
}

fn kind_of(stage: Stage) -> Option<NoteKind> {
    match stage {
        Stage::NeedsPerson => Some(NoteKind::NeedsPerson),
        Stage::Done => Some(NoteKind::Finished),
        Stage::Working | Stage::Quiet => None,
    }
}

/// What the Dock badge shows for `count` tasks: nothing for none, `99+` past ninety-nine.
pub fn badge_label(count: u32) -> Option<String> {
    match count {
        0 => None,
        count if count <= BADGE_MAX => Some(count.to_string()),
        _ => Some(format!("{BADGE_MAX}+")),
    }
}

#[cfg(test)]
mod tests;
