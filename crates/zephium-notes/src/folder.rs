//! Disk operations on one notes folder. Paths are folder-relative with `/`
//! separators and checked by `names::valid_path`; nothing here follows a
//! symbolic link or writes outside the folder.
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use zephium_core::notes::MAX_NOTE_BYTES;

use crate::names::{self, MAX_DEPTH, TRASH};

/// Upper bound on files considered in one scan, above the note capacity so a
/// crowded folder is reported rather than silently truncated mid-directory.
const MAX_SCANNED: usize = 20_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileMeta {
    pub size: u64,
    pub modified: i64,
    pub created: i64,
    /// Device and inode, where the platform exposes them, to follow a file
    /// renamed behind our back.
    pub identity: Option<(u64, u64)>,
}

#[derive(Debug)]
pub struct Entry {
    pub path: String,
    pub meta: FileMeta,
}

#[derive(Debug)]
pub struct Contents {
    /// At most `MAX_NOTE_BYTES + 1` bytes; one more than the limit proves the
    /// file is too large without reading all of it.
    pub bytes: Vec<u8>,
    pub meta: FileMeta,
}

impl Contents {
    pub fn complete(&self) -> bool {
        self.bytes.len() <= MAX_NOTE_BYTES
    }
}

pub struct Folder {
    root: PathBuf,
}

fn millis(time: io::Result<SystemTime>) -> Option<i64> {
    let duration = time.ok()?.duration_since(UNIX_EPOCH).ok()?;
    i64::try_from(duration.as_millis()).ok()
}

fn file_meta(meta: &fs::Metadata) -> FileMeta {
    let modified = millis(meta.modified()).unwrap_or(0);
    #[cfg(unix)]
    let identity = {
        use std::os::unix::fs::MetadataExt;
        Some((meta.dev(), meta.ino()))
    };
    #[cfg(not(unix))]
    let identity = None;
    FileMeta {
        size: meta.len(),
        modified,
        created: millis(meta.created()).unwrap_or(modified),
        identity,
    }
}

fn not_regular() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, "not a regular note file")
}

fn invalid_path() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, "note path outside the folder")
}

/// Creates a directory the browser owns, or accepts an existing real one.
fn ensure_directory(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_dir() => Ok(()),
        Ok(_) => Err(not_regular()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let builder = fs::DirBuilder::new();
            #[cfg(unix)]
            let builder = {
                use std::os::unix::fs::DirBuilderExt;
                let mut builder = builder;
                builder.mode(0o700);
                builder
            };
            match builder.create(path) {
                Err(error) if error.kind() != io::ErrorKind::AlreadyExists => Err(error),
                _ => match fs::symlink_metadata(path) {
                    Ok(meta) if meta.file_type().is_dir() => Ok(()),
                    _ => Err(not_regular()),
                },
            }
        }
        Err(error) => Err(error),
    }
}

/// Durably records a directory's entries after a create or rename in it.
fn sync_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        let directory = File::open(path)?;
        rustix::fs::fsync(&directory)?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// A plain `fsync`. On Apple platforms `File::sync_all` issues F_FULLFSYNC,
/// which flushes the whole drive cache; SQLite's default durability is a
/// plain fsync too, and a note saved every few seconds must not cost more.
fn sync_file(file: &File) -> io::Result<()> {
    #[cfg(unix)]
    {
        rustix::fs::fsync(file)?;
        Ok(())
    }
    #[cfg(not(unix))]
    file.sync_all()
}

fn open_options() -> OpenOptions {
    let options = OpenOptions::new();
    #[cfg(unix)]
    let options = {
        use std::os::unix::fs::OpenOptionsExt;
        let mut options = options;
        options.custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32);
        options
    };
    options
}

