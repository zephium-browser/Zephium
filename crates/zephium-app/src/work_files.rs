//! Bounded file steps inside folders the person granted for one run.
//! Paths resolve through granted roots and protected names are excluded.
//! Read handles refuse symlinks and hard links. Approved writes and edits are
//! atomic. The separate qualified text Trash path keeps durable recovery; generic
//! moves and deletes remain unavailable.
use sha2::{Digest, Sha256};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};
use zephium_core::work::runtime::*;
#[cfg(not(unix))]
use zephium_core::work::WorkArtifactId;

pub(crate) mod git_metadata;
pub mod trash;

#[cfg(unix)]
mod unix_write;
#[cfg(windows)]
mod windows_marker;
#[cfg(unix)]
use unix_write::write_atomic;

const MAX_READ_BYTES: u64 = 1024 * 1024;
const MAX_LIST_ENTRIES: usize = 200;
const MAX_SEARCH_FILES: usize = 2000;
const MAX_SEARCH_HITS: usize = 64;
const SEARCH_BUDGET: Duration = Duration::from_millis(200);
const SKIPPED_DIRS: [&str; 6] = [".git", "node_modules", "target", ".cache", "dist", "build"];
const DENIED_UNDER_HOME: [&str; 8] = [
    ".ssh",
    ".gnupg",
    ".aws",
    ".config/gcloud",
    "Library/Keychains",
    "Library/Application Support/app.zephium",
    "Library/Application Support/app.zephium.dev",
    "Library/Cookies",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkFileError {
    Denied,
    NotFound,
    NotADirectory,
    NotAFile,
    TooLarge,
    Binary,
    Sensitive,
    Ambiguous,
    Io,
    Changed,
    Exists,
    Pattern,
    Unavailable,
}
impl WorkFileError {
    /// Closed wording for the step note and the agent.
    pub fn note(self) -> &'static str {
        match self {
            Self::Denied => "The path is not available to Work",
            Self::Unavailable => "File changes are unavailable until confined writes and recoverable deletion are supported",
            Self::NotFound => "No such file or folder",
            Self::NotADirectory => "Not a folder",
            Self::NotAFile => "Not a file",
            Self::TooLarge => "Larger than the read limit",
            Self::Binary => "Not a text file",
            Self::Sensitive => "Recognizable credential content is not available to Work",
            Self::Ambiguous => "The passage to replace is missing or not unique",
            Self::Changed => "The file changed since it was read",
            Self::Exists => "The destination already exists",
            Self::Pattern => "The search pattern could not be used",
            Self::Io => "The file could not be accessed",
        }
    }
}

