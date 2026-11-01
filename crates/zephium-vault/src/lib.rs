//! File operations on the person's vault folder.
//!
//! Every path handed in is vault-relative with `/` separators and validated by
//! `zephium_core::vault::valid_name` per component. Directories along a path
//! are checked with `symlink_metadata`, so nothing here follows a symbolic link
//! or junction out of the vault, and nothing is ever permanently deleted: the
//! trash is the vault's own `.trash` folder, shared with Obsidian.
#![forbid(unsafe_code)]

mod search;
mod watch;

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use zephium_core::vault::{
    valid_name, VaultEntry, VaultEntryKind, VaultError, MAX_VAULT_LIST, MAX_VAULT_READ_BYTES,
};

pub use search::NameIndex;
pub use watch::{watch, VaultWatcher};

pub const TRASH: &str = ".trash";
const SAVE_SUFFIX: &str = ".zephium-save";

pub type Result<T> = std::result::Result<T, VaultError>;

fn io_error(error: io::Error) -> VaultError {
    match error.kind() {
        io::ErrorKind::NotFound => VaultError::NotFound,
        io::ErrorKind::AlreadyExists => VaultError::Exists,
        io::ErrorKind::InvalidInput | io::ErrorKind::InvalidData => VaultError::Invalid,
        _ => VaultError::Io,
    }
}

fn millis(time: io::Result<SystemTime>) -> u128 {
    time.ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |duration| duration.as_millis())
}

/// Hidden entries (`.obsidian`, `.trash`, `.git`) and our own save temporaries
/// never appear in listings or search.
pub fn hidden(name: &str) -> bool {
    name.starts_with('.') || name.ends_with(SAVE_SUFFIX) || name.eq_ignore_ascii_case("desktop.ini")
}

#[derive(Clone, Debug)]
pub struct Vault {
    root: PathBuf,
}

