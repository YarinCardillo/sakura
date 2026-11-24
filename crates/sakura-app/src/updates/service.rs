use super::model::{Checker, UpdateMachine, UpdateState};
use super::{Snapshot, CURRENT_VERSION};
use gpui::{App, AppContext as _, Context, Entity, Global, Task};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub(super) struct Installed(pub Entity<Service>);
impl Global for Installed {}

pub(super) struct Service {
    machine: UpdateMachine,
    checker: Option<Arc<dyn Checker>>,
    #[cfg(target_os = "macos")]
    sparkle: Option<super::macos::Sparkle>,
    is_automatic: bool,
    last_snapshot: Snapshot,
    checking: Option<Task<()>>,
    timer: Option<Task<()>>,
}

impl Service {
    pub(super) fn platform(automatic: bool) -> Self {
        #[cfg(target_os = "macos")]
        let sparkle = super::macos::Sparkle::new(automatic);
        #[cfg(target_os = "macos")]
        let checker = None;
        #[cfg(not(target_os = "macos"))]
        let checker = super::github::Repository::parse(env!("CARGO_PKG_REPOSITORY"))
            .map(|repo| Arc::new(repo) as Arc<dyn Checker>);
        let mut service = Self::with_checker(checker, automatic);
        #[cfg(target_os = "macos")]
        {
            service.sparkle = sparkle;
        }
        service.last_snapshot = service.snapshot();
        #[cfg(target_os = "macos")]
        if service.sparkle.is_some() {
            eprintln!("sakura: Sparkle updater configured");
        }
        service
    }

    pub(super) fn with_checker(checker: Option<Arc<dyn Checker>>, automatic: bool) -> Self {
        Self {
            machine: UpdateMachine::new(checker.is_some()),
            checker,
            #[cfg(target_os = "macos")]
            sparkle: None,
            is_automatic: automatic,
            last_snapshot: Snapshot {
                state: UpdateState::NotSetUp,
                can_check: false,
            },
            checking: None,
            timer: None,
        }
    }

    pub(super) fn snapshot(&self) -> Snapshot {
        #[cfg(target_os = "macos")]
        if let Some(sparkle) = &self.sparkle {
            return Snapshot {
                state: sparkle.state(),
                can_check: sparkle.can_check(),
            };
        }
        Snapshot {
            state: self.machine.state().clone(),
            can_check: self.machine.can_check(),
        }
    }

    pub(super) fn start(&mut self, cx: &mut Context<Self>) {
        if !self.snapshot().is_set_up() {
            return;
        }
        self.tick(cx);
        let interval = if cfg!(target_os = "macos") { 1 } else { 60 };
        self.timer = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(Duration::from_secs(interval))
                .await;
            if this.update(cx, |service, cx| service.tick(cx)).is_err() {
                break;
            }
        }));
    }

    pub(super) fn check_now(&mut self, cx: &mut Context<Self>) {
        #[cfg(target_os = "macos")]
        if let Some(sparkle) = &self.sparkle {
            sparkle.check_now();
            self.changed(cx);
            return;
        }
        let Some(checker) = self.checker.clone() else {
            return;
        };
        if !self.machine.begin(Instant::now()) {
            return;
        }
        self.changed(cx);
        let check = cx.background_spawn(async move { checker.check() });
        self.checking = Some(cx.spawn(async move |this, cx| {
            let result = check.await;
            drop(this.update(cx, |service, cx| {
                service.machine.finish(CURRENT_VERSION, result);
                service.changed(cx);
            }));
        }));
    }

    /// Takes the update found: Sparkle's own install and relaunch when it found it, else the release page.
    pub(super) fn install_update(&mut self, cx: &mut Context<Self>) {
        #[cfg(target_os = "macos")]
        if let Some(sparkle) = &self.sparkle {
            sparkle.check_now();
            self.changed(cx);
            return;
        }
        if let UpdateState::Available(release) = self.machine.state() {
            cx.open_url(&release.url);
        }
    }

    pub(super) fn set_automatic(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.is_automatic = enabled;
        #[cfg(target_os = "macos")]
        if let Some(sparkle) = &self.sparkle {
            sparkle.set_automatic(enabled);
        }
        self.tick(cx);
    }

    fn tick(&mut self, cx: &mut Context<Self>) {
        if self.checker.is_some() && self.machine.is_due(self.is_automatic, Instant::now()) {
            self.check_now(cx);
        }
        self.changed(cx);
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        let snapshot = self.snapshot();
        if snapshot != self.last_snapshot {
            self.last_snapshot = snapshot;
            cx.refresh_windows();
        }
    }

    #[cfg(test)]
    pub(super) fn set_state(&mut self, state: UpdateState) {
        self.machine.set_state(state);
    }
}

/// Installs once per application, so reopening a window cannot duplicate the updater.
pub(super) fn install(automatic: bool, cx: &mut App) {
    if cx.try_global::<Installed>().is_some() {
        return;
    }
    let service = cx.new(|_| Service::platform(automatic));
    cx.set_global(Installed(service.clone()));
    service.update(cx, |service, cx| service.start(cx));
}