/// Folders admitted for one run, canonical and policy-checked.
#[derive(Clone, Debug, Default)]
pub struct WorkFileGrant {
    /// Canonical roots, and the roots as the person wrote them.
    roots: Vec<PathBuf>,
    written: Vec<PathBuf>,
}
impl WorkFileGrant {
    /// Admits each folder that is an existing directory under the home
    /// folder and outside the denylist; refused ones come back by name.
    pub fn admit(folders: &[String]) -> (Self, Vec<String>) {
        let home = grant_home().and_then(|home| home.canonicalize().ok());
        let permitted = permitted_folders(home.as_deref());
        let protected = protected_folders(home.as_deref());
        let mut roots = Vec::new();
        let mut written = Vec::new();
        let mut refused = Vec::new();
        for folder in folders {
            match admit_root(folder, home.as_deref(), &permitted, &protected) {
                Some(root) => {
                    if !roots.contains(&root) {
                        roots.push(root);
                    }
                    written.push(PathBuf::from(folder));
                }
                None => refused.push(folder.clone()),
            }
        }
        (Self { roots, written }, refused)
    }
    pub fn is_empty(&self) -> bool {
        self.roots.is_empty()
    }
    /// Canonical admitted roots, in grant order.
    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }
    /// The canonical target when the path (or, for a new file, its parent)
    /// lies inside a granted root.
    pub(crate) fn resolve(&self, path: &str, may_create: bool) -> Result<PathBuf, WorkFileError> {
        validate_file_path(path).map_err(|_| WorkFileError::Denied)?;
        let candidate = Path::new(path);
        if protected_path(candidate) {
            return Err(WorkFileError::Denied);
        }
        if !candidate.is_absolute()
            || candidate
                .components()
                .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
        {
            return Err(WorkFileError::Denied);
        }
        // Refuse by the written path first, so nothing outside a root is
        // even probed; the canonical path is checked again for symlinks.
        if !self
            .roots
            .iter()
            .chain(&self.written)
            .any(|root| path_contains(root, candidate))
        {
            return Err(WorkFileError::Denied);
        }
        let resolved = match std::fs::canonicalize(candidate) {
            Ok(resolved) => resolved,
            Err(_) if may_create => {
                let parent = candidate.parent().ok_or(WorkFileError::Denied)?;
                let name = candidate.file_name().ok_or(WorkFileError::Denied)?;
                std::fs::canonicalize(parent)
                    .map_err(|_| WorkFileError::NotFound)?
                    .join(name)
            }
            Err(_) => return Err(WorkFileError::NotFound),
        };
        if !protected_path(&resolved)
            && self.roots.iter().any(|root| path_contains(root, &resolved))
        {
            Ok(resolved)
        } else {
            Err(WorkFileError::Denied)
        }
    }
    pub fn list(&self, path: &str) -> Result<WorkFileEvidenceV1, WorkFileError> {
        self.list_at(path, 1)
    }
    pub fn list_at(&self, path: &str, depth: u8) -> Result<WorkFileEvidenceV1, WorkFileError> {
        if !(1..=3).contains(&depth) {
            return Err(WorkFileError::Denied);
        }
        let root = self.resolve(path, false)?;
        if !root.is_dir() {
            return Err(WorkFileError::NotADirectory);
        }
        let mut pending = vec![(root.clone(), 0u8)];
        let mut lines = Vec::new();
        let mut cut = false;
        while let Some((dir, level)) = pending.pop() {
            let _directories = pin_directories(&dir)?;
            let entries = read_directory(&dir)?;
            cut |= entries.truncated;
            let mut entries: Vec<_> = entries
                .take(MAX_LIST_ENTRIES + 1)
                .filter_map(Result::ok)
                .collect();
            if entries.len() > MAX_LIST_ENTRIES {
                cut = true;
            }
            entries.sort_by_key(|e| e.file_name());
            for entry in entries {
                let name = entry.file_name().to_string_lossy().into_owned();
                if protected_path(&entry.path()) || SKIPPED_DIRS.contains(&name.as_str()) {
                    continue;
                }
                let Ok(meta) = entry.file_type() else {
                    continue;
                };
                if entry_is_link(&entry, &meta) {
                    continue;
                }
                if lines.len() == MAX_LIST_ENTRIES {
                    cut = true;
                    break;
                }
                let relative = entry
                    .path()
                    .strip_prefix(&root)
                    .unwrap_or(entry.path().as_path())
                    .to_string_lossy()
                    .into_owned();
                #[cfg(windows)]
                let relative = relative.replace('\\', "/");
                lines.push(if meta.is_dir() {
                    format!("{relative}/")
                } else {
                    format!(
                        "{relative}\t{}",
                        entry.metadata().map(|m| m.len()).unwrap_or(0)
                    )
                });
                if meta.is_dir() && level + 1 < depth {
                    pending.push((entry.path(), level + 1));
                }
            }
            if lines.len() == MAX_LIST_ENTRIES {
                cut |= !pending.is_empty();
                break;
            }
        }
        lines.sort();
        let mut result = evidence(
            &root,
            WorkFileKindV1::Directory,
            lines.len() as u32,
            String::new(),
            lines.join("\n") + "\n",
        );
        result.truncated |= cut;
        Ok(result)
    }
    pub fn read(&self, path: &str) -> Result<WorkFileEvidenceV1, WorkFileError> {
        self.read_at(path, 1, 200)
    }
    pub fn read_at(
        &self,
        path: &str,
        offset: u32,
        limit: u32,
    ) -> Result<WorkFileEvidenceV1, WorkFileError> {
        if offset == 0 || !(1..=2000).contains(&limit) {
            return Err(WorkFileError::Denied);
        }
        let file = self.resolve(path, false)?;
        let (bytes, digest, text) = read_text(&file)?;
        let Some(text) = text else {
            return Ok(evidence(
                &file,
                WorkFileKindV1::Binary,
                bytes,
                digest,
                String::new(),
            ));
        };
        let total = text.lines().count() as u32;
        let mut excerpt = String::new();
        let mut last = offset.saturating_sub(1).min(total);
        let mut cut = false;
        for (index, line) in text
            .lines()
            .enumerate()
            .skip((offset - 1) as usize)
            .take(limit as usize)
        {
            let numbered = format!("{}: {line}\n", index + 1);
            if excerpt.len() + numbered.len() > MAX_WORK_FILE_TEXT_BYTES {
                let (part, _) = clip_to(&numbered, MAX_WORK_FILE_TEXT_BYTES - excerpt.len());
                excerpt.push_str(&part);
                last = index as u32 + 1;
                cut = true;
                break;
            }
            excerpt.push_str(&numbered);
            last = index as u32 + 1;
        }
        let mut result = evidence(&file, WorkFileKindV1::Text, bytes, digest, excerpt);
        result.truncated |= cut || offset > 1 || last < total;
        result.lines = Some(WorkFileLinesV1 {
            first: if offset <= total { offset } else { 0 },
            last: if offset <= total { last } else { 0 },
            total,
        });
        Ok(result)
    }
    pub fn search(&self, path: &str, query: &str) -> Result<WorkFileEvidenceV1, WorkFileError> {
        self.search_with(path, query, None, false)
    }
    pub fn search_with(
        &self,
        path: &str,
        query: &str,
        glob: Option<&str>,
        regex: bool,
    ) -> Result<WorkFileEvidenceV1, WorkFileError> {
        if query.trim().is_empty() || query.len() > MAX_WORK_FILE_QUERY_BYTES {
            return Err(WorkFileError::Pattern);
        }
        let root = self.resolve(path, false)?;
        if !root.is_dir() {
            return Err(WorkFileError::NotADirectory);
        }
        let pattern = if regex {
            query.to_owned()
        } else {
            regex::escape(query)
        };
        let re = regex::RegexBuilder::new(&pattern)
            .case_insensitive(true)
            .size_limit(1024 * 1024)
            .dfa_size_limit(1024 * 1024)
            .build()
            .map_err(|_| WorkFileError::Pattern)?;
        let filter = glob
            .map(|g| {
                if g.len() > MAX_WORK_FILE_QUERY_BYTES {
                    return Err(WorkFileError::Pattern);
                }
                globset::Glob::new(g)
                    .map(|g| g.compile_matcher())
                    .map_err(|_| WorkFileError::Pattern)
            })
            .transpose()?;
        let started = Instant::now();
        let mut pending = vec![root.clone()];
        let mut visited = 0;
        let mut hits = 0;
        let mut text = String::new();
        let mut cut = false;
        'walk: while let Some(dir) = pending.pop() {
            let Ok(_directories) = pin_directories(&dir) else {
                cut = true;
                continue;
            };
            let Ok(entries) = read_directory(&dir) else {
                cut = true;
                continue;
            };
            cut |= entries.truncated;
            for entry in entries.filter_map(Result::ok) {
                if visited >= MAX_SEARCH_FILES || started.elapsed() >= SEARCH_BUDGET {
                    cut = true;
                    break 'walk;
                }
                visited += 1;
                if protected_path(&entry.path()) {
                    continue;
                }
                let Ok(meta) = entry.file_type() else {
                    continue;
                };
                if entry_is_link(&entry, &meta) {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().into_owned();
                if meta.is_dir() {
                    if !name.starts_with('.') && !SKIPPED_DIRS.contains(&name.as_str()) {
                        pending.push(entry.path());
                    }
                    continue;
                }
                if !meta.is_file() {
                    continue;
                }
                let relative = entry.path().strip_prefix(&root).unwrap().to_path_buf();
                #[cfg(windows)]
                let relative = PathBuf::from(relative.to_string_lossy().replace('\\', "/"));
                if filter.as_ref().is_some_and(|f| !f.is_match(&relative)) {
                    continue;
                }
                let Ok((_, _, Some(content))) = read_text(&entry.path()) else {
                    continue;
                };
                let lines: Vec<_> = content.lines().collect();
                for (index, line) in lines.iter().enumerate() {
                    if started.elapsed() >= SEARCH_BUDGET {
                        cut = true;
                        break 'walk;
                    }
                    if !re.is_match(line) {
                        continue;
                    }
                    hits += 1;
                    for (n, context) in lines
                        .iter()
                        .enumerate()
                        .take(index + 2)
                        .skip(index.saturating_sub(1))
                    {
                        text.push_str(&format!(
                            "{}:{}{} {}\n",
                            relative.to_string_lossy(),
                            n + 1,
                            if n == index { ":" } else { "-" },
                            context.chars().take(200).collect::<String>()
                        ));
                    }
                    if hits >= MAX_SEARCH_HITS || text.len() >= MAX_WORK_FILE_TEXT_BYTES {
                        cut = true;
                        break 'walk;
                    }
                }
            }
        }
        let mut result = evidence(
            &root,
            WorkFileKindV1::Search,
            hits as u32,
            String::new(),
            text,
        );
        result.truncated |= cut;
        Ok(result)
    }
    pub fn prepare(
        &self,
        kind: &WorkStepKindV1,
        known: Option<&str>,
    ) -> Result<PreparedChange, WorkFileError> {
        if matches!(
            kind,
            WorkStepKindV1::DeleteFile { .. } | WorkStepKindV1::MoveFile { .. }
        ) {
            ensure_removals_available()?;
        }
        let (path, create) = match kind {
            WorkStepKindV1::WriteFile { path, .. } => (path, true),
            WorkStepKindV1::EditFile { path, .. } | WorkStepKindV1::DeleteFile { path, .. } => {
                (path, false)
            }
            WorkStepKindV1::MoveFile { from, .. } => (from, false),
            _ => return Err(WorkFileError::Denied),
        };
        let file = self.resolve(path, create)?;
        let current = match read_text(&file) {
            Ok(value) => Some(value),
            Err(WorkFileError::NotFound) if create => None,
            Err(error) => return Err(error),
        };
        let before = current.as_ref().map(|(_, digest, _)| digest.clone());
        if known.is_some_and(|digest| Some(digest) != before.as_deref()) {
            return Err(WorkFileError::Changed);
        }
        let mut destination = None;
        let mut next = None;
        let proposal = match kind {
            WorkStepKindV1::WriteFile { content, .. } => {
                if zephium_agentic::contains_recognized_secret(content) {
                    return Err(WorkFileError::Sensitive);
                }
                if binary_looking(content) {
                    return Err(WorkFileError::Binary);
                }
                let old = match &current {
                    Some((_, _, Some(text))) => text.as_str(),
                    None => "",
                    _ => return Err(WorkFileError::Binary),
                };
                next = Some(content.clone());
                diff(old, content)
            }
            WorkStepKindV1::EditFile {
                old,
                new,
                replacements,
                ..
            } => {
                validate_replacements(old, new, replacements)
                    .map_err(|_| WorkFileError::Ambiguous)?;
                let text = current
                    .as_ref()
                    .and_then(|(_, _, text)| text.as_ref())
                    .ok_or(WorkFileError::Binary)?;
                let legacy = [WorkFileReplacementV1 {
                    old: old.clone(),
                    new: new.clone(),
                }];
                let replacements = if replacements.is_empty() {
                    &legacy[..]
                } else {
                    replacements
                };
                let mut spans = Vec::new();
                for r in replacements {
                    let start = text.find(&r.old).ok_or(WorkFileError::Ambiguous)?;
                    let following = start + text[start..].chars().next().unwrap().len_utf8();
                    if text[following..].contains(&r.old) {
                        return Err(WorkFileError::Ambiguous);
                    }
                    spans.push((start, start + r.old.len(), r.new.as_str()));
                }
                spans.sort_by_key(|s| s.0);
                if spans.windows(2).any(|w| w[0].1 > w[1].0) {
                    return Err(WorkFileError::Ambiguous);
                }
                let mut edited = text.clone();
                for (start, end, replacement) in spans.iter().rev() {
                    edited.replace_range(*start..*end, replacement);
                }
                if edited.len() as u64 > MAX_READ_BYTES {
                    return Err(WorkFileError::TooLarge);
                }
                if binary_looking(&edited) {
                    return Err(WorkFileError::Binary);
                }
                if zephium_agentic::contains_recognized_secret(&edited) {
                    return Err(WorkFileError::Sensitive);
                }
                let proposal = replacement_diff(text, &edited, &spans);
                next = Some(edited);
                proposal
            }
            WorkStepKindV1::MoveFile { to, .. } => {
                let target = self.resolve(to, true)?;
                if target.try_exists().map_err(|_| WorkFileError::Io)? {
                    return Err(WorkFileError::Exists);
                }
                destination = Some(target);
                format!("Move {path} → {to}")
            }
            WorkStepKindV1::DeleteFile { .. } => format!("Delete {path}"),
            _ => return Err(WorkFileError::Denied),
        };
        let (proposal, truncated) = clip(proposal);
        Ok(PreparedChange {
            original: path.clone(),
            file,
            destination,
            before,
            next,
            proposal,
            truncated,
        })
    }
    pub fn apply(&self, change: PreparedChange) -> Result<WorkFileEvidenceV1, WorkFileError> {
        if change.next.is_none() {
            ensure_removals_available()?;
        }
        let file = self.resolve(&change.original, change.before.is_none())?;
        if file != change.file {
            return Err(WorkFileError::Changed);
        }
        let _directories = pin_mutation_directory(file.parent().ok_or(WorkFileError::Denied)?)?;
        let current = match read_text(&file) {
            Ok((_, digest, _)) => Some(digest),
            Err(WorkFileError::NotFound) => None,
            Err(e) => return Err(e),
        };
        if current != change.before {
            return Err(WorkFileError::Changed);
        }
        let (path, kind, after, bytes) = if let Some(next) = change.next {
            write_atomic(&file, next.as_bytes(), change.before.as_deref())?;
            // Publication attests the bytes from our owned output handle. A
            // later pathname read could observe a different, concurrently edited file.
            let digest = format!("{:x}", Sha256::digest(next.as_bytes()));
            (
                file,
                WorkFileKindV1::Written,
                Some(digest),
                next.len() as u32,
            )
        } else if let Some(to) = change.destination {
            let _destination_directories =
                pin_mutation_directory(to.parent().ok_or(WorkFileError::Denied)?)?;
            if self.resolve(&to.to_string_lossy(), true)? != to {
                return Err(WorkFileError::Changed);
            }
            move_exclusive(&file, &to)?;
            let (bytes, digest, _) = read_text(&to)?;
            (to, WorkFileKindV1::Moved, Some(digest), bytes)
        } else {
            std::fs::remove_file(&file).map_err(|_| WorkFileError::Io)?;
            (file, WorkFileKindV1::Deleted, None, 0)
        };
        let mut record = evidence(
            &path,
            kind,
            bytes,
            after
                .clone()
                .or_else(|| change.before.clone())
                .unwrap_or_default(),
            change.proposal,
        );
        record.truncated |= change.truncated;
        record.before_digest = change.before;
        record.after_digest = after;
        Ok(record)
    }
    pub fn propose_write(&self, path: &str, content: &str) -> Result<String, WorkFileError> {
        Ok(self
            .prepare(
                &WorkStepKindV1::WriteFile {
                    path: path.into(),
                    content: content.into(),
                    decision: None,
                },
                None,
            )?
            .proposal)
    }
    pub fn apply_write(
        &self,
        path: &str,
        content: &str,
    ) -> Result<WorkFileEvidenceV1, WorkFileError> {
        self.apply(self.prepare(
            &WorkStepKindV1::WriteFile {
                path: path.into(),
                content: content.into(),
                decision: None,
            },
            None,
        )?)
    }
    pub fn propose_edit(&self, path: &str, old: &str, new: &str) -> Result<String, WorkFileError> {
        Ok(self
            .prepare(
                &WorkStepKindV1::EditFile {
                    path: path.into(),
                    old: old.into(),
                    new: new.into(),
                    replacements: vec![],
                    decision: None,
                },
                None,
            )?
            .proposal)
    }
    pub fn apply_edit(
        &self,
        path: &str,
        old: &str,
        new: &str,
    ) -> Result<WorkFileEvidenceV1, WorkFileError> {
        self.apply(self.prepare(
            &WorkStepKindV1::EditFile {
                path: path.into(),
                old: old.into(),
                new: new.into(),
                replacements: vec![],
                decision: None,
            },
            None,
        )?)
    }
}