impl Vault {
    /// Opens an existing folder as a vault.
    pub fn open(root: &Path) -> Result<Self> {
        let root = fs::canonicalize(root).map_err(io_error)?;
        if !fs::metadata(&root).map_err(io_error)?.is_dir() {
            return Err(VaultError::Invalid);
        }
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The absolute path for a vault-relative path. Every directory on the way
    /// must be a real directory; the final component may be anything or absent.
    pub fn resolve(&self, path: &str) -> Result<PathBuf> {
        let mut absolute = self.root.clone();
        if path.is_empty() {
            return Ok(absolute);
        }
        let parts: Vec<&str> = path.split('/').collect();
        for (index, part) in parts.iter().enumerate() {
            if !valid_name(part) {
                return Err(VaultError::Invalid);
            }
            absolute.push(part);
            match fs::symlink_metadata(&absolute) {
                Ok(meta) if meta.file_type().is_symlink() => return Err(VaultError::Invalid),
                Ok(meta) if index + 1 < parts.len() && !meta.is_dir() => {
                    return Err(VaultError::NotFound)
                }
                Ok(_) => {}
                Err(error)
                    if index + 1 == parts.len() && error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(io_error(error)),
            }
        }
        Ok(absolute)
    }

    fn resolve_dir(&self, dir: &str) -> Result<PathBuf> {
        let path = self.resolve(dir)?;
        match fs::symlink_metadata(&path) {
            Ok(meta) if meta.is_dir() => Ok(path),
            Ok(_) => Err(VaultError::Invalid),
            Err(error) => Err(io_error(error)),
        }
    }

    fn resolve_existing(&self, path: &str) -> Result<PathBuf> {
        let absolute = self.resolve(path)?;
        fs::symlink_metadata(&absolute).map_err(io_error)?;
        Ok(absolute)
    }

    /// The vault-relative path of an absolute path inside the vault.
    pub fn relative(&self, absolute: &Path) -> Option<String> {
        let rest = absolute.strip_prefix(&self.root).ok()?;
        let mut parts = Vec::new();
        for component in rest.components() {
            match component {
                std::path::Component::Normal(part) => parts.push(part.to_str()?.to_owned()),
                _ => return None,
            }
        }
        Some(parts.join("/"))
    }

    fn entry(&self, relative: String, absolute: &Path) -> Result<VaultEntry> {
        let meta = fs::symlink_metadata(absolute).map_err(io_error)?;
        let name = relative.rsplit('/').next().unwrap_or_default().to_owned();
        let folder = meta.is_dir();
        let ext = if folder {
            String::new()
        } else {
            Path::new(&name)
                .extension()
                .and_then(|ext| ext.to_str())
                .map(str::to_ascii_lowercase)
                .unwrap_or_default()
        };
        Ok(VaultEntry {
            name,
            path: relative,
            kind: if folder {
                VaultEntryKind::Folder
            } else {
                VaultEntryKind::File
            },
            ext,
            size: if folder {
                "0".into()
            } else {
                meta.len().to_string()
            },
            modified: millis(meta.modified()).to_string(),
        })
    }

    pub fn stat(&self, path: &str) -> Result<VaultEntry> {
        let absolute = self.resolve_existing(path)?;
        self.entry(path.to_owned(), &absolute)
    }

    /// One folder's visible children: folders first, then files, each in
    /// natural order ("Topic 2" before "Topic 10").
    pub fn list(&self, dir: &str) -> Result<Vec<VaultEntry>> {
        let absolute = self.resolve_dir(dir)?;
        let mut entries = Vec::new();
        for item in fs::read_dir(&absolute).map_err(io_error)? {
            let Ok(item) = item else { continue };
            let Some(name) = item.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if hidden(&name) || !valid_name(&name) {
                continue;
            }
            let Ok(kind) = item.file_type() else { continue };
            if kind.is_symlink() {
                continue;
            }
            let relative = if dir.is_empty() {
                name
            } else {
                format!("{dir}/{name}")
            };
            if let Ok(entry) = self.entry(relative, &item.path()) {
                entries.push(entry);
            }
            if entries.len() >= MAX_VAULT_LIST {
                break;
            }
        }
        entries.sort_by(|a, b| {
            (a.kind != VaultEntryKind::Folder)
                .cmp(&(b.kind != VaultEntryKind::Folder))
                .then_with(|| natural_cmp(&a.name, &b.name))
        });
        Ok(entries)
    }

    fn join(dir: &str, name: &str) -> String {
        if dir.is_empty() {
            name.to_owned()
        } else {
            format!("{dir}/{name}")
        }
    }

    /// `name`, or `name 1`, `name 2`... before the extension, whichever is free.
    fn free_name(&self, dir: &Path, name: &str) -> Result<String> {
        if !exists(&dir.join(name)) {
            return Ok(name.to_owned());
        }
        let (stem, ext) = match name.rfind('.') {
            Some(at) if at > 0 => (&name[..at], &name[at..]),
            _ => (name, ""),
        };
        for n in 1..1000 {
            let candidate = format!("{stem} {n}{ext}");
            if !exists(&dir.join(&candidate)) {
                return Ok(candidate);
            }
        }
        Err(VaultError::Exists)
    }

    pub fn create_folder(&self, dir: &str, name: &str) -> Result<VaultEntry> {
        let parent = self.resolve_dir(dir)?;
        let name = self.free_name(&parent, name)?;
        let absolute = parent.join(&name);
        fs::create_dir(&absolute).map_err(io_error)?;
        self.entry(Self::join(dir, &name), &absolute)
    }

    pub fn create_file(&self, dir: &str, name: &str, bytes: &[u8]) -> Result<VaultEntry> {
        let parent = self.resolve_dir(dir)?;
        let name = self.free_name(&parent, name)?;
        let absolute = parent.join(&name);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&absolute)
            .map_err(io_error)?;
        file.write_all(bytes).map_err(io_error)?;
        file.sync_all().map_err(io_error)?;
        self.entry(Self::join(dir, &name), &absolute)
    }

    /// Renames in place. A case-only rename is allowed on case-insensitive
    /// file systems; any other existing target is refused.
    pub fn rename(&self, path: &str, name: &str) -> Result<VaultEntry> {
        if !valid_name(name) {
            return Err(VaultError::Invalid);
        }
        let from = self.resolve_existing(path)?;
        let dir = parent_of(path);
        let to = from.with_file_name(name);
        let case_only = from
            .file_name()
            .and_then(|old| old.to_str())
            .is_some_and(|old| old != name && old.eq_ignore_ascii_case(name));
        if exists(&to) && !case_only {
            return Err(VaultError::Exists);
        }
        fs::rename(&from, &to).map_err(io_error)?;
        self.entry(Self::join(dir, name), &to)
    }

    /// Moves a file or folder into another folder, keeping its name (or the
    /// next free one). A folder cannot move into itself.
    pub fn move_to(&self, path: &str, dest: &str) -> Result<VaultEntry> {
        if dest == path || dest.starts_with(&format!("{path}/")) {
            return Err(VaultError::Invalid);
        }
        let from = self.resolve_existing(path)?;
        let target_dir = self.resolve_dir(dest)?;
        let name = from
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(VaultError::Invalid)?
            .to_owned();
        if parent_of(path) == dest {
            return self.stat(path);
        }
        let name = self.free_name(&target_dir, &name)?;
        let to = target_dir.join(&name);
        move_path(&from, &to)?;
        self.entry(Self::join(dest, &name), &to)
    }

