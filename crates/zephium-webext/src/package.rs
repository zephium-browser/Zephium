//! Immutable extension package publication shared by both native adapters.

use sha2::{Digest, Sha256};
use std::io::{self, Read};
use std::path::Path;

pub fn bytes_digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Bind provenance, transformation revision and prepared bytes without long
/// nested names that would consume Windows native path budgets.
pub fn publication_identity(source: &str, revision: &str, prepared: &str) -> String {
    format!(
        "pkg-{}",
        bytes_digest(format!("{source}\0{revision}\0{prepared}").as_bytes())
    )
}

/// Hash a bounded regular-file tree, including unambiguous relative names.
/// Publication only accepts trees produced by the safe archive boundary.
pub fn tree_digest(root: &Path) -> io::Result<String> {
    fn visit(
        root: &Path,
        directory: &Path,
        hash: &mut Sha256,
        entries: &mut usize,
        bytes: &mut u64,
    ) -> io::Result<()> {
        let limits = crate::archive::Limits::default();
        let mut children = Vec::new();
        for entry in std::fs::read_dir(directory)? {
            if children.len() + *entries >= limits.entries {
                return Err(io::Error::other("too many package files"));
            }
            children.push(entry?);
        }
        children.sort_by_key(|entry| entry.file_name());
        for entry in children {
            *entries += 1;
            if *entries > limits.entries {
                return Err(io::Error::other("too many package files"));
            }
            let path = entry.path();
            let relative = path.strip_prefix(root).map_err(io::Error::other)?;
            let name = relative
                .to_str()
                .ok_or_else(|| io::Error::other("invalid package filename"))?;
            let kind = entry.file_type()?;
            hash.update((name.len() as u64).to_le_bytes());
            hash.update(name.as_bytes());
            if kind.is_dir() {
                hash.update([0]);
                visit(root, &path, hash, entries, bytes)?;
            } else if kind.is_file() {
                hash.update([1]);
                let mut file = std::fs::File::open(&path)?;
                let length = file.metadata()?.len();
                *bytes = bytes
                    .checked_add(length)
                    .ok_or_else(|| io::Error::other("package too large"))?;
                if length > limits.file || *bytes > limits.total {
                    return Err(io::Error::other("package too large"));
                }
                hash.update(length.to_le_bytes());
                let copied =
                    io::copy(&mut file.by_ref().take(length + 1), &mut DigestWriter(hash))?;
                if copied != length {
                    return Err(io::Error::other("package changed during hashing"));
                }
            } else {
                return Err(io::Error::other("package contains a nonregular entry"));
            }
        }
        Ok(())
    }
    let mut hash = Sha256::new();
    visit(root, root, &mut hash, &mut 0, &mut 0)?;
    Ok(format!("{:x}", hash.finalize()))
}

struct DigestWriter<'a>(&'a mut Sha256);
impl std::io::Write for DigestWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Never remove or overwrite an existing executable tree. A concurrent or
/// repeated publication may reuse it only when its exact prepared bytes agree.
pub fn publish_directory(staged: &Path, target: &Path) -> io::Result<()> {
    let digest = tree_digest(staged)?;
    if target.exists() {
        if target.is_dir() && tree_digest(target)? == digest {
            return std::fs::remove_dir_all(staged);
        }
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "different published package",
        ));
    }
    match std::fs::rename(staged, target) {
        Ok(()) => Ok(()),
        Err(_) if target.is_dir() && tree_digest(target)? == digest => {
            std::fs::remove_dir_all(staged)?;
            Ok(())
        }
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publication_never_replaces_another_sources_executable_tree() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        let target = dir.path().join("target");
        std::fs::create_dir(&a).unwrap();
        std::fs::create_dir(&b).unwrap();
        std::fs::write(a.join("script.js"), b"trusted").unwrap();
        std::fs::write(b.join("script.js"), b"replaced").unwrap();
        publish_directory(&a, &target).unwrap();
        assert!(publish_directory(&b, &target).is_err());
        assert_eq!(std::fs::read(target.join("script.js")).unwrap(), b"trusted");
        assert!(b.is_dir());
    }
}

#[cfg(test)]
mod concurrent_tests {
    use super::*;

    #[test]
    fn simultaneous_publications_reuse_only_identical_prepared_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("published");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let handles: Vec<_> = ["signed", "unsigned"]
            .into_iter()
            .map(|code| {
                let staged = directory.path().join(code);
                std::fs::create_dir(&staged).unwrap();
                std::fs::write(staged.join("worker.js"), code).unwrap();
                let target = target.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    publish_directory(&staged, &target)
                })
            })
            .collect();
        assert_eq!(
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap().is_ok())
                .filter(|accepted| *accepted)
                .count(),
            1
        );
        assert!(["signed", "unsigned"].contains(
            &std::fs::read_to_string(target.join("worker.js"))
                .unwrap()
                .as_str()
        ));
    }
}
