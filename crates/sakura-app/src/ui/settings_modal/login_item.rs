//! Opening Sakura when the person logs in: a launch agent in their own `LaunchAgents` folder that opens the
//! app by its bundle identifier. The file is the whole state: present means on.

use std::io;
use std::path::{Path, PathBuf};

/// The agent's label, and its file's name without the extension.
const LABEL: &str = "dev.yarin.sakura.login";

/// Where the agent's file is, in the person's `home`.
fn agent_path(home: &Path) -> PathBuf {
    home.join("Library/LaunchAgents")
        .join(format!("{LABEL}.plist"))
}

/// Whether Sakura opens at login.
pub fn is_enabled(home: &Path) -> bool {
    agent_path(home).exists()
}

/// Turns opening at login on or off. Turning it off when it is off is not an error.
pub fn set_enabled(home: &Path, is_on: bool) -> io::Result<()> {
    let path = agent_path(home);
    if !is_on {
        return match std::fs::remove_file(&path) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        };
    }
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder)?;
    }
    std::fs::write(path, agent_text())
}

/// The agent: open the app by its bundle identifier, once, at login.
fn agent_text() -> String {
    let app = crate::window::APP_ID;
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\"><dict>\n\
         <key>Label</key><string>{LABEL}</string>\n\
         <key>ProgramArguments</key><array><string>/usr/bin/open</string><string>-b</string><string>{app}</string></array>\n\
         <key>RunAtLoad</key><true/>\n\
         </dict></plist>\n"
    )
}
