//! Stdio descendants enter an owned kill-on-close job before the server runs.
use std::mem::{size_of, size_of_val};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};

use ::windows::core::PCWSTR;
use ::windows::Win32::Foundation::{ERROR_NO_MORE_FILES, HANDLE, WAIT_TIMEOUT};
use ::windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
};
use ::windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use ::windows::Win32::System::Threading::{
    GetProcessId, GetProcessIdOfThread, OpenThread, ResumeThread, WaitForSingleObject,
    THREAD_QUERY_LIMITED_INFORMATION, THREAD_SUSPEND_RESUME,
};
use tokio::process::Child;

use super::McpError;

pub(super) struct Job(OwnedHandle);

fn handle(owned: &OwnedHandle) -> HANDLE {
    HANDLE(owned.as_raw_handle())
}

impl Job {
    pub(super) fn new() -> Result<Self, McpError> {
        let job = unsafe { CreateJobObjectW(None, PCWSTR::null()) }.map_err(|_| McpError::Spawn)?;
        let job = Self(unsafe { OwnedHandle::from_raw_handle(job.0) });
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        unsafe {
            SetInformationJobObject(
                handle(&job.0),
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of_val(&limits) as u32,
            )
        }
        .map_err(|_| McpError::Spawn)?;
        Ok(job)
    }

    pub(super) fn stop(&self) {
        unsafe {
            let _ = TerminateJobObject(handle(&self.0), 1);
        }
    }

    pub(super) fn assign_and_resume(&self, child: &Child) -> Result<(), McpError> {
        let process = HANDLE(child.raw_handle().ok_or(McpError::Spawn)?);
        let pid = child.id().ok_or(McpError::Spawn)?;
        if unsafe { GetProcessId(process) } != pid
            || unsafe { WaitForSingleObject(process, 0) } != WAIT_TIMEOUT
        {
            return Err(McpError::Spawn);
        }
        unsafe { AssignProcessToJobObject(handle(&self.0), process) }
            .map_err(|_| McpError::Spawn)?;
        let thread = suspended_thread(pid)?;
        if unsafe { ResumeThread(handle(&thread)) } != 1 {
            self.stop();
            return Err(McpError::Spawn);
        }
        Ok(())
    }
}

fn suspended_thread(pid: u32) -> Result<OwnedHandle, McpError> {
    let snapshot =
        unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) }.map_err(|_| McpError::Spawn)?;
    let snapshot = unsafe { OwnedHandle::from_raw_handle(snapshot.0) };
    let mut entry = THREADENTRY32 {
        dwSize: size_of::<THREADENTRY32>() as u32,
        ..Default::default()
    };
    unsafe { Thread32First(handle(&snapshot), &mut entry) }.map_err(|_| McpError::Spawn)?;
    let mut found = None;
    let mut observed = 0;
    loop {
        observed += 1;
        if observed > 65536 || entry.dwSize < size_of::<THREADENTRY32>() as u32 {
            return Err(McpError::Spawn);
        }
        if entry.th32OwnerProcessID == pid {
            if found.is_some() {
                return Err(McpError::Spawn);
            }
            let thread = unsafe {
                OpenThread(
                    THREAD_SUSPEND_RESUME | THREAD_QUERY_LIMITED_INFORMATION,
                    false,
                    entry.th32ThreadID,
                )
            }
            .map_err(|_| McpError::Spawn)?;
            let thread = unsafe { OwnedHandle::from_raw_handle(thread.0) };
            if unsafe { GetProcessIdOfThread(handle(&thread)) } != pid {
                return Err(McpError::Spawn);
            }
            found = Some(thread);
        }
        entry.dwSize = size_of::<THREADENTRY32>() as u32;
        match unsafe { Thread32Next(handle(&snapshot), &mut entry) } {
            Ok(()) => {}
            Err(error) if error.code() == ERROR_NO_MORE_FILES.to_hresult() => break,
            Err(_) => return Err(McpError::Spawn),
        }
    }
    found.ok_or(McpError::Spawn)
}

#[cfg(test)]
mod tests {
    use super::super::tests::orphaned_descendant;
    use super::*;
    use ::windows::Win32::Foundation::WAIT_OBJECT_0;
    use ::windows::Win32::System::JobObjects::IsProcessInJob;
    use ::windows::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
    };

    #[tokio::test]
    async fn dropping_server_ends_a_descendant_even_after_parent_exits() {
        let (process, pid, _stdout) = orphaned_descendant().await;
        let descendant = unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                false,
                pid,
            )
        }
        .unwrap();
        let descendant = unsafe { OwnedHandle::from_raw_handle(descendant.0) };
        let mut assigned = Default::default();
        unsafe {
            IsProcessInJob(
                handle(&descendant),
                Some(handle(&process.job.0)),
                &mut assigned,
            )
        }
        .unwrap();
        assert!(assigned.as_bool());
        assert_eq!(
            unsafe { WaitForSingleObject(handle(&descendant), 0) },
            WAIT_TIMEOUT
        );
        drop(process);
        assert_eq!(
            unsafe { WaitForSingleObject(handle(&descendant), 5000) },
            WAIT_OBJECT_0
        );
    }
}
