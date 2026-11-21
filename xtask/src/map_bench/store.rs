use std::fs::{self, File};
use std::io::{ErrorKind, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;
use zephium_public_fetch::{
    PublicBody, ReadRefusal, SizeLimit, StatusClass, Timings, MAX_DECODED_BYTES,
};

use super::corpus::valid_id;
use super::engine::{
    url_text, Expectation, FetchRefusal, FetchRequest, FetchResult, ResultsPage, MAX_REQUESTS,
};
use super::{BenchError, Result};

pub(super) const MAX_ARTIFACT_BYTES: usize = 2 * 1024 * 1024;
const JOURNAL: &str = "requests.jsonl";
const LOCK: &str = ".record.lock";
const CHECKPOINT_PREFIX: &str = ".map-bench-checkpoint-";

pub(super) fn external_path(path: &Path, repository: &Path) -> Result<PathBuf> {
    let absolute =
        std::path::absolute(path).map_err(|error| BenchError::io("absolute_path", &error))?;
    let repository = repository
        .canonicalize()
        .map_err(|error| BenchError::io("repository_path", &error))?;
    let mut ancestor = absolute.as_path();
    let mut missing = Vec::new();
    let mut resolved = loop {
        match ancestor.canonicalize() {
            Ok(path) => break path,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                if ancestor.symlink_metadata().is_ok() {
                    return Err(BenchError::new("unresolved_symlink"));
                }
                let name = ancestor.components().next_back();
                let (Some(Component::Normal(name)), Some(parent)) = (name, ancestor.parent())
                else {
                    return Err(BenchError::new("unresolved_path"));
                };
                missing.push(name);
                ancestor = parent;
            }
            Err(error) => return Err(BenchError::io("canonical_path", &error)),
        }
    };
    resolved.extend(missing.into_iter().rev());
    if resolved.starts_with(&repository)
        || resolved
            .ancestors()
            .any(|ancestor| ancestor.join(".git").symlink_metadata().is_ok())
    {
        return Err(BenchError::new("path_inside_repository"));
    }
    Ok(resolved)
}

fn node(path: &Path) -> Result<Option<fs::FileType>> {
    match path.symlink_metadata() {
        Ok(metadata) if super::platform::redirects(&metadata) => {
            Err(BenchError::new("artifact_symlink"))
        }
        Ok(metadata) => Ok(Some(metadata.file_type())),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(BenchError::io("artifact_metadata", &error)),
    }
}

pub(super) fn exists(path: &Path) -> Result<bool> {
    match node(path)? {
        Some(kind) if kind.is_file() => Ok(true),
        None => Ok(false),
        _ => Err(BenchError::new("expected_artifact_file")),
    }
}

pub(super) fn directory(path: &Path, create: bool) -> Result<bool> {
    match node(path)? {
        Some(kind) if kind.is_dir() => Ok(true),
        Some(_) => Err(BenchError::new("expected_artifact_directory")),
        None if !create => Ok(false),
        None => {
            let mut builder = fs::DirBuilder::new();
            super::platform::private_directory(&mut builder);
            match builder.create(path) {
                Ok(()) => Ok(true),
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                    if directory(path, false)? {
                        Ok(true)
                    } else {
                        Err(BenchError::new("directory_creation_race"))
                    }
                }
                Err(error) => Err(BenchError::io("create_directory", &error)),
            }
        }
    }
}

pub(super) fn create_parents(path: &Path) -> Result<()> {
    let mut missing = Vec::new();
    let mut current = path;
    while !directory(current, false)? {
        missing.push(current);
        current = current
            .parent()
            .ok_or(BenchError::new("directory_has_no_parent"))?;
    }
    for path in missing.into_iter().rev() {
        directory(path, true)?;
    }
    Ok(())
}

