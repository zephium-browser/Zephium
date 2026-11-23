//! One reserved download-coordinator timer per UI thread, separate from agent capacity.
use std::cell::RefCell;
use std::marker::PhantomData;
use std::rc::Rc;
use std::time::Duration;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer};
type Callback = Box<dyn FnOnce()>;
thread_local! {static PENDING:RefCell<Option<(usize,Callback)>>=const{RefCell::new(None)};}
pub(in crate::host::downloads) struct DownloadTimer {
    id: usize,
    _thread: PhantomData<Rc<()>>,
}
impl Drop for DownloadTimer {
    fn drop(&mut self) {
        unsafe {
            let _ = KillTimer(None, self.id);
        }
        let _ = PENDING.try_with(|slot| {
            let Ok(mut slot) = slot.try_borrow_mut() else {
                return;
            };
            if slot.as_ref().is_some_and(|(id, _)| *id == self.id) {
                slot.take();
            }
        });
    }
}
unsafe extern "system" fn fired(_: HWND, _: u32, id: usize, _: u32) {
    let callback = PENDING
        .try_with(|slot| {
            let Ok(mut slot) = slot.try_borrow_mut() else {
                return None;
            };
            if slot.as_ref().is_some_and(|(expected, _)| *expected == id) {
                slot.take().map(|(_, callback)| callback)
            } else {
                None
            }
        })
        .ok()
        .flatten();
    if let Some(callback) = callback {
        unsafe {
            let _ = KillTimer(None, id);
        }
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(callback));
    }
}
pub(super) fn schedule(
    duration: Duration,
    callback: impl FnOnce() + 'static,
) -> Option<DownloadTimer> {
    if !PENDING.with(|slot| slot.borrow().is_none()) {
        return None;
    }
    let milliseconds = u32::try_from(duration.as_millis()).ok()?.max(10);
    // SAFETY: fixed ABI callback and a window-less timer on this UI thread.
    let id = unsafe { SetTimer(None, 0, milliseconds, Some(fired)) };
    if id == 0 {
        return None;
    }
    PENDING.with(|slot| *slot.borrow_mut() = Some((id, Box::new(callback))));
    Some(DownloadTimer {
        id,
        _thread: PhantomData,
    })
}
