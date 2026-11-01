//! A flat list of every visible file and folder name, for the launcher, the
//! quick switcher and the "File to..." picker. Rebuilt lazily after changes.
use std::fs;

use zephium_core::vault::{valid_name, VaultEntry, VaultEntryKind};

use crate::{hidden, Vault};

const MAX_ENTRIES: usize = 100_000;
const MAX_DEPTH: usize = 24;

#[derive(Default)]
pub struct NameIndex {
    entries: Vec<(String, VaultEntryKind)>,
    stale: bool,
    built: bool,
}

impl NameIndex {
    pub fn invalidate(&mut self) {
        self.stale = true;
    }

    fn rebuild(&mut self, vault: &Vault) {
        self.entries.clear();
        let mut stack = vec![(String::new(), 0usize)];
        while let Some((dir, depth)) = stack.pop() {
            let Ok(absolute) = vault.resolve(&dir) else {
                continue;
            };
            let Ok(items) = fs::read_dir(absolute) else {
                continue;
            };
            for item in items.flatten() {
                let Some(name) = item.file_name().to_str().map(str::to_owned) else {
                    continue;
                };
                if hidden(&name) || !valid_name(&name) {
                    continue;
                }
                let Ok(kind) = item.file_type() else { continue };
                if kind.is_symlink() {
                    continue;
                }
                let path = if dir.is_empty() {
                    name
                } else {
                    format!("{dir}/{name}")
                };
                if kind.is_dir() {
                    self.entries.push((path.clone(), VaultEntryKind::Folder));
                    if depth + 1 < MAX_DEPTH {
                        stack.push((path, depth + 1));
                    }
                } else {
                    self.entries.push((path, VaultEntryKind::File));
                }
                if self.entries.len() >= MAX_ENTRIES {
                    return;
                }
            }
        }
    }

    /// Best matches for `query`. Every query word must appear, in order, as a
    /// subsequence of the path; matches in the file name and at word starts
    /// rank higher, as do shorter paths.
    pub fn search(
        &mut self,
        vault: &Vault,
        query: &str,
        limit: usize,
        folders_only: bool,
    ) -> Vec<VaultEntry> {
        if !self.built || self.stale {
            self.rebuild(vault);
            self.built = true;
            self.stale = false;
        }
        let query = query.trim().to_lowercase();
        let mut scored: Vec<(i64, &str)> = self
            .entries
            .iter()
            .filter(|(_, kind)| !folders_only || *kind == VaultEntryKind::Folder)
            .filter_map(|(path, _)| score(path, &query).map(|score| (score, path.as_str())))
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.len().cmp(&b.1.len())));
        scored
            .into_iter()
            .take(limit)
            .filter_map(|(_, path)| vault.stat(path).ok())
            .collect()
    }
}

fn score(path: &str, query: &str) -> Option<i64> {
    if query.is_empty() {
        return Some(-(path.len() as i64));
    }
    let lower = path.to_lowercase();
    let name_at = lower.rfind('/').map_or(0, |at| at + 1);
    let chars: Vec<char> = lower.chars().collect();
    let name_char_at = lower[..name_at].chars().count();
    let mut total = 0i64;
    let mut at = 0usize;
    for word in query.split_whitespace() {
        let mut previous: Option<usize> = None;
        for wanted in word.chars() {
            let found = (at..chars.len()).find(|&index| chars[index] == wanted)?;
            let mut points = 1;
            if found == 0 || matches!(chars[found - 1], '/' | ' ' | '_' | '-' | '.' | '[' | '(') {
                points += 6;
            }
            if previous.is_some_and(|before| before + 1 == found) {
                points += 4;
            }
            if found >= name_char_at {
                points += 3;
            }
            total += points;
            previous = Some(found);
            at = found + 1;
        }
    }
    Some(total * 10 - chars.len() as i64)
}

#[cfg(test)]
mod tests {
    use super::score;

    #[test]
    fn words_and_names_rank_first() {
        let a = score("DBKF/Year 2/Sem 2/ITF/CA1/Brief.pdf", "itf ca1").unwrap();
        let b = score("DBKF/Year 2/Sem 2/FI/Topic 1/notes.md", "itf ca1");
        assert!(b.is_none());
        let c = score("ITF/CA1", "itf ca1").unwrap();
        assert!(c > a);
        assert!(score("abc", "abd").is_none());
    }
}
