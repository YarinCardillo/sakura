//! Where a terminal's bytes come from and go to. A source is a pty (local, or a `sakura-core` session); the
//! terminal writes to it from a dedicated thread so a slow or stuck child never blocks the window.

use crate::size::GridSize;
use std::collections::VecDeque;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};

/// A failure reported by a source, with a message meant for people.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceError {
    message: String,
}

impl SourceError {
    /// An error with `message`.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for SourceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for SourceError {}

/// What a source delivers to its terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceOutput<'a> {
    /// Bytes the child wrote, in order.
    Bytes(&'a [u8]),
    /// The child exited, with its exit code when known. Nothing follows.
    Exited(Option<i32>),
}

/// Handed to [`TerminalSource::start`]; the source calls it from any thread for every chunk of output.
#[derive(Clone)]
pub struct OutputSink {
    deliver: Arc<dyn Fn(SourceOutput<'_>) + Send + Sync>,
}

impl OutputSink {
    /// A sink that hands every output to `deliver`.
    pub fn new(deliver: impl Fn(SourceOutput<'_>) + Send + Sync + 'static) -> Self {
        Self {
            deliver: Arc::new(deliver),
        }
    }

    /// Delivers bytes the child wrote.
    pub fn bytes(&self, bytes: &[u8]) {
        (self.deliver)(SourceOutput::Bytes(bytes));
    }

    /// Reports that the child exited.
    pub fn exited(&self, exit_code: Option<i32>) {
        (self.deliver)(SourceOutput::Exited(exit_code));
    }
}

/// A pty a terminal reads from and writes to.
///
/// `write`, `resize` and `close` are called from the terminal's writer thread, never from the UI thread, so
/// they may block.
pub trait TerminalSource: Send + Sync + 'static {
    /// Starts delivering output to `sink`. Called once, before any other method.
    fn start(&self, sink: OutputSink);

    /// Sends input bytes to the child.
    fn write(&self, bytes: &[u8]) -> Result<(), SourceError>;

    /// Tells the child the new grid size.
    fn resize(&self, size: GridSize) -> Result<(), SourceError>;

