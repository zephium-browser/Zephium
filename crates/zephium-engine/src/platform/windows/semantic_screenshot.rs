#![deny(unsafe_op_in_unsafe_fn)]
#![deny(clippy::undocumented_unsafe_blocks)]
#![deny(clippy::dbg_macro, clippy::print_stderr, clippy::print_stdout)]
#![cfg_attr(
    not(test),
    deny(clippy::panic, clippy::unreachable, clippy::unwrap_used)
)]

//! Bounded WebView2 viewport capture for an exact owned document.
//!
//! `CapturePreview` writes directly into a custom Rust-owned `IStream`. The
//! stream refuses every byte beyond the request ceiling before allocation and
//! never delegates to generic system storage or a filesystem stream. The
//! functional core remains responsible for full PNG structure/CRC validation
//! and in-place metadata removal.

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::panic::AssertUnwindSafe;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2CapturePreviewCompletedHandler, ICoreWebView2CapturePreviewCompletedHandler_Impl,
    COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG,
};
use windows::Win32::Foundation::{
    E_NOTIMPL, STG_E_INVALIDFUNCTION, STG_E_INVALIDPOINTER, STG_E_MEDIUMFULL,
};
use windows::Win32::System::Com::{
    ISequentialStream_Impl, IStream, IStream_Impl, LOCKTYPE, STATFLAG, STATSTG, STGC, STGM_WRITE,
    STGTY_STREAM, STREAM_SEEK, STREAM_SEEK_CUR, STREAM_SEEK_END, STREAM_SEEK_SET,
};
use windows_core::HRESULT;
use wry::WebViewExtWindows as _;
use zephium_agentic::{
    SemanticCaptureInstant, SemanticScreenshotNativeCapture, SemanticScreenshotNativeFailure,
    SemanticScreenshotNativeRequest, SemanticScreenshotPaintEvidence,
};

use crate::platform::agent_screenshot_buffer::{
    bounded_png_dimensions, BoundedScreenshotBuffer, PngHeaderFailure, ScreenshotBufferFailure,
    ScreenshotSeekOrigin,
};

type NativeCompletion = Box<
    dyn FnOnce(Result<SemanticScreenshotNativeCapture, SemanticScreenshotNativeFailure>) + 'static,
>;
type PanicCallback = Rc<dyn Fn() + 'static>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum StreamFailure {
    ResourceExhausted,
    Transport,
}

pub(super) struct SharedCaptureStream {
    pub(super) buffer: RefCell<BoundedScreenshotBuffer>,
    pub(super) failure: Cell<Option<StreamFailure>>,
}

impl SharedCaptureStream {
    pub(super) fn new(limit: usize) -> Self {
        Self {
            buffer: RefCell::new(BoundedScreenshotBuffer::new(limit)),
            failure: Cell::new(None),
        }
    }

    fn record(&self, failure: StreamFailure) {
        if self.failure.get().is_none() {
            self.failure.set(Some(failure));
        }
    }

    fn reject_hresult(&self) -> HRESULT {
        match self.failure.get() {
            Some(StreamFailure::ResourceExhausted) => STG_E_MEDIUMFULL,
            Some(StreamFailure::Transport) | None => STG_E_INVALIDFUNCTION,
        }
    }
}

#[windows_core::implement(IStream)]
pub(super) struct BoundedCaptureStream {
    pub(super) shared: Rc<SharedCaptureStream>,
}

impl ISequentialStream_Impl for BoundedCaptureStream_Impl {
    fn Read(&self, _pv: *mut c_void, _cb: u32, pcbread: *mut u32) -> HRESULT {
        if !pcbread.is_null() {
            // SAFETY: COM promises a writable `ULONG` when this optional out
            // pointer is non-null.
            unsafe { pcbread.write(0) };
        }
        self.shared.record(StreamFailure::Transport);
        E_NOTIMPL
    }

