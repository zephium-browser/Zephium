use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Manager};

static DATA_ROOT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
static CUSTOM_ROOT: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ArgumentError {
    MissingPath,
    Duplicate,
    EmptyPath,
}

impl std::fmt::Display for ArgumentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingPath => f.write_str("--user-data-dir requires a path"),
            Self::Duplicate => f.write_str("--user-data-dir may be supplied only once"),
            Self::EmptyPath => f.write_str("--user-data-dir path must not be empty"),
        }
    }
}

pub(crate) fn parse_argument<I, S>(arguments: I) -> Result<Option<PathBuf>, ArgumentError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let mut args = arguments.into_iter();
    let mut found = None;
    while let Some(argument) = args.next() {
        if argument.as_ref() != "--user-data-dir" {
            continue;
        }
        if found.is_some() {
            return Err(ArgumentError::Duplicate);
        }
        let Some(path) = args.next() else {
            return Err(ArgumentError::MissingPath);
        };
        if path.as_ref().to_string_lossy().starts_with("--") {
            return Err(ArgumentError::MissingPath);
        }
        let path = PathBuf::from(path.as_ref());
        if path.as_os_str().is_empty() {
            return Err(ArgumentError::EmptyPath);
        }
        found = Some(path);
    }
    Ok(found)
}

pub(crate) fn external_arguments<I, S>(arguments: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let mut args = arguments.into_iter();
    let mut external = Vec::new();
    while let Some(argument) = args.next() {
        if argument.as_ref() == "--user-data-dir" {
            let _ = args.next();
        } else {
            external.push(argument.as_ref().to_string_lossy().into_owned());
        }
    }
    external
}

pub(crate) fn resolve(path: &Path) -> std::io::Result<PathBuf> {
    let path = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    if path.components().any(|component| {
        matches!(
            component,
            std::path::Component::CurDir | std::path::Component::ParentDir
        )
    }) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "user data directory contains relative traversal components",
        ));
    }
    #[cfg(target_os = "windows")]
    {
        let path =
            zephium_private_fs::prepare_user_data_root(&path).map_err(std::io::Error::other)?;
        zephium_core::webview2::resolve_data_root(&path)
    }
    #[cfg(not(target_os = "windows"))]
    {
        #[cfg(unix)]
        validate_unix_ancestors(&path)?;
        #[cfg(unix)]
        let existing = std::fs::symlink_metadata(&path).ok();
        std::fs::create_dir_all(&path)?;
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "user data directory must be a real directory",
            ));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            if existing.is_none() {
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
            }
            let metadata = std::fs::symlink_metadata(&path)?;
            if metadata.uid() != rustix::process::geteuid().as_raw() || metadata.mode() & 0o077 != 0
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "existing user data directory must be owned by this user and private",
                ));
            }
            validate_unix_ancestors(&path)?;
        }
        let canonical = path.canonicalize()?;
        #[cfg(unix)]
        validate_unix_ancestors(&canonical)?;
        zephium_core::webview2::resolve_data_root(&canonical)
    }
}

#[cfg(unix)]
fn validate_unix_ancestors(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::MetadataExt;

    let euid = rustix::process::geteuid().as_raw();
    for ancestor in path
        .ancestors()
        .skip(1)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
    {
        let metadata = match std::fs::symlink_metadata(ancestor) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        let mode = metadata.mode();
        let uid = metadata.uid();
        if metadata.file_type().is_symlink() {
            if uid != euid && uid != 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    format!(
                        "user data directory has an unsafe ancestor: {}",
                        ancestor.display()
                    ),
                ));
            }
            continue;
        }
        if !metadata.is_dir()
            || (uid != euid && uid != 0)
            || (mode & 0o022 != 0 && (mode & 0o1000 == 0 || (uid != euid && uid != 0)))
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!(
                    "user data directory has an unsafe ancestor: {}",
                    ancestor.display()
                ),
            ));
        }
    }
    Ok(())
}

pub(crate) fn install(path: PathBuf) -> std::io::Result<()> {
    DATA_ROOT.set(path).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "data root already resolved",
        )
    })
}

pub(crate) fn initialize_override(path: &Path) -> std::io::Result<PathBuf> {
    let path = resolve(path)?;
    install(path.clone())?;
    CUSTOM_ROOT.store(true, Ordering::Release);
    Ok(path)
}

pub(crate) fn is_installed() -> bool {
    DATA_ROOT.get().is_some()
}

pub(crate) fn root(app: &AppHandle) -> std::io::Result<PathBuf> {
    DATA_ROOT
        .get()
        .cloned()
        .map(Ok)
        .unwrap_or_else(|| app.path().app_data_dir().map_err(std::io::Error::other))
}

