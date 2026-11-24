//! The rules of how a lane runs, as `sakura engine rules --json` prints them: the preset in force and, for
//! each step of the pipeline, the rules it can be changed by. Each rule says its own type, so the app draws
//! a switch, a stepper or a choice without knowing what the rule means.

use serde::Deserialize;
use serde_json::Value;

/// The engine's rules.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Rules {
    /// The preset in force, and whether a rule was changed since it was chosen.
    pub preset: String,
    /// The preset in words (`Standard`), as the engine says it.
    #[serde(default)]
    pub preset_label: Option<String>,
    #[serde(default)]
    pub edited: bool,
    /// How many accounts the person has: with one, no rule names an account.
    #[serde(default)]
    pub accounts_count: u32,
    #[serde(default)]
    pub nodes: Vec<RuleNode>,
    /// Every account of the engine's registry, flat under its name, with the models its program lists.
    #[serde(default)]
    pub agents: Vec<AgentAccount>,
}

/// One account a step may run on, as the person knows it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct AgentAccount {
    pub id: String,
    /// Its name as Leeway gives it (`Claude`, `Claude Work`, `Codex`).
    #[serde(default)]
    pub label: String,
    /// `claude` or `codex`.
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub models: Vec<AgentModel>,
    /// Whether the lists are `kept`, being read (`refreshing`), could not be read (`unavailable`) or are not read
    /// yet (`missing`); empty from an engine that does not say.
    #[serde(default)]
    pub models_state: String,
    /// The engine's words for a list that is not kept; empty when it is.
    #[serde(default)]
    pub models_note: String,
    pub upstream_provider: Option<String>,
    pub fetched_at: Option<String>,
    pub source: Option<String>,
    pub models_reason: Option<String>,
}

/// A model its program lists, with the efforts it takes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct AgentModel {
    pub id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub efforts: Vec<String>,
    #[serde(default)]
    pub default_effort: Option<String>,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub is_default: bool,
    #[serde(default)]
    pub resolved: String,
}

/// What a step's model rule says of the agent it runs on.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RuleAgent {
    /// The account the step is pinned to, `project` when it runs on the project's own.
    #[serde(default)]
    pub account: String,
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub effort: String,
    /// Why the step runs on an account of another provider than its own, in the engine's words; empty when it
    /// does not.
    #[serde(default)]
    pub note: String,
    /// The name of the account the step is pinned to, as Leeway gives it; empty from an engine that does not say.
    #[serde(default, deserialize_with = "super::snapshot::none_when_null")]
    pub account_label: String,
    /// What is known of the lists of the step's account (see [`AgentAccount::models_state`]), with the words.
    #[serde(default)]
    pub models_state: String,
    #[serde(default)]
    pub models_note: String,
}

impl Rules {
    /// The preset in force in words: the engine's label for it, else its name with a capital.
    pub fn preset_name(&self) -> String {
        match &self.preset_label {
            Some(label) => label.clone(),
            None => super::snapshot::capitalised(&self.preset),
        }
    }
}

/// The rules of one step, review or fixed part of the pipeline: `start`, `plan`, `plan_review`, `build`,
/// `task_review`, `final_review`, `checks`, `deliver` or `guard`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RuleNode {
    pub node: String,
    #[serde(default)]
    pub rules: Vec<Rule>,
}

/// One rule: its dotted id (what a change is sent under), its words, its type and its value.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Rule {
    pub id: String,
    #[serde(default)]
    pub label: String,
    /// `bool`, `int` or `choice`; anything else is shown, not changed.
    #[serde(rename = "type")]
    pub kind: String,
    pub value: Value,
    #[serde(default)]
    pub choices: Vec<String>,
    #[serde(default)]
    pub min: Option<i64>,
    #[serde(default)]
    pub max: Option<i64>,
    /// Where the value comes from: the preset, the person's own layer.
    #[serde(default)]
    pub origin: String,
    #[serde(default)]
    pub read_only: bool,
    /// For a step's model rule: the account, program, model and effort it runs on.
    #[serde(default)]
    pub agent: Option<RuleAgent>,
}

impl Rules {
    /// The name of the account `id` as Leeway gives it; none for an account the engine does not list.
    pub fn account_label(&self, id: &str) -> Option<&str> {
        let found = self.agents.iter().find(|account| account.id == id);
        found
            .map(|account| account.label.as_str())
            .filter(|label| !label.is_empty())
    }

    /// The name of the model `choice` (`claude:opus`, `provider:model`) as its program lists it.
    pub fn model_label(&self, choice: &str) -> Option<&str> {
        let (provider, id) = choice.split_once(':')?;
        self.agents
            .iter()
            .filter(|account| account.provider == provider)
            .flat_map(|account| &account.models)
            .find(|model| model.id == id)
            .map(|model| model.label.as_str())
            .filter(|label| !label.is_empty())
    }