    fn Write(&self, pv: *const c_void, cb: u32, pcbwritten: *mut u32) -> HRESULT {
        if !pcbwritten.is_null() {
            // SAFETY: COM promises a writable `ULONG` when this optional out
            // pointer is non-null.
            unsafe { pcbwritten.write(0) };
        }
        if pv.is_null() {
            self.shared.record(StreamFailure::Transport);
            return STG_E_INVALIDPOINTER;
        }
        if self.shared.failure.get().is_some() {
            return self.shared.reject_hresult();
        }
        let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| {
            // SAFETY: `ISequentialStream::Write` promises `pv` addresses `cb`
            // readable bytes for this call; the null case was rejected above.
            let source = unsafe { std::slice::from_raw_parts(pv.cast::<u8>(), cb as usize) };
            let mut buffer = self
                .shared
                .buffer
                .try_borrow_mut()
                .map_err(|_| StreamFailure::Transport)?;
            buffer.write(source).map_err(map_buffer_failure)
        }));
        match outcome {
            Ok(Ok(written)) => {
                if !pcbwritten.is_null() {
                    // SAFETY: same optional COM out-pointer contract as above.
                    unsafe { pcbwritten.write(written as u32) };
                }
                HRESULT(0)
            }
            Ok(Err(failure)) => {
                self.shared.record(failure);
                self.shared.reject_hresult()
            }
            Err(_) => {
                self.shared.record(StreamFailure::Transport);
                STG_E_INVALIDFUNCTION
            }
        }
    }
}

impl IStream_Impl for BoundedCaptureStream_Impl {
    fn Seek(
        &self,
        dlibmove: i64,
        dworigin: STREAM_SEEK,
        plibnewposition: *mut u64,
    ) -> windows_core::Result<()> {
        let origin = if dworigin == STREAM_SEEK_SET {
            ScreenshotSeekOrigin::Start
        } else if dworigin == STREAM_SEEK_CUR {
            ScreenshotSeekOrigin::Current
        } else if dworigin == STREAM_SEEK_END {
            ScreenshotSeekOrigin::End
        } else {
            self.shared.record(StreamFailure::Transport);
            return Err(windows_core::Error::from_hresult(STG_E_INVALIDFUNCTION));
        };
        let position = self.run_mut(|buffer| buffer.seek(dlibmove, origin))?;
        if !plibnewposition.is_null() {
            // SAFETY: COM promises a writable `ULARGE_INTEGER` when this
            // optional out pointer is non-null.
            unsafe { plibnewposition.write(position as u64) };
        }
        Ok(())
    }

    fn SetSize(&self, libnewsize: u64) -> windows_core::Result<()> {
        let length = usize::try_from(libnewsize).map_err(|_| {
            self.shared.record(StreamFailure::ResourceExhausted);
            windows_core::Error::from_hresult(STG_E_MEDIUMFULL)
        })?;
        self.run_mut(|buffer| buffer.set_len(length))
    }

    fn CopyTo(
        &self,
        _pstm: windows_core::Ref<'_, IStream>,
        _cb: u64,
        pcbread: *mut u64,
        pcbwritten: *mut u64,
    ) -> windows_core::Result<()> {
        if !pcbread.is_null() {
            // SAFETY: COM promises writable optional count out-pointers.
            unsafe { pcbread.write(0) };
        }
        if !pcbwritten.is_null() {
            // SAFETY: COM promises writable optional count out-pointers.
            unsafe { pcbwritten.write(0) };
        }
        self.unsupported()
    }

    fn Commit(&self, _grfcommitflags: &STGC) -> windows_core::Result<()> {
        if self.shared.failure.get().is_some() {
            return Err(windows_core::Error::from_hresult(
                self.shared.reject_hresult(),
            ));
        }
        Ok(())
    }

    fn Revert(&self) -> windows_core::Result<()> {
        self.unsupported()
    }

    fn LockRegion(
        &self,
        _liboffset: u64,
        _cb: u64,
        _dwlocktype: &LOCKTYPE,
    ) -> windows_core::Result<()> {
        self.unsupported()
    }

    fn UnlockRegion(
        &self,
        _liboffset: u64,
        _cb: u64,
        _dwlocktype: u32,
    ) -> windows_core::Result<()> {
        self.unsupported()
    }

