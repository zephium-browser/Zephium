//! The child never receives a source grant. It sees only bounded checked copies.
use super::Failure;
#[cfg(not(target_os = "macos"))]
use crate::work_files::WorkFileGrant;
use crate::work_files::{protected_path, snapshot_bytes};
use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
};
use tempfile::TempDir;

const MAX_FILES: usize = 2_000;
const MAX_FILE: u64 = 1024 * 1024;
pub(super) const MAX_TOTAL: usize = 32 * 1024 * 1024;
const MAX_DEPTH: usize = 32;

pub(super) struct Snapshot {
    // This owned root contains only broker-authored names. Source paths and Git
    // metadata are never mounted, linked or copied without file-policy checks.
    _directory: TempDir,
    pub root: PathBuf,
    pub work: PathBuf,
    pub home: PathBuf,
    pub temporary: PathBuf,
    pub empty: PathBuf,
    pub files: BTreeMap<PathBuf, Vec<u8>>,
}
impl Snapshot {
    #[cfg(not(target_os = "macos"))]
    pub fn capture_checked(
        grant: &WorkFileGrant,
        requested: &Path,
        selected: &[PathBuf],
        check: &dyn Fn() -> Result<(), Failure>,
    ) -> Result<Self, Failure> {
        check()?;
        let root = grant
            .resolve(requested.to_str().ok_or(Failure::Input)?, false)
            .map_err(|_| Failure::Input)?;
        if root != requested {
            return Err(Failure::Input);
        }
        let files = collect_checked(&root, selected, check)?;
        Self::prepare_checked(files, true, check)
    }

    #[cfg(test)]
    pub(super) fn from_files(files: BTreeMap<PathBuf, Vec<u8>>) -> Result<Self, Failure> {
        Self::prepare(files, true)
    }

    #[cfg(test)]
    pub(super) fn read_only(files: BTreeMap<PathBuf, Vec<u8>>) -> Result<Self, Failure> {
        Self::prepare(files, false)
    }

    pub(super) fn from_files_checked(
        files: BTreeMap<PathBuf, Vec<u8>>,
        check: &dyn Fn() -> Result<(), Failure>,
    ) -> Result<Self, Failure> {
        Self::prepare_checked(files, true, check)
    }
    pub(super) fn read_only_checked(
        files: BTreeMap<PathBuf, Vec<u8>>,
        check: &dyn Fn() -> Result<(), Failure>,
    ) -> Result<Self, Failure> {
        Self::prepare_checked(files, false, check)
    }

