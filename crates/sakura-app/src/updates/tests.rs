use super::github::Repository;
use super::model::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

struct FakeChecker {
    calls: AtomicUsize,
    result: Result<Release, CheckError>,
}

impl Checker for FakeChecker {
    fn check(&self) -> Result<Release, CheckError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.result.clone()
    }
}

fn release(version: &str) -> Release {
    Release {
        version: version.into(),
        url: "https://github.com/example/app/releases/tag/v1.2.0".into(),
    }
}

fn run(machine: &mut UpdateMachine, checker: &dyn Checker) {
    if machine.begin(Instant::now()) {
        machine.finish("1.0.0", checker.check());
    }
}

#[test]
fn versions_use_semantic_precedence_including_prereleases_and_build_metadata() {
    for (current, candidate) in [
        ("1.9.0", "v1.10.0"),
        ("1.0.0-rc.1", "1.0.0"),
        ("1.0.0-beta.2", "1.0.0-beta.11"),
        ("1.0.0-alpha", "1.0.0-beta"),
    ] {
        assert_eq!(
            is_newer(current, candidate),
            Ok(true),
            "{current} -> {candidate}"
        );
    }
    for (current, candidate) in [
        ("1.0.0", "1.0.0"),
        ("2.0.0", "1.9.0"),
        ("1.0.0", "1.0.0-rc.2"),
        ("1.0.0+one", "1.0.0+two"),
        ("1.0.0-rc.1", "v1.0.0-rc.1"),
    ] {
        assert_eq!(is_newer(current, candidate), Ok(false));
    }
    for invalid in ["latest", "1.0", "01.0.0", "1.0.0\n", ""] {
        assert_eq!(is_newer("1.0.0", invalid), Err(CheckError));
        assert_eq!(is_newer(invalid, "1.0.0"), Err(CheckError));
    }
}

#[test]
fn missing_configuration_never_calls_the_checker() {
    let fake = FakeChecker {
        calls: AtomicUsize::new(0),
        result: Ok(release("1.2.0")),
    };
    let mut machine = UpdateMachine::new(false);
    assert_eq!(machine.state(), &UpdateState::NotSetUp);
    assert!(!machine.can_check());
    run(&mut machine, &fake);
    assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
    assert!(!machine.is_due(true, Instant::now()));
}

#[test]
fn checks_cannot_overlap_and_a_newer_release_is_retained() {
    let mut machine = UpdateMachine::new(true);
    assert_eq!(machine.state(), &UpdateState::Ready);
    assert!(machine.can_check());
    assert!(machine.begin(Instant::now()));
    assert_eq!(machine.state(), &UpdateState::Checking);
    assert!(!machine.can_check());
    assert!(!machine.begin(Instant::now()));
    machine.finish("1.0.0", Ok(release("1.2.0")));
    assert_eq!(machine.state(), &UpdateState::Available(release("1.2.0")));
    assert!(machine.can_check());
}

#[test]
fn equal_older_and_failed_checks_are_retryable_through_the_injected_checker() {
    let mut machine = UpdateMachine::new(true);
    for (result, state) in [
        (Ok(release("1.0.0")), UpdateState::UpToDate),
        (Ok(release("0.9.0")), UpdateState::UpToDate),
        (Err(CheckError), UpdateState::Failed),
        (Ok(release("invalid")), UpdateState::Failed),
        (
            Ok(release("1.2.0")),
            UpdateState::Available(release("1.2.0")),
        ),
    ] {
        let fake = FakeChecker {
            calls: AtomicUsize::new(0),
            result,
        };
        run(&mut machine, &fake);
        assert_eq!(machine.state(), &state);
        assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
    }
}

#[test]
fn automatic_checks_run_at_launch_and_daily_only_while_enabled() {
    let mut machine = UpdateMachine::new(true);
    let now = Instant::now();
    assert!(machine.is_due(true, now));
    assert!(!machine.is_due(false, now));
    assert!(machine.begin(now));
    assert!(!machine.is_due(true, now + CHECK_INTERVAL));
    machine.finish("1.0.0", Err(CheckError));
    assert!(!machine.is_due(true, now + CHECK_INTERVAL / 2));
    assert!(machine.is_due(true, now + CHECK_INTERVAL));
    assert!(!machine.is_due(false, now + CHECK_INTERVAL));
    assert!(
        machine.begin(now),
        "manual checks ignore automatic preference and interval"
    );
}