    fn Stat(&self, pstatstg: *mut STATSTG, _grfstatflag: &STATFLAG) -> windows_core::Result<()> {
        if pstatstg.is_null() {
            self.shared.record(StreamFailure::Transport);
            return Err(windows_core::Error::from_hresult(STG_E_INVALIDPOINTER));
        }
        if self.shared.failure.get().is_some() {
            return Err(windows_core::Error::from_hresult(
                self.shared.reject_hresult(),
            ));
        }
        let length = std::panic::catch_unwind(AssertUnwindSafe(|| {
            self.shared
                .buffer
                .try_borrow()
                .map(|buffer| buffer.len())
                .map_err(|_| StreamFailure::Transport)
        }))
        .map_err(|_| StreamFailure::Transport)
        .and_then(std::convert::identity)
        .map_err(|failure| self.stream_error(failure))?;
        let stat = STATSTG {
            r#type: STGTY_STREAM.0 as u32,
            cbSize: length as u64,
            grfMode: STGM_WRITE,
            ..STATSTG::default()
        };
        // SAFETY: the null case was rejected and COM promises writable
        // `STATSTG` storage for this call.
        unsafe { pstatstg.write(stat) };
        Ok(())
    }

    fn Clone(&self) -> windows_core::Result<IStream> {
        self.unsupported()
    }
}

impl BoundedCaptureStream_Impl {
    fn run_mut<T>(
        &self,
        operation: impl FnOnce(&mut BoundedScreenshotBuffer) -> Result<T, ScreenshotBufferFailure>,
    ) -> windows_core::Result<T> {
        if self.shared.failure.get().is_some() {
            return Err(windows_core::Error::from_hresult(
                self.shared.reject_hresult(),
            ));
        }
        let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| {
            let mut buffer = self
                .shared
                .buffer
                .try_borrow_mut()
                .map_err(|_| StreamFailure::Transport)?;
            operation(&mut buffer).map_err(map_buffer_failure)
        }))
        .map_err(|_| StreamFailure::Transport)
        .and_then(std::convert::identity);
        outcome.map_err(|failure| self.stream_error(failure))
    }

    fn unsupported<T>(&self) -> windows_core::Result<T> {
        self.shared.record(StreamFailure::Transport);
        Err(windows_core::Error::from_hresult(E_NOTIMPL))
    }

    fn stream_error(&self, failure: StreamFailure) -> windows_core::Error {
        self.shared.record(failure);
        windows_core::Error::from_hresult(self.shared.reject_hresult())
    }
}

fn map_buffer_failure(failure: ScreenshotBufferFailure) -> StreamFailure {
    match failure {
        ScreenshotBufferFailure::Limit | ScreenshotBufferFailure::Allocation => {
            StreamFailure::ResourceExhausted
        }
        ScreenshotBufferFailure::Invalid => StreamFailure::Transport,
    }
}

struct PendingCapture {
    request: SemanticScreenshotNativeRequest,
    completion: NativeCompletion,
    admitted_at: Instant,
    started_at: SemanticCaptureInstant,
    cancelled: Arc<AtomicBool>,
    shared: Rc<SharedCaptureStream>,
    // Retain the exact COM stream until the native completion callback even if
    // a WebView2 build releases its call-site reference early.
    _stream: IStream,
}

#[windows_core::implement(ICoreWebView2CapturePreviewCompletedHandler)]
struct CaptureCompletion {
    pending: Rc<RefCell<Option<PendingCapture>>>,
    callback_panicked: PanicCallback,
}

impl ICoreWebView2CapturePreviewCompletedHandler_Impl for CaptureCompletion_Impl {
    fn Invoke(&self, errorcode: HRESULT) -> windows_core::Result<()> {
        let pending = self
            .pending
            .try_borrow_mut()
            .ok()
            .and_then(|mut pending| pending.take());
        let Some(pending) = pending else {
            signal_callback_panic(&self.callback_panicked);
            return Ok(());
        };
        let PendingCapture {
            request,
            completion,
            admitted_at,
            started_at,
            cancelled,
            shared,
            _stream,
        } = pending;
        let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| {
            finish_capture(
                request,
                admitted_at,
                started_at,
                &cancelled,
                &shared,
                errorcode,
            )
        }))
        .unwrap_or(Err(SemanticScreenshotNativeFailure::Transport));
        if std::panic::catch_unwind(AssertUnwindSafe(|| completion(outcome))).is_err() {
            signal_callback_panic(&self.callback_panicked);
        }
        Ok(())
    }
}

