//! Pseudo-terminals for `box attach`, `box login` and `box shell`. Bytes go to the sink as raw slices in the
//! order they were read; closing a terminal kills only its own child process, by pid, and silences it: the
//! sink hears nothing about a terminal once its `close` returned.

use crate::box_cli::{BoxCli, CHILD_ENV};
use crate::error::{BridgeError, BridgeResult, ErrorCode};
use crate::models::{TerminalExit, TerminalKind, TerminalRequest};
use crate::said::Said;
use crate::validate;
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

const READ_CHUNK_BYTES: usize = 16 * 1024;
const MAX_WRITE_BYTES: usize = 4 * 1024 * 1024;
const TERM_VALUE: &str = "xterm-256color";
const KILL_GRACE: Duration = Duration::from_secs(2);

/// What a terminal reports to its sink.
#[derive(Debug, Clone, PartialEq)]
pub enum TerminalEvent<'a> {
    /// Bytes the child wrote to terminal `id`, in order. At most 16 KiB per event.
    Data { id: &'a str, bytes: &'a [u8] },
    /// The child of a terminal exited. Nothing follows for that id.
    Exit(TerminalExit),
}

/// Receives terminal events, from the thread that reads each terminal. It must not call [`Terminals::close`]
/// or [`Terminals::close_all`]: they wait for a callback in flight to finish.
pub type TerminalSink = Arc<dyn Fn(TerminalEvent<'_>) + Send + Sync>;

struct Session {
    writer: Mutex<Box<dyn Write + Send>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    pid: Option<u32>,
    has_exited: Arc<AtomicBool>,
    /// `true` while the sink may be called for this session; held for the length of each call, so closing
    /// (which sets it to `false`) waits for a callback in flight and no callback follows.
    is_reporting: Arc<Mutex<bool>>,
}

/// A freshly spawned child with its session record and output reader.
struct Spawned {
    session: Session,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    reader: Box<dyn Read + Send>,
    has_exited: Arc<AtomicBool>,
}

type Sessions = Arc<Mutex<HashMap<String, Arc<Session>>>>;

/// All open terminals. Cheap to clone; clones share the same terminals.
#[derive(Clone)]
pub struct Terminals {
    cli: BoxCli,
    cwd: std::path::PathBuf,
    sink: TerminalSink,
    sessions: Sessions,
    next_id: Arc<AtomicU64>,
}

impl Terminals {
    /// Terminals run `cli` with `cwd` as working directory and report to `sink`.
    pub fn new(cli: BoxCli, cwd: std::path::PathBuf, sink: TerminalSink) -> Self {
        Self {
            cli,
            cwd,
            sink,
            sessions: Arc::default(),
            next_id: Arc::new(AtomicU64::new(1)),
        }
    }

    /// Spawns the terminal described by `request` and returns its id.
    pub fn open(&self, request: &TerminalRequest) -> BridgeResult<String> {
        let args = terminal_args(request)?;
        let (cols, rows) = validate::terminal_size(request.cols, request.rows)?;
        self.cli.check()?;
        let spawned = self.spawn(&args, size(cols, rows))?;
        let id = format!("t{}", self.next_id.fetch_add(1, Ordering::Relaxed));
        let pump = Pump {
            id: id.clone(),
            sink: Arc::clone(&self.sink),
            sessions: Arc::clone(&self.sessions),
            is_reporting: Arc::clone(&spawned.session.is_reporting),
        };
        lock(&self.sessions).insert(id.clone(), Arc::new(spawned.session));
        let (mut child, reader, has_exited) = (spawned.child, spawned.reader, spawned.has_exited);
        std::thread::spawn(move || {
            pump.run(reader);
            let code = child
                .wait()
                .ok()
                .map(|status| i32::try_from(status.exit_code()).unwrap_or(i32::MAX));
            has_exited.store(true, Ordering::SeqCst);
            pump.finish(code);
        });
        Ok(id)
    }

    /// Sends input bytes to the terminal.
    pub fn write(&self, id: &str, bytes: &[u8]) -> BridgeResult<()> {
        if bytes.len() > MAX_WRITE_BYTES {
            return Err(BridgeError::invalid(Said::new(
                "error.terminal.too_much_data",
            )));
        }
        let session = self.session(id)?;
        let mut writer = lock(&session.writer);
        writer
            .write_all(bytes)
            .and_then(|()| writer.flush())
            .map_err(|error| {
                BridgeError::new(
                    ErrorCode::TerminalClosed,
                    Said::new("error.terminal.not_answering"),
                )
                .with_details(Some(error.to_string()))
            })
    }

    /// Resizes the terminal.
    pub fn resize(&self, id: &str, cols: u16, rows: u16) -> BridgeResult<()> {
        let (cols, rows) = validate::terminal_size(cols, rows)?;
        let session = self.session(id)?;
        let master = lock(&session.master);
        master.resize(size(cols, rows)).map_err(pty_error)
    }

    /// Closes one terminal: hangs up its child by pid and forgets the session. When this returns the sink will
    /// not hear about the terminal again, not even its exit. Unknown ids are an error.
    pub fn close(&self, id: &str) -> BridgeResult<()> {
        let session = lock(&self.sessions)
            .remove(id)
            .ok_or_else(|| closed_error(id))?;
        stop(&session);
        Ok(())
    }

    /// Closes every terminal. Called when the app quits.
    pub fn close_all(&self) {
        let sessions: Vec<Arc<Session>> = lock(&self.sessions)
            .drain()
            .map(|(_, session)| session)
            .collect();
        for session in sessions {
            stop(&session);
        }
    }

    fn spawn(&self, args: &[String], pty_size: PtySize) -> BridgeResult<Spawned> {
        let pair = native_pty_system().openpty(pty_size).map_err(pty_error)?;
        let child = pair
            .slave
            .spawn_command(self.command(args))
            .map_err(|error| {
                BridgeError::internal(Said::new("error.terminal.start_failed"))
                    .with_details(Some(error.to_string()))
            })?;
        drop(pair.slave);
        let reader = pair.master.try_clone_reader().map_err(pty_error)?;
        let writer = pair.master.take_writer().map_err(pty_error)?;
        let has_exited = Arc::new(AtomicBool::new(false));
        let session = Session {
            writer: Mutex::new(writer),
            master: Mutex::new(pair.master),
            killer: Mutex::new(child.clone_killer()),
            pid: child.process_id(),
            has_exited: Arc::clone(&has_exited),
            is_reporting: Arc::new(Mutex::new(true)),
        };
        Ok(Spawned {
            session,
            child,
            reader,
            has_exited,
        })
    }

    fn session(&self, id: &str) -> BridgeResult<Arc<Session>> {
        lock(&self.sessions)
            .get(id)
            .cloned()
            .ok_or_else(|| closed_error(id))
    }

    fn command(&self, args: &[String]) -> CommandBuilder {
        let launch = crate::platform::Launch::of(self.cli.bin());
        let mut command = CommandBuilder::new(&launch.program);
        command.args(&launch.leading);
        command.args(args);
        for (name, value) in CHILD_ENV {
            command.env(name, value);
        }
        command.env("TERM", TERM_VALUE);
        command.cwd(&self.cwd);
        command
    }
}

struct Pump {
    id: String,
    sink: TerminalSink,
    sessions: Sessions,
    is_reporting: Arc<Mutex<bool>>,
}

impl Pump {
    fn run(&self, mut reader: Box<dyn Read + Send>) {
        let mut chunk = vec![0u8; READ_CHUNK_BYTES];
        while let Ok(read) = reader.read(&mut chunk) {
            if read == 0 {
                break;
            }
            let is_reporting = lock(&self.is_reporting);
            if !*is_reporting {
                break;
            }
            (self.sink)(TerminalEvent::Data {
                id: &self.id,
                bytes: &chunk[..read],
            });
        }
    }

    fn finish(&self, exit_code: Option<i32>) {
        lock(&self.sessions).remove(&self.id);
        let is_reporting = lock(&self.is_reporting);
        if *is_reporting {
            (self.sink)(TerminalEvent::Exit(TerminalExit {
                id: self.id.clone(),
                exit_code,
            }));
        }
    }
}

/// Hangs up the child, then makes sure of it: SIGKILL after a grace period unless it already exited.
fn stop(session: &Arc<Session>) {
    *lock(&session.is_reporting) = false;
    let _ = lock(&session.killer).kill();
    let session = Arc::clone(session);
    std::thread::spawn(move || {
        std::thread::sleep(KILL_GRACE);
        let Some(pid) = session.pid else {
            return;
        };
        // `pid` is the child this session spawned and it has not been reaped yet (the flag is set right
        // after `wait` returns).
        if !session.has_exited.load(Ordering::SeqCst) {
            crate::platform::kill_process(pid);
        }
    });
}

fn terminal_args(request: &TerminalRequest) -> BridgeResult<Vec<String>> {
    let required = |value: &Option<String>, missing: &'static str| {
        let value = value.clone();
        value.ok_or_else(|| BridgeError::invalid(Said::new(missing)))
    };
    let args = match request.kind {
        TerminalKind::Agent => [
            "attach".to_string(),
            validate::task(&required(&request.task, "error.terminal.missing_task")?)?.to_string(),
        ],
        TerminalKind::Shell => [
            "shell".to_string(),
            validate::task(&required(&request.task, "error.terminal.missing_task")?)?.to_string(),
        ],
        TerminalKind::Login => [
            "login".to_string(),
            validate::account(&required(
                &request.account,
                "error.terminal.missing_account",
            )?)?
            .to_string(),
        ],
    };
    Ok(args.to_vec())
}

