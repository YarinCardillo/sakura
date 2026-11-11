//! A source that runs a command in a local pty: used by the example binary and by tests.

use crate::size::GridSize;
use crate::source::{OutputSink, SourceError, TerminalSource};
use portable_pty::{native_pty_system, Child, ChildKiller, CommandBuilder, MasterPty, PtySize};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Mutex, MutexGuard, PoisonError};

const READ_CHUNK_BYTES: usize = 64 * 1024;
const TERM_VALUE: &str = "xterm-256color";
const FALLBACK_SHELL: &str = "/bin/zsh";
const FALLBACK_LANG: &str = "en_US.UTF-8";

/// A command running in a pty on this machine.
pub struct LocalPty {
    writer: Mutex<Box<dyn Write + Send>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    pending: Mutex<Option<Pending>>,
}

/// The reader and the child, until `start` hands them to the reader thread.
struct Pending {
    reader: Box<dyn Read + Send>,
    child: Box<dyn Child + Send + Sync>,
}

impl LocalPty {
    /// Runs `program` with `args` in a new pty of `size`, in `cwd` when given.
    pub fn spawn(
        program: &str,
        args: &[String],
        cwd: Option<&Path>,
        size: GridSize,
    ) -> Result<Self, SourceError> {
        let pair = native_pty_system()
            .openpty(pty_size(size))
            .map_err(pty_error)?;
        let child = pair
            .slave
            .spawn_command(command(program, args, cwd))
            .map_err(|error| SourceError::new(format!("Could not start {program}: {error}.")))?;
        drop(pair.slave);
        let reader = pair.master.try_clone_reader().map_err(pty_error)?;
        let writer = pair.master.take_writer().map_err(pty_error)?;
        Ok(Self {
            writer: Mutex::new(writer),
            killer: Mutex::new(child.clone_killer()),
            master: Mutex::new(pair.master),
            pending: Mutex::new(Some(Pending { reader, child })),
        })
    }

    /// Runs the user's login shell (`$SHELL -l`).
    pub fn login_shell(cwd: Option<&Path>, size: GridSize) -> Result<Self, SourceError> {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| FALLBACK_SHELL.to_string());
        Self::spawn(&shell, &["-l".to_string()], cwd, size)
    }
}

impl TerminalSource for LocalPty {
    fn start(&self, sink: OutputSink) {
        let Some(Pending {
            mut reader,
            mut child,
        }) = lock(&self.pending).take()
        else {
            log::warn!("local pty started twice; the second start is ignored");
            return;
        };
        let spawned = std::thread::Builder::new()
            .name("sakura-terminal-pty-reader".into())
            .spawn(move || {
                let mut chunk = vec![0u8; READ_CHUNK_BYTES];
                while let Ok(read) = reader.read(&mut chunk) {
                    if read == 0 {
                        break;
                    }
                    sink.bytes(&chunk[..read]);
                }
                let code = child
                    .wait()
                    .ok()
                    .map(|status| i32::try_from(status.exit_code()).unwrap_or(i32::MAX));
                sink.exited(code);
            });
        if let Err(error) = spawned {
            log::error!("pty reader thread did not start: {error}");
        }
    }

    fn write(&self, bytes: &[u8]) -> Result<(), SourceError> {
        let mut writer = lock(&self.writer);
        writer
            .write_all(bytes)
            .and_then(|()| writer.flush())
            .map_err(|error| {
                SourceError::new(format!("The terminal does not accept input: {error}."))
            })
    }

    fn resize(&self, size: GridSize) -> Result<(), SourceError> {
        lock(&self.master).resize(pty_size(size)).map_err(pty_error)
    }

    fn close(&self) {
        if let Err(error) = lock(&self.killer).kill() {
            log::debug!("closing the local pty: {error}");
        }
    }
}

fn command(program: &str, args: &[String], cwd: Option<&Path>) -> CommandBuilder {
    let mut command = CommandBuilder::new(program);
    command.args(args);
    command.env("TERM", TERM_VALUE);
    command.env("COLORTERM", "truecolor");
    if std::env::var_os("LANG").is_none() {
        command.env("LANG", FALLBACK_LANG);
    }
    if let Some(cwd) = cwd {
        command.cwd(cwd);
    } else if let Some(home) = std::env::var_os("HOME") {
        command.cwd(home);
    }
    command
}

fn pty_size(size: GridSize) -> PtySize {
    PtySize {
        rows: size.rows,
        cols: size.columns,
        pixel_width: size.cell_width.saturating_mul(size.columns),
        pixel_height: size.cell_height.saturating_mul(size.rows),
    }
}

fn pty_error(error: impl std::fmt::Display) -> SourceError {
    SourceError::new(format!("Terminal unavailable: {error}."))
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::SourceOutput;
    use std::sync::mpsc::channel;
    use std::time::Duration;

    #[test]
    fn local_pty_keeps_output_of_a_command_that_exits_at_once() {
        let pty =
            LocalPty::spawn("/bin/echo", &["quick".into()], None, GridSize::new(80, 24)).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        let (sender, outputs) = channel();
        pty.start(OutputSink::new(move |output| {
            let owned = match output {
                SourceOutput::Bytes(bytes) => Ok(bytes.to_vec()),
                SourceOutput::Exited(code) => Err(code),
            };
            let _ = sender.send(owned);
        }));
        let mut text = String::new();
        while let Ok(bytes) = outputs
            .recv_timeout(Duration::from_secs(10))
            .expect("pty output")
        {
            text.push_str(&String::from_utf8_lossy(&bytes));
        }
        assert!(text.contains("quick"), "{text:?}");
    }

    #[test]
    fn local_pty_runs_a_command_reports_size_and_exit_code() {
        let script = "read line; stty size; echo got:$line; exit 7".to_string();
        let pty = LocalPty::spawn(
            "/bin/sh",
            &["-c".into(), script],
            None,
            GridSize::new(80, 24),
        )
        .unwrap();
        let (sender, outputs) = channel();
        pty.start(OutputSink::new(move |output| {
            let owned = match output {
                SourceOutput::Bytes(bytes) => Ok(bytes.to_vec()),
                SourceOutput::Exited(code) => Err(code),
            };
            let _ = sender.send(owned);
        }));
        pty.resize(GridSize::new(132, 43)).unwrap();
        pty.write(b"hello\n").unwrap();
        let mut text = String::new();
        let exit = loop {
            match outputs
                .recv_timeout(Duration::from_secs(10))
                .expect("pty output")
            {
                Ok(bytes) => text.push_str(&String::from_utf8_lossy(&bytes)),
                Err(code) => break code,
            }
        };
        assert!(text.contains("43 132"), "{text:?}");
        assert!(text.contains("got:hello"), "{text:?}");
        assert_eq!(exit, Some(7));
    }
}
