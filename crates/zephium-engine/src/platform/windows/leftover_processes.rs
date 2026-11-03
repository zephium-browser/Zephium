//! The browser processes a WebView2 generation recorded, named by id and
//! creation time, so a later launch can prove them gone or end an orphan.
//! Windows reuses process ids; the creation time is what keeps a check or a
//! termination on the exact process that was recorded.

use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::time::{Duration, Instant};

use windows::core::PWSTR;
use windows::Wdk::System::Threading::{NtQueryInformationProcess, ProcessBasicInformation};
use windows::Win32::Foundation::{
    ERROR_INVALID_PARAMETER, ERROR_NO_MORE_FILES, FILETIME, HANDLE, WAIT_OBJECT_0,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, QueryFullProcessImageNameW, TerminateProcess,
    WaitForSingleObject, PROCESS_ACCESS_RIGHTS, PROCESS_BASIC_INFORMATION, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
};
use zephium_core::webview2::{
    BrowserProcessRecord, GenerationReport, LeftoverProcesses, RecordedProcessState,
};

const BROWSER_IMAGE: &str = "msedgewebview2.exe";
/// How long an ended orphan and its children may take to be gone, together,
/// before their data is left alone.
const EXIT_WAIT_MS: u32 = 2_000;

pub struct WindowsLeftoverProcesses;

impl LeftoverProcesses for WindowsLeftoverProcesses {
    fn state(&self, process: BrowserProcessRecord) -> RecordedProcessState {
        match open_exact(
            process,
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
        ) {
            Opened::Live(handle, _) if !exited(&handle, 0) => RecordedProcessState::Running,
            Opened::Live(..) | Opened::Gone => RecordedProcessState::Gone,
            Opened::Unknown => RecordedProcessState::Indeterminate,
        }
    }

    fn terminate(&self, process: BrowserProcessRecord) -> bool {
        let access = PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE | PROCESS_TERMINATE;
        let browser = match open_exact(process, access) {
            Opened::Live(handle, _) => handle,
            Opened::Gone => return true,
            Opened::Unknown => return false,
        };
        // A record names a WebView2 browser; anything else is left running.
        if !image_is(&browser, BROWSER_IMAGE) {
            return false;
        }
        // Renderers, the GPU process and utilities are the browser's direct
        // children. Every child is checked before anything is ended: one this
        // launch may not end could still hold the generation's files open.
        let Some(children) = children_of(process, access) else {
            return false;
        };
        let mut ended = Vec::new();
        let mut checks = Vec::new();
        for child in children {
            checks.push(match child {
                Opened::Live(handle, _) if image_is(&handle, BROWSER_IMAGE) => {
                    ended.push(handle);
                    Child::Endable
                }
                Opened::Live(..) => Child::Foreign,
                Opened::Gone => Child::Gone,
                Opened::Unknown => Child::Unopenable,
            });
        }
        if !children_may_end(&checks) {
            return false;
        }
        // From here ending is not all or nothing: a process that will not
        // exit in time keeps the generation, though others may be gone.
        ended.push(browser);
        for handle in &ended {
            // SAFETY: a live handle opened with PROCESS_TERMINATE.
            let _ = unsafe { TerminateProcess(raw(handle), 1) };
        }
        let deadline = Instant::now() + Duration::from_millis(u64::from(EXIT_WAIT_MS));
        ended.iter().all(|handle| {
            let left = deadline.saturating_duration_since(Instant::now());
            exited(
                handle,
                u32::try_from(left.as_millis()).unwrap_or(EXIT_WAIT_MS),
            )
        })
    }
}

/// What opening a process found. Only an id that names no process at all,
/// or names one created at another time, proves the process gone.
enum Opened {
    Live(OwnedHandle, u64),
    Gone,
    Unknown,
}

fn open(id: u32, access: PROCESS_ACCESS_RIGHTS) -> Opened {
    // SAFETY: OpenProcess returns an owned handle or an error.
    match unsafe { OpenProcess(access, false, id) } {
        // SAFETY: the handle was just returned to this owner.
        Ok(handle) => {
            let handle = unsafe { OwnedHandle::from_raw_handle(handle.0) };
            match created(&handle) {
                Some(created) => Opened::Live(handle, created),
                None => Opened::Unknown,
            }
        }
        Err(error) if error.code() == ERROR_INVALID_PARAMETER.to_hresult() => Opened::Gone,
        Err(_) => Opened::Unknown,
    }
}