#[test]
fn repository_metadata_is_a_public_github_repository_without_credentials() {
    let repo = Repository::parse("https://github.com/example/app.git").unwrap();
    assert_eq!(
        repo.api_url(),
        "https://api.github.com/repos/example/app/releases/latest"
    );
    for invalid in [
        "",
        "http://github.com/a/b",
        "https://example.com/a/b",
        "https://u:p@github.com/a/b",
        "https://github.com/a/b?q=1",
        "https://github.com/a/b#fragment",
        "https://github.com/a/b/extra",
    ] {
        assert!(Repository::parse(invalid).is_none(), "{invalid}");
    }
    assert!(Repository::parse(env!("CARGO_PKG_REPOSITORY")).is_some());
}

#[test]
fn release_response_accepts_encoded_version_tags_and_canonical_repository_case() {
    let repo = Repository::parse("https://github.com/example/app").unwrap();
    let url = "https://github.com/Example/App/releases/tag/v1.2.0%2Bbuild.1";
    let body = serde_json::json!({"tag_name":"v1.2.0+build.1", "html_url":url,
        "draft":false,"prerelease":false})
    .to_string();
    assert_eq!(
        repo.release(&body),
        Ok(Release {
            version: "1.2.0+build.1".into(),
            url: url.into()
        })
    );
}

#[test]
fn release_response_rejects_drafts_prereleases_bad_versions_and_foreign_links() {
    let repo = Repository::parse("https://github.com/example/app").unwrap();
    let body = |version: &str, url: &str, draft: bool, prerelease: bool| {
        serde_json::json!({
            "tag_name": version, "html_url": url, "draft": draft, "prerelease": prerelease,
        })
        .to_string()
    };
    let url = "https://github.com/example/app/releases/tag/v1.2.0";
    assert_eq!(
        repo.release(&body("v1.2.0", url, false, false)),
        Ok(release("1.2.0"))
    );
    for invalid in [
        body("latest", url, false, false),
        body("1.2.0", url, true, false),
        body("1.2.0-rc.1", url, false, true),
        body("1.2.0", "https://evil.example/app", false, false),
        body(
            "1.2.0",
            "https://github.com/other/app/releases/tag/v1.2.0",
            false,
            false,
        ),
        "{}".into(),
    ] {
        assert_eq!(repo.release(&invalid), Err(CheckError));
    }
}

#[test]
fn updates_metadata_requires_an_https_feed_and_a_32_byte_public_key() {
    let key = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
    assert!(super::has_metadata(
        Some("https://example.com/appcast.xml"),
        Some(key)
    ));
    for (feed, key) in [
        (None, None),
        (Some("https://example.com/feed"), None),
        (None, Some(key)),
        (Some("http://example.com/feed"), Some(key)),
        (Some("https://example.com/feed"), Some("bad")),
        (Some("https://user:password@example.com/feed"), Some(key)),
    ] {
        assert!(!super::has_metadata(feed, key));
    }
}

#[test]
fn updates_settings_sentences_cover_every_state_in_both_languages() {
    use crate::l10n::{set_for_test, Language};
    for (language, sentences) in [
        (
            Language::English,
            [
                "Updates are not set up in this build",
                "Ready to check for updates",
                "Checking…",
                concat!(
                    "Sakura ",
                    env!("CARGO_PKG_VERSION"),
                    " is the latest version"
                ),
                "Sakura 1.2.0 is available",
                "Could not check for updates. Try again later",
            ],
        ),
        (
            Language::Italian,
            [
                "Gli aggiornamenti non sono configurati in questa versione",
                "Pronto per cercare aggiornamenti",
                "Controllo in corso…",
                concat!("Sakura ", env!("CARGO_PKG_VERSION"), " è l'ultima versione"),
                "Sakura 1.2.0 è disponibile",
                "Impossibile cercare aggiornamenti. Riprova più tardi",
            ],
        ),
    ] {
        set_for_test(language);
        for (state, expected) in [
            UpdateState::NotSetUp,
            UpdateState::Ready,
            UpdateState::Checking,
            UpdateState::UpToDate,
            UpdateState::Available(release("1.2.0")),
            UpdateState::Failed,
        ]
        .into_iter()
        .zip(sentences)
        {
            assert_eq!(super::sentence(&state), expected);
        }
    }
}

#[test]
fn a_failed_check_is_a_plain_sentence_with_no_error_text_and_no_address() {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let words = super::sentence(&UpdateState::Failed);
    assert!(
        !words.contains("://") && !words.to_lowercase().contains("error"),
        "{words}"
    );
}
