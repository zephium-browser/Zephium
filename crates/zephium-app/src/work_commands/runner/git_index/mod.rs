//! Current-index content inspection through a guarded metadata broker. No config,
//! hooks, alternates, history, original index or pack reaches the confined child.
mod index;
mod pack;
#[cfg(test)]
mod tests;
use super::{Failure, Output, ReadOperation};
#[cfg(not(target_os = "macos"))]
use super::{GitOperation, Snapshot};
use crate::work_files::{
    git_metadata::{GitMetadata, GitMetadataBytes, GitMetadataPath},
    WorkFileError, WorkFileGrant,
};
use sha1::{Digest, Sha1};
use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
    io::Read,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
type Oid = [u8; 20];
const MAX_INDEX: usize = 16 * 1024 * 1024;
const MAX_OBJECTS: usize = 100_000;
const MAX_BLOB: usize = 1024 * 1024;
const MAX_CONTENT: usize = 32 * 1024 * 1024;
const MAX_INPUT: usize = 128 * 1024 * 1024;
const MAX_INFLATED: usize = 64 * 1024 * 1024;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Error {
    Input,
    Unsupported,
    Invalid,
    Limit,
    Missing,
    Denied,
    Changed,
    Stopped,
    Runner(Failure),
}
impl From<Failure> for Error {
    fn from(error: Failure) -> Self {
        match error {
            Failure::Stopped => Self::Stopped,
            Failure::Limit => Self::Limit,
            error => Self::Runner(error),
        }
    }
}
impl super::worker::WorkerError for Error {
    fn stopped(&self) -> bool {
        matches!(self, Self::Stopped | Self::Runner(Failure::Stopped))
    }
    fn unsettled(&self) -> bool {
        matches!(self, Self::Runner(failure) if failure.unsettled())
    }
}
impl Error {
    pub fn note(self) -> &'static str {
        match self{
        Self::Input=>"Choose 1 to 64 ordinary file paths inside the working folder",
        Self::Unsupported=>"This Git index, object layout or selected entry is not supported",
        Self::Invalid=>"The Git data could not be verified",
        Self::Limit=>"The Git inspection exceeded its bounded data or time limit",
        Self::Missing=>"A selected file or its current-index object is unavailable",
        Self::Denied=>"The selected Git content is not available to Work",
        Self::Changed=>"The index or a selected working file changed during inspection; retry after it settles",
        Self::Stopped=>"The Git inspection stopped",
        Self::Runner(error)=>error.note(),
    }
    }
}
fn file_error(error: WorkFileError) -> Error {
    match error {
        WorkFileError::NotFound => Error::Missing,
        WorkFileError::TooLarge => Error::Limit,
        WorkFileError::Changed => Error::Changed,
        _ => Error::Denied,
    }
}
fn hash(bytes: &[u8]) -> Oid {
    Sha1::digest(bytes).into()
}
fn blob_hash(bytes: &[u8]) -> Oid {
    let mut hash = Sha1::new();
    hash.update(format!("blob {}\0", bytes.len()));
    hash.update(bytes);
    hash.finalize().into()
}
struct Budget {
    input: usize,
    output: usize,
    started: Instant,
    cancelled: Arc<AtomicBool>,
}
impl Budget {
    fn new() -> Self {
        Self {
            input: 0,
            output: 0,
            started: Instant::now(),
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }
    fn with_cancel(cancelled: Arc<AtomicBool>) -> Self {
        Self {
            cancelled,
            ..Self::new()
        }
    }
    fn check(&self) -> Result<(), Error> {
        if self.cancelled.load(Ordering::Acquire) {
            return Err(Error::Stopped);
        }
        if self.started.elapsed() > Duration::from_secs(5) {
            Err(Error::Limit)
        } else {
            Ok(())
        }
    }
    fn input(&mut self, amount: usize) -> Result<(), Error> {
        self.check()?;
        self.input = self
            .input
            .checked_add(amount)
            .filter(|n| *n <= MAX_INPUT)
            .ok_or(Error::Limit)?;
        Ok(())
    }
    fn output(&mut self, amount: usize) -> Result<(), Error> {
        self.check()?;
        self.output = self
            .output
            .checked_add(amount)
            .filter(|n| *n <= MAX_INFLATED)
            .ok_or(Error::Limit)?;
        Ok(())
    }
}
struct Objects<'a> {
    metadata: &'a GitMetadata,
    protected: &'a BTreeSet<Oid>,
    packs: Vec<Arc<pack::Pack>>,
    pack_ids: Option<Vec<Oid>>,
    next_pack: usize,
    budget: Budget,
}
impl<'a> Objects<'a> {
    fn new(metadata: &'a GitMetadata, protected: &'a BTreeSet<Oid>, budget: Budget) -> Self {
        Self {
            metadata,
            protected,
            packs: vec![],
            pack_ids: None,
            next_pack: 0,
            budget,
        }
    }
    fn read(&mut self, path: GitMetadataPath, limit: usize) -> Result<GitMetadataBytes, Error> {
        self.budget.check()?;
        let remaining = MAX_INPUT
            .checked_sub(self.budget.input)
            .filter(|n| *n > 0)
            .ok_or(Error::Limit)?;
        let file = self
            .metadata
            .read(path, limit.min(remaining))
            .map_err(file_error)?;
        self.budget.input(file.bytes.len())?;
        Ok(file)
    }
    fn loose(&mut self, id: Oid) -> Result<Option<Vec<u8>>, Error> {
        let raw = match self.read(GitMetadataPath::Loose(id), 2 * MAX_BLOB) {
            Ok(raw) => raw,
            Err(Error::Missing) => return Ok(None),
            Err(error) => return Err(error),
        };
        let mut decoder = flate2::bufread::ZlibDecoder::new(raw.bytes.as_slice());
        let mut header = Vec::new();
        loop {
            let mut byte = [0];
            decoder.read_exact(&mut byte).map_err(|_| Error::Invalid)?;
            if byte[0] == 0 {
                break;
            }
            header.push(byte[0]);
            if header.len() > 64 {
                return Err(Error::Invalid);
            }
        }
        let length = std::str::from_utf8(&header)
            .map_err(|_| Error::Invalid)?
            .strip_prefix("blob ")
            .ok_or(Error::Unsupported)?;
        if length.is_empty()
            || !length.bytes().all(|byte| byte.is_ascii_digit())
            || (length.len() > 1 && length.starts_with('0'))
        {
            return Err(Error::Invalid);
        }
        let length = length.parse::<usize>().map_err(|_| Error::Invalid)?;
        if length > MAX_BLOB {
            return Err(Error::Limit);
        }
        self.budget.output(length)?;
        let mut content = Vec::new();
        decoder
            .by_ref()
            .take(length as u64 + 1)
            .read_to_end(&mut content)
            .map_err(|_| Error::Invalid)?;
        if content.len() != length
            || decoder.total_in() as usize != raw.bytes.len()
            || blob_hash(&content) != id
        {
            return Err(Error::Invalid);
        }
        Ok(Some(content))
    }
    fn pack_for(&mut self, id: &Oid) -> Result<Arc<pack::Pack>, Error> {
        if let Some(pack) = self.packs.iter().find(|pack| pack.contains(id)) {
            return Ok(pack.clone());
        }
        if self.pack_ids.is_none() {
            self.pack_ids = Some(self.metadata.pack_ids().map_err(file_error)?);
        }
        while let Some(pack_id) = self
            .pack_ids
            .as_ref()
            .and_then(|ids| ids.get(self.next_pack))
            .copied()
        {
            self.next_pack += 1;
            let index = self.read(GitMetadataPath::PackIndex(pack_id), MAX_INDEX)?;
            let data = self.read(GitMetadataPath::Pack(pack_id), 64 * 1024 * 1024)?;
            let pack = Arc::new(pack::Pack::new(&index.bytes, data.bytes, pack_id)?);
            let contains = pack.contains(id);
            self.packs.push(pack.clone());
            if contains {
                return Ok(pack);
            }
        }
        Err(Error::Missing)
    }
    fn blob(&mut self, id: Oid) -> Result<Vec<u8>, Error> {
        self.decode(id, &mut BTreeSet::new(), 0)
    }
    fn decode(
        &mut self,
        id: Oid,
        visiting: &mut BTreeSet<Oid>,
        depth: usize,
    ) -> Result<Vec<u8>, Error> {
        if self.protected.contains(&id) {
            return Err(Error::Denied);
        }
        if depth >= 32 {
            return Err(Error::Limit);
        }
        if !visiting.insert(id) {
            return Err(Error::Invalid);
        }
        self.budget.check()?;
        let result = if let Some(content) = self.loose(id)? {
            content
        } else {
            let pack = self.pack_for(&id)?;
            match pack.payload(&id, &mut self.budget)? {
                pack::Payload::Blob(content) => content,
                pack::Payload::Delta { base, bytes } => {
                    if self.protected.contains(&base) {
                        return Err(Error::Denied);
                    }
                    let base = self.decode(base, visiting, depth + 1)?;
                    pack::apply_delta(&base, &bytes, &mut self.budget)?
                }
            }
        };
        visiting.remove(&id);
        if blob_hash(&result) != id {
            return Err(Error::Invalid);
        }
        Ok(result)
    }
}
struct Captured {
    metadata: GitMetadata,
    index: GitMetadataBytes,
    before: BTreeMap<PathBuf, Vec<u8>>,
    after: BTreeMap<PathBuf, Option<Vec<u8>>>,
    identities: BTreeMap<PathBuf, Option<(u64, u64)>>,
}
fn capture(
    grant: &WorkFileGrant,
    root: &Path,
    paths: &[String],
    cancelled: Arc<AtomicBool>,
) -> Result<Captured, Error> {
    let budget = Budget::with_cancel(cancelled);
    budget.check()?;
    if paths.is_empty() || paths.len() > 64 {
        return Err(Error::Input);
    }
    let mut selected = BTreeSet::new();
    for path in paths {
        let path = index::relative(path)?;
        if crate::work_files::protected_path(&path) {
            return Err(Error::Denied);
        }
        if !selected.insert(path) {
            return Err(Error::Input);
        }
    }
    let metadata = GitMetadata::open(grant, root).map_err(file_error)?;
    let original = metadata
        .read(GitMetadataPath::Index, MAX_INDEX)
        .map_err(file_error)?;
    let index = index::parse(&original.bytes, &|| budget.check())?;
    let mut objects = Objects::new(&metadata, &index.protected, budget);
    objects.budget.input(original.bytes.len())?;
    let mut before = BTreeMap::new();
    let mut after = BTreeMap::new();
    let mut identities = BTreeMap::new();
    let mut total = 0usize;
    for path in selected {
        objects.budget.check()?;
        let entry = index.entries.get(&path).ok_or(Error::Missing)?;
        if !entry.ordinary {
            return Err(Error::Unsupported);
        }
        if index.protected.contains(&entry.oid) {
            return Err(Error::Denied);
        }
        if !entry.intent {
            let bytes = objects.blob(entry.oid)?;
            if std::str::from_utf8(&bytes).map_or(true, |text| {
                text.chars()
                    .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
            }) {
                return Err(Error::Unsupported);
            }
            if std::str::from_utf8(&bytes).is_ok_and(zephium_agentic::contains_recognized_secret) {
                return Err(Error::Denied);
            }
            total = total.checked_add(bytes.len()).ok_or(Error::Limit)?;
            before.insert(path.clone(), bytes);
        }
        let working = metadata.working(&path, MAX_BLOB).map_err(file_error)?;
        identities.insert(path.clone(), working.as_ref().map(|file| file.identity));
        let bytes = working.map(|file| file.bytes);
        if let Some(bytes) = &bytes {
            if std::str::from_utf8(bytes).map_or(true, |text| {
                text.chars()
                    .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
            }) {
                return Err(Error::Unsupported);
            }
            if std::str::from_utf8(bytes).is_ok_and(zephium_agentic::contains_recognized_secret) {
                return Err(Error::Denied);
            }
            total = total.checked_add(bytes.len()).ok_or(Error::Limit)?;
        }
        if total > MAX_CONTENT {
            return Err(Error::Limit);
        }
        after.insert(path, bytes);
    }
    drop(objects);
    Ok(Captured {
        metadata,
        index: original,
        before,
        after,
        identities,
    })
}
impl Captured {
    fn unchanged(&self, cancelled: &AtomicBool) -> Result<(), Error> {
        if cancelled.load(Ordering::Acquire) {
            return Err(Error::Stopped);
        }
        let now = self
            .metadata
            .read(GitMetadataPath::Index, MAX_INDEX)
            .map_err(file_error)?;
        if now.identity != self.index.identity || now.bytes != self.index.bytes {
            return Err(Error::Changed);
        }
        for (path, expected) in &self.after {
            if cancelled.load(Ordering::Acquire) {
                return Err(Error::Stopped);
            }
            let working = self
                .metadata
                .working(path, MAX_BLOB)
                .map_err(|_| Error::Changed)?;
            if working.as_ref().map(|file| file.identity) != self.identities[path]
                || working.map(|file| file.bytes).as_ref() != expected.as_ref()
            {
                return Err(Error::Changed);
            }
        }
        Ok(())
    }
}
fn slots() -> &'static Arc<super::worker_slots::Slots> {
    static SLOTS: std::sync::OnceLock<Arc<super::worker_slots::Slots>> = std::sync::OnceLock::new();
    SLOTS.get_or_init(|| Arc::new(super::worker_slots::Slots::new(4)))
}
pub(crate) async fn inspect<C, CF>(
    grant: &WorkFileGrant,
    root: &Path,
    paths: &[String],
    operation: ReadOperation,
    cancelled: C,
) -> Result<Output, Error>
where
    C: Fn() -> CF,
    CF: Future<Output = bool>,
{
    super::ensure_available().map_err(Error::Runner)?;
    let (grant, root, paths) = (grant.clone(), root.to_owned(), paths.to_vec());
    super::worker::run(
        slots(),
        &cancelled,
        super::worker::Limits::standard(Duration::from_secs(100)),
        move |control| async move {
            let captured = capture(&grant, &root, &paths, control.cancelled.clone())?;
            control.check().map_err(Error::from)?;
            #[cfg(target_os = "macos")]
            {
                let after = captured
                    .after
                    .iter()
                    .filter_map(|(path, bytes)| {
                        bytes.as_ref().map(|bytes| (path.clone(), bytes.clone()))
                    })
                    .collect();
                control.native();
                let output = super::guard::git(&captured.before, &after, operation, &|| async {
                    control.cancelled.load(Ordering::Acquire)
                })
                .await
                .map_err(Error::from)?;
                control.filesystem();
                captured.unchanged(&control.cancelled)?;
                control.check().map_err(Error::from)?;
                Ok(output)
            }
            #[cfg(not(target_os = "macos"))]
            {
                let mut captured = captured;
                let snapshot =
                    Snapshot::from_files_checked(std::mem::take(&mut captured.before), &|| {
                        control.check()
                    })
                    .map_err(Error::from)?;
                let result = async {
                    let cancelled = || async { control.cancelled.load(Ordering::Acquire) };
                    for operation in [
                        GitOperation::Initialize,
                        GitOperation::CaptureIndex,
                        GitOperation::CommitBaseline,
                    ] {
                        control.check().map_err(Error::from)?;
                        control.native();
                        if !super::execute(&snapshot, operation, &cancelled)
                            .await
                            .map_err(Error::from)?
                            .success
                        {
                            return Err(Error::Invalid);
                        }
                    }
                    control.filesystem();
                    for (path, bytes) in &captured.after {
                        control.check().map_err(Error::from)?;
                        match bytes {
                            Some(bytes) => snapshot.write(path, bytes).map_err(Error::from)?,
                            None => {
                                let target = snapshot.work.join(path);
                                if target.exists() {
                                    std::fs::remove_file(target).map_err(|_| Error::Invalid)?;
                                }
                            }
                        }
                    }
                    control.check().map_err(Error::from)?;
                    control.native();
                    if !super::execute(&snapshot, GitOperation::IntentToAdd, &cancelled)
                        .await
                        .map_err(Error::from)?
                        .success
                    {
                        return Err(Error::Invalid);
                    }
                    control.check().map_err(Error::from)?;
                    let output =
                        super::execute(&snapshot, GitOperation::Read(operation), &cancelled)
                            .await
                            .map_err(Error::from)?;
                    control.filesystem();
                    captured.unchanged(&control.cancelled)?;
                    control.check().map_err(Error::from)?;
                    Ok(output)
                }
                .await;
                control.filesystem();
                super::worker::closed(result, snapshot.close())
            }
        },
    )
    .await
}
