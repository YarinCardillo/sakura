//! Where the files of a task are read from. The views ask a [`Source`] and never `box` itself, so a
//! test hands them one that counts what was read. The real one, [`DirectSource`], reads the task's
//! folder in-process (`sakura_core::direct`) on the runtime's blocking threads, with no process started;
//! a repository in a form that reader does not read is read through `box` ([`BoxSource`]) instead.
//! Answers are awaited from GPUI's executor, so the window never waits on a read.

#[cfg(feature = "boxes")]
use sakura_core::box_cli::BoxCli;
#[cfg(feature = "boxes")]
use sakura_core::direct::{Readers, TaskReader};
#[cfg(feature = "boxes")]
use sakura_core::error::ErrorCode;
use sakura_core::error::{BridgeError, BridgeResult};
#[cfg(feature = "boxes")]
use sakura_core::files;
use sakura_core::files::{FileBody, Level, Matches};
use sakura_core::said::Said;
use std::future::Future;
use std::pin::Pin;
#[cfg(feature = "boxes")]
use std::sync::Arc;
use tokio::runtime::Runtime;

/// An answer of `box` on its way.
pub type Reading<T> = Pin<Box<dyn Future<Output = BridgeResult<T>> + Send>>;

/// The files of one task.
pub trait Source {
    /// One directory level; `dir` is empty for the root.
    fn level(&self, dir: &str) -> Reading<Level>;
    /// The body of one file.
    fn file(&self, path: &str) -> Reading<FileBody>;
    /// The files whose path holds `text`.
    fn search(&self, text: &str) -> Reading<Matches>;
}

/// The files of the task `task`, read with `box tree`, `box file` and `box search`.
#[derive(Clone)]
#[cfg(feature = "boxes")]
pub struct BoxSource {
    cli: BoxCli,
    task: String,
    runtime: Arc<Runtime>,
}

#[cfg(feature = "boxes")]
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

#[cfg(feature = "boxes")]
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

/// The files of the task `task`, read in-process.
#[cfg(feature = "boxes")]
pub struct DirectSource {
    readers: Arc<Readers>,
    service: BoxSource,
}

#[cfg(feature = "boxes")]
impl DirectSource {
    /// The source of `task` read with `readers`; `service` reads what they cannot.
    pub fn new(readers: Arc<Readers>, service: BoxSource) -> Self {
        Self { readers, service }
    }

    /// Runs `read` on the task's reader off the runtime's threads; a repository the reader does not
    /// read (`Unsupported`) is asked of `fallback` instead.
    fn read<T: Send + 'static>(
        &self,
        read: impl Fn(&TaskReader) -> BridgeResult<T> + Send + 'static,
        fallback: impl FnOnce() -> Reading<T> + Send + 'static,
    ) -> Reading<T> {
        let (readers, task) = (self.readers.clone(), self.service.task.clone());
        on_runtime(&self.service.runtime, async move {
            let direct = tokio::task::spawn_blocking(move || readers.with_task(&task, read)).await;
            match direct.unwrap_or_else(|error| Err(joined(error))) {
                Err(error) if error.code == ErrorCode::Unsupported => fallback().await,
                answer => answer,
            }
        })
    }
}

#[cfg(feature = "boxes")]
impl Source for DirectSource {
    fn level(&self, dir: &str) -> Reading<Level> {
        let (service, wanted) = (self.service.clone(), dir.to_string());
        let dir = dir.to_string();
        self.read(
            move |reader| reader.level(&dir),
            move || service.level(&wanted),
        )
    }

    fn file(&self, path: &str) -> Reading<FileBody> {
        let (service, wanted) = (self.service.clone(), path.to_string());
        let path = path.to_string();
        self.read(
            move |reader| reader.file(&path),
            move || service.file(&wanted),
        )
    }

    fn search(&self, text: &str) -> Reading<Matches> {
        let (service, wanted) = (self.service.clone(), text.to_string());
        let text = text.to_string();
        self.read(
            move |reader| reader.search(&text),
            move || service.search(&wanted),
        )
    }
}

/// A read that could not finish: the runtime is shutting down.
pub fn joined(error: tokio::task::JoinError) -> BridgeError {
    BridgeError::internal(Said::new("error.internal")).with_details(Some(error.to_string()))
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