impl Folder {
    /// Opens the folder at `base/components…`, creating what is missing
    /// below `base`, which must already exist.
    pub fn open(base: &Path, components: &[&str]) -> io::Result<Self> {
        let mut current = fs::canonicalize(base)?;
        for component in components {
            if component.is_empty() || component.contains(['/', '\\']) || *component == ".." {
                return Err(invalid_path());
            }
            current.push(component);
            ensure_directory(&current)?;
        }
        Ok(Self { root: current })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The absolute path of a valid folder-relative path whose directories
    /// are all real directories. The file itself is checked when opened.
    pub fn resolve(&self, path: &str) -> io::Result<PathBuf> {
        if !names::valid_path(path) {
            return Err(invalid_path());
        }
        let mut absolute = self.root.clone();
        let mut components = path.split('/').peekable();
        while let Some(component) = components.next() {
            absolute.push(component);
            if components.peek().is_some() {
                match fs::symlink_metadata(&absolute) {
                    Ok(meta) if meta.file_type().is_dir() => {}
                    Ok(_) => return Err(not_regular()),
                    Err(error) => return Err(error),
                }
            }
        }
        Ok(absolute)
    }

    fn ensure_parent(&self, path: &str) -> io::Result<()> {
        let mut current = self.root.clone();
        let components: Vec<&str> = path.split('/').collect();
        for component in &components[..components.len() - 1] {
            current.push(component);
            ensure_directory(&current)?;
        }
        Ok(())
    }

    /// Every note file in the folder, and separately those in its trash.
    /// Hidden entries, links and anything that is not `.md` are skipped.
    pub fn scan(&self) -> io::Result<(Vec<Entry>, Vec<Entry>)> {
        let mut notes = Vec::new();
        let mut trash = Vec::new();
        let mut pending = vec![(self.root.clone(), String::new(), 0usize)];
        let mut seen = 0usize;
        while let Some((directory, prefix, depth)) = pending.pop() {
            let entries = match fs::read_dir(&directory) {
                Ok(entries) => entries,
                // The root must be readable; a subfolder that vanished or is
                // unreadable is only missing from this scan.
                Err(error) if depth == 0 => return Err(error),
                Err(_) => continue,
            };
            for entry in entries {
                let Ok(entry) = entry else { continue };
                let Ok(name) = entry.file_name().into_string() else {
                    continue;
                };
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                let path = format!("{prefix}{name}");
                let in_trash = prefix.starts_with(TRASH);
                if kind.is_dir() {
                    let descend = if depth == 0 && name == TRASH {
                        true
                    } else {
                        !name.starts_with('.')
                    };
                    if descend && depth + 1 < MAX_DEPTH {
                        pending.push((entry.path(), format!("{path}/"), depth + 1));
                    }
                    continue;
                }
                if !kind.is_file() || !names::valid_path(&path) {
                    continue;
                }
                seen += 1;
                if seen > MAX_SCANNED {
                    return Ok((notes, trash));
                }
                let Ok(meta) = entry.metadata() else { continue };
                let entry = Entry {
                    path,
                    meta: file_meta(&meta),
                };
                if in_trash {
                    trash.push(entry);
                } else {
                    notes.push(entry);
                }
            }
        }
        Ok((notes, trash))
    }

    pub fn stat(&self, path: &str) -> io::Result<FileMeta> {
        let meta = fs::symlink_metadata(self.resolve(path)?)?;
        if !meta.file_type().is_file() {
            return Err(not_regular());
        }
        Ok(file_meta(&meta))
    }

    pub fn read(&self, path: &str) -> io::Result<Contents> {
        let file = open_options().read(true).open(self.resolve(path)?)?;
        let meta = file.metadata()?;
        if !meta.file_type().is_file() {
            return Err(not_regular());
        }
        let mut bytes = Vec::with_capacity((meta.len() as usize).min(MAX_NOTE_BYTES + 1));
        file.take(MAX_NOTE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)?;
        Ok(Contents {
            bytes,
            meta: file_meta(&meta),
        })
    }

    /// Writes a new file and fails if the name is already taken.
    pub fn create(
        &self,
        path: &str,
        bytes: &[u8],
        modified: Option<SystemTime>,
    ) -> io::Result<FileMeta> {
        self.ensure_parent(path)?;
        let absolute = self.resolve(path)?;
        let mut options = open_options();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&absolute)?;
        let written = file.write_all(bytes).and_then(|()| {
            if let Some(time) = modified {
                file.set_modified(time)?;
            }
            sync_file(&file)
        });
        if let Err(error) = written {
            drop(file);
            let _ = fs::remove_file(&absolute);
            return Err(error);
        }
        let meta = file_meta(&file.metadata()?);
        sync_directory(absolute.parent().unwrap_or(&self.root))?;
        Ok(meta)
    }

