//! The scripted steps of a snapshot build (`SAKURA_SNAPSHOT_DRIVE`) press Approve, confirm and clean up with no
//! person to see it, so they exist only in a debug build with the `snapshot` feature. A build that has them is
//! refused at compile time when its debug assertions are off (a release), and a build that does not have them
//! refuses to start when the variable asks for them.

use std::ffi::OsStr;

/// The variable that holds the steps.
pub const DRIVE_VAR: &str = "SAKURA_SNAPSHOT_DRIVE";

/// Whether this build has the scripted steps: a debug build with the `snapshot` feature.
pub const STEPS_COMPILED: bool = cfg!(all(feature = "snapshot", debug_assertions));

/// Whether a launch with `steps` in the variable may go on in a build that has the scripted steps or not.
pub fn check(steps: Option<&OsStr>, are_steps_compiled: bool) -> Result<(), String> {
    match (steps, are_steps_compiled) {
        (Some(_), false) => Err(format!(
            "{DRIVE_VAR} is set, but this build has no scripted steps (they exist only in a debug build with the \
             snapshot feature); unset it to start Sakura"
        )),
        _ => Ok(()),
    }
}

/// Refuses to start, with a plain message and status 2, when the variable asks for steps this build does not
/// have.
pub fn enforce() {
    let steps = std::env::var_os(DRIVE_VAR);
    if let Err(refusal) = check(steps.as_deref(), STEPS_COMPILED) {
        eprintln!("sakura: {refusal}");
        std::process::exit(2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_build_without_the_steps_refuses_a_launch_that_asks_for_them() {
        let refusal = check(Some(OsStr::new("master|say:hi")), false).unwrap_err();
        assert!(refusal.contains(DRIVE_VAR), "{refusal}");
    }

    #[test]
    fn a_launch_without_steps_and_a_build_with_them_go_on() {
        assert_eq!(check(None, false), Ok(()));
        assert_eq!(check(None, true), Ok(()));
        assert_eq!(check(Some(OsStr::new("master")), true), Ok(()));
    }

    #[test]
    fn the_steps_exist_only_in_a_debug_build_with_the_snapshot_feature() {
        assert_eq!(
            STEPS_COMPILED,
            cfg!(feature = "snapshot") && cfg!(debug_assertions)
        );
    }
}
