//! The adapter between `sakura-core`'s pty sessions and a terminal view.
//!
//! `sakura_core::terminal::Terminals` reports every session's output through one callback, by session id. A
//! [`CoreRouter`] is that callback: it hands the bytes to the view showing the session, keeping what arrives
//! before the view attaches. A [`CoreSession`] is the [`TerminalSource`] for one
//! session id.

use crate::size::GridSize;
use crate::source::{OutputSink, SourceError, TerminalSource};
use sakura_core::error::ErrorCode;
use sakura_core::terminal::{TerminalEvent, TerminalSink, Terminals};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// Most bytes kept for a session no view has attached to yet; older output beyond this is dropped.
const MAX_PENDING_BYTES: usize = 1024 * 1024;
/// Largest write sent to `sakura-core` in one call (it refuses more than 4 MiB).
const MAX_WRITE_CHUNK: usize = 1024 * 1024;

#[derive(Default)]
struct Pending {
    bytes: Vec<u8>,
    exit: Option<Option<i32>>,
}

#[derive(Default)]
struct RouterState {
    sinks: HashMap<String, OutputSink>,
    pending: HashMap<String, Pending>,
}

/// Routes `sakura-core` terminal events to the terminal attached to each session id.
#[derive(Clone, Default)]
pub struct CoreRouter {
    state: Arc<Mutex<RouterState>>,
}

impl CoreRouter {
    /// A router with no sessions attached.
    pub fn new() -> Self {
        Self::default()
    }

    /// The callback to pass to `Terminals::new`.
    pub fn core_sink(&self) -> TerminalSink {
        let router = self.clone();
        Arc::new(move |event| router.deliver(event))
    }

    fn deliver(&self, event: TerminalEvent<'_>) {
        let (id, output) = match event {
            TerminalEvent::Data { id, bytes } => (id.to_string(), Ok(bytes.to_vec())),
            TerminalEvent::Exit(exit) => (exit.id, Err(exit.exit_code)),
        };
        let sink = {
            let mut state = lock(&self.state);
            match state.sinks.get(&id) {
                Some(sink) => sink.clone(),
                None => {
                    keep_pending(state.pending.entry(id).or_default(), output);
                    return;
                }
            }
        };
        match output {
            Ok(bytes) => sink.bytes(&bytes),
            Err(code) => sink.exited(code),
        }
    }

    /// Attaches `sink` to session `id`, first handing it what arrived before.
    fn attach(&self, id: &str, sink: OutputSink) {
        let mut state = lock(&self.state);
        // Flushing under the lock keeps the order: output that arrives meanwhile waits for the lock.
        if let Some(pending) = state.pending.remove(id) {
            if !pending.bytes.is_empty() {
                sink.bytes(&pending.bytes);
            }
            if let Some(code) = pending.exit {
                sink.exited(code);
            }
        }
        state.sinks.insert(id.to_string(), sink);
    }

    /// Whether the router holds nothing for any session.
    #[cfg(test)]
    fn is_empty(&self) -> bool {
        let state = lock(&self.state);
        state.sinks.is_empty() && state.pending.is_empty()
    }

    fn detach(&self, id: &str) {
        let mut state = lock(&self.state);
        state.sinks.remove(id);
        state.pending.remove(id);
    }
}

fn keep_pending(pending: &mut Pending, output: Result<Vec<u8>, Option<i32>>) {
    match output {
        Ok(bytes) => {
            pending.bytes.extend_from_slice(&bytes);
            let excess = pending.bytes.len().saturating_sub(MAX_PENDING_BYTES);
            pending.bytes.drain(..excess);
        }
        Err(code) => pending.exit = Some(code),
    }
}

/// One `sakura-core` session (`box attach`, `box shell` or `box login`) as a terminal source.
pub struct CoreSession {
    terminals: Terminals,
    id: String,
    router: CoreRouter,
}

impl CoreSession {
    /// The session `id` opened with `terminals`, whose events reach `router`.
    pub fn new(terminals: Terminals, id: impl Into<String>, router: CoreRouter) -> Self {
        Self {
            terminals,
            id: id.into(),
            router,
        }
    }
}

impl TerminalSource for CoreSession {
    fn start(&self, sink: OutputSink) {
        self.router.attach(&self.id, sink);
    }

    fn write(&self, bytes: &[u8]) -> Result<(), SourceError> {
        for chunk in bytes.chunks(MAX_WRITE_CHUNK) {
            self.terminals
                .write(&self.id, chunk)
                .map_err(|error| SourceError::new(error.to_string()))?;
        }
        Ok(())
    }

    fn resize(&self, size: GridSize) -> Result<(), SourceError> {
        self.terminals
            .resize(&self.id, size.columns, size.rows)
            .map_err(|error| SourceError::new(error.to_string()))
    }

