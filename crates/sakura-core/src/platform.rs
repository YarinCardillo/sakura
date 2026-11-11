//! What differs between the systems Sakura runs on: where its data lives, where the service it carries
//! is, and how `box` is started. macOS and Linux run `box` directly; Windows runs it under WSL, because
//! the service is written for a Unix shell.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The variable that names the data folder for `box` and for the app.
pub const HOME_VAR: &str = "AGENTBOX_HOME";
/// The data folder's name where folders are capitalised (macOS, Windows) and where they are not (XDG).
const FOLDER_NAME: &str = "Sakura";
const XDG_FOLDER_NAME: &str = "sakura";
/// Where the bundle, the tarball and a build inside the repository keep `service/`, from the executable.
const SERVICE_FROM_EXECUTABLE: [&str; 3] = ["../Resources/service", "service", "../../service"];
const BOX_IN_SERVICE: &str = "bin/box";
const WSL_PROGRAM: &str = "wsl.exe";

/// A system Sakura runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum System {
    Mac,
    Linux,
    Windows,
}

impl System {
    /// The system this build is for.
    pub const fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::Mac
        } else if cfg!(windows) {
            Self::Windows
        } else {
            Self::Linux
        }
    }
}

/// The data folder: `$AGENTBOX_HOME` when set, else the system's application data folder.
pub fn data_home() -> PathBuf {
    data_home_on(System::current(), &|name| std::env::var_os(name))
}

/// The data folder on `system`, with `variable` reading the environment.
pub fn data_home_on(system: System, variable: &dyn Fn(&str) -> Option<OsString>) -> PathBuf {
    let folder = |name: &str| {
        variable(name)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    };
    if let Some(chosen) = folder(HOME_VAR) {
        return chosen;
    }
    let home = folder("HOME").unwrap_or_default();
    match system {
        System::Mac => home.join("Library/Application Support").join(FOLDER_NAME),
        System::Linux => folder("XDG_DATA_HOME")
            .unwrap_or_else(|| home.join(".local/share"))
            .join(XDG_FOLDER_NAME),
        System::Windows => folder("APPDATA")
            .or_else(|| folder("USERPROFILE").map(|profile| profile.join("AppData/Roaming")))
            .unwrap_or_default()
            .join(FOLDER_NAME),
    }
}

/// Creates the data folder and names it to every child of this process. Call once, before any thread
/// is started. On Windows `box` runs under WSL and keeps its data there, so the variable is left alone.
pub fn prepare_data_home() -> std::io::Result<PathBuf> {
    let home = data_home();
    std::fs::create_dir_all(&home)?;
    if System::current() != System::Windows && std::env::var_os(HOME_VAR).is_none() {
        std::env::set_var(HOME_VAR, &home);
    }
    Ok(home)
}

/// The `box` of the service carried beside `executable`, when it is there.
pub fn carried_box(executable: &Path) -> Option<PathBuf> {
    let folder = executable.parent()?;
    SERVICE_FROM_EXECUTABLE
        .iter()
        .map(|service| folder.join(service).join(BOX_IN_SERVICE))
        .find(|candidate| candidate.is_file())
}

/// Where `box` is expected when nothing names it: the carried one, or its place beside the executable
/// so that the message about a missing `box` names a real path.
pub fn default_box() -> PathBuf {
    let executable = std::env::current_exe().unwrap_or_default();
    carried_box(&executable).unwrap_or_else(|| {
        let folder = executable.parent().unwrap_or(Path::new(""));
        folder.join(SERVICE_FROM_EXECUTABLE[1]).join(BOX_IN_SERVICE)
    })
}

/// How `box` is started: the program, and the arguments that come before `box`'s own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub program: PathBuf,
    pub leading: Vec<String>,
}

impl Launch {
    /// The launch of the `box` at `bin` on this system.
    pub fn of(bin: &Path) -> Self {
        Self::on(System::current(), bin)
    }

    /// The launch of the `box` at `bin` on `system`.
    pub fn on(system: System, bin: &Path) -> Self {
        match system {
            System::Mac | System::Linux => Self {
                program: bin.to_path_buf(),
                leading: Vec::new(),
            },
            System::Windows => Self {
                program: PathBuf::from(WSL_PROGRAM),
                leading: vec![
                    "--exec".into(),
                    "bash".into(),
                    wsl_path(&bin.to_string_lossy()),
                ],
            },
        }
    }
}

/// A Windows path as WSL mounts it: `C:\Tools\box` is `/mnt/c/Tools/box`. A path without a drive letter
/// is given with its separators turned, which is right for one that is already a WSL path.
pub fn wsl_path(windows: &str) -> String {
    let turned = windows.replace('\\', "/");
    let mut letters = turned.chars();
    match (letters.next(), letters.next()) {
        (Some(drive), Some(':')) if drive.is_ascii_alphabetic() => {
            format!("/mnt/{}{}", drive.to_ascii_lowercase(), &turned[2..])
        }
        _ => turned,
    }
}

/// Whether the file at `path` can be run. Windows has no such permission: there a file is enough.
pub fn is_runnable(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    meta.is_file() && has_execute_permission(&meta)
}

