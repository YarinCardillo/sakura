//! The accounts the master can run on: every account of the engine's list, one flat entry under the name Leeway
//! gives it, each running one program (Claude Code or Codex). A conversation's account is fixed: another entry
//! opens a new conversation.

use super::master_agent::MasterAgent;
use sakura_core::engine::snapshot::Snapshot;
use std::collections::BTreeMap;

/// One account the master may run on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MasterAccount {
    pub id: String,
    /// Its name as Leeway gives it.
    pub label: String,
    /// The program it runs: its provider.
    pub agent: MasterAgent,
}

/// The accounts of `snapshot` the master can run on, in the engine's order: those that are on and whose
/// program the app knows.
pub fn listed(snapshot: &Snapshot) -> Vec<MasterAccount> {
    snapshot
        .accounts
        .iter()
        .filter(|account| !account.is_hidden())
        .filter_map(|account| {
            let agent = MasterAgent::ALL
                .into_iter()
                .find(|agent| agent.code() == account.provider)?;
            Some(MasterAccount {
                id: account.id.clone(),
                label: super::account_words::display_name(account),
                agent,
            })
        })
        .collect()
}

/// The key a conversation's session is kept under: the account, and for a conversation held by a tab of its
/// own, the tab's slot after it. The first conversation has no slot, so the sessions kept before still open.
pub fn session_key(account: &str, slot: &str) -> String {
    match slot.is_empty() {
        true => account.to_string(),
        false => format!("{account}@{slot}"),
    }
}

const HOST_ACCOUNT: &str = "@host";

pub(super) fn conversation_key(
    agent: MasterAgent,
    account: &str,
    session: String,
) -> sakura_core::engine::tracks::ConversationKey {
    let account = if account.is_empty() {
        HOST_ACCOUNT
    } else {
        account
    };
    sakura_core::engine::tracks::ConversationKey {
        provider: agent.code().into(),
        account: account.into(),
        session,
    }
}

pub(super) fn stored_account(key: &sakura_core::engine::tracks::ConversationKey) -> &str {
    if key.account == HOST_ACCOUNT {
        ""
    } else {
        &key.account
    }
}

/// The account a new conversation starts on: `chosen` when it is listed, else the first Claude account, else
/// the first account. None when there are none.
pub fn default_account(accounts: &[MasterAccount], chosen: Option<&str>) -> Option<String> {
    let listed = chosen.and_then(|id| accounts.iter().find(|account| account.id == id));
    let first_claude = accounts
        .iter()
        .find(|account| account.agent == MasterAgent::Claude);
    let found = listed.or(first_claude).or(accounts.first());
    found.map(|account| account.id.clone())
}

pub fn default_with_engine<'a>(
    accounts: &'a [MasterAccount],
    chosen: Option<&str>,
    engine: Option<&sakura_core::engine::accounts::AccountRef>,
) -> Option<&'a MasterAccount> {
    let saved = chosen.and_then(|id| accounts.iter().find(|account| account.id == id));
    let selected = engine.and_then(|reference| {
        accounts.iter().find(|account| {
            account.id == reference.id && account.agent.code() == reference.provider
        })
    });
    saved.or(selected).or_else(|| {
        let id = default_account(accounts, None)?;
        accounts.iter().find(|account| account.id == id)
    })
}

/// The name the account chip says: the conversation's account as the engine lists it, else the name saved
/// with it; else the account new conversations use (Settings) in the same two ways; else the first account.
/// None when there is no account at all, and only then does the chip say the program's name.
pub fn chip_name(
    accounts: &[MasterAccount],
    conversation: Option<&str>,
    saved: &BTreeMap<String, String>,
    new_conversations: Option<&str>,
) -> Option<String> {
    let named = |id: &str| {
        let listed = accounts.iter().find(|account| account.id == id);
        listed
            .map(|account| account.label.clone())
            .or_else(|| saved.get(id).cloned())
    };
    conversation
        .and_then(named)
        .or_else(|| new_conversations.and_then(named))
        .or_else(|| accounts.first().map(|account| account.label.clone()))
}

