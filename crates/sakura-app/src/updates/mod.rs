//! Platform update checks and their state, independent of the views that display them.

#[cfg(any(target_os = "macos", test))]
mod configuration;
#[cfg(any(not(target_os = "macos"), test))]
mod github;
#[cfg(target_os = "macos")]
mod macos;
mod model;
mod presentation;
mod service;

#[cfg(any(target_os = "macos", test))]
pub use configuration::has_metadata;
pub use model::{Release, UpdateState};
pub use presentation::sentence;

use crate::prefs::Preferences;
use gpui::App;
#[cfg(test)]
use gpui::AppContext as _;
use service::Installed;

/// The version of this executable, supplied by Cargo's package metadata.
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// What views need to display, including Sparkle's readiness during installation sessions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub state: UpdateState,
    pub can_check: bool,
}

impl Snapshot {
    /// Whether this build has a functioning platform updater and its public metadata.
    pub fn is_set_up(&self) -> bool {
        self.state != UpdateState::NotSetUp
    }
}

/// Initializes one updater for the application, across window reopenings.
pub fn install(automatic: bool, cx: &mut App) {
    service::install(automatic, cx);
}

/// The current platform result, or a quiet unconfigured state before initialization.
pub fn status(cx: &App) -> Snapshot {
    cx.try_global::<Installed>()
        .map(|installed| installed.0.read(cx).snapshot())
        .unwrap_or(Snapshot {
            state: UpdateState::NotSetUp,
            can_check: false,
        })
}

/// Starts a manual check when the platform is ready; missing setup is harmless.
pub fn check_now(cx: &mut App) {
    if let Some(service) = cx
        .try_global::<Installed>()
        .map(|installed| installed.0.clone())
    {
        service.update(cx, |service, cx| service.check_now(cx));
    }
}

/// The stored preference, on by default even in builds without update metadata.
pub fn is_automatic(cx: &App) -> bool {
    Preferences::try_current(cx).is_none_or(|prefs| prefs.is_automatic_updates_enabled)
}

/// Stores the preference and immediately applies it to the platform scheduler and downloads.
pub fn set_automatic(enabled: bool, cx: &mut App) {
    if Preferences::try_current(cx).is_none() {
        return;
    }
    Preferences::change(cx, |prefs| prefs.is_automatic_updates_enabled = enabled);
    if let Some(service) = cx
        .try_global::<Installed>()
        .map(|installed| installed.0.clone())
    {
        service.update(cx, |service, cx| service.set_automatic(enabled, cx));
    }
    cx.refresh_windows();
}

/// The update that is ready to take, however it was found: through the public release page, or by the
/// platform updater, which has no page to show.
#[cfg(any(test, not(feature = "boxes")))]
pub fn ready_release(cx: &App) -> Option<Release> {
    match status(cx).state {
        UpdateState::Available(release) => Some(release),
        _ => None,
    }
}

/// Takes the update that is ready: the platform updater installs it and restarts the app; a build that
/// only checks the public page opens it.
pub fn install_update(cx: &mut App) {
    if let Some(service) = cx
        .try_global::<Installed>()
        .map(|installed| installed.0.clone())
    {
        service.update(cx, |service, cx| service.install_update(cx));
    }
}

/// The public release page for non-installing checkers; Sparkle provides its own update UI.
pub fn available_release(cx: &App) -> Option<Release> {
    match status(cx).state {
        UpdateState::Available(release) if !release.url.is_empty() => Some(release),
        _ => None,
    }
}

#[cfg(test)]
pub(crate) fn install_for_test(state: UpdateState, cx: &mut App) {
    let service = cx.new(|_| service::Service::with_checker(None, false));
    service.update(cx, |service, _| service.set_state(state));
    cx.set_global(Installed(service));
}

#[cfg(test)]
mod runtime_tests;
#[cfg(test)]
mod tests;