pub(crate) fn cache(app: &AppHandle) -> std::io::Result<PathBuf> {
    if CUSTOM_ROOT.load(Ordering::Acquire) {
        let root = DATA_ROOT
            .get()
            .ok_or_else(|| std::io::Error::other("custom data root is unavailable"))?;
        Ok(root.join("cache"))
    } else {
        app.path().app_cache_dir().map_err(std::io::Error::other)
    }
}

pub(crate) fn logs(app: &AppHandle) -> std::io::Result<PathBuf> {
    if CUSTOM_ROOT.load(Ordering::Acquire) {
        let root = DATA_ROOT
            .get()
            .ok_or_else(|| std::io::Error::other("custom data root is unavailable"))?;
        Ok(root.join("logs"))
    } else {
        app.path().app_log_dir().map_err(std::io::Error::other)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_optional_directory_and_rejects_malformed_arguments() {
        assert_eq!(parse_argument(["--other", "x"]).unwrap(), None);
        assert_eq!(
            parse_argument(["--user-data-dir", "state"]).unwrap(),
            Some("state".into())
        );
        assert_eq!(
            parse_argument(["--user-data-dir"]).unwrap_err(),
            ArgumentError::MissingPath
        );
        assert_eq!(
            parse_argument(["--user-data-dir", "--other"]).unwrap_err(),
            ArgumentError::MissingPath
        );
        assert_eq!(
            parse_argument(["--user-data-dir", ""]).unwrap_err(),
            ArgumentError::EmptyPath
        );
        assert_eq!(
            parse_argument(["--user-data-dir", "a", "--user-data-dir", "b"]).unwrap_err(),
            ArgumentError::Duplicate
        );
        assert_eq!(
            external_arguments([
                "https://example.com",
                "--user-data-dir",
                "https://other.test"
            ]),
            vec!["https://example.com".to_owned()]
        );
    }

    #[test]
    fn custom_roots_persist_and_are_isolated() {
        let temp = tempfile::tempdir().unwrap();
        let first = resolve(&temp.path().join("one")).unwrap();
        let second = resolve(&temp.path().join("two")).unwrap();
        std::fs::write(first.join("settings.json"), b"one").unwrap();
        assert_eq!(std::fs::read(first.join("settings.json")).unwrap(), b"one");
        assert!(!second.join("settings.json").exists());
    }

    #[test]
    fn rejects_a_file_as_custom_root() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("file");
        std::fs::write(&file, b"not a directory").unwrap();
        assert!(resolve(&file).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_group_writable_non_sticky_ancestor_without_changing_it() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let temp = tempfile::tempdir().unwrap();
        let ancestor = temp.path();
        let original = std::fs::metadata(ancestor).unwrap().mode() & 0o7777;
        std::fs::set_permissions(ancestor, std::fs::Permissions::from_mode(0o770)).unwrap();
        let result = resolve(&ancestor.join("custom-root"));
        let after = std::fs::symlink_metadata(ancestor).unwrap();
        assert_eq!(after.mode() & 0o7777, 0o770);
        assert_eq!(after.uid(), rustix::process::geteuid().as_raw());
        assert_eq!(
            result.unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied
        );
        std::fs::set_permissions(ancestor, std::fs::Permissions::from_mode(original)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn allows_sticky_shared_ancestor_without_changing_its_mode() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let temp = tempfile::tempdir().unwrap();
        let ancestor = temp.path();
        let original = std::fs::metadata(ancestor).unwrap().mode() & 0o7777;
        std::fs::set_permissions(ancestor, std::fs::Permissions::from_mode(0o1777)).unwrap();
        let result = resolve(&ancestor.join("custom-root"));
        let after = std::fs::symlink_metadata(ancestor).unwrap();
        assert_eq!(after.mode() & 0o7777, 0o1777);
        assert_eq!(after.uid(), rustix::process::geteuid().as_raw());
        assert!(result.is_ok(), "{result:?}");
        std::fs::set_permissions(ancestor, std::fs::Permissions::from_mode(original)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn rejects_a_shared_existing_root_without_changing_its_permissions() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("shared");
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
        let before = std::fs::symlink_metadata(&root).unwrap().mode() & 0o7777;
        assert!(resolve(&root).is_err());
        let after = std::fs::symlink_metadata(root).unwrap().mode() & 0o7777;
        assert_eq!(before, 0o755);
        assert_eq!(after, before);
        assert_eq!(
            rustix::process::geteuid().as_raw(),
            temp.path().metadata().unwrap().uid()
        );
    }
}
