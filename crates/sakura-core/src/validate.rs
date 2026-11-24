//! Argument validation. Every value that reaches a `box` argument vector passes through here first, so a
//! malformed value is rejected before any process starts.

use crate::error::{BridgeError, BridgeResult};
use crate::said::{Key, Said};

const SLUG_MAX_LEN: usize = 40;
const TASK_ID_MAX_LEN: usize = 48;
const VERB_MAX_LEN: usize = 31;
const SHA_MIN_LEN: usize = 7;
const SHA_MAX_LEN: usize = 40;
const SINCE_MAX_LEN: usize = 40;
const TERMINAL_MAX_DIMENSION: u16 = 1000;
const PATH_MAX_LEN: usize = 4096;
const BRANCH_MAX_LEN: usize = 255;
/// The longest value `box` takes for a field of a project.
const TEXT_MAX_LEN: usize = 2000;

/// Task id follows the engine's `flow.store.TASK_ID` pattern and cannot be read as a flag.
pub fn task(value: &str) -> BridgeResult<&str> {
    let is_alphanumeric = |byte: u8| byte.is_ascii_lowercase() || byte.is_ascii_digit();
    let mut bytes = value.bytes();
    if value.len() <= TASK_ID_MAX_LEN
        && bytes.next().is_some_and(is_alphanumeric)
        && bytes.all(|byte| is_alphanumeric(byte) || byte == b'-')
    {
        Ok(value)
    } else {
        Err(invalid_name(
            Key("validate.label.task"),
            value,
            Key("validate.task_id_rule"),
        ))
    }
}

/// Account name for the `box` interface.
pub fn account(value: &str) -> BridgeResult<&str> {
    slug(Key("validate.label.account"), value)
}

/// Project name for the `box` interface.
pub fn project(value: &str) -> BridgeResult<&str> {
    slug(Key("validate.label.project"), value)
}

/// A folder to hand to `box`: an absolute path with no control character, so it is never read as an
/// option and never breaks a line.
pub fn folder(value: &str) -> BridgeResult<&str> {
    let is_plain = !value.chars().any(char::is_control);
    if value.starts_with('/') && is_plain && value.len() <= PATH_MAX_LEN {
        Ok(value)
    } else {
        Err(BridgeError::invalid(
            Said::new("error.invalid.folder").text("value", value),
        ))
    }
}

/// A branch name to hand to `box`: never empty, never read as an option, no space or control character,
/// no `..` (git refuses the rest itself).
pub fn branch(value: &str) -> BridgeResult<&str> {
    let has_bad_character = value.chars().any(|c| c.is_control() || c.is_whitespace());
    let is_name = !value.is_empty() && !value.starts_with('-') && !value.contains("..");
    if is_name && !has_bad_character && value.len() <= BRANCH_MAX_LEN {
        Ok(value)
    } else {
        Err(BridgeError::invalid(
            Said::new("error.invalid.branch").text("value", value),
        ))
    }
}

/// A command or a list of patterns of a project, as text on one line: `box` refuses longer values and
/// control characters.
pub fn text(value: &str) -> BridgeResult<&str> {
    let is_plain = !value.chars().any(char::is_control);
    if is_plain && value.len() <= TEXT_MAX_LEN {
        Ok(value)
    } else {
        Err(BridgeError::invalid(
            Said::new("error.invalid.text").text("value", value),
        ))
    }
}

/// Verb: `^[a-z][a-z0-9-]{0,30}$`.
pub fn verb(value: &str) -> BridgeResult<&str> {
    let mut chars = value.chars();
    let starts_with_letter = chars.next().is_some_and(|c| c.is_ascii_lowercase());
    let rest_ok = chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if starts_with_letter && rest_ok && value.len() <= VERB_MAX_LEN {
        Ok(value)
    } else {
        Err(BridgeError::invalid(
            Said::new("error.invalid.verb").text("value", value),
        ))
    }
}

/// Commit id: `^[0-9a-f]{7,40}$`.
pub fn sha(value: &str) -> BridgeResult<&str> {
    let is_hex = value
        .bytes()
        .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    if is_hex && (SHA_MIN_LEN..=SHA_MAX_LEN).contains(&value.len()) {
        Ok(value)
    } else {
        Err(BridgeError::invalid(
            Said::new("error.invalid.commit").text("value", value),
        ))
    }
}

/// Journal cut-off: an ISO 8601 style timestamp, made only of digits and `-:.TZ+`.
pub fn since(value: &str) -> BridgeResult<&str> {
    let allowed = |c: char| c.is_ascii_digit() || matches!(c, '-' | ':' | '.' | 'T' | 'Z' | '+');
    let starts_with_digit = value.starts_with(|c: char| c.is_ascii_digit());
    if starts_with_digit && value.len() <= SINCE_MAX_LEN && value.chars().all(allowed) {
        Ok(value)
    } else {
        Err(BridgeError::invalid(
            Said::new("error.invalid.date").text("value", value),
        ))
    }
}

