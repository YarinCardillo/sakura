//! Who the engine's ledger names for what the app asks of it: `app:<login>`, the person at the Sakura window.
//! The login is written as the engine writes it for a command line, so the same person is the same name.

use nix::unistd::{getuid, User};

/// The source the engine gives the person at the Sakura window.
const APP_SOURCE: &str = "app";
/// The longest login the engine accepts.
const MAX_LOGIN_CHARS: usize = 64;
/// What stands for a login nobody could read.
const UNKNOWN_LOGIN: &str = "unknown";
/// The variables that name the person, in the order the engine reads them.
const LOGIN_VARIABLES: [&str; 4] = ["LOGNAME", "USER", "LNAME", "USERNAME"];

/// The actor the app passes to the commands that take one: `app:<login>`.
pub fn app_actor() -> String {
    let login = LOGIN_VARIABLES
        .iter()
        .find_map(|name| std::env::var(name).ok().filter(|value| !value.is_empty()))
        .or_else(account_login)
        .unwrap_or_default();
    actor_of(&login)
}

/// `app:<login>` with `login` cleaned as the engine cleans one: a character it does not accept becomes `-`, the
/// login is cut to its limit, and none at all is `unknown`.
pub fn actor_of(login: &str) -> String {
    let cleaned: String = login
        .chars()
        .map(
            |letter| match letter.is_ascii_alphanumeric() || "._@+-".contains(letter) {
                true => letter,
                false => '-',
            },
        )
        .take(MAX_LOGIN_CHARS)
        .collect();
    let login = if cleaned.is_empty() {
        UNKNOWN_LOGIN
    } else {
        &cleaned
    };
    format!("{APP_SOURCE}:{login}")
}

/// The login of the account the process runs as, from the system's account database.
fn account_login() -> Option<String> {
    User::from_uid(getuid())
        .ok()
        .flatten()
        .map(|user| user.name)
}

#[cfg(test)]
mod tests {
    use super::{actor_of, app_actor};

    #[test]
    fn the_actor_is_the_app_source_and_the_login() {
        assert_eq!(actor_of("alice"), "app:alice");
    }

    #[test]
    fn a_character_the_engine_does_not_accept_becomes_a_dash_and_a_long_login_is_cut() {
        assert_eq!(actor_of("a b/c"), "app:a-b-c");
        assert_eq!(
            actor_of(&"x".repeat(100)),
            format!("app:{}", "x".repeat(64))
        );
    }

    #[test]
    fn no_login_is_unknown() {
        assert_eq!(actor_of(""), "app:unknown");
    }

    #[test]
    fn the_actor_of_this_process_names_an_app_login() {
        let actor = app_actor();
        assert!(actor.starts_with("app:") && actor.len() > 4, "{actor}");
        assert!(!actor.contains(' '), "{actor}");
    }
}
