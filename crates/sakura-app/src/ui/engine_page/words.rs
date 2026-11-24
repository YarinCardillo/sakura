//! The words the Engine page uses for what the engine names with an id: a model (`claude:haiku`, `codex`) and
//! an account (`project`). The page shows Haiku, Sonnet, Opus, Codex and "The project's account", whole, and
//! sends the engine its own id back.

use crate::l10n::{tr, Key};
use gpui::SharedString;

/// The id the engine gives a step that runs on the project's own account.
const PROJECT_ACCOUNT: &str = "project";

/// A model as a person names it: the engine's `claude:haiku` is Haiku, `codex` is Codex and a spelling the
/// engine gives in words (`Claude Opus`) loses the provider's own word. An id this build has no word for is
/// its name with a capital.
pub fn model_words(model: &str) -> SharedString {
    let name = match model.split_once(':') {
        Some((_, name)) if !name.is_empty() => name,
        Some((provider, _)) => provider,
        None => model.strip_prefix("Claude ").unwrap_or(model),
    };
    if name.starts_with("gpt-") {
        return SharedString::from(crate::ui::agent_words::model_id_words(name));
    }
    let mut letters = name.chars();
    let words = match letters.next() {
        Some(first) => first.to_uppercase().chain(letters).collect(),
        None => String::new(),
    };
    SharedString::from(words)
}

/// An account as a person names it: the engine's `project` is the project's account; any other id or name
/// stays as the engine gave it.
pub fn account_words(account: &str) -> SharedString {
    match account == PROJECT_ACCOUNT {
        true => tr(Key("engine_page.account.project")).into(),
        false => SharedString::from(account.to_string()),
    }
}

/// The words of a rule's value: a model's, an account's, or the value as it is. `rule` is the rule's dotted id.
pub fn rule_value_words(rule: &str, value: &str) -> SharedString {
    match rule.rsplit('.').next() {
        Some("model") => model_words(value),
        Some("account") => account_words(value),
        _ => SharedString::from(value.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_codex_model_id_is_written_as_its_maker_writes_it() {
        assert_eq!(model_words("codex:gpt-5.5"), "GPT-5.5");
        assert_eq!(model_words("codex"), "Codex");
    }

    #[test]
    fn the_engine_s_model_ids_are_said_as_their_names() {
        let words = |id: &str| model_words(id).to_string();
        assert_eq!(words("claude:haiku"), "Haiku");
        assert_eq!(words("claude:sonnet"), "Sonnet");
        assert_eq!(words("claude:opus"), "Opus");
        assert_eq!(words("codex"), "Codex");
        assert_eq!(words("Claude Opus"), "Opus");
        assert_eq!(words("Codex"), "Codex");
        assert_eq!(words("claude:"), "Claude");
        assert_eq!(words(""), "");
    }

    #[test]
    fn the_project_s_account_is_said_in_words_and_any_other_account_as_named() {
        crate::l10n::set_for_test(crate::l10n::Language::English);
        assert_eq!(account_words("project"), "The project's account");
        assert_eq!(account_words("Work"), "Work");
    }

    #[test]
    fn a_rule_says_its_value_by_what_the_rule_is() {
        crate::l10n::set_for_test(crate::l10n::Language::English);
        let said = |rule: &str, value: &str| rule_value_words(rule, value).to_string();
        assert_eq!(said("steps.build.model", "claude:sonnet"), "Sonnet");
        assert_eq!(
            said("steps.build.account", "project"),
            "The project's account"
        );
        assert_eq!(said("report_language", "it"), "it");
    }
}
