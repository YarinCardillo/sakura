//! What a master conversation runs on, kept by the agent's session: the model, the effort and the mode the
//! agent last said, so that a conversation reopened later resumes on them and not on the engine's defaults.

use super::thread::StartChoice;
use sakura_acp::model::SessionChoices;
use serde_json::{json, Value};

/// The modes of the app; the adapter's own are never kept.
const KEPT_MODES: [&str; 2] = ["default", "plan"];

pub(super) fn is_master_mode(mode: &str) -> bool {
    KEPT_MODES.contains(&mode)
}

/// What `session` says it runs on, as a choice to start with; empty before it says anything.
pub(super) fn picks_of(session: &SessionChoices) -> StartChoice {
    let current = |selector: &Option<sakura_acp::model::Selector>| {
        let selector = selector.as_ref()?;
        Some(selector.current.clone()).filter(|id| !id.is_empty())
    };
    StartChoice {
        model: current(&session.models),
        effort: current(&session.efforts),
        mode: current(&session.modes).filter(|mode| is_master_mode(mode)),
        uses_session_effort: session.uses_session_effort.unwrap_or(false),
    }
}

/// The preferences' words for `picks`.
pub(super) fn to_json(picks: &StartChoice) -> Value {
    json!({
        "model": picks.model, "effort": picks.effort, "mode": picks.mode,
        "uses_session_effort": picks.uses_session_effort,
    })
}

/// The choice the preferences kept; what cannot be read is left out.
pub(super) fn from_json(kept: &Value) -> StartChoice {
    let text = |key: &str| kept.get(key)?.as_str().map(str::to_string);
    StartChoice {
        model: text("model"),
        effort: text("effort"),
        mode: text("mode").filter(|mode| is_master_mode(mode)),
        uses_session_effort: kept["uses_session_effort"].as_bool().unwrap_or(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sakura_acp::model::Selector;

    fn selector(current: &str) -> Option<Selector> {
        Some(Selector {
            current: current.into(),
            ..Selector::default()
        })
    }

    #[test]
    fn picks_keep_what_the_agent_runs_on_and_never_one_of_its_own_modes() {
        let session = SessionChoices {
            models: selector("sonnet"),
            efforts: selector("low"),
            modes: selector("bypassPermissions"),
            ..SessionChoices::default()
        };
        let picks = picks_of(&session);
        assert_eq!(picks.model.as_deref(), Some("sonnet"));
        assert_eq!(picks.effort.as_deref(), Some("low"));
        assert_eq!(picks.mode, None);
        assert_eq!(from_json(&to_json(&picks)), picks);
    }

    #[test]
    fn a_session_that_has_said_nothing_keeps_nothing() {
        assert_eq!(picks_of(&SessionChoices::default()), StartChoice::default());
    }
}
