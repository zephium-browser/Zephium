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

pub(crate) fn instance_identifier(identifier: &str, root: Option<&Path>) -> String {
    instance_identifier_for_data_dir(identifier, root, dirs::data_dir())
}

fn instance_identifier_for_data_dir(
    identifier: &str,
    root: Option<&Path>,
    data_dir: Option<PathBuf>,
) -> String {
    let Some(root) = root else {
        return identifier.to_owned();
    };
    if is_default_data_root(identifier, root, data_dir) {
        return identifier.to_owned();
    }
    use sha2::Digest;
    let digest = sha2::Sha256::digest(root.as_os_str().as_encoded_bytes());
    format!("{identifier}.userdata.{digest:x}")
}

fn is_default_data_root(identifier: &str, root: &Path, data_dir: Option<PathBuf>) -> bool {
    data_dir
        .map(|path| path.join(identifier))
        .and_then(|path| path.canonicalize().ok())
        .and_then(|path| zephium_core::webview2::resolve_data_root(&path).ok())
        .as_deref()
        == Some(root)
}

pub(crate) fn resolve(path: &Path) -> std::io::Result<PathBuf> {
    let path = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    let path = normalize_absolute_path(&path)?;
    #[cfg(target_os = "windows")]
    {
        let path =
            zephium_private_fs::prepare_user_data_root(&path).map_err(std::io::Error::other)?;
        zephium_core::webview2::resolve_data_root(&path)
    }
    #[cfg(not(target_os = "windows"))]
    {
        #[cfg(unix)]
        let canonical = prepare_unix_root(&path)?;
        #[cfg(not(unix))]
        {
            std::fs::create_dir_all(&path)?;
            let metadata = std::fs::symlink_metadata(&path)?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "user data directory must be a real directory",
                ));
            }
        }
        #[cfg(not(unix))]
        let canonical = path.canonicalize()?;
        zephium_core::webview2::resolve_data_root(&canonical)
    }
}

fn normalize_absolute_path(path: &Path) -> std::io::Result<PathBuf> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            component => {
                normalized.push(component.as_os_str());
                match normalized.canonicalize() {
                    Ok(canonical) => normalized = canonical,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                }
            }
        }
    }
    Ok(normalized)
}

#[cfg(unix)]
fn prepare_unix_root(path: &Path) -> std::io::Result<PathBuf> {
    use rustix::fs::{Mode, OFlags};
    use std::os::unix::fs::MetadataExt;

    if std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "user data directory must be a real directory",
        ));
    }
    let canonical = canonicalize_with_missing_suffix(path)?;
    let mut directory = std::fs::File::from(
        rustix::fs::open(
            "/",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )
        .map_err(std::io::Error::from)?,
    );
    let components = canonical
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(name) => Some(name.to_owned()),
            _ => None,
        })
        .collect::<Vec<_>>();
    for (index, name) in components.iter().enumerate() {
        let child = match rustix::fs::openat(
            &directory,
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        ) {
            Ok(child) => std::fs::File::from(child),
            Err(error) if error == rustix::io::Errno::NOENT => {
                let created =
                    match rustix::fs::mkdirat(&directory, name, Mode::from_raw_mode(0o700)) {
                        Ok(()) => true,
                        Err(rustix::io::Errno::EXIST) => false,
                        Err(error) => return Err(std::io::Error::from(error)),
                    };
                let child = rustix::fs::openat(
                    &directory,
                    name,
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
                    Mode::empty(),
                )
                .map_err(std::io::Error::from)?;
                let child = std::fs::File::from(child);
                if created {
                    rustix::fs::fchmod(&child, Mode::from_raw_mode(0o700))
                        .map_err(std::io::Error::from)?;
                }
                child
            }
            Err(error) => return Err(std::io::Error::from(error)),
        };
        let metadata = child.metadata()?;
        if index + 1 == components.len() {
            validate_unix_root(&metadata)?;
        } else {
            validate_unix_directory_ancestor(&metadata)?;
        }
        directory = child;
    }
    let metadata = directory.metadata()?;
    if metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o077 != 0
        || metadata.mode() & 0o700 != 0o700
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "existing user data directory must be owned by this user and private",
        ));
    }
    Ok(canonical)
}

#[cfg(unix)]
fn canonicalize_with_missing_suffix(path: &Path) -> std::io::Result<PathBuf> {
    let mut existing = path;
    let mut suffix = Vec::new();
    loop {
        match existing.canonicalize() {
            Ok(mut canonical) => {
                for component in suffix.into_iter().rev() {
                    canonical.push(component);
                }
                return Ok(canonical);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let missing_component = || {
                    std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        "user data directory has no existing ancestor",
                    )
                };
                let name = existing.file_name().ok_or_else(missing_component)?;
                suffix.push(name.to_owned());
                existing = existing.parent().ok_or_else(missing_component)?;
            }
            Err(error) => return Err(error),
        }
    }
}

