//! The main native window's frame, stored with application/session settings.

use serde::{Deserialize, Serialize};
use zephium_core::ports::store::Store;

use crate::SqliteStore;

const KEY: &str = "window.main.frame.v1";

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct WindowRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl WindowRect {
    pub fn is_valid(self) -> bool {
        [self.x, self.y, self.width, self.height]
            .into_iter()
            .all(|value| value.is_finite() && value.abs() <= 10_000_000.0)
            && self.width > 0.0
            && self.height > 0.0
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WindowMonitor {
    pub name: Option<String>,
    pub bounds: WindowRect,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WindowFrame {
    pub bounds: WindowRect,
    pub monitor: WindowMonitor,
    pub maximized: bool,
}

impl WindowFrame {
    pub fn is_valid(&self) -> bool {
        self.bounds.is_valid()
            && self.monitor.bounds.is_valid()
            && self
                .monitor
                .name
                .as_ref()
                .is_none_or(|name| name.len() <= 1024)
    }
}

impl SqliteStore {
    pub fn main_window_frame(&self) -> Option<WindowFrame> {
        let frame: WindowFrame = serde_json::from_str(&self.app_setting(KEY)?).ok()?;
        frame.is_valid().then_some(frame)
    }

    /// Admission is coalesced by the settings actor; its normal shutdown
    /// durability barrier commits this record along with the session.
    pub fn save_main_window_frame(&self, frame: &WindowFrame) -> bool {
        frame.is_valid()
            && serde_json::to_string(frame)
                .is_ok_and(|value| self.set_app_setting(KEY.into(), value))
    }
}