/// A capability gate, independent of approval: a file written or edited in a
/// granted folder is the person's to approve, diff by diff; deleting or moving
/// one waits until it goes to the Trash instead of away for good.
pub(crate) fn ensure_removals_available() -> Result<(), WorkFileError> {
    Err(WorkFileError::Unavailable)
}

/// Deliberately conservative name policy, applied before probing and again to
/// resolved paths. This does not claim to detect credentials in arbitrary text.
pub(crate) fn protected_path(path: &Path) -> bool {
    path.components().any(|part| {
        let Component::Normal(name) = part else {
            return false;
        };
        let Some(name) = name.to_str() else {
            return true;
        };
        if zephium_agentic::contains_recognized_secret(name.trim_start_matches('.')) {
            return true;
        }
        // Case-insensitive filesystems also equate characters whose uppercase
        // expands to ASCII (for example long-s ſ with s). Lowercase alone misses
        // these aliases, and macOS realpath preserves the caller's spelling.
        let folded = name.to_uppercase();
        if zephium_agentic::contains_recognized_secret(folded.trim_start_matches('.')) {
            return true;
        }
        let name = folded.to_lowercase();
        name.starts_with(".env")
            || matches!(name.as_str(),
                ".zephium-work-recovery" | ".ssh" | ".gnupg" | ".aws" | ".azure" | ".kube" | ".config"
                | ".git" | ".gitconfig" | ".git-credentials" | ".npmrc" | ".netrc" | "_netrc" | ".pypirc"
                | ".docker" | ".codex" | ".claude" | ".gem" | ".bundle"
                | "credentials" | "credentials.json" | "credentials.toml"
                | "secrets" | "secrets.json" | "secrets.yaml" | "secrets.yml"
                | "auth.json" | "tokens.json" | "keychains" | "appdata"
                | "id_rsa" | "id_dsa" | "id_ecdsa" | "id_ed25519"
                | "cookies" | "login data" | "web data" | "key4.db" | "logins.json")
            || name.starts_with("app.zephium")
            || [".pem", ".key", ".p12", ".pfx", ".kdbx", ".keychain", ".keychain-db"]
                .iter().any(|extension| name.ends_with(extension))
            // Win32 strips trailing dots/spaces and recognizes alternate streams.
            // Refuse these spellings on every host so copied plans remain safe.
            || name.ends_with(['.', ' ']) || name.contains(':')
    })
}

#[cfg(unix)]
pub(crate) fn open_unix(path: &Path, directory: bool) -> Result<std::fs::File, WorkFileError> {
    use std::os::{
        fd::{AsRawFd, FromRawFd},
        unix::ffi::OsStrExt,
    };
    if !path.is_absolute() {
        return Err(WorkFileError::Denied);
    }
    let mut parent = std::fs::File::open("/").map_err(|_| WorkFileError::Io)?;
    let parts: Vec<_> = path
        .components()
        .filter(|c| *c != Component::RootDir)
        .collect();
    if parts.len() > 128 {
        return Err(WorkFileError::Denied);
    }
    for (index, part) in parts.iter().enumerate() {
        let Component::Normal(name) = part else {
            return Err(WorkFileError::Denied);
        };
        let name = std::ffi::CString::new(name.as_bytes()).map_err(|_| WorkFileError::Denied)?;
        let is_directory = index + 1 < parts.len() || directory;
        let flags = libc::O_RDONLY
            | libc::O_CLOEXEC
            | libc::O_NOFOLLOW
            | libc::O_NONBLOCK
            | if is_directory { libc::O_DIRECTORY } else { 0 };
        // Each child is opened relative to its held parent. No symlink in an
        // intermediate or final component can redirect the read after checking.
        let fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
        if fd < 0 {
            return Err(match std::io::Error::last_os_error().kind() {
                std::io::ErrorKind::NotFound => WorkFileError::NotFound,
                _ => WorkFileError::Denied,
            });
        }
        parent = unsafe { std::fs::File::from_raw_fd(fd) };
    }
    Ok(parent)
}

pub(crate) fn single_link(file: &std::fs::File) -> Result<bool, WorkFileError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(file.metadata().map_err(|_| WorkFileError::Io)?.nlink() == 1)
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows::Win32::{
            Foundation::HANDLE,
            Storage::FileSystem::{GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION},
        };
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info) }
            .map_err(|_| WorkFileError::Denied)?;
        Ok(info.nNumberOfLinks == 1)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = file;
        Err(WorkFileError::Denied)
    }
}

pub(crate) struct DirectoryEntries {
    entries: std::vec::IntoIter<std::io::Result<DirectoryEntry>>,
    pub(crate) truncated: bool,
}
impl Iterator for DirectoryEntries {
    type Item = std::io::Result<DirectoryEntry>;
    fn next(&mut self) -> Option<Self::Item> {
        self.entries.next()
    }
}

pub(crate) struct DirectoryEntry {
    path: PathBuf,
    metadata: std::fs::Metadata,
}
impl DirectoryEntry {
    pub(crate) fn path(&self) -> PathBuf {
        self.path.clone()
    }
    pub(crate) fn file_name(&self) -> std::ffi::OsString {
        self.path.file_name().unwrap().to_owned()
    }
    pub(crate) fn file_type(&self) -> std::io::Result<std::fs::FileType> {
        Ok(self.metadata.file_type())
    }
    pub(crate) fn metadata(&self) -> std::io::Result<std::fs::Metadata> {
        Ok(self.metadata.clone())
    }
}

#[cfg(not(unix))]
pub(crate) fn read_directory(path: &Path) -> Result<DirectoryEntries, WorkFileError> {
    let _directories = pin_directories(path)?;
    let entries = std::fs::read_dir(path).map_err(|_| WorkFileError::Io)?;
    let mut result = Vec::new();
    let mut visited = 0;
    for entry in entries.take(MAX_SEARCH_FILES + 1).filter_map(Result::ok) {
        visited += 1;
        let path = entry.path();
        if protected_path(&path) {
            continue;
        }
        let metadata = std::fs::symlink_metadata(&path).map_err(|_| WorkFileError::Io)?;
        let mut entry = DirectoryEntry { path, metadata };
        let kind = entry.metadata.file_type();
        if entry_is_link(&entry, &kind) {
            continue;
        }
        if kind.is_file() {
            let Ok(checked) = open_checked_file(&entry.path, false) else {
                continue;
            };
            entry.metadata = checked.metadata;
        }
        if kind.is_file() || kind.is_dir() {
            result.push(Ok(entry));
        }
    }
    Ok(DirectoryEntries {
        entries: result.into_iter(),
        truncated: visited > MAX_SEARCH_FILES,
    })
}

#[cfg(unix)]
pub(crate) fn read_directory(path: &Path) -> Result<DirectoryEntries, WorkFileError> {
    use std::os::{
        fd::{AsRawFd, FromRawFd, IntoRawFd},
        unix::ffi::OsStrExt,
    };
    let directory = open_unix(path, true)?;
    let fd = directory.as_raw_fd();
    // fdopendir owns its descriptor; retain `directory` for relative child opens.
    let copy = directory
        .try_clone()
        .map_err(|_| WorkFileError::Io)?
        .into_raw_fd();
    let stream = unsafe { libc::fdopendir(copy) };
    if stream.is_null() {
        unsafe {
            libc::close(copy);
        }
        return Err(WorkFileError::Io);
    }
    struct Stream(*mut libc::DIR);
    impl Drop for Stream {
        fn drop(&mut self) {
            unsafe {
                libc::closedir(self.0);
            }
        }
    }
    let stream = Stream(stream);
    let mut entries = Vec::new();
    let mut visited = 0;
    while visited <= MAX_SEARCH_FILES {
        visited += 1;
        let entry = unsafe { libc::readdir(stream.0) };
        if entry.is_null() {
            break;
        }
        let name = unsafe { std::ffi::CStr::from_ptr((*entry).d_name.as_ptr()) };
        if matches!(name.to_bytes(), b"." | b"..") {
            continue;
        }
        let child_path = path.join(std::ffi::OsStr::from_bytes(name.to_bytes()));
        if protected_path(&child_path) {
            continue;
        }
        let child = unsafe {
            libc::openat(
                fd,
                name.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
            )
        };
        if child < 0 {
            continue;
        }
        let child = unsafe { std::fs::File::from_raw_fd(child) };
        let Ok(metadata) = child.metadata() else {
            continue;
        };
        if !metadata.is_dir() && (!metadata.is_file() || !single_link(&child)?) {
            continue;
        }
        entries.push(Ok(DirectoryEntry {
            path: child_path,
            metadata,
        }));
    }
    Ok(DirectoryEntries {
        entries: entries.into_iter(),
        truncated: visited > MAX_SEARCH_FILES,
    })
}

