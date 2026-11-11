//! Where Play is asked and its log read: a [`PlaySource`], never `box` itself, so a test hands the
//! view one that counts what was asked. The real one, [`BoxPlay`], runs `box play` on the tokio
//! runtime; its answers are awaited from GPUI's executor, so the window never waits on a process.

use crate::ui::files::{on_runtime, Reading};
use sakura_core::box_cli::BoxCli;
use sakura_core::play::{self, Action, Log, Outcome};
use std::sync::Arc;
use tokio::runtime::Runtime;

/// The Play of one task.
pub trait PlaySource {
    /// Starts it, stops it, or asks where it stands.
    fn play(&self, action: Action) -> Reading<Outcome>;
    /// The newest lines of its output.
    fn log(&self) -> Reading<Log>;
}

/// The Play of the task `task`, over `box play`.
pub struct BoxPlay {
    cli: BoxCli,
    task: String,
    runtime: Arc<Runtime>,
}

impl BoxPlay {
    /// Of `task` over `cli`, whose calls run on `runtime`.
    pub fn new(cli: BoxCli, task: impl Into<String>, runtime: Arc<Runtime>) -> Self {
        Self {
            cli,
            task: task.into(),
            runtime,
        }
    }
}

impl PlaySource for BoxPlay {
    fn play(&self, action: Action) -> Reading<Outcome> {
        let (cli, task) = (self.cli.clone(), self.task.clone());
        on_runtime(&self.runtime, async move {
            play::play(&cli, &task, action).await
        })
    }

    fn log(&self) -> Reading<Log> {
        let (cli, task) = (self.cli.clone(), self.task.clone());
        on_runtime(
            &self.runtime,
            async move { play::read_log(&cli, &task).await },
        )
    }
}
