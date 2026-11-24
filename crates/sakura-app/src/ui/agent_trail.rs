//! Where the right pane follows a task's agent to: the place of its newest located tool call, or of a
//! change it made on disk that no call named (a shell command that writes a file names none), whichever
//! is newer. The changes are read from the task's patch: the first patch read is where they start from.

use sakura_core::files::fresh_changes;
use sakura_core::patch::Patch;
use std::sync::Arc;
use std::time::Instant;

/// Where an agent is: a file as its tool call or the patch names it, and the line.
pub(super) type Place = (String, Option<u32>);

/// A located tool call of the agent: its entry in the conversation, its place, and when it was seen.
struct Call {
    entry: usize,
    place: Place,
    seen_at: Instant,
}

/// What is known of where one task's agent went.
#[derive(Default)]
pub(super) struct AgentTrail {
    place: Option<Place>,
    call: Option<Call>,
    /// The files, inside the task, that located calls named since the patch before, and when.
    named: Vec<(String, Instant)>,
    patch: Option<Arc<Patch>>,
    /// The agent's turn ran since the patch before was read.
    has_worked: bool,
}

impl AgentTrail {
    /// Where the agent is now.
    pub fn place(&self) -> Option<&Place> {
        self.place.as_ref()
    }

    /// The agent's newest located call is the entry `entry` of its conversation, at `place`, seen `now`;
    /// `file` is that place inside the task. A call not seen before moves the agent there. Returns
    /// whether the agent moved.
    pub fn saw_call(
        &mut self,
        entry: usize,
        place: Place,
        file: Option<String>,
        now: Instant,
    ) -> bool {
        let is_known = self.call.as_ref();
        if is_known.is_some_and(|call| call.entry == entry && call.place == place) {
            return false;
        }
        self.named.extend(file.map(|file| (file, now)));
        let seen_at = now;
        self.call = Some(Call {
            entry,
            place: place.clone(),
            seen_at,
        });
        self.move_to(place)
    }

    /// The agent's turn is running.
    pub fn worked(&mut self) {
        self.has_worked = true;
    }

    /// Follow the last newly changed file in patch order, at its first new line, if the agent worked.
    /// Skip paths already named by a tool call. A call seen after `read_from` takes precedence.
    pub fn saw_patch(&mut self, patch: &Arc<Patch>, read_from: Instant, is_working: bool) -> bool {
        if self
            .patch
            .as_ref()
            .is_some_and(|held| Arc::ptr_eq(held, patch))
        {
            return false;
        }
        let before = self.patch.replace(patch.clone());
        let has_worked = std::mem::replace(&mut self.has_worked, is_working) || is_working;
        let (named, later): (Vec<_>, Vec<_>) = std::mem::take(&mut self.named)
            .into_iter()
            .partition(|(_, at)| *at < read_from);
        self.named = later;
        let is_call_newer = self
            .call
            .as_ref()
            .is_some_and(|call| call.seen_at >= read_from);
        let Some(before) = before.filter(|_| has_worked && !is_call_newer) else {
            return false;
        };
        let fresh = fresh_changes(&before, patch).into_iter().rev();
        let mut unnamed =
            fresh.filter(|change| !named.iter().any(|(file, _)| *file == change.path));
        unnamed
            .next()
            .is_some_and(|change| self.move_to((change.path, Some(change.line))))
    }

    fn move_to(&mut self, place: Place) -> bool {
        let has_moved = self.place.as_ref() != Some(&place);
        self.place = Some(place);
        has_moved
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const README: &str = "diff --git a/README.md b/README.md\n--- a/README.md\n+++ b/README.md\n@@ -1 +1,2 @@\n # Demo\n+one line\n";
    const NOTES: &str = "diff --git a/docs/NOTE.md b/docs/NOTE.md\n--- a/docs/NOTE.md\n+++ b/docs/NOTE.md\n@@ -1,2 +1,3 @@\n a\n+b\n c\n";

    fn patch(text: &str) -> Arc<Patch> {
        Arc::new(Patch::parse(text))
    }

    fn place(path: &str, line: u32) -> Place {
        (path.to_string(), Some(line))
    }

    /// A trail whose first patch, the empty one, was read at `start`.
    fn started(start: Instant) -> AgentTrail {
        let mut trail = AgentTrail::default();
        assert!(
            !trail.saw_patch(&patch(""), start, false),
            "the first patch is where changes start"
        );
        trail
    }

    #[test]
    fn a_change_while_the_agent_works_is_where_it_went_the_last_file_of_several() {
        let start = Instant::now();
        let mut trail = started(start);
        let both = patch(&format!("{README}{NOTES}"));
        assert!(trail.saw_patch(&both, start, true));
        assert_eq!(trail.place(), Some(&place("docs/NOTE.md", 2)));
        assert!(
            !trail.saw_patch(&both, start, true),
            "the same patch again moves nothing"
        );
    }

    #[test]
    fn a_change_moves_the_agent_only_when_its_turn_ran_since_the_patch_before() {
        let start = Instant::now();
        let mut trail = started(start);
        assert!(
            !trail.saw_patch(&patch(README), start, false),
            "the person's own edit"
        );
        assert_eq!(trail.place(), None);
        trail.worked();
        assert!(
            trail.saw_patch(&patch(NOTES), start, false),
            "a turn that just ended"
        );
        assert_eq!(trail.place(), Some(&place("docs/NOTE.md", 2)));
        assert!(
            !trail.saw_patch(&patch(README), start, false),
            "and no other since"
        );
    }

    #[test]
    fn undoing_a_change_leaves_the_agent_where_it_was() {
        let start = Instant::now();
        let mut trail = started(start);
        assert!(trail.saw_patch(&patch(README), start, true));
        assert!(!trail.saw_patch(&patch(""), start, true));
        assert_eq!(trail.place(), Some(&place("README.md", 2)));
    }

    #[test]
    fn a_located_call_newer_than_the_read_wins_and_a_file_a_call_named_is_no_news() {
        let start = Instant::now();
        let mut trail = started(start);
        let call = place("/work/demo/src/a.rs", 3);
        assert!(trail.saw_call(4, call.clone(), Some("src/a.rs".into()), start));
        assert!(
            !trail.saw_call(4, call.clone(), Some("src/a.rs".into()), start),
            "seen"
        );
        assert!(
            !trail.saw_patch(&patch(README), start, true),
            "the call came after"
        );
        assert_eq!(trail.place(), Some(&call));
        let later = start + Duration::from_secs(1);
        let named = patch(&format!(
            "{NOTES}{}",
            README.replace("README.md", "src/a.rs")
        ));
        let again = trail.saw_call(9, call.clone(), Some("src/a.rs".into()), start);
        assert!(!again, "a new call at the same place is no move");
        assert!(trail.saw_patch(&named, later, true), "src/a.rs is skipped");
        assert_eq!(trail.place(), Some(&place("docs/NOTE.md", 2)));
        assert!(
            trail.saw_call(12, call.clone(), None, later),
            "a newer call moves it back"
        );
        assert_eq!(trail.place(), Some(&call));
    }
}
