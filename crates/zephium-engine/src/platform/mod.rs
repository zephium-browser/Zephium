#[cfg(all(feature = "agentic-browser", any(target_os = "windows", test)))]
#[cfg_attr(all(target_os = "windows", not(test)), allow(dead_code))]
mod agent_cookie_preflight;
#[cfg(all(
    feature = "agentic-browser",
    any(target_os = "macos", target_os = "windows", test)
))]
mod agent_navigation;
#[cfg(all(feature = "agentic-browser", any(target_os = "windows", test)))]
mod agent_screenshot_buffer;
// Only the Windows runtime calls the action helpers; Windows tests keep them honest.
#[cfg(all(feature = "agentic-browser", any(target_os = "windows", test)))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
#[cfg_attr(all(target_os = "windows", not(test)), allow(dead_code))]
mod agent_semantic_cdp_protocol;
#[cfg(all(feature = "agentic-browser", any(target_os = "windows", test)))]
pub(crate) mod agent_suspension;
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub(crate) mod cosmetic_pull;
#[cfg(all(
    feature = "agentic-browser",
    any(target_os = "macos", target_os = "windows", test)
))]
pub(crate) mod work_document_navigation;

#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "macos")]
pub use macos as imp;

#[cfg(target_os = "windows")]
pub mod windows;
#[cfg(target_os = "windows")]
pub use windows as imp;

#[cfg(all(unix, not(target_os = "macos")))]
pub mod linux;
#[cfg(all(unix, not(target_os = "macos")))]
pub use linux as imp;
pub(crate) mod content_pause;

/// The frame's content ground in each theme (`--color-page`); native covers
/// paint it so a page that has not painted yet reads as the empty pane.
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub(crate) const PAGE_GROUND_DARK: (u8, u8, u8) = (0x1a, 0x1a, 0x1d);
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub(crate) const PAGE_GROUND_LIGHT: (u8, u8, u8) = (0xf1, 0xf1, 0xf3);
