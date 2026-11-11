//! What a login says in plain words: what is happening while `box login` runs, and why it failed when it
//! did. `box` ends a failed login with one line that starts with `box:`; the known ones are told by
//! the one mapping of `box`'s lines, any other as a general sentence, and what `box` printed stays in the
//! terminal under the details. Never only an exit code, never a raw line of `box`.

use crate::l10n::{tr, Key, Said};
use sakura_core::box_words::{self, Known};

/// Where a login is, as the pane's header tells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginPhase {
    /// The session is being opened: `box` starts the container.
    Opening,
    /// `box login` runs: the sign-in happens in the terminal.
    SigningIn,
    /// The login printed its sign-in address and the app opened it in the browser.
    InBrowser,
    /// `box login` ended well: the account is being read again.
    Checking,
}

impl LoginPhase {
    /// The sentence of the phase.
    pub fn sentence(self) -> &'static str {
        match self {
            LoginPhase::Opening => tr(Key("login.opening")),
            LoginPhase::SigningIn => tr(Key("login.in_terminal")),
            LoginPhase::InBrowser => tr(Key("login.in_browser")),
            LoginPhase::Checking => tr(Key("login.checking")),
        }
    }
}

/// Why a login failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginFailure {
    /// The sign-in service could not be reached.
    NoConnection,
    /// The sign-in was left before its end.
    NotCompleted,
    /// The sign-in ended, but the account does not read as logged in.
    NotLoggedIn,
    /// The state of the account could not be read after the sign-in.
    StatusUnreadable,
    /// The person interrupted it.
    Cancelled,
    /// Anything else, as what `box`'s mapping or the app has to say of it.
    Other(Said),
}

impl LoginFailure {
    /// What to say to the person. The failures `box` names are keyed by the one mapping of its lines
    /// (`sakura_core::box_words`).
    pub fn said(&self) -> Said {
        let key = match self {
            LoginFailure::NoConnection => Known::NoConnection.key(),
            LoginFailure::NotCompleted => Known::SignInNotCompleted.key(),
            LoginFailure::NotLoggedIn => Known::StillNotLoggedIn.key(),
            LoginFailure::StatusUnreadable => Known::LoginStatusUnreadable.key(),
            LoginFailure::Cancelled => Key("login.cancelled"),
            LoginFailure::Other(said) => return said.clone(),
        };
        key.into()
    }
}

/// The exit code of a process ended by an interrupt (Ctrl-C), alone or inside `box`'s sentence.
const INTERRUPTED_EXIT_CODE: i32 = 130;
/// What a failure says when `box` gave no reason, or one Sakura does not know: what `box` printed stays in
/// the terminal under the details.
pub const NO_REASON: Key = Key("login.failed");