pub(super) fn capture_viewport(
    webview: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2,
    request: SemanticScreenshotNativeRequest,
    admitted_at: Instant,
    cancelled: Arc<AtomicBool>,
    completion: impl FnOnce(Result<SemanticScreenshotNativeCapture, SemanticScreenshotNativeFailure>)
        + 'static,
    callback_panicked: impl Fn() + 'static,
) -> Result<(), SemanticScreenshotNativeFailure> {
    if cancelled.load(Ordering::Acquire) {
        return Err(SemanticScreenshotNativeFailure::Cancelled);
    }
    let started_at = mapped_instant(request.requested_at(), admitted_at, Instant::now())
        .ok_or(SemanticScreenshotNativeFailure::TimedOut)?;
    if started_at > request.deadline() {
        return Err(SemanticScreenshotNativeFailure::TimedOut);
    }
    let limit = usize::try_from(request.budget().max_png_bytes())
        .map_err(|_| SemanticScreenshotNativeFailure::ResourceExhausted)?;
    let shared = Rc::new(SharedCaptureStream::new(limit));
    let stream: IStream = BoundedCaptureStream {
        shared: shared.clone(),
    }
    .into();
    let callback_panicked: PanicCallback = Rc::new(callback_panicked);
    let pending = Rc::new(RefCell::new(Some(PendingCapture {
        request,
        completion: Box::new(completion),
        admitted_at,
        started_at,
        cancelled,
        shared: shared.clone(),
        _stream: stream.clone(),
    })));
    let handler: ICoreWebView2CapturePreviewCompletedHandler = CaptureCompletion {
        pending: pending.clone(),
        callback_panicked,
    }
    .into();
    // SAFETY: the live WebView2, stream, and completion handler belong to the
    // same STA. WebView2 AddRefs the two COM arguments for asynchronous use;
    // `PendingCapture` also retains the stream until the one completion.
    let dispatched = unsafe {
        webview.CapturePreview(
            COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG,
            &stream,
            &handler,
        )
    };
    if dispatched.is_err() {
        let unsettled = pending
            .try_borrow_mut()
            .ok()
            .and_then(|mut pending| pending.take());
        if unsettled.is_none() {
            // A synchronous callback already consumed the exact completion.
            return Ok(());
        }
        return Err(map_stream_terminal(shared.failure.get()));
    }
    Ok(())
}

fn finish_capture(
    request: SemanticScreenshotNativeRequest,
    admitted_at: Instant,
    started_at: SemanticCaptureInstant,
    cancelled: &AtomicBool,
    shared: &SharedCaptureStream,
    errorcode: HRESULT,
) -> Result<SemanticScreenshotNativeCapture, SemanticScreenshotNativeFailure> {
    if cancelled.load(Ordering::Acquire) {
        return Err(SemanticScreenshotNativeFailure::Cancelled);
    }
    let completed_at = mapped_instant(request.requested_at(), admitted_at, Instant::now())
        .ok_or(SemanticScreenshotNativeFailure::TimedOut)?;
    if completed_at > request.deadline() {
        return Err(SemanticScreenshotNativeFailure::TimedOut);
    }
    if errorcode.is_err() || shared.failure.get().is_some() {
        return Err(map_stream_terminal(shared.failure.get()));
    }
    let png = shared
        .buffer
        .try_borrow_mut()
        .map_err(|_| SemanticScreenshotNativeFailure::Transport)?
        .take_bytes();
    if png.is_empty() {
        return Err(SemanticScreenshotNativeFailure::Transport);
    }
    let (width, height) =
        bounded_png_dimensions(&png, request.budget()).map_err(|failure| match failure {
            PngHeaderFailure::Malformed => SemanticScreenshotNativeFailure::Transport,
            PngHeaderFailure::ResourceExhausted => {
                SemanticScreenshotNativeFailure::ResourceExhausted
            }
        })?;
    if cancelled.load(Ordering::Acquire) {
        return Err(SemanticScreenshotNativeFailure::Cancelled);
    }
    Ok(request.complete(
        SemanticScreenshotPaintEvidence::ExactDocumentContentAvailable,
        started_at,
        completed_at,
        width,
        height,
        png,
    ))
}

