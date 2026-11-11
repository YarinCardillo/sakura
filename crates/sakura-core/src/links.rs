//! Which addresses printed in a terminal may be opened in the browser. What a terminal prints is not
//! trusted: an address is parsed as a URL, never matched as text. The sign-in address of a login is opened by
//! the app on its own, so it must be `https` on a host that is exactly one of Claude's sign-in hosts, with no
//! user information and no port. An address the person Cmd+clicks may be any `http` or `https` one; no other
//! scheme is ever opened.

use std::ops::Range;
use url::Url;

/// The hosts a sign-in address may be on. Exact names: a subdomain or a longer name is another host.
pub const SIGN_IN_HOSTS: [&str; 4] = [
    "claude.com",
    "claude.ai",
    "platform.claude.com",
    "console.anthropic.com",
];

/// Characters that surround an address in a sentence without being part of it.
const WRAPPING: [char; 9] = ['<', '>', '"', '\'', '(', ')', '[', ']', ','];

/// The sign-in address in `text` (a line of terminal output), when one of its words is one.
pub fn sign_in_url(text: &str) -> Option<String> {
    words(text).find_map(|word| {
        let url = Url::parse(word.text).ok()?;
        is_sign_in(&url).then(|| url.to_string())
    })
}

/// The address `word` is, when it is an `http` or `https` one.
pub fn web_url(word: &str) -> Option<String> {
    let url = Url::parse(word).ok()?;
    let is_web = matches!(url.scheme(), "http" | "https") && url.has_host();
    is_web.then(|| url.to_string())
}

/// The host of `word`, when it is an `http` or `https` address.
pub fn web_host(word: &str) -> Option<String> {
    let url = Url::parse(word).ok()?;
    let is_web = matches!(url.scheme(), "http" | "https");
    url.host_str().filter(|_| is_web).map(str::to_string)
}

/// The `http` or `https` address under column `column` of `line` (columns are characters), with the
/// columns it spans.
pub fn web_url_at(line: &str, column: usize) -> Option<(Range<usize>, String)> {
    let word = words(line).find(|word| word.columns.contains(&column))?;
    Some((word.columns.clone(), web_url(word.text)?))
}

/// `https`, on exactly one of the sign-in hosts, with no user information and no port.
fn is_sign_in(url: &Url) -> bool {
    let is_plain = url.username().is_empty() && url.password().is_none() && url.port().is_none();
    let is_host = url
        .host_str()
        .is_some_and(|host| SIGN_IN_HOSTS.contains(&host));
    url.scheme() == "https" && is_plain && is_host
}

/// A word of a line: its text without what wraps it, and the columns that text spans.
struct Word<'a> {
    text: &'a str,
    columns: Range<usize>,
}

