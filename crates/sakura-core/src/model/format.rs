//! Figures and facts for the window, without words: durations in unit symbols, and the age of facts, the
//! machine and the runner as things to say ([`Said`]), which the window words in its language.

use crate::models::{Machine, MemoryPressure, RunnerPhase, RunnerState};
use crate::said::{Key, Said};

/// Facts younger than this are not given an age.
pub const FRESH_FACTS_SECONDS: u64 = 30;
const SECONDS_PER_MINUTE: u64 = 60;
const SECONDS_PER_HOUR: u64 = 3600;

/// A duration in whole units: `38 s`, `7 min 11 s`, `1 h 5 min`. Negative or invalid input reads `0 s`.
pub fn duration(seconds: f64) -> String {
    let total = if seconds.is_finite() && seconds > 0.0 {
        seconds.round() as u64
    } else {
        0
    };
    let (hours, minutes, rest) = (
        total / SECONDS_PER_HOUR,
        total % SECONDS_PER_HOUR / SECONDS_PER_MINUTE,
        total % SECONDS_PER_MINUTE,
    );
    match (hours, minutes, rest) {
        (0, 0, s) => format!("{s} s"),
        (0, m, 0) => format!("{m} min"),
        (0, m, s) => format!("{m} min {s} s"),
        (h, 0, _) => format!("{h} h"),
        (h, m, _) => format!("{h} h {m} min"),
    }
}

/// How old facts are (so many seconds, minutes or hours ago), or `None` while they are fresh (up to
/// [`FRESH_FACTS_SECONDS`]).
pub fn facts_age(age_seconds: u64) -> Option<Said> {
    if age_seconds <= FRESH_FACTS_SECONDS {
        return None;
    }
    let said = if age_seconds < SECONDS_PER_MINUTE {
        Said::new("age.seconds").number("n", age_seconds)
    } else if age_seconds < SECONDS_PER_HOUR {
        Said::new("age.minutes").number("n", age_seconds / SECONDS_PER_MINUTE)
    } else {
        Said::new("age.hours").number("n", age_seconds / SECONDS_PER_HOUR)
    };
    Some(said)
}

/// The load: the one minute average to one decimal and the cores, said as high above the core count.
pub fn load(machine: &Machine) -> Said {
    let key = if is_load_high(machine) {
        "machine.load_high"
    } else {
        "machine.load"
    };
    Said::new(key)
        .tenths("load", machine.load1)
        .number("cores", u64::from(machine.cpus))
}

/// Whether more work waits than there are cores: the load line takes the warning tone.
pub fn is_load_high(machine: &Machine) -> bool {
    machine.load1 > f64::from(machine.cpus)
}

/// Whether memory pressure is critical: the memory line takes the warning tone.
pub fn is_memory_critical(machine: &Machine) -> bool {
    machine.memory_pressure == MemoryPressure::Critical
}

/// Memory pressure.
pub fn memory(machine: &Machine) -> Key {
    Key(match machine.memory_pressure {
        MemoryPressure::Normal => "machine.memory.normal",
        MemoryPressure::Warn => "machine.memory.warn",
        MemoryPressure::Critical => "machine.memory.critical",
    })
}

/// What the runner does: the check it runs, or that none runs, or that it is stopped (which means for
/// the person that no check will run).
pub fn runner_status(runner: &RunnerState) -> Said {
    match (runner.state, &runner.current) {
        (RunnerPhase::Stopped, _) => Said::new("runner.stopped"),
        (RunnerPhase::Running, Some(run)) => Said::new("runner.running")
            .text("verb", &run.verb)
            .text("task", &run.task),
        (RunnerPhase::Running, None) => Said::new("runner.idle"),
    }
}

