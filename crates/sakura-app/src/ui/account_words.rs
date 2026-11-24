//! How an account is named in a list: its own name on one line, and under it what kind of account it is.
//! The provider and the plan are said once: a plan the engine labels "Claude Max" under an account named
//! "Claude" reads "Max", never "Claude Claude Max".

use crate::l10n::{tr, Key};
use sakura_core::engine::accounts::Account;
#[cfg(not(feature = "boxes"))]
use sakura_core::engine::accounts::Readiness;

pub(crate) fn display_name(account: &Account) -> String {
    if account.label.trim().is_empty() {
        return unnamed();
    }
    account.label.clone()
}

pub(crate) fn unnamed() -> String {
    tr(Key("settings_modal.accounts.unnamed")).to_string()
}

/// The provider's own name: the word the engine gave, capitalised.
#[cfg_attr(feature = "boxes", allow(dead_code))]
pub(crate) fn provider_name(provider: &str) -> String {
    match provider {
        "claude" => tr(Key("settings_modal.accounts.provider.claude")).to_string(),
        "codex" => tr(Key("settings_modal.accounts.provider.codex")).to_string(),
        "goose" => tr(Key("settings_modal.accounts.provider.goose")).to_string(),
        other => other.to_string(),
    }
}

/// The plan in words, or the provider if it adds information to the account name.
#[cfg_attr(feature = "boxes", allow(dead_code))]
pub(crate) fn kind_line(account: &Account) -> Option<String> {
    let plan = account.plan_name();
    let kind = plan
        .clone()
        .unwrap_or_else(|| provider_name(&account.provider));
    let name = display_name(account).to_lowercase();
    let said: Vec<&str> = name.split_whitespace().collect();
    let left: Vec<&str> = kind
        .split_whitespace()
        .filter(|word| !said.contains(&word.to_lowercase().as_str()))
        .collect();
    if left.is_empty() {
        plan
    } else {
        Some(left.join(" "))
    }
}

#[cfg(not(feature = "boxes"))]
pub(crate) fn readiness_words(state: Readiness, reason: Option<&str>) -> &'static str {
    reason_words(reason).unwrap_or_else(|| {
        tr(match state {
            Readiness::Ready => Key("goose.checked"),
            Readiness::Unknown => Key("goose.unknown"),
            Readiness::NeedsSetup => Key("goose.needs_setup"),
            Readiness::Unavailable => Key("goose.unavailable"),
        })
    })
}

#[cfg(not(feature = "boxes"))]
pub(crate) fn reason_words(reason: Option<&str>) -> Option<&'static str> {
    Some(tr(match reason? {
        "install_program" => Key("goose.install_program"),
        "install_adapter" => Key("goose.install_adapter"),
        "qualification_required" | "release_proof_required" | "master_release_proof_required" => {
            Key("goose.qualification_required")
        }
        "incompatible_install" => Key("goose.incompatible_install"),
        "program_untrusted" => Key("goose.program_untrusted"),
        "safe_setup_required" => Key("goose.safe_setup_required"),
        "setup_required" => Key("goose.setup_required"),
        "configuration_changed" | "goose_account_configuration_changed" => {
            Key("goose.configuration_changed")
        }
        "connection_failed" => Key("goose.connection_failed"),
        "account_disabled" => Key("goose.account_disabled"),
        "unsupported_capability" => Key("goose.unsupported_capability"),
        "connection_required" | "goose_account_connection_check_required" => Key("goose.unknown"),
        _ => return None,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(label: &str, provider: &str, plan_label: Option<&str>) -> Account {
        let mut account: Account = serde_json::from_value(serde_json::json!({
            "id": format!("{provider}-1"),
            "label": label,
            "provider": provider,
        }))
        .expect("an account");
        account.plan = plan_label.map(|_| "plan".to_string());
        account.plan_label = plan_label.map(str::to_string);
        account
    }

    #[test]
    fn a_goose_account_uses_the_harness_name_in_both_locales() {
        for language in [
            crate::l10n::Language::English,
            crate::l10n::Language::Italian,
        ] {
            crate::l10n::set_for_test(language);
            assert_eq!(
                kind_line(&account("Local models", "goose", None)).as_deref(),
                Some("Goose")
            );
        }
    }

    #[test]
    fn an_account_named_for_its_provider_shows_only_the_rest_of_its_plan() {
        let claude = account("Claude", "claude", Some("Claude Max"));
        assert_eq!(kind_line(&claude).as_deref(), Some("Max"));
        let codex = account("Codex", "codex", Some("Codex Pro Lite"));
        assert_eq!(kind_line(&codex).as_deref(), Some("Pro Lite"));
    }

    #[test]
    fn a_word_the_name_already_has_is_dropped_wherever_it_stands() {
        let second = account("Claude Work", "claude", Some("Claude Max"));
        assert_eq!(kind_line(&second).as_deref(), Some("Max"));
    }

    #[test]
    fn a_name_with_no_word_of_the_plan_keeps_the_whole_plan() {
        let studio = account("Studio", "claude", Some("Claude Max"));
        assert_eq!(kind_line(&studio).as_deref(), Some("Claude Max"));
    }

    #[test]
    fn an_account_with_no_plan_says_its_provider_unless_its_name_does() {
        assert_eq!(
            kind_line(&account("Archive", "codex", None)).as_deref(),
            Some("Codex")
        );
        assert_eq!(kind_line(&account("Codex", "codex", None)), None);
    }

    #[test]
    fn a_reported_plan_is_visible_when_the_name_already_contains_it() {
        assert_eq!(
            kind_line(&account("Claude Max", "claude", Some("Max"))).as_deref(),
            Some("Max")
        );
    }
}
