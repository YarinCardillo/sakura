use super::model::UpdateState;
use super::CURRENT_VERSION;
use crate::l10n::{tr, tr_with, Key};

/// The quiet Settings sentence for a check, in the current interface language.
pub fn sentence(state: &UpdateState) -> String {
    let key = match state {
        UpdateState::NotSetUp => "updates.not_set_up",
        UpdateState::Ready => "updates.ready",
        UpdateState::Checking => "updates.checking",
        UpdateState::UpToDate => {
            return tr_with(Key("updates.up_to_date"), &[("version", CURRENT_VERSION)]).to_string()
        }
        UpdateState::Failed => "updates.failed",
        UpdateState::Available(release) => {
            return tr_with(Key("updates.available"), &[("version", &release.version)]).to_string()
        }
    };
    tr(Key(key)).to_string()
}