    /// Hangs up the child. Called once, when the terminal is dropped.
    fn close(&self);
}

/// Most bytes that may wait for the child at once, counting the write in flight. Keeps a child that never
/// reads its input from growing the host's memory.
pub(crate) const MAX_QUEUED_BYTES: usize = 1024 * 1024;

enum Command {
    Write(Vec<u8>),
    Resize(GridSize),
}

/// Input the writer queue had no room for. Nothing of it was queued.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct InputRefused;

#[derive(Default)]
struct Queue {
    commands: VecDeque<Command>,
    queued_bytes: usize,
    is_closed: bool,
}

struct WriterState {
    source: Arc<dyn TerminalSource>,
    queue: Mutex<Queue>,
    ready: Condvar,
    is_writing: AtomicBool,
    is_close_claimed: AtomicBool,
}

impl WriterState {
    /// Waits for the next command; `None` once the writer is closed and what was queued is written.
    fn next_command(&self) -> Option<Command> {
        let mut queue = lock(&self.queue);
        loop {
            if let Some(command) = queue.commands.pop_front() {
                self.is_writing.store(true, Ordering::Release);
                return Some(command);
            }
            if queue.is_closed {
                return None;
            }
            queue = self
                .ready
                .wait(queue)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    fn run(&self, command: Command) {
        let (result, written) = match command {
            Command::Write(bytes) => (self.source.write(&bytes), bytes.len()),
            Command::Resize(size) => (self.source.resize(size), 0),
        };
        if let Err(error) = result {
            log::warn!("terminal source refused a write or resize: {error}");
        }
        let mut queue = lock(&self.queue);
        queue.queued_bytes = queue.queued_bytes.saturating_sub(written);
        self.is_writing.store(false, Ordering::Release);
    }

    /// Closes the source, once, from whichever thread gets here first.
    fn close_source(&self) {
        if !self.is_close_claimed.swap(true, Ordering::AcqRel) {
            self.source.close();
        }
    }

    fn enqueue(&self, bytes: Vec<u8>) -> bool {
        let mut queue = lock(&self.queue);
        let fits = queue.queued_bytes.saturating_add(bytes.len()) <= MAX_QUEUED_BYTES;
        if fits && !queue.is_closed {
            queue.queued_bytes += bytes.len();
            queue.commands.push_back(Command::Write(bytes));
            self.ready.notify_one();
        }
        fits
    }

    fn enqueue_resize(&self, size: GridSize) {
        let mut queue = lock(&self.queue);
        // Only the newest size matters, so a resize waiting behind a stuck write is replaced, not stacked.
        if let Some(Command::Resize(waiting)) = queue.commands.back_mut() {
            *waiting = size;
        } else {
            queue.commands.push_back(Command::Resize(size));
        }
        self.ready.notify_one();
    }
}

/// Runs a source's blocking calls on its own thread, in the order they were requested. Its queue holds at most
/// [`MAX_QUEUED_BYTES`].
pub(crate) struct SourceWriter {
    state: Option<Arc<WriterState>>,
}

impl SourceWriter {
    /// Starts the writer thread for `source`. When the thread cannot start, input is refused and logged.
    pub fn spawn(source: Arc<dyn TerminalSource>) -> Self {
        let state = Arc::new(WriterState {
            source,
            queue: Mutex::default(),
            ready: Condvar::new(),
            is_writing: AtomicBool::new(false),
            is_close_claimed: AtomicBool::new(false),
        });
        let worker = Arc::clone(&state);
        let spawned = std::thread::Builder::new()
            .name("sakura-terminal-writer".into())
            .spawn(move || {
                while let Some(command) = worker.next_command() {
                    worker.run(command);
                }
                worker.close_source();
            });
        match spawned {
            Ok(_) => Self { state: Some(state) },
            Err(error) => {
                log::error!("terminal writer thread did not start: {error}");
                Self { state: None }
            }
        }
    }

    /// Queues what the person typed or pasted. When it does not fit it is refused whole, never in part.
    pub fn write_input(&self, bytes: Vec<u8>) -> Result<(), InputRefused> {
        if bytes.is_empty() {
            return Ok(());
        }
        match &self.state {
            Some(state) if state.enqueue(bytes) => Ok(()),
            _ => {
                log::warn!("terminal input refused: the child is not reading");
                Err(InputRefused)
            }
        }
    }

    /// Queues bytes the terminal itself produces (query replies, mouse and focus reports). They are dropped
    /// when the queue is full: a program that never reads must not make them pile up.
    pub fn write_reply(&self, bytes: Vec<u8>) {
        if bytes.is_empty() {
            return;
        }
        if !self
            .state
            .as_ref()
            .is_some_and(|state| state.enqueue(bytes))
        {
            log::debug!("terminal reply dropped: the child is not reading");
        }
    }

    /// Queues a resize for the child.
    pub fn resize(&self, size: GridSize) {
        if let Some(state) = &self.state {
            state.enqueue_resize(size);
        }
    }

    /// Bytes waiting for the child, the write in flight included.
    #[cfg(test)]
    pub fn queued_bytes(&self) -> usize {
        self.state
            .as_ref()
            .map_or(0, |state| lock(&state.queue).queued_bytes)
    }
}

impl Drop for SourceWriter {
    /// Lets the writer thread finish what is queued and close the source. A write stuck on a child that never
    /// reads would keep it from ever closing, so then the queue is discarded and the source is closed from a
    /// thread of its own, which also frees the stuck write.
    fn drop(&mut self) {
        let Some(state) = self.state.take() else {
            return;
        };
        let is_writing = {
            let mut queue = lock(&state.queue);
            queue.is_closed = true;
            let is_writing = state.is_writing.load(Ordering::Acquire);
            if is_writing {
                queue.commands.clear();
                queue.queued_bytes = 0;
            }
            state.ready.notify_one();
            is_writing
        };
        if is_writing {
            let closer = std::thread::Builder::new()
                .name("sakura-terminal-closer".into())
                .spawn(move || state.close_source());
            if let Err(error) = closer {
                log::error!("terminal closer thread did not start: {error}");
            }
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[derive(Default)]
    struct Recorder {
        log: Mutex<Vec<String>>,
    }

    impl TerminalSource for Recorder {
        fn start(&self, _sink: OutputSink) {}
        fn write(&self, bytes: &[u8]) -> Result<(), SourceError> {
            self.push(format!("write {}", String::from_utf8_lossy(bytes)));
            Ok(())
        }
        fn resize(&self, size: GridSize) -> Result<(), SourceError> {
            self.push(format!("resize {}x{}", size.columns, size.rows));
            Ok(())
        }
        fn close(&self) {
            self.push("close".into());
        }
    }

    impl Recorder {
        fn push(&self, entry: String) {
            self.log.lock().unwrap().push(entry);
        }
    }

    #[test]
    fn writer_keeps_order_and_closes_the_source_when_dropped() {
        let recorder = Arc::new(Recorder::default());
        let writer = SourceWriter::spawn(recorder.clone());
        writer.write_input(b"ls".to_vec()).unwrap();
        writer.resize(GridSize::new(100, 30));
        writer.write_input(b"\r".to_vec()).unwrap();
        drop(writer);
        let deadline = Instant::now() + Duration::from_secs(5);
        while recorder.log.lock().unwrap().len() < 4 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        let log = recorder.log.lock().unwrap().clone();
        assert_eq!(log, ["write ls", "resize 100x30", "write \r", "close"]);
    }

    /// A child that never reads: the first write blocks until the test opens the gate.
    #[derive(Default)]
    struct StuckChild {
        gate: Mutex<bool>,
        opened: Condvar,
        writes: Mutex<Vec<usize>>,
        is_closed: AtomicBool,
    }

    impl StuckChild {
        fn open(&self) {
            *self.gate.lock().unwrap() = true;
            self.opened.notify_all();
        }
    }

    impl TerminalSource for StuckChild {
        fn start(&self, _sink: OutputSink) {}
        fn write(&self, bytes: &[u8]) -> Result<(), SourceError> {
            self.writes.lock().unwrap().push(bytes.len());
            let mut gate = self.gate.lock().unwrap();
            while !*gate {
                gate = self.opened.wait(gate).unwrap();
            }
            Ok(())
        }
        fn resize(&self, _size: GridSize) -> Result<(), SourceError> {
            Ok(())
        }
        fn close(&self) {
            self.is_closed.store(true, Ordering::SeqCst);
            self.open();
        }
    }

    fn wait_until(condition: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !condition() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(condition(), "condition not reached in time");
    }

    #[test]
    fn replies_to_a_child_that_never_reads_stay_within_the_byte_bound() {
        let child = Arc::new(StuckChild::default());
        let writer = SourceWriter::spawn(child.clone());
        let reply = vec![b'r'; 1000];
        for _ in 0..50_000 {
            writer.write_reply(reply.clone());
            assert!(writer.queued_bytes() <= MAX_QUEUED_BYTES);
        }
        assert!(writer.queued_bytes() > MAX_QUEUED_BYTES - reply.len());
        let started = Instant::now();
        drop(writer);
        assert!(started.elapsed() < Duration::from_secs(1));
        wait_until(|| child.is_closed.load(Ordering::SeqCst));
    }

    #[test]
    fn input_that_does_not_fit_is_refused_whole_and_never_written() {
        let child = Arc::new(StuckChild::default());
        let writer = SourceWriter::spawn(child.clone());
        writer.write_reply(vec![b'r'; MAX_QUEUED_BYTES - 10]);
        assert_eq!(writer.write_input(vec![b'p'; 11]), Err(InputRefused));
        assert_eq!(
            writer.write_input(vec![b'p'; MAX_QUEUED_BYTES + 1]),
            Err(InputRefused)
        );
        assert_eq!(writer.write_input(vec![b'k'; 10]), Ok(()));
        child.open();
        wait_until(|| writer.queued_bytes() == 0);
        assert_eq!(*child.writes.lock().unwrap(), [MAX_QUEUED_BYTES - 10, 10]);
    }

    #[test]
    fn a_resize_waiting_behind_a_stuck_write_is_replaced_by_the_newest() {
        let child = Arc::new(StuckChild::default());
        let writer = SourceWriter::spawn(child.clone());
        writer.write_reply(b"x".to_vec());
        wait_until(|| !child.writes.lock().unwrap().is_empty());
        for columns in 80..100 {
            writer.resize(GridSize::new(columns, 24));
        }
        let state = writer.state.as_ref().unwrap();
        assert_eq!(lock(&state.queue).commands.len(), 1);
    }
}
