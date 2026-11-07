//! Reads bookmarks and history from other browsers on this device. Pure
//! readers: they find profiles, copy what a running browser holds open, and
//! return neutral values. Nothing here writes to Zephium's own storage.
//!
//! Every browser keeps its data in one of four layouts: Chromium's (Chrome
//! and what is built on it), Firefox's (and its forks), Safari's, or Arc's
//! own sidebar.

#![forbid(unsafe_code)]

mod arc;
mod chromium;
mod firefox;
mod icons;
mod mozlz4;
mod safari;
mod snapshot;
mod zen;

use std::path::{Path, PathBuf};

pub use icons::{https_origin, SourceIcon, MAX_ICONS};
pub use zephium_core::bookmarks::ImportNode;
pub use zephium_core::ports::store::ImportedVisit;

/// Imported trees are opened into their parent past this depth. Real
/// browsers nest a handful of levels; a hostile file cannot exhaust the stack.
pub const MAX_TREE_DEPTH: usize = 48;
/// Bookmarks read from one source, folders included.
pub const MAX_NODES: usize = 50_000;
/// How far back history is read.
pub const HISTORY_DAYS: i64 = 180;
/// Essentials read from one source; a row of kept sites, not a library.
pub const MAX_ESSENTIALS: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Browser {
    Arc,
    Zen,
    Chrome,
    Safari,
    Brave,
    Helium,
    Dia,
    Firefox,
    Edge,
    Vivaldi,
    Opera,
    OperaGx,
    Comet,
    Chromium,
    Floorp,
    LibreWolf,
    Waterfox,
}

impl Browser {
    /// In the order they are offered: where this audience most often comes
    /// from first.
    pub const ALL: [Browser; 17] = [
        Browser::Arc,
        Browser::Zen,
        Browser::Chrome,
        Browser::Safari,
        Browser::Brave,
        Browser::Helium,
        Browser::Dia,
        Browser::Firefox,
        Browser::Edge,
        Browser::Vivaldi,
        Browser::Opera,
        Browser::OperaGx,
        Browser::Comet,
        Browser::Chromium,
        Browser::Floorp,
        Browser::LibreWolf,
        Browser::Waterfox,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Browser::Arc => "arc",
            Browser::Zen => "zen",
            Browser::Chrome => "chrome",
            Browser::Safari => "safari",
            Browser::Brave => "brave",
            Browser::Helium => "helium",
            Browser::Dia => "dia",
            Browser::Firefox => "firefox",
            Browser::Edge => "edge",
            Browser::Vivaldi => "vivaldi",
            Browser::Opera => "opera",
            Browser::OperaGx => "opera-gx",
            Browser::Comet => "comet",
            Browser::Chromium => "chromium",
            Browser::Floorp => "floorp",
            Browser::LibreWolf => "librewolf",
            Browser::Waterfox => "waterfox",
        }
    }

    pub fn from_id(id: &str) -> Option<Browser> {
        Browser::ALL.into_iter().find(|browser| browser.id() == id)
    }

    pub fn name(self) -> &'static str {
        match self {
            Browser::Arc => "Arc",
            Browser::Zen => "Zen",
            Browser::Chrome => "Chrome",
            Browser::Safari => "Safari",
            Browser::Brave => "Brave",
            Browser::Helium => "Helium",
            Browser::Dia => "Dia",
            Browser::Firefox => "Firefox",
            Browser::Edge => "Edge",
            Browser::Vivaldi => "Vivaldi",
            Browser::Opera => "Opera",
            Browser::OperaGx => "Opera GX",
            Browser::Comet => "Comet",
            Browser::Chromium => "Chromium",
            Browser::Floorp => "Floorp",
            Browser::LibreWolf => "LibreWolf",
            Browser::Waterfox => "Waterfox",
        }
    }

    /// Keeps Firefox's profile tree: `profiles.ini` and `places.sqlite`.
    fn is_firefox_family(self) -> bool {
        matches!(
            self,
            Browser::Firefox
                | Browser::Zen
                | Browser::Floorp
                | Browser::LibreWolf
                | Browser::Waterfox
        )
    }
}

/// The platform whose layout is read: the host's in production, any of the
/// three in tests, so every platform's paths are covered on one machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Os {
    Mac,
    Windows,
    Linux,
}

