//! Finding files and text inside a granted folder, with ripgrep's own
//! matchers and local `.gitignore` / `.ignore` rules, hidden files skipped, binary
//! files and symlinks passed over. Every result is capped and says what it
//! left out, so the agent narrows instead of paging through a tree.
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use crate::work_files::{protected_path, read_directory, read_text};
use grep_regex::RegexMatcherBuilder;
use grep_searcher::{BinaryDetection, Searcher, SearcherBuilder, Sink, SinkContext, SinkMatch};

/// Directories never walked, gitignored or not.
const SKIPPED: [&str; 6] = [".git", "node_modules", "target", ".cache", "dist", "build"];
const MAX_VISITED: usize = 60_000;
const BUDGET: Duration = Duration::from_millis(2500);
pub const MAX_GLOB_FILES: usize = 100;
pub const MAX_GREP_FILES: usize = 100;
pub const MAX_GREP_LINES: usize = 200;
/// One shown line, in characters; the rest of a minified line is noise.
const MAX_LINE_CHARS: usize = 240;
const MAX_OUTPUT_BYTES: usize = 12 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FindError {
    Pattern,
    NotADirectory,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum GrepOutput {
    #[default]
    Files,
    Lines,
    Count,
}

pub struct GrepQuery<'a> {
    pub pattern: &'a str,
    pub glob: Option<&'a str>,
    pub output: GrepOutput,
    pub context: u8,
    pub case_insensitive: bool,
    pub limit: Option<usize>,
}

/// What a search found, already in the agent's compact form.
#[derive(Debug, Default)]
pub struct Found {
    pub text: String,
    /// Files (glob, files, count) or matching lines (lines).
    pub shown: usize,
    pub total: usize,
    /// The walk stopped early on its time or entry budget.
    pub partial: bool,
}

/// Walk only directory handles checked by the file boundary. Ignore rules are
/// loaded through the same bounded opener; parent/global configuration is never
/// probed. Avoid a second path-based walker which could follow a swapped parent.
struct Walker {
    pending: Vec<PathBuf>,
    entries: Vec<(PathBuf, SystemTime)>,
    ignores: Vec<(PathBuf, ignore::gitignore::Gitignore)>,
    started: Instant,
    partial: bool,
    visited: usize,
}
impl Walker {
    fn new(root: &Path) -> Self {
        let mut value = Self {
            pending: Vec::new(),
            entries: Vec::new(),
            ignores: Vec::new(),
            visited: 0,
            started: Instant::now(),
            partial: false,
        };
        if !protected_path(root) {
            if root.is_dir() {
                value.pending.push(root.to_owned());
            } else {
                value
                    .entries
                    .push((root.to_owned(), SystemTime::UNIX_EPOCH));
            }
        }
        value
    }
}
impl Iterator for Walker {
    type Item = (PathBuf, SystemTime);
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.visited >= MAX_VISITED || self.started.elapsed() >= BUDGET {
                self.partial = true;
                return None;
            }
            if let Some(entry) = self.entries.pop() {
                self.visited += 1;
                return Some(entry);
            }
            let dir = self.pending.pop()?;
            let Ok(entries) = read_directory(&dir) else {
                self.partial = true;
                continue;
            };
            self.partial |= entries.truncated;
            let mut rules = ignore::gitignore::GitignoreBuilder::new(&dir);
            for name in [".gitignore", ".ignore"] {
                if let Ok((_, _, Some(text))) = read_text(&dir.join(name)) {
                    for line in text.lines() {
                        let _ = rules.add_line(None, line);
                    }
                }
            }
            if let Ok(rules) = rules.build() {
                self.ignores.push((dir.clone(), rules));
            }
            for entry in entries.filter_map(Result::ok) {
                self.visited += 1;
                if self.visited >= MAX_VISITED || self.started.elapsed() >= BUDGET {
                    self.partial = true;
                    break;
                }
                let path = entry.path();
                let name = entry.file_name();
                let Some(name) = name.to_str() else { continue };
                if name.starts_with('.') || SKIPPED.contains(&name) || protected_path(&path) {
                    continue;
                }
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                if kind.is_symlink() || (!kind.is_dir() && !kind.is_file()) {
                    continue;
                }
                // Descendants are visited after ancestors. The deepest matching
                // rule wins, including a whitelist that overrides an ancestor.
                let ignored = self
                    .ignores
                    .iter()
                    .rev()
                    .find_map(|(base, rules)| {
                        if !path.starts_with(base) {
                            return None;
                        }
                        let matched = rules.matched(&path, kind.is_dir());
                        (!matched.is_none()).then(|| matched.is_ignore())
                    })
                    .unwrap_or(false);
                if ignored {
                    continue;
                }
                if kind.is_dir() {
                    self.pending.push(path);
                } else if let Ok(metadata) = entry.metadata() {
                    if metadata.len() <= 1024 * 1024 {
                        self.entries
                            .push((path, metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH)));
                    }
                }
            }
        }
    }
}

