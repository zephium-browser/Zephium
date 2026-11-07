//! Sound and mute for one page, from WebView2's own events. Mute is also how
//! focus stills a page here (WebView2 cannot suspend playback), so the native
//! bit is the union of the person's mute and focus's, and is put back if
//! anything else changes it.
use std::cell::Cell;
use std::rc::Rc;

use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2_8;
use webview2_com::{IsDocumentPlayingAudioChangedEventHandler, IsMutedChangedEventHandler};
use windows_core::Interface;
use wry::WebViewExtWindows;

/// Why a page is muted. Shared with the observer, which restores it.
#[derive(Default)]
pub struct MuteIntent {
    person: Cell<bool>,
    focus: Cell<bool>,
}

impl MuteIntent {
    fn wanted(&self) -> bool {
        self.person.get() || self.focus.get()
    }
}

pub struct AudioObserver {
    core: ICoreWebView2_8,
    intent: Rc<MuteIntent>,
    audio_token: i64,
    muted_token: i64,
}

impl Drop for AudioObserver {
    fn drop(&mut self) {
        // SAFETY: the exact registrations on the STA-owned core.
        unsafe {
            let _ = self
                .core
                .remove_IsDocumentPlayingAudioChanged(self.audio_token);
            let _ = self.core.remove_IsMutedChanged(self.muted_token);
        }
    }
}

impl AudioObserver {
    pub fn set_muted(&self, muted: bool) {
        self.intent.person.set(muted);
        apply(&self.core, &self.intent);
    }

    pub fn set_focus_muted(&self, muted: bool) {
        self.intent.focus.set(muted);
        apply(&self.core, &self.intent);
    }
}

fn apply(core: &ICoreWebView2_8, intent: &MuteIntent) {
    let mut muted = windows_core::BOOL::default();
    // SAFETY: STA-owned core and an initialized out value.
    if unsafe { core.IsMuted(&mut muted) }.is_ok() && muted.as_bool() == intent.wanted() {
        return;
    }
    let _ = unsafe { core.SetIsMuted(intent.wanted()) };
}

/// Reports each change of whether the page plays sound, muted or not. `None`
/// when the runtime predates ICoreWebView2_8; the page then shows no speaker
/// and cannot be muted.
pub fn install_audio_observer(
    view: &wry::WebView,
    deliver: impl Fn(bool) + 'static,
) -> Option<AudioObserver> {
    let core = view.webview().cast::<ICoreWebView2_8>().ok()?;
    let intent = Rc::new(MuteIntent::default());
    let source = core.clone();
    let audio = IsDocumentPlayingAudioChangedEventHandler::create(Box::new(move |_, _| {
        let mut playing = windows_core::BOOL::default();
        // SAFETY: STA-owned core and an initialized out value.
        if unsafe { source.IsDocumentPlayingAudio(&mut playing) }.is_ok() {
            deliver(playing.as_bool());
        }
        Ok(())
    }));
    let target = core.clone();
    let wanted = intent.clone();
    let muted = IsMutedChangedEventHandler::create(Box::new(move |_, _| {
        apply(&target, &wanted);
        Ok(())
    }));
    let mut audio_token = 0_i64;
    let mut muted_token = 0_i64;
    // SAFETY: STA-owned core; each handler captures only the interface it is
    // registered on and is removed before the controller drops.
    unsafe {
        core.add_IsDocumentPlayingAudioChanged(&audio, &mut audio_token)
            .ok()?;
        if core.add_IsMutedChanged(&muted, &mut muted_token).is_err() {
            let _ = core.remove_IsDocumentPlayingAudioChanged(audio_token);
            return None;
        }
    }
    Some(AudioObserver {
        core,
        intent,
        audio_token,
        muted_token,
    })
}
