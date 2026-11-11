//! Machine facts on Linux, from `/proc`: the load, the memory still available and the swap. The parsing
//! is tested on every system; the reading is compiled on Linux only.

use crate::models::MemoryPressure;

const KB_PER_MB: u64 = 1024;
/// Memory still available, as a share of the total, below which the pressure is a warning or critical.
const WARN_BELOW_AVAILABLE: f64 = 0.15;
const CRITICAL_BELOW_AVAILABLE: f64 = 0.05;

/// The one minute load average: the first field of `/proc/loadavg`.
pub(super) fn load_one_minute(loadavg: &str) -> Option<f64> {
    let value: f64 = loadavg.split_whitespace().next()?.parse().ok()?;
    Some((value * 100.0).round() / 100.0)
}

/// A `/proc/meminfo` value in kB.
fn kilobytes(meminfo: &str, name: &str) -> Option<u64> {
    let line = meminfo.lines().find(|line| {
        line.strip_prefix(name)
            .is_some_and(|rest| rest.starts_with(':'))
    })?;
    line.split_whitespace().nth(1)?.parse().ok()
}

/// Swap used and total, in MB.
pub(super) fn swap_mb(meminfo: &str) -> Option<(u64, u64)> {
    let total = kilobytes(meminfo, "SwapTotal")?;
    let free = kilobytes(meminfo, "SwapFree")?;
    Some((total.saturating_sub(free) / KB_PER_MB, total / KB_PER_MB))
}

/// The pressure, from how much of the memory is still available.
pub(super) fn pressure(meminfo: &str) -> Option<MemoryPressure> {
    let total = kilobytes(meminfo, "MemTotal")?;
    let available = kilobytes(meminfo, "MemAvailable")?;
    if total == 0 {
        return None;
    }
    let share = available as f64 / total as f64;
    Some(if share < CRITICAL_BELOW_AVAILABLE {
        MemoryPressure::Critical
    } else if share < WARN_BELOW_AVAILABLE {
        MemoryPressure::Warn
    } else {
        MemoryPressure::Normal
    })
}

/// Reads the current machine facts. The disk figure is for the volume holding `home`.
#[cfg(target_os = "linux")]
pub(super) fn read(home: &std::path::Path) -> crate::error::BridgeResult<crate::models::Machine> {
    use crate::box_cli::internal;
    let file = |path: &str| {
        std::fs::read_to_string(path).map_err(|error| internal(format!("{path}: {error}")))
    };
    let unreadable = |what: &str| internal(format!("{what} could not be read from /proc"));
    let (loadavg, meminfo) = (file("/proc/loadavg")?, file("/proc/meminfo")?);
    let (swap_used_mb, swap_total_mb) = swap_mb(&meminfo).ok_or_else(|| unreadable("the swap"))?;
    let cpus = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
    Ok(crate::models::Machine {
        cpus: u32::try_from(cpus).unwrap_or(u32::MAX),
        load1: load_one_minute(&loadavg).ok_or_else(|| unreadable("the load"))?,
        memory_pressure: pressure(&meminfo).ok_or_else(|| unreadable("the memory"))?,
        swap_used_mb,
        swap_total_mb,
        disk_free_gb: super::disk_free_gb(home)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const MEMINFO: &str = "MemTotal:       16000000 kB\nMemFree:          500000 kB\n\
        MemAvailable:    8000000 kB\nSwapCached:            0 kB\nSwapTotal:       2097152 kB\n\
        SwapFree:        1048576 kB\n";

    #[test]
    fn the_load_is_the_first_field_rounded() {
        assert_eq!(load_one_minute("1.234 0.80 0.50 2/345 6789\n"), Some(1.23));
        assert_eq!(load_one_minute(""), None);
        assert_eq!(load_one_minute("high 0 0"), None);
    }

    #[test]
    fn swap_is_used_and_total_in_megabytes() {
        assert_eq!(swap_mb(MEMINFO), Some((1024, 2048)));
        assert_eq!(swap_mb("MemTotal: 1 kB\n"), None);
    }

    #[test]
    fn a_name_that_only_starts_the_same_is_another_value() {
        assert_eq!(kilobytes(MEMINFO, "Swap"), None);
        assert_eq!(kilobytes(MEMINFO, "SwapTotal"), Some(2_097_152));
    }

    #[test]
    fn the_pressure_follows_the_memory_still_available() {
        let with = |available: u64| format!("MemTotal: 1000 kB\nMemAvailable: {available} kB\n");
        assert_eq!(pressure(&with(500)), Some(MemoryPressure::Normal));
        assert_eq!(pressure(&with(100)), Some(MemoryPressure::Warn));
        assert_eq!(pressure(&with(40)), Some(MemoryPressure::Critical));
        assert_eq!(pressure("MemTotal: 0 kB\nMemAvailable: 0 kB\n"), None);
    }
}
