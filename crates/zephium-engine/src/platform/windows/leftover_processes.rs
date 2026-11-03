//! The browser processes a WebView2 generation recorded, named by id and
//! creation time, so a later launch can prove them gone or end an orphan.
//! Windows reuses process ids; the creation time is what keeps a check or a
//! termination on the exact process that was recorded.

use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};

use windows::core::PWSTR;
use windows::Win32::Foundation::{FILETIME, HANDLE, WAIT_OBJECT_0};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, QueryFullProcessImageNameW, TerminateProcess,
    WaitForSingleObject, PROCESS_ACCESS_RIGHTS, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
};
use zephium_core::webview2::{
    BrowserProcessRecord, GenerationReport, LeftoverProcesses, RecordedProcessState,
};

const BROWSER_IMAGE: &str = "msedgewebview2.exe";
/// How long an ended orphan may take to be gone before its data is left alone.
const EXIT_WAIT_MS: u32 = 2_000;

pub struct WindowsLeftoverProcesses;

impl LeftoverProcesses for WindowsLeftoverProcesses {
    fn state(&self, process: BrowserProcessRecord) -> RecordedProcessState {
        match open_exact(
            process,
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
        ) {
            Some(handle) if !exited(&handle, 0) => RecordedProcessState::Running,
            _ => RecordedProcessState::Gone,
        }
    }

    fn terminate(&self, process: BrowserProcessRecord) -> bool {
        let access = PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE | PROCESS_TERMINATE;
        let Some(browser) = open_exact(process, access) else {
            return true;
        };
        // A record names a WebView2 browser; anything else is left running.
        if !image_is(&browser, BROWSER_IMAGE) {
            return false;
        }
        // Renderers, the GPU process and utilities are the browser's direct
        // children. A child is only one created after the browser, so an id
        // the browser's parent slot once pointed at is never taken for it.
        for child in children_of(process) {
            if let Some(child) = open_exact(child, access) {
                if image_is(&child, BROWSER_IMAGE) {
                    // SAFETY: a live handle opened with PROCESS_TERMINATE.
                    let _ = unsafe { TerminateProcess(raw(&child), 1) };
                }
            }
        }
        // SAFETY: a live handle opened with PROCESS_TERMINATE.
        let _ = unsafe { TerminateProcess(raw(&browser), 1) };
        exited(&browser, EXIT_WAIT_MS)
    }
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

fn open_exact(process: BrowserProcessRecord, access: PROCESS_ACCESS_RIGHTS) -> Option<OwnedHandle> {
    // SAFETY: OpenProcess returns an owned handle or an error.
    let handle = unsafe { OpenProcess(access, false, process.id) }
        .ok()
        // SAFETY: the handle was just returned to this owner.
        .map(|handle| unsafe { OwnedHandle::from_raw_handle(handle.0) })?;
    (created(&handle)? == process.created).then_some(handle)
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

fn children_of(parent: BrowserProcessRecord) -> Vec<BrowserProcessRecord> {
    // SAFETY: a process snapshot; the handle is owned below.
    let Ok(snapshot) = (unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }) else {
        return Vec::new();
    };
    // SAFETY: the snapshot handle was just returned to this owner.
    let snapshot = unsafe { OwnedHandle::from_raw_handle(snapshot.0) };
    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut children = Vec::new();
    // SAFETY: entry carries its size; the snapshot is live.
    let mut more = unsafe { Process32FirstW(raw(&snapshot), &mut entry) }.is_ok();
    while more {
        if entry.th32ParentProcessID == parent.id && entry.th32ProcessID != parent.id {
            if let Some(child) = browser_process_record(entry.th32ProcessID)
                .filter(|child| child.created >= parent.created)
            {
                children.push(child);
            }
        }
        // SAFETY: as above.
        more = unsafe { Process32NextW(raw(&snapshot), &mut entry) }.is_ok();
    }
    children
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