/// A glob without a slash matches at any depth, as people mean `*.rs`.
fn matcher(pattern: &str) -> Result<globset::GlobMatcher, FindError> {
    let pattern = pattern.trim().trim_start_matches("./");
    if pattern.is_empty() || pattern.len() > 256 || pattern.contains("..") {
        return Err(FindError::Pattern);
    }
    let full = if pattern.contains('/') {
        pattern.to_owned()
    } else {
        format!("**/{pattern}")
    };
    globset::GlobBuilder::new(&full)
        .literal_separator(true)
        .build()
        .map(|glob| glob.compile_matcher())
        .map_err(|_| FindError::Pattern)
}

fn relative(root: &Path, path: &Path) -> String {
    shown_path(path.strip_prefix(root).unwrap_or(path))
}

pub(super) fn shown_path(path: &Path) -> String {
    let shown = path.to_string_lossy().into_owned();
    #[cfg(windows)]
    let shown = shown.replace('\\', "/");
    shown
}

/// Newest first, then by path, so what was just changed leads.
fn by_recency(files: &mut [(PathBuf, SystemTime)]) {
    files.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
}

/// Files under `root` whose path relative to it matches `pattern`.
pub fn glob(root: &Path, pattern: &str) -> Result<Found, FindError> {
    if !root.is_absolute() {
        return Err(FindError::NotADirectory);
    }
    if !root.is_dir() || protected_path(root) {
        return Err(FindError::NotADirectory);
    }
    let matcher = matcher(pattern)?;
    let mut hits = Vec::new();
    let mut walker = Walker::new(root);
    for (path, at) in walker.by_ref() {
        if matcher.is_match(path.strip_prefix(root).unwrap_or(&path)) {
            hits.push((path, at));
        }
    }
    let mut files = hits;
    by_recency(&mut files);
    let total = files.len();
    let mut text = String::new();
    for (path, _) in files.iter().take(MAX_GLOB_FILES) {
        text.push_str(&relative(root, path));
        text.push('\n');
    }
    Ok(Found {
        shown: total.min(MAX_GLOB_FILES),
        total,
        partial: walker.partial,
        text,
    })
}

#[derive(Default)]
struct FileHits {
    path: PathBuf,
    modified: Option<SystemTime>,
    /// (line number, is a match, text), in file order; `None` marks a gap.
    lines: Vec<Option<(u64, bool, String)>>,
    count: usize,
}

struct Collect<'a> {
    hits: &'a mut FileHits,
    keep_lines: bool,
    cap: usize,
}
impl Collect<'_> {
    fn push(&mut self, number: Option<u64>, matched: bool, bytes: &[u8]) {
        if !self.keep_lines {
            return;
        }
        let text = String::from_utf8_lossy(bytes);
        let text = text.trim_end_matches(['\n', '\r']);
        let mut shown: String = text.chars().take(MAX_LINE_CHARS).collect();
        if shown.len() < text.len() {
            shown.push('…');
        }
        self.hits
            .lines
            .push(Some((number.unwrap_or(0), matched, shown)));
    }
}
impl Sink for Collect<'_> {
    type Error = std::io::Error;
    fn matched(&mut self, _: &Searcher, found: &SinkMatch<'_>) -> Result<bool, Self::Error> {
        self.hits.count += 1;
        self.push(found.line_number(), true, found.bytes());
        Ok(self.hits.count < self.cap)
    }
    fn context(&mut self, _: &Searcher, found: &SinkContext<'_>) -> Result<bool, Self::Error> {
        self.push(found.line_number(), false, found.bytes());
        Ok(true)
    }
    fn context_break(&mut self, _: &Searcher) -> Result<bool, Self::Error> {
        if self.keep_lines {
            self.hits.lines.push(None);
        }
        Ok(true)
    }
    fn binary_data(&mut self, _: &Searcher, _: u64) -> Result<bool, Self::Error> {
        Ok(false)
    }
}

