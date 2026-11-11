//! Where a path the agent names is, relative to its workspace. One reading for both uses: the confinement
//! policy decides with it, and the window shortens a path for display with it, so a path can never be
//! shown as inside the workspace and judged as outside, or the reverse.
//!
//! The reading is lexical: `.` and `..` are resolved as text, relative paths are taken from the workspace,
//! and a path is inside only when the workspace's components are its first components (so `/work/a-b` is
//! not inside `/work/a`). A symbolic link inside the workspace that points outside it is not seen.

use std::path::{Component, Path, PathBuf};

/// Where a path is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Place {
    /// Inside the workspace: the path from the workspace, empty for the workspace itself.
    Inside(String),
    /// Outside the workspace: the whole absolute path, resolved; or the path as it was given when it
    /// cannot be resolved (it climbs above the root).
    Outside(String),
}

impl Place {
    pub fn is_inside(&self) -> bool {
        matches!(self, Place::Inside(_))
    }
}

/// Where `path` is relative to `workspace` (an absolute path; with any other workspace nothing is inside).
pub fn place(path: &str, workspace: &Path) -> Place {
    let Some(resolved) = normalized(&workspace.join(path)) else {
        return Place::Outside(path.to_string());
    };
    let inside = match workspace.is_absolute() {
        true => resolved.strip_prefix(workspace).ok(),
        false => None,
    };
    match inside {
        Some(rest) => Place::Inside(rest.to_string_lossy().into_owned()),
        None => Place::Outside(resolved.to_string_lossy().into_owned()),
    }
}

/// `path` without `.` and `..`; `None` when it climbs above the root.
fn normalized(path: &Path) -> Option<PathBuf> {
    let mut clean = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                let is_root = clean.parent().is_none();
                if is_root || !clean.pop() {
                    return None;
                }
            }
            other => clean.push(other),
        }
    }
    Some(clean)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn placed(path: &str) -> Place {
        place(path, Path::new("/work/m1"))
    }

    fn inside(rest: &str) -> Place {
        Place::Inside(rest.to_string())
    }

    fn outside(path: &str) -> Place {
        Place::Outside(path.to_string())
    }

    #[test]
    fn place_paths_under_the_workspace_are_inside_and_shown_from_it() {
        assert_eq!(placed("/work/m1/src/lib.rs"), inside("src/lib.rs"));
        assert_eq!(placed("src/lib.rs"), inside("src/lib.rs"));
        assert_eq!(placed("./src/"), inside("src"));
        assert_eq!(placed("/work/m1"), inside(""));
        assert_eq!(placed("/work/m1/"), inside(""));
    }

    #[test]
    fn place_a_sibling_that_shares_the_workspace_prefix_is_outside_and_whole() {
        assert_eq!(placed("/work/m1-evil/x"), outside("/work/m1-evil/x"));
        assert_eq!(placed("/work/m10/x"), outside("/work/m10/x"));
        assert_eq!(placed("/work/m"), outside("/work/m"));
    }

    #[test]
    fn place_dots_are_resolved_before_the_path_is_judged() {
        assert_eq!(placed("/work/m1/a/../b.rs"), inside("b.rs"));
        assert_eq!(placed("/work/m1/../other/x"), outside("/work/other/x"));
        assert_eq!(placed("../other/x"), outside("/work/other/x"));
        assert_eq!(placed("src/../../m1-evil/x"), outside("/work/m1-evil/x"));
        assert_eq!(placed("/work/m1/../m1/x"), inside("x"));
    }

    #[test]
    fn place_a_path_that_climbs_above_the_root_is_outside_as_it_was_given() {
        assert_eq!(placed("/../etc/passwd"), outside("/../etc/passwd"));
        assert_eq!(placed("../../../x"), outside("../../../x"));
    }

    #[test]
    fn place_other_absolute_paths_are_outside() {
        assert_eq!(placed("/etc/passwd"), outside("/etc/passwd"));
        assert_eq!(placed("/"), outside("/"));
    }

    #[test]
    fn place_with_a_workspace_that_is_not_absolute_nothing_is_inside() {
        assert!(!place("src/lib.rs", Path::new("work")).is_inside());
        assert!(!place("x", Path::new("")).is_inside());
    }
}