#[cfg(test)]
thread_local! {
    static TEST_GRANT_HOME: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// Current-thread fixture home; never mutates HOME or changes production policy.
#[cfg(test)]
pub(crate) struct TestGrantHome {
    previous: Option<PathBuf>,
    _thread: std::marker::PhantomData<std::rc::Rc<()>>,
}
#[cfg(test)]
impl TestGrantHome {
    pub(crate) fn new(home: PathBuf) -> Self {
        let previous = TEST_GRANT_HOME.with(|slot| slot.replace(Some(home)));
        Self {
            previous,
            _thread: std::marker::PhantomData,
        }
    }
}
#[cfg(test)]
impl Drop for TestGrantHome {
    fn drop(&mut self) {
        TEST_GRANT_HOME.with(|slot| {
            slot.replace(self.previous.take());
        });
    }
}

#[cfg(not(windows))]
fn grant_home() -> Option<PathBuf> {
    #[cfg(test)]
    if let Some(home) = TEST_GRANT_HOME.with(|slot| slot.borrow().clone()) {
        return Some(home);
    }
    std::env::var_os("HOME").map(PathBuf::from)
}

#[cfg(windows)]
fn grant_home() -> Option<PathBuf> {
    #[cfg(test)]
    if let Some(home) = TEST_GRANT_HOME.with(|slot| slot.borrow().clone()) {
        return Some(home);
    }
    #[cfg(test)]
    if let Some(home) = std::env::var_os("HOME") {
        return Some(home.into());
    }
    known_folder(&windows::Win32::UI::Shell::FOLDERID_Profile)
}

fn permitted_folders(home: Option<&Path>) -> Vec<PathBuf> {
    let folders: Vec<_> = home.map(Path::to_path_buf).into_iter().collect();
    #[cfg(windows)]
    let mut folders = folders;
    #[cfg(windows)]
    {
        use windows::Win32::UI::Shell::{FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads};
        #[cfg(test)]
        if std::env::var_os("HOME").is_some() {
            return folders;
        }
        for id in [FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads] {
            if let Some(folder) = known_folder(&id).and_then(|folder| folder.canonicalize().ok()) {
                folders.push(folder);
            }
        }
    }
    folders
}

fn protected_folders(home: Option<&Path>) -> Vec<PathBuf> {
    let folders: Vec<_> = home
        .into_iter()
        .flat_map(|home| {
            DENIED_UNDER_HOME
                .iter()
                .map(move |denied| home.join(denied))
        })
        .collect();
    #[cfg(windows)]
    let mut folders = folders;
    #[cfg(windows)]
    {
        use windows::Win32::UI::Shell::{
            FOLDERID_LocalAppData, FOLDERID_LocalAppDataLow, FOLDERID_RoamingAppData,
        };
        if let Some(home) = home {
            folders.push(home.join("AppData"));
            folders.push(home.join(".zephium-native-v1"));
        }
        let native = !cfg!(test) || std::env::var_os("HOME").is_none();
        if native {
            for id in [
                FOLDERID_LocalAppData,
                FOLDERID_LocalAppDataLow,
                FOLDERID_RoamingAppData,
            ] {
                if let Some(folder) = known_folder(&id) {
                    folders.push(folder);
                }
            }
        }
        let canonical: Vec<_> = folders
            .iter()
            .filter_map(|folder| folder.canonicalize().ok())
            .collect();
        folders.extend(canonical);
    }
    folders
}

#[cfg(windows)]
fn known_folder(id: &windows::core::GUID) -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{SHGetKnownFolderPath, KF_FLAG_DEFAULT};
    let path = unsafe { SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None) }.ok()?;
    let value = unsafe { path.as_wide() };
    let folder = PathBuf::from(std::ffi::OsString::from_wide(value));
    unsafe { CoTaskMemFree(Some(path.0.cast())) };
    Some(folder)
}

fn admit_root(
    folder: &str,
    home: Option<&Path>,
    permitted: &[PathBuf],
    protected: &[PathBuf],
) -> Option<PathBuf> {
    validate_file_path(folder).ok()?;
    if !Path::new(folder).is_absolute() || protected_path(Path::new(folder)) {
        return None;
    }
    let root = std::fs::canonicalize(folder).ok()?;
    if !root.is_dir() || protected_path(&root) {
        return None;
    }
    let home = home?;
    if same_path(&root, home)
        || !permitted.iter().any(|folder| path_contains(folder, &root))
        // A parent such as ~/Library would expose the protected folder
        // beneath it as surely as granting that folder itself.
        || protected
            .iter()
            .any(|folder| path_contains(folder, &root) || path_contains(&root, folder))
    {
        return None;
    }
    let _directories = pin_directories(&root).ok()?;
    Some(root)
}

#[cfg(not(windows))]
fn path_contains(root: &Path, path: &Path) -> bool {
    path.starts_with(root)
}

#[cfg(windows)]
fn path_contains(root: &Path, path: &Path) -> bool {
    let mut parts = path.components();
    root.components().all(|part| {
        parts
            .next()
            .is_some_and(|candidate| windows_path_part_eq(part.as_os_str(), candidate.as_os_str()))
    })
}

fn same_path(left: &Path, right: &Path) -> bool {
    path_contains(left, right) && path_contains(right, left)
}

#[cfg(windows)]
fn windows_path_part_eq(left: &std::ffi::OsStr, right: &std::ffi::OsStr) -> bool {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Globalization::{CompareStringOrdinal, CSTR_EQUAL};
    // Canonical paths use a verbatim prefix; written paths ordinarily do not.
    let normalize = |part: &std::ffi::OsStr| {
        part.to_string_lossy()
            .replace('/', "\\")
            .strip_prefix(r"\\?\UNC\")
            .map(|unc| format!(r"\\{unc}"))
            .unwrap_or_else(|| {
                part.to_string_lossy()
                    .replace('/', "\\")
                    .trim_start_matches(r"\\?\")
                    .to_owned()
            })
    };
    let left = normalize(left);
    let right = normalize(right);
    let left: Vec<_> = std::ffi::OsStr::new(&left).encode_wide().collect();
    let right: Vec<_> = std::ffi::OsStr::new(&right).encode_wide().collect();
    unsafe { CompareStringOrdinal(&left, &right, true) == CSTR_EQUAL }
}

fn entry_is_link(entry: &DirectoryEntry, kind: &std::fs::FileType) -> bool {
    if kind.is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        use std::os::windows::fs::OpenOptionsExt;
        use windows::Win32::Storage::FileSystem::{
            FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
            FILE_READ_ATTRIBUTES, FILE_SHARE_READ,
        };
        let Ok(metadata) = std::fs::symlink_metadata(entry.path()) else {
            return true;
        };
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 == 0 {
            return false;
        }
        std::fs::OpenOptions::new()
            .access_mode(FILE_READ_ATTRIBUTES.0)
            .share_mode(FILE_SHARE_READ.0)
            .custom_flags((FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT).0)
            .open(entry.path())
            .map_or(true, |file| !safe_reparse(&file))
    }
    #[cfg(not(windows))]
    {
        let _ = entry;
        false
    }
}

#[cfg(unix)]
fn pin_directories(path: &Path) -> Result<Vec<std::fs::File>, WorkFileError> {
    Ok(vec![open_unix(path, true)?])
}

#[cfg(not(any(unix, windows)))]
fn pin_directories(_path: &Path) -> Result<Vec<std::fs::File>, WorkFileError> {
    Err(WorkFileError::Denied)
}

#[cfg(windows)]
pub(crate) fn pin_directories(path: &Path) -> Result<Vec<std::fs::File>, WorkFileError> {
    pin_directories_with_write(path, false)
}

#[cfg(windows)]
fn pin_directories_with_write(
    path: &Path,
    mutation: bool,
) -> Result<Vec<std::fs::File>, WorkFileError> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_READ,
        FILE_SHARE_READ, FILE_SHARE_WRITE,
    };
    let mut paths: Vec<_> = path.ancestors().filter(|path| path.is_absolute()).collect();
    if paths.len() > 128 {
        return Err(WorkFileError::Denied);
    }
    paths.reverse();
    let mut pinned = Vec::with_capacity(paths.len());
    let leaf = paths.len().saturating_sub(1);
    for (index, path) in paths.into_iter().enumerate() {
        let share = if mutation || index < leaf {
            FILE_SHARE_READ | FILE_SHARE_WRITE
        } else {
            FILE_SHARE_READ
        };
        let file = std::fs::OpenOptions::new()
            .access_mode(FILE_GENERIC_READ.0)
            .share_mode(share.0)
            .custom_flags((FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT).0)
            .open(path)
            .map_err(|_| WorkFileError::Denied)?;
        let metadata = file.metadata().map_err(|_| WorkFileError::Denied)?;
        if !metadata.is_dir() || !safe_reparse(&file) {
            return Err(WorkFileError::Denied);
        }
        pinned.push(file);
    }
    Ok(pinned)
}

fn pin_mutation_directory(path: &Path) -> Result<Vec<std::fs::File>, WorkFileError> {
    #[cfg(not(windows))]
    let pinned = pin_directories(path)?;
    #[cfg(windows)]
    {
        let mut pinned = pin_directories_with_write(path, true)?;
        let parent = pinned.last().ok_or(WorkFileError::Denied)?;
        pinned.push(mutation_marker(parent, path)?);
        Ok(pinned)
    }
    #[cfg(not(windows))]
    Ok(pinned)
}

#[cfg(windows)]
fn mutation_marker(parent: &std::fs::File, path: &Path) -> Result<std::fs::File, WorkFileError> {
    // A held child keeps the parent nonempty, so its reparse tag cannot change.
    // Create relative to the held parent: an in-place junction raced before the
    // marker must never cause even a temporary write through its target path.
    if !safe_reparse(parent) || !opened_at(parent, path) {
        return Err(WorkFileError::Denied);
    }
    let name = format!(".zephium-pin-{}", WorkArtifactId::generate());
    let marker_file = windows_marker::create(parent, &name)?;
    if !safe_reparse(parent) || !opened_at(&marker_file, &path.join(name)) {
        return Err(WorkFileError::Denied);
    }
    Ok(marker_file)
}

