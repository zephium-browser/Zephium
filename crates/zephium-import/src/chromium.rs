//! Chrome and the browsers built on Chromium share its profile layout: a
//! user-data directory listing profiles in `Local State`, each with a
//! `Bookmarks` JSON file and a `History` database. A profile signed in to an
//! account may keep that account's bookmarks apart, in `AccountBookmarks`, in
//! the same shape.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::{
    child, snapshot, Browser, ImportError, ImportNode, ImportedVisit, Locations, Os, Profile,
};

/// Microseconds between 1601-01-01 (Chromium's epoch) and 1970-01-01.
const EPOCH_OFFSET_SECONDS: i64 = 11_644_473_600;
/// The profile's own bookmarks, and those of the account it is signed in to.
const BOOKMARK_FILES: [&str; 2] = ["Bookmarks", "AccountBookmarks"];
/// The id of a profile kept in the user-data directory itself, as Opera does
/// on macOS and Linux.
const FLAT: &str = ".";

fn under(base: PathBuf, parts: &[&str]) -> PathBuf {
    parts.iter().fold(base, |path, part| path.join(part))
}

/// Where each browser keeps its user-data directory, most likely first. On
/// Linux the same browser may be a native package, a Flatpak or a Snap. A
/// browser absent from a platform has no roots there.
fn roots(locations: &Locations, browser: Browser) -> Vec<PathBuf> {
    match locations.os {
        Os::Mac => {
            let parts: &[&str] = match browser {
                Browser::Chrome => &["Google", "Chrome"],
                Browser::Brave => &["BraveSoftware", "Brave-Browser"],
                Browser::Edge => &["Microsoft Edge"],
                Browser::Helium => &["net.imput.helium"],
                Browser::Dia => &["Dia", "User Data"],
                Browser::Vivaldi => &["Vivaldi"],
                Browser::Opera => &["com.operasoftware.Opera"],
                Browser::OperaGx => &["com.operasoftware.OperaGX"],
                Browser::Comet => &["Comet"],
                Browser::Chromium => &["Chromium"],
                _ => return Vec::new(),
            };
            vec![under(locations.support(), parts)]
        }
        Os::Windows => {
            let local = |parts: &[&str]| {
                locations
                    .local_app_data
                    .clone()
                    .map(|base| under(base, parts))
            };
            let roaming =
                |parts: &[&str]| locations.app_data.clone().map(|base| under(base, parts));
            match browser {
                Browser::Chrome => local(&["Google", "Chrome", "User Data"])
                    .into_iter()
                    .collect(),
                Browser::Brave => local(&["BraveSoftware", "Brave-Browser", "User Data"])
                    .into_iter()
                    .collect(),
                Browser::Edge => local(&["Microsoft", "Edge", "User Data"])
                    .into_iter()
                    .collect(),
                Browser::Helium => local(&["imput", "Helium", "User Data"])
                    .into_iter()
                    .collect(),
                // A Store package, like Arc's.
                Browser::Dia => locations
                    .store_packages("TheBrowserCompany.Dia")
                    .into_iter()
                    .map(|package| under(package, &["LocalCache", "Local", "Dia", "User Data"]))
                    .chain(local(&["Dia", "User Data"]))
                    .collect(),
                Browser::Vivaldi => local(&["Vivaldi", "User Data"]).into_iter().collect(),
                Browser::Opera => roaming(&["Opera Software", "Opera Stable"])
                    .into_iter()
                    .collect(),
                Browser::OperaGx => roaming(&["Opera Software", "Opera GX Stable"])
                    .into_iter()
                    .collect(),
                Browser::Comet => local(&["Perplexity", "Comet", "User Data"])
                    .into_iter()
                    .collect(),
                Browser::Chromium => local(&["Chromium", "User Data"]).into_iter().collect(),
                _ => Vec::new(),
            }
        }
        Os::Linux => {
            let config = |parts: &[&str]| under(locations.home.join(".config"), parts);
            let flatpak = |app: &str, parts: &[&str]| {
                under(
                    locations
                        .home
                        .join(".var")
                        .join("app")
                        .join(app)
                        .join("config"),
                    parts,
                )
            };
            match browser {
                Browser::Chrome => vec![
                    config(&["google-chrome"]),
                    flatpak("com.google.Chrome", &["google-chrome"]),
                ],
                Browser::Brave => vec![
                    config(&["BraveSoftware", "Brave-Browser"]),
                    flatpak("com.brave.Browser", &["BraveSoftware", "Brave-Browser"]),
                ],
                Browser::Edge => vec![
                    config(&["microsoft-edge"]),
                    flatpak("com.microsoft.Edge", &["microsoft-edge"]),
                ],
                Browser::Helium => vec![config(&["net.imput.helium"])],
                Browser::Vivaldi => vec![
                    config(&["vivaldi"]),
                    flatpak("com.vivaldi.Vivaldi", &["vivaldi"]),
                ],
                Browser::Opera => vec![config(&["opera"]), flatpak("com.opera.Opera", &["opera"])],
                Browser::Chromium => vec![
                    config(&["chromium"]),
                    under(
                        locations.home.clone(),
                        &["snap", "chromium", "common", "chromium"],
                    ),
                    flatpak("org.chromium.Chromium", &["chromium"]),
                ],
                _ => Vec::new(),
            }
        }
    }
}