impl Os {
    pub(crate) const HOST: Os = if cfg!(target_os = "macos") {
        Os::Mac
    } else if cfg!(windows) {
        Os::Windows
    } else {
        Os::Linux
    };
}

/// Where this user's browsers keep their data. Built from the environment in
/// production and from a scratch directory in tests.
#[derive(Clone, Debug)]
pub struct Locations {
    pub home: PathBuf,
    /// Windows %LOCALAPPDATA%.
    pub local_app_data: Option<PathBuf>,
    /// Windows %APPDATA%.
    pub app_data: Option<PathBuf>,
    pub(crate) os: Os,
}

impl Locations {
    pub fn from_env() -> Option<Locations> {
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from)?;
        Some(Locations {
            home,
            local_app_data: std::env::var_os("LOCALAPPDATA").map(PathBuf::from),
            app_data: std::env::var_os("APPDATA").map(PathBuf::from),
            os: Os::HOST,
        })
    }

    /// A scratch home laid out as `os` would have it, with `Local` and
    /// `Roaming` standing in for %LOCALAPPDATA% and %APPDATA%.
    #[cfg(test)]
    pub(crate) fn scratch(os: Os, home: &Path) -> Locations {
        Locations {
            home: home.to_owned(),
            local_app_data: Some(home.join("Local")),
            app_data: Some(home.join("Roaming")),
            os,
        }
    }

    /// macOS `~/Library/Application Support`.
    fn support(&self) -> PathBuf {
        self.home.join("Library").join("Application Support")
    }

    /// Windows Store apps keep their data in `%LOCALAPPDATA%\Packages\
    /// <name>_<publisher id>`; the publisher id is not worth hard-coding.
    fn store_packages(&self, name: &str) -> Vec<PathBuf> {
        let Some(packages) = self.local_app_data.as_ref().map(|dir| dir.join("Packages")) else {
            return Vec::new();
        };
        let prefix = format!("{name}_");
        let mut found: Vec<PathBuf> = std::fs::read_dir(packages)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
            .map(|entry| entry.path())
            .collect();
        found.sort();
        found
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Profile {
    /// Stable within the source: a Chromium profile directory name, or a
    /// Firefox profile path as profiles.ini records it.
    pub id: String,
    pub name: String,
    pub bookmarks: bool,
    pub history: bool,
    /// Sites kept in the sidebar, which Zephium calls Essentials.
    pub essentials: bool,
}

/// A site another browser keeps pinned at the top of its sidebar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Essential {
    pub title: String,
    pub url: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    pub browser: Browser,
    pub profiles: Vec<Profile>,
    /// The system must grant access first (Safari's Full Disk Access).
    pub needs_permission: bool,
}

#[derive(Debug)]
pub enum ImportError {
    /// The source or profile is not on this device any more.
    Missing,
    /// The system refused access (Safari without Full Disk Access).
    Permission,
    /// The browser holds the file locked (Windows, while it runs).
    Busy,
    /// The file is not in the shape this browser writes.
    Unreadable,
}

impl From<std::io::Error> for ImportError {
    fn from(error: std::io::Error) -> Self {
        match error.kind() {
            std::io::ErrorKind::NotFound => ImportError::Missing,
            std::io::ErrorKind::PermissionDenied => ImportError::Permission,
            // ERROR_SHARING_VIOLATION and ERROR_LOCK_VIOLATION.
            _ if matches!(error.raw_os_error(), Some(32 | 33)) => ImportError::Busy,
            _ => ImportError::Unreadable,
        }
    }
}

impl From<rusqlite::Error> for ImportError {
    fn from(_: rusqlite::Error) -> Self {
        ImportError::Unreadable
    }
}

/// Browsers found on this device with at least one profile that has
/// something to import.
pub fn discover(locations: &Locations) -> Vec<Source> {
    Browser::ALL
        .into_iter()
        .filter_map(|browser| source(locations, browser))
        .collect()
}

/// One browser's source, if it is on this device with something to import.
pub fn source(locations: &Locations, browser: Browser) -> Option<Source> {
    let profiles = match browser {
        Browser::Safari => return safari::source(locations),
        Browser::Arc => arc::profiles(locations),
        browser if browser.is_firefox_family() => firefox::profiles(locations, browser),
        browser => chromium::installed(locations, browser)
            .map(|(_, profiles)| profiles)
            .unwrap_or_default(),
    };
    (!profiles.is_empty()).then_some(Source {
        browser,
        profiles,
        needs_permission: false,
    })
}

