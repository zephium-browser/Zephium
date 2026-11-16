//! Selected stdlib unittest modules in a read-only text snapshot. No shell,
//! dependency installation, arbitrary interpreter options or project startup hook.
#[cfg(any(windows, all(test, target_os = "macos")))]
use super::Snapshot;
use super::{Failure, Output};
use crate::work_files::{protected_path, read_text_limited, WorkFileGrant};
#[cfg(any(target_os = "macos", windows))]
use std::sync::atomic::Ordering;
use std::{
    collections::BTreeSet,
    future::Future,
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::Duration,
};

#[cfg(any(target_os = "macos", windows))]
pub(super) const BOOTSTRAP: &str = include_str!("python_bootstrap.py");
#[derive(Clone)]
pub(crate) struct UnittestRequest {
    modules: Vec<String>,
    selected: Vec<PathBuf>,
}
impl UnittestRequest {
    pub fn new(tests: &[String], supporting: &[String]) -> Result<Self, Failure> {
        if tests.is_empty() || tests.len() > 64 || tests.len() + supporting.len() > 2_000 {
            return Err(Failure::Input);
        }
        let mut modules = Vec::new();
        let mut selected = BTreeSet::new();
        for test in tests {
            let path = relative(test)?;
            let stem = test.strip_suffix(".py").ok_or(Failure::Input)?;
            if !stem.split('/').all(identifier) {
                return Err(Failure::Input);
            }
            let module = stem
                .strip_suffix("/__init__")
                .unwrap_or(stem)
                .replace('/', ".");
            if modules.contains(&module) {
                return Err(Failure::Input);
            }
            modules.push(module);
            selected.insert(path);
        }
        for path in supporting {
            selected.insert(relative(path)?);
        }
        Ok(Self {
            modules,
            selected: selected.into_iter().collect(),
        })
    }
    pub fn count(&self) -> usize {
        self.modules.len()
    }
}
fn identifier(part: &str) -> bool {
    let mut bytes = part.bytes();
    bytes
        .next()
        .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}
fn relative(value: &str) -> Result<PathBuf, Failure> {
    let path = Path::new(value);
    if value.is_empty()
        || value.len() > 1024
        || value.contains('\\')
        || path.is_absolute()
        || protected_path(path)
        || path.components().count() > 32
        || !path
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
    {
        return Err(Failure::Input);
    }
    Ok(path.to_owned())
}
pub(crate) fn ensure_available() -> Result<(), Failure> {
    #[cfg(target_os = "macos")]
    if super::macos::qualified() && super::guard::installed() {
        return Ok(());
    }
    // Windows requires packaged runtime hashes and native LPAC/Job qualification.
    Err(Failure::PythonUnavailable)
}
fn slots() -> &'static Arc<super::worker_slots::Slots> {
    static SLOTS: std::sync::OnceLock<Arc<super::worker_slots::Slots>> = std::sync::OnceLock::new();
    SLOTS.get_or_init(|| Arc::new(super::worker_slots::Slots::new(4)))
}
pub(crate) async fn run<C, CF>(
    grant: &WorkFileGrant,
    root: &Path,
    request: &UnittestRequest,
    cancelled: C,
) -> Result<Output, Failure>
where
    C: Fn() -> CF,
    CF: Future<Output = bool>,
{
    ensure_available()?;
    let (grant, root, request) = (grant.clone(), root.to_owned(), request.clone());
    super::worker::run(
        slots(),
        &cancelled,
        super::worker::Limits::standard(Duration::from_secs(40)),
        move |control| async move {
            control.check()?;
            if grant
                .resolve(root.to_str().ok_or(Failure::Input)?, false)
                .map_err(|_| Failure::Input)?
                != root
            {
                return Err(Failure::Input);
            }
            let mut files = std::collections::BTreeMap::new();
            let mut total = 0usize;
            for path in &request.selected {
                control.check()?;
                let (_, _, text) =
                    read_text_limited(&root.join(path), 1024 * 1024).map_err(|error| {
                        if error == crate::work_files::WorkFileError::Sensitive {
                            Failure::Sensitive
                        } else {
                            Failure::Input
                        }
                    })?;
                let text = text.ok_or(Failure::Input)?;
                total = total.checked_add(text.len()).ok_or(Failure::Limit)?;
                if total > 32 * 1024 * 1024 {
                    return Err(Failure::Limit);
                }
                files.insert(path.clone(), text.into_bytes());
            }
            control.check()?;
            control.native();
            #[cfg(any(target_os = "macos", windows))]
            let cancelled = || async { control.cancelled.load(Ordering::Acquire) };
            #[cfg(target_os = "macos")]
            let output = super::guard::run(&files, &request.modules, &cancelled).await?;
            #[cfg(windows)]
            let output = {
                control.filesystem();
                let snapshot = Snapshot::read_only_checked(files.clone(), &|| control.check())?;
                control.native();
                let result = super::windows::python(&snapshot, &request.modules, &cancelled).await;
                control.filesystem();
                super::worker::closed(result, snapshot.close())?
            };
            #[cfg(not(any(target_os = "macos", windows)))]
            return Err(Failure::PythonUnavailable);
            #[cfg(any(target_os = "macos", windows))]
            {
                control.filesystem();
                for (path, bytes) in &files {
                    control.check()?;
                    if crate::work_files::snapshot_bytes(&root.join(path), 1024 * 1024)
                        .ok()
                        .as_ref()
                        != Some(bytes)
                    {
                        return Err(Failure::Changed);
                    }
                }
                control.check()?;
                Ok(output)
            }
        },
    )
    .await
}

#[cfg(test)]
mod tests;
