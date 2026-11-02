//! The icon WebView2 chose for a page. The web engine already finds, fetches
//! and decodes it, so this is exact and costs one event per page. The PNG it
//! hands back is bounded here and decoded by the privileged side with the same
//! bounded decoders as any fetched icon.
use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2_15, COREWEBVIEW2_FAVICON_IMAGE_FORMAT_PNG,
};
use webview2_com::{FaviconChangedEventHandler, GetFaviconCompletedHandler};
use windows::Win32::System::Com::IStream;
use windows_core::{Interface, PWSTR};
use wry::WebViewExtWindows;

/// Largest PNG accepted; a 32 px icon is a few KiB, a large one under 64 KiB.
const MAX_PNG_BYTES: usize = 256 * 1024;
const CHUNK: usize = 8 * 1024;

pub struct FaviconObserver {
    core: ICoreWebView2_15,
    token: i64,
}

impl Drop for FaviconObserver {
    fn drop(&mut self) {
        // SAFETY: the exact registration on the STA-owned core.
        let _ = unsafe { self.core.remove_FaviconChanged(self.token) };
    }
}

/// Reports each icon the engine settles on. `page` names the document an event
/// belongs to (its committed address) and is asked again when the PNG arrives,
/// so an icon fetched for a page since left is dropped. `deliver` receives the
/// address and PNG bytes. `None` when this WebView2 runtime predates the API;
/// the declared icon addresses are the fallback then.
pub fn install_favicon_observer<K: PartialEq + 'static>(
    view: &wry::WebView,
    page: impl Fn() -> Option<(K, String)> + Clone + 'static,
    deliver: impl Fn(String, Vec<u8>) + Clone + 'static,
) -> Option<FaviconObserver> {
    let core = view.webview().cast::<ICoreWebView2_15>().ok()?;
    let source = core.clone();
    let handler = FaviconChangedEventHandler::create(Box::new(move |_, _| {
        let Some(before) = page() else {
            return Ok(());
        };
        if !has_icon(&source) {
            return Ok(());
        }
        let page = page.clone();
        let deliver = deliver.clone();
        let done = GetFaviconCompletedHandler::create(Box::new(move |result, stream| {
            let Some(png) = result.ok().and(stream).and_then(|stream| read_png(&stream)) else {
                return Ok(());
            };
            if page().is_some_and(|after| after == before) {
                deliver(before.1, png);
            }
            Ok(())
        }));
        // SAFETY: STA-owned core; the completion captures neither it nor the
        // WebView.
        let _ = unsafe { source.GetFavicon(COREWEBVIEW2_FAVICON_IMAGE_FORMAT_PNG, &done) };
        Ok(())
    }));
    let mut token = 0_i64;
    // SAFETY: STA-owned core; the handler captures only the interface it was
    // registered on and is removed before the controller drops.
    unsafe { core.add_FaviconChanged(&handler, &mut token) }.ok()?;
    Some(FaviconObserver { core, token })
}

/// Whether the page has an icon address; the event also fires when it is
/// cleared on the way to another page.
fn has_icon(core: &ICoreWebView2_15) -> bool {
    let mut uri = PWSTR::null();
    // SAFETY: an initialized out pointer; the callee allocates it and
    // `take_pwstr` frees it.
    if unsafe { core.FaviconUri(&mut uri) }.is_err() || uri.is_null() {
        return false;
    }
    !webview2_com::take_pwstr(uri).is_empty()
}

fn read_png(stream: &IStream) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; CHUNK];
    loop {
        let mut read = 0_u32;
        // SAFETY: the buffer outlives the call and `read` is initialized.
        let result =
            unsafe { stream.Read(chunk.as_mut_ptr().cast(), CHUNK as u32, Some(&mut read)) };
        if result.is_err() {
            return None;
        }
        if read == 0 {
            break;
        }
        if bytes.len() + read as usize > MAX_PNG_BYTES {
            return None;
        }
        bytes.extend_from_slice(&chunk[..read as usize]);
    }
    (!bytes.is_empty()).then_some(bytes)
}