fn size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    }
}

fn pty_error(error: impl std::fmt::Display) -> BridgeError {
    BridgeError::internal(Said::new("error.terminal.unavailable"))
        .with_details(Some(error.to_string()))
}

fn closed_error(id: &str) -> BridgeError {
    BridgeError::new(
        ErrorCode::TerminalClosed,
        Said::new("error.terminal.closed"),
    )
    .with_details(Some(id.to_string()))
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::box_cli::test_support::fake_box;
    use std::sync::mpsc::{channel, Receiver};
    use std::time::Instant;

    /// How long a test waits for output or an exit before it calls it a hang: these tests start real
    /// processes on a machine that may be running every other test.
    const WAIT: Duration = Duration::from_secs(120);

    /// A [`TerminalEvent`] the test keeps beyond the callback.
    #[derive(Debug)]
    enum Heard {
        Data { id: String, bytes: Vec<u8> },
        Exit(TerminalExit),
    }

    fn owned_sink(sender: Mutex<std::sync::mpsc::Sender<Heard>>) -> TerminalSink {
        Arc::new(move |event| {
            let heard = match event {
                TerminalEvent::Data { id, bytes } => Heard::Data {
                    id: id.to_string(),
                    bytes: bytes.to_vec(),
                },
                TerminalEvent::Exit(exit) => Heard::Exit(exit),
            };
            let _ = lock(&sender).send(heard);
        })
    }

    struct Rig {
        dir: tempfile::TempDir,
        terminals: Terminals,
        events: Receiver<Heard>,
    }

    fn rig(body: &str) -> Rig {
        let dir = tempfile::tempdir().unwrap();
        let cli = fake_box(dir.path(), body);
        let (sender, events) = channel();
        let sink = owned_sink(Mutex::new(sender));
        let terminals = Terminals::new(cli, dir.path().to_path_buf(), sink);
        Rig {
            dir,
            terminals,
            events,
        }
    }

    fn request(kind: TerminalKind, task: Option<&str>, account: Option<&str>) -> TerminalRequest {
        TerminalRequest {
            kind,
            task: task.map(str::to_string),
            account: account.map(str::to_string),
            cols: 100,
            rows: 30,
        }
    }

    /// Collects output of terminal `id` until `needle` appears or the terminal exits.
    fn read_until(rig: &Rig, id: &str, needle: &str) -> (String, Option<TerminalExit>) {
        let (mut text, deadline) = (String::new(), Instant::now() + WAIT);
        while Instant::now() < deadline {
            match rig.events.recv_timeout(Duration::from_millis(200)) {
                Ok(Heard::Data { id: from, bytes }) if from == id => {
                    text.push_str(&String::from_utf8_lossy(&bytes));
                    if text.contains(needle) {
                        return (text, None);
                    }
                }
                Ok(Heard::Exit(exit)) if exit.id == id => return (text, Some(exit)),
                _ => {}
            }
        }
        (text, None)
    }

    fn is_alive(pid: i32) -> bool {
        // SAFETY: signal 0 only checks that the process exists.
        unsafe { libc::kill(pid, 0) == 0 }
    }

    fn wait_dead(pid: i32) -> bool {
        let deadline = Instant::now() + WAIT;
        while Instant::now() < deadline {
            if !is_alive(pid) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }

    #[test]
    fn echoes_input_back_and_close_kills_only_that_child() {
        let rig = rig(r#"echo $$ > "$(dirname "$0")/pid-$2"; exec cat"#);
        let first = rig
            .terminals
            .open(&request(TerminalKind::Agent, Some("m1"), None))
            .unwrap();
        let second = rig
            .terminals
            .open(&request(TerminalKind::Shell, Some("m2"), None))
            .unwrap();
        rig.terminals.write(&first, b"hello\n").unwrap();
        let (text, exit) = read_until(&rig, &first, "hello\r\nhello");
        assert!(text.contains("hello"), "{text:?}");
        assert!(exit.is_none());
        let read_pid = |task: &str| -> i32 {
            std::fs::read_to_string(rig.dir.path().join(format!("pid-{task}")))
                .unwrap()
                .trim()
                .parse()
                .unwrap()
        };
        let pids = [read_pid("m1"), read_pid("m2")];
        rig.terminals.close(&first).unwrap();
        assert!(wait_dead(pids[0]), "closed child still alive");
        assert!(is_alive(pids[1]), "the other terminal's child was killed");
        rig.terminals.write(&second, b"still\n").unwrap();
        assert!(read_until(&rig, &second, "still").0.contains("still"));
        rig.terminals.close_all();
        assert!(wait_dead(pids[1]));
    }

    #[test]
    fn child_that_prints_and_exits_reports_output_then_exit_code() {
        let rig = rig("echo bye\nexit 3");
        let id = rig
            .terminals
            .open(&request(TerminalKind::Login, None, Some("personal")))
            .unwrap();
        let (text, exit) = read_until(&rig, &id, "never appears");
        assert!(text.contains("bye"));
        assert_eq!(
            exit,
            Some(TerminalExit {
                id: id.clone(),
                exit_code: Some(3)
            })
        );
        let error = rig.terminals.write(&id, b"x").unwrap_err();
        assert_eq!(error.code, ErrorCode::TerminalClosed);
        assert_eq!(
            rig.terminals.close(&id).unwrap_err().code,
            ErrorCode::TerminalClosed
        );
    }

    #[test]
    fn a_terminal_s_box_command_says_it_comes_from_sakura() {
        let body = r#"echo "channel=$BOX_CHANNEL" >> "$(dirname "$0")/args""#;
        let rig = rig(body);
        let id = rig
            .terminals
            .open(&request(TerminalKind::Shell, Some("m1"), None))
            .unwrap();
        read_until(&rig, &id, "never appears");
        let args = std::fs::read_to_string(rig.dir.path().join("args")).unwrap();
        assert_eq!(args.trim(), "channel=sakura");
    }

    #[test]
    fn each_kind_runs_the_matching_box_command_with_term_and_path() {
        let body = r#"echo "$@ TERM=$TERM PATH=$PATH" >> "$(dirname "$0")/args""#;
        let rig = rig(body);
        for req in [
            request(TerminalKind::Agent, Some("m1"), None),
            request(TerminalKind::Shell, Some("m2"), None),
            request(TerminalKind::Login, None, Some("personal")),
        ] {
            let id = rig.terminals.open(&req).unwrap();
            read_until(&rig, &id, "never appears");
        }
        let args = std::fs::read_to_string(rig.dir.path().join("args")).unwrap();
        let mut lines: Vec<&str> = args.lines().collect();
        lines.sort_unstable();
        let tail = format!("TERM=xterm-256color PATH={}", crate::box_cli::CHILD_PATH);
        assert_eq!(
            lines,
            vec![
                format!("attach m1 {tail}"),
                format!("login personal {tail}"),
                format!("shell m2 {tail}"),
            ]
        );
    }

    #[test]
    fn invalid_requests_never_start_a_process() {
        let rig = rig(r#"touch "$(dirname "$0")/ran""#);
        let bad = [
            request(TerminalKind::Agent, None, None),
            request(TerminalKind::Shell, Some("A b"), None),
            request(TerminalKind::Login, None, Some("-x")),
            request(TerminalKind::Login, Some("m1"), None),
            TerminalRequest {
                cols: 0,
                ..request(TerminalKind::Agent, Some("m1"), None)
            },
        ];
        for req in bad {
            assert_eq!(
                rig.terminals.open(&req).unwrap_err().code,
                ErrorCode::InvalidArgument
            );
        }
        std::thread::sleep(Duration::from_millis(300));
        assert!(!rig.dir.path().join("ran").exists());
    }

    #[test]
    fn resize_reaches_the_child() {
        let rig = rig(r#"read x; stty size; exec cat"#);
        let id = rig
            .terminals
            .open(&request(TerminalKind::Agent, Some("m1"), None))
            .unwrap();
        rig.terminals.resize(&id, 132, 43).unwrap();
        rig.terminals.write(&id, b"\n").unwrap();
        assert!(read_until(&rig, &id, "43 132").0.contains("43 132"));
        assert!(rig.terminals.resize(&id, 0, 5).is_err());
        rig.terminals.close_all();
    }

    #[test]
    fn oversized_writes_and_unknown_ids_are_rejected() {
        let rig = rig("exec cat");
        let id = rig
            .terminals
            .open(&request(TerminalKind::Agent, Some("m1"), None))
            .unwrap();
        let too_much = vec![b'x'; MAX_WRITE_BYTES + 1];
        assert_eq!(
            rig.terminals.write(&id, &too_much).unwrap_err().code,
            ErrorCode::InvalidArgument
        );
        assert_eq!(
            rig.terminals.write("t999", b"").unwrap_err().code,
            ErrorCode::TerminalClosed
        );
        rig.terminals.close_all();
    }

    #[test]
    fn nothing_is_heard_about_a_terminal_after_close_returns() {
        let rig = rig(r#"while :; do printf 'xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\n'; done"#);
        let id = rig
            .terminals
            .open(&request(TerminalKind::Agent, Some("m1"), None))
            .unwrap();
        assert!(read_until(&rig, &id, "xxxx").0.contains("xxxx"));
        rig.terminals.close(&id).unwrap();
        while rig.events.try_recv().is_ok() {}
        std::thread::sleep(Duration::from_secs(3));
        let late: Vec<Heard> = rig.events.try_iter().collect();
        assert!(late.is_empty(), "heard after close: {} events", late.len());
    }

    #[test]
    fn large_output_arrives_complete_and_in_order_in_chunks_up_to_16k() {
        let rig = rig("i=0; while [ $i -lt 4000 ]; do printf 'line-%05d\\n' $i; i=$((i+1)); done");
        let id = rig
            .terminals
            .open(&request(TerminalKind::Agent, Some("m1"), None))
            .unwrap();
        assert!(id.starts_with('t'));
        let (mut text, deadline) = (String::new(), Instant::now() + WAIT);
        let mut largest = 0;
        while Instant::now() < deadline {
            match rig.events.recv_timeout(Duration::from_millis(200)) {
                Ok(Heard::Data { bytes, .. }) => {
                    largest = largest.max(bytes.len());
                    text.push_str(&String::from_utf8_lossy(&bytes));
                }
                Ok(Heard::Exit(_)) => break,
                Err(_) => {}
            }
        }
        let expected: String = (0..4000).map(|i| format!("line-{i:05}\r\n")).collect();
        assert_eq!(text, expected);
        assert!(largest <= READ_CHUNK_BYTES);
    }

    #[test]
    fn missing_box_is_reported_before_any_pty_is_made() {
        let (sender, _events) = channel();
        let sink = owned_sink(Mutex::new(sender));
        let terminals = Terminals::new(BoxCli::at("/nonexistent/box"), "/".into(), sink);
        let error = terminals
            .open(&request(TerminalKind::Agent, Some("m1"), None))
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::BoxMissing);
    }
}