/// Lines matching a regular expression under `root` (a folder or one file).
pub fn grep(root: &Path, query: &GrepQuery<'_>) -> Result<Found, FindError> {
    if !root.is_absolute() {
        return Err(FindError::NotADirectory);
    }
    if protected_path(root) {
        return Err(FindError::NotADirectory);
    }
    if query.pattern.is_empty() || query.pattern.len() > 512 {
        return Err(FindError::Pattern);
    }
    let matcher = RegexMatcherBuilder::new()
        .case_smart(!query.case_insensitive)
        .case_insensitive(query.case_insensitive)
        .size_limit(1024 * 1024)
        .dfa_size_limit(2 * 1024 * 1024)
        .build(query.pattern)
        .map_err(|_| FindError::Pattern)?;
    let filter = query.glob.map(matcher_for_filter).transpose()?;
    let lines = query.output == GrepOutput::Lines;
    let context = if lines {
        usize::from(query.context.min(10))
    } else {
        0
    };
    let base = if root.is_file() {
        root.parent().unwrap_or(root).to_path_buf()
    } else {
        root.to_path_buf()
    };
    let mut found = Vec::<FileHits>::new();
    let mut searcher = SearcherBuilder::new()
        .line_number(true)
        .before_context(context)
        .after_context(context)
        .binary_detection(BinaryDetection::quit(0))
        .build();
    let mut walker = Walker::new(root);
    for (path, modified) in walker.by_ref() {
        if filter
            .as_ref()
            .is_some_and(|f| !f.is_match(path.strip_prefix(&base).unwrap_or(&path)))
        {
            continue;
        }
        let Ok((_, _, Some(content))) = read_text(&path) else {
            continue;
        };
        let mut hits = FileHits {
            path,
            ..Default::default()
        };
        let mut sink = Collect {
            hits: &mut hits,
            keep_lines: lines,
            cap: MAX_GREP_LINES + 1,
        };
        if searcher
            .search_slice(&matcher, content.as_bytes(), &mut sink)
            .is_err()
            || hits.count == 0
        {
            continue;
        }
        hits.modified = Some(modified);
        found.push(hits);
        if found.len() >= 20 * MAX_GREP_FILES {
            walker.partial = true;
            break;
        }
    }
    found.sort_by(|a, b| {
        b.modified
            .cmp(&a.modified)
            .then_with(|| a.path.cmp(&b.path))
    });
    Ok(render(
        &base,
        found,
        query.output,
        query.limit,
        walker.partial,
    ))
}

fn matcher_for_filter(glob: &str) -> Result<globset::GlobMatcher, FindError> {
    matcher(glob)
}