#[cfg(unix)]
fn has_execute_permission(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn has_execute_permission(_meta: &std::fs::Metadata) -> bool {
    true
}

/// Kills the process group started for a child, or on Windows the child and everything it started.
pub fn kill_tree(pid: u32) {
    if pid <= 1 {
        return;
    }
    kill_tree_of(pid);
}

#[cfg(unix)]
fn kill_tree_of(pid: u32) {
    let Ok(group) = i32::try_from(pid) else {
        return;
    };
    // SAFETY: killpg takes plain integers and has no memory effects. The id is a child we spawned in its
    // own group, and ids 0 and 1 are excluded by the caller.
    unsafe {
        libc::killpg(group, libc::SIGKILL);
    }
}

/// Kills one child process. On Windows whatever it started ends with it.
pub fn kill_process(pid: u32) {
    if pid <= 1 {
        return;
    }
    kill_process_of(pid);
}

#[cfg(unix)]
fn kill_process_of(pid: u32) {
    let Ok(pid) = i32::try_from(pid) else {
        return;
    };
    // SAFETY: plain integer arguments; ids 0 and 1 are excluded by the caller.
    unsafe {
        libc::kill(pid, libc::SIGKILL);
    }
}

#[cfg(not(unix))]
fn kill_process_of(pid: u32) {
    kill_tree_of(pid);
}

#[cfg(not(unix))]
fn kill_tree_of(pid: u32) {
    let ended = std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    if let Err(error) = ended {
        eprintln!("sakura: could not end process {pid}: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn environment(
        pairs: &'static [(&'static str, &'static str)],
    ) -> impl Fn(&str) -> Option<OsString> {
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| OsString::from(value))
        }
    }

    #[test]
    fn the_named_folder_wins_on_every_system() {
        let named = environment(&[("AGENTBOX_HOME", "/data/here"), ("HOME", "/home/me")]);
        for system in [System::Mac, System::Linux, System::Windows] {
            assert_eq!(data_home_on(system, &named), PathBuf::from("/data/here"));
        }
    }

    #[test]
    fn each_system_has_its_application_data_folder() {
        let mac = environment(&[("HOME", "/home/me")]);
        assert_eq!(
            data_home_on(System::Mac, &mac),
            PathBuf::from("/home/me/Library/Application Support/Sakura")
        );
        assert_eq!(
            data_home_on(System::Linux, &mac),
            PathBuf::from("/home/me/.local/share/sakura")
        );
        let xdg = environment(&[("HOME", "/home/me"), ("XDG_DATA_HOME", "/data")]);
        assert_eq!(
            data_home_on(System::Linux, &xdg),
            PathBuf::from("/data/sakura")
        );
        let windows = environment(&[("APPDATA", "C:/Profiles/me/AppData/Roaming")]);
        assert_eq!(
            data_home_on(System::Windows, &windows),
            PathBuf::from("C:/Profiles/me/AppData/Roaming").join("Sakura")
        );
    }

    #[test]
    fn an_empty_variable_counts_as_unset() {
        let empty = environment(&[
            ("AGENTBOX_HOME", ""),
            ("HOME", "/home/me"),
            ("XDG_DATA_HOME", ""),
        ]);
        assert_eq!(
            data_home_on(System::Linux, &empty),
            PathBuf::from("/home/me/.local/share/sakura")
        );
    }

    #[test]
    fn the_carried_box_is_found_in_a_bundle_and_beside_the_executable() {
        let dir = tempfile::tempdir().unwrap();
        let bundle = dir.path().join("Sakura.app/Contents");
        std::fs::create_dir_all(bundle.join("MacOS")).unwrap();
        std::fs::create_dir_all(bundle.join("Resources/service/bin")).unwrap();
        std::fs::write(bundle.join("Resources/service/bin/box"), "").unwrap();
        let found = carried_box(&bundle.join("MacOS/sakura")).unwrap();
        assert!(found.ends_with("Resources/service/bin/box"));

        let flat = dir.path().join("flat");
        std::fs::create_dir_all(flat.join("service/bin")).unwrap();
        std::fs::write(flat.join("service/bin/box"), "").unwrap();
        assert_eq!(
            carried_box(&flat.join("sakura")).unwrap(),
            flat.join("service/bin/box")
        );

        assert_eq!(carried_box(&dir.path().join("alone/sakura")), None);
    }

    #[test]
    fn box_is_started_directly_on_unix_and_under_wsl_on_windows() {
        let direct = Launch::on(System::Linux, Path::new("/opt/sakura/service/bin/box"));
        assert_eq!(direct.program, PathBuf::from("/opt/sakura/service/bin/box"));
        assert!(direct.leading.is_empty());

        let under_wsl = Launch::on(
            System::Windows,
            Path::new(r"C:\Tools\Sakura\service\bin\box"),
        );
        assert_eq!(under_wsl.program, PathBuf::from("wsl.exe"));
        assert_eq!(
            under_wsl.leading,
            ["--exec", "bash", "/mnt/c/Tools/Sakura/service/bin/box"]
        );
    }

    #[test]
    fn a_windows_path_becomes_the_one_wsl_mounts() {
        assert_eq!(wsl_path(r"D:\a b\box"), "/mnt/d/a b/box");
        assert_eq!(wsl_path("/home/me/box"), "/home/me/box");
        assert_eq!(wsl_path("box"), "box");
    }

    #[cfg(unix)]
    #[test]
    fn a_file_without_the_execute_permission_cannot_be_run() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("box");
        std::fs::write(&path, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(!is_runnable(&path));
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(is_runnable(&path));
        assert!(!is_runnable(&dir.path().join("missing")));
    }
}
