//! The terminal model as a GPUI entity: it owns the emulator, parses output on the source's reader thread and
//! wakes the window at most once per batch of output, so a flood costs one repaint per frame.

use crate::core::{Advance, TermCore};
use crate::palette::TerminalPalette;
use crate::size::GridSize;
use crate::source::{OutputSink, SourceOutput, SourceWriter, TerminalSource};
use alacritty_terminal::sync::FairMutex;
use gpui::{Context, EventEmitter, Task};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, Instant};

/// How long a resize waits for the next one before the child is told (window drags send many).
const RESIZE_DEBOUNCE: Duration = Duration::from_millis(50);

/// What the terminal reports to the app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalChange {
    /// The program set (`Some`) or reset (`None`) the title with OSC 0 or 2.
    TitleChanged(Option<String>),
    /// The program rang the bell.
    Bell,
    /// The child exited, with its exit code when known.
    Exited(Option<i32>),
}

/// Events noted on the reader thread, handed to the window on the next wake.
#[derive(Default)]
struct Noted {
    title: Option<Option<String>>,
    bell: bool,
    exit: Option<Option<i32>>,
}

/// State shared between the window and the source's reader thread.
pub(crate) struct Shared {
    core: FairMutex<TermCore>,
    writer: SourceWriter,
    wake: async_channel::Sender<()>,
    is_wake_pending: AtomicBool,
    noted: Mutex<Noted>,
}

impl Shared {
    /// Locks the emulator. Held briefly: the reader thread parses one chunk per lock.
    pub fn lock(&self) -> impl std::ops::DerefMut<Target = TermCore> + '_ {
        self.core.lock()
    }

    /// Bytes waiting for the child.
    #[cfg(all(test, feature = "test-support"))]
    pub fn queued_bytes(&self) -> usize {
        self.writer.queued_bytes()
    }

    fn deliver(&self, output: SourceOutput<'_>) {
        match output {
            SourceOutput::Bytes(bytes) => {
                let advance = self.core.lock().advance(bytes);
                self.after_advance(advance);
            }
            SourceOutput::Exited(code) => {
                lock(&self.noted).exit = Some(code);
                self.wake();
            }
        }
    }

    fn after_advance(&self, advance: Advance) {
        self.writer.write_reply(advance.reply);
        if advance.title.is_some() || advance.bell {
            let mut noted = lock(&self.noted);
            if advance.title.is_some() {
                noted.title = advance.title;
            }
            noted.bell |= advance.bell;
        }
        self.wake();
    }

    /// Wakes the window unless a wake is already on its way.
    fn wake(&self) {
        if !self.is_wake_pending.swap(true, Ordering::AcqRel) {
            // The receiver lives as long as the terminal entity; after it is gone nobody needs the wake.
            let _ = self.wake.try_send(());
        }
    }
}

/// A terminal: the emulator fed by a [`TerminalSource`]. Create it with [`Terminal::new`] inside
/// `cx.new`, then show it in a [`crate::TerminalView`].
pub struct Terminal {
    shared: Arc<Shared>,
    title: Option<String>,
    exit: Option<Option<i32>>,
    resize_task: Option<Task<()>>,
    sync_task: Option<Task<()>>,
    _wake_task: Task<()>,
}

impl EventEmitter<TerminalChange> for Terminal {}

impl Terminal {
    /// Starts showing `source`. The grid starts at 80x24 and takes the view's size on its first layout.
    pub fn new(
        source: impl TerminalSource,
        palette: TerminalPalette,
        cx: &mut Context<Self>,
    ) -> Self {
        let source: Arc<dyn TerminalSource> = Arc::new(source);
        let (wake, woken) = async_channel::bounded(1);
        let shared = Arc::new(Shared {
            core: FairMutex::new(TermCore::new(GridSize::default(), palette)),
            writer: SourceWriter::spawn(Arc::clone(&source)),
            wake,
            is_wake_pending: AtomicBool::new(false),
            noted: Mutex::default(),
        });
        let weak: Weak<Shared> = Arc::downgrade(&shared);
        source.start(OutputSink::new(move |output| {
            if let Some(shared) = weak.upgrade() {
                shared.deliver(output);
            }
        }));
        let wake_task = Self::spawn_wake_loop(woken, cx);
        Self {
            shared,
            title: None,
            exit: None,
            resize_task: None,
            sync_task: None,
            _wake_task: wake_task,
        }
    }

