//! Where a new task lives before the person chooses: the place chosen last for its project, else in a
//! box when a box can be used (Docker answers and an account of the boxes is signed in), else on the
//! Mac. Under the choice of the Mac one plain line says what it means.

use crate::l10n::Key;
use sakura_core::models::{Account, Place};

/// What working on the Mac means, said under the choice.
pub const ON_THE_MAC: Key = Key("new_task.place.mac_means");

/// The place a new task of a project is preset to: `remembered`, the one chosen last for it, wins;
/// else a box when `docker_answers` and one of `accounts` of the boxes is signed in; else the Mac.
pub fn preset(remembered: Option<Place>, docker_answers: bool, accounts: &[Account]) -> Place {
    let has_box_account = accounts
        .iter()
        .any(|account| account.place == Place::Box && account.is_usable());
    let default = match docker_answers && has_box_account {
        true => Place::Box,
        false => Place::Mac,
    };
    remembered.unwrap_or(default)
}

/// The place kept for a project as the preferences write it.
pub fn remembered(word: Option<&str>) -> Option<Place> {
    match word {
        Some("box") => Some(Place::Box),
        Some("mac") => Some(Place::Mac),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(place: Place, logged_in: bool) -> Account {
        Account {
            name: "a".into(),
            place,
            logged_in,
            exists: None,
        }
    }

    #[test]
    fn a_box_is_the_default_when_docker_answers_and_an_account_of_the_boxes_is_signed_in() {
        let signed_in = [account(Place::Box, true)];
        assert_eq!(preset(None, true, &signed_in), Place::Box);
    }

    #[test]
    fn the_mac_is_the_default_when_no_box_can_be_used() {
        let signed_in = [account(Place::Box, true)];
        assert_eq!(preset(None, false, &signed_in), Place::Mac, "no Docker");
        let not_signed_in = [account(Place::Box, false), account(Place::Mac, true)];
        assert_eq!(preset(None, true, &not_signed_in), Place::Mac, "no account");
    }

    #[test]
    fn the_place_chosen_last_for_the_project_wins_over_the_default() {
        let signed_in = [account(Place::Box, true)];
        assert_eq!(preset(Some(Place::Mac), true, &signed_in), Place::Mac);
        assert_eq!(preset(Some(Place::Box), false, &[]), Place::Box);
        assert_eq!(remembered(Some("box")), Some(Place::Box));
        assert_eq!(remembered(Some("altrove")), None);
    }
}