    /// Replaces an existing file atomically: a crash leaves either the old
    /// contents or the new, never a torn note.
    pub fn replace(&self, path: &str, bytes: &[u8]) -> io::Result<FileMeta> {
        let absolute = self.resolve(path)?;
        let existing = fs::symlink_metadata(&absolute)?;
        if !existing.file_type().is_file() {
            return Err(not_regular());
        }
        let directory = absolute.parent().unwrap_or(&self.root).to_path_buf();
        let mut options = open_options();
        options.write(true).create_new(true);
        // External editors may use the full filesystem component limit for
        // a note name. Keep our temporary component independent of that name,
        // and reserve it atomically rather than treating any stat error as free.
        let mut reserved = None;
        for n in 0..16 {
            let temporary = directory.join(format!(".{}.{n}.zephium-save", std::process::id()));
            match options.open(&temporary) {
                Ok(file) => {
                    reserved = Some((temporary, file));
                    break;
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
        }
        let (temporary, mut file) = reserved
            .ok_or_else(|| io::Error::new(io::ErrorKind::AlreadyExists, "no temporary name"))?;
        let result = (|| {
            file.set_permissions(existing.permissions())?;
            file.write_all(bytes)?;
            sync_file(&file)?;
            drop(file);
            fs::rename(&temporary, &absolute)?;
            sync_directory(&directory)
        })();
        if let Err(error) = result {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
        self.stat(path)
    }

    /// Moves a note to a free name. The destination must not exist unless it
    /// names the same file, which is how a case-only rename looks on a
    /// case-insensitive disk.
    pub fn rename(&self, from: &str, to: &str) -> io::Result<FileMeta> {
        let source = self.resolve(from)?;
        let source_meta = fs::symlink_metadata(&source)?;
        if !source_meta.file_type().is_file() {
            return Err(not_regular());
        }
        self.ensure_parent(to)?;
        let target = self.resolve(to)?;
        if let Ok(target_meta) = fs::symlink_metadata(&target) {
            let same = file_meta(&target_meta).identity.is_some()
                && file_meta(&target_meta).identity == file_meta(&source_meta).identity;
            #[cfg(windows)]
            let same = same || fs::canonicalize(&source)? == fs::canonicalize(&target)?;
            if !same {
                return Err(io::Error::new(io::ErrorKind::AlreadyExists, "name taken"));
            }
        }
        fs::rename(&source, &target)?;
        sync_directory(source.parent().unwrap_or(&self.root))?;
        let target_directory = target.parent().unwrap_or(&self.root);
        if Some(target_directory) != source.parent() {
            sync_directory(target_directory)?;
        }
        self.stat(to)
    }

    pub fn remove(&self, path: &str) -> io::Result<()> {
        let absolute = self.resolve(path)?;
        if !fs::symlink_metadata(&absolute)?.file_type().is_file() {
            return Err(not_regular());
        }
        fs::remove_file(&absolute)?;
        sync_directory(absolute.parent().unwrap_or(&self.root))
    }

    pub fn exists(&self, path: &str) -> bool {
        self.resolve(path).and_then(fs::symlink_metadata).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder() -> (tempfile::TempDir, Folder) {
        let base = tempfile::tempdir().unwrap();
        let folder = Folder::open(base.path(), &["profile", "Notes"]).unwrap();
        (base, folder)
    }

    #[test]
    fn creates_reads_replaces_and_renames() {
        let (_base, folder) = folder();
        folder.create("Plans.md", b"# Plans", None).unwrap();
        assert!(folder.create("Plans.md", b"again", None).is_err());
        let meta = folder.replace("Plans.md", b"# Plans\n\nMore").unwrap();
        assert_eq!(meta.size, 13);
        assert_eq!(folder.read("Plans.md").unwrap().bytes, b"# Plans\n\nMore");
        folder.rename("Plans.md", "Trips/Lisbon.md").unwrap();
        assert!(!folder.exists("Plans.md"));
        assert_eq!(
            folder.read("Trips/Lisbon.md").unwrap().bytes,
            b"# Plans\n\nMore"
        );
        let (notes, trash) = folder.scan().unwrap();
        assert_eq!(notes.len(), 1);
        assert!(trash.is_empty());
        assert!(fs::read_dir(folder.root().join("Trips"))
            .unwrap()
            .all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with("zephium-save")));
    }

    #[test]
    fn renames_never_overwrite_another_note() {
        let (_base, folder) = folder();
        folder.create("A.md", b"a", None).unwrap();
        folder.create("B.md", b"b", None).unwrap();
        assert!(folder.rename("A.md", "B.md").is_err());
        assert_eq!(folder.read("B.md").unwrap().bytes, b"b");
    }

    #[test]
    fn replaces_notes_with_maximum_length_external_names() {
        let (_base, folder) = folder();
        let path = format!("{}.md", "x".repeat(252));
        assert!(names::valid_path(&path));
        folder.create(&path, b"before", None).unwrap();
        folder.replace(&path, b"after").unwrap();
        assert_eq!(folder.read(&path).unwrap().bytes, b"after");
    }

    #[test]
    fn scans_skip_hidden_links_and_other_files_but_see_the_trash() {
        let (_base, folder) = folder();
        folder.create("Visible.md", b"v", None).unwrap();
        folder.create(".trash/Gone.md", b"g", None).unwrap();
        fs::create_dir(folder.root().join(".obsidian")).unwrap();
        fs::write(folder.root().join(".obsidian/Config.md"), "c").unwrap();
        fs::write(folder.root().join("image.png"), "p").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            folder.root().join("Visible.md"),
            folder.root().join("Link.md"),
        )
        .unwrap();
        let (notes, trash) = folder.scan().unwrap();
        assert_eq!(
            notes.iter().map(|e| e.path.as_str()).collect::<Vec<_>>(),
            ["Visible.md"]
        );
        assert_eq!(
            trash.iter().map(|e| e.path.as_str()).collect::<Vec<_>>(),
            [".trash/Gone.md"]
        );
    }

    #[cfg(unix)]
    #[test]
    fn links_are_never_followed() {
        let (base, folder) = folder();
        let outside = base.path().join("outside.md");
        fs::write(&outside, "secret").unwrap();
        std::os::unix::fs::symlink(&outside, folder.root().join("Link.md")).unwrap();
        assert!(folder.read("Link.md").is_err());
        assert!(folder.replace("Link.md", b"x").is_err());
        std::os::unix::fs::symlink(base.path(), folder.root().join("Dir")).unwrap();
        assert!(folder.create("Dir/Escape.md", b"x", None).is_err());
        assert_eq!(fs::read_to_string(&outside).unwrap(), "secret");
        assert!(!base.path().join("Escape.md").exists());
    }

    #[test]
    fn reads_are_bounded() {
        let (_base, folder) = folder();
        folder
            .create("Big.md", &vec![b'a'; MAX_NOTE_BYTES + 10], None)
            .unwrap();
        let contents = folder.read("Big.md").unwrap();
        assert_eq!(contents.bytes.len(), MAX_NOTE_BYTES + 1);
        assert!(!contents.complete());
    }
}