fn render(
    base: &Path,
    found: Vec<FileHits>,
    output: GrepOutput,
    limit: Option<usize>,
    partial: bool,
) -> Found {
    let mut text = String::new();
    let mut shown = 0;
    let fits = |text: &String, next: &str| text.len() + next.len() <= MAX_OUTPUT_BYTES;
    match output {
        GrepOutput::Files | GrepOutput::Count => {
            let cap = limit.unwrap_or(MAX_GREP_FILES).clamp(1, MAX_GREP_FILES);
            for hits in found.iter().take(cap) {
                let path = relative(base, &hits.path);
                let line = if output == GrepOutput::Count {
                    let over = if hits.count > MAX_GREP_LINES { "+" } else { "" };
                    format!("{path}:{}{over}\n", hits.count.min(MAX_GREP_LINES))
                } else {
                    format!("{path}\n")
                };
                if !fits(&text, &line) {
                    break;
                }
                text.push_str(&line);
                shown += 1;
            }
            Found {
                text,
                shown,
                total: found.len(),
                partial,
            }
        }
        GrepOutput::Lines => {
            let cap = limit.unwrap_or(MAX_GREP_LINES).clamp(1, MAX_GREP_LINES);
            let total = found.iter().map(|hits| hits.count).sum();
            'files: for hits in &found {
                let path = relative(base, &hits.path);
                let mut block = String::new();
                for line in &hits.lines {
                    match line {
                        None => block.push_str("--\n"),
                        Some((number, matched, line)) => {
                            if *matched {
                                if shown == cap {
                                    break;
                                }
                                shown += 1;
                            }
                            let mark = if *matched { ':' } else { '-' };
                            block.push_str(&format!("{path}{mark}{number}{mark}{line}\n"));
                        }
                    }
                }
                let block = block.trim_end_matches("--\n");
                if !fits(&text, block) {
                    break 'files;
                }
                if !text.is_empty() && !block.is_empty() {
                    text.push_str("--\n");
                }
                text.push_str(block);
                if shown == cap {
                    break;
                }
            }
            Found {
                text,
                shown,
                total,
                partial,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> tempfile::TempDir {
        let dir = crate::work_files::test_tempdir();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src/deep")).unwrap();
        std::fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
        std::fs::create_dir_all(root.join("generated")).unwrap();
        std::fs::write(root.join(".gitignore"), "generated/\n*.log\n").unwrap();
        std::fs::write(
            root.join("src/lib.rs"),
            "pub fn add(a: i32) -> i32 {\n    a + 1\n}\n",
        )
        .unwrap();
        std::fs::write(
            root.join("src/deep/util.rs"),
            "// Add helper\nfn helper() {}\nfn add_twice() {}\n",
        )
        .unwrap();
        std::fs::write(
            root.join("node_modules/pkg/index.js"),
            "function add() {}\n",
        )
        .unwrap();
        std::fs::write(root.join("generated/out.rs"), "fn add() {}\n").unwrap();
        std::fs::write(root.join("debug.log"), "add\n").unwrap();
        std::fs::write(root.join(".hidden.rs"), "fn add() {}\n").unwrap();
        std::fs::write(root.join("image.bin"), b"add\0\x01\x02").unwrap();
        dir
    }

    #[test]
    fn search_withholds_recognized_secret_body_and_filename() {
        let directory = crate::work_files::test_tempdir();
        let root = directory.path().canonicalize().unwrap();
        std::fs::write(root.join("plain.txt"), "needle ordinary").unwrap();
        std::fs::write(root.join("body.txt"), "needle\nghp_inertFixtureToken").unwrap();
        std::fs::write(
            root.join("key.txt"),
            "needle\n-----BEGIN PGP PRIVATE KEY BLOCK-----",
        )
        .unwrap();
        std::fs::write(root.join("ghp_inertFixtureFilename.txt"), "needle").unwrap();
        let found = grep(
            &root,
            &GrepQuery {
                pattern: "needle",
                glob: None,
                output: GrepOutput::Lines,
                context: 0,
                case_insensitive: false,
                limit: None,
            },
        )
        .unwrap();
        assert!(found.text.contains("plain.txt"));
        assert!(!found.text.contains("body.txt"));
        assert!(!found.text.contains("key.txt"));
        assert!(!found.text.contains("ghp_"));
        let found = glob(&root, "*.txt").unwrap();
        assert!(!found.text.contains("ghp_"));
    }

    #[test]
    fn child_whitelist_overrides_parent_ignore_rule() {
        let directory = crate::work_files::test_tempdir();
        let root = directory.path().canonicalize().unwrap();
        std::fs::create_dir(root.join("child")).unwrap();
        std::fs::write(root.join(".gitignore"), "*.txt\n").unwrap();
        std::fs::write(root.join("child/.gitignore"), "!keep.txt\n").unwrap();
        for path in ["root.txt", "child/keep.txt", "child/ignored.txt"] {
            std::fs::write(root.join(path), "needle\n").unwrap();
        }
        let found = glob(&root, "*.txt").unwrap();
        assert_eq!(found.text, "child/keep.txt\n");
        assert_eq!((found.shown, found.total, found.partial), (1, 1, false));
        let found = grep(
            &root,
            &GrepQuery {
                pattern: "needle",
                glob: Some("*.txt"),
                output: GrepOutput::Files,
                context: 0,
                case_insensitive: false,
                limit: None,
            },
        )
        .unwrap();
        assert_eq!(found.text, "child/keep.txt\n");
        assert_eq!((found.shown, found.total, found.partial), (1, 1, false));
    }

    #[test]
    fn protected_files_and_external_ignore_rules_never_enter_results() {
        let parent = crate::work_files::test_tempdir();
        let parent = parent.path().canonicalize().unwrap();
        let root = parent.join("project");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(parent.join(".gitignore"), "*.rs\n").unwrap();
        std::fs::write(root.join("main.rs"), "public-needle\n").unwrap();
        std::fs::write(root.join(".env"), "private-needle\n").unwrap();
        std::fs::hard_link(root.join(".env"), root.join("alias.txt")).unwrap();
        std::fs::write(root.join("credentials.json"), "private-needle\n").unwrap();
        let paths = glob(&root, "*").unwrap();
        assert_eq!(paths.text, "main.rs\n");
        let query = GrepQuery {
            pattern: "needle",
            glob: None,
            output: GrepOutput::Lines,
            context: 0,
            case_insensitive: false,
            limit: None,
        };
        let found = grep(&root, &query).unwrap();
        assert!(found.text.contains("public-needle"));
        assert!(!found.text.contains("private-needle"));
        assert!(grep(&root.join(".env"), &query).is_err());
        #[cfg(unix)]
        {
            let outside = parent.join("outside");
            std::fs::create_dir(&outside).unwrap();
            std::fs::write(outside.join("outside.txt"), "outside-needle").unwrap();
            std::fs::rename(&root, parent.join("old")).unwrap();
            std::os::unix::fs::symlink(&outside, &root).unwrap();
            assert!(glob(&root, "*").unwrap().text.is_empty());
            assert!(grep(&root, &query).unwrap().text.is_empty());
        }
    }

    #[test]
    fn glob_respects_gitignore_and_sorts_by_recency() {
        let dir = tree();
        let canonical = dir.path().canonicalize().unwrap();
        let root = canonical.as_path();
        let found = glob(root, "*.rs").unwrap();
        let listed: Vec<&str> = found.text.lines().collect();
        assert_eq!(listed.len(), 2, "{listed:?}");
        assert!(listed.contains(&"src/lib.rs"));
        assert!(listed.contains(&"src/deep/util.rs"));
        let later = SystemTime::now() + Duration::from_secs(60);
        std::fs::File::options()
            .write(true)
            .open(root.join("src/deep/util.rs"))
            .unwrap()
            .set_modified(later)
            .unwrap();
        assert_eq!(
            glob(root, "**/*.rs").unwrap().text.lines().next(),
            Some("src/deep/util.rs")
        );
        assert_eq!(glob(root, "src/*.rs").unwrap().text, "src/lib.rs\n");
        assert!(glob(root, "../*.rs").is_err());
        assert!(glob(root, "[").is_err());
    }

    #[test]
    fn grep_modes_context_and_caps() {
        let dir = tree();
        let canonical = dir.path().canonicalize().unwrap();
        let root = canonical.as_path();
        let query = |output, context| GrepQuery {
            pattern: "add",
            glob: None,
            output,
            context,
            case_insensitive: false,
            limit: None,
        };
        let files = grep(root, &query(GrepOutput::Files, 0)).unwrap();
        assert_eq!(files.total, 2, "{}", files.text);
        assert!(!files.text.contains("node_modules") && !files.text.contains("generated"));
        let counts = grep(root, &query(GrepOutput::Count, 0)).unwrap();
        assert!(counts.text.contains("src/lib.rs:1\n"));
        // Smart case: lowercase matches "Add" too.
        assert!(
            counts.text.contains("src/deep/util.rs:2\n"),
            "{}",
            counts.text
        );
        let lines = grep(root, &query(GrepOutput::Lines, 1)).unwrap();
        assert!(lines.text.contains("src/lib.rs:1:pub fn add"));
        assert!(lines.text.contains("src/lib.rs-2-    a + 1"));
        let sensitive = grep(
            root,
            &GrepQuery {
                pattern: "Add",
                ..query(GrepOutput::Files, 0)
            },
        )
        .unwrap();
        assert_eq!(sensitive.text, "src/deep/util.rs\n");
        let filtered = grep(
            root,
            &GrepQuery {
                glob: Some("src/deep/*.rs"),
                ..query(GrepOutput::Files, 0)
            },
        )
        .unwrap();
        assert_eq!(filtered.text, "src/deep/util.rs\n");
        let one = grep(&root.join("src/lib.rs"), &query(GrepOutput::Lines, 0)).unwrap();
        assert_eq!(one.text, "lib.rs:1:pub fn add(a: i32) -> i32 {\n");
        assert!(grep(
            root,
            &GrepQuery {
                pattern: "(",
                ..query(GrepOutput::Files, 0)
            }
        )
        .is_err());
    }

    #[test]
    fn grep_caps_lines_and_clips_long_ones() {
        let dir = crate::work_files::test_tempdir();
        let body: String = (0..500).map(|n| format!("needle {n}\n")).collect();
        std::fs::write(dir.path().join("many.txt"), body).unwrap();
        std::fs::write(
            dir.path().join("wide.txt"),
            format!("needle {}\n", "x".repeat(5000)),
        )
        .unwrap();
        let found = grep(
            &dir.path().canonicalize().unwrap(),
            &GrepQuery {
                pattern: "needle",
                glob: None,
                output: GrepOutput::Lines,
                context: 0,
                case_insensitive: false,
                limit: None,
            },
        )
        .unwrap();
        assert_eq!(found.shown, MAX_GREP_LINES);
        assert!(found.total > MAX_GREP_LINES);
        assert!(found.text.len() <= MAX_OUTPUT_BYTES);
        assert!(found
            .text
            .lines()
            .all(|line| line.chars().count() < MAX_LINE_CHARS + 32));
    }
}