/// The first of the browser's roots with something to import, and its
/// profiles. Two installs of one browser would make profile ids ambiguous, so
/// only one is read.
pub(crate) fn installed(
    locations: &Locations,
    browser: Browser,
) -> Option<(PathBuf, Vec<Profile>)> {
    roots(locations, browser).into_iter().find_map(|root| {
        let found = profiles(&root);
        (!found.is_empty()).then_some((root, found))
    })
}

/// Profiles named in `Local State`, Default first, keeping only those with a
/// bookmarks file or a history database. A root with none of those but a
/// profile's files of its own is a single profile (Opera's layout).
pub(crate) fn profiles(root: &Path) -> Vec<Profile> {
    let named: Vec<(String, String)> = std::fs::read(root.join("Local State"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|state| {
            let cache = state.get("profile")?.get("info_cache")?.as_object()?;
            Some(
                cache
                    .iter()
                    .map(|(dir, info)| {
                        let name = info
                            .get("name")
                            .and_then(Value::as_str)
                            .filter(|name| !name.trim().is_empty())
                            .unwrap_or(dir);
                        (dir.clone(), name.to_owned())
                    })
                    .collect(),
            )
        })
        .unwrap_or_else(|| vec![("Default".into(), "Default".into())]);
    let mut profiles: Vec<Profile> = named
        .into_iter()
        .filter_map(|(id, name)| profile(&child(root, &id)?, id, name))
        .collect();
    profiles.sort_by(|a, b| (a.id != "Default", &a.id).cmp(&(b.id != "Default", &b.id)));
    if profiles.is_empty() {
        profiles.extend(profile(root, FLAT.into(), "Default".into()));
    }
    profiles
}

fn profile(dir: &Path, id: String, name: String) -> Option<Profile> {
    let bookmarks = BOOKMARK_FILES.iter().any(|file| dir.join(file).is_file());
    let history = dir.join("History").is_file();
    (bookmarks || history).then_some(Profile {
        id,
        name,
        bookmarks,
        history,
        essentials: false,
    })
}

pub(crate) fn profile_dir(
    locations: &Locations,
    browser: Browser,
    profile: &str,
) -> Result<PathBuf, ImportError> {
    let (root, known) = installed(locations, browser).ok_or(ImportError::Missing)?;
    // Only a profile this source still lists may be read.
    if !known.iter().any(|known| known.id == profile) {
        return Err(ImportError::Missing);
    }
    if profile == FLAT {
        return Ok(root);
    }
    child(&root, profile).ok_or(ImportError::Missing)
}

/// The bookmarks bar opens at the top of the import; the other roots keep
/// their own folder. The profile's file and its account's are read together:
/// either may hold everything the person sees, and a file that is there but
/// cannot be read is a failure, not an empty one.
pub(crate) fn bookmarks(profile: &Path) -> Result<Vec<ImportNode>, ImportError> {
    let mut files = Vec::new();
    for name in BOOKMARK_FILES {
        match std::fs::read(profile.join(name)) {
            Ok(bytes) => files.push(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    if files.is_empty() {
        return Err(ImportError::Missing);
    }
    parse_bookmark_files(&files.iter().map(Vec::as_slice).collect::<Vec<_>>())
}

#[cfg(test)]
pub(crate) fn parse_bookmarks(bytes: &[u8]) -> Result<Vec<ImportNode>, ImportError> {
    parse_bookmark_files(&[bytes])
}

/// What one root of the bookmarks tree holds across the files read.
#[derive(Default)]
struct Root {
    name: Option<String>,
    nodes: Vec<ImportNode>,
}

impl Root {
    fn add(&mut self, root: Option<&Value>) {
        let Some(root) = root else { return };
        if self.name.is_none() {
            self.name = root
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.trim().is_empty())
                .map(str::to_owned);
        }
        self.nodes.extend(children(root));
    }
}

pub(crate) fn parse_bookmark_files(files: &[&[u8]]) -> Result<Vec<ImportNode>, ImportError> {
    let (mut bar, mut other, mut mobile) = (Root::default(), Root::default(), Root::default());
    for bytes in files {
        let file: Value = serde_json::from_slice(bytes).map_err(|_| ImportError::Unreadable)?;
        let roots = file.get("roots").ok_or(ImportError::Unreadable)?;
        bar.add(roots.get("bookmark_bar"));
        other.add(roots.get("other"));
        mobile.add(roots.get("synced"));
    }
    let mut nodes = bar.nodes;
    for (root, fallback) in [(other, "Other bookmarks"), (mobile, "Mobile bookmarks")] {
        if !root.nodes.is_empty() {
            nodes.push(ImportNode::Folder {
                title: root.name.unwrap_or_else(|| fallback.to_owned()),
                children: root.nodes,
            });
        }
    }
    Ok(nodes)
}

fn children(folder: &Value) -> Vec<ImportNode> {
    folder
        .get("children")
        .and_then(Value::as_array)
        .map(|nodes| nodes.iter().filter_map(node).collect())
        .unwrap_or_default()
}

fn node(value: &Value) -> Option<ImportNode> {
    let title = value
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match value.get("type").and_then(Value::as_str)? {
        "url" => Some(ImportNode::Link {
            title: title.to_owned(),
            url: value.get("url")?.as_str()?.to_owned(),
        }),
        "folder" => Some(ImportNode::Folder {
            title: title.to_owned(),
            children: children(value),
        }),
        _ => None,
    }
}

pub(crate) fn history(
    profile: &Path,
    since: i64,
    limit: usize,
) -> Result<Vec<ImportedVisit>, ImportError> {
    let snapshot = snapshot::open(&profile.join("History"))?;
    let since = (since + EPOCH_OFFSET_SECONDS).saturating_mul(1_000_000);
    let mut rows = snapshot.conn.prepare(
        "SELECT u.url, u.title, v.visit_time FROM visits v JOIN urls u ON u.id = v.url
         WHERE v.visit_time >= ?1 ORDER BY v.visit_time DESC LIMIT ?2",
    )?;
    let visits = rows
        .query_map(rusqlite::params![since, limit as i64], |row| {
            Ok(ImportedVisit {
                url: row.get(0)?,
                title: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                visited_at: row.get::<_, i64>(2)? / 1_000_000 - EPOCH_OFFSET_SECONDS,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(visits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bar_opens_at_the_top_and_other_roots_keep_their_folder() {
        let file = br#"{
          "roots": {
            "bookmark_bar": { "children": [
              { "type": "url", "name": "Docs", "url": "https://docs.example/" },
              { "type": "folder", "name": "Work", "children": [
                { "type": "url", "name": "Mail", "url": "https://mail.example/" }
              ]}
            ], "name": "Bookmarks bar", "type": "folder" },
            "other": { "children": [
              { "type": "url", "name": "Later", "url": "https://later.example/" }
            ], "name": "Other bookmarks", "type": "folder" },
            "synced": { "children": [], "name": "Mobile bookmarks", "type": "folder" }
          },
          "version": 1
        }"#;
        let nodes = parse_bookmarks(file).unwrap();
        assert_eq!(
            nodes,
            vec![
                ImportNode::Link {
                    title: "Docs".into(),
                    url: "https://docs.example/".into()
                },
                ImportNode::Folder {
                    title: "Work".into(),
                    children: vec![ImportNode::Link {
                        title: "Mail".into(),
                        url: "https://mail.example/".into()
                    }]
                },
                ImportNode::Folder {
                    title: "Other bookmarks".into(),
                    children: vec![ImportNode::Link {
                        title: "Later".into(),
                        url: "https://later.example/".into()
                    }]
                },
            ]
        );
        assert!(parse_bookmarks(b"{}").is_err());
        assert!(parse_bookmarks(b"not json").is_err());
    }

    /// Shaped like a current Chrome on Windows with a Russian interface.
    fn chrome_file(bar: &str, other: &str, mobile: &str) -> String {
        let root = |id: u32, name: &str, children: &str| {
            format!(
                r#"{{"children":[{children}],"date_added":"13345678901234567",
                "date_last_used":"0","date_modified":"13345678901234567",
                "guid":"0bc5d13f-2cba-5d74-951f-3f233fe6c9{id:02}","id":"{id}",
                "name":"{name}","type":"folder"}}"#
            )
        };
        format!(
            r#"{{"checksum":"d41d8cd98f00b204e9800998ecf8427e","roots":{{
            "bookmark_bar":{},"other":{},"synced":{}}},
            "sync_metadata":"CgA=","version":1}}"#,
            root(1, "Панель закладок", bar),
            root(2, "Другие закладки", other),
            root(3, "Мобильные закладки", mobile),
        )
    }

    fn url(id: u32, title: &str, address: &str) -> String {
        format!(
            r#"{{"date_added":"13345678901234567","date_last_used":"0","guid":"6f1c2f0c-0000-4000-8000-0000000000{id:02}",
            "id":"{id}","meta_info":{{"power_bookmark_meta":""}},"name":"{title}","type":"url","url":"{address}"}}"#
        )
    }

    fn link(title: &str, address: &str) -> ImportNode {
        ImportNode::Link {
            title: title.into(),
            url: address.into(),
        }
    }

    #[test]
    fn a_signed_in_profile_keeps_its_bookmarks_in_the_account_file() {
        let profile = tempfile::tempdir().unwrap();
        // The profile's own file is there and empty, as Chrome leaves it once
        // everything lives with the account.
        std::fs::write(
            profile.path().join("Bookmarks"),
            chrome_file("", "", "").as_bytes(),
        )
        .unwrap();
        std::fs::write(
            profile.path().join("AccountBookmarks"),
            chrome_file(
                &[
                    url(5, "Яндекс Почта", "https://mail.yandex.ru/"),
                    url(
                        6,
                        "Википедия",
                        "https://ru.wikipedia.org/wiki/Заглавная_страница",
                    ),
                ]
                .join(","),
                &url(7, "Позже", "https://later.example/"),
                "",
            )
            .as_bytes(),
        )
        .unwrap();
        assert_eq!(
            bookmarks(profile.path()).unwrap(),
            vec![
                link("Яндекс Почта", "https://mail.yandex.ru/"),
                link(
                    "Википедия",
                    "https://ru.wikipedia.org/wiki/Заглавная_страница"
                ),
                ImportNode::Folder {
                    title: "Другие закладки".into(),
                    children: vec![link("Позже", "https://later.example/")]
                },
            ]
        );
    }

    #[test]
    fn the_profile_and_its_account_are_read_together_root_by_root() {
        let profile = tempfile::tempdir().unwrap();
        std::fs::write(
            profile.path().join("Bookmarks"),
            chrome_file(
                &url(5, "Local bar", "https://local.example/"),
                &url(6, "Local other", "https://local-other.example/"),
                "",
            )
            .as_bytes(),
        )
        .unwrap();
        std::fs::write(
            profile.path().join("AccountBookmarks"),
            chrome_file(
                &url(5, "Account bar", "https://account.example/"),
                &url(6, "Account other", "https://account-other.example/"),
                &url(7, "Phone", "https://phone.example/"),
            )
            .as_bytes(),
        )
        .unwrap();
        assert_eq!(
            bookmarks(profile.path()).unwrap(),
            vec![
                link("Local bar", "https://local.example/"),
                link("Account bar", "https://account.example/"),
                ImportNode::Folder {
                    title: "Другие закладки".into(),
                    children: vec![
                        link("Local other", "https://local-other.example/"),
                        link("Account other", "https://account-other.example/")
                    ]
                },
                ImportNode::Folder {
                    title: "Мобильные закладки".into(),
                    children: vec![link("Phone", "https://phone.example/")]
                },
            ]
        );
    }

    #[test]
    fn either_file_alone_is_enough_and_none_is_missing() {
        let profile = tempfile::tempdir().unwrap();
        assert!(matches!(
            bookmarks(profile.path()),
            Err(ImportError::Missing)
        ));
        std::fs::write(
            profile.path().join("Bookmarks"),
            chrome_file(&url(5, "A", "https://a.example/"), "", "").as_bytes(),
        )
        .unwrap();
        assert_eq!(
            bookmarks(profile.path()).unwrap(),
            vec![link("A", "https://a.example/")]
        );
        std::fs::remove_file(profile.path().join("Bookmarks")).unwrap();
        std::fs::write(
            profile.path().join("AccountBookmarks"),
            chrome_file(&url(5, "B", "https://b.example/"), "", "").as_bytes(),
        )
        .unwrap();
        assert_eq!(
            bookmarks(profile.path()).unwrap(),
            vec![link("B", "https://b.example/")]
        );
    }

    #[test]
    fn a_file_that_cannot_be_read_is_a_failure_not_an_empty_import() {
        let profile = tempfile::tempdir().unwrap();
        std::fs::write(
            profile.path().join("Bookmarks"),
            chrome_file(&url(5, "A", "https://a.example/"), "", "").as_bytes(),
        )
        .unwrap();
        std::fs::write(profile.path().join("AccountBookmarks"), b"{ truncated").unwrap();
        assert!(matches!(
            bookmarks(profile.path()),
            Err(ImportError::Unreadable)
        ));
    }

    #[test]
    fn a_profile_with_only_account_bookmarks_is_offered() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join("Local State"),
            br#"{"profile":{"info_cache":{"Default":{"name":"Person 1"}}}}"#,
        )
        .unwrap();
        std::fs::create_dir_all(root.path().join("Default")).unwrap();
        std::fs::write(root.path().join("Default").join("AccountBookmarks"), b"{}").unwrap();
        let found = profiles(root.path());
        assert_eq!(found.len(), 1);
        assert!(found[0].bookmarks && !found[0].history);
    }

    #[test]
    fn profiles_come_from_local_state_and_must_hold_something() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join("Local State"),
            br#"{"profile":{"info_cache":{
                "Profile 1":{"name":"Work"},
                "Default":{"name":"Personal"},
                "Profile 2":{"name":"Empty"},
                "../escape":{"name":"Escape"}
            }}}"#,
        )
        .unwrap();
        for (dir, file) in [("Default", "Bookmarks"), ("Profile 1", "History")] {
            std::fs::create_dir_all(root.path().join(dir)).unwrap();
            std::fs::write(root.path().join(dir).join(file), b"").unwrap();
        }
        std::fs::create_dir_all(root.path().join("Profile 2")).unwrap();
        let found = profiles(root.path());
        assert_eq!(
            found
                .iter()
                .map(|p| (p.id.as_str(), p.name.as_str(), p.bookmarks, p.history))
                .collect::<Vec<_>>(),
            vec![
                ("Default", "Personal", true, false),
                ("Profile 1", "Work", false, true)
            ]
        );
    }

    /// Where each browser's user-data directory sits under the home, spelled
    /// out apart from the code that looks there.
    const LAYOUTS: &[(Os, Browser, &str)] = &[
        (
            Os::Mac,
            Browser::Chrome,
            "Library/Application Support/Google/Chrome",
        ),
        (
            Os::Mac,
            Browser::Brave,
            "Library/Application Support/BraveSoftware/Brave-Browser",
        ),
        (
            Os::Mac,
            Browser::Edge,
            "Library/Application Support/Microsoft Edge",
        ),
        (
            Os::Mac,
            Browser::Helium,
            "Library/Application Support/net.imput.helium",
        ),
        (
            Os::Mac,
            Browser::Dia,
            "Library/Application Support/Dia/User Data",
        ),
        (
            Os::Mac,
            Browser::Vivaldi,
            "Library/Application Support/Vivaldi",
        ),
        (
            Os::Mac,
            Browser::Opera,
            "Library/Application Support/com.operasoftware.Opera",
        ),
        (
            Os::Mac,
            Browser::OperaGx,
            "Library/Application Support/com.operasoftware.OperaGX",
        ),
        (Os::Mac, Browser::Comet, "Library/Application Support/Comet"),
        (
            Os::Mac,
            Browser::Chromium,
            "Library/Application Support/Chromium",
        ),
        (
            Os::Windows,
            Browser::Chrome,
            "Local/Google/Chrome/User Data",
        ),
        (
            Os::Windows,
            Browser::Brave,
            "Local/BraveSoftware/Brave-Browser/User Data",
        ),
        (Os::Windows, Browser::Edge, "Local/Microsoft/Edge/User Data"),
        (Os::Windows, Browser::Helium, "Local/imput/Helium/User Data"),
        (Os::Windows, Browser::Dia, "Local/Dia/User Data"),
        (Os::Windows, Browser::Vivaldi, "Local/Vivaldi/User Data"),
        (
            Os::Windows,
            Browser::Opera,
            "Roaming/Opera Software/Opera Stable",
        ),
        (
            Os::Windows,
            Browser::OperaGx,
            "Roaming/Opera Software/Opera GX Stable",
        ),
        (
            Os::Windows,
            Browser::Comet,
            "Local/Perplexity/Comet/User Data",
        ),
        (Os::Windows, Browser::Chromium, "Local/Chromium/User Data"),
        (Os::Linux, Browser::Chrome, ".config/google-chrome"),
        (
            Os::Linux,
            Browser::Chrome,
            ".var/app/com.google.Chrome/config/google-chrome",
        ),
        (
            Os::Linux,
            Browser::Brave,
            ".config/BraveSoftware/Brave-Browser",
        ),
        (
            Os::Linux,
            Browser::Brave,
            ".var/app/com.brave.Browser/config/BraveSoftware/Brave-Browser",
        ),
        (Os::Linux, Browser::Edge, ".config/microsoft-edge"),
        (
            Os::Linux,
            Browser::Edge,
            ".var/app/com.microsoft.Edge/config/microsoft-edge",
        ),
        (Os::Linux, Browser::Helium, ".config/net.imput.helium"),
        (Os::Linux, Browser::Vivaldi, ".config/vivaldi"),
        (
            Os::Linux,
            Browser::Vivaldi,
            ".var/app/com.vivaldi.Vivaldi/config/vivaldi",
        ),
        (Os::Linux, Browser::Opera, ".config/opera"),
        (
            Os::Linux,
            Browser::Opera,
            ".var/app/com.opera.Opera/config/opera",
        ),
        (Os::Linux, Browser::Chromium, ".config/chromium"),
        (
            Os::Linux,
            Browser::Chromium,
            "snap/chromium/common/chromium",
        ),
        (
            Os::Linux,
            Browser::Chromium,
            ".var/app/org.chromium.Chromium/config/chromium",
        ),
    ];

    const CHROMIUM_BASED: [Browser; 10] = [
        Browser::Chrome,
        Browser::Brave,
        Browser::Edge,
        Browser::Helium,
        Browser::Dia,
        Browser::Vivaldi,
        Browser::Opera,
        Browser::OperaGx,
        Browser::Comet,
        Browser::Chromium,
    ];

    fn history_file(profile: &Path) {
        std::fs::create_dir_all(profile).unwrap();
        std::fs::write(profile.join("History"), b"").unwrap();
    }

    #[test]
    fn each_browser_is_found_where_it_keeps_its_data_and_nowhere_else() {
        for (os, browser, tree) in LAYOUTS {
            let home = tempfile::tempdir().unwrap();
            history_file(&home.path().join(tree).join("Default"));
            let locations = Locations::scratch(*os, home.path());
            let (root, found) = installed(&locations, *browser)
                .unwrap_or_else(|| panic!("{} on {os:?} at {tree}", browser.name()));
            assert_eq!(root, home.path().join(tree));
            assert_eq!(found.len(), 1);
            assert_eq!(found[0].id, "Default");
            assert_eq!(
                profile_dir(&locations, *browser, "Default").unwrap(),
                root.join("Default")
            );
            for other in CHROMIUM_BASED.into_iter().filter(|other| other != browser) {
                assert!(
                    installed(&locations, other).is_none(),
                    "{} on {os:?} sees {}'s data",
                    other.name(),
                    browser.name()
                );
            }
        }
    }

    #[test]
    fn a_browser_is_only_looked_for_on_the_platforms_it_exists_on() {
        let home = tempfile::tempdir().unwrap();
        let locations = |os| Locations::scratch(os, home.path());
        for browser in [Browser::Dia, Browser::Comet, Browser::OperaGx] {
            assert!(
                roots(&locations(Os::Linux), browser).is_empty(),
                "{browser:?}"
            );
        }
        for browser in CHROMIUM_BASED {
            for os in [Os::Mac, Os::Windows] {
                assert!(
                    !roots(&locations(os), browser).is_empty(),
                    "{browser:?} {os:?}"
                );
            }
        }
        assert!(roots(&locations(Os::Mac), Browser::Firefox).is_empty());
        assert!(roots(&locations(Os::Mac), Browser::Arc).is_empty());
    }

    #[test]
    fn dia_for_windows_is_a_store_package_found_by_its_name() {
        let home = tempfile::tempdir().unwrap();
        let package = home
            .path()
            .join("Local/Packages/TheBrowserCompany.Dia_ttt1ap7aakyb4");
        history_file(&package.join("LocalCache/Local/Dia/User Data/Default"));
        // Another app of the same publisher is not Dia.
        history_file(
            &home
                .path()
                .join("Local/Packages/TheBrowserCompany.Arc_ttt1ap7aakyb4")
                .join("LocalCache/Local/Dia/User Data/Default"),
        );
        let locations = Locations::scratch(Os::Windows, home.path());
        let (root, found) = installed(&locations, Browser::Dia).unwrap();
        assert_eq!(root, package.join("LocalCache/Local/Dia/User Data"));
        assert_eq!(found[0].id, "Default");
    }

    #[test]
    fn the_first_install_with_something_to_import_is_the_one_read() {
        let home = tempfile::tempdir().unwrap();
        let locations = Locations::scratch(Os::Linux, home.path());
        // A native install left empty, and a Flatpak that holds the data.
        std::fs::create_dir_all(home.path().join(".config/chromium/Default")).unwrap();
        let flatpak = home
            .path()
            .join(".var/app/org.chromium.Chromium/config/chromium");
        history_file(&flatpak.join("Default"));
        assert_eq!(installed(&locations, Browser::Chromium).unwrap().0, flatpak);
    }

    #[test]
    fn opera_may_keep_its_one_profile_in_the_user_data_directory_itself() {
        let home = tempfile::tempdir().unwrap();
        let locations = Locations::scratch(Os::Linux, home.path());
        let root = home.path().join(".config/opera");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("Bookmarks"), b"{}").unwrap();
        history_file(&root);
        let (_, found) = installed(&locations, Browser::Opera).unwrap();
        assert_eq!(
            found
                .iter()
                .map(|p| (p.id.as_str(), p.bookmarks, p.history))
                .collect::<Vec<_>>(),
            vec![(".", true, true)]
        );
        assert_eq!(profile_dir(&locations, Browser::Opera, ".").unwrap(), root);

        // Once it lists a profile of its own, the root's loose files are old ones.
        std::fs::write(
            root.join("Local State"),
            br#"{"profile":{"info_cache":{"Default":{"name":"Person 1"}}}}"#,
        )
        .unwrap();
        history_file(&root.join("Default"));
        let (_, found) = installed(&locations, Browser::Opera).unwrap();
        assert_eq!(
            found.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
            ["Default"]
        );
        assert!(profile_dir(&locations, Browser::Opera, ".").is_err());
    }

    #[test]
    fn history_reads_a_copy_with_unix_times_newest_first() {
        let profile = tempfile::tempdir().unwrap();
        let conn = rusqlite::Connection::open(profile.path().join("History")).unwrap();
        conn.execute_batch(
            "CREATE TABLE urls(id INTEGER PRIMARY KEY, url TEXT, title TEXT);
             CREATE TABLE visits(id INTEGER PRIMARY KEY, url INTEGER, visit_time INTEGER);",
        )
        .unwrap();
        conn.execute("INSERT INTO urls VALUES (1, 'https://a.example/', 'A')", [])
            .unwrap();
        for (id, unix) in [(1, 1_700_000_000_i64), (2, 1_700_000_900), (3, 1_000)] {
            conn.execute(
                "INSERT INTO visits VALUES (?1, 1, ?2)",
                rusqlite::params![id, (unix + EPOCH_OFFSET_SECONDS) * 1_000_000],
            )
            .unwrap();
        }
        drop(conn);
        let visits = history(profile.path(), 1_600_000_000, 10).unwrap();
        assert_eq!(
            visits.iter().map(|v| v.visited_at).collect::<Vec<_>>(),
            vec![1_700_000_900, 1_700_000_000]
        );
        assert_eq!(visits[0].title, "A");
    }
}
