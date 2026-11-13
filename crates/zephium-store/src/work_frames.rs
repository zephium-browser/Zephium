//! File-backed last frames of pages an agent read, so page cards keep their
//! picture after a restart. One PNG and one small sidecar per (attempt, step),
//! per profile, bounded in count and size. No page content beyond the picture.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use zephium_core::ids::ProfileId;
use zephium_core::work::{WorkAttemptId, WorkExecutionId, WorkId, WorkStepId};

pub const MAX_PROFILE_FRAMES: usize = 256;
pub const MAX_FRAME_BYTES: usize = 1 << 20;
const MAX_URL_BYTES: usize = 2048;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WorkFrameRecord {
    pub work: WorkId,
    pub execution: WorkExecutionId,
    pub attempt: WorkAttemptId,
    pub step: WorkStepId,
    pub url: String,
    pub generation: u32,
    pub width: u32,
    pub height: u32,
}

pub struct WorkFrameStore {
    root: PathBuf,
    retired: std::sync::Mutex<std::collections::HashSet<ProfileId>>,
}
impl WorkFrameStore {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            retired: Default::default(),
        }
    }
    /// Closes writers before erasing the profile's exact frame directory.
    pub fn retire_profile(&self, profile: ProfileId) -> std::io::Result<()> {
        let mut retired = self.retired.lock().unwrap_or_else(|p| p.into_inner());
        retired.insert(profile);
        match std::fs::symlink_metadata(&self.root) {
            Ok(meta) if meta.file_type().is_dir() => {}
            Ok(_) => return Err(std::io::Error::from(std::io::ErrorKind::InvalidInput)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        }
        let dir = self.dir(profile);
        match std::fs::symlink_metadata(&dir) {
            Ok(meta) if meta.file_type().is_dir() => std::fs::remove_dir_all(&dir)?,
            Ok(_) => std::fs::remove_file(&dir)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        #[cfg(unix)]
        std::fs::File::open(&self.root)?.sync_all()?;
        Ok(())
    }
    fn dir(&self, profile: ProfileId) -> PathBuf {
        self.root.join(profile.to_string())
    }
    fn stem(record_attempt: WorkAttemptId, step: WorkStepId) -> String {
        format!("{record_attempt}-{step}")
    }
    /// Writes both files atomically; the oldest frames make room past the cap.
    pub fn put(
        &self,
        profile: ProfileId,
        record: &WorkFrameRecord,
        png: &[u8],
    ) -> std::io::Result<()> {
        let retired = self.retired.lock().unwrap_or_else(|p| p.into_inner());
        if retired.contains(&profile) {
            return Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
        }
        if png.len() > MAX_FRAME_BYTES || record.url.len() > MAX_URL_BYTES {
            return Err(std::io::Error::other("frame out of bounds"));
        }
        let dir = self.dir(profile);
        std::fs::create_dir_all(&dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&self.root, std::fs::Permissions::from_mode(0o700)).ok();
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
        }
        let stem = Self::stem(record.attempt, record.step);
        if !dir.join(format!("{stem}.png")).is_file() {
            self.make_room(&dir);
        }
        let meta = serde_json::to_vec(record).map_err(std::io::Error::other)?;
        write_atomic(&dir.join(format!("{stem}.json")), &meta)?;
        write_atomic(&dir.join(format!("{stem}.png")), png)
    }
    /// Every stored frame of one work, oldest first.
    pub fn list(&self, profile: ProfileId, work: WorkId) -> Vec<WorkFrameRecord> {
        let retired = self.retired.lock().unwrap_or_else(|p| p.into_inner());
        if retired.contains(&profile) {
            return Vec::new();
        }
        let mut records: Vec<(std::time::SystemTime, WorkFrameRecord)> = Vec::new();
        let Ok(entries) = std::fs::read_dir(self.dir(profile)) else {
            return Vec::new();
        };
        for entry in entries.flatten().take(MAX_PROFILE_FRAMES * 2) {
            let path = entry.path();
            if path.extension().is_none_or(|ext| ext != "json") {
                continue;
            }
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if metadata.len() > 8192 {
                continue;
            }
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            let Ok(record) = serde_json::from_slice::<WorkFrameRecord>(&bytes) else {
                continue;
            };
            if record.work == work && path.with_extension("png").is_file() {
                let modified = metadata.modified().unwrap_or(std::time::UNIX_EPOCH);
                records.push((modified, record));
            }
        }
        records.sort_by_key(|(modified, _)| *modified);
        records.into_iter().map(|(_, record)| record).collect()
    }
    pub fn read(
        &self,
        profile: ProfileId,
        attempt: WorkAttemptId,
        step: WorkStepId,
    ) -> Option<Vec<u8>> {
        let retired = self.retired.lock().unwrap_or_else(|p| p.into_inner());
        if retired.contains(&profile) {
            return None;
        }
        let path = self
            .dir(profile)
            .join(format!("{}.png", Self::stem(attempt, step)));
        let metadata = std::fs::metadata(&path).ok()?;
        if !metadata.is_file() || metadata.len() > MAX_FRAME_BYTES as u64 {
            return None;
        }
        std::fs::read(path).ok()
    }
    pub fn remove_work(&self, profile: ProfileId, work: WorkId) {
        let dir = self.dir(profile);
        for record in self.list(profile, work) {
            let stem = Self::stem(record.attempt, record.step);
            let _ = std::fs::remove_file(dir.join(format!("{stem}.png")));
            let _ = std::fs::remove_file(dir.join(format!("{stem}.json")));
        }
    }
    fn make_room(&self, dir: &std::path::Path) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut pngs: Vec<(std::time::SystemTime, PathBuf)> = entries
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                (path.extension().is_some_and(|ext| ext == "png")).then(|| {
                    let modified = entry
                        .metadata()
                        .and_then(|m| m.modified())
                        .unwrap_or(std::time::UNIX_EPOCH);
                    (modified, path)
                })
            })
            .collect();
        if pngs.len() < MAX_PROFILE_FRAMES {
            return;
        }
        pngs.sort_by_key(|(modified, _)| *modified);
        for (_, path) in pngs.iter().take(pngs.len() + 1 - MAX_PROFILE_FRAMES) {
            let _ = std::fs::remove_file(path);
            let _ = std::fs::remove_file(path.with_extension("json"));
        }
    }
}

