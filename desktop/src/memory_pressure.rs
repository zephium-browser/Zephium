//! OS and process-family residency input. Windows combines system memory
//! notifications with the browser process tree's private bytes so a large
//! session can raise Warning/Critical without waiting for the machine to run
//! out of free physical RAM.
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

/// Private bytes of this process family → residency tier. Soft matches OS
/// Warning policy; hard matches Critical. Tuned for multi-tab WebView2 sessions
/// that stay Normal on free-physical-RAM notifications alone.
#[cfg(any(test, target_os = "windows"))]
fn self_budget_tier(private_bytes: u64) -> zephium_core::ports::engine::MemoryPressure {
    use zephium_core::ports::engine::MemoryPressure;
    const SOFT: u64 = 2 << 30;
    const HARD: u64 = 4 << 30;
    if private_bytes >= HARD {
        MemoryPressure::Critical
    } else if private_bytes >= SOFT {
        MemoryPressure::Warning
    } else {
        MemoryPressure::Normal
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

    #[test]
    fn self_budget_raises_before_the_machine_is_out_of_ram() {
        assert_eq!(self_budget_tier(0), MemoryPressure::Normal);
        assert_eq!(self_budget_tier((2 << 30) - 1), MemoryPressure::Normal);
        assert_eq!(self_budget_tier(2 << 30), MemoryPressure::Warning);
        assert_eq!(self_budget_tier((4 << 30) - 1), MemoryPressure::Warning);
        assert_eq!(self_budget_tier(4 << 30), MemoryPressure::Critical);
        assert!(MemoryPressure::Critical > MemoryPressure::Warning);
        assert!(MemoryPressure::Warning > MemoryPressure::Normal);
        assert_eq!(
            MemoryPressure::Normal.max(MemoryPressure::Warning),
            MemoryPressure::Warning
        );
        assert_eq!(
            MemoryPressure::Critical.max(MemoryPressure::Warning),
            MemoryPressure::Critical
        );
    }
}

#[cfg(target_os = "windows")]
mod native {
    use std::collections::{HashMap, HashSet};
    use std::sync::{Arc, Mutex};
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows::Win32::System::Memory::{
        CreateMemoryResourceNotification, HighMemoryResourceNotification,
        LowMemoryResourceNotification, QueryMemoryResourceNotification,
    };
    use windows::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
    };
    use windows::Win32::System::Threading::{
        CreateEventW, OpenProcess, SetEvent, WaitForMultipleObjects,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use zephium_app::{Command, Handle};
    use zephium_core::ports::engine::MemoryPressure;

    /// How often a critical OS state is read again to notice it easing.
    const CRITICAL_RECHECK_MS: u32 = 10_000;
    /// Process-family sample interval. A long Normal interval would miss a
    /// burst that crosses soft/hard before the first elevated wait.
    const SELF_SAMPLE_MS: u32 = 5_000;

    fn signaled(event: &Event) -> Option<bool> {
        let mut state = windows::core::BOOL::default();
        // SAFETY: a live memory resource notification handle.
        unsafe { QueryMemoryResourceNotification(event.handle(), &mut state) }.ok()?;
        Some(state.as_bool())
    }

    fn private_bytes(pid: u32) -> Option<u64> {
        // SAFETY: query-only handle closed before return.
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
            let mut memory = PROCESS_MEMORY_COUNTERS_EX::default();
            let ok = GetProcessMemoryInfo(
                handle,
                &mut memory as *mut PROCESS_MEMORY_COUNTERS_EX as *mut PROCESS_MEMORY_COUNTERS,
                std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
            );
            let _ = CloseHandle(handle);
            ok.ok()?;
            Some(memory.PrivateUsage as u64)
        }
    }

    /// Root process and every descendant, matching the soak sampler's tree.
    fn family_private_bytes(root: u32) -> Option<u64> {
        // SAFETY: snapshot handle is owned and closed below.
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }.ok()?;
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut parents: HashMap<u32, u32> = HashMap::new();
        // SAFETY: Toolhelp enumeration against the live snapshot.
        let mut more = unsafe { Process32FirstW(snapshot, &mut entry) }.is_ok();
        while more {
            parents.insert(entry.th32ProcessID, entry.th32ParentProcessID);
            more = unsafe { Process32NextW(snapshot, &mut entry) }.is_ok();
        }
        let _ = unsafe { CloseHandle(snapshot) };

        let mut ids = HashSet::new();
        ids.insert(root);
        let mut grew = true;
        while grew {
            grew = false;
            for (&pid, &ppid) in &parents {
                if ids.contains(&ppid) && ids.insert(pid) {
                    grew = true;
                }
            }
        }
        let mut total = 0_u64;
        for pid in ids {
            total = total.saturating_add(private_bytes(pid).unwrap_or(0));
        }
        Some(total)
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
            let root = std::process::id();
            let worker = std::thread::Builder::new()
                .name("memory-pressure".into())
                .spawn(move || {
                    let read_os = || Some(super::windows_tier(signaled(&low)?, signaled(&high)?));
                    let read_self = || {
                        family_private_bytes(root).map_or(MemoryPressure::Normal, |bytes| {
                            super::self_budget_tier(bytes)
                        })
                    };
                    let Some(mut os_tier) = read_os() else {
                        return;
                    };
                    let mut effective = os_tier.max(read_self());
                    // Launching under pressure is not a normal machine.
                    if effective != MemoryPressure::Normal {
                        let _ = shell.dispatch(Command::SetMemoryPressure(effective));
                    }
                    loop {
                        // Waiting on a signaled notification would return at
                        // once, so each OS tier waits only for what can end it.
                        // Self-budget sampling always needs a finite timeout:
                        // Normal OS pressure previously waited forever.
                        let (watch_low, watch_high, os_recheck) = super::windows_watch(os_tier);
                        let timeout = if os_recheck {
                            CRITICAL_RECHECK_MS.min(SELF_SAMPLE_MS)
                        } else {
                            SELF_SAMPLE_MS
                        };
                        let mut handles = vec![stopped.handle()];
                        if watch_low {
                            handles.push(low.handle());
                        }
                        if watch_high {
                            handles.push(high.handle());
                        }
                        // SAFETY: every handle remains owned across this wait;
                        // shutdown signals index zero before joining this thread.
                        let result = unsafe { WaitForMultipleObjects(&handles, false, timeout) };
                        let notified = (WAIT_OBJECT_0.0 + 1
                            ..WAIT_OBJECT_0.0 + handles.len() as u32)
                            .contains(&result.0);
                        if !notified && result != WAIT_TIMEOUT {
                            break;
                        }
                        let Some(next_os) = read_os() else {
                            break;
                        };
                        os_tier = next_os;
                        let next = os_tier.max(read_self());
                        if next != effective {
                            effective = next;
                            let _ = shell.dispatch(Command::SetMemoryPressure(effective));
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