#[cfg(windows)]
fn safe_reparse(file: &std::fs::File) -> bool {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{
        FileAttributeTagInfo, GetFileInformationByHandleEx, FILE_ATTRIBUTE_REPARSE_POINT,
        FILE_ATTRIBUTE_TAG_INFO,
    };
    let mut info = FILE_ATTRIBUTE_TAG_INFO::default();
    if unsafe {
        GetFileInformationByHandleEx(
            HANDLE(file.as_raw_handle()),
            FileAttributeTagInfo,
            (&mut info as *mut FILE_ATTRIBUTE_TAG_INFO).cast(),
            std::mem::size_of_val(&info) as u32,
        )
    }
    .is_err()
    {
        return false;
    }
    info.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT.0 == 0 || cloud_reparse_tag(info.ReparseTag)
}

#[cfg(windows)]
fn opened_at(file: &std::fs::File, expected: &Path) -> bool {
    use std::os::windows::{ffi::OsStringExt, io::AsRawHandle};
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{GetFinalPathNameByHandleW, FILE_NAME_NORMALIZED};
    let mut name = vec![0; 32768];
    let length = unsafe {
        GetFinalPathNameByHandleW(
            HANDLE(file.as_raw_handle()),
            &mut name,
            FILE_NAME_NORMALIZED,
        )
    } as usize;
    length != 0
        && length < name.len()
        && std::ffi::OsString::from_wide(&name[..length]) == expected.as_os_str()
}

#[cfg(windows)]
fn cloud_reparse_tag(tag: u32) -> bool {
    // Cloud placeholders retain their name; junctions and other tags are refused.
    tag & !0x0000_f000 == 0x9000_001a
}
fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "/".into())
}
/// A checked handle and the guards which keep its namespace stable while used.
/// Metadata inspection never needs to open/read the payload on Windows.
struct CheckedFile {
    input: std::fs::File,
    metadata: std::fs::Metadata,
    _guards: Vec<std::fs::File>,
}

fn open_checked_file(file: &Path, payload: bool) -> Result<CheckedFile, WorkFileError> {
    if protected_path(file) {
        return Err(WorkFileError::Denied);
    }
    let guards = pin_directories(file.parent().ok_or(WorkFileError::Denied)?)?;
    #[cfg(windows)]
    let mut guards = guards;
    let open_error = |error: std::io::Error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            WorkFileError::NotFound
        } else {
            WorkFileError::Io
        }
    };
    #[cfg(windows)]
    let input = {
        use std::os::windows::fs::OpenOptionsExt;
        use windows::Win32::Storage::FileSystem::{
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES, FILE_SHARE_READ,
        };
        let original = std::fs::OpenOptions::new()
            .access_mode(FILE_READ_ATTRIBUTES.0)
            .share_mode(FILE_SHARE_READ.0)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
            .open(file)
            .map_err(open_error)?;
        if !safe_reparse(&original) || !opened_at(&original, file) {
            return Err(WorkFileError::Denied);
        }
        if payload {
            let input = std::fs::OpenOptions::new()
                .read(true)
                .share_mode(FILE_SHARE_READ.0)
                .open(file)
                .map_err(open_error)?;
            guards.push(original);
            input
        } else {
            original
        }
    };
    #[cfg(unix)]
    let input = {
        let _ = (payload, open_error);
        open_unix(file, false)?
    };
    #[cfg(not(any(unix, windows)))]
    let input = {
        let _ = payload;
        std::fs::File::open(file).map_err(open_error)?
    };
    #[cfg(windows)]
    if !opened_at(&input, file) {
        return Err(WorkFileError::Denied);
    }
    let metadata = input.metadata().map_err(|_| WorkFileError::Io)?;
    if !metadata.is_file() {
        return Err(WorkFileError::NotAFile);
    }
    if !single_link(&input)? {
        return Err(WorkFileError::Denied);
    }
    Ok(CheckedFile {
        input,
        metadata,
        _guards: guards,
    })
}

/// Bytes, digest and, for UTF-8 text without NUL, the content.
pub(crate) fn read_text(file: &Path) -> Result<(u32, String, Option<String>), WorkFileError> {
    read_text_limited(file, MAX_READ_BYTES)
}

pub(crate) fn file_available(file: &Path) -> bool {
    open_checked_file(file, false).is_ok()
}

pub(crate) fn directory_available(path: &Path) -> bool {
    !protected_path(path) && pin_directories(path).is_ok()
}

/// Bounded payload for a private runner snapshot, under the shared path policy.
pub(crate) fn snapshot_bytes(file: &Path, limit: u64) -> Result<Vec<u8>, WorkFileError> {
    if limit == 0 || limit > MAX_READ_BYTES {
        return Err(WorkFileError::Denied);
    }
    let mut checked = open_checked_file(file, true)?;
    if checked.metadata.len() > limit {
        return Err(WorkFileError::TooLarge);
    }
    use std::io::Read;
    let mut bytes = Vec::new();
    checked
        .input
        .by_ref()
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| WorkFileError::Io)?;
    if bytes.len() as u64 > limit {
        return Err(WorkFileError::TooLarge);
    }
    Ok(bytes)
}

pub(crate) fn read_text_limited(
    file: &Path,
    limit: u64,
) -> Result<(u32, String, Option<String>), WorkFileError> {
    let bytes = snapshot_bytes(file, limit)?;
    let byte_count = bytes.len() as u32;
    let digest = format!("{:x}", Sha256::digest(&bytes));
    let text = match String::from_utf8(bytes) {
        Ok(text) if zephium_agentic::contains_recognized_secret(&text) => {
            return Err(WorkFileError::Sensitive);
        }
        Ok(text) if !text.contains('\0') => Some(text),
        _ => None,
    };
    Ok((byte_count, digest, text))
}
#[cfg(not(unix))]
fn write_atomic(file: &Path, bytes: &[u8], expected: Option<&str>) -> Result<(), WorkFileError> {
    use std::io::Write;
    let parent = file.parent().ok_or(WorkFileError::Denied)?;
    let _directories = pin_mutation_directory(parent)?;
    let temp = parent.join(format!(".zephium-{}", WorkArtifactId::generate()));
    let result = (|| {
        let mut output = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|_| WorkFileError::Io)?;
        #[cfg(windows)]
        if !opened_at(&output, &temp) {
            return Err(WorkFileError::Denied);
        }
        if let Ok(meta) = std::fs::metadata(file) {
            output
                .set_permissions(meta.permissions())
                .map_err(|_| WorkFileError::Io)?;
        }
        output.write_all(bytes).map_err(|_| WorkFileError::Io)?;
        output.sync_all().map_err(|_| WorkFileError::Io)?;
        let current = match read_text(file) {
            Ok((_, digest, _)) => Some(digest),
            Err(WorkFileError::NotFound) => None,
            Err(error) => return Err(error),
        };
        if current.as_deref() != expected {
            return Err(WorkFileError::Changed);
        }
        if expected.is_none() {
            move_exclusive(&temp, file).map_err(|error| {
                if error == WorkFileError::Exists {
                    WorkFileError::Changed
                } else {
                    error
                }
            })
        } else {
            #[cfg(windows)]
            {
                move_windows(&temp, file, true)
            }
            #[cfg(not(windows))]
            std::fs::rename(&temp, file).map_err(|_| WorkFileError::Io)
        }
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temp);
    }
    result
}
fn move_exclusive(from: &Path, to: &Path) -> Result<(), WorkFileError> {
    #[cfg(windows)]
    {
        move_windows(from, to, false)
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStrExt;
        let from = std::ffi::CString::new(from.as_os_str().as_bytes())
            .map_err(|_| WorkFileError::Denied)?;
        let to =
            std::ffi::CString::new(to.as_os_str().as_bytes()).map_err(|_| WorkFileError::Denied)?;
        // RENAME_EXCL is atomic and refuses replacing a destination created during review.
        if unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) } == 0 {
            Ok(())
        } else if std::io::Error::last_os_error().kind() == std::io::ErrorKind::AlreadyExists {
            Err(WorkFileError::Exists)
        } else {
            Err(WorkFileError::Io)
        }
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        std::fs::hard_link(from, to).map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                WorkFileError::Exists
            } else {
                WorkFileError::Io
            }
        })?;
        std::fs::remove_file(from).map_err(|_| WorkFileError::Io)
    }
}

