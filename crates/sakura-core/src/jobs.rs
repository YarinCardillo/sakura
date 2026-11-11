//! Tracked background jobs. `box new` can take minutes (install), so it runs detached from the call that
//! started it. The job's last output line and its final outcome stay readable until the workspace shows up
//! in `box status` or the failure is dismissed.

use crate::box_cli::{failure, kill_group, sanitize_line, BoxCli};
use crate::box_words::{SAKURA_BOX_MISSING, SAKURA_BOX_UNUSABLE, SAKURA_PREFIX, SAKURA_TIMED_OUT};
use crate::models::Preparing;
use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;
use std::process::Stdio;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt};

const LINE_BUFFER_BYTES: usize = 4096;
const STDERR_LINES_KEPT: usize = 20;
const READ_CHUNK_BYTES: usize = 4096;
/// Shortest time between two notifications for output lines.
const NOTIFY_INTERVAL: Duration = Duration::from_millis(250);

/// Called when a job's state changes.
pub type JobNotifier = Arc<dyn Fn() + Send + Sync>;

#[derive(Debug, Clone)]
struct Job {
    profile: String,
    line: String,
    failed: bool,
    is_running: bool,
}

/// A job whose workspace `box status` does not list yet.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingJob {
    pub name: String,
    pub profile: String,
    pub preparing: Preparing,
}

/// Registry of `new` jobs, keyed by task name. Cheap to clone; clones share state.
#[derive(Debug, Clone, Default)]
pub struct Jobs {
    inner: Arc<Mutex<HashMap<String, Job>>>,
}

impl Jobs {
    /// Starts tracking `name`. Returns false, changing nothing, when a job for it is already running.
    pub fn begin(&self, name: &str, profile: &str) -> bool {
        let mut jobs = self.lock();
        if jobs.get(name).is_some_and(|job| job.is_running) {
            return false;
        }
        let job = Job {
            profile: profile.to_string(),
            line: String::new(),
            failed: false,
            is_running: true,
        };
        jobs.insert(name.to_string(), job);
        true
    }

    /// Progress of `name`, if a job is tracked for it.
    pub fn preparing(&self, name: &str) -> Option<Preparing> {
        self.lock().get(name).map(Job::preparing)
    }

    /// Jobs whose task is absent from `listed`, sorted by name.
    pub fn pending_outside(&self, listed: &HashSet<&str>) -> Vec<PendingJob> {
        let jobs = self.lock();
        let mut pending: Vec<PendingJob> = jobs
            .iter()
            .filter(|(name, _)| !listed.contains(name.as_str()))
            .map(|(name, job)| PendingJob {
                name: name.clone(),
                profile: job.profile.clone(),
                preparing: job.preparing(),
            })
            .collect();
        pending.sort_by(|a, b| a.name.cmp(&b.name));
        pending
    }

    /// The job tracked for `name`, with its profile.
    pub fn pending(&self, name: &str) -> Option<PendingJob> {
        self.lock().get(name).map(|job| PendingJob {
            name: name.to_string(),
            profile: job.profile.clone(),
            preparing: job.preparing(),
        })
    }

    /// Forgets a finished failed job. Returns whether one was removed. A running job is kept.
    pub fn dismiss_failed(&self, name: &str) -> bool {
        let mut jobs = self.lock();
        if jobs.get(name).is_some_and(|job| job.failed) {
            jobs.remove(name);
            return true;
        }
        false
    }

    fn set_line(&self, name: &str, line: &str) {
        if let Some(job) = self.lock().get_mut(name) {
            job.line = line.to_string();
        }
    }

    fn succeed(&self, name: &str) {
        self.lock().remove(name);
    }