/// Why a login that exited with `code` failed, from the last line it printed.
pub fn login_failure(code: Option<i32>, last_line: Option<&str>) -> LoginFailure {
    let reason = last_line.and_then(box_words::reason);
    let interrupted = format!("(exit {INTERRUPTED_EXIT_CODE})");
    match reason.map(|reason| (reason, box_words::known(reason))) {
        Some((_, Some(Known::NoConnection))) => LoginFailure::NoConnection,
        Some((_, Some(Known::SignInNotCompleted))) => LoginFailure::NotCompleted,
        Some((_, Some(Known::StillNotLoggedIn))) => LoginFailure::NotLoggedIn,
        Some((_, Some(Known::LoginStatusUnreadable))) => LoginFailure::StatusUnreadable,
        Some((reason, _)) if reason.contains(&interrupted) => LoginFailure::Cancelled,
        Some((_, Some(known))) => LoginFailure::Other(known.key().into()),
        Some((_, None)) => LoginFailure::Other(NO_REASON.into()),
        None if code.is_none_or(|code| code == INTERRUPTED_EXIT_CODE) => LoginFailure::Cancelled,
        None => LoginFailure::Other(NO_REASON.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failure(code: i32, line: &str) -> LoginFailure {
        login_failure(Some(code), Some(line))
    }

    #[test]
    fn the_failures_box_names_are_told_apart() {
        assert_eq!(
            failure(
                1,
                "box: no connection to the sign-in service; is the proxy running (box up)?"
            ),
            LoginFailure::NoConnection
        );
        assert_eq!(
            failure(1, "box: the sign-in was not completed for account 'personal'; run box login personal again"),
            LoginFailure::NotCompleted
        );
        assert_eq!(
            failure(1, "box: the sign-in finished but account 'personal' is still not logged in; run box login personal again"),
            LoginFailure::NotLoggedIn
        );
        assert_eq!(
            failure(
                1,
                "box: could not read the login status of account 'personal' after signing in"
            ),
            LoginFailure::StatusUnreadable
        );
    }

    #[test]
    fn an_interrupted_login_is_cancelled() {
        let line =
            "box: the login container failed (exit 130); account 'personal' is not logged in";
        assert_eq!(failure(1, line), LoginFailure::Cancelled);
        assert_eq!(login_failure(Some(130), None), LoginFailure::Cancelled);
        assert_eq!(
            login_failure(None, None),
            LoginFailure::Cancelled,
            "ended by a signal"
        );
    }

    #[test]
    fn any_other_line_from_box_is_said_in_sakura_s_words_and_never_shown_as_the_sentence() {
        assert_eq!(
            failure(1, "box: fd 7: Bad file descriptor"),
            LoginFailure::Other(NO_REASON.into()),
            "a line Sakura does not know: the general sentence, the line stays in the terminal"
        );
        assert_eq!(
            failure(
                1,
                "box: account 'personal' has no login yet (box login personal)"
            ),
            LoginFailure::Other(Known::AccountNotLoggedIn.key().into()),
            "a line Sakura knows from elsewhere: its own sentence"
        );
    }

    #[test]
    fn a_failure_without_a_line_from_box_still_says_something_true() {
        let silent = login_failure(Some(1), Some("Error: something printed by the program"));
        assert_eq!(silent, LoginFailure::Other(NO_REASON.into()));
        assert_eq!(login_failure(Some(7), None), silent);
    }

    #[test]
    fn every_failure_is_one_plain_sentence_and_never_an_exit_code() {
        let failures = [
            LoginFailure::NoConnection,
            LoginFailure::NotCompleted,
            LoginFailure::NotLoggedIn,
            LoginFailure::StatusUnreadable,
            LoginFailure::Cancelled,
            login_failure(Some(1), None),
        ];
        let sentence = |failure: &LoginFailure| crate::l10n::say(&failure.said());
        for language in [
            crate::l10n::Language::English,
            crate::l10n::Language::Italian,
        ] {
            crate::l10n::set_for_test(language);
            for failure in &failures {
                let sentence = sentence(failure);
                assert!(sentence.ends_with('.'), "{sentence:?}");
                assert!(!sentence.contains("codice"), "{sentence:?}");
                assert!(!sentence.contains("code"), "{sentence:?}");
            }
        }
        assert_eq!(
            sentence(&LoginFailure::NoConnection),
            "Nessuna connessione ad Anthropic. Controlla la rete e riprova."
        );
        assert_eq!(
            sentence(&LoginFailure::NotCompleted),
            "L'accesso non è stato completato."
        );
        assert_eq!(sentence(&LoginFailure::Cancelled), "Accesso annullato.");
    }

    #[test]
    fn every_phase_has_its_sentence() {
        crate::l10n::set_for_test(crate::l10n::Language::Italian);
        assert_eq!(LoginPhase::Opening.sentence(), "Apro la box per l'accesso.");
        assert_eq!(
            LoginPhase::SigningIn.sentence(),
            "Completa l'accesso nel terminale qui sotto."
        );
        assert_eq!(LoginPhase::Checking.sentence(), "Verifico l'accesso.");
        assert_eq!(
            LoginPhase::InBrowser.sentence(),
            "Continua nel browser, poi incolla qui il codice"
        );
    }
}
