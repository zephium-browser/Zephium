#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "macos")]
pub use macos as imp;
#[cfg(target_os = "macos")]
mod window_controls;

/// The corner-reveal state is pure timing, shared with tests off macOS.
pub(crate) mod fullscreen_reveal;

#[cfg(target_os = "windows")]
pub mod windows;
#[cfg(target_os = "windows")]
pub use windows as imp;

#[cfg(all(unix, not(target_os = "macos")))]
pub mod linux;
#[cfg(all(unix, not(target_os = "macos")))]
pub use linux as imp;

#[cfg(target_os = "windows")]
pub(crate) use windows::set_window_fullscreen;

#[cfg(not(target_os = "windows"))]
pub(crate) fn set_window_fullscreen(
    window: &tauri::WebviewWindow,
    enter: bool,
) -> tauri::Result<()> {
    window.set_fullscreen(enter)
}