    /// Runs on the window's thread: each wake from the reader thread becomes one `on_wake`.
    fn spawn_wake_loop(woken: async_channel::Receiver<()>, cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |this, cx| {
            while woken.recv().await.is_ok() {
                if this
                    .update(cx, |terminal, cx| terminal.on_wake(cx))
                    .is_err()
                {
                    break;
                }
            }
        })
    }

    /// The title the program set, if any.
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// The last line of text on the screen, if any: the message of a command that failed at once.
    pub fn last_line(&self) -> Option<String> {
        self.shared.lock().last_line()
    }

    /// The lines on screen as the program printed them, rows the terminal wrapped joined into one.
    pub fn logical_lines(&self) -> Vec<String> {
        self.shared.lock().logical_lines()
    }

    /// `Some` once the child exited, with its exit code when known.
    pub fn exit_status(&self) -> Option<Option<i32>> {
        self.exit
    }

    /// Sends bytes to the child as if typed, and brings the view back to the live screen. When the child is
    /// not reading and its input queue is full, the bytes are refused whole and the terminal rings the bell.
    pub fn input(&mut self, bytes: Vec<u8>, cx: &mut Context<Self>) {
        let is_scrolled = {
            let mut core = self.shared.lock();
            let is_scrolled = core.display_offset() != 0;
            core.scroll_to_bottom();
            is_scrolled
        };
        if self.shared.writer.write_input(bytes).is_err() {
            cx.emit(TerminalChange::Bell);
        }
        if is_scrolled {
            cx.notify();
        }
    }

    /// Changes the colours; every row is drawn again.
    pub fn set_palette(&mut self, palette: TerminalPalette, cx: &mut Context<Self>) {
        self.shared.lock().set_palette(palette);
        cx.notify();
    }

    pub(crate) fn shared(&self) -> &Arc<Shared> {
        &self.shared
    }

    /// Resizes the grid now and tells the child once resizing pauses. Does not notify: it runs during layout.
    pub(crate) fn resize(&mut self, size: GridSize, cx: &mut Context<Self>) {
        self.shared.lock().resize(size);
        let shared = Arc::clone(&self.shared);
        let timer = cx.background_executor().timer(RESIZE_DEBOUNCE);
        self.resize_task = Some(cx.spawn(async move |_, _| {
            timer.await;
            shared.writer.resize(size);
        }));
    }

    /// Writes bytes the terminal itself produces (mouse and focus reports): they never scroll the view.
    pub(crate) fn report(&self, bytes: Vec<u8>) {
        self.shared.writer.write_reply(bytes);
    }

    fn on_wake(&mut self, cx: &mut Context<Self>) {
        self.shared.is_wake_pending.store(false, Ordering::Release);
        let noted = std::mem::take(&mut *lock(&self.shared.noted));
        if let Some(title) = noted.title {
            self.title.clone_from(&title);
            cx.emit(TerminalChange::TitleChanged(title));
        }
        if noted.bell {
            cx.emit(TerminalChange::Bell);
        }
        if let Some(code) = noted.exit {
            self.exit = Some(code);
            cx.emit(TerminalChange::Exited(code));
        }
        self.schedule_sync_flush(cx);
        cx.notify();
    }

    /// A synchronized update that never ends is shown anyway once its deadline passes.
    fn schedule_sync_flush(&mut self, cx: &mut Context<Self>) {
        let Some(deadline) = self.shared.lock().sync_deadline() else {
            self.sync_task = None;
            return;
        };
        let timer = cx
            .background_executor()
            .timer(deadline.saturating_duration_since(Instant::now()));
        let shared = Arc::clone(&self.shared);
        self.sync_task = Some(cx.spawn(async move |_, _| {
            timer.await;
            let advance = shared.lock().flush_expired_sync();
            shared.after_advance(advance);
        }));
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