fn write_atomic(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| std::io::Error::other("no parent"))?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| std::io::Error::other("no name"))?;
    let temp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    std::fs::write(&temp, bytes)?;
    std::fs::rename(&temp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&temp);
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn record(work: WorkId, n: u32) -> WorkFrameRecord {
        WorkFrameRecord {
            work,
            execution: WorkExecutionId::generate(),
            attempt: WorkAttemptId::generate(),
            step: WorkStepId::generate(),
            url: format!("https://example.com/{n}"),
            generation: n,
            width: 640,
            height: 400,
        }
    }
    #[test]
    fn frames_round_trip_per_work_and_stay_bounded() {
        let dir = std::env::temp_dir().join(format!("zephium-frames-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = WorkFrameStore::new(dir.clone());
        let profile = ProfileId::generate();
        let (a, b) = (WorkId::generate(), WorkId::generate());
        let first = record(a, 1);
        store.put(profile, &first, b"png-a").unwrap();
        store.put(profile, &record(b, 2), b"png-b").unwrap();
        assert_eq!(store.list(profile, a), vec![first.clone()]);
        assert_eq!(
            store.read(profile, first.attempt, first.step).unwrap(),
            b"png-a"
        );
        assert!(store
            .put(profile, &first, &vec![0u8; MAX_FRAME_BYTES + 1])
            .is_err());
        store.remove_work(profile, a);
        assert!(store.list(profile, a).is_empty());
        assert_eq!(store.list(profile, b).len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn retired_profile_frames_cannot_be_recreated_by_late_publishers() {
        let dir = tempfile::tempdir().unwrap();
        let store = WorkFrameStore::new(dir.path().join("frames"));
        let profile = ProfileId::from(99);
        let record = record(WorkId::from(1), 1);
        store.put(profile, &record, b"private pixels").unwrap();
        store.retire_profile(profile).unwrap();
        assert!(store.put(profile, &record, b"late pixels").is_err());
        assert!(store.read(profile, record.attempt, record.step).is_none());
        assert!(!store.dir(profile).exists());
    }
}