    #[cfg(test)]
    fn prepare(files: BTreeMap<PathBuf, Vec<u8>>, git_metadata: bool) -> Result<Self, Failure> {
        Self::prepare_checked(files, git_metadata, &|| Ok(()))
    }
    pub(super) fn validate_files(
        files: &BTreeMap<PathBuf, Vec<u8>>,
        check: &dyn Fn() -> Result<(), Failure>,
    ) -> Result<(), Failure> {
        check()?;
        if files.len() > MAX_FILES {
            return Err(Failure::Limit);
        }
        let mut total = 0usize;
        for (path, bytes) in files {
            check()?;
            if path.as_os_str().is_empty()
                || path.is_absolute()
                || protected_path(path)
                || path.components().count() > MAX_DEPTH
                || !path
                    .components()
                    .all(|part| matches!(part, Component::Normal(_)))
            {
                return Err(Failure::Input);
            }
            total = total.checked_add(bytes.len()).ok_or(Failure::Limit)?;
            if bytes.len() > MAX_FILE as usize || total > MAX_TOTAL {
                return Err(Failure::Limit);
            }
        }
        for bytes in files.values() {
            check()?;
            reject_recognized_secret(bytes)?;
        }
        Ok(())
    }
    pub(super) fn validate_git_files(
        files: &BTreeMap<PathBuf, Vec<u8>>,
        check: &dyn Fn() -> Result<(), Failure>,
    ) -> Result<(), Failure> {
        Self::validate_files(files, check)?;
        if files.keys().any(|path| {
            path.components().any(|part| {
                part.as_os_str()
                    .to_str()
                    .is_none_or(|name| name.to_uppercase().to_lowercase() == ".gitattributes")
            })
        }) {
            // Project attributes can change diff/encoding semantics. Ordinary
            // read-only copies remain available; Git copies never interpret them.
            return Err(Failure::Input);
        }
        Ok(())
    }
    fn prepare_checked(
        files: BTreeMap<PathBuf, Vec<u8>>,
        git_metadata: bool,
        check: &dyn Fn() -> Result<(), Failure>,
    ) -> Result<Self, Failure> {
        if git_metadata {
            Self::validate_git_files(&files, check)?;
        } else {
            Self::validate_files(&files, check)?;
        }
        let mut builder = tempfile::Builder::new();
        builder.prefix("zephium-work-copy-");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // tempfile directories otherwise inherit the process umask. This
            // root also contains Git-generated metadata and must never be public.
            builder.permissions(fs::Permissions::from_mode(0o700));
        }
        #[cfg(target_os = "macos")]
        let directory = builder.tempdir_in("/private/tmp");
        #[cfg(not(target_os = "macos"))]
        let directory = builder.tempdir();
        let directory = directory.map_err(|_| Failure::Storage)?;
        let private = match directory.path().canonicalize() {
            Ok(path) => path,
            Err(_) => {
                directory.close().map_err(|_| Failure::CleanupPending)?;
                return Err(Failure::Storage);
            }
        };
        #[cfg(windows)]
        if let Err(error) = super::windows::protect_root(&private) {
            directory.close().map_err(|_| Failure::CleanupPending)?;
            return Err(error);
        }
        let result = Self {
            _directory: directory,
            work: private.join("work"),
            home: private.join("home"),
            temporary: private.join("tmp"),
            empty: private.join("empty"),
            root: private,
            files,
        };
        let prepared = (|| -> Result<(), Failure> {
            for directory in [&result.work, &result.home, &result.temporary, &result.empty] {
                check()?;
                fs::create_dir(directory).map_err(|_| Failure::Storage)?;
            }
            if git_metadata {
                check()?;
                fs::create_dir(result.work.join(".git")).map_err(|_| Failure::Storage)?;
                fs::create_dir(result.work.join(".git/info")).map_err(|_| Failure::Storage)?;
                // Git's highest-priority attributes are broker-owned: copied attribute
                // files are data and cannot transform, hide, or disable checks of edits.
                fs::write(result.work.join(".git/info/attributes"),
            b"* -filter -ident -text -eol -working-tree-encoding diff whitespace=blank-at-eol,blank-at-eof,space-before-tab\n")
            .map_err(|_| Failure::Storage)?;
            }
            for (path, bytes) in &result.files {
                check()?;
                // Create-only catches case/normalization aliases that collide on
                // the destination filesystem instead of silently merging files.
                use std::io::Write;
                let destination = result.work.join(path);
                fs::create_dir_all(destination.parent().ok_or(Failure::Input)?)
                    .map_err(|_| Failure::Storage)?;
                let mut options = fs::OpenOptions::new();
                options.write(true).create_new(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(0o600);
                }
                options
                    .open(destination)
                    .map_err(|_| Failure::Input)?
                    .write_all(bytes)
                    .map_err(|_| Failure::Storage)?;
            }
            check()?;
            Ok(())
        })();
        if let Err(error) = prepared {
            result.close()?;
            return Err(error);
        }
        Ok(result)
    }

    pub(super) fn close(self) -> Result<(), Failure> {
        self._directory.close().map_err(|_| Failure::CleanupPending)
    }

    #[cfg(test)]
    pub(super) fn fixture(files: &[(&str, &[u8])]) -> Self {
        Self::from_files(
            files
                .iter()
                .map(|(name, bytes)| (PathBuf::from(name), bytes.to_vec()))
                .collect(),
        )
        .unwrap()
    }

    #[cfg(any(test, not(target_os = "macos")))]
    pub fn write(&self, relative: &Path, bytes: &[u8]) -> Result<(), Failure> {
        self.write_mode(relative, bytes, false)
    }
    #[cfg(target_os = "macos")]
    pub(super) fn write_new(&self, relative: &Path, bytes: &[u8]) -> Result<(), Failure> {
        self.write_mode(relative, bytes, true)
    }
    fn write_mode(&self, relative: &Path, bytes: &[u8], exclusive: bool) -> Result<(), Failure> {
        if relative.is_absolute()
            || protected_path(relative)
            || !relative
                .components()
                .all(|part| matches!(part, Component::Normal(_)))
            || bytes.len() > MAX_FILE as usize
        {
            return Err(Failure::Input);
        }
        reject_recognized_secret(bytes)?;
        let path = self.work.join(relative);
        fs::create_dir_all(path.parent().ok_or(Failure::Input)?).map_err(|_| Failure::Storage)?;
        #[cfg(unix)]
        {
            use std::{
                io::Write,
                os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
            };
            // Creation and replacement both keep source contents private, even
            // if an earlier copied entry carried broader mode bits.
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create(!exclusive)
                .create_new(exclusive)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
                .open(path)
                .map_err(|_| Failure::Storage)?;
            let metadata = file.metadata().map_err(|_| Failure::Storage)?;
            if !metadata.is_file() || metadata.nlink() != 1 {
                return Err(Failure::Storage);
            }
            file.set_permissions(fs::Permissions::from_mode(0o600))
                .map_err(|_| Failure::Storage)?;
            file.set_len(0).map_err(|_| Failure::Storage)?;
            file.write_all(bytes).map_err(|_| Failure::Storage)
        }
        #[cfg(not(unix))]
        {
            if exclusive {
                use std::io::Write;
                fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(path)
                    .and_then(|mut file| file.write_all(bytes))
                    .map_err(|_| Failure::Storage)
            } else {
                fs::write(path, bytes).map_err(|_| Failure::Storage)
            }
        }
    }
}

