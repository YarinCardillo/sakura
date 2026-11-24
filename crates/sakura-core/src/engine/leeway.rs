//! The Leeway helper inside the app bundle, and the variable that tells the engine where it is.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// Environment variable the engine reads to find the Leeway command (`leeway.locate` on the engine side).
pub const LEEWAY_VAR: &str = "SAKURA_LEEWAY";

/// The helper's file name in the bundle.
const HELPER_NAME: &str = "leeway";
/// Any of the person, group and other execute bits.
const EXECUTE_BITS: u32 = 0o111;

/// The helper of the bundle the running program is in: `<app>/Contents/Helpers/leeway`, when the program is
/// `<app>/Contents/MacOS/<name>` and the helper is a file the person can run. None for a build run from a
/// source tree, where the engine looks for Leeway on its own.
pub fn bundled_helper(executable: &Path) -> Option<PathBuf> {
    bundled_program(executable, HELPER_NAME)
}

/// `<app>/Contents/Helpers/<name>` for the bundle the program `executable` is in, when it is a file the
/// person can run (shared with the other programs the bundle carries).
pub(super) fn bundled_program(executable: &Path, name: &str) -> Option<PathBuf> {
    let macos = executable.parent()?;
    let contents = macos.parent()?;
    if macos.file_name()? != "MacOS" || contents.file_name()? != "Contents" {
        return None;
    }
    let helper = contents.join("Helpers").join(name);
    is_runnable_file(&helper).then_some(helper)
}

/// [`bundled_helper`] for the program that is running now.
pub fn bundled_helper_of_process() -> Option<PathBuf> {
    bundled_helper(&std::env::current_exe().ok()?)
}

fn is_runnable_file(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|meta| meta.is_file() && meta.permissions().mode() & EXECUTE_BITS != 0)
        .unwrap_or(false)
}
