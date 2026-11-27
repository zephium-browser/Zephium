//! Runs list-update work below the priority of the pages being browsed.

/// Runs `work` at background priority on the calling thread, then restores the
/// thread's own priority, also when `work` unwinds. The compiler thread is
/// shared with the profile compiles that hold first pages, so it must never
/// stay lowered.
pub(crate) fn in_background<T>(work: impl FnOnce() -> T) -> T {
    let _restore = Lowered::begin();
    work()
}

struct Lowered {
    #[cfg(target_os = "macos")]
    previous: Option<u32>,
    #[cfg(target_os = "windows")]
    lowered: bool,
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod qos {
    pub(super) const UTILITY: u32 = 0x11;
    pub(super) const DEFAULT: u32 = 0x15;
    const UNSPECIFIED: u32 = 0x00;

    unsafe extern "C" {
        fn pthread_self() -> usize;
        fn pthread_get_qos_class_np(thread: usize, qos: *mut u32, relative: *mut i32) -> i32;
        fn pthread_set_qos_class_self_np(qos: u32, relative: i32) -> i32;
    }

    pub(super) fn current() -> Option<u32> {
        let (mut class, mut relative) = (UNSPECIFIED, 0);
        // SAFETY: both out-pointers are valid locals for the call.
        let status =
            unsafe { pthread_get_qos_class_np(pthread_self(), &raw mut class, &raw mut relative) };
        (status == 0).then_some(if class == UNSPECIFIED { DEFAULT } else { class })
    }

    pub(super) fn set(class: u32) -> bool {
        // SAFETY: plain value arguments; the call only affects this thread.
        unsafe { pthread_set_qos_class_self_np(class, 0) == 0 }
    }
}

impl Lowered {
    #[cfg(target_os = "macos")]
    fn begin() -> Self {
        let previous = qos::current().filter(|_| qos::set(qos::UTILITY));
        Self { previous }
    }

    #[cfg(target_os = "windows")]
    #[allow(unsafe_code)]
    fn begin() -> Self {
        use windows::Win32::System::Threading::{
            GetCurrentThread, SetThreadPriority, THREAD_MODE_BACKGROUND_BEGIN,
        };
        // SAFETY: the pseudo-handle names the calling thread.
        let lowered =
            unsafe { SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN) }.is_ok();
        Self { lowered }
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    fn begin() -> Self {
        Self {}
    }
}

impl Drop for Lowered {
    #[cfg(target_os = "macos")]
    fn drop(&mut self) {
        if let Some(previous) = self.previous {
            let _ = qos::set(previous);
        }
    }

    #[cfg(target_os = "windows")]
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        use windows::Win32::System::Threading::{
            GetCurrentThread, SetThreadPriority, THREAD_MODE_BACKGROUND_END,
        };
        if self.lowered {
            // SAFETY: the pseudo-handle names the calling thread.
            let _ = unsafe { SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_END) };
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    fn drop(&mut self) {}
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn the_thread_gets_its_priority_back_even_when_the_work_panics() {
        std::thread::spawn(|| {
            assert!(qos::set(0x19));
            assert_eq!(in_background(qos::current), Some(qos::UTILITY));
            assert_eq!(qos::current(), Some(0x19));
            let unwound = std::panic::catch_unwind(|| in_background(|| panic!("compile")));
            assert!(unwound.is_err());
            assert_eq!(qos::current(), Some(0x19));
        })
        .join()
        .unwrap();
    }
}