    /// The rules of the step `node`, when the engine has it.
    pub fn node(&self, node: &str) -> Option<&RuleNode> {
        self.nodes.iter().find(|candidate| candidate.node == node)
    }

    /// The rule `id`, wherever it is.
    pub fn rule(&self, id: &str) -> Option<&Rule> {
        self.nodes
            .iter()
            .flat_map(|node| &node.rules)
            .find(|rule| rule.id == id)
    }
}

impl Rule {
    /// The value as a switch's state; none when it is not one.
    pub fn as_bool(&self) -> Option<bool> {
        self.value.as_bool()
    }

    /// The value as a whole number; none when it is not one.
    pub fn as_int(&self) -> Option<i64> {
        self.value.as_i64()
    }

    /// The value as words: a choice's text, a number or a switch written out.
    pub fn as_text(&self) -> String {
        match &self.value {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../../tests/fixtures/engine-rules.json");

    #[test]
    fn the_engine_s_rules_read_with_their_types_values_and_limits() {
        let rules: Rules = serde_json::from_str(FIXTURE).expect("the fixture");
        assert_eq!(
            (rules.preset.as_str(), rules.edited, rules.accounts_count),
            ("standard", false, 3)
        );
        let review = rules.rule("reviews.plan_review.on").expect("the rule");
        assert_eq!(
            (review.kind.as_str(), review.as_bool()),
            ("bool", Some(true))
        );
        let rounds = rules.rule("reviews.task_review.rounds").expect("the rule");
        assert_eq!(
            (rounds.as_int(), rounds.min, rounds.max),
            (Some(2), Some(0), Some(5))
        );
        let model = rules.rule("steps.build.model").expect("the rule");
        assert_eq!(model.as_text(), "Claude Sonnet");
        assert!(rules
            .node("guard")
            .expect("the guard")
            .rules
            .iter()
            .all(|rule| rule.read_only));
        assert!(rules.node("nowhere").is_none());
    }

    #[tokio::test]
    async fn the_cli_reads_the_rules_the_engine_prints_and_a_refusal_is_an_error() {
        use crate::engine::cli::EngineCli;
        let dir = tempfile::tempdir().expect("a folder");
        let write = |name: &str, body: &str| {
            let path = dir.path().join(name);
            crate::test_support::write_script(&path, &format!("#!/bin/sh\n{body}\n"));
            EngineCli::at(path)
        };
        let answer = dir.path().join("rules.json");
        std::fs::write(&answer, FIXTURE).expect("a file");
        let body = format!(
            "echo \"$*\" > '{}/args'\ncat '{}'",
            dir.path().display(),
            answer.display()
        );
        let rules = write("sakura", &body).rules().await.expect("rules");
        assert_eq!(rules.preset, "standard");
        let args = std::fs::read_to_string(dir.path().join("args")).expect("the arguments");
        assert_eq!(args.trim(), "engine rules --json");
        let refused = write("refusing", "echo 'no rules here'; exit 2")
            .rules()
            .await;
        assert!(
            refused.is_err(),
            "an engine that does not know the command is an error"
        );
    }

    #[test]
    fn a_document_with_only_a_preset_is_still_rules() {
        let rules: Rules = serde_json::from_str(r#"{"preset": "solo"}"#).expect("rules");
        assert!(rules.nodes.is_empty());
    }
}

#[cfg(test)]
mod agent_tests {
    use super::*;

    fn rules() -> Rules {
        serde_json::from_str(
            r#"{"preset": "standard", "nodes": [], "agents": [
              {"id": "a", "label": "Claude Work", "provider": "claude",
               "models": [{"id": "opus", "label": "Opus 5.5", "efforts": ["low", "high"], "default_effort": "high"}]},
              {"id": "b", "label": "Codex", "provider": "codex",
               "models": [{"id": "gpt-6.2-sol", "label": "GPT-6.2-Sol", "efforts": ["medium"]}]}]}"#,
        )
        .expect("rules")
    }

    #[test]
    fn an_account_and_a_model_are_named_as_their_program_and_leeway_name_them() {
        let rules = rules();
        assert_eq!(rules.account_label("a"), Some("Claude Work"));
        assert_eq!(rules.account_label("zz"), None);
        assert_eq!(rules.model_label("claude:opus"), Some("Opus 5.5"));
        assert_eq!(rules.model_label("codex:gpt-6.2-sol"), Some("GPT-6.2-Sol"));
        assert_eq!(rules.model_label("claude:gpt-6.2-sol"), None);
    }
}