pub(super) fn record_lock(root: &Path, create: bool) -> Result<File> {
    let path = root.join(LOCK);
    if !exists(&path)? && !create {
        return Err(BenchError::new("missing_record_lock"));
    }
    let file = File::options()
        .create(create)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .map_err(|error| BenchError::io("open_record_lock", &error))?;
    file.try_lock()
        .map_err(|_| BenchError::new("recording_in_use"))?;
    Ok(file)
}

pub(super) fn read_lock(root: &Path) -> Result<File> {
    let path = root.join(LOCK);
    if !exists(&path)? {
        return Err(BenchError::new("missing_record_lock"));
    }
    let file = File::open(path).map_err(|error| BenchError::io("open_record_lock", &error))?;
    file.try_lock_shared()
        .map_err(|_| BenchError::new("recording_in_use"))?;
    Ok(file)
}

pub(super) fn ensure_empty_run(root: &Path) -> Result<()> {
    for entry in children(root)? {
        let entry = entry.map_err(|error| BenchError::io("list_run_entry", &error))?;
        let name = entry.file_name();
        let checkpoint = name
            .to_str()
            .is_some_and(|name| name.starts_with(CHECKPOINT_PREFIX))
            && exists(&entry.path())?;
        if name != LOCK && !checkpoint {
            return Err(BenchError::new("run_directory_not_empty"));
        }
    }
    Ok(())
}

pub(super) fn children(path: &Path) -> Result<fs::ReadDir> {
    fs::read_dir(path).map_err(|error| BenchError::io("list_artifacts", &error))
}

pub(super) fn query_dir(root: &Path, id: &str, create: bool) -> Result<PathBuf> {
    if !valid_id(id) {
        return Err(BenchError::new("invalid_query_id"));
    }
    let queries = root.join("queries");
    directory(&queries, create)?;
    let path = queries.join(id);
    directory(&path, create)?;
    Ok(path)
}

pub(super) fn read_bytes(path: &Path, max: usize) -> Result<Vec<u8>> {
    if !exists(path)? {
        return Err(BenchError::new("missing_artifact"));
    }
    let file = File::open(path).map_err(|error| BenchError::io("open_artifact", &error))?;
    let mut bytes = Vec::new();
    file.take(max as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| BenchError::io("read_artifact", &error))?;
    if bytes.len() > max {
        return Err(BenchError::new("artifact_size_limit"));
    }
    Ok(bytes)
}

pub(super) fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let bytes = read_bytes(path, MAX_ARTIFACT_BYTES)?;
    serde_json::from_slice(&bytes)
        .map_err(|error| BenchError::new("invalid_json_artifact").line(error.line()))
}

pub(super) fn optional_json<T: DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    exists(path)?.then(|| read_json(path)).transpose()
}

pub(super) fn write_json(path: &Path, data: &impl Serialize) -> Result<()> {
    let mut output = BoundedBuffer::default();
    serde_json::to_writer(&mut output, data).map_err(|_| output.error())?;
    write_blob(path, &output.bytes)
}

#[derive(Default)]
struct BoundedBuffer {
    bytes: Vec<u8>,
    refused: bool,
}

impl BoundedBuffer {
    fn error(&self) -> BenchError {
        BenchError::new(if self.refused {
            "artifact_size_limit"
        } else {
            "serialize_artifact"
        })
    }
}

