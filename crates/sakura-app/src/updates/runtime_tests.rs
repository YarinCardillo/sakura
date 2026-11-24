use super::model::{CheckError, Checker};
use super::service::{Installed, Service};
use super::*;
use gpui::TestAppContext;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

struct Fake {
    calls: AtomicUsize,
    result: Result<Release, CheckError>,
}

impl Checker for Fake {
    fn check(&self) -> Result<Release, CheckError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.result.clone()
    }
}

fn install_fake(
    result: Result<Release, CheckError>,
    automatic: bool,
    cx: &mut TestAppContext,
) -> Arc<Fake> {
    let checker = Arc::new(Fake {
        calls: AtomicUsize::new(0),
        result,
    });
    let supplied = checker.clone();
    cx.update(|cx| {
        crate::prefs::Preferences {
            is_automatic_updates_enabled: automatic,
            ..crate::prefs::Preferences::default()
        }
        .install(None, cx);
        let service = cx.new(|_| Service::with_checker(Some(supplied), automatic));
        cx.set_global(Installed(service.clone()));
        service.update(cx, |service, cx| service.start(cx));
    });
    checker
}

fn newer() -> Release {
    Release {
        version: "999.0.0".into(),
        url: "https://github.com/example/app/releases/tag/v999.0.0".into(),
    }
}

#[gpui::test]
fn updates_manual_check_uses_injected_checker_without_overlapping_requests(
    cx: &mut TestAppContext,
) {
    let checker = install_fake(Ok(newer()), false, cx);
    cx.update(|cx| {
        assert_eq!(status(cx).state, UpdateState::Ready);
        check_now(cx);
        assert_eq!(status(cx).state, UpdateState::Checking);
        check_now(cx);
    });
    cx.run_until_parked();
    assert_eq!(checker.calls.load(Ordering::SeqCst), 1);
    cx.update(|cx| assert_eq!(available_release(cx), Some(newer())));
}

#[gpui::test]
fn updates_automatic_checks_follow_the_persisted_preference_and_failure_stays_quiet(
    cx: &mut TestAppContext,
) {
    let checker = install_fake(Err(CheckError), false, cx);
    cx.run_until_parked();
    assert_eq!(checker.calls.load(Ordering::SeqCst), 0);
    cx.update(|cx| set_automatic(true, cx));
    cx.run_until_parked();
    assert_eq!(checker.calls.load(Ordering::SeqCst), 1);
    cx.update(|cx| {
        assert_eq!(status(cx).state, UpdateState::Failed);
        assert!(available_release(cx).is_none());
        set_automatic(false, cx);
        assert!(!crate::prefs::Preferences::current(cx).is_automatic_updates_enabled);
        check_now(cx);
    });
    cx.run_until_parked();
    assert_eq!(checker.calls.load(Ordering::SeqCst), 2);
}

#[gpui::test]
fn updates_automatic_checks_start_at_launch_when_enabled(cx: &mut TestAppContext) {
    let checker = install_fake(Ok(newer()), true, cx);
    cx.run_until_parked();
    assert_eq!(checker.calls.load(Ordering::SeqCst), 1);
}

#[gpui::test]
fn updates_reopening_a_window_does_not_replace_the_checker_or_start_another_timer(
    cx: &mut TestAppContext,
) {
    let checker = install_fake(Ok(newer()), false, cx);
    cx.update(|cx| {
        let original = cx.global::<Installed>().0.entity_id();
        install(true, cx);
        assert_eq!(cx.global::<Installed>().0.entity_id(), original);
        assert_eq!(status(cx).state, UpdateState::Ready);
    });
    cx.run_until_parked();
    assert_eq!(checker.calls.load(Ordering::SeqCst), 0);
}

#[gpui::test]
fn updates_release_link_is_absent_for_all_non_release_states(cx: &mut TestAppContext) {
    for state in [
        UpdateState::NotSetUp,
        UpdateState::Ready,
        UpdateState::Checking,
        UpdateState::UpToDate,
        UpdateState::Failed,
    ] {
        cx.update(|cx| {
            install_for_test(state, cx);
            assert!(available_release(cx).is_none());
        });
    }
}

#[gpui::test]
fn updates_a_check_that_finds_the_running_version_ends_up_to_date_with_that_version_said(
    cx: &mut TestAppContext,
) {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    let running = Release {
        version: CURRENT_VERSION.into(),
        url: String::new(),
    };
    install_fake(Ok(running), false, cx);
    cx.update(|cx| {
        check_now(cx);
        assert_eq!(sentence(&status(cx).state), "Checking…");
    });
    cx.run_until_parked();
    cx.update(|cx| {
        assert_eq!(status(cx).state, UpdateState::UpToDate);
        let expected = format!("Sakura {CURRENT_VERSION} is the latest version");
        assert_eq!(sentence(&status(cx).state), expected);
    });
}

#[gpui::test]
fn updates_a_newer_release_is_available_with_its_version_said(cx: &mut TestAppContext) {
    crate::l10n::set_for_test(crate::l10n::Language::English);
    install_fake(Ok(newer()), false, cx);
    cx.update(check_now);
    cx.run_until_parked();
    cx.update(|cx| {
        assert_eq!(sentence(&status(cx).state), "Sakura 999.0.0 is available");
        assert_eq!(ready_release(cx), Some(newer()));
    });
}

#[gpui::test]
fn updates_a_release_found_by_the_platform_updater_is_ready_though_it_has_no_page(
    cx: &mut TestAppContext,
) {
    let found = Release {
        version: "999.0.0".into(),
        url: String::new(),
    };
    cx.update(|cx| {
        install_for_test(UpdateState::Available(found.clone()), cx);
        assert!(available_release(cx).is_none(), "no page to show");
        assert_eq!(ready_release(cx), Some(found));
    });
}