fn open_exact(process: BrowserProcessRecord, access: PROCESS_ACCESS_RIGHTS) -> Opened {
    match open(process.id, access) {
        Opened::Live(handle, created) if created == process.created => {
            Opened::Live(handle, created)
        }
        // The id now names a later process.
        Opened::Live(..) => Opened::Gone,
        other => other,
    }
}

/// What checking one of an orphan's children found.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Child {
    Endable,
    Gone,
    /// Running, but not a WebView2 process.
    Foreign,
    /// Running, or not known not to be, and could not be opened to be ended
    /// or shown to be the browser's own child.
    Unopenable,
}

fn children_may_end(children: &[Child]) -> bool {
    children
        .iter()
        .all(|child| matches!(child, Child::Endable | Child::Gone))
}

/// The record for a running browser process, read from the process itself.
pub fn browser_process_record(process_id: u32) -> Option<BrowserProcessRecord> {
    // SAFETY: OpenProcess returns an owned handle or an error.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id) }
        .ok()
        // SAFETY: the handle was just returned to this owner.
        .map(|handle| unsafe { OwnedHandle::from_raw_handle(handle.0) })?;
    Some(BrowserProcessRecord {
        id: process_id,
        created: created(&handle)?,
    })
}

fn raw(handle: &OwnedHandle) -> HANDLE {
    HANDLE(handle.as_raw_handle())
}

fn created(handle: &OwnedHandle) -> Option<u64> {
    let mut created = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    // SAFETY: a live handle with query rights and four correctly sized outputs.
    unsafe { GetProcessTimes(raw(handle), &mut created, &mut exit, &mut kernel, &mut user) }
        .ok()?;
    let value = (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime);
    (value != 0).then_some(value)
}

/// The id of the process that started this one, read from the process
/// itself.
fn parent_id(handle: &OwnedHandle) -> Option<u32> {
    let mut info = PROCESS_BASIC_INFORMATION::default();
    let mut length = 0;
    // SAFETY: a live handle with query rights and an output of the class's size.
    let status = unsafe {
        NtQueryInformationProcess(
            raw(handle),
            ProcessBasicInformation,
            (&raw mut info).cast(),
            size_of::<PROCESS_BASIC_INFORMATION>() as u32,
            &mut length,
        )
    };
    if status.is_err() {
        return None;
    }
    u32::try_from(info.InheritedFromUniqueProcessId).ok()
}

fn exited(handle: &OwnedHandle, wait_ms: u32) -> bool {
    // SAFETY: a live handle opened with SYNCHRONIZE.
    unsafe { WaitForSingleObject(raw(handle), wait_ms) == WAIT_OBJECT_0 }
}

fn image_is(handle: &OwnedHandle, image: &str) -> bool {
    let mut buffer = [0u16; 1024];
    let mut length = buffer.len() as u32;
    // SAFETY: the buffer and its length describe the same writable storage.
    let named = unsafe {
        QueryFullProcessImageNameW(
            raw(handle),
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut length,
        )
    };
    if named.is_err() {
        return false;
    }
    let path = String::from_utf16_lossy(&buffer[..length as usize]);
    path.rsplit('\\')
        .next()
        .is_some_and(|name| name.eq_ignore_ascii_case(image))
}

