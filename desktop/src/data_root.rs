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
    std::fs::create_dir_all(&path)?;
    #[cfg(target_os = "windows")]
    {
        zephium_core::webview2::resolve_data_root(&path)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "user data directory must be a real directory",
            ));
        }
        let canonical = path.canonicalize()?;
        zephium_core::webview2::resolve_data_root(&canonical)
    }
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
}
