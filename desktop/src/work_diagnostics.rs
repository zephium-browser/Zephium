//! Bounded, content-free diagnostics for explicitly enabled development traces.
//! Call sites supply identities, counters and closed lifecycle/failure enums only.
use std::io::{Seek, SeekFrom, Write};
use std::sync::{Mutex, OnceLock};

const MAX_BYTES: u64 = 1_048_576;
const MAX_RECORD_BYTES: usize = 4096;
static LOG: OnceLock<Mutex<std::fs::File>> = OnceLock::new();

pub(crate) fn install(app: &tauri::AppHandle) {
    let Ok(directory) = super::data_root::logs(app) else {
        return;
    };
    if std::fs::create_dir_all(&directory).is_err() {
        return;
    }
    let mut options = std::fs::OpenOptions::new();
    options.create(true).read(true).write(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    if let Ok(file) = options.open(directory.join("work-development.log")) {
        let _ = LOG.set(Mutex::new(file));
    }
}

pub(crate) fn record(arguments: std::fmt::Arguments<'_>) {
    super::write_diagnostic(arguments);
    let Some(log) = LOG.get() else { return };
    let Ok(mut file) = log.try_lock() else { return };
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |time| time.as_millis());
    let line = format!("{millis} {arguments}\n");
    let _ = append_bounded(&mut file, line.as_bytes());
}

fn append_bounded(file: &mut std::fs::File, line: &[u8]) -> std::io::Result<()> {
    if line.len() > MAX_RECORD_BYTES {
        return Ok(());
    }
    if file.metadata()?.len().saturating_add(line.len() as u64) > MAX_BYTES {
        file.set_len(0)?;
    }
    file.seek(SeekFrom::End(0))?;
    file.write_all(line)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn diagnostics_remain_bounded_and_discard_oversized_records() {
        let mut file = tempfile::tempfile().expect("file");
        file.set_len(MAX_BYTES - 2).expect("fill");
        append_bounded(&mut file, b"done\n").expect("roll over");
        assert_eq!(file.metadata().expect("metadata").len(), 5);
        append_bounded(&mut file, &vec![b'x'; MAX_RECORD_BYTES + 1]).expect("discard");
        assert_eq!(file.metadata().expect("metadata").len(), 5);
    }
}