    /// Moves into `.trash`, keeping the folder structure flat like Obsidian.
    pub fn trash(&self, path: &str) -> Result<()> {
        let from = self.resolve_existing(path)?;
        let trash = self.root.join(TRASH);
        match fs::symlink_metadata(&trash) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => return Err(VaultError::Io),
            Err(_) => fs::create_dir(&trash).map_err(io_error)?,
        }
        let name = from
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(VaultError::Invalid)?;
        let name = self.free_name(&trash, name)?;
        move_path(&from, &trash.join(name))
    }

    /// Copies a regular file from anywhere into `dest`.
    pub fn import(&self, source: &Path, dest: &str) -> Result<VaultEntry> {
        let target_dir = self.resolve_dir(dest)?;
        let meta = fs::metadata(source).map_err(io_error)?;
        if !meta.is_file() {
            return Err(VaultError::Invalid);
        }
        let name = source
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| valid_name(name))
            .ok_or(VaultError::Invalid)?;
        let name = self.free_name(&target_dir, name)?;
        let to = target_dir.join(&name);
        if fs::canonicalize(source)
            .ok()
            .is_some_and(|real| real.starts_with(&self.root))
        {
            // A file already in the vault (the inbox) moves rather than copies.
            move_path(source, &to)?;
        } else {
            fs::copy(source, &to).map_err(io_error)?;
        }
        self.entry(Self::join(dest, &name), &to)
    }

    /// Reads a whole regular file, refusing anything above `max` bytes.
    pub fn read(&self, path: &str, max: u64) -> Result<(Vec<u8>, String)> {
        let absolute = self.resolve_existing(path)?;
        let mut file = File::open(&absolute).map_err(io_error)?;
        let meta = file.metadata().map_err(io_error)?;
        if !meta.is_file() {
            return Err(VaultError::Invalid);
        }
        if meta.len() > max.min(MAX_VAULT_READ_BYTES) {
            return Err(VaultError::TooLarge);
        }
        let mut bytes = Vec::with_capacity(meta.len() as usize);
        Read::by_ref(&mut file)
            .take(max + 1)
            .read_to_end(&mut bytes)
            .map_err(io_error)?;
        if bytes.len() as u64 > max {
            return Err(VaultError::TooLarge);
        }
        Ok((bytes, millis(meta.modified()).to_string()))
    }

    /// Replaces an existing file through a temporary file and rename, only if
    /// it still has the modification time the writer read.
    pub fn write(&self, path: &str, bytes: &[u8], expected_modified: &str) -> Result<String> {
        let absolute = self.resolve_existing(path)?;
        let meta = fs::symlink_metadata(&absolute).map_err(io_error)?;
        if !meta.is_file() {
            return Err(VaultError::Invalid);
        }
        if millis(meta.modified()).to_string() != expected_modified {
            return Err(VaultError::Changed);
        }
        let name = absolute
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(VaultError::Invalid)?;
        let temporary = absolute.with_file_name(format!(".{name}{SAVE_SUFFIX}"));
        let written = (|| -> io::Result<()> {
            let mut file = File::create(&temporary)?;
            file.write_all(bytes)?;
            file.sync_all()
        })();
        if let Err(error) = written {
            let _ = fs::remove_file(&temporary);
            return Err(io_error(error));
        }
        if let Err(error) = fs::rename(&temporary, &absolute) {
            let _ = fs::remove_file(&temporary);
            return Err(io_error(error));
        }
        Ok(millis(fs::metadata(&absolute).and_then(|meta| meta.modified())).to_string())
    }
}

fn exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

fn parent_of(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(parent, _)| parent)
}

/// Rename, falling back to copy-and-remove for a file across volumes.
fn move_path(from: &Path, to: &Path) -> Result<()> {
    if exists(to) {
        return Err(VaultError::Exists);
    }
    match fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(_) if fs::symlink_metadata(from).is_ok_and(|meta| meta.is_file()) => {
            fs::copy(from, to).map_err(io_error)?;
            fs::remove_file(from).map_err(io_error)
        }
        Err(error) => Err(io_error(error)),
    }
}

/// Case-insensitive comparison in which runs of digits compare by value.
pub fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let take = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut digits = String::new();
                    while let Some(c) = it.peek().copied().filter(char::is_ascii_digit) {
                        digits.push(c);
                        it.next();
                    }
                    digits
                };
                let (x, y) = (take(&mut a), take(&mut b));
                let (xs, ys) = (x.trim_start_matches('0'), y.trim_start_matches('0'));
                let order = xs.len().cmp(&ys.len()).then_with(|| xs.cmp(ys));
                if order != Ordering::Equal {
                    return order;
                }
            }
            (Some(x), Some(y)) => {
                let order = x.to_lowercase().cmp(y.to_lowercase());
                if order != Ordering::Equal {
                    return order;
                }
                a.next();
                b.next();
            }
        }
    }
}

#[cfg(test)]
mod tests;