#[cfg(test)]
fn collect(root: &Path, selected: &[PathBuf]) -> Result<BTreeMap<PathBuf, Vec<u8>>, Failure> {
    collect_checked(root, selected, &|| Ok(()))
}
pub(super) fn collect_checked(
    root: &Path,
    selected: &[PathBuf],
    check: &dyn Fn() -> Result<(), Failure>,
) -> Result<BTreeMap<PathBuf, Vec<u8>>, Failure> {
    if selected.len() > MAX_FILES {
        return Err(Failure::Limit);
    }
    let mut files = BTreeMap::new();
    let mut total = 0usize;
    for relative in selected {
        check()?;
        if relative.is_absolute()
            || protected_path(relative)
            || relative.as_os_str().is_empty()
            || !relative
                .components()
                .all(|part| matches!(part, Component::Normal(_)))
        {
            return Err(Failure::Input);
        }
        if relative.components().count() > MAX_DEPTH {
            return Err(Failure::Limit);
        }
        let bytes = snapshot_bytes(&root.join(relative), MAX_FILE).map_err(|_| Failure::Input)?;
        check()?;
        total = total.checked_add(bytes.len()).ok_or(Failure::Limit)?;
        if total > MAX_TOTAL {
            return Err(Failure::Limit);
        }
        if files.insert(relative.to_owned(), bytes).is_some() {
            return Err(Failure::Input);
        }
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn git_copies_refuse_attributes_without_restricting_plain_read_only_copies() {
        let files: BTreeMap<_, _> = [(PathBuf::from(".gitattributes"), b"* -diff\n".to_vec())]
            .into_iter()
            .collect();
        assert!(matches!(
            Snapshot::from_files(files.clone()),
            Err(Failure::Input)
        ));
        let plain = Snapshot::read_only(files).unwrap();
        assert_eq!(
            fs::read(plain.work.join(".gitattributes")).unwrap(),
            b"* -diff\n"
        );
        plain.close().unwrap();
    }
    #[cfg(unix)]
    fn assert_private_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let copied = Snapshot::fixture(&[("existing.txt", b"inert before")]);
        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&copied.root), 0o700);
        assert_eq!(mode(&copied.work.join("existing.txt")), 0o600);
        // Match a newly approved file removed while constructing its baseline.
        fs::remove_file(copied.work.join("existing.txt")).unwrap();
        copied
            .write(Path::new("existing.txt"), b"inert recreated")
            .unwrap();
        assert_eq!(mode(&copied.work.join("existing.txt")), 0o600);
        copied.write(Path::new("new.txt"), b"inert new").unwrap();
        assert_eq!(mode(&copied.work.join("new.txt")), 0o600);
        fs::set_permissions(
            copied.work.join("new.txt"),
            fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        copied
            .write(Path::new("new.txt"), b"inert rewritten")
            .unwrap();
        assert_eq!(mode(&copied.work.join("new.txt")), 0o600);
        assert_eq!(
            fs::read(copied.work.join("new.txt")).unwrap(),
            b"inert rewritten"
        );
    }
    #[cfg(unix)]
    #[test]
    fn copied_root_and_created_or_rewritten_files_are_private() {
        assert_private_permissions();
    }

    #[cfg(unix)]
    #[test]
    fn permissive_umask_cannot_publish_snapshot_contents() {
        // The mask changes only in this dedicated child, never the shared test
        // process. No shell or model-supplied program is involved.
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "work_commands::runner::snapshot::tests::permission_fixture_child",
                "--ignored",
                "--test-threads=1",
            ])
            .env_clear()
            .env("ZEPHIUM_SNAPSHOT_PERMISSION_CHILD", "1")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success());
    }
    #[cfg(unix)]
    #[test]
    #[ignore = "Only the isolated permissive-umask fixture subprocess changes its process mask"]
    fn permission_fixture_child() {
        if std::env::var("ZEPHIUM_SNAPSHOT_PERMISSION_CHILD").as_deref() != Ok("1") {
            return;
        }
        unsafe {
            libc::umask(0);
        }
        assert_private_permissions();
    }

    #[test]
    fn protected_files_aliases_and_links_are_not_staged() {
        let scratch = crate::work_files::test_tempdir();
        let root = scratch.path().canonicalize().unwrap();
        fs::create_dir(root.join(".git")).unwrap();
        fs::write(root.join(".git/config"), "inert hidden config").unwrap();
        for path in [
            ".env",
            "credentialſ.json",
            "secrets",
            "private.key",
            ".gitconfig",
            ".git-credentials",
        ] {
            fs::write(root.join(path), "inert secret").unwrap();
        }
        fs::write(root.join("ordinary.rs"), "fn main() {}\n").unwrap();
        fs::hard_link(root.join(".env"), root.join("innocent.txt")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(root.join("ordinary.rs"), root.join("alias.rs")).unwrap();
        let copied = collect(&root, &["ordinary.rs".into()]).unwrap();
        assert_eq!(copied.len(), 1);
        assert_eq!(copied[Path::new("ordinary.rs")], b"fn main() {}\n");
        for relative in [
            ".git/config",
            ".env",
            "credentialſ.json",
            "secrets",
            "private.key",
            ".gitconfig",
            ".git-credentials",
            "innocent.txt",
            "../outside",
        ] {
            assert!(
                matches!(collect(&root, &[relative.into()]), Err(Failure::Input)),
                "{relative}"
            );
        }
        #[cfg(unix)]
        assert!(matches!(
            collect(&root, &["alias.rs".into()]),
            Err(Failure::Input)
        ));
    }
}

fn reject_recognized_secret(bytes: &[u8]) -> Result<(), Failure> {
    if std::str::from_utf8(bytes).is_ok_and(zephium_agentic::contains_recognized_secret) {
        return Err(Failure::Sensitive);
    }
    Ok(())
}

#[cfg(test)]
mod disclosure_tests {
    use super::*;
    #[test]
    fn secret_baseline_and_overlay_never_enter_a_snapshot() {
        let secret = b"-----BEGIN PRIVATE KEY-----";
        assert!(matches!(
            Snapshot::from_files([(PathBuf::from("ordinary.txt"), secret.to_vec())].into()),
            Err(Failure::Sensitive)
        ));
        let snapshot = Snapshot::fixture(&[("ordinary.txt", b"ordinary")]);
        assert_eq!(
            snapshot.write(Path::new("ordinary.txt"), secret),
            Err(Failure::Sensitive)
        );
        assert_eq!(
            fs::read(snapshot.work.join("ordinary.txt")).unwrap(),
            b"ordinary"
        );
    }
}