fn map_stream_terminal(failure: Option<StreamFailure>) -> SemanticScreenshotNativeFailure {
    match failure {
        Some(StreamFailure::ResourceExhausted) => {
            SemanticScreenshotNativeFailure::ResourceExhausted
        }
        Some(StreamFailure::Transport) | None => SemanticScreenshotNativeFailure::Transport,
    }
}

fn mapped_instant(
    requested_at: SemanticCaptureInstant,
    admitted_at: Instant,
    now: Instant,
) -> Option<SemanticCaptureInstant> {
    let elapsed = u64::try_from(now.saturating_duration_since(admitted_at).as_millis()).ok()?;
    requested_at
        .millis()
        .checked_add(elapsed)
        .map(SemanticCaptureInstant::from_millis)
}

fn signal_callback_panic(callback: &PanicCallback) {
    let _ = std::panic::catch_unwind(AssertUnwindSafe(|| callback()));
}

#[path = "work_frame_image.rs"]
mod work_frame_image;

type FrameCompletion = Box<dyn FnOnce(work_frame_image::FrameImage)>;
const MAX_FRAME_IMAGE_WORKERS: usize = 2;
static FRAME_IMAGE_WORKERS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

// At most eight source streams/byte jobs and two active decoders globally.
// Each stream owns <=16MiB; each decoder has <=48MiB codec working allocation,
// <=35.2MiB RGBA, <=1.6MiB resized pixels and <=1MiB encoded output. These are
// separate limits, not a claim that max_alloc bounds the entire codec pipeline.
const MAX_FRAME_CAPTURE_JOBS: usize = 8;
static FRAME_CAPTURE_JOBS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
struct FrameCaptureJob;
impl FrameCaptureJob {
    fn acquire() -> Option<Self> {
        FRAME_CAPTURE_JOBS
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < MAX_FRAME_CAPTURE_JOBS).then_some(count + 1)
            })
            .ok()
            .map(|_| Self)
    }
}
impl Drop for FrameCaptureJob {
    fn drop(&mut self) {
        FRAME_CAPTURE_JOBS.fetch_sub(1, Ordering::AcqRel);
    }
}

fn complete_frame(completion: FrameCompletion, image: work_frame_image::FrameImage) {
    let _ = std::panic::catch_unwind(AssertUnwindSafe(|| completion(image)));
}

struct FrameImageWorker;
impl FrameImageWorker {
    fn acquire() -> Option<Self> {
        FRAME_IMAGE_WORKERS
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < MAX_FRAME_IMAGE_WORKERS).then_some(count + 1)
            })
            .ok()
            .map(|_| Self)
    }
}
impl Drop for FrameImageWorker {
    fn drop(&mut self) {
        FRAME_IMAGE_WORKERS.fetch_sub(1, Ordering::AcqRel);
    }
}

/// The worker owns bytes only. The original completion and its document/resource
/// fences remain on the originating STA, and capture debt remains set until it
/// is delivered here. No native object or non-Send callback crosses threads.
struct FrameDelivery {
    reply: std::sync::mpsc::Receiver<work_frame_image::FrameImage>,
    completion: RefCell<Option<FrameCompletion>>,
    timer: RefCell<Option<super::ContentPolicyTimeout>>,
}
impl FrameDelivery {
    fn finish(&self, image: work_frame_image::FrameImage) {
        self.timer.borrow_mut().take();
        let completion = self.completion.borrow_mut().take();
        if let Some(completion) = completion {
            complete_frame(completion, image);
        }
    }
    fn poll(self: Rc<Self>) {
        self.timer.borrow_mut().take();
        match self.reply.try_recv() {
            Ok(image) => self.finish(image),
            Err(std::sync::mpsc::TryRecvError::Disconnected) => self.finish(None),
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                let next = self.clone();
                let timer = super::schedule_content_policy_timeout(
                    std::time::Duration::from_millis(16),
                    move || next.poll(),
                );
                if timer.is_some() {
                    *self.timer.borrow_mut() = timer;
                } else {
                    self.finish(None);
                }
            }
        }
    }
}

