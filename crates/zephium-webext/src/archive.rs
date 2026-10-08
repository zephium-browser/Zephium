//! Extraction of extension archives into a fresh directory.

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Cursor, Read};
use std::path::{Path, PathBuf};

use thiserror::Error;
use zip::result::ZipError;
use zip::ZipArchive;

const S_IFMT: u32 = 0o170000;
const S_IFREG: u32 = 0o100000;
const S_IFDIR: u32 = 0o040000;
const S_IFLNK: u32 = 0o120000;

/// Chrome Web Store signing metadata, never part of the extension itself.
const METADATA_DIR: &str = "_metadata";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub total: u64,
    pub file: u64,
    pub entries: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            total: 1024 * 1024 * 1024,
            file: 64 * 1024 * 1024,
            entries: 20_000,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ExtractStats {
    pub files: usize,
    pub bytes: u64,
}

#[derive(Debug, Error)]
pub enum ArchiveError {
    #[error("destination already exists")]
    DestinationExists,
    #[error("invalid ZIP archive: {0}")]
    Zip(#[from] ZipError),
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("unsafe entry path {0:?}")]
    UnsafePath(String),
    #[error("entry {0:?} is a symbolic link")]
    Symlink(String),
    #[error("entry {0:?} is not a regular file or directory")]
    UnsupportedEntry(String),
    #[error("entry {0:?} is encrypted")]
    Encrypted(String),
    #[error("entry {0:?} duplicates another entry")]
    Duplicate(String),
    #[error("archive has more than {0} entries")]
    TooManyEntries(usize),
    #[error("entry {0:?} exceeds the per-file size limit")]
    FileTooLarge(String),
    #[error("archive exceeds the total size limit")]
    TooLarge,
}

/// Copies an unpacked extension from `src` into `dest`, which must not exist
/// yet, under the same limits as extraction. Symbolic links are refused, so
/// nothing outside `src` is copied. On failure nothing is left at `dest`.
pub fn copy_dir(src: &Path, dest: &Path, limits: &Limits) -> Result<ExtractStats, ArchiveError> {
    if dest.exists() {
        return Err(ArchiveError::DestinationExists);
    }
    let mut stats = ExtractStats { files: 0, bytes: 0 };
    let result = copy_level(src, dest, src, limits, &mut stats);
    if result.is_err() {
        let _ = fs::remove_dir_all(dest);
    }
    result.map(|()| stats)
}

fn copy_level(
    root: &Path,
    dest: &Path,
    dir: &Path,
    limits: &Limits,
    stats: &mut ExtractStats,
) -> Result<(), ArchiveError> {
    fs::create_dir_all(dest.join(dir.strip_prefix(root).unwrap_or(Path::new(""))))?;
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let relative = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .into_owned();
        let kind = entry.file_type()?;
        stats.files += 1;
        if stats.files > limits.entries {
            return Err(ArchiveError::TooManyEntries(limits.entries));
        }
        if kind.is_symlink() {
            return Err(ArchiveError::Symlink(relative));
        } else if kind.is_dir() {
            copy_level(root, dest, &path, limits, stats)?;
        } else if kind.is_file() {
            let size = entry.metadata()?.len();
            if size > limits.file {
                return Err(ArchiveError::FileTooLarge(relative));
            }
            stats.bytes += size;
            if stats.bytes > limits.total {
                return Err(ArchiveError::TooLarge);
            }
            fs::copy(&path, dest.join(&relative))?;
        } else {
            return Err(ArchiveError::UnsupportedEntry(relative));
        }
    }
    Ok(())
}

struct Entry {
    index: usize,
    path: String,
    is_dir: bool,
}

/// Extracts `zip` into `dest`, which must not exist yet. On failure nothing
/// is left behind at `dest`.
pub fn extract(zip: &[u8], dest: &Path, limits: &Limits) -> Result<ExtractStats, ArchiveError> {
    let mut archive = ZipArchive::new(Cursor::new(zip))?;
    if archive.len() > limits.entries {
        return Err(ArchiveError::TooManyEntries(limits.entries));
    }
    let entries = plan(&mut archive)?;

    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::create_dir(dest).map_err(|error| match error.kind() {
        io::ErrorKind::AlreadyExists => ArchiveError::DestinationExists,
        _ => error.into(),
    })?;
    write_entries(&mut archive, &entries, dest, limits).inspect_err(|_| {
        let _ = fs::remove_dir_all(dest);
    })
}

