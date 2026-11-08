use crate::platform::agent_screenshot_buffer::{
    BoundedScreenshotBuffer, ScreenshotBufferFailure, ScreenshotSeekOrigin,
};
use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::panic::AssertUnwindSafe;
use std::rc::Rc;
use windows::Win32::Foundation::{
    E_NOTIMPL, STG_E_INVALIDFUNCTION, STG_E_INVALIDPOINTER, STG_E_MEDIUMFULL,
};
use windows::Win32::System::Com::{
    ISequentialStream_Impl, IStream, IStream_Impl, LOCKTYPE, STATFLAG, STATSTG, STGC, STGM_WRITE,
    STGTY_STREAM, STREAM_SEEK, STREAM_SEEK_CUR, STREAM_SEEK_END, STREAM_SEEK_SET,
};
use windows_core::HRESULT;
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
