use std::time::{Duration, Instant};

/// The minimum interval between automatic public release checks.
pub const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateState {
    NotSetUp,
    Ready,
    Checking,
    UpToDate,
    Available(Release),
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CheckError;

pub trait Checker: Send + Sync {
    /// Fetches one release without changing application state.
    fn check(&self) -> Result<Release, CheckError>;
}

pub struct UpdateMachine {
    state: UpdateState,
    last_check: Option<Instant>,
}

impl UpdateMachine {
    /// A checker is ready only when its platform metadata is available.
    pub fn new(is_set_up: bool) -> Self {
        Self {
            state: if is_set_up {
                UpdateState::Ready
            } else {
                UpdateState::NotSetUp
            },
            last_check: None,
        }
    }

    /// The latest result, or the check currently in flight.
    pub fn state(&self) -> &UpdateState {
        &self.state
    }

    /// Manual checks are independent of the automatic-check preference.
    pub fn can_check(&self) -> bool {
        !matches!(self.state, UpdateState::NotSetUp | UpdateState::Checking)
    }

    /// Reserves one check; another cannot start until it finishes.
    pub fn begin(&mut self, now: Instant) -> bool {
        if !self.can_check() {
            return false;
        }
        self.state = UpdateState::Checking;
        self.last_check = Some(now);
        true
    }

    /// Applies the result of an in-flight check, including invalid release metadata.
    pub fn finish(&mut self, current: &str, result: Result<Release, CheckError>) {
        if self.state != UpdateState::Checking {
            return;
        }
        self.state = match result {
            Ok(release) => match is_newer(current, &release.version) {
                Ok(true) => UpdateState::Available(release),
                Ok(false) => UpdateState::UpToDate,
                Err(_) => UpdateState::Failed,
            },
            Err(_) => UpdateState::Failed,
        };
    }

    /// Whether automatic checks are enabled and at least a day has elapsed.
    pub fn is_due(&self, automatic: bool, now: Instant) -> bool {
        automatic
            && self.can_check()
            && self
                .last_check
                .is_none_or(|last| now.saturating_duration_since(last) >= CHECK_INTERVAL)
    }

    #[cfg(test)]
    pub(super) fn set_state(&mut self, state: UpdateState) {
        self.state = state;
    }
}

/// Compares SemVer precedence; build metadata never turns an equal version into an update.
pub fn is_newer(current: &str, candidate: &str) -> Result<bool, CheckError> {
    Ok(parse_version(current)?
        .cmp_precedence(&parse_version(candidate)?)
        .is_lt())
}

pub(super) fn parse_version(text: &str) -> Result<semver::Version, CheckError> {
    semver::Version::parse(text.strip_prefix('v').unwrap_or(text)).map_err(|_| CheckError)
}
