//! The tmux inside the app bundle, and the variable that tells the engine where it is.

use super::leeway::bundled_program;
use std::path::{Path, PathBuf};

/// Environment variable the engine reads to find its tmux (`programs.py` on the engine side).
pub const TMUX_VAR: &str = "SAKURA_TMUX";

/// The tmux's file name in the bundle.
const TMUX_NAME: &str = "tmux";

/// The tmux of the bundle the running program is in: `<app>/Contents/Helpers/tmux`, when the program is
/// `<app>/Contents/MacOS/<name>` and the file can be run. None for a build run from a source tree, where
/// the engine looks for tmux on the PATH.
pub fn bundled_tmux(executable: &Path) -> Option<PathBuf> {
    bundled_program(executable, TMUX_NAME)
}

/// [`bundled_tmux`] for the program that is running now.
pub fn bundled_tmux_of_process() -> Option<PathBuf> {
    bundled_tmux(&std::env::current_exe().ok()?)
}