/// How many requests wait, or `None` when the queue is empty.
pub fn runner_waiting(runner: &RunnerState) -> Option<Said> {
    match runner.queue.len() {
        0 => None,
        waiting => Some(Said::new("runner.waiting").count(waiting as u64)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{QueuedRequest, RunningRequest};

    fn machine(load1: f64, swap_used_mb: u64, pressure: MemoryPressure) -> Machine {
        Machine {
            cpus: 10,
            load1,
            memory_pressure: pressure,
            swap_used_mb,
            swap_total_mb: 4096,
            disk_free_gb: 100.0,
        }
    }

    fn runner(state: RunnerPhase, is_busy: bool, waiting: usize) -> RunnerState {
        let current = is_busy.then(|| RunningRequest {
            task: "m1".into(),
            verb: "test".into(),
            sha: "a".repeat(40),
            started_at: "2026-11-10T06:41:12Z".into(),
        });
        let queued = QueuedRequest {
            task: "m2".into(),
            verb: "build".into(),
            sha: "b".repeat(40),
            queued_at: "2026-11-10T06:43:50Z".into(),
        };
        RunnerState {
            state,
            current,
            queue: vec![queued; waiting],
        }
    }

    #[test]
    fn durations_read_in_whole_units() {
        assert_eq!(duration(38.0), "38 s");
        assert_eq!(duration(431.0), "7 min 11 s");
        assert_eq!(duration(420.0), "7 min");
        assert_eq!(duration(3900.0), "1 h 5 min");
        assert_eq!(duration(7200.0), "2 h");
        assert_eq!(duration(-1.0), "0 s");
        assert_eq!(duration(f64::NAN), "0 s");
    }

    #[test]
    fn fresh_facts_have_no_age_and_older_ones_do() {
        assert_eq!(facts_age(30), None);
        assert_eq!(
            facts_age(31),
            Some(Said::new("age.seconds").number("n", 31))
        );
        assert_eq!(
            facts_age(150),
            Some(Said::new("age.minutes").number("n", 2))
        );
        assert_eq!(facts_age(7300), Some(Said::new("age.hours").number("n", 2)));
    }

    fn load_said(key: &'static str, load: &str) -> Said {
        Said::new(key)
            .tenths("load", load.parse().unwrap())
            .number("cores", 10)
    }

    #[test]
    fn machine_lines_carry_their_figures_to_one_decimal() {
        let m = machine(3.24, 1229, MemoryPressure::Warn);
        assert_eq!(load(&m), load_said("machine.load", "3.2"));
        assert_eq!(memory(&m), Key("machine.memory.warn"));
        assert_eq!(
            load(&machine(4.0, 0, MemoryPressure::Normal)),
            load_said("machine.load", "4")
        );
    }

    #[test]
    fn load_above_the_core_count_says_so_and_warns() {
        let m = machine(14.1, 0, MemoryPressure::Normal);
        assert_eq!(load(&m), load_said("machine.load_high", "14.1"));
        assert!(is_load_high(&m));
    }

    #[test]
    fn load_at_or_below_the_core_count_is_plain() {
        let m = machine(10.0, 0, MemoryPressure::Normal);
        assert_eq!(load(&m), load_said("machine.load", "10"));
        assert!(!is_load_high(&m));
    }

    #[test]
    fn only_critical_memory_pressure_warns() {
        assert!(is_memory_critical(&machine(
            1.0,
            0,
            MemoryPressure::Critical
        )));
        assert!(!is_memory_critical(&machine(1.0, 0, MemoryPressure::Warn)));
        assert!(!is_memory_critical(&machine(
            1.0,
            0,
            MemoryPressure::Normal
        )));
    }

    #[test]
    fn runner_status_names_the_run_or_the_idle_state() {
        assert_eq!(
            runner_status(&runner(RunnerPhase::Running, true, 1)),
            Said::new("runner.running")
                .text("verb", "test")
                .text("task", "m1")
        );
        assert_eq!(
            runner_status(&runner(RunnerPhase::Running, false, 0)),
            Said::new("runner.idle")
        );
        assert_eq!(
            runner_status(&runner(RunnerPhase::Stopped, false, 2)),
            Said::new("runner.stopped")
        );
    }

    #[test]
    fn runner_waiting_counts_the_queue() {
        assert_eq!(runner_waiting(&runner(RunnerPhase::Running, true, 0)), None);
        assert_eq!(
            runner_waiting(&runner(RunnerPhase::Running, true, 1)),
            Some(Said::new("runner.waiting").count(1))
        );
        assert_eq!(
            runner_waiting(&runner(RunnerPhase::Stopped, false, 3)),
            Some(Said::new("runner.waiting").count(3))
        );
    }
}
