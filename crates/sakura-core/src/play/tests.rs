use super::*;
use crate::box_cli::test_support::fake_box;

const TASK: &str = "sort-folders";
/// What `box play <task> start --json` documents.
const RUNNING: &str = r#"{ "task": "sort-folders", "running": true, "pid": 4711,
  "ports": [56000, 56001, 56002], "since": "2026-11-10T16:00:00Z" }"#;
const STOPPED: &str = r#"{ "task": "sort-folders", "running": false, "pid": null,
  "ports": [56000, 56001, 56002], "since": null }"#;
const NEVER: &str =
    r#"{ "task": "sort-folders", "running": false, "pid": null, "ports": [], "since": null }"#;

fn rig(body: &str) -> (tempfile::TempDir, BoxCli) {
    let dir = tempfile::tempdir().unwrap();
    let script = format!(
        r#"d="$(dirname "$0")"; printf '%s\n' "$*" >> "$d/calls.log"
{body}"#
    );
    let cli = fake_box(dir.path(), &script);
    (dir, cli)
}

fn calls(dir: &tempfile::TempDir) -> Vec<String> {
    let log = std::fs::read_to_string(dir.path().join("calls.log")).unwrap_or_default();
    log.lines().map(str::to_string).collect()
}

#[test]
fn the_state_is_read_as_box_documents_it() {
    let running = parse(TASK, RUNNING.as_bytes()).unwrap();
    assert_eq!(running.ports, [56000, 56001, 56002]);
    let since = crate::graph::parse_time("2026-11-10T16:00:00Z");
    assert_eq!(running.run, Run::Running { pid: 4711, since });
    assert!(running.is_running());
    assert_eq!(running.first_port(), Some(56000));
    let stopped = parse(TASK, STOPPED.as_bytes()).unwrap();
    assert_eq!((stopped.run, stopped.ports.len()), (Run::Stopped, 3));
    let never = parse(TASK, NEVER.as_bytes()).unwrap();
    assert_eq!((never.is_running(), never.first_port()), (false, None));
}

#[test]
fn a_state_that_cannot_be_is_refused_whole() {
    let refused = |text: String| parse(TASK, text.as_bytes()).unwrap_err().said;
    let not_a_state = Said::new("play.invalid.state");
    let with = |from: &str, to: &str| RUNNING.replace(from, to);
    assert_eq!(refused(with("sort-folders", "another-task")), not_a_state);
    assert_eq!(
        refused(with("4711", "null")),
        not_a_state,
        "running with no process"
    );
    assert_eq!(refused(with("4711", "0")), not_a_state);
    assert_eq!(refused(with("4711", "-1")), not_a_state);
    assert_eq!(refused(with("4711", "4294967296")), not_a_state);
    assert_eq!(refused(with("56001", "0")), not_a_state);
    assert_eq!(refused(with("56001", "65536")), not_a_state);
    assert_eq!(refused(with("56001", "56000")), not_a_state, "a port twice");
    assert_eq!(
        refused(with("2026-11-10T16:00:00Z", "tomorrow")),
        not_a_state
    );
    assert_eq!(
        refused(STOPPED.replace("null,", "12,")),
        not_a_state,
        "stopped with a process"
    );
    let many: Vec<String> = (0..=PORTS_MAX).map(|n| (50_000 + n).to_string()).collect();
    assert_eq!(
        refused(with("56000, 56001, 56002", &many.join(","))),
        not_a_state
    );
}

#[test]
fn a_port_is_opened_on_this_mac_only() {
    assert_eq!(url(56000), "http://localhost:56000");
}

#[test]
fn where_a_task_lives_and_its_project_s_command_say_whether_it_can_play() {
    assert_eq!(
        Availability::of(Place::Mac, Some("npm run dev")),
        Availability::Ready
    );
    assert_eq!(
        Availability::of(Place::Mac, None),
        Availability::NoRunCommand
    );
    assert_eq!(
        Availability::of(Place::Mac, Some("  ")),
        Availability::NoRunCommand
    );
    assert_eq!(
        Availability::of(Place::Box, Some("npm run dev")),
        Availability::InBox
    );
    assert_eq!(Availability::of(Place::Box, None), Availability::InBox);
}