impl Write for BoundedBuffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > MAX_ARTIFACT_BYTES - self.bytes.len() {
            self.refused = true;
            return Err(ErrorKind::FileTooLarge.into());
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub(super) fn write_blob(path: &Path, bytes: &[u8]) -> Result<()> {
    exists(path)?;
    let parent = path
        .parent()
        .ok_or(BenchError::new("artifact_has_no_parent"))?;
    let mut temp = tempfile::Builder::new()
        .prefix(CHECKPOINT_PREFIX)
        .tempfile_in(parent)
        .map_err(|error| BenchError::io("create_checkpoint", &error))?;
    temp.write_all(bytes)
        .and_then(|()| temp.as_file().sync_all())
        .map_err(|error| BenchError::io("write_checkpoint", &error))?;
    temp.persist(path)
        .map_err(|error| BenchError::io("publish_checkpoint", &error.error))?;
    Ok(())
}

pub(super) fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub(super) fn valid_digest(value: &str) -> bool {
    const SHA256_HEX_BYTES: usize = 64;
    value.len() == SHA256_HEX_BYTES
        && value
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

fn write_body(root: &Path, query: &Path, bytes: &[u8]) -> Result<String> {
    if bytes.len() > MAX_DECODED_BYTES {
        return Err(BenchError::new("body_size_limit"));
    }
    let hash = digest(bytes);
    let bodies = root.join("bodies");
    directory(&bodies, true)?;
    let canonical = bodies.join(&hash);
    if exists(&canonical)? {
        read_body(root, &hash, bytes.len())?;
    } else {
        write_blob(&canonical, bytes)?;
    }
    let link_dir = query.join("bodies");
    directory(&link_dir, true)?;
    let link = link_dir.join(&hash);
    if !exists(&link)? {
        fs::hard_link(canonical, link).map_err(|error| BenchError::io("link_body", &error))?;
    }
    Ok(hash)
}

fn read_body(root: &Path, hash: &str, length: usize) -> Result<Vec<u8>> {
    if !valid_digest(hash) || length > MAX_DECODED_BYTES {
        return Err(BenchError::new("invalid_body_reference"));
    }
    let bodies = root.join("bodies");
    directory(&bodies, false)?;
    let bytes = read_bytes(&bodies.join(hash), length)?;
    if bytes.len() != length || digest(&bytes) != hash {
        return Err(BenchError::new("body_digest_mismatch"));
    }
    Ok(bytes)
}

#[derive(Deserialize, Eq, PartialEq, Serialize)]
struct RequestKey {
    method: Method,
    #[serde(with = "url_text")]
    url: Url,
    expectation: Expectation,
}

impl From<&FetchRequest> for RequestKey {
    fn from(request: &FetchRequest) -> Self {
        let mut url = request.url.clone();
        url.set_fragment(None);
        Self {
            method: Method::Get,
            url,
            expectation: request.expect,
        }
    }
}

#[derive(Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "UPPERCASE")]
enum Method {
    Get,
}

#[derive(Deserialize, Serialize)]
#[serde(untagged)]
enum ResultClass {
    Body(BodyClass),
    Refused(FetchRefusal),
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum BodyClass {
    Body,
}

#[derive(Deserialize, Serialize)]
struct RequestRow {
    key: RequestKey,
    cap: usize,
    timeout: Duration,
    result_class: ResultClass,
    wire_bytes: Option<u64>,
    decoded_bytes: usize,
    timings: TransferTimes,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    elapsed_at_start: Option<Duration>,
    elapsed_at_return: Duration,
    #[serde(default)]
    completion_order: Option<usize>,
    body: Option<BodyRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    robots_denied_origin: Option<JournalUrl>,
}

#[derive(Deserialize, Serialize)]
struct JournalUrl(#[serde(with = "url_text")] Url);

#[derive(Deserialize, Serialize)]
struct TransferTimes {
    queued: Option<Duration>,
    connect: Option<Duration>,
    first_byte: Option<Duration>,
    total: Duration,
}

#[derive(Deserialize, Serialize)]
struct BodyRef {
    #[serde(with = "url_text")]
    final_url: Url,
    final_url_hash: String,
    content_type: Option<String>,
    charset: Option<String>,
    hops: u8,
    http_status: u16,
    truncated: bool,
    sha256: String,
}

impl RequestRow {
    fn reserve(request: &FetchRequest, elapsed: Duration) -> Self {
        Self {
            key: request.into(),
            cap: request.max_bytes,
            timeout: request.timeout,
            result_class: ResultClass::Refused(FetchRefusal::Interrupted),
            wire_bytes: None,
            decoded_bytes: 0,
            timings: TransferTimes {
                queued: None,
                connect: None,
                first_byte: None,
                total: Duration::ZERO,
            },
            elapsed_at_start: Some(elapsed),
            elapsed_at_return: elapsed,
            completion_order: None,
            body: None,
            robots_denied_origin: None,
        }
    }

    fn finish(
        &mut self,
        root: &Path,
        query: &Path,
        fetch: &FetchResult,
        transfer: Transfer,
    ) -> Result<()> {
        self.elapsed_at_start = Some(transfer.start);
        self.elapsed_at_return = transfer.end;
        self.wire_bytes = transfer.wire_bytes;
        self.timings.total = transfer.total;
        match fetch {
            Ok(body) => {
                let sha256 = write_body(root, query, &body.bytes)?;
                self.decoded_bytes = body.bytes.len();
                self.result_class = ResultClass::Body(BodyClass::Body);
                self.timings = TransferTimes {
                    queued: Some(body.timings.queued),
                    connect: body.timings.connect,
                    first_byte: Some(body.timings.first_byte),
                    total: body.timings.total,
                };
                self.body = Some(BodyRef {
                    final_url: body.final_url.clone(),
                    final_url_hash: digest(body.final_url.as_str().as_bytes()),
                    content_type: body.media_type.clone(),
                    charset: body.charset.clone(),
                    hops: body.hops,
                    http_status: match body.status {
                        StatusClass::Ok => 200,
                        StatusClass::NonAuthoritative => 203,
                    },
                    truncated: body.truncated,
                    sha256,
                });
            }
            Err(class) => self.result_class = ResultClass::Refused(*class),
        }
        Ok(())
    }

    fn answers(&self, request: &FetchRequest) -> bool {
        let completion = self.elapsed_at_start.map_or(self.timings.total, |start| {
            self.elapsed_at_return
                .saturating_sub(start)
                .max(self.timings.total)
        });
        match (&self.result_class, &self.body) {
            (ResultClass::Body(_), Some(body)) => {
                request.timeout >= completion
                    && if body.truncated {
                        request.max_bytes <= self.cap
                    } else {
                        request.max_bytes >= self.decoded_bytes
                    }
            }
            (ResultClass::Refused(FetchRefusal::Reader(class)), None) => match class {
                ReadRefusal::TooLarge(SizeLimit::Headers) => request.timeout >= completion,
                ReadRefusal::TooLarge(SizeLimit::Wire) | ReadRefusal::Encoding => {
                    request.max_bytes >= self.cap && request.timeout >= completion
                }
                ReadRefusal::Timeout(_) => request.timeout <= self.timeout,
                ReadRefusal::NotPublicHttps
                | ReadRefusal::RedirectRefused
                | ReadRefusal::TooManyRedirects
                | ReadRefusal::DnsRefused
                | ReadRefusal::ProxyUnsupported
                | ReadRefusal::RouteChanged
                | ReadRefusal::Transport => true,
                ReadRefusal::Status(_) | ReadRefusal::WrongContentType => {
                    request.timeout >= completion
                }
            },
            _ => false,
        }
    }

    fn result(&self, query: &Path) -> Result<FetchResult> {
        match (&self.result_class, &self.body) {
            (ResultClass::Refused(class), None) if self.decoded_bytes == 0 => Ok(Err(*class)),
            (ResultClass::Body(_), Some(body)) => {
                let status = match body.http_status {
                    200 => StatusClass::Ok,
                    203 => StatusClass::NonAuthoritative,
                    _ => return Err(BenchError::new("invalid_body_status")),
                };
                if body.final_url_hash != digest(body.final_url.as_str().as_bytes()) {
                    return Err(BenchError::new("final_url_digest_mismatch"));
                }
                Ok(Ok(PublicBody {
                    final_url: body.final_url.clone(),
                    hops: body.hops,
                    status,
                    media_type: body.content_type.clone(),
                    charset: body.charset.clone(),
                    bytes: read_body(query, &body.sha256, self.decoded_bytes)?,
                    truncated: body.truncated,
                    wire_bytes: self
                        .wire_bytes
                        .ok_or(BenchError::new("missing_wire_bytes"))?,
                    timings: Timings {
                        queued: self
                            .timings
                            .queued
                            .ok_or(BenchError::new("missing_queue_time"))?,
                        connect: self.timings.connect,
                        first_byte: self
                            .timings
                            .first_byte
                            .ok_or(BenchError::new("missing_first_byte_time"))?,
                        total: self.timings.total,
                    },
                }))
            }
            _ => Err(BenchError::new("invalid_request_result")),
        }
    }
}

#[derive(Default)]
pub(super) struct Journal(Vec<RequestRow>);

pub(super) struct Transfer {
    pub(super) wire_bytes: Option<u64>,
    pub(super) start: Duration,
    pub(super) total: Duration,
    pub(super) end: Duration,
}

pub(super) enum MatchPolicy {
    Probe,
    Exact,
    Replay,
}

pub(super) struct RecordedMatch {
    pub(super) index: usize,
    pub(super) end: Duration,
    pub(super) order: usize,
    pub(super) relaxed_cap: Option<usize>,
}

impl Journal {
    pub(super) fn load(query: &Path) -> Result<Self> {
        let path = query.join(JOURNAL);
        if !exists(&path)? {
            return Ok(Self::default());
        }
        let bytes = read_bytes(&path, MAX_ARTIFACT_BYTES)?;
        if bytes.is_empty() {
            return Ok(Self::default());
        }
        let Some(body) = bytes.strip_suffix(b"\n") else {
            return Err(BenchError::new("invalid_request_journal"));
        };
        let mut journal = Self::default();
        for (index, line) in body.split(|&byte| byte == b'\n').enumerate() {
            if index >= MAX_REQUESTS {
                return Err(BenchError::new("request_limit"));
            }
            let row: RequestRow = serde_json::from_slice(line)
                .map_err(|_| BenchError::new("invalid_request_journal").line(index + 1))?;
            if row
                .elapsed_at_start
                .is_some_and(|start| start > row.elapsed_at_return)
                || row.completion_order.is_some_and(|order| {
                    order >= MAX_REQUESTS
                        || journal
                            .0
                            .iter()
                            .any(|prior| prior.completion_order == Some(order))
                })
            {
                return Err(BenchError::new("invalid_request_timeline").line(index + 1));
            }
            journal.0.push(row);
        }
        Ok(journal)
    }

    fn save(&self, query: &Path) -> Result<()> {
        if self.0.is_empty() {
            return Ok(());
        }
        let mut output = BoundedBuffer::default();
        for row in &self.0 {
            serde_json::to_writer(&mut output, row).map_err(|_| output.error())?;
            output.write_all(b"\n").map_err(|_| output.error())?;
        }
        write_blob(&query.join(JOURNAL), &output.bytes)
    }

    pub(super) fn initialize(&self, query: &Path) -> Result<()> {
        let path = query.join(JOURNAL);
        if !exists(&path)? {
            write_blob(&path, b"")?;
        }
        Ok(())
    }

    pub(super) fn find(
        &self,
        request: &FetchRequest,
        mut eligible: impl FnMut(usize) -> bool,
        policy: MatchPolicy,
    ) -> Option<RecordedMatch> {
        let key = request.into();
        let mut fallback = None;
        for (index, row) in self.0.iter().enumerate() {
            if row.key != key || !eligible(index) {
                continue;
            }
            let matched = |relaxed_cap| RecordedMatch {
                index,
                end: row.elapsed_at_return,
                order: row.completion_order.unwrap_or(index),
                relaxed_cap,
            };
            if matches!(policy, MatchPolicy::Probe)
                || (row.cap == request.max_bytes && row.timeout == request.timeout)
            {
                return Some(matched(None));
            }
            if matches!(policy, MatchPolicy::Replay) && fallback.is_none() && row.answers(request) {
                fallback = Some(matched(Some(request.max_bytes)));
            }
        }
        fallback
    }

    pub(super) fn result(&self, index: usize, query: &Path) -> Result<FetchResult> {
        self.0
            .get(index)
            .ok_or(BenchError::new("missing_request_reservation"))?
            .result(query)
    }

    pub(super) fn robots_denied_origin(&self, index: usize) -> Option<&Url> {
        self.0
            .get(index)?
            .robots_denied_origin
            .as_ref()
            .map(|url| &url.0)
    }

    pub(super) fn elapsed(&self) -> Duration {
        self.0
            .iter()
            .map(|row| row.elapsed_at_return)
            .max()
            .unwrap_or_default()
    }

    pub(super) fn reserve(
        &mut self,
        request: &FetchRequest,
        query: &Path,
        elapsed: Duration,
    ) -> Result<usize> {
        if self.0.len() >= MAX_REQUESTS {
            return Err(BenchError::new("request_limit"));
        }
        let index = self.0.len();
        self.0.push(RequestRow::reserve(request, elapsed));
        self.save(query)?;
        Ok(index)
    }

    pub(super) fn finish(
        &mut self,
        root: &Path,
        query: &Path,
        index: usize,
        fetch: &FetchResult,
        transfer: Transfer,
        robots_denied_origin: Option<Url>,
    ) -> Result<()> {
        let order = self.0.iter().filter_map(|row| row.completion_order).max();
        let row = self
            .0
            .get_mut(index)
            .ok_or(BenchError::new("missing_request_reservation"))?;
        row.finish(root, query, fetch, transfer)?;
        row.robots_denied_origin = robots_denied_origin.map(JournalUrl);
        row.completion_order = Some(order.map_or(0, |order| order + 1));
        self.save(query)
    }

    pub(super) fn prior_transfer(&self) -> (u64, u64, u64) {
        self.0
            .iter()
            .fold((0_u64, 0_u64, 0_u64), |(reads, wire, decoded), row| {
                let completed = !matches!(
                    row.result_class,
                    ResultClass::Refused(FetchRefusal::Interrupted)
                );
                (
                    reads + u64::from(completed),
                    wire.saturating_add(row.wire_bytes.unwrap_or_default()),
                    decoded.saturating_add(row.decoded_bytes as u64),
                )
            })
    }

    pub(super) fn prior_refusals(&self) -> impl Iterator<Item = FetchRefusal> + '_ {
        self.0.iter().filter_map(|row| match row.result_class {
            ResultClass::Refused(class) => Some(class),
            _ => None,
        })
    }
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub(super) enum ResultsSlot {
    Unavailable,
    Available {
        #[serde(with = "url_text")]
        url: Url,
        sha256: String,
        decoded_bytes: usize,
        elapsed_at_return: Duration,
        #[serde(default)]
        synthetic: bool,
    },
}

impl ResultsSlot {
    pub(super) fn initialize(query: &Path) -> Result<()> {
        let path = query.join("results-page.json");
        if !exists(&path)? {
            write_json(&path, &Self::Unavailable)?;
        }
        Ok(())
    }

    pub(super) fn load(query: &Path) -> Result<(ResultsPage, Duration)> {
        match optional_json(&query.join("results-page.json"))? {
            Some(Self::Available {
                url,
                sha256,
                decoded_bytes,
                elapsed_at_return,
                synthetic,
            }) => Ok((
                ResultsPage::Available {
                    url,
                    bytes: read_body(query, &sha256, decoded_bytes)?,
                    synthetic,
                },
                elapsed_at_return,
            )),
            _ => Ok((ResultsPage::Unavailable, Duration::ZERO)),
        }
    }

    pub(super) fn save(
        root: &Path,
        query: &Path,
        url: &Url,
        bytes: &[u8],
        elapsed: Duration,
    ) -> Result<()> {
        let sha256 = write_body(root, query, bytes)?;
        write_json(
            &query.join("results-page.json"),
            &Self::Available {
                url: url.clone(),
                sha256,
                decoded_bytes: bytes.len(),
                elapsed_at_return: elapsed,
                synthetic: true,
            },
        )
    }
}