fn deliver_frame_thumbnail(
    png: Vec<u8>,
    dimensions: (u32, u32),
    completion: FrameCompletion,
    capture_job: FrameCaptureJob,
    crop: Option<crate::platform::record_media::Crop>,
) {
    let Some(permit) = FrameImageWorker::acquire() else {
        complete_frame(completion, None);
        return;
    };
    let (send, reply) = std::sync::mpsc::sync_channel(1);
    let worker = std::thread::Builder::new()
        .name("work-frame-image".into())
        .spawn(move || {
            let _permit = permit;
            let _capture_job = capture_job;
            let image = std::panic::catch_unwind(AssertUnwindSafe(|| match crop {
                Some(crop) => work_frame_image::region(png, dimensions, crop),
                None => work_frame_image::thumbnail(png, dimensions),
            }))
            .ok()
            .flatten();
            let _ = send.send(image);
        });
    if worker.is_err() {
        complete_frame(completion, None);
        return;
    }
    // Dropping the JoinHandle detaches this finite, allocation-bounded byte
    // job; the receiver owns its completion and retains the host capture debt.
    drop(worker);
    Rc::new(FrameDelivery {
        reply,
        completion: RefCell::new(Some(completion)),
        timer: RefCell::new(None),
    })
    .poll();
}