#[test]
fn a_log_keeps_its_last_lines_within_bounds_and_no_colour_codes() {
    let log = Log::of("ready on \u{1b}[32mhttp://localhost:56000\u{1b}[0m\nGET / 200\n");
    assert_eq!(
        log.lines(),
        ["ready on http://localhost:56000", "GET / 200"]
    );
    assert!(!log.is_cut());
    // `box` takes the escape character away and leaves the rest of the code: that goes too.
    let left = Log::of("[1m[32mready[0m in 300 ms [3] items[]\n");
    assert_eq!(left.lines(), ["ready in 300 ms [3] items[]"]);
    // Other control characters go, a tab stays as spaces.
    assert_eq!(Log::of("a\u{7}b\tc\r\n").lines(), ["ab    c"]);
    let long: String = (0..LOG_MAX_LINES + 50)
        .map(|n| format!("line {n}\n"))
        .collect();
    let log = Log::of(&long);
    assert_eq!(log.lines().len(), LOG_MAX_LINES);
    assert_eq!(log.lines()[0], "line 50", "the newest lines are kept");
    assert!(log.is_cut());
    let wide = Log::of(&"x".repeat(LINE_MAX_CHARS + 10));
    assert_eq!(wide.lines()[0].chars().count(), LINE_MAX_CHARS);
    assert!(wide.is_cut());
    assert!(Log::of("").lines().is_empty());
    assert_eq!(Log::of("a\nb\n").text(), "a\nb");
}

#[cfg_attr(windows, ignore = "Unix shell fixture")]
#[tokio::test]
async fn play_is_asked_with_the_documented_arguments() {
    let (dir, cli) = rig(&format!(
        r#"case "$3" in
  start) echo '{}' ;;
  log) printf 'ready\n' ;;
  *) echo '{}' ;;
esac"#,
        RUNNING.replace('\n', " "),
        STOPPED.replace('\n', " ")
    ));
    let started = play(&cli, TASK, Action::Start).await.unwrap();
    assert!(matches!(started, Outcome::State(state) if state.is_running()));
    let stopped = play(&cli, TASK, Action::Stop).await.unwrap();
    assert!(matches!(stopped, Outcome::State(state) if !state.is_running()));
    play(&cli, TASK, Action::Status).await.unwrap();
    assert_eq!(read_log(&cli, TASK).await.unwrap().lines(), ["ready"]);
    assert_eq!(
        calls(&dir),
        [
            "play sort-folders start --json",
            "play sort-folders stop --json",
            "play sort-folders status --json",
            "play sort-folders log",
        ]
    );
    assert!(play(&cli, "Bad Name", Action::Start).await.is_err());
    assert!(read_log(&cli, "--help").await.is_err());
    assert_eq!(calls(&dir).len(), 4, "a bad name never reaches box");
}

#[cfg_attr(windows, ignore = "Unix shell fixture")]
#[tokio::test]
async fn a_task_in_a_box_is_an_answer_and_a_log_not_written_yet_is_empty() {
    let (_dir, cli) = rig(r#"case "$3" in
  log) echo "box: no output yet" >&2; exit 3 ;;
  *) echo "box: Play in a box is not built yet (task '$2' is in a box)" >&2; exit 81 ;;
esac"#);
    assert_eq!(play(&cli, TASK, Action::Start).await, Ok(Outcome::NotInBox));
    assert_eq!(
        play(&cli, TASK, Action::Status).await,
        Ok(Outcome::NotInBox)
    );
    assert!(read_log(&cli, TASK).await.unwrap().lines().is_empty());
}

#[cfg_attr(windows, ignore = "Unix shell fixture")]
#[tokio::test]
async fn a_start_that_fails_is_a_failure_with_box_s_line() {
    let (_dir, cli) =
        rig(r#"echo "box: the command ended at once: see box play sort-folders log" >&2; exit 1"#);
    let failure = play(&cli, TASK, Action::Start).await.unwrap_err();
    assert!(failure.details.unwrap().contains("ended at once"));
}