/// The directory holding one profile's databases. Safari has none: it keeps
/// them in a fixed place.
fn profile_dir(
    locations: &Locations,
    browser: Browser,
    profile: &str,
) -> Result<PathBuf, ImportError> {
    match browser {
        Browser::Safari => Err(ImportError::Missing),
        Browser::Arc => arc::profile_dir(locations, profile),
        browser if browser.is_firefox_family() => firefox::profile_dir(locations, browser, profile),
        browser => chromium::profile_dir(locations, browser, profile),
    }
}

/// Bookmarks of one profile, in that browser's own order.
pub fn bookmarks(
    locations: &Locations,
    browser: Browser,
    profile: &str,
) -> Result<Vec<ImportNode>, ImportError> {
    let mut budget = MAX_NODES;
    let nodes = match browser {
        Browser::Safari => safari::bookmarks(locations)?,
        Browser::Arc => arc::bookmarks(locations, profile)?,
        Browser::Zen => {
            let dir = profile_dir(locations, browser, profile)?;
            // The library first, then what each space keeps pinned.
            let mut nodes = firefox::bookmarks(&dir)?;
            if zen::has_session(&dir) {
                nodes.extend(zen::pinned(&dir)?);
            }
            nodes
        }
        browser if browser.is_firefox_family() => {
            firefox::bookmarks(&profile_dir(locations, browser, profile)?)?
        }
        browser => chromium::bookmarks(&profile_dir(locations, browser, profile)?)?,
    };
    Ok(bound(nodes, 0, &mut budget))
}

/// Visits of one profile from the last `HISTORY_DAYS`, newest first.
pub fn history(
    locations: &Locations,
    browser: Browser,
    profile: &str,
    now: i64,
) -> Result<Vec<ImportedVisit>, ImportError> {
    let since = now - HISTORY_DAYS * 24 * 3600;
    let limit = zephium_core::ports::store::MAX_IMPORTED_VISITS;
    match browser {
        Browser::Safari => safari::history(locations, since, limit),
        browser if browser.is_firefox_family() => {
            firefox::history(&profile_dir(locations, browser, profile)?, since, limit)
        }
        // Arc keeps Chromium's history, though not its bookmarks.
        browser => chromium::history(&profile_dir(locations, browser, profile)?, since, limit),
    }
}

/// The icons one profile already holds for `wanted` origins, most wanted
/// first. Safari keeps its icon cache out of reach, so it has none to offer.
pub fn icons(
    locations: &Locations,
    browser: Browser,
    profile: &str,
    wanted: &[String],
) -> Result<Vec<SourceIcon>, ImportError> {
    match browser {
        Browser::Safari => Ok(Vec::new()),
        browser if browser.is_firefox_family() => {
            icons::firefox(&profile_dir(locations, browser, profile)?, wanted)
        }
        browser => icons::chromium(&profile_dir(locations, browser, profile)?, wanted),
    }
}

/// Sites one profile keeps at the top of its sidebar, in its order. Only
/// browsers with such a row have any.
pub fn essentials(
    locations: &Locations,
    browser: Browser,
    profile: &str,
) -> Result<Vec<Essential>, ImportError> {
    match browser {
        Browser::Arc => arc::essentials(locations, profile),
        Browser::Zen => zen::essentials(&profile_dir(locations, browser, profile)?),
        _ => Ok(Vec::new()),
    }
}

/// Caps a tree's size and depth: folders past the depth limit are opened
/// into their parent, and reading stops at the node budget. Only links
/// Zephium would open are kept, so what is counted is what can be added.
fn bound(nodes: Vec<ImportNode>, depth: usize, budget: &mut usize) -> Vec<ImportNode> {
    let mut kept = Vec::new();
    for node in nodes {
        if *budget == 0 {
            break;
        }
        match node {
            ImportNode::Folder { title, children } if depth < MAX_TREE_DEPTH => {
                *budget -= 1;
                let children = bound(children, depth + 1, budget);
                if !children.is_empty() {
                    kept.push(ImportNode::Folder { title, children });
                }
            }
            ImportNode::Folder { children, .. } => kept.extend(bound(children, depth, budget)),
            ImportNode::Link { ref url, .. } if !zephium_core::navigation::is_allowed_str(url) => {}
            link => {
                *budget -= 1;
                kept.push(link);
            }
        }
    }
    kept
}