#[cfg(windows)]
fn move_windows(from: &Path, to: &Path, replace: bool) -> Result<(), WorkFileError> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, ERROR_FILE_EXISTS};
    use windows::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };
    let from: Vec<_> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<_> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut flags = MOVEFILE_WRITE_THROUGH;
    if replace {
        flags |= MOVEFILE_REPLACE_EXISTING;
    }
    unsafe { MoveFileExW(PCWSTR(from.as_ptr()), PCWSTR(to.as_ptr()), flags) }.map_err(|error| {
        if error.code() == ERROR_ALREADY_EXISTS.to_hresult()
            || error.code() == ERROR_FILE_EXISTS.to_hresult()
        {
            WorkFileError::Exists
        } else {
            WorkFileError::Io
        }
    })
}
pub struct PreparedChange {
    original: String,
    file: PathBuf,
    destination: Option<PathBuf>,
    before: Option<String>,
    next: Option<String>,
    pub proposal: String,
    truncated: bool,
}
impl PreparedChange {
    pub fn fact(&self) -> WorkLocalStepV1 {
        WorkLocalStepV1 {
            proposal: Some(self.proposal.clone()),
            before_digest: self.before.clone(),
            ..Default::default()
        }
    }
}
fn binary_looking(text: &str) -> bool {
    text.chars()
        .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
}
fn evidence(
    path: &Path,
    kind: WorkFileKindV1,
    bytes: u32,
    digest: String,
    text: String,
) -> WorkFileEvidenceV1 {
    let (text, truncated) = clip(text);
    WorkFileEvidenceV1 {
        path: path.to_string_lossy().into_owned(),
        name: file_name(path),
        kind,
        bytes,
        digest,
        text,
        truncated,
        before_digest: None,
        after_digest: None,
        lines: None,
    }
}
fn clip_to(text: &str, max: usize) -> (String, bool) {
    let mut end = text.len().min(max);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (text[..end].to_owned(), end < text.len())
}
/// Removed and added lines around each change; enough to judge, never a
/// full copy of both versions.
fn diff(current: &str, next: &str) -> String {
    let a: Vec<&str> = current.lines().collect();
    let b: Vec<&str> = next.lines().collect();
    let common_start = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let common_end = a[common_start..]
        .iter()
        .rev()
        .zip(b[common_start..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let mut out = format!(
        "@@ -{},{} +{},{} @@\n",
        common_start + 1,
        a.len() - common_start - common_end,
        common_start + 1,
        b.len() - common_start - common_end
    );
    for line in &a[common_start..a.len() - common_end] {
        out.push('-');
        out.push_str(line);
        out.push('\n');
    }
    for line in &b[common_start..b.len() - common_end] {
        out.push('+');
        out.push_str(line);
        out.push('\n');
    }
    out
}
/// Separate distant replacements into hunks so unchanged file bodies do not
/// consume the review budget before the later edits are visible.
fn replacement_diff(current: &str, next: &str, spans: &[(usize, usize, &str)]) -> String {
    let a: Vec<_> = current.lines().collect();
    let b: Vec<_> = next.lines().collect();
    let mut ranges: Vec<(usize, usize, usize, usize)> = Vec::new();
    let mut shift = 0isize;
    for (start, end, replacement) in spans {
        let first = current[..*start].bytes().filter(|c| *c == b'\n').count();
        let last = (current[..*end].bytes().filter(|c| *c == b'\n').count() + 1).min(a.len());
        let delta = replacement.bytes().filter(|c| *c == b'\n').count() as isize
            - current[*start..*end]
                .bytes()
                .filter(|c| *c == b'\n')
                .count() as isize;
        let new_first = first.saturating_add_signed(shift).min(b.len());
        let new_last = last.saturating_add_signed(shift + delta).min(b.len());
        let range = (
            first.saturating_sub(3),
            (last + 3).min(a.len()),
            new_first.saturating_sub(3),
            (new_last + 3).min(b.len()),
        );
        if let Some(previous) = ranges.last_mut().filter(|r| range.0 <= r.1) {
            previous.1 = range.1;
            previous.3 = range.3;
        } else {
            ranges.push(range);
        }
        shift += delta;
    }
    let mut out = String::new();
    for (a0, a1, b0, b1) in ranges {
        let old = &a[a0..a1];
        let new = &b[b0..b1];
        let prefix = old.iter().zip(new).take_while(|(x, y)| x == y).count();
        let suffix = old[prefix..]
            .iter()
            .rev()
            .zip(new[prefix..].iter().rev())
            .take_while(|(x, y)| x == y)
            .count();
        out.push_str(&format!(
            "@@ -{},{} +{},{} @@\n",
            a0 + 1,
            a1 - a0,
            b0 + 1,
            b1 - b0
        ));
        for line in &old[..prefix] {
            out.push_str(&format!(" {line}\n"));
        }
        for line in &old[prefix..old.len() - suffix] {
            out.push_str(&format!("-{line}\n"));
        }
        for line in &new[prefix..new.len() - suffix] {
            out.push_str(&format!("+{line}\n"));
        }
        for line in &old[old.len() - suffix..] {
            out.push_str(&format!(" {line}\n"));
        }
    }
    out
}
/// Keeps the text within the disclosed limit on a character boundary and
/// without control characters other than newline and tab.
fn clip(text: String) -> (String, bool) {
    let cleaned: String = text
        .chars()
        .map(|c| {
            if c.is_control() && c != '\n' && c != '\t' {
                ' '
            } else {
                c
            }
        })
        .collect();
    if cleaned.len() <= MAX_WORK_FILE_TEXT_BYTES {
        return (cleaned, false);
    }
    let mut end = MAX_WORK_FILE_TEXT_BYTES;
    while !cleaned.is_char_boundary(end) {
        end -= 1;
    }
    (cleaned[..end].to_owned(), true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home_grant() -> (tempfile::TempDir, TestGrantHome, WorkFileGrant, PathBuf) {
        // Keep the test home thread-local so parallel tests never observe a
        // temporary HOME value or inherit one after this fixture is dropped.
        let home = tempfile::tempdir().unwrap();
        let home_scope = TestGrantHome::new(home.path().to_path_buf());
        let project = home.path().join("Documents").join("project");
        std::fs::create_dir_all(project.join("src")).unwrap();
        std::fs::create_dir_all(home.path().join(".ssh")).unwrap();
        std::fs::write(project.join("README.md"), "# Project\nhello world\n").unwrap();
        std::fs::write(
            project.join("src/main.rs"),
            "fn main() {\n    println!(\"hello\");\n}\n",
        )
        .unwrap();
        std::fs::write(project.join("logo.png"), [0x89, b'P', b'N', b'G', 0, 1]).unwrap();
        let (grant, refused) = WorkFileGrant::admit(&[
            project.to_string_lossy().into_owned(),
            home.path().join(".ssh").to_string_lossy().into_owned(),
            home.path().to_string_lossy().into_owned(),
            "/etc".into(),
            "relative/path".into(),
        ]);
        assert_eq!(refused.len(), 4);
        (home, home_scope, grant, project)
    }

    #[test]
    fn recognized_secret_content_is_refused_before_excerpt_or_edit() {
        let _serial = crate::WORK_RUNTIME_TEST_SERIAL
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let (_home, _home_scope, grant, project) = home_grant();
        let path = project.join("ordinary.txt");
        for secret in [
            "-----BEGIN OPENSSH PRIVATE KEY-----",
            "ghp_inertFixtureToken",
        ] {
            std::fs::write(
                &path,
                format!("ordinary first line\n{}\n{secret}", "filler\n".repeat(500)),
            )
            .unwrap();
            let error = grant.read_at(&path.to_string_lossy(), 1, 1).unwrap_err();
            assert_eq!(error, WorkFileError::Sensitive);
            assert!(!error.note().contains(secret));
        }
        std::fs::write(&path, "ordinary first line\n").unwrap();
        let kind = WorkStepKindV1::WriteFile {
            path: path.to_string_lossy().into_owned(),
            content: "ghp_inertFixtureToken".into(),
            decision: None,
        };
        assert!(matches!(
            grant.prepare(&kind, None),
            Err(WorkFileError::Sensitive)
        ));
        for name in [
            "ghp_inertFixtureToken.txt",
            ".ghp_inertFixtureToken",
            "ſk-inertFixtureToken.txt",
        ] {
            std::fs::write(project.join(name), "ordinary").unwrap();
            assert!(protected_path(&project.join(name)));
        }
        let listed = grant.list(&project.to_string_lossy()).unwrap();
        assert!(!listed.text.contains("ghp_"));
    }

    #[test]
    fn listing_checks_metadata_without_applying_payload_size_limit() {
        let _serial = crate::WORK_RUNTIME_TEST_SERIAL
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let (_home, _home_scope, grant, project) = home_grant();
        let large = project.join("large.txt");
        std::fs::File::create(&large)
            .unwrap()
            .set_len(MAX_READ_BYTES + 1)
            .unwrap();
        let listed = grant.list(&project.to_string_lossy()).unwrap();
        assert!(listed
            .text
            .contains(&format!("large.txt\t{}", MAX_READ_BYTES + 1)));
        assert!(!listed.truncated);
        assert_eq!(
            grant.read(&large.to_string_lossy()).unwrap_err(),
            WorkFileError::TooLarge
        );
        let canonical = large.canonicalize().unwrap();
        assert_eq!(
            open_checked_file(&canonical, false).unwrap().metadata.len(),
            MAX_READ_BYTES + 1
        );
    }

    #[test]
    fn protected_names_links_and_mutation_gate_are_enforced() {
        let _serial = crate::WORK_RUNTIME_TEST_SERIAL
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let (_home, _home_scope, grant, project) = home_grant();
        let marker = "fixture-secret-never-return";
        for name in [
            ".env",
            ".env.local",
            ".ENV.PRODUCTION",
            "credentials.json",
            "private.pem",
        ] {
            std::fs::write(project.join(name), marker).unwrap();
            assert_eq!(
                grant
                    .read(&project.join(name).to_string_lossy())
                    .unwrap_err(),
                WorkFileError::Denied
            );
        }
        let secret = project.join(".env");
        let alias = project.join("innocent.txt");
        std::fs::hard_link(&secret, &alias).unwrap();
        assert_eq!(
            grant.read(&alias.to_string_lossy()).unwrap_err(),
            WorkFileError::Denied
        );
        let listing = grant.list_at(&project.to_string_lossy(), 3).unwrap();
        assert!(
            !listing.text.contains(".env")
                && !listing.text.contains("credentials")
                && !listing.text.contains("innocent")
        );
        assert_eq!(
            grant
                .search(&project.to_string_lossy(), marker)
                .unwrap()
                .bytes,
            0
        );
        let path = project.join("README.md").to_string_lossy().into_owned();
        // Writes and edits are the person's to approve; moving and deleting wait for the Trash.
        for step in [
            WorkStepKindV1::MoveFile {
                from: path.clone(),
                to: project.join("moved").to_string_lossy().into_owned(),
                decision: None,
            },
            WorkStepKindV1::DeleteFile {
                path: path.clone(),
                decision: None,
            },
        ] {
            assert!(matches!(
                grant.prepare(&step, None),
                Err(WorkFileError::Unavailable)
            ));
        }
        // An already prepared/approved operation is gated again at apply.
        let pending = PreparedChange {
            original: path.clone(),
            file: PathBuf::from(&path),
            destination: None,
            before: Some(grant.read(&path).unwrap().digest),
            next: None,
            proposal: "Delete".into(),
            truncated: false,
        };
        assert!(matches!(
            grant.apply(pending),
            Err(WorkFileError::Unavailable)
        ));
        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            "# Project\nhello world\n"
        );
        for name in [
            ".env:stream",
            ".env.",
            ".env ",
            ".SSH",
            "AppData",
            "token.PFX",
            ".ſſh",
            "ſecrets",
            "credentialſ.json",
            "id_rſa",
            ".ßh",
            ".confıg",
        ] {
            assert!(protected_path(Path::new(name)), "{name}");
        }
    }

    #[test]
    fn unicode_case_aliases_never_open_protected_files() {
        let scratch = tempfile::tempdir().unwrap();
        let root = scratch.path().canonicalize().unwrap();
        for (original, alias) in [
            ("secrets", "ſecrets"),
            ("credentials.json", "credentialſ.json"),
            ("id_rsa", "id_rſa"),
        ] {
            std::fs::write(root.join(original), "inert protected fixture").unwrap();
            // APFS resolves each alias to the existing protected file; on a
            // case-sensitive volume the same policy still refuses the spelling.
            assert_eq!(read_text(&root.join(alias)), Err(WorkFileError::Denied));
            assert!(!file_available(&root.join(alias)));
        }
        std::fs::create_dir(root.join(".ssh")).unwrap();
        std::fs::write(root.join(".ssh/note"), "inert protected fixture").unwrap();
        assert!(!directory_available(&root.join(".ſſh")));
        assert_eq!(
            read_text(&root.join(".ſſh/note")),
            Err(WorkFileError::Denied)
        );
    }

    #[cfg(unix)]
    #[test]
    fn unix_reader_refuses_swapped_parent_final_links_and_fifos() {
        use std::os::unix::{ffi::OsStrExt, fs::symlink};
        let scratch = tempfile::tempdir().unwrap();
        let root = scratch.path().canonicalize().unwrap();
        let dir = root.join("granted");
        let outside = root.join("outside");
        std::fs::create_dir(&dir).unwrap();
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("notes.txt"), "fixture-private").unwrap();
        let held = pin_directories(&dir).unwrap();
        std::fs::rename(&dir, root.join("old")).unwrap();
        symlink(&outside, &dir).unwrap();
        assert!(read_text(&dir.join("notes.txt")).is_err());
        assert!(read_directory(&dir).is_err());
        drop(held);
        let final_link = root.join("notes.txt");
        symlink(outside.join("notes.txt"), &final_link).unwrap();
        assert!(read_text(&final_link).is_err());
        let pipe = root.join("pipe");
        let name = std::ffi::CString::new(pipe.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert_eq!(read_text(&pipe).unwrap_err(), WorkFileError::NotAFile);
    }

    #[test]
    fn steps_stay_inside_granted_folders_and_bounds() {
        let _serial = crate::WORK_RUNTIME_TEST_SERIAL
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let (home, _home_scope, grant, project) = home_grant();
        let listing = grant.list(&project.to_string_lossy()).unwrap();
        assert_eq!(listing.kind, WorkFileKindV1::Directory);
        assert!(listing.text.contains("src/\n"));
        assert!(listing.text.contains("README.md\t"));
        let read = grant
            .read(&project.join("src/main.rs").to_string_lossy())
            .unwrap();
        assert_eq!(read.kind, WorkFileKindV1::Text);
        assert_eq!(read.digest.len(), 64);
        assert!(read.text.contains("println"));
        let binary = grant
            .read(&project.join("logo.png").to_string_lossy())
            .unwrap();
        assert_eq!(binary.kind, WorkFileKindV1::Binary);
        assert!(binary.text.is_empty());
        let hits = grant.search(&project.to_string_lossy(), "HELLO").unwrap();
        assert_eq!(hits.kind, WorkFileKindV1::Search);
        assert!(hits.text.contains("README.md:2: hello world"));
        assert!(hits.text.contains("src/main.rs:2:"));
        for denied in [
            home.path().join(".ssh/id_rsa"),
            home.path().join("Documents/other.txt"),
            project.join("../secret.txt"),
            PathBuf::from("/etc/hosts"),
        ] {
            assert_eq!(
                grant.read(&denied.to_string_lossy()).unwrap_err(),
                WorkFileError::Denied,
                "{}",
                denied.display()
            );
        }
        assert_eq!(
            grant
                .read(&project.join("missing.txt").to_string_lossy())
                .unwrap_err(),
            WorkFileError::NotFound
        );
        let link = home.path().join("Documents/project/escape");
        #[cfg(unix)]
        std::os::unix::fs::symlink(home.path().join(".ssh"), &link).unwrap();
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // A junction exercises directory escapes without symlink privileges.
            let result = std::process::Command::new("cmd.exe")
                .args([
                    "/D",
                    "/C",
                    "mklink",
                    "/J",
                    "Documents\\project\\escape",
                    ".ssh",
                ])
                .current_dir(home.path())
                .creation_flags(0x0800_0000)
                .output()
                .unwrap();
            assert!(result.status.success(), "junction fixture: {result:?}");
        }
        assert_eq!(
            grant.list(&link.to_string_lossy()).unwrap_err(),
            WorkFileError::Denied
        );
        std::fs::write(home.path().join(".ssh/secret.txt"), "escape-only-needle").unwrap();
        assert!(!grant
            .list_at(&project.to_string_lossy(), 3)
            .unwrap()
            .text
            .contains("escape/"));
        assert_eq!(
            grant
                .search(&project.to_string_lossy(), "escape-only-needle")
                .unwrap()
                .bytes,
            0
        );
    }

    #[test]
    fn grants_refuse_folders_that_contain_a_protected_folder() {
        let _serial = crate::WORK_RUNTIME_TEST_SERIAL
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let (home, _home_scope, _grant, project) = home_grant();
        for protected in ["Library/Keychains", ".config/gcloud", "Library/Cookies"] {
            std::fs::create_dir_all(home.path().join(protected)).unwrap();
        }
        for parent in ["Library", ".config"] {
            let folder = home.path().join(parent);
            let (grant, refused) = WorkFileGrant::admit(&[folder.to_string_lossy().into_owned()]);
            assert!(grant.is_empty(), "{parent}");
            assert_eq!(refused.len(), 1, "{parent}");
        }
        let (grant, refused) = WorkFileGrant::admit(&[project.to_string_lossy().into_owned()]);
        assert!(!grant.is_empty());
        assert!(refused.is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn windows_grants_admit_native_paths_and_refuse_protected_descendants() {
        let _serial = crate::WORK_RUNTIME_TEST_SERIAL
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let (home, _home_scope, grant, project) = home_grant();
        for protected in [
            ".ssh/keys",
            ".config/gcloud/accounts",
            "AppData/Roaming/app.zephium",
            ".zephium-native-v1",
            ".ZEPHIUM-NATIVE-V1/app.zephium.webext-qa/session",
        ] {
            let folder = home.path().join(protected);
            std::fs::create_dir_all(&folder).unwrap();
            let (grant, refused) = WorkFileGrant::admit(&[folder.to_string_lossy().into_owned()]);
            assert!(grant.is_empty(), "{protected}");
            assert_eq!(refused.len(), 1);
        }
        let sibling = home.path().join(".zephium-native-v1-public");
        std::fs::create_dir(&sibling).unwrap();
        let (sibling_grant, refused) =
            WorkFileGrant::admit(&[sibling.to_string_lossy().into_owned()]);
        assert!(!sibling_grant.is_empty());
        assert!(refused.is_empty());
        let path = project.join("README.md");
        assert!(grant.read(&path.to_string_lossy()).is_ok());
        assert!(grant
            .read(&path.to_string_lossy().replace('\\', "/"))
            .is_ok());
        assert!(grant.read(&path.to_string_lossy().to_uppercase()).is_ok());
        assert_eq!(
            grant
                .read(&format!("{}:secret", path.display()))
                .unwrap_err(),
            WorkFileError::Denied
        );
        assert_eq!(
            grant.read(&format!("{}.", path.display())).unwrap_err(),
            WorkFileError::Denied
        );
    }

    #[cfg(windows)]
    #[test]
    fn redirected_known_folders_are_explicit_policy_roots() {
        let home = tempfile::tempdir().unwrap();
        let redirected = tempfile::tempdir().unwrap();
        let home = home.path().canonicalize().unwrap();
        let redirected = redirected.path().canonicalize().unwrap();
        let path = redirected.to_string_lossy();
        assert!(admit_root(&path, Some(&home), std::slice::from_ref(&home), &[]).is_none());
        assert!(admit_root(&path, Some(&home), &[home.clone(), redirected.clone()], &[]).is_some());
        assert!(admit_root(
            &home.to_string_lossy(),
            Some(&home),
            std::slice::from_ref(&home),
            &[]
        )
        .is_none());
        assert!(admit_root(
            &path,
            Some(&home),
            &[home.clone(), redirected.clone()],
            std::slice::from_ref(&redirected)
        )
        .is_none());
    }

    #[cfg(windows)]
    #[test]
    fn pinned_directory_cannot_be_replaced_during_a_file_operation() {
        let root = tempfile::tempdir().unwrap();
        let folder = root.path().join("files");
        std::fs::create_dir(&folder).unwrap();
        let pins = pin_directories(&folder.canonicalize().unwrap()).unwrap();
        assert!(std::fs::rename(&folder, root.path().join("replaced")).is_err());
        drop(pins);
        std::fs::rename(&folder, root.path().join("replaced")).unwrap();
    }

    #[cfg(windows)]
    fn set_junction(directory: &Path, target: &Path) -> windows::core::Result<()> {
        use std::os::windows::{ffi::OsStrExt, fs::OpenOptionsExt, io::AsRawHandle};
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::Storage::FileSystem::{
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        };
        use windows::Win32::System::IO::DeviceIoControl;
        let directory = std::fs::OpenOptions::new()
            .write(true)
            .custom_flags((FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT).0)
            .open(directory)
            .map_err(|_| windows::core::Error::from_win32())?;
        let target = target.to_string_lossy();
        let target = target.strip_prefix(r"\\?\").unwrap_or(&target);
        let substitute: Vec<u8> = std::ffi::OsStr::new(&format!(r"\??\{target}"))
            .encode_wide()
            .flat_map(u16::to_le_bytes)
            .collect();
        let print: Vec<u8> = std::ffi::OsStr::new(target)
            .encode_wide()
            .flat_map(u16::to_le_bytes)
            .collect();
        let mut data = Vec::new();
        data.extend(0xa000_0003u32.to_le_bytes());
        data.extend(((8 + substitute.len() + 2 + print.len() + 2) as u16).to_le_bytes());
        data.extend(0u16.to_le_bytes());
        data.extend(0u16.to_le_bytes());
        data.extend((substitute.len() as u16).to_le_bytes());
        data.extend(((substitute.len() + 2) as u16).to_le_bytes());
        data.extend((print.len() as u16).to_le_bytes());
        data.extend(substitute);
        data.extend(0u16.to_le_bytes());
        data.extend(print);
        data.extend(0u16.to_le_bytes());
        let mut returned = 0;
        unsafe {
            DeviceIoControl(
                HANDLE(directory.as_raw_handle()),
                0x0009_00a4,
                Some(data.as_ptr().cast()),
                data.len() as u32,
                None,
                0,
                Some(&mut returned),
                None,
            )
        }
    }

    #[cfg(windows)]
    #[test]
    fn approved_mutation_guard_refuses_in_place_junctions_before_marker_creation() {
        let root = tempfile::tempdir().unwrap();
        let folder = root.path().join("files");
        let outside = root.path().join("outside");
        std::fs::create_dir(&folder).unwrap();
        std::fs::create_dir(&outside).unwrap();
        let folder = folder.canonicalize().unwrap();
        let outside = outside.canonicalize().unwrap();
        let read_pins = pin_directories(&folder).unwrap();
        assert!(set_junction(&folder, &outside).is_err());
        drop(read_pins);
        let guard = pin_mutation_directory(&folder).unwrap();
        assert!(set_junction(&folder, &outside).is_err());
        drop(guard);
        assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 0);
        let pins = pin_directories_with_write(&folder, true).unwrap();
        set_junction(&folder, &outside).unwrap();
        assert!(matches!(
            mutation_marker(pins.last().unwrap(), &folder),
            Err(WorkFileError::Denied)
        ));
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
        assert!(pin_mutation_directory(&folder).is_err());
        drop(pins);
        std::fs::remove_dir(&folder).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn cloud_placeholders_are_distinct_from_name_surrogates() {
        for variant in 0..=15 {
            assert!(cloud_reparse_tag(0x9000_001a | (variant << 12)));
        }
        for tag in [0xa000_0003, 0xa000_000c, 0x8000_001b, 0x9000_001b] {
            assert!(!cloud_reparse_tag(tag));
        }
    }

    #[test]
    fn writes_are_proposed_as_diffs_and_applied_atomically() {
        let _serial = crate::WORK_RUNTIME_TEST_SERIAL
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let (_home, _home_scope, grant, project) = home_grant();
        let readme = project.join("README.md").to_string_lossy().into_owned();
        let proposal = grant
            .propose_edit(&readme, "hello world", "hello, world")
            .unwrap();
        assert!(proposal.contains("-hello world\n+hello, world\n"));
        assert_eq!(
            grant.propose_edit(&readme, "absent", "x").unwrap_err(),
            WorkFileError::Ambiguous
        );
        let applied = grant
            .apply_edit(&readme, "hello world", "hello, world")
            .unwrap();
        assert_eq!(applied.kind, WorkFileKindV1::Written);
        assert_eq!(
            std::fs::read_to_string(&readme).unwrap(),
            "# Project\nhello, world\n"
        );
        let fresh = project.join("notes.txt").to_string_lossy().into_owned();
        assert!(grant
            .propose_write(&fresh, "one\ntwo\n")
            .unwrap()
            .contains("+one\n+two\n"));
        grant.apply_write(&fresh, "one\ntwo\n").unwrap();
        assert_eq!(std::fs::read_to_string(&fresh).unwrap(), "one\ntwo\n");
        assert!(!project.join(".notes.txt.zephium-0").exists());
        assert_eq!(
            grant
                .apply_write(&project.join("../out.txt").to_string_lossy(), "x")
                .unwrap_err(),
            WorkFileError::Denied
        );
        let (text, cut) = clip("a".repeat(MAX_WORK_FILE_TEXT_BYTES + 10));
        assert!(cut && text.len() == MAX_WORK_FILE_TEXT_BYTES);
    }
    #[test]
    fn work_read_offsets_and_bounded_search() {
        let _serial = crate::WORK_RUNTIME_TEST_SERIAL
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let (_home, _home_scope, grant, project) = home_grant();
        let path = project.join("many.txt");
        std::fs::write(
            &path,
            (1..=500)
                .map(|n| format!("value {n}\n"))
                .collect::<String>(),
        )
        .unwrap();
        let read = grant.read_at(&path.to_string_lossy(), 199, 3).unwrap();
        assert_eq!(
            read.text,
            "199: value 199\n200: value 200\n201: value 201\n"
        );
        assert_eq!(
            read.lines,
            Some(WorkFileLinesV1 {
                first: 199,
                last: 201,
                total: 500
            })
        );
        assert!(read.truncated);
        assert!(grant.read_at(&path.to_string_lossy(), 0, 1).is_err());
        assert!(grant.read_at(&path.to_string_lossy(), 1, 2001).is_err());
        let found = grant
            .search_with(
                &project.to_string_lossy(),
                "value 20[01]$",
                Some("*.txt"),
                true,
            )
            .unwrap();
        assert_eq!(found.bytes, 2);
        assert!(found.text.contains("many.txt:199- value 199"));
        assert!(found.text.contains("many.txt:201: value 201"));
        assert!(grant
            .search_with(&project.to_string_lossy(), "[", None, true)
            .is_err());
        assert_eq!(
            grant
                .search_with(&project.to_string_lossy(), "value", Some("*.rs"), false)
                .unwrap()
                .bytes,
            0
        );
        assert!(grant
            .list_at(&project.to_string_lossy(), 3)
            .unwrap()
            .text
            .contains("src/main.rs"));
    }
    #[test]
    fn work_multi_edit_is_atomic_and_detects_changed_digests() {
        let _serial = crate::WORK_RUNTIME_TEST_SERIAL
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let (_home, _home_scope, grant, project) = home_grant();
        let path = project.join("README.md").to_string_lossy().into_owned();
        let before = grant.read(&path).unwrap();
        let mut kind = WorkStepKindV1::EditFile {
            path: path.clone(),
            old: String::new(),
            new: String::new(),
            decision: None,
            replacements: vec![
                WorkFileReplacementV1 {
                    old: "Project".into(),
                    new: "Demo".into(),
                },
                WorkFileReplacementV1 {
                    old: "missing".into(),
                    new: "world".into(),
                },
            ],
        };
        assert!(grant.prepare(&kind, Some(&before.digest)).is_err());
        assert_eq!(grant.read(&path).unwrap().digest, before.digest);
        if let WorkStepKindV1::EditFile { replacements, .. } = &mut kind {
            replacements[1].old = "hello world".into();
        }
        let proposal = grant.prepare(&kind, Some(&before.digest)).unwrap();
        std::fs::write(&path, "someone else edited this\n").unwrap();
        assert!(matches!(grant.apply(proposal), Err(WorkFileError::Changed)));
        assert!(matches!(
            grant.prepare(&kind, Some(&before.digest)),
            Err(WorkFileError::Changed)
        ));
        std::fs::write(&path, "# Project\nhello world\n").unwrap();
        let applied = grant
            .apply(grant.prepare(&kind, Some(&before.digest)).unwrap())
            .unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# Demo\nworld\n");
        assert_eq!(applied.before_digest, Some(before.digest));
        assert_eq!(applied.after_digest, Some(applied.digest.clone()));
        std::fs::write(&path, "aaa").unwrap();
        assert_eq!(
            grant.propose_edit(&path, "aa", "b").unwrap_err(),
            WorkFileError::Ambiguous
        );
    }
    #[test]
    #[ignore = "Native confined mutation and Trash gate is closed; retained for writer qualification"]
    fn work_moves_and_deletes_are_reviewed_and_exclusive() {
        let _serial = crate::WORK_RUNTIME_TEST_SERIAL
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let (_home, _home_scope, grant, project) = home_grant();
        let from = project.join("README.md").to_string_lossy().into_owned();
        let to = project.join("moved.md").to_string_lossy().into_owned();
        let kind = WorkStepKindV1::MoveFile {
            from: from.clone(),
            to: to.clone(),
            decision: None,
        };
        let change = grant.prepare(&kind, None).unwrap();
        assert!(Path::new(&from).exists());
        std::fs::write(&to, "other").unwrap();
        assert!(matches!(grant.apply(change), Err(WorkFileError::Exists)));
        assert!(Path::new(&from).exists());
        std::fs::remove_file(&to).unwrap();
        let record = grant.apply(grant.prepare(&kind, None).unwrap()).unwrap();
        assert_eq!(record.before_digest, record.after_digest);
        assert!(!Path::new(&from).exists());
        let change = grant
            .prepare(
                &WorkStepKindV1::DeleteFile {
                    path: to.clone(),
                    decision: None,
                },
                Some(&record.digest),
            )
            .unwrap();
        assert!(Path::new(&to).exists());
        let deleted = grant.apply(change).unwrap();
        assert!(!Path::new(&to).exists());
        assert!(deleted.before_digest.is_some());
        assert!(deleted.after_digest.is_none());
        assert!(grant
            .prepare(
                &WorkStepKindV1::DeleteFile {
                    path: project.to_string_lossy().into_owned(),
                    decision: None
                },
                None
            )
            .is_err());
        assert!(grant.propose_write(&to, "a\0b").is_err());
        assert!(grant.propose_write(&to, "a\u{1}b").is_err());
    }
}
