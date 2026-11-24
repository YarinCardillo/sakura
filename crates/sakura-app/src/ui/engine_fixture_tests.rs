//! The engine's fixture with the parts the redesigned engine adds (its state, accounts, automations and
//! connectors), for the tests of the Engine page and of Settings.

use super::engine_rig_tests::SNAPSHOT;
use serde_json::Value;

/// The parts the redesigned engine adds, as it prints them.
const EXTRAS: &str = include_str!("../../../sakura-core/tests/fixtures/engine-extras.json");

/// The snapshot of the fixture with every added part.
pub(super) fn full() -> String {
    edited(|_| {})
}

/// The full snapshot, changed by `change`.
pub(super) fn edited(change: impl FnOnce(&mut Value)) -> String {
    let mut snapshot: Value = serde_json::from_str(SNAPSHOT).expect("the fixture");
    let extras: Value = serde_json::from_str(EXTRAS).expect("the extras");
    for (name, part) in extras.as_object().expect("an object") {
        snapshot[name.as_str()] = part.clone();
    }
    change(&mut snapshot);
    snapshot.to_string()
}

/// The full snapshot of a person with one account.
pub(super) fn one_account() -> String {
    edited(|snapshot| {
        let accounts = snapshot["accounts"].as_array_mut().expect("the accounts");
        accounts.truncate(1);
    })
}

/// The full snapshot with the accounts as a real person has them: two Claude accounts and a Codex one, the
/// plans labelled as the engine labels them ("Claude Max" under an account named "Claude"), the first on its
/// way to run out of its week.
pub(super) fn owner_accounts() -> String {
    edited(|snapshot| {
        let model = snapshot["accounts"][0].clone();
        let account = |id: &str, label: &str, provider: &str, plan: &str, used: u32, end: u32| {
            let mut account = model.clone();
            account["id"] = id.into();
            account["key"] = id.into();
            account["label"] = label.into();
            account["provider"] = provider.into();
            account["plan"] = "plan".into();
            account["plan_label"] = plan.into();
            account["week_pct"] = used.into();
            account["forecast_at_reset_pct"] = end.into();
            account["week_resets_at"] = "2026-11-21T17:00:00Z".into();
            account["enabled"] = true.into();
            account["hidden"] = false.into();
            account
        };
        snapshot["usage"] = serde_json::json!({"as_of": "2026-11-18T07:55:00Z"});
        snapshot["accounts"] = Value::Array(vec![
            account("claude-a", "Claude", "claude", "Claude Max", 50, 131),
            account("codex-b", "Codex", "codex", "Codex Pro Lite", 9, 35),
            account("claude-c", "Claude Work", "claude", "Claude Max", 1, 1),
        ]);
    })
}

/// The rules of the fixture, changed by `change`.
pub(super) fn rules_edited(change: impl FnOnce(&mut Value)) -> String {
    let mut rules: Value = serde_json::from_str(super::engine_rig_tests::RULES).expect("the rules");
    change(&mut rules);
    rules.to_string()
}

/// The rules of a person with one account: no rule names an account.
pub(super) fn rules_for_one_account() -> String {
    rules_edited(|rules| {
        rules["accounts_count"] = 1.into();
        for node in rules["nodes"].as_array_mut().expect("nodes") {
            let kept: Vec<Value> = node["rules"]
                .as_array()
                .expect("rules")
                .iter()
                .filter(|rule| !rule["id"].as_str().unwrap_or("").ends_with(".account"))
                .cloned()
                .collect();
            node["rules"] = kept.into();
        }
    })
}