    fn fail(&self, name: &str, message: &str) {
        if let Some(job) = self.lock().get_mut(name) {
            job.line = message.to_string();
            job.failed = true;
            job.is_running = false;
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Job>> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Job {
    fn preparing(&self) -> Preparing {
        Preparing {
            line: self.line.clone(),
            failed: self.failed,
            has_workspace: false,
        }
    }
}

/// A lavoro to create: its name, its project, and the arguments of `box` that create it (checked by
/// whoever built them).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewJob {
    pub name: String,
    pub project: String,
    pub arguments: Vec<String>,
}

/// Runs `box new` for `job` as a tracked job. Returns at once; false when a job for the same name is
/// already running. `notify` fires at most every 250 ms while output arrives, and always once on the final
/// outcome.
pub fn start_new(cli: BoxCli, jobs: Jobs, job: NewJob, notify: JobNotifier) -> bool {
    if !jobs.begin(&job.name, &job.project) {
        return false;
    }
    tokio::spawn(async move {
        let outcome = run_new(&cli, &jobs, &job, &notify).await;
        match outcome {
            Ok(()) => jobs.succeed(&job.name),
            Err(message) => jobs.fail(&job.name, &message),
        }
        notify();
    });
    true
}

async fn run_new(
    cli: &BoxCli,
    jobs: &Jobs,
    job: &NewJob,
    notify: &JobNotifier,
) -> Result<(), String> {
    let name = job.name.as_str();
    let arguments: Vec<&str> = job.arguments.iter().map(String::as_str).collect();
    // A reason of Sakura's own is written as a line with its prefix: a job's last line is always data, and
    // `box_words` tells what to say of it.
    cli.check()
        .map_err(|_| format!("{SAKURA_PREFIX}{SAKURA_BOX_MISSING}"))?;
    let mut child = cli
        .command(&arguments)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("{SAKURA_PREFIX}{SAKURA_BOX_UNUSABLE}{error}"))?;
    let pid = child.id();
    let finished = async {
        let stderr_tail = stream_output(&mut child, jobs, name, notify).await;
        (stderr_tail, child.wait().await)
    };
    let Ok((stderr_tail, status)) = tokio::time::timeout(cli.new_limit(), finished).await else {
        kill_group(pid);
        let seconds = cli.new_limit().as_secs();
        return Err(format!("{SAKURA_PREFIX}{SAKURA_TIMED_OUT}{seconds} s"));
    };
    let status = status.map_err(|error| format!("{SAKURA_PREFIX}{SAKURA_BOX_UNUSABLE}{error}"))?;
    if status.success() {
        return Ok(());
    }
    let text = stderr_tail.into_iter().collect::<Vec<_>>().join("\n");
    // A creation's last line is kept as it was printed: the row shows Sakura's sentence for it and the line
    // itself under "Dettagli" (`box_words`).
    let failed = failure(status.code(), text.as_bytes());
    // With no line at all, what is said of the exit is said by the row itself: the line is empty.
    Err(failed.details.unwrap_or_default())
}

/// Feeds the child's output to the job and returns the last stderr lines. The newest line is kept as it
/// arrives but published, with one notification, at most every [`NOTIFY_INTERVAL`], so a child printing
/// endlessly costs a few notifications a second.
async fn stream_output(
    child: &mut tokio::process::Child,
    jobs: &Jobs,
    name: &str,
    notify: &JobNotifier,
) -> VecDeque<String> {
    let newest: Mutex<Option<String>> = Mutex::new(None);
    let mut stderr_tail = VecDeque::new();
    let mut record = |line: String| *lock(&newest) = Some(line);
    let mut remember = |line: String| {
        *lock(&newest) = Some(line.clone());
        stderr_tail.push_back(line);
        if stderr_tail.len() > STDERR_LINES_KEPT {
            stderr_tail.pop_front();
        }
    };
    let streaming = async {
        tokio::join!(
            pump_lines(child.stdout.take(), &mut record),
            pump_lines(child.stderr.take(), &mut remember)
        );
    };
    publish_while(streaming, jobs, name, &newest, notify).await;
    // The last lines may have arrived after the final tick; the caller notifies once the outcome is known.
    if let Some(line) = lock(&newest).take() {
        jobs.set_line(name, &line);
    }
    stderr_tail
}

/// Runs `streaming`, publishing the newest line every [`NOTIFY_INTERVAL`] meanwhile.
async fn publish_while(
    streaming: impl Future<Output = ()>,
    jobs: &Jobs,
    name: &str,
    newest: &Mutex<Option<String>>,
    notify: &JobNotifier,
) {
    tokio::pin!(streaming);
    let mut ticker = tokio::time::interval(NOTIFY_INTERVAL);
    loop {
        tokio::select! {
            () = &mut streaming => return,
            _ = ticker.tick() => publish_newest(jobs, name, newest, notify),
        }
    }
}

/// Moves the newest line, if there is one, to the job and tells the notifier.
fn publish_newest(jobs: &Jobs, name: &str, newest: &Mutex<Option<String>>, notify: &JobNotifier) {
    let line = lock(newest).take();
    if let Some(line) = line {
        jobs.set_line(name, &line);
        notify();
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Splits a byte stream into lines on `\n` and `\r` (progress bars rewrite lines) and reports each
/// non-blank one, sanitised. Memory stays bounded however long a line is.
async fn pump_lines(reader: Option<impl AsyncRead + Unpin>, on_line: &mut impl FnMut(String)) {
    let Some(mut reader) = reader else { return };
    let mut chunk = [0u8; READ_CHUNK_BYTES];
    let mut pending: Vec<u8> = Vec::new();
    while let Ok(read) = reader.read(&mut chunk).await {
        if read == 0 {
            break;
        }
        for &byte in &chunk[..read] {
            if byte == b'\n' || byte == b'\r' {
                flush_line(&mut pending, on_line);
            } else if pending.len() < LINE_BUFFER_BYTES {
                pending.push(byte);
            }
        }
    }
    flush_line(&mut pending, on_line);
}

fn flush_line(pending: &mut Vec<u8>, on_line: &mut impl FnMut(String)) {
    let line = sanitize_line(String::from_utf8_lossy(pending).trim());
    pending.clear();
    if !line.is_empty() {
        on_line(line);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The job of `box new <name> <project>` with nothing else said.
    fn plain(name: &str, project: &str) -> NewJob {
        NewJob {
            name: name.into(),
            project: project.into(),
            arguments: vec!["new".into(), name.into(), project.into()],
        }
    }
    use crate::box_cli::test_support::fake_box;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Instant;

    fn silent() -> JobNotifier {
        Arc::new(|| {})
    }

    /// How long a test waits for a state before it calls it a hang.
    const HANG: Duration = Duration::from_secs(120);

    /// Waits until the job of `name` has ended (gone, or failed), and gives what is left of it.
    async fn settled(jobs: &Jobs, name: &str) -> Option<Preparing> {
        let deadline = Instant::now() + HANG;
        while Instant::now() < deadline {
            if jobs.preparing(name).is_none_or(|p| p.failed) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        jobs.preparing(name)
    }

    #[test]
    fn begin_is_refused_while_a_job_runs_and_allowed_after_a_failure() {
        let jobs = Jobs::default();
        assert!(jobs.begin("a", "p"));
        assert!(!jobs.begin("a", "p"));
        jobs.fail("a", "boom");
        assert!(jobs.begin("a", "p"));
        assert!(!jobs.preparing("a").unwrap().failed);
    }

    #[test]
    fn pending_outside_lists_only_unlisted_jobs() {
        let jobs = Jobs::default();
        jobs.begin("b", "p1");
        jobs.begin("a", "p2");
        let listed = HashSet::from(["b"]);
        let pending = jobs.pending_outside(&listed);
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].name, "a");
        assert_eq!(pending[0].profile, "p2");
    }

    #[test]
    fn dismiss_failed_keeps_running_jobs() {
        let jobs = Jobs::default();
        jobs.begin("a", "p");
        assert!(!jobs.dismiss_failed("a"));
        jobs.fail("a", "x");
        assert!(jobs.dismiss_failed("a"));
        assert!(jobs.preparing("a").is_none());
    }

    #[cfg_attr(windows, ignore = "Unix shell fixture")]
    #[tokio::test]
    async fn successful_new_exposes_progress_then_disappears() {
        let dir = tempfile::tempdir().unwrap();
        // The script says "cloning" and goes on only when the test has seen it (the file `go`): what
        // the test looks at does not depend on how fast either of them runs.
        let go = dir.path().join("go");
        let body = format!(
            "echo cloning\nwhile [ ! -f {} ]; do sleep 0.05; done\nprintf 'installing 50%%\\rinstalling 100%%\\n'\nsleep 0.3",
            go.display()
        );
        let cli = fake_box(dir.path(), &body);
        let jobs = Jobs::default();
        assert!(start_new(cli, jobs.clone(), plain("m1", "p"), silent()));
        let deadline = Instant::now() + HANG;
        while Instant::now() < deadline {
            if jobs.preparing("m1").is_some_and(|p| p.line == "cloning") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert_eq!(jobs.preparing("m1").unwrap().line, "cloning");
        std::fs::write(&go, "").unwrap();
        let last = settled(&jobs, "m1").await;
        assert_eq!(last, None);
    }

    #[cfg_attr(windows, ignore = "Unix shell fixture")]
    #[tokio::test]
    async fn failed_new_keeps_the_last_stderr_line_and_failed_flag() {
        let dir = tempfile::tempdir().unwrap();
        let cli = fake_box(dir.path(), "echo working\necho 'npm ERR! boom' >&2\nexit 1");
        let jobs = Jobs::default();
        start_new(cli, jobs.clone(), plain("m1", "p"), silent());
        let last = settled(&jobs, "m1").await.unwrap();
        assert_eq!(
            last,
            Preparing {
                line: "npm ERR! boom".into(),
                failed: true,
                has_workspace: false,
            }
        );
    }

    #[cfg_attr(windows, ignore = "Unix shell fixture")]
    #[tokio::test]
    async fn a_new_past_its_limit_is_killed_with_its_children_and_fails_with_a_sentence() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("late");
        let body = format!("(sleep 2; echo x > {}) &\nsleep 30", marker.display());
        let timeouts = crate::box_cli::Timeouts {
            new: Duration::from_secs(1),
            ..crate::box_cli::Timeouts::default()
        };
        let cli = fake_box(dir.path(), &body).with_timeouts(timeouts);
        let jobs = Jobs::default();
        start_new(cli, jobs.clone(), plain("m1", "p"), silent());
        let last = settled(&jobs, "m1").await.unwrap();
        assert!(last.failed);
        assert_eq!(last.line, "sakura: box new timed out after 1 s");
        let limit = crate::said::Said::new("limit.seconds").count(1);
        assert_eq!(
            crate::box_words::said(&last.line).said,
            crate::said::Said::new("error.new.timeout").said("limit", limit),
            "the line is data: what is said of it names the limit"
        );
        tokio::time::sleep(Duration::from_millis(2500)).await;
        assert!(!marker.exists(), "the process group was killed");
    }

    #[cfg_attr(windows, ignore = "Unix shell fixture")]
    #[tokio::test]
    async fn the_same_new_started_twice_runs_once() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("runs");
        let body = format!("echo x >> {}\nsleep 0.5", marker.display());
        let cli = fake_box(dir.path(), &body);
        let jobs = Jobs::default();
        assert!(start_new(
            cli.clone(),
            jobs.clone(),
            plain("m1", "p"),
            silent()
        ));
        assert!(!start_new(cli, jobs.clone(), plain("m1", "p"), silent()));
        settled(&jobs, "m1").await;
        assert_eq!(std::fs::read_to_string(marker).unwrap().lines().count(), 1);
    }

    #[cfg_attr(windows, ignore = "Unix shell fixture")]
    #[tokio::test]
    async fn missing_box_fails_the_job_with_a_readable_line() {
        let jobs = Jobs::default();
        let cli = BoxCli::at("/nonexistent/box");
        start_new(cli, jobs.clone(), plain("m1", "p"), silent());
        let last = settled(&jobs, "m1").await.unwrap();
        assert!(last.failed);
        assert!(last.line.contains("box"));
    }

    #[cfg_attr(windows, ignore = "Unix shell fixture")]
    #[tokio::test]
    async fn notifier_fires_when_the_job_ends() {
        let dir = tempfile::tempdir().unwrap();
        let cli = fake_box(dir.path(), "true");
        let jobs = Jobs::default();
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let notify: JobNotifier = Arc::new(move || {
            let _ = sender.send(());
        });
        start_new(cli, jobs, plain("m1", "p"), notify);
        let fired = tokio::time::timeout(HANG, receiver.recv()).await;
        assert!(fired.is_ok());
    }

    #[cfg_attr(windows, ignore = "Unix shell fixture")]
    #[tokio::test]
    async fn a_flood_of_lines_gives_bounded_notifications_the_newest_line_and_the_outcome() {
        let dir = tempfile::tempdir().unwrap();
        let cli = fake_box(dir.path(), "seq 1 100000\necho newest-line\nsleep 1");
        let jobs = Jobs::default();
        let calls = Arc::new(AtomicUsize::new(0));
        let outcome_seen = Arc::new(AtomicUsize::new(0));
        let notify: JobNotifier = {
            let (calls, outcome_seen, jobs) = (calls.clone(), outcome_seen.clone(), jobs.clone());
            Arc::new(move || {
                calls.fetch_add(1, Ordering::SeqCst);
                outcome_seen.store(
                    usize::from(jobs.preparing("m1").is_none()),
                    Ordering::SeqCst,
                );
            })
        };
        let started = Instant::now();
        start_new(cli, jobs.clone(), plain("m1", "p"), notify);
        let mut newest_seen = false;
        while jobs.preparing("m1").is_some() && started.elapsed() < Duration::from_secs(20) {
            newest_seen |= jobs
                .preparing("m1")
                .is_some_and(|p| p.line == "newest-line");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        let allowed = started.elapsed().as_millis() / NOTIFY_INTERVAL.as_millis() + 3;
        assert!(newest_seen, "the newest line never showed");
        assert!(
            (calls.load(Ordering::SeqCst) as u128) <= allowed,
            "{} notifications in {:?}",
            calls.load(Ordering::SeqCst),
            started.elapsed()
        );
        assert_eq!(
            outcome_seen.load(Ordering::SeqCst),
            1,
            "last call was not the outcome"
        );
    }
}