fn plan(archive: &mut ZipArchive<Cursor<&[u8]>>) -> Result<Vec<Entry>, ArchiveError> {
    let mut seen = HashSet::new();
    let mut entries = Vec::with_capacity(archive.len());
    for index in 0..archive.len() {
        let file = archive.by_index_raw(index)?;
        let name = file.name().to_owned();
        let mode = file.unix_mode().unwrap_or(0) & S_IFMT;
        let is_dir = name.ends_with('/') || mode == S_IFDIR;
        let path = validate_path(name.strip_suffix('/').unwrap_or(&name))
            .ok_or_else(|| ArchiveError::UnsafePath(name.clone()))?;

        if path
            .split('/')
            .next()
            .is_some_and(|first| first.eq_ignore_ascii_case(METADATA_DIR))
        {
            continue;
        }
        match mode {
            S_IFLNK => return Err(ArchiveError::Symlink(name)),
            0 | S_IFREG | S_IFDIR => {}
            _ => return Err(ArchiveError::UnsupportedEntry(name)),
        }
        if file.encrypted() {
            return Err(ArchiveError::Encrypted(name));
        }
        if !seen.insert(path.to_lowercase()) {
            return Err(ArchiveError::Duplicate(name));
        }
        entries.push(Entry {
            index,
            path,
            is_dir,
        });
    }
    Ok(entries)
}

fn validate_path(name: &str) -> Option<String> {
    let safe = !name.is_empty()
        && !name.contains('\\')
        && !name.chars().any(char::is_control)
        && name.split('/').all(|part| {
            !part.is_empty() && part != "." && part != ".." && crate::manifest::portable_name(part)
        });
    safe.then(|| name.to_owned())
}

fn write_entries(
    archive: &mut ZipArchive<Cursor<&[u8]>>,
    entries: &[Entry],
    dest: &Path,
    limits: &Limits,
) -> Result<ExtractStats, ArchiveError> {
    let mut stats = ExtractStats::default();
    for entry in entries {
        let target = dest.join(&entry.path);
        if entry.is_dir {
            create_dirs(dest, &target)?;
            continue;
        }
        if let Some(parent) = target.parent() {
            create_dirs(dest, parent)?;
        }

        let remaining = limits.total - stats.bytes;
        let allowed = limits.file.min(remaining);
        let mut out = create_file(&target)?;
        let mut reader = archive.by_index(entry.index)?.take(allowed + 1);
        let written = io::copy(&mut reader, &mut out)?;
        if written > allowed {
            return Err(if written > limits.file {
                ArchiveError::FileTooLarge(entry.path.clone())
            } else {
                ArchiveError::TooLarge
            });
        }
        stats.bytes += written;
        stats.files += 1;
    }
    Ok(stats)
}