/// The words of `line`, split at white space, each without the brackets, quotes and sentence punctuation
/// around it.
fn words(line: &str) -> impl Iterator<Item = Word<'_>> {
    let mut column = 0;
    line.split_inclusive(char::is_whitespace)
        .filter_map(move |piece| {
            let start = column;
            column += piece.chars().count();
            let leading = piece.trim_start_matches(WRAPPING);
            let text = leading
                .trim_end()
                .trim_end_matches(WRAPPING)
                .trim_end_matches(['.', ';']);
            let first = start + piece.chars().count() - leading.chars().count();
            (!text.is_empty()).then(|| Word {
                text,
                columns: first..first + text.chars().count(),
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const REAL: &str = "https://claude.com/cai/oauth/authorize?code=true&client_id=9d1c250a-e61b-44d9-88ed-5944d1962f5e&response_type=code&redirect_uri=https%3A%2F%2Fplatform.claude.com%2Foauth%2Fcode%2Fcallback&scope=user%3Ainference&state=abc";

    #[test]
    fn the_line_a_login_prints_gives_exactly_its_address() {
        let line = format!("If the browser didn't open, visit: {REAL}");
        assert_eq!(sign_in_url(&line).as_deref(), Some(REAL));
    }

    #[test]
    fn every_sign_in_host_is_accepted_and_only_over_https() {
        for host in SIGN_IN_HOSTS {
            let address = format!("https://{host}/oauth/authorize?x=1");
            assert_eq!(sign_in_url(&address).as_deref(), Some(address.as_str()));
            assert_eq!(sign_in_url(&format!("http://{host}/oauth/authorize")), None);
        }
    }

    #[test]
    fn a_host_that_only_looks_like_one_is_refused() {
        let refused = [
            "https://claude.com.evil.test/cai/oauth/authorize",
            "https://evil.test/claude.com",
            "https://evil.test/?next=https://claude.com/",
            "https://notclaude.com/oauth",
            "https://login.claude.com/oauth",
            "https://claude.com./oauth",
            "https://xn--claude-9db.com/oauth",
        ];
        for address in refused {
            assert_eq!(sign_in_url(address), None, "{address}");
        }
    }

    #[test]
    fn user_information_before_the_host_and_a_port_are_refused() {
        let refused = [
            "https://claude.com@evil.test/oauth",
            "https://user:pass@claude.com/oauth",
            "https://user@claude.com/oauth",
            "https://claude.com:8443/oauth",
        ];
        for address in refused {
            assert_eq!(sign_in_url(address), None, "{address}");
        }
    }

    #[test]
    fn other_schemes_are_never_a_sign_in_address() {
        let refused = [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "ftp://claude.com/x",
            "claude.com/oauth",
            "data:text/html,claude.com",
        ];
        for address in refused {
            assert_eq!(sign_in_url(address), None, "{address}");
            assert_eq!(
                sign_in_url(&format!("visit: {address} now")),
                None,
                "{address}"
            );
        }
    }

    #[test]
    fn the_first_sign_in_address_of_a_line_is_the_one() {
        let line =
            "open https://evil.test/a or https://claude.ai/login?x=1 or https://claude.com/b";
        assert_eq!(
            sign_in_url(line).as_deref(),
            Some("https://claude.ai/login?x=1")
        );
        assert_eq!(sign_in_url("no address here"), None);
    }

    #[test]
    fn an_address_in_brackets_or_before_a_comma_is_read_without_them() {
        assert_eq!(
            sign_in_url("visit <https://claude.com/x>, then paste").as_deref(),
            Some("https://claude.com/x")
        );
    }

    #[test]
    fn a_clicked_word_opens_only_as_http_or_https() {
        assert_eq!(
            web_url("https://example.com/docs?a=1").as_deref(),
            Some("https://example.com/docs?a=1")
        );
        assert_eq!(
            web_url("http://localhost:3000/").as_deref(),
            Some("http://localhost:3000/")
        );
        for refused in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "example.com",
            "ssh://host",
            "",
        ] {
            assert_eq!(web_url(refused), None, "{refused}");
        }
    }

    #[test]
    fn the_address_under_a_column_comes_with_the_columns_it_spans() {
        let line = "see (https://example.com/a) and more";
        let (columns, url) = web_url_at(line, 10).expect("an address");
        assert_eq!(url, "https://example.com/a");
        assert_eq!(columns, 5..26);
        assert_eq!(web_url_at(line, 1), None, "a plain word");
        assert_eq!(web_url_at(line, 4), None, "the bracket before it");
        assert_eq!(web_url_at(line, 200), None, "past the end");
        assert_eq!(
            web_url_at("cafés https://example.com/è", 8)
                .map(|(_, url)| url)
                .as_deref(),
            Some("https://example.com/%C3%A8")
        );
    }

    #[test]
    fn the_host_of_a_web_address_is_read_and_no_other_kind_of_address_has_one() {
        assert_eq!(
            web_host("https://crates.io/crates/serde").as_deref(),
            Some("crates.io")
        );
        assert_eq!(
            web_host("http://localhost:3000/x").as_deref(),
            Some("localhost")
        );
        assert_eq!(web_host("file:///etc/passwd"), None);
        assert_eq!(web_host("javascript:alert(1)"), None);
        assert_eq!(web_host("crates.io"), None);
    }
}