/// A child path that stays inside `root`: no separators, no parent steps.
fn child(root: &Path, name: &str) -> Option<PathBuf> {
    let plain = !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\'])
        && !name.contains('\0');
    plain.then(|| root.join(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(url: &str) -> ImportNode {
        ImportNode::Link {
            title: url.into(),
            url: url.into(),
        }
    }

    #[test]
    fn browsers_are_named_once_and_found_again_by_id() {
        let mut ids = std::collections::HashSet::new();
        for browser in Browser::ALL {
            assert!(ids.insert(browser.id()), "{}", browser.id());
            assert_eq!(Browser::from_id(browser.id()), Some(browser));
        }
        assert_eq!(Browser::from_id("netscape"), None);
        assert_eq!(
            Browser::ALL[..7],
            [
                Browser::Arc,
                Browser::Zen,
                Browser::Chrome,
                Browser::Safari,
                Browser::Brave,
                Browser::Helium,
                Browser::Dia
            ]
        );
    }

    #[test]
    fn discovery_lists_what_is_installed_in_the_order_people_are_offered_it() {
        let home = tempfile::tempdir().unwrap();
        for tree in [
            "Library/Application Support/Vivaldi/Default",
            "Library/Application Support/Google/Chrome/Default",
            "Library/Application Support/net.imput.helium/Default",
        ] {
            std::fs::create_dir_all(home.path().join(tree)).unwrap();
            std::fs::write(home.path().join(tree).join("History"), b"").unwrap();
        }
        let found = discover(&Locations::scratch(Os::Mac, home.path()));
        // Safari ships with every Mac, so it is always listed.
        assert_eq!(
            found
                .iter()
                .map(|source| source.browser)
                .collect::<Vec<_>>(),
            [
                Browser::Chrome,
                Browser::Safari,
                Browser::Helium,
                Browser::Vivaldi
            ]
        );
        assert!(discover(&Locations::scratch(Os::Linux, home.path())).is_empty());
    }

    #[test]
    fn bounding_opens_deep_folders_and_drops_empty_ones() {
        let mut deep = link("https://deep.example/");
        for level in 0..(MAX_TREE_DEPTH + 5) {
            deep = ImportNode::Folder {
                title: format!("{level}"),
                children: vec![deep],
            };
        }
        let empty = ImportNode::Folder {
            title: "Empty".into(),
            children: vec![],
        };
        let mut budget = MAX_NODES;
        let bounded = bound(vec![deep, empty], 0, &mut budget);
        assert_eq!(bounded.len(), 1);
        let mut depth = 0;
        let mut node = &bounded[0];
        while let ImportNode::Folder { children, .. } = node {
            depth += 1;
            node = &children[0];
        }
        assert_eq!(depth, MAX_TREE_DEPTH);
        assert_eq!(node, &link("https://deep.example/"));
    }

    #[test]
    fn bounding_keeps_only_links_that_can_be_opened() {
        let mut budget = MAX_NODES;
        let bounded = bound(
            vec![
                link("https://ok.example/"),
                link("javascript:alert(1)"),
                link("chrome://settings/"),
                link("file:///C:/notes.txt"),
                link("https://user:secret@example.com/"),
                ImportNode::Folder {
                    title: "Internal".into(),
                    children: vec![link("edge://favorites/"), link("about:config")],
                },
                ImportNode::Folder {
                    title: "Mixed".into(),
                    children: vec![link("chrome://flags/"), link("http://also-ok.example/")],
                },
            ],
            0,
            &mut budget,
        );
        assert_eq!(
            bounded,
            vec![
                link("https://ok.example/"),
                ImportNode::Folder {
                    title: "Mixed".into(),
                    children: vec![link("http://also-ok.example/")]
                }
            ]
        );
    }

    #[test]
    fn bounding_stops_at_the_node_budget() {
        let mut budget = 3;
        let bounded = bound(
            (0..10)
                .map(|i| link(&format!("https://{i}.example/")))
                .collect(),
            0,
            &mut budget,
        );
        assert_eq!(bounded.len(), 3);
    }

    #[test]
    fn profile_names_cannot_leave_their_directory() {
        let root = Path::new("/data");
        assert_eq!(child(root, "Default"), Some(PathBuf::from("/data/Default")));
        for name in ["", ".", "..", "a/b", "a\\b", "../x"] {
            assert_eq!(child(root, name), None, "{name}");
        }
    }
}
