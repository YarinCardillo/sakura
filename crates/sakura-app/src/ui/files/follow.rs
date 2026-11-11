//! "Follow the agent": while it is on, the pane shows the file and line of the agent's current tool
//! call. It turns itself off as soon as the person takes the view (a scroll, a click on a file, a
//! selection) and keeps the last place the agent was at, so one call turns it on again and goes there.

/// A place in a lavoro's files: a path inside the lavoro and, when known, a line numbered from 1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    pub path: String,
    pub line: Option<u32>,
}

impl Location {
    /// The place `line` of `path`.
    pub fn new(path: impl Into<String>, line: Option<u32>) -> Self {
        Self {
            path: path.into(),
            line,
        }
    }
}

/// Whether the pane follows the agent, and where the agent was last.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Follow {
    is_on: bool,
    last: Option<Location>,
}

impl Default for Follow {
    /// Following is on until the person takes the view.
    fn default() -> Self {
        Self {
            is_on: true,
            last: None,
        }
    }
}

impl Follow {
    /// Whether the pane follows the agent now.
    pub fn is_on(&self) -> bool {
        self.is_on
    }

    /// The agent is at `location`: it is kept, and given back to show when following is on.
    pub fn agent_at(&mut self, location: Location) -> Option<&Location> {
        self.last = Some(location);
        self.last.as_ref().filter(|_| self.is_on)
    }

    /// The person took the view. Returns whether following was on, and so just turned off.
    pub fn person_acted(&mut self) -> bool {
        std::mem::replace(&mut self.is_on, false)
    }

    /// Turns following on again and gives the last place of the agent to go to. Returns whether it was
    /// off, with that place.
    pub fn turn_on(&mut self) -> (bool, Option<&Location>) {
        let was_off = !std::mem::replace(&mut self.is_on, true);
        (was_off, self.last.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn following_shows_each_place_of_the_agent_until_the_person_takes_the_view() {
        let mut follow = Follow::default();
        assert!(follow.is_on());
        let first = Location::new("src/a.ts", Some(12));
        assert_eq!(follow.agent_at(first.clone()), Some(&first));
        assert!(follow.person_acted(), "it was on");
        assert!(!follow.is_on());
        assert!(!follow.person_acted(), "already off: nothing changed");
        let second = Location::new("src/b.ts", None);
        assert_eq!(follow.agent_at(second.clone()), None, "kept, not shown");
        assert_eq!(follow.turn_on(), (true, Some(&second)));
        assert_eq!(follow.turn_on(), (false, Some(&second)), "it was on");
    }

    #[test]
    fn turning_on_before_the_agent_went_anywhere_goes_nowhere() {
        let mut follow = Follow::default();
        follow.person_acted();
        assert_eq!(follow.turn_on(), (true, None));
    }
}
