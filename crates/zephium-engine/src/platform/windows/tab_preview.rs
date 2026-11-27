use super::semantic_screenshot::{BoundedCaptureStream, SharedCaptureStream};
use crate::platform::preview_image;
use std::{cell::RefCell, rc::Rc};
use webview2_com::Microsoft::Web::WebView2::Win32::COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_JPEG;
use windows::Win32::System::Com::IStream;
use wry::WebViewExtWindows;

type Completion = Box<dyn FnOnce(Option<Vec<u8>>) + Send>;

pub(crate) fn capture(view: &wry::WebView, done: Completion) {
    let mut bounds = windows::Win32::Foundation::RECT::default();
    let valid = unsafe { view.controller().Bounds(&mut bounds) }.is_ok();
    let dimensions = (
        bounds
            .right
            .checked_sub(bounds.left)
            .and_then(|v| u32::try_from(v).ok()),
        bounds
            .bottom
            .checked_sub(bounds.top)
            .and_then(|v| u32::try_from(v).ok()),
    );
    let (Some(width), Some(height)) = dimensions else {
        done(None);
        return;
    };
    if !valid || !preview_image::input_dimensions(width, height) {
        done(None);
        return;
    }
    let shared = Rc::new(SharedCaptureStream::new(preview_image::RAW_BYTES));
    let stream: IStream = BoundedCaptureStream {
        shared: shared.clone(),
    }
    .into();
    let pending = Rc::new(RefCell::new(Some((done, shared, stream.clone()))));
    let callback = pending.clone();
    let handler = webview2_com::CapturePreviewCompletedHandler::create(Box::new(move |result| {
        let Some((done, shared, _stream)) = callback.borrow_mut().take() else {
            return Ok(());
        };
        if result.is_err() || shared.failure.get().is_some() {
            done(None);
            return Ok(());
        }
        let bytes = shared.buffer.borrow_mut().take_bytes();
        let completion = std::sync::Arc::new(std::sync::Mutex::new(Some(done)));
        let worker_completion = completion.clone();
        if std::thread::Builder::new()
            .name("tab-preview".into())
            .spawn(move || {
                unsafe {
                    windows::Win32::System::Threading::SetThreadPriority(
                        windows::Win32::System::Threading::GetCurrentThread(),
                        windows::Win32::System::Threading::THREAD_PRIORITY_BELOW_NORMAL,
                    )
                    .ok();
                }
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    preview_image::thumbnail(bytes, (width, height))
                }))
                .ok()
                .flatten();
                if let Some(done) = worker_completion
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .take()
                {
                    done(result);
                }
            })
            .is_err()
        {
            if let Some(done) = completion.lock().unwrap_or_else(|e| e.into_inner()).take() {
                done(None);
            }
        }
        Ok(())
    }));
    let mut visible = windows_core::BOOL::default();
    if unsafe { view.controller().IsVisible(&mut visible) }.is_err()
        || !visible.as_bool()
        || unsafe {
            view.webview().CapturePreview(
                COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_JPEG,
                &stream,
                &handler,
            )
        }
        .is_err()
    {
        if let Some((done, _, _)) = pending.borrow_mut().take() {
            done(None);
        }
    }
}