fn create_dirs(root: &Path, dir: &Path) -> io::Result<()> {
    let mut current = PathBuf::from(root);
    for part in dir.strip_prefix(root).unwrap_or(dir).components() {
        current.push(part);
        match fs::create_dir(&current) {
            Ok(()) => set_mode(&current, 0o755)?,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                if !fs::symlink_metadata(&current)?.is_dir() {
                    return Err(error);
                }
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn create_file(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new().write(true).create_new(true).open(path)?;
    set_mode(path, 0o644)?;
    Ok(file)
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn set_mode(_: &Path, _: u32) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use zip::write::SimpleFileOptions;
    use zip::ZipWriter;

    use super::*;
    use crate::crx::tests::zip_of;

    #[test]
    fn entry_names_stay_inside_the_package_on_every_platform() {
        for name in ["manifest.json", "js/app.js", "_locales/en/messages.json"] {
            assert!(validate_path(name).is_some(), "{name}");
        }
        for name in [
            "C:/Windows/evil.dll",
            "C:x",
            "js/file.js:stream",
            "CON",
            "js/nul.txt",
            "js/com1.js",
            "js/trailing.",
            "js/trailing ",
            "../outside",
            "/absolute",
        ] {
            assert!(validate_path(name).is_none(), "{name}");
        }
    }

    fn extract_to_temp(
        zip: &[u8],
        limits: &Limits,
    ) -> (
        tempfile::TempDir,
        PathBuf,
        Result<ExtractStats, ArchiveError>,
    ) {
        let temp = tempfile::tempdir().unwrap();
        let dest = temp.path().join("out");
        let result = extract(zip, &dest, limits);
        (temp, dest, result)
    }

    #[test]
    fn extracts_files_and_skips_store_metadata() {
        let zip = zip_of(&[
            ("manifest.json", b"{}"),
            ("js/background.js", b"console.log(1)"),
            ("_metadata/verified_contents.json", b"[]"),
        ]);
        let (_temp, dest, result) = extract_to_temp(&zip, &Limits::default());
        let stats = result.unwrap();
        assert_eq!(
            stats,
            ExtractStats {
                files: 2,
                bytes: 16
            }
        );
        assert_eq!(
            fs::read(dest.join("js/background.js")).unwrap(),
            b"console.log(1)"
        );
        assert!(!dest.join("_metadata").exists());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&dest.join("manifest.json")), 0o644);
            assert_eq!(mode(&dest.join("js")), 0o755);
        }
    }

    #[test]
    fn rejects_existing_destination() {
        let temp = tempfile::tempdir().unwrap();
        let result = extract(&zip_of(&[("a", b"")]), temp.path(), &Limits::default());
        assert!(matches!(result, Err(ArchiveError::DestinationExists)));
        assert!(temp.path().exists());
    }

    #[test]
    fn rejects_path_traversal() {
        for name in [
            "../evil.js",
            "a/../../evil.js",
            "/etc/passwd",
            "a\\b.js",
            "a//b",
            "a\u{1}b",
        ] {
            let (_temp, dest, result) =
                extract_to_temp(&zip_of(&[("ok.js", b""), (name, b"x")]), &Limits::default());
            assert!(
                matches!(result, Err(ArchiveError::UnsafePath(_))),
                "{name:?} was accepted"
            );
            assert!(!dest.exists());
        }
    }

    #[test]
    fn rejects_symlinks() {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .add_symlink("link", "/etc/passwd", SimpleFileOptions::default())
            .unwrap();
        let zip = writer.finish().unwrap().into_inner();
        let (_temp, dest, result) = extract_to_temp(&zip, &Limits::default());
        assert!(matches!(result, Err(ArchiveError::Symlink(_))));
        assert!(!dest.exists());
    }

    #[test]
    fn copies_unpacked_folders_and_refuses_links() {
        let temp = tempfile::tempdir().unwrap();
        let src = temp.path().join("src");
        fs::create_dir_all(src.join("js")).unwrap();
        fs::write(src.join("manifest.json"), b"{}").unwrap();
        fs::write(src.join("js/bg.js"), b"x").unwrap();
        let stats = copy_dir(&src, &temp.path().join("copy"), &Limits::default()).unwrap();
        assert_eq!(stats.bytes, 3);
        assert_eq!(fs::read(temp.path().join("copy/js/bg.js")).unwrap(), b"x");

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/etc/hosts", src.join("hosts")).unwrap();
            let dest = temp.path().join("linked");
            assert!(matches!(
                copy_dir(&src, &dest, &Limits::default()),
                Err(ArchiveError::Symlink(_))
            ));
            assert!(!dest.exists());
        }
    }

    #[test]
    fn rejects_case_insensitive_duplicates() {
        let zip = zip_of(&[("Popup.html", b"a"), ("popup.HTML", b"b")]);
        let (_temp, _, result) = extract_to_temp(&zip, &Limits::default());
        assert!(matches!(result, Err(ArchiveError::Duplicate(_))));
    }

    #[test]
    fn enforces_size_and_entry_limits() {
        let big = vec![0u8; 4096];
        let zip = zip_of(&[("a.bin", &big), ("b.bin", &big)]);

        let per_file = Limits {
            file: 4095,
            ..Limits::default()
        };
        let (_temp, dest, result) = extract_to_temp(&zip, &per_file);
        assert!(matches!(result, Err(ArchiveError::FileTooLarge(_))));
        assert!(!dest.exists());

        let total = Limits {
            total: 8191,
            ..Limits::default()
        };
        let (_temp, _, result) = extract_to_temp(&zip, &total);
        assert!(matches!(result, Err(ArchiveError::TooLarge)));

        let entries = Limits {
            entries: 1,
            ..Limits::default()
        };
        let (_temp, _, result) = extract_to_temp(&zip, &entries);
        assert!(matches!(result, Err(ArchiveError::TooManyEntries(1))));

        let exact = Limits {
            total: 8192,
            file: 4096,
            entries: 2,
        };
        let (_temp, _, result) = extract_to_temp(&zip, &exact);
        assert_eq!(result.unwrap().bytes, 8192);
    }

    #[test]
    fn distrusts_declared_sizes() {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        writer.start_file("bomb.bin", options).unwrap();
        writer.write_all(&vec![0u8; 1 << 20]).unwrap();
        let mut zip = writer.finish().unwrap().into_inner();

        // Understate the uncompressed size in the central directory record.
        let central = zip.windows(4).rposition(|w| w == b"PK\x01\x02").unwrap();
        zip[central + 24..central + 28].copy_from_slice(&16u32.to_le_bytes());
        let local = zip.windows(4).position(|w| w == b"PK\x03\x04").unwrap();
        zip[local + 22..local + 26].copy_from_slice(&16u32.to_le_bytes());

        let limits = Limits {
            file: 1024,
            ..Limits::default()
        };
        let (_temp, dest, result) = extract_to_temp(&zip, &limits);
        assert!(
            matches!(result, Err(ArchiveError::FileTooLarge(_))),
            "{result:?}"
        );
        assert!(!dest.exists());
    }
}