/// What the engine is told with `--account`: nothing while no account is chosen, which is the host's own.
pub fn engine_argument(id: &str) -> Option<String> {
    (!id.is_empty()).then(|| id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unlabelled_account_is_named_without_exposing_its_id() {
        crate::l10n::set_for_test(crate::l10n::Language::English);
        let snapshot = Snapshot::parse(
            br#"{"api":1,"items":[],"accounts":[{"id":"claude-opaque-id","label":" ","provider":"claude"}]}"#,
        )
        .unwrap();
        let listed = listed(&snapshot);
        assert_eq!(listed[0].id, "claude-opaque-id");
        assert_eq!(listed[0].label, "Unnamed account");
    }

    #[test]
    fn a_tab_s_conversation_is_kept_apart_from_the_first_one_of_the_same_account() {
        assert_eq!(session_key("work", ""), "work");
        assert_eq!(session_key("work", "3"), "work@3");
        assert_eq!(session_key("", "3"), "@3");
    }

    fn account(id: &str, agent: MasterAgent) -> MasterAccount {
        MasterAccount {
            id: id.into(),
            label: id.into(),
            agent,
        }
    }

    #[test]
    fn a_new_conversation_starts_on_the_account_chosen_else_the_first_claude_account_else_the_first(
    ) {
        use MasterAgent::{Claude, Codex};
        let mixed = [
            account("c", Codex),
            account("a", Claude),
            account("b", Claude),
        ];
        assert_eq!(default_account(&mixed, None).as_deref(), Some("a"));
        assert_eq!(default_account(&mixed, Some("b")).as_deref(), Some("b"));
        assert_eq!(
            default_account(&mixed, Some("gone")).as_deref(),
            Some("a"),
            "an account no longer listed is not kept"
        );
        let only_codex = [account("c", Codex), account("d", Codex)];
        assert_eq!(default_account(&only_codex, None).as_deref(), Some("c"));
        assert_eq!(default_account(&[], None), None);
    }

    fn names(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(id, name)| (id.to_string(), name.to_string()))
            .collect()
    }

    fn named(id: &str, label: &str) -> MasterAccount {
        MasterAccount {
            label: label.into(),
            ..account(id, MasterAgent::Claude)
        }
    }

    #[test]
    fn the_chip_names_the_conversation_s_account_first_as_the_engine_lists_it() {
        let listed = [named("a", "Personal"), named("b", "Work")];
        let saved = names(&[("b", "Old work name")]);
        let found = chip_name(&listed, Some("b"), &saved, Some("a"));
        assert_eq!(found.as_deref(), Some("Work"));
    }

    #[test]
    fn the_chip_names_the_name_saved_with_the_conversation_while_its_account_is_not_listed() {
        let saved = names(&[("b", "Work"), ("a", "Personal")]);
        let found = chip_name(&[], Some("b"), &saved, Some("a"));
        assert_eq!(found.as_deref(), Some("Work"));
        let others = [named("c", "Other")];
        let found = chip_name(&others, Some("b"), &saved, None);
        assert_eq!(
            found.as_deref(),
            Some("Work"),
            "an account that is gone keeps its name"
        );
    }

    #[test]
    fn the_chip_names_the_account_new_conversations_use_when_the_conversation_names_none() {
        let listed = [named("a", "Personal"), named("b", "Work")];
        let found = chip_name(&listed, None, &BTreeMap::new(), Some("b"));
        assert_eq!(found.as_deref(), Some("Work"));
        let saved = names(&[("b", "Work")]);
        let found = chip_name(&[], None, &saved, Some("b"));
        assert_eq!(
            found.as_deref(),
            Some("Work"),
            "by the saved name while the list loads"
        );
    }

    #[test]
    fn the_chip_names_the_first_account_when_nothing_else_names_one_and_nothing_when_there_is_none()
    {
        let listed = [named("a", "Personal"), named("b", "Work")];
        let found = chip_name(&listed, None, &BTreeMap::new(), None);
        assert_eq!(found.as_deref(), Some("Personal"));
        assert_eq!(chip_name(&[], None, &BTreeMap::new(), None), None);
    }

    #[test]
    fn no_account_chosen_is_never_named_to_the_engine() {
        assert_eq!(engine_argument(""), None);
        assert_eq!(engine_argument("work"), Some("work".to_string()));
    }
}
