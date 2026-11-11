//! The accounts "New task" offers for the place chosen, and why a lavoro cannot be requested with the
//! one chosen. On the Mac only the logins that are there are offered. For the boxes every account is,
//! the ones not signed in saying so: a list is never empty while there is an account to name.

use crate::l10n::Key;
use sakura_core::models::{Account, Place};

/// One account the sheet offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    pub name: String,
    /// Whether an agent can start with it now.
    pub is_signed_in: bool,
}

/// No account of the boxes exists yet.
pub const NO_BOX_ACCOUNT: Key = Key("new_task.blocked.no_box_account");
/// There are accounts of the boxes and none is chosen.
pub const CHOOSE_ACCOUNT: Key = Key("new_task.blocked.choose_account");
/// The account chosen is not signed in.
pub const NOT_SIGNED_IN: Key = Key("new_task.blocked.not_signed_in");

/// The accounts of `place` among `accounts`, in the order `box` lists them.
pub fn offered(accounts: &[Account], place: Place) -> Vec<Offer> {
    let of_place = accounts.iter().filter(|account| account.place == place);
    let offers = of_place.map(|account| Offer {
        name: account.name.clone(),
        is_signed_in: account.is_usable(),
    });
    match place {
        Place::Box => offers.collect(),
        Place::Mac => offers.filter(|offer| offer.is_signed_in).collect(),
    }
}

/// Why a lavoro in `place` cannot be requested with the account `chosen` among `offers`; nothing when
/// it can. A lavoro on the Mac starts as the person: no account stops it.
pub fn blocker(offers: &[Offer], place: Place, chosen: Option<&str>) -> Option<Key> {
    if place == Place::Mac {
        return None;
    }
    if offers.is_empty() {
        return Some(NO_BOX_ACCOUNT);
    }
    let chosen = offers
        .iter()
        .find(|offer| Some(offer.name.as_str()) == chosen);
    match chosen {
        Some(offer) if offer.is_signed_in => None,
        Some(_) => Some(NOT_SIGNED_IN),
        None => Some(CHOOSE_ACCOUNT),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(name: &str, place: Place, is_signed_in: bool) -> Account {
        Account {
            name: name.to_string(),
            place,
            logged_in: is_signed_in,
            exists: (place == Place::Mac).then_some(is_signed_in),
        }
    }

    fn accounts() -> Vec<Account> {
        vec![
            account("personal", Place::Mac, true),
            account("assente", Place::Mac, false),
            account("scatola", Place::Box, false),
            account("seconda", Place::Box, true),
        ]
    }

    #[test]
    fn offered_every_account_of_the_boxes_is_listed_and_the_ones_not_signed_in_say_so() {
        let offers = offered(&accounts(), Place::Box);
        let listed: Vec<_> = offers
            .iter()
            .map(|offer| (offer.name.as_str(), offer.is_signed_in))
            .collect();
        assert_eq!(listed, [("scatola", false), ("seconda", true)]);
    }

    #[test]
    fn offered_on_the_mac_only_the_logins_that_are_there() {
        let offers = offered(&accounts(), Place::Mac);
        assert_eq!(offers.len(), 1);
        assert_eq!(offers[0].name, "personal");
    }

    #[test]
    fn blocker_a_box_needs_an_account_that_is_signed_in_and_says_which_step_is_missing() {
        let offers = offered(&accounts(), Place::Box);
        assert_eq!(blocker(&[], Place::Box, None), Some(NO_BOX_ACCOUNT));
        assert_eq!(blocker(&offers, Place::Box, None), Some(CHOOSE_ACCOUNT));
        let not_signed_in = blocker(&offers, Place::Box, Some("scatola"));
        assert_eq!(not_signed_in, Some(NOT_SIGNED_IN));
        assert_eq!(blocker(&offers, Place::Box, Some("seconda")), None);
    }

    #[test]
    fn blocker_nothing_stops_a_lavoro_on_the_mac() {
        assert_eq!(blocker(&[], Place::Mac, None), None);
    }
}
