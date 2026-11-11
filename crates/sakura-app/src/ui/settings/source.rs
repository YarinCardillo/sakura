//! Where Settings reads and writes: a [`SettingsSource`], never `box` itself, so a test hands the
//! window one that counts what was asked. The real one, [`BoxSettings`], runs `box` on the tokio
//! runtime; its answers are awaited from GPUI's executor, so the window never waits on a process.

use crate::ui::files::{on_runtime, Reading};
use sakura_core::box_cli::{BoxCli, Limit};
use sakura_core::models::{Account, BoxStatus};
use sakura_core::settings::detect::{self, Addition, Detected};
use sakura_core::settings::{self, Branches, Change, Listed};
use std::sync::Arc;
use tokio::runtime::Runtime;

/// What Settings asks of the service.
pub trait SettingsSource {
    fn projects(&self) -> Reading<Vec<Listed>>;
    fn branches(&self, project: &str) -> Reading<Branches>;
    fn accounts(&self) -> Reading<Vec<Account>>;
    /// `box status --json`: the runner and the agents, for "Machine".
    fn status(&self) -> Reading<BoxStatus>;
    fn set(&self, project: &str, change: Change) -> Reading<()>;
    fn detect(&self, folder: &str) -> Reading<Detected>;
    fn add(&self, addition: Addition) -> Reading<()>;
    fn remove(&self, project: &str) -> Reading<()>;
}

/// Settings over `box`.
pub struct BoxSettings {
    cli: BoxCli,
    runtime: Arc<Runtime>,
}

impl BoxSettings {
    /// Over `cli`, whose calls run on `runtime`.
    pub fn new(cli: BoxCli, runtime: Arc<Runtime>) -> Self {
        Self { cli, runtime }
    }
}

impl SettingsSource for BoxSettings {
    fn projects(&self) -> Reading<Vec<Listed>> {
        let cli = self.cli.clone();
        on_runtime(
            &self.runtime,
            async move { settings::read_projects(&cli).await },
        )
    }

    fn branches(&self, project: &str) -> Reading<Branches> {
        let (cli, project) = (self.cli.clone(), project.to_string());
        on_runtime(&self.runtime, async move {
            settings::read_branches(&cli, &project).await
        })
    }

    fn accounts(&self) -> Reading<Vec<Account>> {
        let cli = self.cli.clone();
        on_runtime(
            &self.runtime,
            async move { settings::read_accounts(&cli).await },
        )
    }

    fn status(&self) -> Reading<BoxStatus> {
        let cli = self.cli.clone();
        on_runtime(&self.runtime, async move {
            cli.run_json(&["status", "--json"], Limit::Slow).await
        })
    }

    fn set(&self, project: &str, change: Change) -> Reading<()> {
        let (cli, project) = (self.cli.clone(), project.to_string());
        on_runtime(&self.runtime, async move {
            settings::set_project(&cli, &project, &change).await
        })
    }

    fn detect(&self, folder: &str) -> Reading<Detected> {
        let (cli, folder) = (self.cli.clone(), folder.to_string());
        on_runtime(
            &self.runtime,
            async move { detect::detect(&cli, &folder).await },
        )
    }

    fn add(&self, addition: Addition) -> Reading<()> {
        let cli = self.cli.clone();
        on_runtime(&self.runtime, async move {
            detect::add_project(&cli, &addition).await
        })
    }

    fn remove(&self, project: &str) -> Reading<()> {
        let (cli, project) = (self.cli.clone(), project.to_string());
        on_runtime(&self.runtime, async move {
            settings::remove_project(&cli, &project).await
        })
    }
}
