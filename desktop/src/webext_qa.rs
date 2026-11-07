//! Named disposable sessions in an already isolated QA data root (extensions
//! and protection QA). No product flag.
use std::path::PathBuf;

/// `flag` is the whole `--name=` prefix of the QA application's own argument.
pub(super) fn session_label(flag: &str) -> std::io::Result<Option<String>> {
    let labels: Vec<_> = std::env::args()
        .filter_map(|arg| arg.strip_prefix(flag).map(str::to_owned))
        .collect();
    match labels.as_slice() {
        [] => Ok(None),
        [label] if valid_label(label) => Ok(Some(label.clone())),
        _ => Err(std::io::Error::other(
            "Use one alphanumeric QA session label (1–48 characters).",
        )),
    }
}

pub(super) fn data_dir(root: PathBuf, label: Option<&str>) -> std::io::Result<PathBuf> {
    match label {
        Some(label) if valid_label(label) => session_dir(root, label),
        None => Ok(root),
        Some(_) => Err(std::io::Error::other("Invalid QA session label.")),
    }
}

fn session_dir(root: PathBuf, label: &str) -> std::io::Result<PathBuf> {
    let selected = root.join("qa-sessions").join(format!("session-{label}"));
    std::fs::create_dir_all(&root)?;
    let root = std::fs::canonicalize(root)?;
    let sessions = root.join("qa-sessions");
    std::fs::create_dir_all(&sessions)?;
    let sessions = std::fs::canonicalize(sessions)?;
    if !sessions.starts_with(&root) {
        return Err(std::io::Error::other(
            "QA sessions escaped the isolated root.",
        ));
    }
    let target = sessions.join(format!("session-{label}"));
    std::fs::create_dir_all(&target)?;
    let target = std::fs::canonicalize(target)?;
    if !target.starts_with(&sessions) {
        return Err(std::io::Error::other(
            "QA session escaped the isolated root.",
        ));
    }
    // Canonical paths above are only for containment checks. Preserve Tauri's
    // ordinary Win32 spelling for consumers: WebView2 rejects some extensions
    // when AddBrowserExtension receives Rust's canonical \\?\ path spelling
    // (qualified with identical Grammarly bytes, same directory, native lab).
    Ok(selected)
}

fn valid_label(label: &str) -> bool {
    !label.is_empty()
        && label.len() <= 48
        && label
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

#[cfg(test)]
mod tests {
    use super::{session_dir, valid_label};
    #[test]
    fn containment_checks_do_not_replace_the_native_path_spelling() {
        let root = tempfile::tempdir().unwrap();
        let selected = session_dir(root.path().to_path_buf(), "ab").unwrap();
        assert_eq!(selected, root.path().join("qa-sessions/session-ab"));
        assert!(selected.is_dir());
        assert!(selected
            .canonicalize()
            .unwrap()
            .starts_with(root.path().canonicalize().unwrap()));
    }
    #[test]
    fn session_labels_cannot_select_a_path_or_stream() {
        assert!(valid_label("memory-review-20260930"));
        for value in [
            "",
            "..",
            "../real-profile",
            "C:\\profile",
            "a:b",
            "a/b",
            "a\\b",
        ] {
            assert!(!valid_label(value));
        }
        assert!(!valid_label(&"a".repeat(49)));
    }
}
