//! The things the engine does on its own, as its snapshot lists them: the services that are always on (the
//! keeper, the phone page) and the scheduled automations, and, beside them, the morning digest's settings and
//! sources. The engine puts the first under `automations.items` and the second under `digest`.

use serde::{Deserialize, Deserializer};

/// The names of the services that run all the time; the engine lists them among the automations.
const SERVICE_NAMES: [&str; 2] = ["keeper", "phone-page"];

/// One service or scheduled automation.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Automation {
    /// What names it for the engine: `keeper`, `phone-page`, or an automation's own name.
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// When it runs, as the engine writes it: `daily 06:30`, `every 60 minutes`, `always on`.
    #[serde(default)]
    pub schedule_text: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub installed: bool,
    #[serde(default)]
    pub last_run_at: Option<String>,
    /// How the last run went; none before the first.
    #[serde(default)]
    pub last_result: Option<String>,
}

impl Automation {
    /// Whether this is an always-on service (the keeper, the phone page) and not a scheduled automation.
    pub fn is_service(&self) -> bool {
        SERVICE_NAMES.contains(&self.name.as_str())
    }
}

/// The settings of the morning digest, with its sources.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
pub struct Digest {
    /// The time of day it runs, `HH:MM`.
    #[serde(default)]
    pub at: String,
    #[serde(default)]
    pub lookback_hours: Option<u32>,
    /// A language code, `en` or `it`.
    #[serde(default)]
    pub language: String,
    #[serde(default)]
    pub model: String,
    /// The account it runs on; none (the engine's empty string) leaves it to the project's.
    #[serde(default, deserialize_with = "none_when_empty")]
    pub account: Option<String>,
    #[serde(default)]
    pub next_at: Option<String>,
    #[serde(default)]
    pub last_run_at: Option<String>,
    #[serde(default)]
    pub sources: Vec<DigestSource>,
    /// Why the settings could not be read, as the engine says it.
    #[serde(default)]
    pub problem: Option<String>,
}

/// One place the digest reads: a connector, a feed or a program.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DigestSource {
    pub id: String,
    /// What the source is called, as the engine words it: its own label, else its connector's name, else its
    /// id as words.
    #[serde(default)]
    pub label: String,
    /// What it reads, in one phrase; empty when the engine has none.
    #[serde(default)]
    pub reads: String,
    /// `connector`, `feed` or `program`.
    #[serde(default)]
    pub kind: String,
    /// The connector it reads through, when it is one.
    #[serde(default)]
    pub connector: Option<String>,
    #[serde(default)]
    pub enabled: bool,
    /// When it was last read without a failure.
    #[serde(default)]
    pub last_good_run: Option<String>,
}

impl DigestSource {
    /// What the source is called: the engine's name for it, else the connector it reads through, else its id.
    pub fn name(&self) -> &str {
        match (self.label.is_empty(), self.connector.as_deref()) {
            (false, _) => &self.label,
            (true, Some(connector)) => connector,
            (true, None) => &self.id,
        }
    }
}

/// An empty string is no value.
fn none_when_empty<'de, D: Deserializer<'de>>(reader: D) -> Result<Option<String>, D::Error> {
    let value = Option::<String>::deserialize(reader)?;
    Ok(value.filter(|text| !text.is_empty()))
}

#[cfg(test)]
mod tests {
    use crate::engine::snapshot::Snapshot;

    fn parse(extra: &str) -> Snapshot {
        let text = format!(r#"{{"api": 1, "waits": [], "items": [] {extra}}}"#);
        Snapshot::parse(text.as_bytes()).expect("a snapshot")
    }

    #[test]
    fn snapshot_reads_the_services_the_schedules_and_the_digest_with_its_sources() {
        let snapshot = parse(
            r#", "automations": {"items": [
              {"name": "keeper", "schedule_text": "always on", "enabled": true, "last_result": null},
              {"name": "weekly-wake", "description": "Wake the lanes", "schedule_text": "daily 07:00",
               "enabled": true, "last_run_at": "2026-11-18T07:00:00Z", "last_result": "ok"}],
              "target": "[\"host\",\"automations\"]", "actions": []},
              "digest": {"at": "07:00", "lookback_hours": 24, "language": "it", "account": "work",
                         "sources": [{"id": "github", "kind": "connector", "connector": "github",
                                      "enabled": true, "last_good_run": "2026-11-18T07:00:00Z"}]}"#,
        );
        let [keeper, wake] = snapshot
            .automations
            .expect("listed")
            .try_into()
            .expect("two");
        assert!(keeper.is_service() && keeper.enabled && !wake.is_service());
        assert_eq!(wake.last_result.as_deref(), Some("ok"));
        let settings = snapshot.digest.expect("the digest's settings");
        assert_eq!(
            (settings.at.as_str(), settings.lookback_hours),
            ("07:00", Some(24))
        );
        assert_eq!(settings.account.as_deref(), Some("work"));
        assert_eq!(settings.sources[0].id, "github");
    }

    #[test]
    fn an_engine_with_no_automations_part_gives_none() {
        let snapshot = parse("");
        assert_eq!((snapshot.automations, snapshot.digest), (None, None));
    }
}
