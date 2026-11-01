//! The vault: a folder the person owns (often inside OneDrive) that Zephium
//! browses and manages as a file explorer. Paths crossing the IPC boundary are
//! vault-relative with `/` separators; the empty string is the vault root.
use serde::{Deserialize, Serialize};
use specta::Type;

/// Largest file the frame may read through IPC or write back (PDF annotation).
pub const MAX_VAULT_READ_BYTES: u64 = 256 * 1024 * 1024;
pub const MAX_VAULT_WRITE_BYTES: usize = 128 * 1024 * 1024;
/// Most entries one folder listing returns.
pub const MAX_VAULT_LIST: usize = 5_000;
pub const MAX_VAULT_PATH_BYTES: usize = 1024;
pub const MAX_VAULT_NAME_BYTES: usize = 255;
const MAX_QUERY_BYTES: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum VaultEntryKind {
    Folder,
    File,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct VaultEntry {
    pub name: String,
    /// Vault-relative path with `/` separators.
    pub path: String,
    pub kind: VaultEntryKind,
    /// Lower-case extension without the dot; empty for folders.
    pub ext: String,
    /// Bytes, as a decimal string so large files survive JavaScript numbers.
    pub size: String,
    /// Milliseconds since the Unix epoch, as a decimal string.
    pub modified: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum VaultError {
    /// No vault folder is chosen for this profile.
    NoRoot,
    Invalid,
    NotFound,
    Exists,
    /// The file changed on disk since it was read.
    Changed,
    TooLarge,
    Unavailable,
    Io,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum VaultCall {
    List {
        dir: String,
    },
    Stat {
        path: String,
    },
    CreateFolder {
        dir: String,
        name: String,
    },
    CreateFile {
        dir: String,
        name: String,
        text: String,
    },
    Rename {
        path: String,
        name: String,
    },
    Move {
        path: String,
        dest: String,
    },
    /// Moves into the vault's `.trash` folder; never permanent. Answers the
    /// trashed entry (under `.trash/`) so the person can undo.
    Trash {
        path: String,
    },
    /// Copies a file from outside the vault (a download) into `dest`.
    Import {
        source: String,
        dest: String,
    },
    /// Base64 contents, bounded by `MAX_VAULT_READ_BYTES`.
    Read {
        path: String,
    },
    /// Replaces a file atomically if it still has `expected_modified`.
    Write {
        path: String,
        base64: String,
        expected_modified: String,
    },
    /// Fuzzy file and folder names.
    Search {
        query: String,
        limit: u16,
        folders_only: bool,
    },
    OpenNative {
        path: String,
    },
    Reveal {
        path: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum VaultResponse {
    Entries { entries: Vec<VaultEntry> },
    Entry { entry: VaultEntry },
    Bytes { base64: String, modified: String },
    Done,
    Error { error: VaultError },
}

fn valid_path(path: &str, allow_root: bool) -> bool {
    if path.is_empty() {
        return allow_root;
    }
    path.len() <= MAX_VAULT_PATH_BYTES && path.split('/').all(valid_name)
}

/// One path component a person may create on Windows, macOS and OneDrive.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_VAULT_NAME_BYTES
        && name != "."
        && name != ".."
        && !name.ends_with([' ', '.'])
        && !name.starts_with(' ')
        && !name.chars().any(|c| {
            matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control()
        })
        && !reserved_on_windows(name)
}

fn reserved_on_windows(name: &str) -> bool {
    let base = name
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((base.starts_with("COM") || base.starts_with("LPT"))
            && base.len() == 4
            && base.as_bytes()[3].is_ascii_digit())
}

impl VaultCall {
    pub fn validate(&self) -> bool {
        match self {
            Self::List { dir } => valid_path(dir, true),
            Self::Stat { path } | Self::Read { path } | Self::OpenNative { path } => {
                valid_path(path, false)
            }
            Self::Reveal { path } => valid_path(path, true),
            Self::Trash { path } => valid_path(path, false),
            Self::CreateFolder { dir, name } => valid_path(dir, true) && valid_name(name),
            Self::CreateFile { dir, name, text } => {
                valid_path(dir, true) && valid_name(name) && text.len() <= MAX_VAULT_WRITE_BYTES
            }
            Self::Rename { path, name } => valid_path(path, false) && valid_name(name),
            Self::Move { path, dest } => valid_path(path, false) && valid_path(dest, true),
            Self::Import { source, dest } => {
                !source.is_empty() && source.len() <= 4096 && valid_path(dest, true)
            }
            Self::Write {
                path,
                base64,
                expected_modified,
            } => {
                valid_path(path, false)
                    && base64.len() <= MAX_VAULT_WRITE_BYTES / 3 * 4 + 4
                    && expected_modified.len() <= 24
            }
            Self::Search { query, limit, .. } => {
                query.len() <= MAX_QUERY_BYTES && (1..=200).contains(limit)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_follow_windows_rules() {
        assert!(valid_name("[Due 16 Nov 1700h] ITF CA1.pdf"));
        assert!(valid_name("Year 2"));
        assert!(valid_name(".trash"));
        for bad in [
            "", ".", "..", "a/b", "a\\b", "x:y", "CON", "lpt1.txt", "trail.", "trail ", " lead",
        ] {
            assert!(!valid_name(bad), "{bad}");
        }
    }

    #[test]
    fn paths_reject_traversal() {
        assert!(VaultCall::List { dir: String::new() }.validate());
        assert!(VaultCall::List {
            dir: "DBKF/Year 2".into()
        }
        .validate());
        assert!(!VaultCall::List {
            dir: "DBKF/../..".into()
        }
        .validate());
        assert!(!VaultCall::List { dir: "/abs".into() }.validate());
        assert!(!VaultCall::Trash {
            path: String::new()
        }
        .validate());
        assert!(!VaultCall::Move {
            path: "a".into(),
            dest: "b//c".into()
        }
        .validate());
    }
}
