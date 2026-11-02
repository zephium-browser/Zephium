//! OS-driven residency input. No process sampling; only a critically low
//! state on Windows is read again periodically, to notice it easing.
#[cfg(any(target_os = "macos", target_os = "windows"))]
use tauri::Manager;

pub(crate) fn install(app: &tauri::AppHandle) {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    if let Some(shell) = app.try_state::<zephium_app::Handle>() {
        if let Some(monitor) = native::Monitor::new(shell.inner().clone()) {
            app.manage(monitor);
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let _ = app;
}

#[cfg(target_os = "macos")]
mod native {
    use dispatch2::{DispatchObject, DispatchRetained, DispatchSource};
    use zephium_app::{Command, Handle};
    use zephium_core::ports::engine::MemoryPressure;

    pub(super) struct Monitor(DispatchRetained<DispatchSource>);

    extern "C" {
        fn sysctlbyname(
            name: *const std::ffi::c_char,
            oldp: *mut std::ffi::c_void,
            oldlenp: *mut usize,
            newp: *mut std::ffi::c_void,
            newlen: usize,
        ) -> std::ffi::c_int;
    }

    /// The kernel's current level. Source data OR-merges every transition
    /// since the last callback, so a coalesced warn-then-normal would
    /// otherwise read as a warning until the next transition.
    fn current_level() -> Option<MemoryPressure> {
        let mut level: std::ffi::c_int = 0;
        let mut size = std::mem::size_of::<std::ffi::c_int>();
        // SAFETY: a read-only sysctl into a correctly sized integer.
        let status = unsafe {
            sysctlbyname(
                c"kern.memorystatus_vm_pressure_level".as_ptr(),
                (&mut level as *mut std::ffi::c_int).cast(),
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        };
        (status == 0).then_some(match level {
            4 => MemoryPressure::Critical,
            2 => MemoryPressure::Warning,
            _ => MemoryPressure::Normal,
        })
    }

    impl Monitor {
        pub(super) fn new(shell: Handle) -> Option<Self> {
            // SAFETY: this public source type takes handle 0 and the documented
            // NORMAL/WARN/CRITICAL mask; libdispatch owns its default queue.
            let source = unsafe {
                DispatchSource::new(
                    std::ptr::addr_of!(dispatch2::_dispatch_source_type_memorypressure).cast_mut(),
                    0,
                    1 | 2 | 4,
                    None,
                )
            };
            // The handler must not retain its own source. Libdispatch keeps it
            // alive for an executing handler, including cancellation races.
            let address = (&*source as *const DispatchSource) as usize;
            let initial = shell.clone();
            let handler = block2::RcBlock::new(move || {
                // SAFETY: this block is invoked only by this live source;
                // cancellation prevents future events before source disposal.
                let flags = unsafe { &*(address as *const DispatchSource) }.data();
                let pressure = if let Some(level) = current_level() {
                    level
                } else if flags & 4 != 0 {
                    MemoryPressure::Critical
                } else if flags & 2 != 0 {
                    MemoryPressure::Warning
                } else if flags & 1 != 0 {
                    MemoryPressure::Normal
                } else {
                    return;
                };
                let _ = shell.dispatch(Command::SetMemoryPressure(pressure));
            });
            // SAFETY: the source copies the block before activation. Its only
            // cross-thread state is the Send + Sync Shell handle.
            unsafe { source.set_event_handler_with_block(block2::RcBlock::as_ptr(&handler)) };
            source.activate();
            // The source reports transitions only; launching under pressure
            // must not look like a normal machine until the next change.
            if let Some(level) = current_level().filter(|level| *level != MemoryPressure::Normal) {
                let _ = initial.dispatch(Command::SetMemoryPressure(level));
            }
            Some(Self(source))
        }
    }

    impl Drop for Monitor {
        fn drop(&mut self) {
            self.0.cancel();
        }
    }
}

/// Windows signals two documented states: available memory is low, or it
/// is high (three times the low threshold). Between the two, memory is
/// tight: macOS's warning.
#[cfg(any(test, target_os = "windows"))]
fn windows_tier(low: bool, high: bool) -> zephium_core::ports::engine::MemoryPressure {
    use zephium_core::ports::engine::MemoryPressure;
    if low {
        MemoryPressure::Critical
    } else if high {
        MemoryPressure::Normal
    } else {
        MemoryPressure::Warning
    }
}

/// The notifications that can end `tier`, as (low, high, recheck). Only an
/// unsignaled notification can be waited for, so a signaled one is never in
/// the set. Leaving the low state cannot be waited for at all: while
/// critical, the state is read again after a pause.
#[cfg(any(test, target_os = "windows"))]
fn windows_watch(tier: zephium_core::ports::engine::MemoryPressure) -> (bool, bool, bool) {
    use zephium_core::ports::engine::MemoryPressure;
    match tier {
        MemoryPressure::Normal => (true, false, false),
        MemoryPressure::Warning => (true, true, false),
        MemoryPressure::Critical => (false, true, true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zephium_core::ports::engine::MemoryPressure;

    #[test]
    fn windows_tiers_follow_the_two_memory_notifications() {
        assert_eq!(windows_tier(true, false), MemoryPressure::Critical);
        assert_eq!(windows_tier(false, false), MemoryPressure::Warning);
        assert_eq!(windows_tier(false, true), MemoryPressure::Normal);
    }

    #[test]
    fn each_windows_tier_waits_only_for_what_can_end_it() {
        for (low, high) in [(true, false), (false, false), (false, true)] {
            let (watch_low, watch_high, _) = windows_watch(windows_tier(low, high));
            assert!(
                !(watch_low && low),
                "a signaled notification never returns a wait"
            );
            assert!(!(watch_high && high));
            assert!(watch_low || watch_high);
        }
        assert_eq!(windows_watch(MemoryPressure::Critical), (false, true, true));
        assert!(!windows_watch(MemoryPressure::Normal).2);
        assert!(!windows_watch(MemoryPressure::Warning).2);
    }
}

#[cfg(target_os = "windows")]
mod native {
    use std::sync::{Arc, Mutex};
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
    use windows::Win32::System::Memory::{
        CreateMemoryResourceNotification, HighMemoryResourceNotification,
        LowMemoryResourceNotification, QueryMemoryResourceNotification,
    };
    use windows::Win32::System::Threading::{
        CreateEventW, SetEvent, WaitForMultipleObjects, INFINITE,
    };
    use zephium_app::{Command, Handle};
    use zephium_core::ports::engine::MemoryPressure;

    /// How often a critical state is read again to notice it easing.
    const CRITICAL_RECHECK_MS: u32 = 10_000;

    fn signaled(event: &Event) -> Option<bool> {
        let mut state = windows::core::BOOL::default();
        // SAFETY: a live memory resource notification handle.
        unsafe { QueryMemoryResourceNotification(event.handle(), &mut state) }.ok()?;
        Some(state.as_bool())
    }

    struct Event(HANDLE);
    impl Event {
        fn handle(&self) -> HANDLE {
            self.0
        }
    }
    // SAFETY: these are owned kernel waitable handles. They are closed only
    // after the last Arc/user has gone; no GUI-thread affinity is involved.
    unsafe impl Send for Event {}
    // SAFETY: concurrent waiting/signaling is explicitly supported by Win32.
    unsafe impl Sync for Event {}
    impl Drop for Event {
        fn drop(&mut self) {
            // SAFETY: this wrapper owns exactly one successful handle creation.
            let _ = unsafe { CloseHandle(self.0) };
        }
    }

    pub(super) struct Monitor {
        stop: Arc<Event>,
        worker: Mutex<Option<std::thread::JoinHandle<()>>>,
    }

    impl Monitor {
        pub(super) fn new(shell: Handle) -> Option<Self> {
            // SAFETY: unnamed, noninherited handles; all successful creations
            // immediately acquire one RAII owner, including failure paths.
            let (stop, low, high) = unsafe {
                (
                    Arc::new(Event(CreateEventW(None, true, false, PCWSTR::null()).ok()?)),
                    Event(CreateMemoryResourceNotification(LowMemoryResourceNotification).ok()?),
                    Event(CreateMemoryResourceNotification(HighMemoryResourceNotification).ok()?),
                )
            };
            let stopped = stop.clone();
            let worker = std::thread::Builder::new()
                .name("memory-pressure".into())
                .spawn(move || {
                    let read = || Some(super::windows_tier(signaled(&low)?, signaled(&high)?));
                    let Some(mut tier) = read() else {
                        return;
                    };
                    // Launching under pressure is not a normal machine.
                    if tier != MemoryPressure::Normal {
                        let _ = shell.dispatch(Command::SetMemoryPressure(tier));
                    }
                    loop {
                        // Waiting on a signaled notification would return at
                        // once, so each tier waits only for what can end it.
                        let (watch_low, watch_high, recheck) = super::windows_watch(tier);
                        let mut handles = vec![stopped.handle()];
                        if watch_low {
                            handles.push(low.handle());
                        }
                        if watch_high {
                            handles.push(high.handle());
                        }
                        let timeout = if recheck {
                            CRITICAL_RECHECK_MS
                        } else {
                            INFINITE
                        };
                        // SAFETY: every handle remains owned across this wait;
                        // shutdown signals index zero before joining this thread.
                        let result = unsafe { WaitForMultipleObjects(&handles, false, timeout) };
                        let notified = (WAIT_OBJECT_0.0 + 1
                            ..WAIT_OBJECT_0.0 + handles.len() as u32)
                            .contains(&result.0);
                        if !notified && result != WAIT_TIMEOUT {
                            break;
                        }
                        let Some(next) = read() else {
                            break;
                        };
                        if next != tier {
                            tier = next;
                            let _ = shell.dispatch(Command::SetMemoryPressure(tier));
                        }
                    }
                })
                .ok()?;
            Some(Self {
                stop,
                worker: Mutex::new(Some(worker)),
            })
        }
    }

    impl Drop for Monitor {
        fn drop(&mut self) {
            // SAFETY: the stop handle outlives both signaling and worker join.
            let _ = unsafe { SetEvent(self.stop.0) };
            if let Ok(worker) = self.worker.get_mut() {
                if let Some(worker) = worker.take() {
                    let _ = worker.join();
                }
            }
        }
    }
}
