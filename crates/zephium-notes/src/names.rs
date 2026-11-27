//! File names for notes. A name follows its note's title while the two agree,
//! and is otherwise the person's to choose.
use unicode_normalization::UnicodeNormalization;

pub const EXTENSION: &str = ".md";
pub const TRASH: &str = ".trash";
const MAX_STEM_CHARS: usize = 120;
const MAX_STEM_BYTES: usize = 200;
const MAX_COMPONENT_BYTES: usize = 255;
const MAX_PATH_BYTES: usize = 1024;
pub const MAX_DEPTH: usize = 8;

/// The file stem a title would be saved under. Characters that no supported
/// file system (or a sync service between them) accepts become spaces.
pub fn stem_for(title: &str) -> String {
    let mut stem = String::new();
    for c in title.nfc() {
        let replaced =
            matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control();
        let c = if replaced { ' ' } else { c };
        if c.is_whitespace() {
            if !stem.is_empty() && !stem.ends_with(' ') {
                stem.push(' ');
            }
        } else {
            stem.push(c);
        }
    }
    let mut stem: String = stem
        .trim_matches(|c: char| c == ' ' || c == '.')
        .chars()
        .take(MAX_STEM_CHARS)
        .collect();
    while stem.len() > MAX_STEM_BYTES {
        stem.pop();
    }
    let mut stem = stem.trim_end_matches([' ', '.']).to_string();
    if stem.is_empty() {
        stem = "Untitled".into();
    }
    if reserved_on_windows(&stem) {
        let suffix_at = stem.find('.').unwrap_or(stem.len());
        stem.insert_str(suffix_at, " note");
    }
    stem
}

fn reserved_on_windows(stem: &str) -> bool {
    let base = stem
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((base.starts_with("COM") || base.starts_with("LPT"))
            && base.len() == 4
            && base.as_bytes()[3].is_ascii_digit())
}

/// Compares names the way case-insensitive, normalization-insensitive file
/// systems (APFS and NTFS defaults) do, so two notes never collide on disk.
pub fn path_key(path: &str) -> String {
    path.nfc().collect::<String>().to_lowercase()
}

/// The stem of a folder-relative note path.
pub fn stem_of(path: &str) -> &str {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.strip_suffix(EXTENSION).unwrap_or(name)
}

/// A folder-relative path this crate will read or write: plain components,
/// nothing hidden except the trash, nothing that climbs out of the folder.
pub fn valid_path(path: &str) -> bool {
    if path.is_empty() || path.len() > MAX_PATH_BYTES || !path.ends_with(EXTENSION) {
        return false;
    }
    let components: Vec<&str> = path.split('/').collect();
    components.len() <= MAX_DEPTH
        && components.iter().enumerate().all(|(index, component)| {
            let trash = index == 0 && *component == TRASH && components.len() > 1;
            !component.is_empty()
                && component.len() <= MAX_COMPONENT_BYTES
                && (trash || !component.starts_with('.'))
                && !component.contains(['\\', '\0'])
                && !component.chars().any(char::is_control)
        })
}

/// `stem.md`, `stem 2.md`, `stem 3.md`… in `directory`, the first one `taken`
/// refuses.
pub fn unique_path(
    directory: &str,
    stem: &str,
    mut taken: impl FnMut(&str) -> bool,
) -> Option<String> {
    let prefix = if directory.is_empty() {
        String::new()
    } else {
        format!("{directory}/")
    };
    (1..10_000).find_map(|n| {
        let candidate = if n == 1 {
            format!("{prefix}{stem}{EXTENSION}")
        } else {
            format!("{prefix}{stem} {n}{EXTENSION}")
        };
        (valid_path(&candidate) && !taken(&candidate)).then_some(candidate)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_become_portable_stems() {
        assert_eq!(stem_for("  Trip: Lisbon / Porto?  "), "Trip Lisbon Porto");
        assert_eq!(stem_for("..hidden."), "hidden");
        assert_eq!(stem_for("\u{7}"), "Untitled");
        assert_eq!(stem_for("con"), "con note");
        assert_eq!(stem_for("COM1.txt"), "COM1 note.txt");
        assert_eq!(stem_for(&"a".repeat(500)).chars().count(), MAX_STEM_CHARS);
        assert!(stem_for(&"é".repeat(150)).len() <= MAX_STEM_BYTES);
    }

    #[test]
    fn reserved_names_with_dots_are_changed_before_the_first_extension() {
        for (title, expected) in [
            ("COM1.txt", "COM1 note.txt"),
            ("aux.backup", "aux note.backup"),
            ("NUL.folder.part", "NUL note.folder.part"),
        ] {
            let stem = stem_for(title);
            assert_eq!(stem, expected);
            assert!(!reserved_on_windows(&stem));
        }
    }

    #[test]
    fn paths_stay_inside_the_folder() {
        assert!(valid_path("Plans.md"));
        assert!(valid_path("Work/Plans.md"));
        assert!(valid_path(".trash/Plans.md"));
        assert!(!valid_path("../Plans.md"));
        assert!(!valid_path("/Plans.md"));
        assert!(!valid_path(".obsidian/Plans.md"));
        assert!(!valid_path("Work/.trash/Plans.md"));
        assert!(!valid_path("Plans.txt"));
        assert!(!valid_path("a\\b.md"));
        assert!(valid_path(&format!("{}n.md", "a/".repeat(MAX_DEPTH - 1))));
        assert!(!valid_path(&format!("{}n.md", "a/".repeat(MAX_DEPTH))));
    }

    #[test]
    fn collisions_are_found_regardless_of_case_or_normalization() {
        assert_eq!(path_key("Cafe\u{301}.md"), path_key("CAFÉ.md"));
        let taken = [path_key("Plans.md"), path_key("Plans 2.md")];
        let path = unique_path("", "plans", |path| taken.contains(&path_key(path)));
        assert_eq!(path.as_deref(), Some("plans 3.md"));
    }
}