/// Terminal size in cells: both dimensions between 1 and 1000.
pub fn terminal_size(cols: u16, rows: u16) -> BridgeResult<(u16, u16)> {
    let in_range = |value: u16| (1..=TERMINAL_MAX_DIMENSION).contains(&value);
    if in_range(cols) && in_range(rows) {
        Ok((cols, rows))
    } else {
        let said = Said::new("error.terminal.size")
            .number("cols", u64::from(cols))
            .number("rows", u64::from(rows));
        Err(BridgeError::invalid(said))
    }
}

/// The rule for `box` account and project names.
pub const SLUG_RULE: Key = Key("validate.slug_rule");

/// `box`'s name pattern `^[a-z0-9]+(-[a-z0-9]+)*$`, at most 40 characters.
fn is_slug(value: &str) -> bool {
    let is_part = |part: &str| {
        !part.is_empty()
            && part
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    };
    (1..=SLUG_MAX_LEN).contains(&value.len()) && value.split('-').all(is_part)
}

fn slug(label: Key, value: &str) -> BridgeResult<&str> {
    if is_slug(value) {
        Ok(value)
    } else {
        Err(invalid_name(label, value, SLUG_RULE))
    }
}

fn invalid_name(label: Key, value: &str, rule: Key) -> BridgeError {
    BridgeError::invalid(
        Said::new("error.invalid.name")
            .said("label", label.into())
            .text("value", value)
            .said("rule", rule.into()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorCode;

    #[test]
    fn a_folder_is_an_absolute_path_on_one_line() {
        assert!(folder("/home/me/code/shop").is_ok());
        assert!(folder("/home/me/a folder/àè").is_ok());
        for bad in ["", "code/shop", "--repo", "~/code", "/a\nb", "/a\u{0}b"] {
            assert_eq!(
                folder(bad).unwrap_err().code,
                ErrorCode::InvalidArgument,
                "{bad:?}"
            );
        }
    }

    #[test]
    fn a_branch_is_never_an_option_and_has_no_space() {
        for good in ["main", "feature/cart", "sakura/fix-login", "v1.2"] {
            assert!(branch(good).is_ok(), "{good}");
        }
        for bad in ["", "-b", "--force", "a b", "a..b", "a\tb", "a\nb"] {
            assert_eq!(
                branch(bad).unwrap_err().code,
                ErrorCode::InvalidArgument,
                "{bad:?}"
            );
        }
    }

    #[test]
    fn a_project_s_text_is_one_line_of_at_most_two_thousand_characters() {
        assert!(text("npm run test -- --watch=false").is_ok());
        assert!(text("").is_ok());
        assert!(text("a\nb").is_err());
        assert!(text(&"x".repeat(2001)).is_err());
    }

    #[test]
    fn task_matches_the_engine_alphabet_and_length() {
        for value in ["sample-accent", "m1", "0", "a--b", "x-", &"a".repeat(48)] {
            assert!(task(value).is_ok(), "{value:?}");
        }
        for validate in [account, project] {
            for value in ["a--b", "x-", &"a".repeat(41)] {
                assert!(validate(value).is_err(), "{value:?}");
            }
        }
    }

    #[test]
    fn task_rejects_everything_else() {
        let bad = [
            "",
            "-x",
            "A",
            "a b",
            "a/b",
            "a..b",
            "a_b",
            "a;ls",
            "a\n",
            "$(id)",
            "è",
            "..",
            &"a".repeat(49),
        ];
        for value in bad {
            let error = task(value).unwrap_err();
            assert_eq!(error.code, ErrorCode::InvalidArgument, "{value:?}");
        }
    }

    #[test]
    fn verb_must_start_with_a_letter() {
        assert!(verb("test").is_ok());
        assert!(verb("e2e-smoke").is_ok());
        assert!(verb("").is_err());
        assert!(verb("1test").is_err());
        assert!(verb("-test").is_err());
        assert!(verb("Test").is_err());
        assert!(verb("a b").is_err());
        assert!(verb(&"a".repeat(32)).is_err());
        assert!(verb(&"a".repeat(31)).is_ok());
    }

    #[test]
    fn sha_is_lowercase_hex_between_7_and_40() {
        assert!(sha("abc1234").is_ok());
        assert!(sha(&"f".repeat(40)).is_ok());
        assert!(sha("abc123").is_err());
        assert!(sha(&"f".repeat(41)).is_err());
        assert!(sha("ABC1234").is_err());
        assert!(sha("abc123g").is_err());
        assert!(sha("--force1").is_err());
    }

    #[test]
    fn since_accepts_timestamps_only() {
        assert!(since("2026-11-10T06:41:12Z").is_ok());
        assert!(since("2026-11-10T06:41:12.123+01:00").is_ok());
        assert!(since("-1").is_err());
        assert!(since("2026 11").is_err());
        assert!(since("2026;rm").is_err());
        assert!(since("").is_err());
    }

    #[test]
    fn terminal_size_must_be_positive_and_bounded() {
        assert!(terminal_size(80, 24).is_ok());
        assert!(terminal_size(0, 24).is_err());
        assert!(terminal_size(80, 0).is_err());
        assert!(terminal_size(1001, 24).is_err());
    }
}
