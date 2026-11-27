//! OS and process-family residency input. Windows may raise Warning from the
//! browser process tree's private bytes so grace/watermarks tighten before the
//! machine runs out of free physical RAM. Soft sits high because PrivateUsage
//! overstates reclaimable working set on WebView2. Critical stays OS-only.
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

/// Enter Warning at `soft`; leave only below soft−1 GiB. Never emits Critical:
/// that tier is reserved for OS low-memory notifications.
#[cfg(any(test, target_os = "windows"))]
fn self_budget_tier(
    private_bytes: u64,
    previous: zephium_core::ports::engine::MemoryPressure,
    soft: u64,
) -> zephium_core::ports::engine::MemoryPressure {
    use zephium_core::ports::engine::MemoryPressure;
    let exit = soft.saturating_sub(1 << 30);
    match previous {
        MemoryPressure::Warning if private_bytes >= exit => MemoryPressure::Warning,
        _ if private_bytes >= soft => MemoryPressure::Warning,
        _ => MemoryPressure::Normal,
    }
}

/// Apply a family sample: `None` keeps `previous` so undercount cannot clear Warning.
#[cfg(any(test, target_os = "windows"))]
fn self_tier_after_sample(
    sample: Option<u64>,
    previous: zephium_core::ports::engine::MemoryPressure,
    soft: u64,
) -> zephium_core::ports::engine::MemoryPressure {
    match sample {
        Some(bytes) => self_budget_tier(bytes, previous, soft),
        None => previous,
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

#[cfg(any(test, target_os = "windows"))]
fn windows_wait_timeout(
    effective: zephium_core::ports::engine::MemoryPressure,
    os_recheck: bool,
) -> u32 {
    use zephium_core::ports::engine::MemoryPressure;

    const CRITICAL_RECHECK_MS: u32 = 10_000;
    const SELF_SAMPLE_NORMAL_MS: u32 = 30_000;
    const SELF_SAMPLE_ELEVATED_MS: u32 = 5_000;
    let self_ms = if effective == MemoryPressure::Normal {
        SELF_SAMPLE_NORMAL_MS
    } else {
        SELF_SAMPLE_ELEVATED_MS
    };
    if os_recheck {
        CRITICAL_RECHECK_MS.min(self_ms)
    } else {
        self_ms
    }
}

/// A complete snapshot is needed before querying the browser family. Dropping
/// edges from newer parents avoids following a reused parent PID.
#[cfg(any(test, target_os = "windows"))]
fn sum_family_private_bytes<E>(
    root: u32,
    processes: impl IntoIterator<Item = Result<(u32, u32), E>>,
    mut query: impl FnMut(u32) -> Option<(u64, u64)>,
) -> Option<u64> {
    use std::collections::{HashMap, HashSet};

    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for process in processes {
        let (pid, parent) = process.ok()?;
        children.entry(parent).or_default().push(pid);
    }

    let (mut total, root_start) = query(root)?;
    let mut stack = vec![(root, root_start)];
    let mut seen = HashSet::from([root]);
    while let Some((parent, parent_start)) = stack.pop() {
        let Some(kids) = children.get(&parent) else {
            continue;
        };
        for &pid in kids {
            if !seen.insert(pid) {
                continue;
            }
            let (bytes, start) = query(pid)?;
            if parent_start > start {
                continue;
            }
            total = total.saturating_add(bytes);
            stack.push((pid, start));
        }
    }
    Some(total)
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
    fn windows_sampling_wait_tracks_effective_pressure() {
        assert_eq!(windows_wait_timeout(MemoryPressure::Normal, false), 30_000);
        assert_eq!(windows_wait_timeout(MemoryPressure::Warning, false), 5_000);
        assert_eq!(windows_wait_timeout(MemoryPressure::Critical, true), 5_000);
    }

    #[test]
    fn incomplete_enumeration_cannot_produce_a_family_sample() {
        let processes = [Ok((1, 0)), Ok((2, 1)), Err(())];
        assert_eq!(
            sum_family_private_bytes(1, processes, |_| panic!("partial snapshot was queried")),
            None
        );
    }

    #[test]
    fn family_sample_ignores_reused_parent_edges_and_rejects_missing_queries() {
        let processes = [(1, 0), (2, 1), (3, 2), (4, 1), (5, 4), (6, 0)].map(Ok::<_, ()>);
        let query = |pid| match pid {
            1 => Some((100, 10)),
            2 => Some((200, 20)),
            3 => Some((300, 30)),
            4 => Some((400, 5)),
            _ => panic!("queried a process outside the family"),
        };
        assert_eq!(sum_family_private_bytes(1, processes, query), Some(600));
        assert_eq!(
            sum_family_private_bytes(1, processes, |pid| if pid == 3 { None } else { query(pid) }),
            None
        );
    }

    #[test]
    fn self_budget_raises_warning_with_hysteresis_never_critical() {
        const SOFT: u64 = 8 << 30;
        assert_eq!(
            self_budget_tier(0, MemoryPressure::Normal, SOFT),
            MemoryPressure::Normal
        );
        assert_eq!(
            self_budget_tier(SOFT - 1, MemoryPressure::Normal, SOFT),
            MemoryPressure::Normal
        );
        assert_eq!(
            self_budget_tier(SOFT, MemoryPressure::Normal, SOFT),
            MemoryPressure::Warning
        );
        assert_eq!(
            self_budget_tier(SOFT + (2 << 30), MemoryPressure::Normal, SOFT),
            MemoryPressure::Warning
        );
        assert_eq!(
            self_budget_tier(SOFT - (512 << 20), MemoryPressure::Warning, SOFT),
            MemoryPressure::Warning
        );
        assert_eq!(
            self_budget_tier(SOFT - (1 << 30) - 1, MemoryPressure::Warning, SOFT),
            MemoryPressure::Normal
        );
        assert_ne!(
            self_budget_tier(SOFT * 2, MemoryPressure::Normal, SOFT),
            MemoryPressure::Critical
        );
        assert_eq!(
            MemoryPressure::Critical.max(MemoryPressure::Warning),
            MemoryPressure::Critical
        );
        assert_eq!(
            self_tier_after_sample(None, MemoryPressure::Warning, SOFT),
            MemoryPressure::Warning
        );
        assert_eq!(
            self_tier_after_sample(Some(SOFT - (1 << 30) - 1), MemoryPressure::Warning, SOFT),
            MemoryPressure::Normal
        );
    }
}

#[cfg(target_os = "windows")]
mod native {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::sync::{Arc, Mutex};
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{
        CloseHandle, ERROR_NO_MORE_FILES, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT,
    };
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

    /// Family PrivateUsage soft floor. Private overstates WS on WebView2; 8 GiB
    /// sits above a typical 50-tab private floor (~4–7 GiB) so Warning is not sticky.
    const SELF_SOFT_PRIVATE: u64 = 8 << 30;

    fn signaled(event: &Event) -> Option<bool> {
        let mut state = windows::core::BOOL::default();
        // SAFETY: a live memory resource notification handle.
        unsafe { QueryMemoryResourceNotification(event.handle(), &mut state) }.ok()?;
        Some(state.as_bool())
    }

    fn process_private_and_start(pid: u32) -> Option<(u64, u64)> {
        use windows::Win32::Foundation::FILETIME;
        use windows::Win32::System::Threading::GetProcessTimes;
        // SAFETY: query-only handle closed before return.
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
            let (mut created, mut exited, mut kernel, mut user) = (
                FILETIME::default(),
                FILETIME::default(),
                FILETIME::default(),
                FILETIME::default(),
            );
            let times = GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user);
            let mut memory = PROCESS_MEMORY_COUNTERS_EX::default();
            let counted = GetProcessMemoryInfo(
                handle,
                &mut memory as *mut PROCESS_MEMORY_COUNTERS_EX as *mut PROCESS_MEMORY_COUNTERS,
                std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
            );
            let _ = CloseHandle(handle);
            times.ok()?;
            counted.ok()?;
            let start =
                (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime);
            Some((memory.PrivateUsage as u64, start))
        }
    }

    /// Root plus descendants. Toolhelp builds the parent map cheaply; only the
    /// family is OpenProcess'd. Parent edges where the parent started after the
    /// child are dropped (PID reuse). `None` if the root or any non-reuse child
    /// cannot be queried — callers keep the previous self tier so a partial
    /// undercount cannot clear Warning.
    fn family_private_bytes(root: u32) -> Option<u64> {
        // SAFETY: the new snapshot handle is transferred to one owner.
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }.ok()?;
        let snapshot = unsafe { OwnedHandle::from_raw_handle(snapshot.0) };
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        // SAFETY: Toolhelp enumeration uses a live snapshot and a sized entry.
        unsafe { Process32FirstW(HANDLE(snapshot.as_raw_handle()), &mut entry) }.ok()?;
        let first = (entry.th32ProcessID, entry.th32ParentProcessID);
        let rest = std::iter::from_fn(move || {
            // SAFETY: same live snapshot and sized entry.
            match unsafe { Process32NextW(HANDLE(snapshot.as_raw_handle()), &mut entry) } {
                Ok(()) => Some(Ok((entry.th32ProcessID, entry.th32ParentProcessID))),
                Err(error) if error.code() == ERROR_NO_MORE_FILES.to_hresult() => None,
                Err(error) => Some(Err(error)),
            }
        });
        super::sum_family_private_bytes(
            root,
            std::iter::once(Ok(first)).chain(rest),
            process_private_and_start,
        )
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
                    let Some(mut os_tier) = read_os() else {
                        return;
                    };
                    let mut self_tier = super::self_tier_after_sample(
                        family_private_bytes(root),
                        MemoryPressure::Normal,
                        SELF_SOFT_PRIVATE,
                    );
                    let mut effective = os_tier.max(self_tier);
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
                        let timeout = super::windows_wait_timeout(effective, os_recheck);
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
                        // A failed family sample keeps the prior self tier so
                        // a denied OpenProcess cannot clear Warning to Normal.
                        self_tier = super::self_tier_after_sample(
                            family_private_bytes(root),
                            self_tier,
                            SELF_SOFT_PRIVATE,
                        );
                        let next = os_tier.max(self_tier);
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
