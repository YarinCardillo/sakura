//! The accounts of the book, whole in the rules. The engine lists in its rules the accounts it has read the
//! models of; an account added by path may not be among them yet, and its steps then name an id the person
//! never saw and offer no model. The snapshot lists every account the book has under its Leeway name: they are
//! added to the rules' list, each with the models its program lists in the rules' own choices, so the page names
//! and offers them as it does any other.

use sakura_core::engine::accounts::Account;
use sakura_core::engine::rules::{AgentAccount, AgentModel, Rules};

/// The `models_state` of an entry this page made from the book: the engine did not list it, so it has no list of
/// its own to read.
pub(super) const FROM_THE_BOOK: &str = "book";

/// Adds to `rules.agents` the visible accounts of `accounts` that it does not list: every one when it lists
/// none (the engine could not say), else those a step is pinned to (a step never names an account the page
/// cannot name).
pub(super) fn complete_agents(rules: &mut Rules, accounts: &[Account]) {
    let pinned: Vec<String> = rules
        .nodes
        .iter()
        .flat_map(|node| &node.rules)
        .filter_map(|rule| rule.agent.as_ref())
        .map(|agent| agent.account.clone())
        .collect();
    let lists_none = rules.agents.is_empty();
    for account in accounts.iter().filter(|account| !account.is_hidden()) {
        let is_wanted = lists_none || pinned.contains(&account.id);
        if !is_wanted || rules.agents.iter().any(|listed| listed.id == account.id) {
            continue;
        }
        let models = models_of_choices(rules, &account.provider);
        rules.agents.push(AgentAccount {
            id: account.id.clone(),
            label: crate::ui::account_words::display_name(account),
            provider: account.provider.clone(),
            models,
            models_state: FROM_THE_BOOK.to_string(),
            ..AgentAccount::default()
        });
    }
}

/// The models the rules' choices name for `provider` (`claude:sonnet` is Sonnet), each once, with the efforts the
/// effort rules offer.
fn models_of_choices(rules: &Rules, provider: &str) -> Vec<AgentModel> {
    if provider == "goose" {
        return Vec::new();
    }
    let prefix = format!("{provider}:");
    let all_rules = rules.nodes.iter().flat_map(|node| &node.rules);
    let efforts: Vec<String> = all_rules
        .clone()
        .find(|rule| rule.id.ends_with(".effort"))
        .map(|rule| rule.choices.clone())
        .unwrap_or_default();
    let mut seen = std::collections::HashSet::new();
    all_rules
        .filter(|rule| rule.id.ends_with(".model"))
        .flat_map(|rule| &rule.choices)
        .filter_map(|choice| choice.strip_prefix(&prefix))
        .filter(|id| seen.insert(id.to_string()))
        .map(|id| AgentModel {
            id: id.to_string(),
            label: super::words::model_words(&format!("{prefix}{id}")).to_string(),
            efforts: efforts.clone(),
            ..AgentModel::default()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The engine's own `rules --json` for a book with one account, added by path and named "Claude" (id
    /// `claude-aa0b79cab9b0ea15`): its `agents` list is empty.
    const ONE_ACCOUNT: &str =
        include_str!("../../../../sakura-core/tests/fixtures/engine-rules-one-account.json");

    fn the_book() -> Vec<Account> {
        serde_json::from_value(serde_json::json!([{
            "id": "claude-aa0b79cab9b0ea15", "label": "Claude", "provider": "claude"
        }]))
        .expect("the book")
    }

    fn rules() -> Rules {
        serde_json::from_str(ONE_ACCOUNT).expect("the engine's rules")
    }

    #[test]
    fn an_account_the_rules_do_not_list_is_named_as_leeway_names_it_with_its_programs_models() {
        let mut rules = rules();
        assert!(rules.agents.is_empty(), "the engine listed none");
        complete_agents(&mut rules, &the_book());
        assert_eq!(
            rules.account_label("claude-aa0b79cab9b0ea15"),
            Some("Claude")
        );
        let models: Vec<&str> = rules.agents[0]
            .models
            .iter()
            .map(|model| model.id.as_str())
            .collect();
        assert_eq!(models, ["haiku", "sonnet", "opus"]);
        assert_eq!(
            rules.agents[0].models[0].efforts,
            ["low", "medium", "high", "xhigh", "max", "ultra"]
        );
    }

    /// The engine's own rules (commit a13c991) for a step on an account whose models are being read: its name,
    /// the state and the words, and an `agents` entry with no list yet.
    fn rules_with_the_engine_s_names() -> Rules {
        let mut rules = rules();
        for node in &mut rules.nodes {
            for rule in &mut node.rules {
                if let Some(agent) = rule.agent.as_mut() {
                    agent.account_label = "Claude".into();
                    agent.models_state = "refreshing".into();
                    agent.models_note = "Reading the models of Claude.".into();
                }
            }
        }
        rules
    }

    #[test]
    fn a_step_says_the_name_the_engine_gives_its_account_before_any_list_of_accounts() {
        crate::l10n::set_for_test(crate::l10n::Language::English);
        let rules = rules_with_the_engine_s_names();
        assert!(rules.agents.is_empty());
        let pipeline = super::super::schema::Pipeline::from_rules(Some(&rules));
        let line = pipeline.agent_line(super::super::schema::Node::Build);
        assert_eq!(line.as_deref(), Some("Claude, Haiku medium"));
    }

    #[test]
    fn the_menu_of_a_step_whose_models_are_being_read_leads_with_the_engine_s_words_for_it() {
        let rules = rules_with_the_engine_s_names();
        let build = rules.rule("steps.build.model").expect("the build model");
        let agent = build.agent.as_ref().expect("an agent");
        let words = super::super::inspector::menu_notes(agent);
        assert_eq!(words, ["Reading the models of Claude."]);
        let kept = sakura_core::engine::rules::RuleAgent {
            models_state: "kept".into(),
            models_note: "stale words".into(),
            ..agent.clone()
        };
        assert!(super::super::inspector::menu_notes(&kept).is_empty());
    }

    #[test]
    fn an_account_the_rules_list_is_left_as_the_engine_gave_it() {
        let mut rules = rules();
        complete_agents(&mut rules, &the_book());
        let once = rules.agents.clone();
        complete_agents(&mut rules, &the_book());
        assert_eq!(rules.agents, once);
    }

    #[test]
    fn the_box_and_the_menu_of_a_step_on_that_account_say_claude_and_offer_models_and_efforts() {
        crate::l10n::set_for_test(crate::l10n::Language::English);
        let mut rules = rules();
        complete_agents(&mut rules, &the_book());
        let pipeline = super::super::schema::Pipeline::from_rules(Some(&rules));
        let line = pipeline
            .agent_line(super::super::schema::Node::Build)
            .expect("an agent line");
        assert_eq!(line, "Claude, Haiku medium");
        assert!(!line.contains("aa0b"), "no raw id");
        let build = rules.rule("steps.build.model").expect("the build model");
        let agent = build.agent.as_ref().expect("an agent");
        let models =
            super::super::inspector::models_for_test(&rules, &agent.account, &agent.provider);
        assert_eq!(models.len(), 3);
    }
}
