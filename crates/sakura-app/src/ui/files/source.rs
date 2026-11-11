//! Where the files of a lavoro are read from. The views ask a [`Source`] and never `box` itself, so a
//! test hands them one that counts what was read. The real one, [`BoxSource`], runs `box` on the tokio
//! runtime; its answers are awaited from GPUI's executor, so the window never waits on a process.

use sakura_core::box_cli::BoxCli;
use sakura_core::error::{BridgeError, BridgeResult};
use sakura_core::files::{self, FileBody, Level, Matches};
use sakura_core::said::Said;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio::runtime::Runtime;

/// An answer of `box` on its way.
pub type Reading<T> = Pin<Box<dyn Future<Output = BridgeResult<T>> + Send>>;

/// The files of one lavoro.
pub trait Source {
    /// One directory level; `dir` is empty for the root.
    fn level(&self, dir: &str) -> Reading<Level>;
    /// The body of one file.
    fn file(&self, path: &str) -> Reading<FileBody>;
    /// The files whose path holds `text`.
    fn search(&self, text: &str) -> Reading<Matches>;
}

/// The files of the lavoro `task`, read with `box tree`, `box file` and `box search`.
pub struct BoxSource {
    cli: BoxCli,
    task: String,
    runtime: Arc<Runtime>,
}

impl BoxSource {
    /// The source of `task` over `cli`, whose calls run on `runtime`.
    pub fn new(cli: BoxCli, task: impl Into<String>, runtime: Arc<Runtime>) -> Self {
        Self {
            cli,
            task: task.into(),
            runtime,
        }
    }
}

impl Source for BoxSource {
    fn level(&self, dir: &str) -> Reading<Level> {
        let (cli, task, dir) = (self.cli.clone(), self.task.clone(), dir.to_string());
        on_runtime(&self.runtime, async move {
            files::read_level(&cli, &task, &dir).await
        })
    }

    fn file(&self, path: &str) -> Reading<FileBody> {
        let (cli, task, path) = (self.cli.clone(), self.task.clone(), path.to_string());
        on_runtime(&self.runtime, async move {
            files::read_file(&cli, &task, &path).await
        })
    }

    fn search(&self, text: &str) -> Reading<Matches> {
        let (cli, task, text) = (self.cli.clone(), self.task.clone(), text.to_string());
        on_runtime(&self.runtime, async move {
            files::search(&cli, &task, &text).await
        })
    }
}

/// Runs `work` on `runtime`; the answer is awaited from anywhere. A call that could not finish (the
/// runtime is shutting down) is an internal failure, never a hang.
pub fn on_runtime<T: Send + 'static>(
    runtime: &Runtime,
    work: impl Future<Output = BridgeResult<T>> + Send + 'static,
) -> Reading<T> {
    let handle = runtime.spawn(work);
    Box::pin(async move {
        let joined = handle.await;
        joined.unwrap_or_else(|error| {
            let failure = BridgeError::internal(Said::new("error.internal"));
            Err(failure.with_details(Some(error.to_string())))
        })
    })
}
