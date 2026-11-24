//! The accounts and the connectors the engine lists in its snapshot. Every field the redesigned engine adds
//! is optional, so an older engine's rows still read.

use super::snapshot::Action;
use serde::{Deserialize, Deserializer};

/// A provider-scoped account identity.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, serde::Serialize)]
pub struct AccountRef {
    pub provider: String,
    pub id: String,
}

/// The upstream provider configured through Goose.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, serde::Serialize)]
pub struct ProviderConfig {
    pub provider: String,
}

/// The result of the engine's last structural scan or connection check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Readiness {
    Unknown,
    NeedsSetup,
    Ready,
    Unavailable,
}

/// One account the engine spends: a provider's sign-in with its own weekly reset.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Account {
    pub id: String,
    #[serde(default)]
    pub label: String,
    /// `claude`, `codex` or `goose`.
    #[serde(default)]
    pub provider: String,
    pub readiness: Option<Readiness>,
    pub reason: Option<String>,
    pub provider_config: Option<ProviderConfig>,
    pub forecast_available: Option<bool>,
    /// The plan the account is on, as the provider names it; none while the engine has not read one.
    #[serde(default)]
    pub plan: Option<String>,
    /// The plan in words (`Pro Lite`), when the engine gives them; its own id otherwise.
    #[serde(default)]
    pub plan_label: Option<String>,
    /// How much of the week is used, in percent.
    #[serde(default, deserialize_with = "whole_percent")]
    pub week_pct: Option<u32>,
    #[serde(default)]
    pub week_resets_at: Option<String>,
    /// Where the account is forecast to be when its week resets, in percent.
    #[serde(default, deserialize_with = "whole_percent")]
    pub forecast_at_reset_pct: Option<u32>,
    #[serde(default)]
    pub exhausts_at: Option<String>,
    /// The reading is old.
    #[serde(default)]
    pub stale: bool,
    #[serde(default)]
    pub hidden: bool,
    /// The older engines' switch: `false` is an account turned off.
    #[serde(default)]
    pub enabled: Option<bool>,
    /// What an action on this account is sent to.
    #[serde(default)]
    pub target: String,
    #[serde(default)]
    pub actions: Vec<Action>,
}

impl Account {
    /// Whether the person hid the account (or turned it off): it is not spent and not listed in Pace.
    pub fn is_hidden(&self) -> bool {
        self.hidden || self.enabled == Some(false)
    }

    /// What the account is called: its label, else its id.
    pub fn name(&self) -> &str {
        if self.label.is_empty() {
            &self.id
        } else {
            &self.label
        }
    }

    /// The plan the account is on, in words: the engine's label for it, else its id with a capital.
    pub fn plan_name(&self) -> Option<String> {
        self.plan_label
            .as_deref()
            .filter(|label| !label.trim().is_empty())
            .map(str::to_string)
            .or_else(|| {
                let plan = self.plan.as_deref()?.trim();
                (!plan.is_empty()).then(|| super::snapshot::capitalised(plan))
            })
    }

    /// The action `key` of the account, when the engine offers it.
    pub fn action(&self, key: &str) -> Option<&Action> {
        self.actions.iter().find(|action| action.key == key)
    }
}

/// What the engine says of the usage readings of the accounts as a whole.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
pub struct Usage {
    /// When the newest reading was taken (the engine's ISO text); none while no account has been read.
    #[serde(default)]
    pub as_of: Option<String>,
}

/// One place the engine reads for the person: a sign-in through its own tool, a feed, a program.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Connector {
    pub id: String,
    /// `github`, `notion`, `linear`, `feed` or `program`.
    pub kind: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub connected: bool,
    /// What the engine says of it: who it is signed in as, or why it is not connected.
    #[serde(default)]
    pub detail: String,
    #[serde(default)]
    pub last_used_at: Option<String>,
    pub event_kinds: Option<Vec<String>>,
    pub revision: Option<u64>,
    pub scope: Option<super::connectors::ConnectorScope>,
    pub reads: Option<super::connectors::ConnectorReads>,
    pub operations: Option<Vec<String>>,
    pub events: Option<Vec<String>>,
    pub status: Option<String>,
    pub dependent_tasks: Option<Vec<super::connectors::DependentTask>>,
}