#[cfg(unix)]
fn validate_unix_root(metadata: &std::fs::Metadata) -> std::io::Result<()> {
    use std::os::unix::fs::MetadataExt;

    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "user data directory must be a real directory",
        ));
    }
    if metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o077 != 0
        || metadata.mode() & 0o700 != 0o700
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "existing user data directory must be owned by this user and private",
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn validate_unix_ancestors(path: &Path) -> std::io::Result<()> {
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
        validate_unix_directory_ancestor(&metadata).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!(
                    "user data directory has an unsafe ancestor: {}",
                    ancestor.display()
                ),
            )
        })?;
    }
    Ok(())
}

#[cfg(unix)]
fn validate_unix_directory_ancestor(metadata: &std::fs::Metadata) -> std::io::Result<()> {
    use std::os::unix::fs::MetadataExt;

    let euid = rustix::process::geteuid().as_raw();
    let uid = metadata.uid();
    if metadata.file_type().is_symlink() {
        if uid == euid || uid == 0 {
            return Ok(());
        }
    } else if metadata.is_dir()
        && (uid == euid || uid == 0)
        && (metadata.mode() & 0o022 == 0 || metadata.mode() & 0o1000 != 0)
    {
        return Ok(());
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::PermissionDenied,
        "user data directory has an unsafe ancestor",
    ))
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

pub(crate) fn prepare_logs(root: &Path) -> std::io::Result<PathBuf> {
    let logs = root.join("logs");
    std::fs::create_dir_all(&logs)?;
    Ok(logs)
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
        assert_eq!(instance_identifier("app.zephium", None), "app.zephium");
        let first = instance_identifier("app.zephium", Some(Path::new("/tmp/one")));
        assert_eq!(
            first,
            instance_identifier("app.zephium", Some(Path::new("/tmp/one")))
        );
        assert_ne!(
            first,
            instance_identifier("app.zephium", Some(Path::new("/tmp/two")))
        );
    }

    #[test]
    fn explicit_default_data_root_keeps_the_default_instance_identity() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("app.zephium");
        std::fs::create_dir_all(&root).unwrap();
        let root = zephium_core::webview2::resolve_data_root(&root).unwrap();
        assert!(is_default_data_root(
            "app.zephium",
            &root,
            Some(temp.path().to_owned())
        ));
        assert_eq!(
            instance_identifier_for_data_dir(
                "app.zephium",
                Some(&root),
                Some(temp.path().to_owned())
            ),
            "app.zephium"
        );
    }

    #[test]
    fn log_directory_creation_failure_is_returned() {
        let temp = tempfile::tempdir().unwrap();
        let logs = temp.path().join("logs");
        std::fs::write(&logs, b"file where logs directory belongs").unwrap();

        assert_eq!(
            prepare_logs(temp.path()).unwrap_err().kind(),
            std::io::ErrorKind::AlreadyExists
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
    fn parent_relative_components_resolve_inside_the_selected_directory() {
        let temp = tempfile::tempdir_in(".").unwrap();
        let relative = PathBuf::from(".")
            .join(temp.path().file_name().unwrap())
            .join("unused")
            .join("..")
            .join("browser-state");

        let resolved = resolve(&relative).unwrap();
        assert_eq!(
            resolved,
            temp.path().join("browser-state").canonicalize().unwrap()
        );
    }

    #[cfg(unix)]
    #[test]
    fn resolves_symlink_prefix_before_parent_components() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let target_parent = temp.path().join("actual");
        let target = target_parent.join("nested");
        std::fs::create_dir_all(&target).unwrap();
        let alias = temp.path().join("alias");
        symlink(&target, &alias).unwrap();

        let resolved = resolve(&alias.join("..").join("browser-state")).unwrap();

        assert_eq!(
            resolved,
            target_parent.join("browser-state").canonicalize().unwrap()
        );
        assert!(!temp.path().join("browser-state").exists());
    }

    #[cfg(unix)]
    #[test]
    fn creates_new_root_and_missing_parents_with_private_permissions() {
        use std::os::unix::fs::MetadataExt;

        let temp = tempfile::tempdir().unwrap();
        let root = resolve(&temp.path().join("nested/parent/private-root")).unwrap();
        for path in [
            root.as_path(),
            root.parent().unwrap(),
            root.parent().unwrap().parent().unwrap(),
        ] {
            let metadata = std::fs::symlink_metadata(path).unwrap();
            assert_eq!(metadata.uid(), rustix::process::geteuid().as_raw());
            assert_eq!(metadata.mode() & 0o777, 0o700);
        }
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
    fn rejects_symlink_ancestor_into_shared_writable_directory_before_creating_root() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let temp = tempfile::tempdir().unwrap();
        let shared = temp.path().join("shared");
        std::fs::create_dir(&shared).unwrap();
        std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o777)).unwrap();
        let alias = temp.path().join("alias");
        symlink(&shared, &alias).unwrap();
        let requested = alias.join("new-root");

        assert_eq!(
            resolve(&requested).unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied
        );
        assert!(!shared.join("new-root").exists());
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