    /// Closes the session. `sakura-core` reports nothing about it once its close returned, so detaching after
    /// that leaves the router with nothing for this id.
    fn close(&self) {
        match self.terminals.close(&self.id) {
            Ok(()) => {}
            Err(error) if error.code == ErrorCode::TerminalClosed => {}
            Err(error) => log::warn!("closing terminal {}: {error}", self.id),
        }
        self.router.detach(&self.id);
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::SourceOutput;
    use sakura_core::box_cli::BoxCli;
    use sakura_core::models::{TerminalExit, TerminalKind, TerminalRequest};
    use std::sync::mpsc::{channel, Receiver};
    use std::time::Duration;

    /// Output as received: bytes, or the exit code.
    type Recorded = Result<Vec<u8>, Option<i32>>;

    fn recording_sink() -> (OutputSink, Receiver<Recorded>) {
        let (sender, outputs) = channel();
        let sink = OutputSink::new(move |output| {
            let owned = match output {
                SourceOutput::Bytes(bytes) => Ok(bytes.to_vec()),
                SourceOutput::Exited(code) => Err(code),
            };
            let _ = sender.send(owned);
        });
        (sink, outputs)
    }

    fn data<'a>(id: &'a str, bytes: &'a [u8]) -> TerminalEvent<'a> {
        TerminalEvent::Data { id, bytes }
    }

    #[test]
    fn router_keeps_output_until_attach_then_forwards_in_order() {
        let router = CoreRouter::new();
        let deliver = router.core_sink();
        deliver(data("t1", b"early "));
        deliver(data("t2", b"other"));
        let (sink, outputs) = recording_sink();
        router.attach("t1", sink);
        deliver(data("t1", b"late"));
        deliver(TerminalEvent::Exit(TerminalExit {
            id: "t1".into(),
            exit_code: Some(0),
        }));
        let received: Vec<_> = outputs.try_iter().collect();
        assert_eq!(
            received,
            vec![Ok(b"early ".to_vec()), Ok(b"late".to_vec()), Err(Some(0))]
        );
    }

    #[test]
    fn router_caps_output_kept_for_unattached_sessions() {
        let router = CoreRouter::new();
        let deliver = router.core_sink();
        deliver(data("t1", &vec![b'a'; MAX_PENDING_BYTES]));
        deliver(data("t1", b"tail"));
        let (sink, outputs) = recording_sink();
        router.attach("t1", sink);
        let Ok(bytes) = outputs.recv().unwrap() else {
            panic!("expected bytes");
        };
        assert_eq!(bytes.len(), MAX_PENDING_BYTES);
        assert!(bytes.ends_with(b"tail"));
    }

    #[test]
    fn core_session_drives_a_real_sakura_core_terminal() {
        let dir = std::env::temp_dir().join(format!("sakura-terminal-core-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("box");
        sakura_core::test_support::write_script(
            &script,
            concat!(
                "#!/bin/sh\nread line; stty size; echo \"$1 $2 $line\"",
                "\n"
            ),
        );
        let router = CoreRouter::new();
        let terminals = Terminals::new(BoxCli::at(&script), dir.clone(), router.core_sink());
        let request = TerminalRequest {
            kind: TerminalKind::Shell,
            task: Some("m1".into()),
            account: None,
            cols: 80,
            rows: 24,
        };
        let id = terminals.open(&request).unwrap();
        let session = CoreSession::new(terminals, id, router);
        let (sink, outputs) = recording_sink();
        session.start(sink);
        session.resize(GridSize::new(100, 40)).unwrap();
        session.write(b"hi\n").unwrap();
        let mut text = String::new();
        let exit = loop {
            match outputs
                .recv_timeout(Duration::from_secs(10))
                .expect("session output")
            {
                Ok(bytes) => text.push_str(&String::from_utf8_lossy(&bytes)),
                Err(code) => break code,
            }
        };
        assert!(text.contains("40 100"), "{text:?}");
        assert!(text.contains("shell m1 hi"), "{text:?}");
        assert_eq!(exit, Some(0));
        session.close();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn closing_a_session_during_output_leaves_no_callback_and_an_empty_router() {
        let dir =
            std::env::temp_dir().join(format!("sakura-terminal-close-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("box");
        sakura_core::test_support::write_script(
            &script,
            concat!(
                "#!/bin/sh\nwhile :; do echo xxxxxxxxxxxxxxxxxxxxxxxx; done",
                "\n"
            ),
        );
        let router = CoreRouter::new();
        let terminals = Terminals::new(BoxCli::at(&script), dir.clone(), router.core_sink());
        let request = TerminalRequest {
            kind: TerminalKind::Shell,
            task: Some("m1".into()),
            account: None,
            cols: 80,
            rows: 24,
        };
        let id = terminals.open(&request).unwrap();
        let session = CoreSession::new(terminals, id, router.clone());
        let (sink, outputs) = recording_sink();
        session.start(sink);
        assert!(outputs.recv_timeout(Duration::from_secs(10)).is_ok());
        session.close();
        while outputs.try_recv().is_ok() {}
        std::thread::sleep(Duration::from_secs(3));
        assert!(outputs.try_recv().is_err(), "output after close");
        assert!(router.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
