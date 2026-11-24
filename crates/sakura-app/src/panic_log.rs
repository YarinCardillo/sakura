//! A panic leaves its message, place and backtrace in `~/Library/Logs/Sakura/panic.log`. The app aborts on a panic
//! inside a window callback, and the system's crash report names the frames but never the message, so the message
//! is kept here before the default hook runs.

use std::io;
use std::path::{Path, PathBuf};

/// The folder of the log under the person's home.
const LOG_FOLDER: &str = "Library/Logs/Sakura";

const LOG_FILE: &str = "panic.log";

/// How many panics the log keeps; the oldest go first.
const KEPT_PANICS: usize = 20;

/// The line that starts every entry.
const ENTRY_MARK: &str = "=== panic ";

/// Where the log lives for the person at `home`.
pub fn log_folder(home: &Path) -> PathBuf {
    home.join(LOG_FOLDER)
}

/// Installs the hook: every panic is appended to the log in `folder` and then goes to the hook that was there.
pub fn install(folder: PathBuf) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let entry = entry_text(
            &local_time_text(),
            &info.to_string(),
            &std::backtrace::Backtrace::force_capture().to_string(),
        );
        if let Err(error) = append_entry(&folder, &entry) {
            eprintln!(
                "sakura: cannot write {}: {error}",
                folder.join(LOG_FILE).display()
            );
        }
        previous(info);
    }));
}

/// One entry of the log: a header with the time, the panic's own text (message and place) and the backtrace.
fn entry_text(time: &str, panic: &str, backtrace: &str) -> String {
    format!("{ENTRY_MARK}{time} ===\n{panic}\n{backtrace}\n")
}

/// Adds `entry` to the log in `folder`, which is made private (0700) as is the file (0600), and drops the oldest
/// entries beyond the last `KEPT_PANICS`.
fn append_entry(folder: &Path, entry: &str) -> io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(folder)?;
    std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o700))?;
    let path = folder.join(LOG_FILE);
    let before = std::fs::read_to_string(&path).unwrap_or_default();
    let kept = kept_with(&before, entry);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&path)?;
    io::Write::write_all(&mut file, kept.as_bytes())?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
}

/// The log's text with `entry` added and no more than the last `KEPT_PANICS` entries in it.
fn kept_with(before: &str, entry: &str) -> String {
    let mut entries: Vec<String> =
        before
            .split_inclusive('\n')
            .fold(Vec::new(), |mut found: Vec<String>, line| {
                match found.last_mut() {
                    Some(last) if !line.starts_with(ENTRY_MARK) => last.push_str(line),
                    _ if line.starts_with(ENTRY_MARK) => found.push(line.to_string()),
                    _ => {}
                }
                found
            });
    entries.push(entry.to_string());
    let drop = entries.len().saturating_sub(KEPT_PANICS);
    entries.split_off(drop).concat()
}

/// "2026-11-21 15:59:04" by this Mac's clock and zone.
fn local_time_text() -> String {
    use chrono::{Local, TimeZone};

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64);
    Local.timestamp_opt(now, 0).single().map_or_else(
        || format!("epoch {now}"),
        |time| time.format("%Y-%m-%d %H:%M:%S").to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn mode_of(path: &Path) -> u32 {
        std::fs::metadata(path)
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777
    }

    #[test]
    fn a_panic_is_in_the_log_with_its_message_its_place_and_a_backtrace() {
        let home = tempfile::tempdir().expect("a home");
        let folder = log_folder(home.path());
        let default_hook = std::panic::take_hook();
        install(folder.clone());
        let caught = std::panic::catch_unwind(|| panic!("the injected panic"));
        // Put the default hook back for the tests that run beside this one.
        let _ = std::panic::take_hook();
        std::panic::set_hook(default_hook);
        assert!(caught.is_err());
        let log = std::fs::read_to_string(folder.join(LOG_FILE)).expect("the log");
        assert!(log.contains(ENTRY_MARK), "{log}");
        assert!(log.contains("the injected panic"), "{log}");
        assert!(
            log.contains("panic_log.rs"),
            "the place of the panic: {log}"
        );
        assert!(log.contains("   0: "), "the backtrace frames: {log}");
    }

    #[test]
    fn the_log_is_private_to_the_person_the_folder_and_the_file() {
        let home = tempfile::tempdir().expect("a home");
        let folder = log_folder(home.path());
        append_entry(&folder, &entry_text("now", "a panic", "frames")).expect("written");
        assert_eq!(mode_of(&folder), 0o700);
        assert_eq!(mode_of(&folder.join(LOG_FILE)), 0o600);
    }

    #[test]
    fn a_log_that_was_loose_is_made_private_when_it_is_written_again() {
        let home = tempfile::tempdir().expect("a home");
        let folder = log_folder(home.path());
        std::fs::create_dir_all(&folder).expect("folder");
        std::fs::write(folder.join(LOG_FILE), "").expect("file");
        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o755)).expect("loose");
        std::fs::set_permissions(
            folder.join(LOG_FILE),
            std::fs::Permissions::from_mode(0o644),
        )
        .expect("loose");
        append_entry(&folder, &entry_text("now", "a panic", "frames")).expect("written");
        assert_eq!(mode_of(&folder), 0o700);
        assert_eq!(mode_of(&folder.join(LOG_FILE)), 0o600);
    }

    #[test]
    fn only_the_last_twenty_panics_are_kept() {
        let home = tempfile::tempdir().expect("a home");
        let folder = log_folder(home.path());
        for number in 0..(KEPT_PANICS + 5) {
            let entry = entry_text(
                "now",
                &format!("panic number {number}"),
                "frames\nmore frames",
            );
            append_entry(&folder, &entry).expect("written");
        }
        let log = std::fs::read_to_string(folder.join(LOG_FILE)).expect("the log");
        assert_eq!(log.matches(ENTRY_MARK).count(), KEPT_PANICS);
        assert!(!log.contains("panic number 4\n"), "the oldest are gone");
        assert!(log.contains("panic number 5\n"));
        assert!(log.contains("panic number 24\n"));
    }
}