/// A percentage as a whole number: the engine may print a float.
fn whole_percent<'de, D: Deserializer<'de>>(reader: D) -> Result<Option<u32>, D::Error> {
    let value = Option::<f64>::deserialize(reader)?;
    Ok(value.map(|percent| percent.max(0.).round() as u32))
}

#[cfg(test)]
mod tests {
    use crate::engine::snapshot::Snapshot;

    fn parse(extra: &str) -> Snapshot {
        let text = format!(r#"{{"api": 1, "waits": [], "items": [] {extra}}}"#);
        Snapshot::parse(text.as_bytes()).expect("a snapshot")
    }

    #[test]
    fn accounts_show_a_reported_plan_label_without_a_plan_id() {
        let account: super::Account = serde_json::from_str(
            r#"{"id":"claude","label":"Claude","provider":"claude","plan_label":"Claude Max"}"#,
        )
        .unwrap();
        assert_eq!(account.plan_name().as_deref(), Some("Claude Max"));
    }

    #[test]
    fn snapshot_reads_the_accounts_the_engine_lists_with_their_forecast() {
        let snapshot = parse(
            r#", "accounts": [
              {"id": "personal", "label": "Personal", "provider": "claude", "plan": "Max",
               "week_pct": 41.4, "week_resets_at": "2026-11-23T09:00:00Z", "forecast_at_reset_pct": 86,
               "hidden": false, "target": "[\"account\",\"claude\",\"personal\"]",
               "actions": [{"key": "l", "label": "Rename", "needs": ["label"], "optional": [], "confirm": false}]},
              {"id": "work", "provider": "claude", "enabled": false}]"#,
        );
        let [personal, work] = snapshot.accounts.as_slice() else {
            panic!("two accounts: {:?}", snapshot.accounts);
        };
        assert_eq!((personal.name(), personal.week_pct), ("Personal", Some(41)));
        assert_eq!(personal.forecast_at_reset_pct, Some(86));
        assert!(personal.action("l").is_some() && personal.action("n").is_none());
        assert_eq!(work.name(), "work", "no label: the id");
        assert!(work.is_hidden(), "an account turned off is hidden");
        assert!(!personal.is_hidden());
    }

    #[test]
    fn a_plan_is_named_by_the_engine_s_label_else_by_its_id_with_a_capital() {
        let snapshot = parse(
            r#", "accounts": [
              {"id": "a", "plan": "prolite", "plan_label": "Pro Lite"},
              {"id": "b", "plan": "max"},
              {"id": "c"}]"#,
        );
        let names: Vec<_> = snapshot.accounts.iter().map(|a| a.plan_name()).collect();
        assert_eq!(names, [Some("Pro Lite".into()), Some("Max".into()), None]);
    }

    #[test]
    fn an_engine_with_no_accounts_part_gives_no_accounts() {
        assert!(parse("").accounts.is_empty());
    }

    #[test]
    fn snapshot_reads_the_connectors_and_tells_an_absent_part_from_an_empty_one() {
        let listed = parse(
            r#", "connectors": [{"id": "github", "kind": "github", "label": "GitHub", "connected": true,
               "detail": "Signed in", "last_used_at": "2026-11-18T07:00:00Z"},
              {"id": "linear", "kind": "linear"}]"#,
        );
        let connectors = listed.connectors.expect("listed");
        assert_eq!(connectors.len(), 2);
        assert!(connectors[0].connected && !connectors[1].connected);
        assert_eq!(parse(r#", "connectors": []"#).connectors, Some(Vec::new()));
        assert_eq!(parse("").connectors, None);
    }
}
