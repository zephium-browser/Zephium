pub(super) struct ProcessSample {
    pub(super) cpu_ns: u64,
    pub(super) peak_rss_bytes: u64,
}

pub(super) use imp::{private_directory, sample};

pub(super) fn redirects(metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use nix::sys::resource::{getrusage, UsageWho};
    use nix::sys::time::TimeValLike;
    use std::os::unix::fs::DirBuilderExt;

    use super::ProcessSample;

    pub(in super::super) fn sample() -> Option<ProcessSample> {
        let usage = getrusage(UsageWho::RUSAGE_SELF).ok()?;
        let cpu_us = usage
            .user_time()
            .num_microseconds()
            .checked_add(usage.system_time().num_microseconds())?;
        Some(ProcessSample {
            cpu_ns: u64::try_from(cpu_us).ok()?.checked_mul(1000)?,
            peak_rss_bytes: u64::try_from(usage.max_rss()).ok()?,
        })
    }

    pub(in super::super) fn private_directory(builder: &mut std::fs::DirBuilder) {
        builder.mode(0o700);
    }
}

#[cfg(windows)]
mod imp {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

    use super::ProcessSample;

    fn ticks(time: FILETIME) -> u64 {
        (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime)
    }

    // Read in-process: starting PowerShell per sample cost far more CPU than
    // the map work being measured.
    pub(in super::super) fn sample() -> Option<ProcessSample> {
        let (mut created, mut exited, mut kernel, mut user) = Default::default();
        let mut memory = PROCESS_MEMORY_COUNTERS::default();
        let size = u32::try_from(std::mem::size_of::<PROCESS_MEMORY_COUNTERS>()).ok()?;
        // SAFETY: the current-process pseudo-handle needs no closing, and each
        // out-pointer is a writable local of the type the call fills.
        let (times, counted) = unsafe {
            let process = GetCurrentProcess();
            (
                GetProcessTimes(process, &mut created, &mut exited, &mut kernel, &mut user),
                GetProcessMemoryInfo(process, &mut memory, size),
            )
        };
        times.ok()?;
        counted.ok()?;
        Some(ProcessSample {
            // Kernel and user time come in units of 100 ns.
            cpu_ns: ticks(kernel).checked_add(ticks(user))?.checked_mul(100)?,
            peak_rss_bytes: u64::try_from(memory.PeakWorkingSetSize).ok()?,
        })
    }

    pub(in super::super) fn private_directory(_: &mut std::fs::DirBuilder) {}
}

#[cfg(not(any(target_os = "macos", windows)))]
mod imp {
    use super::ProcessSample;

    pub(in super::super) fn sample() -> Option<ProcessSample> {
        None
    }

    pub(in super::super) fn private_directory(_: &mut std::fs::DirBuilder) {}
}

#[cfg(all(test, any(target_os = "macos", windows)))]
mod tests {
    #[test]
    fn samples_current_process() {
        let first = super::sample().expect("process metrics");
        let second = super::sample().expect("process metrics");
        assert!(first.peak_rss_bytes > 0);
        assert!(second.cpu_ns >= first.cpu_ns);
        assert!(second.peak_rss_bytes >= first.peak_rss_bytes);
    }
}
