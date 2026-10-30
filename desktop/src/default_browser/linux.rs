//! The desktop decides the default browser through xdg-settings, which knows
//! each environment's own setting (GNOME, KDE, XFCE, plain mimeapps.list).

use std::path::PathBuf;
use std::process::{Command, Stdio};

const DESKTOP_ID: &str = concat!(env!("ZEPHIUM_LINUX_APP_ID"), ".desktop");

/// Only an installed desktop entry can be chosen; a development binary has
/// none, and xdg-settings would record a browser that cannot launch.
pub(crate) fn can_request() -> bool {
    installed() && xdg_settings(&["--version"]).is_some()
}

pub(crate) fn is_default() -> bool {
    xdg_settings(&["check", "default-web-browser", DESKTOP_ID])
        .is_some_and(|answer| answer.trim() == "yes")
}

pub(crate) fn request() -> bool {
    can_request() && xdg_settings(&["set", "default-web-browser", DESKTOP_ID]).is_some()
}

fn installed() -> bool {
    let home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")));
    let system = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|dirs| !dirs.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    home.into_iter()
        .chain(system.split(':').map(PathBuf::from))
        .any(|dir| dir.join("applications").join(DESKTOP_ID).is_file())
}

/// Runs xdg-settings and returns its output when it succeeded.
fn xdg_settings(arguments: &[&str]) -> Option<String> {
    let output = Command::new("xdg-settings")
        .args(arguments)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}
