//! Renderer memory per page, for the rule that puts heavy hidden pages to
//! sleep. One asynchronous query per profile, only when the shell asks.

use webview2_com::GetProcessExtendedInfosCompletedHandler;
use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2Environment, ICoreWebView2Environment13, ICoreWebView2FrameInfo,
    ICoreWebView2FrameInfo2, ICoreWebView2ProcessExtendedInfoCollection, ICoreWebView2_20,
    COREWEBVIEW2_PROCESS_KIND_RENDERER,
};
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::ProcessStatus::{
    GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
use windows_core::Interface;
use wry::WebViewExtWindows;

const PROCESS_LIMIT: u32 = 1_024;
const FRAME_LIMIT: usize = 4_096;
const FRAME_DEPTH_LIMIT: usize = 64;

/// The view's main-frame id, as the process query names frames. Zero until
/// the view's first navigation has started loading content.
pub(crate) fn main_frame_id(view: &wry::WebView) -> Option<u32> {
    let core = view.webview().cast::<ICoreWebView2_20>().ok()?;
    let mut id = 0_u32;
    // SAFETY: a live core on its UI thread and an initialized out value.
    unsafe { core.FrameId(&mut id) }.ok()?;
    (id != 0).then_some(id)
}

/// Each renderer process of `environment`, as its private bytes and the
/// main frames of the pages whose frames it hosts. False when this runtime
/// cannot say which process hosts which frame.
pub(crate) fn sample_renderers(
    environment: &ICoreWebView2Environment,
    done: impl FnOnce(Vec<(u64, Vec<u32>)>) + 'static,
) -> bool {
    let Ok(environment) = environment.cast::<ICoreWebView2Environment13>() else {
        return false;
    };
    let handler =
        GetProcessExtendedInfosCompletedHandler::create(Box::new(move |result, infos| {
            let renderers = match (result, infos) {
                (Ok(()), Some(infos)) => renderers(&infos),
                _ => Vec::new(),
            };
            done(renderers);
            Ok(())
        }));
    // SAFETY: the environment's own UI thread; the handler owns its state.
    unsafe { environment.GetProcessExtendedInfos(&handler) }.is_ok()
}

fn renderers(infos: &ICoreWebView2ProcessExtendedInfoCollection) -> Vec<(u64, Vec<u32>)> {
    let mut count = 0_u32;
    // SAFETY: a live collection from the completed query.
    if unsafe { infos.Count(&mut count) }.is_err() {
        return Vec::new();
    }
    let mut renderers = Vec::new();
    for index in 0..count.min(PROCESS_LIMIT) {
        // SAFETY: index is below the collection's own count.
        let Ok(info) = (unsafe { infos.GetValueAtIndex(index) }) else {
            continue;
        };
        let Ok(process) = (unsafe { info.ProcessInfo() }) else {
            continue;
        };
        let (mut kind, mut pid) = (Default::default(), 0_i32);
        // SAFETY: live process info and initialized out values.
        if unsafe { process.Kind(&mut kind) }.is_err()
            || kind != COREWEBVIEW2_PROCESS_KIND_RENDERER
            || unsafe { process.ProcessId(&mut pid) }.is_err()
        {
            continue;
        }
        let Some(bytes) = u32::try_from(pid).ok().and_then(private_bytes) else {
            continue;
        };
        let mut pages = Vec::new();
        // Keep the collection alive until its iterator is dropped. WebView2's
        // iterator borrows its backing storage; releasing `frames` inside an
        // and_then before GetCurrent can cause a native access violation.
        // SAFETY: live collection and iterator, walked to a bounded end.
        if let Ok(frames) = unsafe { info.AssociatedFrameInfos() } {
            if let Ok(iterator) = unsafe { frames.GetIterator() } {
                let mut more = windows_core::BOOL::default();
                for _ in 0..FRAME_LIMIT {
                    if unsafe { iterator.HasCurrent(&mut more) }.is_err() || !more.as_bool() {
                        break;
                    }
                    if let Some(page) = unsafe { iterator.GetCurrent() }.ok().and_then(page_of) {
                        pages.push(page);
                    }
                    if unsafe { iterator.MoveNext(&mut more) }.is_err() {
                        break;
                    }
                }
            }
        }
        renderers.push((bytes, pages));
    }
    renderers
}

/// The main frame a frame belongs to, by walking its parents.
fn page_of(frame: ICoreWebView2FrameInfo) -> Option<u32> {
    let mut frame = frame.cast::<ICoreWebView2FrameInfo2>().ok()?;
    for _ in 0..FRAME_DEPTH_LIMIT {
        // SAFETY: a live frame info snapshot; no parent ends the walk.
        match unsafe { frame.ParentFrameInfo() } {
            Ok(parent) => frame = parent.cast::<ICoreWebView2FrameInfo2>().ok()?,
            Err(_) => break,
        }
    }
    let mut id = 0_u32;
    // SAFETY: as above, with an initialized out value.
    unsafe { frame.FrameId(&mut id) }.ok()?;
    (id != 0).then_some(id)
}

/// Private commit: what the process holds of its own, paged in or not. The
/// working set would understate a hidden page, whose view WebView2 is asked
/// to keep small.
fn private_bytes(pid: u32) -> Option<u64> {
    // SAFETY: limited query rights only, no memory reads; the handle is
    // closed on every path.
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut counters = PROCESS_MEMORY_COUNTERS_EX {
            cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
            ..Default::default()
        };
        let read = GetProcessMemoryInfo(
            process,
            (&mut counters as *mut PROCESS_MEMORY_COUNTERS_EX).cast::<PROCESS_MEMORY_COUNTERS>(),
            counters.cb,
        );
        let _ = CloseHandle(process);
        read.ok()?;
        Some(counters.PrivateUsage as u64)
    }
}
