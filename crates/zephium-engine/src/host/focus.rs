use super::*;
use zephium_core::time::FocusGate;

pub(super) type SharedFocusGate = std::sync::Arc<std::sync::RwLock<Option<FocusGate>>>;

/// Whether a focus round shuts this top-level target right now.
pub(super) fn focus_shuts(gate: &SharedFocusGate, target: &str) -> bool {
    let Ok(url) = url::Url::parse(target) else {
        return false;
    };
    if !matches!(url.scheme(), "http" | "https") {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        });
    gate.read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .as_ref()
        .is_some_and(|gate| gate.blocks(host, now_ms))
}

impl EngineHost {
    pub(crate) fn set_focus_gate(&mut self, gate: Option<FocusGate>) {
        *self
            .focus_gate
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = gate;
    }

    pub(crate) fn set_media_suspended(&self, id: ItemId, suspended: bool) {
        if let Some(view) = self.views.get(&id) {
            #[cfg(target_os = "windows")]
            if let Some(audio) = &view.audio_observer {
                audio.set_focus_muted(suspended);
                return;
            }
            crate::platform::imp::set_media_suspended(view, suspended);
        }
    }

    /// The person's mute for this view; it holds across its navigations.
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    pub(crate) fn set_muted(&self, id: ItemId, muted: bool) {
        let Some(view) = self.views.get(&id) else {
            return;
        };
        #[cfg(target_os = "macos")]
        crate::platform::macos::audio::set_muted(view, muted);
        #[cfg(target_os = "windows")]
        if let Some(audio) = &view.audio_observer {
            audio.set_muted(muted);
        }
    }
}
