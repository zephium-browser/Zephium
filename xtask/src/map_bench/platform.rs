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
    use std::process::{Command, Stdio};

    use super::ProcessSample;

    pub(in super::super) fn sample() -> Option<ProcessSample> {
        let pid = std::process::id();
        let script = format!(
            "$ErrorActionPreference='Stop'; $p=Get-Process -Id {pid}; \
             $p.TotalProcessorTime.Ticks; $p.PeakWorkingSet64"
        );
        let output = Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let output = std::str::from_utf8(&output.stdout).ok()?;
        let mut lines = output.lines();
        Some(ProcessSample {
            cpu_ns: lines.next()?.trim().parse::<u64>().ok()?.checked_mul(100)?,
            peak_rss_bytes: lines.next()?.trim().parse().ok()?,
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
