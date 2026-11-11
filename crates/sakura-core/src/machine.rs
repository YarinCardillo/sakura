//! Machine facts read from the system itself: load, memory pressure, swap, cores and free disk. No process
//! is started. On macOS everything comes from `sysctlbyname` and `statvfs`, on Linux from `/proc` and
//! `statvfs`; on Windows they are not read yet, and [`read`] says so.
#![cfg_attr(not(target_os = "macos"), allow(dead_code, unused_imports))]

#[cfg(any(target_os = "linux", test))]
mod linux;

use crate::box_cli::internal;
use crate::error::BridgeResult;
use crate::models::{Machine, MemoryPressure};
use std::ffi::CString;
use std::mem::{size_of, MaybeUninit};

const BYTES_PER_MB: u64 = 1024 * 1024;
const BYTES_PER_GB: f64 = 1_000_000_000.0;
const PRESSURE_NORMAL: i32 = 1;
const PRESSURE_WARN: i32 = 2;
const PRESSURE_CRITICAL: i32 = 4;

/// `struct loadavg` from `<sys/sysctl.h>`.
#[repr(C)]
#[derive(Clone, Copy)]
struct LoadAverage {
    fixed_point: [u32; 3],
    scale: i64,
}

/// `struct xsw_usage` from `<sys/sysctl.h>`.
#[repr(C)]
#[derive(Clone, Copy)]
struct SwapUsage {
    total: u64,
    available: u64,
    used: u64,
    page_size: u32,
    is_encrypted: u32,
}

/// Reads the current machine facts. The disk figure is for the volume holding `home`.
#[cfg(target_os = "linux")]
pub fn read(home: &std::path::Path) -> BridgeResult<Machine> {
    linux::read(home)
}

/// The machine facts are not read on this system yet.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn read(_home: &std::path::Path) -> BridgeResult<Machine> {
    Err(internal(
        "the machine's load and memory are not read on this system yet".into(),
    ))
}

/// Reads the current machine facts. The disk figure is for the volume holding `home`.
#[cfg(target_os = "macos")]
pub fn read(home: &std::path::Path) -> BridgeResult<Machine> {
    let load: LoadAverage = sysctl("vm.loadavg")?;
    let swap: SwapUsage = sysctl("vm.swapusage")?;
    let level: i32 = sysctl("kern.memorystatus_vm_pressure_level")?;
    let cpus: i32 = sysctl("hw.ncpu")?;
    Ok(Machine {
        cpus: u32::try_from(cpus).map_err(|_| internal("hw.ncpu is not a valid count".into()))?,
        load1: load_one_minute(&load),
        memory_pressure: pressure(level)?,
        swap_used_mb: swap.used / BYTES_PER_MB,
        swap_total_mb: swap.total / BYTES_PER_MB,
        disk_free_gb: disk_free_gb(home)?,
    })
}

fn load_one_minute(load: &LoadAverage) -> f64 {
    if load.scale <= 0 {
        return 0.0;
    }
    let value = f64::from(load.fixed_point[0]) / load.scale as f64;
    (value * 100.0).round() / 100.0
}

fn pressure(level: i32) -> BridgeResult<MemoryPressure> {
    match level {
        PRESSURE_NORMAL => Ok(MemoryPressure::Normal),
        PRESSURE_WARN => Ok(MemoryPressure::Warn),
        PRESSURE_CRITICAL => Ok(MemoryPressure::Critical),
        other => Err(internal(format!("unknown memory pressure level {other}"))),
    }
}

#[cfg(unix)]
fn disk_free_gb(path: &std::path::Path) -> BridgeResult<f64> {
    let c_path = CString::new(path.as_os_str().as_encoded_bytes())
        .map_err(|_| internal("the home path holds a NUL byte".into()))?;
    let mut stats = MaybeUninit::<libc::statvfs>::zeroed();
    // SAFETY: `c_path` is a valid NUL-terminated string and `stats` is a writable statvfs.
    let result = unsafe { libc::statvfs(c_path.as_ptr(), stats.as_mut_ptr()) };
    if result != 0 {
        let error = std::io::Error::last_os_error();
        return Err(internal(format!("statvfs failed: {error}")));
    }
    // SAFETY: statvfs returned 0, so the struct is fully initialised.
    let stats = unsafe { stats.assume_init() };
    // The two fields are 32 or 64 bits wide depending on the system.
    #[allow(clippy::useless_conversion)]
    let bytes = u64::from(stats.f_bavail) * u64::from(stats.f_frsize);
    Ok((bytes as f64 / BYTES_PER_GB * 10.0).round() / 10.0)
}

/// Reads one fixed-size sysctl value, checking that the kernel returned exactly `size_of::<T>()` bytes.
#[cfg(target_os = "macos")]
fn sysctl<T: Copy>(name: &str) -> BridgeResult<T> {
    let c_name =
        CString::new(name).map_err(|_| internal("the sysctl name holds a NUL byte".into()))?;
    let mut value = MaybeUninit::<T>::zeroed();
    let mut length = size_of::<T>();
    // SAFETY: `c_name` is NUL-terminated; `value` has room for `length` bytes; no new value is set.
    let result = unsafe {
        libc::sysctlbyname(
            c_name.as_ptr(),
            value.as_mut_ptr().cast(),
            &mut length,
            std::ptr::null_mut(),
            0,
        )
    };
    if result != 0 || length != size_of::<T>() {
        let error = std::io::Error::last_os_error();
        return Err(internal(format!("sysctl {name} failed: {error}")));
    }
    // SAFETY: the kernel filled all `size_of::<T>()` bytes, and T is a plain-data struct or integer.
    Ok(unsafe { value.assume_init() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pressure_levels_map_to_words() {
        assert_eq!(pressure(1).unwrap(), MemoryPressure::Normal);
        assert_eq!(pressure(2).unwrap(), MemoryPressure::Warn);
        assert_eq!(pressure(4).unwrap(), MemoryPressure::Critical);
        assert!(pressure(3).is_err());
    }

    #[test]
    fn load_is_scaled_and_rounded() {
        let load = LoadAverage {
            fixed_point: [3_277, 0, 0],
            scale: 2_048,
        };
        assert_eq!(load_one_minute(&load), 1.6);
        let broken = LoadAverage {
            fixed_point: [1, 0, 0],
            scale: 0,
        };
        assert_eq!(load_one_minute(&broken), 0.0);
    }

    #[cfg(unix)]
    #[test]
    fn real_machine_facts_are_plausible() {
        let home = std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .unwrap();
        let machine = read(&home).unwrap();
        assert!(machine.cpus >= 1);
        assert!(machine.load1 >= 0.0);
        assert!(machine.disk_free_gb > 0.0);
        assert!(machine.swap_used_mb <= machine.swap_total_mb + 1);
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_disk_path_is_an_error() {
        assert!(disk_free_gb(std::path::Path::new("/nonexistent/volume")).is_err());
    }
}
