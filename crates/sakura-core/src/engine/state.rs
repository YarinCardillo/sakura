//! What the engine says of itself in its snapshot's `engine` part: its pace, the lanes it runs at once, the
//! preset in force and the keeper that holds lanes alive. Every field is optional: an older engine says
//! less, and a part it does not have is none, never a made-up default.

use super::accounts::AccountRef;
use super::presets::PresetChoice;
use serde::{Deserialize, Deserializer};

/// The engine as a whole.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
pub struct EngineState {
    /// `slowest`, `slow`, `normal`, `fast` or `fastest`.
    #[serde(default)]
    pub pace: Option<String>,
    /// Where each account is meant to land at its reset, in percent; none at `fastest`.
    #[serde(default, deserialize_with = "whole")]
    pub target_pct: Option<u32>,
    /// The fewest lanes that run at once, and the most.
    #[serde(default, deserialize_with = "whole")]
    pub lanes_min: Option<u32>,
    #[serde(default, deserialize_with = "whole")]
    pub lanes: Option<u32>,
    /// The preset in force, and whether a rule was changed since.
    #[serde(default)]
    pub preset: Option<String>,
    #[serde(default)]
    pub preset_label: Option<String>,
    #[serde(default)]
    pub edited: bool,
    /// Every preset a person may pick, in the engine's order, with the one in force marked `chosen` (also while
    /// its rules are edited); the legacy names are left out. Empty from an engine that lists none.
    #[serde(default)]
    pub presets: Vec<PresetChoice>,
    /// Whether lanes give way to the person's own sessions, and whether they do now.
    #[serde(default)]
    pub yield_to_person: bool,
    #[serde(default)]
    pub yielding: bool,
    #[serde(default)]
    pub keeper: Option<Keeper>,
    pub master_account: Option<AccountRef>,
    pub master_launch_route: Option<String>,
}

/// The keeper: lanes keep running without the app and resume after a restart.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
pub struct Keeper {
    #[serde(default)]
    pub on: bool,
    #[serde(default)]
    pub since: Option<String>,
    #[serde(default)]
    pub last_restart: Option<Restart>,
}

/// What the keeper did at the last restart.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
pub struct Restart {
    #[serde(default)]
    pub at: Option<String>,
    /// The lanes it resumed (`project/lane`), and those that wait for the person.
    #[serde(default)]
    pub resumed: Vec<String>,
    #[serde(default)]
    pub waiting: Vec<String>,
}

impl EngineState {
    /// Whether the engine says its keeper is on; an engine that does not say has none.
    pub fn is_keeper_on(&self) -> bool {
        self.keeper.as_ref().is_some_and(|keeper| keeper.on)
    }
}

/// A count or percentage as a whole number: the engine may print a float.
fn whole<'de, D: Deserializer<'de>>(reader: D) -> Result<Option<u32>, D::Error> {
    let value = Option::<f64>::deserialize(reader)?;
    Ok(value.map(|number| number.max(0.).round() as u32))
}

#[cfg(test)]
mod tests {
    use crate::engine::snapshot::Snapshot;

    fn parse(extra: &str) -> Snapshot {
        let text = format!(r#"{{"api": 1, "waits": [], "items": [] {extra}}}"#);
        Snapshot::parse(text.as_bytes()).expect("a snapshot")
    }

    #[test]
    fn snapshot_reads_the_engine_part_with_its_keeper_and_its_last_restart() {
        let snapshot = parse(
            r#", "engine": {"pace": "normal", "target_pct": 90, "lanes_min": 1, "lanes": 4,
                "preset": "standard", "edited": true, "yield_to_person": true,
                "keeper": {"on": true, "since": "2026-11-18T07:58:00Z",
                           "last_restart": {"at": "2026-11-18T07:58:00Z", "resumed": ["app/a", "app/b", "app/c"], "waiting": ["app/d"]}}}"#,
        );
        let engine = snapshot.engine.expect("the engine part");
        assert_eq!(engine.pace.as_deref(), Some("normal"));
        assert_eq!((engine.lanes_min, engine.lanes), (Some(1), Some(4)));
        assert_eq!(engine.preset.as_deref(), Some("standard"));
        assert!(engine.edited && engine.yield_to_person && !engine.yielding);
        assert!(engine.is_keeper_on());
        let restart = engine.keeper.and_then(|keeper| keeper.last_restart);
        assert_eq!(
            restart.map(|r| (r.resumed.len(), r.waiting.len())),
            Some((3, 1))
        );
    }

    #[test]
    fn an_older_engine_with_no_engine_part_gives_none_and_a_bare_one_gives_unknowns() {
        assert_eq!(parse("").engine, None);
        let bare = parse(r#", "engine": {}"#).engine.expect("present");
        assert!(!bare.is_keeper_on());
        assert_eq!((bare.pace, bare.lanes, bare.keeper), (None, None, None));
    }
}
