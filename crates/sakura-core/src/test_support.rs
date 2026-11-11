//! Help for tests that put a script in the place of `box`, in this crate and in the ones built on it.

use crate::box_cli::Timeouts;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

/// Writes `text` to `path` as an executable script, through a child process. A test binary runs many
/// tests at once and each forks: a script this process had open for writing could still be held open by
/// another test's fork when it is run, and the run would fail with "text file busy". This process never
/// opens the script for writing: it writes the text beside it and has `cp` make the script.
pub fn write_script(path: &Path, text: &str) {
    let source = path.with_extension("txt");
    std::fs::write(&source, text).expect("the script's text is written");
    let made = Command::new("/bin/sh")
        .args(["-c", r#"/bin/cp "$1" "$2" && /bin/chmod 755 "$2""#, "sh"])
        .args([&source, path])
        .status()
        .expect("sh runs");
    assert!(
        made.success(),
        "the script was not made at {}",
        path.display()
    );
}

/// Limits for tests that are not about a limit: long enough that only a hang reaches them, on a machine
/// that runs every test at once. A test about a limit sets its own.
pub fn patient() -> Timeouts {
    let long = Duration::from_secs(180);
    Timeouts {
        read: long,
        names: long,
        slow: long,
        diff: long,
        action: long,
        take: Duration::from_secs(10 * 60),
        new: Duration::from_secs(45 * 60),
    }
}