/// A canvas frame uses a separate bounded person-image budget; semantic evidence is unchanged.
pub(crate) fn capture_work_frame(
    view: &wry::WebView,
    completion: impl FnOnce(Option<(u32, u32, Vec<u8>)>) + 'static,
) -> bool {
    capture_work_frame_native(&view.webview(), &view.controller(), completion)
}
pub(crate) fn capture_work_frame_native(
    core: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2,
    controller: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Controller,
    completion: impl FnOnce(Option<(u32, u32, Vec<u8>)>) + 'static,
) -> bool {
    capture_person_frame_native(core, controller, None, completion)
}
/// WebView2 has no region CapturePreview API. Its existing bounded native
/// stream/worker crops before encoding; only the admitted ROI is returned.
pub(crate) fn capture_record_frame_native(
    core: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2,
    controller: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Controller,
    crop: crate::platform::record_media::Crop,
    completion: crate::platform::record_media::FrameCompletion,
) -> bool {
    let Some(dimensions) = frame_dimensions(controller) else {
        completion(None);
        return false;
    };
    let Some(geometry) = work_frame_image::region_geometry(dimensions, crop) else {
        completion(None);
        return false;
    };
    capture_person_frame_native(core, controller, Some((crop, dimensions)), move |image| {
        completion(
            image.map(|(_, _, png)| crate::platform::record_media::RecordFrame { png, geometry }),
        );
    })
}
fn frame_dimensions(
    controller: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Controller,
) -> Option<(u32, u32)> {
    let mut bounds = windows::Win32::Foundation::RECT::default();
    // SAFETY: the exact controller and initialized output stay on their owning STA.
    unsafe { controller.Bounds(&mut bounds) }.ok()?;
    Some((
        u32::try_from(bounds.right.checked_sub(bounds.left)?).ok()?,
        u32::try_from(bounds.bottom.checked_sub(bounds.top)?).ok()?,
    ))
}
fn capture_person_frame_native(
    core: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2,
    controller: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Controller,
    record: Option<(crate::platform::record_media::Crop, (u32, u32))>,
    completion: impl FnOnce(Option<(u32, u32, Vec<u8>)>) + 'static,
) -> bool {
    let mut bounds = windows::Win32::Foundation::RECT::default();
    // SAFETY: the exact controller and initialized output remain on its owning
    // STA. Bounds are physical pixels, including the host's DPI conversion.
    let bounds_ok = unsafe { controller.Bounds(&mut bounds) }.is_ok();
    let dimensions = (
        bounds
            .right
            .checked_sub(bounds.left)
            .and_then(|n| u32::try_from(n).ok()),
        bounds
            .bottom
            .checked_sub(bounds.top)
            .and_then(|n| u32::try_from(n).ok()),
    );
    let (Some(width), Some(height)) = dimensions else {
        complete_frame(Box::new(completion), None);
        return false;
    };
    if !bounds_ok || !work_frame_image::input_dimensions(width, height) {
        complete_frame(Box::new(completion), None);
        return false;
    }
    let dimensions = (width, height);
    if record
        .as_ref()
        .is_some_and(|(_, expected)| *expected != dimensions)
    {
        complete_frame(Box::new(completion), None);
        return false;
    }
    let Some(capture_job) = FrameCaptureJob::acquire() else {
        complete_frame(Box::new(completion), None);
        return false;
    };
    let shared = Rc::new(SharedCaptureStream::new(work_frame_image::RAW_PNG_BYTES));
    let stream: IStream = BoundedCaptureStream {
        shared: shared.clone(),
    }
    .into();
    let pending = Rc::new(RefCell::new(Some((
        Box::new(completion) as Box<dyn FnOnce(Option<(u32, u32, Vec<u8>)>)>,
        stream.clone(),
        shared,
        capture_job,
    ))));
    let callback = pending.clone();
    let handler = webview2_com::CapturePreviewCompletedHandler::create(Box::new(move |result| {
        let Some((completion, _stream, shared, capture_job)) = callback
            .try_borrow_mut()
            .ok()
            .and_then(|mut pending| pending.take())
        else {
            return Ok(());
        };
        let png = if result.is_ok() && shared.failure.get().is_none() {
            shared
                .buffer
                .try_borrow_mut()
                .ok()
                .map(|mut buffer| buffer.take_bytes())
        } else {
            None
        };
        if let Some(png) = png {
            deliver_frame_thumbnail(
                png,
                dimensions,
                completion,
                capture_job,
                record.map(|(crop, _)| crop),
            );
        } else {
            complete_frame(completion, None);
        }
        Ok(())
    }));
    // SAFETY: exact live WebView2/stream/handler on the same STA. The callback retains its bounded stream through completion.
    let dispatched = unsafe {
        core.CapturePreview(
            COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG,
            &stream,
            &handler,
        )
        .is_ok()
    };
    if !dispatched {
        if let Some((completion, _, _, _)) = pending.borrow_mut().take() {
            complete_frame(completion, None);
        }
    }
    dispatched
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use windows::Win32::Foundation::STG_E_MEDIUMFULL;
    use windows::Win32::System::Com::{
        STATFLAG_NONAME, STATSTG, STGM_WRITE, STGTY_STREAM, STREAM_SEEK_SET,
    };

    #[test]
    fn frame_delivery_disconnect_finishes_once_on_the_originating_thread() {
        let (send, reply) = std::sync::mpsc::sync_channel(1);
        drop(send);
        let count = Rc::new(Cell::new(0));
        let called = count.clone();
        let original_thread = std::thread::current().id();
        let delivery = Rc::new(FrameDelivery {
            reply,
            completion: RefCell::new(Some(Box::new(move |image| {
                assert!(image.is_none());
                assert_eq!(std::thread::current().id(), original_thread);
                called.set(called.get() + 1);
            }))),
            timer: RefCell::new(None),
        });
        delivery.clone().poll();
        delivery.finish(None);
        assert_eq!(count.get(), 1);
        assert!(delivery.timer.borrow().is_none());
    }

    #[test]
    fn frame_completion_is_taken_before_reentry_and_contains_panics() {
        let (_send, reply) = std::sync::mpsc::sync_channel(1);
        let delivery = Rc::new_cyclic(|weak: &std::rc::Weak<FrameDelivery>| {
            let reenter = weak.clone();
            FrameDelivery {
                reply,
                completion: RefCell::new(Some(Box::new(move |_| {
                    reenter.upgrade().unwrap().finish(None);
                    panic!("fixture callback panic");
                }))),
                timer: RefCell::new(None),
            }
        });
        assert!(std::panic::catch_unwind(AssertUnwindSafe(|| delivery.finish(None))).is_ok());
        assert!(delivery.completion.borrow().is_none());
        assert!(std::panic::catch_unwind(AssertUnwindSafe(|| {
            complete_frame(Box::new(|_| panic!("fixture refusal callback panic")), None);
        }))
        .is_ok());
    }
    fn stream(limit: usize) -> (Rc<SharedCaptureStream>, IStream) {
        let shared = Rc::new(SharedCaptureStream::new(limit));
        let stream = BoundedCaptureStream {
            shared: shared.clone(),
        }
        .into();
        (shared, stream)
    }

    #[test]
    fn com_write_seek_and_stat_preserve_the_exact_bounded_stream() {
        let (shared, stream) = stream(8);
        let first = [1_u8, 2, 3, 4];
        let mut written = 0_u32;
        assert_eq!(
            // SAFETY: `first` is readable for the declared length and `written`
            // supplies valid ULONG out storage for the synchronous COM call.
            unsafe {
                stream.Write(
                    first.as_ptr().cast(),
                    first.len() as u32,
                    Some(&mut written),
                )
            },
            HRESULT(0)
        );
        assert_eq!(written, 4);
        let mut position = 0_u64;
        // SAFETY: the stream is live and `position` is exact writable ULARGE_INTEGER storage.
        unsafe { stream.Seek(1, STREAM_SEEK_SET, Some(&mut position)) }.expect("seek");
        assert_eq!(position, 1);
        let replacement = [9_u8, 8];
        assert_eq!(
            // SAFETY: `replacement` is readable for the declared length and
            // `written` remains valid ULONG out storage.
            unsafe {
                stream.Write(
                    replacement.as_ptr().cast(),
                    replacement.len() as u32,
                    Some(&mut written),
                )
            },
            HRESULT(0)
        );
        let mut stat = STATSTG::default();
        // SAFETY: `stat` is initialized writable STATSTG storage for this synchronous call.
        unsafe { stream.Stat(&mut stat, STATFLAG_NONAME) }.expect("stat");
        assert_eq!(stat.r#type, STGTY_STREAM.0 as u32);
        assert_eq!(stat.cbSize, 4);
        assert_eq!(stat.grfMode, STGM_WRITE);
        assert_eq!(shared.buffer.borrow_mut().take_bytes(), vec![1, 9, 8, 4]);
    }

    #[test]
    fn com_write_refuses_before_byte_overrun_and_stays_failed() {
        let (shared, stream) = stream(4);
        let exact = [1_u8, 2, 3, 4];
        let mut written = 0_u32;
        assert_eq!(
            // SAFETY: `exact` is readable for the declared length and `written`
            // supplies valid ULONG out storage for the synchronous COM call.
            unsafe {
                stream.Write(
                    exact.as_ptr().cast(),
                    exact.len() as u32,
                    Some(&mut written),
                )
            },
            HRESULT(0)
        );
        assert_eq!(written, 4);
        assert_eq!(
            // SAFETY: `exact` remains readable and `written` remains valid out storage.
            unsafe { stream.Write(exact.as_ptr().cast(), 1, Some(&mut written)) },
            STG_E_MEDIUMFULL
        );
        assert_eq!(written, 0);
        assert_eq!(shared.failure.get(), Some(StreamFailure::ResourceExhausted));
        assert_eq!(shared.buffer.borrow().len(), 4);
        assert_eq!(
            // SAFETY: the stream is live; this seek supplies no optional out pointer.
            unsafe { stream.Seek(0, STREAM_SEEK_SET, None) }
                .expect_err("sticky stream failure")
                .code(),
            STG_E_MEDIUMFULL
        );
    }
}