/// The browser's children, each opened for `access`, or None when they cannot
/// be listed. A child is only one created after the browser, so an id the
/// browser's parent slot once pointed at is never taken for it; one that does
/// not itself name the browser as its parent cannot be ended.
fn children_of(parent: BrowserProcessRecord, access: PROCESS_ACCESS_RIGHTS) -> Option<Vec<Opened>> {
    // SAFETY: a process snapshot; the handle is owned below.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }.ok()?;
    // SAFETY: the snapshot handle was just returned to this owner.
    let snapshot = unsafe { OwnedHandle::from_raw_handle(snapshot.0) };
    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut children = Vec::new();
    // SAFETY: entry carries its size; the snapshot is live. A snapshot always
    // holds at least the system's own processes, so a first entry that cannot
    // be read means the listing failed.
    unsafe { Process32FirstW(raw(&snapshot), &mut entry) }.ok()?;
    loop {
        if entry.th32ParentProcessID == parent.id && entry.th32ProcessID != parent.id {
            match open(entry.th32ProcessID, access) {
                Opened::Live(_, created) if created < parent.created => {}
                Opened::Gone => {}
                // The id may have been reused since the snapshot. The caller
                // holds the browser open, so its id still names only it.
                Opened::Live(handle, created) if parent_id(&handle) == Some(parent.id) => {
                    children.push(Opened::Live(handle, created));
                }
                Opened::Live(..) => children.push(Opened::Unknown),
                opened => children.push(opened),
            }
        }
        // SAFETY: as above. Only the documented end of the list completes it.
        if let Err(error) = unsafe { Process32NextW(raw(&snapshot), &mut entry) } {
            return (error.code() == ERROR_NO_MORE_FILES.to_hresult()).then_some(children);
        }
    }
}

/// One plain line for each thing preparing a runtime root did with what
/// earlier runs left behind.
pub fn generation_report_lines(root: &str, report: &GenerationReport) -> Vec<String> {
    let mut lines = Vec::new();
    for (count, line) in [
        (
            report.reclaimed_after_exit.len(),
            "generation(s) left by unclean quits were cleared; their browsers had exited",
        ),
        (
            report.reclaimed_after_termination.len(),
            "generation(s) were cleared after ending their orphaned browsers",
        ),
        (
            report.reclaimed_empty.len(),
            "generation(s) holding no browsing data were cleared",
        ),
        (
            report.kept_running.len(),
            "generation(s) stay until a reboot; their orphaned browsers could not be ended",
        ),
        (
            report.kept_unchecked.len(),
            "generation(s) stay until a reboot; a browser they recorded could not be checked",
        ),
    ] {
        if count > 0 {
            lines.push(format!("privacy: {count} {root} WebView2 {line}"));
        }
    }
    if let Some(overdue) = &report.overdue {
        lines.push(format!(
            "privacy: {root} WebView2 cleanup is overdue: {overdue}"
        ));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_orphan_is_ended_only_when_every_child_can_be_ended_too() {
        assert!(children_may_end(&[]));
        assert!(children_may_end(&[Child::Endable, Child::Gone]));
        assert!(!children_may_end(&[Child::Endable, Child::Foreign]));
        assert!(!children_may_end(&[Child::Gone, Child::Unopenable]));
    }

    #[test]
    fn only_an_id_naming_no_process_proves_it_gone() {
        let query = PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE;
        assert!(matches!(open(0, query), Opened::Gone));
        // The system process exists and is never ours to end.
        assert!(matches!(open(4, PROCESS_TERMINATE), Opened::Unknown));
        let system = BrowserProcessRecord { id: 4, created: 1 };
        assert!(matches!(
            open_exact(system, query),
            Opened::Gone | Opened::Unknown
        ));
    }

    #[test]
    fn a_child_names_its_own_parent() {
        let mut child = std::process::Command::new("cmd.exe")
            .args(["/c", "ping -n 30 127.0.0.1 >nul"])
            .spawn()
            .unwrap();
        let Opened::Live(handle, _) = open(child.id(), PROCESS_QUERY_LIMITED_INFORMATION) else {
            panic!("a child of this process opens");
        };
        assert_eq!(parent_id(&handle), Some(std::process::id()));
        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn a_recorded_process_that_is_not_webview2_is_left_running() {
        let mut child = std::process::Command::new("cmd.exe")
            .args(["/c", "ping -n 30 127.0.0.1 >nul"])
            .spawn()
            .unwrap();
        let record = browser_process_record(child.id()).unwrap();
        assert_eq!(
            WindowsLeftoverProcesses.state(record),
            RecordedProcessState::Running
        );
        assert!(!WindowsLeftoverProcesses.terminate(record));
        assert!(child.try_wait().unwrap().is_none());
        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(
            WindowsLeftoverProcesses.state(record),
            RecordedProcessState::Gone
        );
    }
}
