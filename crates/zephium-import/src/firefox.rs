//! Firefox lists profiles in `profiles.ini`; each keeps bookmarks and history
//! together in `places.sqlite`. Zen, Floorp, LibreWolf and Waterfox are built
//! on Firefox and keep the same files in a tree of their own.

use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};

use crate::{snapshot, Browser, ImportError, ImportNode, ImportedVisit, Locations, Os, Profile};

/// Where each browser keeps its profile tree (the directory holding
/// `profiles.ini`), most current first. On Linux the same browser may be a
/// native package, a Flatpak or a Snap, and a tree may sit in the XDG config
/// directory or in the older dotted directory of the home. A tree is used
/// only where `profiles.ini` is.
fn roots(locations: &Locations, browser: Browser) -> Vec<PathBuf> {
    let home = &locations.home;
    let candidates = match locations.os {
        Os::Mac => {
            let name = match browser {
                Browser::Firefox => "Firefox",
                Browser::Zen => "zen",
                Browser::Floorp => "Floorp",
                Browser::LibreWolf => "LibreWolf",
                Browser::Waterfox => "Waterfox",
                _ => return Vec::new(),
            };
            let mut roots = vec![locations.support().join(name)];
            if browser == Browser::LibreWolf {
                roots.push(home.join(".librewolf"));
            }
            roots
        }
        Os::Windows => {
            let parts: &[&str] = match browser {
                Browser::Firefox => &["Mozilla", "Firefox"],
                Browser::Zen => &["zen"],
                Browser::Floorp => &["Floorp"],
                Browser::LibreWolf => &["LibreWolf"],
                Browser::Waterfox => &["Waterfox"],
                _ => return Vec::new(),
            };
            let mut roots: Vec<PathBuf> = locations
                .app_data
                .iter()
                .map(|app_data| {
                    parts
                        .iter()
                        .fold(app_data.clone(), |path, part| path.join(part))
                })
                .collect();
            if browser == Browser::Firefox {
                // The Store build keeps Roaming inside its package.
                roots.extend(locations.store_packages("Mozilla.Firefox").into_iter().map(
                    |package| {
                        package
                            .join("LocalCache")
                            .join("Roaming")
                            .join("Mozilla")
                            .join("Firefox")
                    },
                ));
            }
            roots
        }
        Os::Linux => {
            let config = home.join(".config");
            let flatpak = |app: &str, parts: &[&str]| {
                parts
                    .iter()
                    .fold(home.join(".var").join("app").join(app), |path, part| {
                        path.join(part)
                    })
            };
            match browser {
                Browser::Firefox => vec![
                    // Firefox 147 puts new installs here; older ones stay put.
                    config.join("mozilla").join("firefox"),
                    home.join(".mozilla").join("firefox"),
                    flatpak("org.mozilla.firefox", &["config", "mozilla", "firefox"]),
                    flatpak("org.mozilla.firefox", &[".mozilla", "firefox"]),
                    // The Snap package keeps its own copy of the profile tree.
                    home.join("snap")
                        .join("firefox")
                        .join("common")
                        .join(".mozilla")
                        .join("firefox"),
                ],
                Browser::Zen => vec![
                    config.join("zen"),
                    home.join(".zen"),
                    // The Flatpak keeps its tree inside the app's own home.
                    flatpak("app.zen_browser.zen", &[".zen"]),
                    flatpak("io.github.zen_browser.zen", &[".zen"]),
                ],
                Browser::Floorp => vec![
                    home.join(".floorp"),
                    flatpak("one.ablaze.floorp", &[".floorp"]),
                ],
                Browser::LibreWolf => vec![
                    config.join("librewolf").join("librewolf"),
                    home.join(".librewolf"),
                    flatpak("io.gitlab.librewolf-community", &[".librewolf"]),
                ],
                Browser::Waterfox => vec![
                    home.join(".waterfox"),
                    flatpak("net.waterfox.waterfox", &[".waterfox"]),
                ],
                _ => return Vec::new(),
            }
        }
    };
    candidates
        .into_iter()
        .filter(|root| root.join("profiles.ini").is_file())
        .collect()
}

struct Listed {
    path: String,
    name: String,
    dir: PathBuf,
    default: bool,
}

/// Profile groups read from one tree; a real one has a handful of files.
const MAX_GROUPS: usize = 8;

/// The profiles of `profiles.ini` in file order, then the rest of any profile
/// group. Only paths listed here are ever read, so a profile id from chrome
/// cannot name another directory.
fn listed(root: &Path) -> Vec<Listed> {
    let mut profiles = ini_profiles(root);
    profiles.extend(grouped(root));
    profiles
}

/// Profiles made with Firefox's profile groups are kept in a database per
/// group, `Profile Groups/<store id>.sqlite`. `profiles.ini` lists only the
/// most recent profile of each group, with a path relative to the tree.
fn grouped(root: &Path) -> Vec<Listed> {
    let mut stores: Vec<PathBuf> = std::fs::read_dir(root.join("Profile Groups"))
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "sqlite")
        })
        .collect();
    stores.sort();
    stores.truncate(MAX_GROUPS);
    stores
        .iter()
        .filter_map(|store| {
            let snapshot = snapshot::open(store).ok()?;
            let mut rows = snapshot
                .conn
                .prepare("SELECT path, name FROM Profiles ORDER BY id")
                .ok()?;
            let found: Vec<(String, String)> = rows
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .ok()?
                .filter_map(Result::ok)
                .collect();
            Some(found)
        })
        .flatten()
        // The database is another program's file: a path may not leave the tree.
        .filter(|(path, _)| {
            !path.is_empty()
                && Path::new(path)
                    .components()
                    .all(|part| matches!(part, Component::Normal(_)))
        })
        .map(|(path, name)| Listed {
            dir: root.join(&path),
            name,
            path,
            default: false,
        })
        .collect()
}

fn ini_profiles(root: &Path) -> Vec<Listed> {
    let Ok(text) = std::fs::read_to_string(root.join("profiles.ini")) else {
        return Vec::new();
    };
    let mut sections: Vec<(String, HashMap<String, String>)> = Vec::new();
    for line in text.lines().map(str::trim) {
        if let Some(name) = line
            .strip_prefix('[')
            .and_then(|line| line.strip_suffix(']'))
        {
            sections.push((name.to_owned(), HashMap::new()));
        } else if let (Some((key, value)), Some((_, values))) =
            (line.split_once('='), sections.last_mut())
        {
            values.insert(key.trim().to_owned(), value.trim().to_owned());
        }
    }
    let installed_default: HashSet<&str> = sections
        .iter()
        .filter(|(name, _)| name.starts_with("Install"))
        .filter_map(|(_, values)| values.get("Default").map(String::as_str))
        .collect();
    sections
        .iter()
        .filter(|(name, _)| name.starts_with("Profile"))
        .filter_map(|(_, values)| {
            let path = values.get("Path")?.clone();
            let relative = values.get("IsRelative").is_none_or(|value| value == "1");
            let dir = if relative {
                root.join(&path)
            } else {
                PathBuf::from(&path)
            };
            Some(Listed {
                name: values.get("Name").cloned().unwrap_or_else(|| path.clone()),
                default: installed_default.contains(path.as_str())
                    || values.get("Default").is_some_and(|value| value == "1"),
                path,
                dir,
            })
        })
        .collect()
}

pub(crate) fn profiles(locations: &Locations, browser: Browser) -> Vec<Profile> {
    let mut profiles: Vec<(bool, Profile)> = roots(locations, browser)
        .iter()
        .flat_map(|root| listed(root))
        .filter(|listed| listed.dir.join("places.sqlite").is_file())
        .map(|listed| {
            (
                listed.default,
                Profile {
                    essentials: browser == Browser::Zen && crate::zen::has_session(&listed.dir),
                    id: listed.path,
                    name: listed.name,
                    bookmarks: true,
                    history: true,
                },
            )
        })
        .collect();
    profiles.sort_by_key(|(default, _)| !*default);
    let mut seen = HashSet::new();
    profiles
        .into_iter()
        .map(|(_, profile)| profile)
        .filter(|profile| seen.insert(profile.id.clone()))
        .collect()
}

pub(crate) fn profile_dir(
    locations: &Locations,
    browser: Browser,
    profile: &str,
) -> Result<PathBuf, ImportError> {
    roots(locations, browser)
        .iter()
        .flat_map(|root| listed(root))
        .find(|listed| listed.path == profile)
        .map(|listed| listed.dir)
        .ok_or(ImportError::Missing)
}

struct Row {
    kind: i64,
    title: String,
    url: Option<String>,
}

/// The toolbar opens at the top of the import; the menu, other and mobile
/// roots keep their own folder.
pub(crate) fn bookmarks(profile: &Path) -> Result<Vec<ImportNode>, ImportError> {
    let snapshot = snapshot::open(&profile.join("places.sqlite"))?;
    let mut statement = snapshot.conn.prepare(
        "SELECT b.id, b.parent, b.type, coalesce(b.title, ''), p.url, b.guid
         FROM moz_bookmarks b LEFT JOIN moz_places p ON p.id = b.fk
         ORDER BY b.parent, b.position",
    )?;
    let mut rows: HashMap<i64, Row> = HashMap::new();
    let mut children: HashMap<i64, Vec<i64>> = HashMap::new();
    let mut roots: HashMap<String, i64> = HashMap::new();
    let mut query = statement.query([])?;
    while let Some(row) = query.next()? {
        let id: i64 = row.get(0)?;
        let parent: Option<i64> = row.get(1)?;
        roots.insert(row.get::<_, String>(5)?, id);
        rows.insert(
            id,
            Row {
                kind: row.get(2)?,
                title: row.get(3)?,
                url: row.get(4)?,
            },
        );
        if let Some(parent) = parent {
            children.entry(parent).or_default().push(id);
        }
        if rows.len() > crate::MAX_NODES * 2 {
            break;
        }
    }
    let tree = Tree {
        rows: &rows,
        children: &children,
    };
    let mut nodes = roots
        .get("toolbar_____")
        .map(|id| tree.children(*id, 0))
        .unwrap_or_default();
    for (guid, title) in [
        ("menu________", "Bookmarks Menu"),
        ("unfiled_____", "Other Bookmarks"),
        ("mobile______", "Mobile Bookmarks"),
    ] {
        let inside = roots
            .get(guid)
            .map(|id| tree.children(*id, 0))
            .unwrap_or_default();
        if !inside.is_empty() {
            nodes.push(ImportNode::Folder {
                title: title.into(),
                children: inside,
            });
        }
    }
    Ok(nodes)
}

struct Tree<'a> {
    rows: &'a HashMap<i64, Row>,
    children: &'a HashMap<i64, Vec<i64>>,
}

impl Tree<'_> {
    /// `depth` stops a damaged file whose parents form a loop.
    fn children(&self, id: i64, depth: usize) -> Vec<ImportNode> {
        if depth > crate::MAX_TREE_DEPTH {
            return Vec::new();
        }
        self.children
            .get(&id)
            .into_iter()
            .flatten()
            .filter_map(|child| {
                let row = self.rows.get(child)?;
                match row.kind {
                    1 => Some(ImportNode::Link {
                        title: row.title.clone(),
                        url: row.url.clone()?,
                    }),
                    2 => Some(ImportNode::Folder {
                        title: row.title.clone(),
                        children: self.children(*child, depth + 1),
                    }),
                    // Separators carry nothing to keep.
                    _ => None,
                }
            })
            .collect()
    }
}

pub(crate) fn history(
    profile: &Path,
    since: i64,
    limit: usize,
) -> Result<Vec<ImportedVisit>, ImportError> {
    let snapshot = snapshot::open(&profile.join("places.sqlite"))?;
    let mut rows = snapshot.conn.prepare(
        "SELECT p.url, coalesce(p.title, ''), v.visit_date
         FROM moz_historyvisits v JOIN moz_places p ON p.id = v.place_id
         WHERE v.visit_date >= ?1 ORDER BY v.visit_date DESC LIMIT ?2",
    )?;
    let visits = rows
        .query_map(
            rusqlite::params![since.saturating_mul(1_000_000), limit as i64],
            |row| {
                Ok(ImportedVisit {
                    url: row.get(0)?,
                    title: row.get(1)?,
                    visited_at: row.get::<_, i64>(2)? / 1_000_000,
                })
            },
        )?
        .collect::<rusqlite::Result<_>>()?;
    Ok(visits)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn places(dir: &Path) {
        let conn = rusqlite::Connection::open(dir.join("places.sqlite")).unwrap();
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE moz_places(id INTEGER PRIMARY KEY, url TEXT, title TEXT);
             CREATE TABLE moz_bookmarks(id INTEGER PRIMARY KEY, type INTEGER, fk INTEGER,
                 parent INTEGER, position INTEGER, title TEXT, guid TEXT);
             CREATE TABLE moz_historyvisits(id INTEGER PRIMARY KEY, place_id INTEGER, visit_date INTEGER);
             INSERT INTO moz_places VALUES (1, 'https://news.example/', 'News'),
                                           (2, 'https://docs.example/', NULL);
             INSERT INTO moz_bookmarks VALUES
                 (1, 2, NULL, NULL, 0, '', 'root________'),
                 (2, 2, NULL, 1, 0, 'menu', 'menu________'),
                 (3, 2, NULL, 1, 1, 'toolbar', 'toolbar_____'),
                 (4, 2, NULL, 1, 2, 'unfiled', 'unfiled_____'),
                 (5, 1, 1, 3, 0, 'News', 'a'),
                 (6, 3, NULL, 3, 1, NULL, 'b'),
                 (7, 2, NULL, 3, 2, 'Reading', 'c'),
                 (8, 1, 2, 7, 0, 'Docs', 'd'),
                 (9, 1, 2, 2, 0, 'Docs in menu', 'e');
             INSERT INTO moz_historyvisits VALUES (1, 1, 1700000000000000), (2, 2, 1700000600000000);",
        )
        .unwrap();
    }

    #[test]
    fn the_toolbar_opens_at_the_top_and_separators_are_dropped() {
        let dir = tempfile::tempdir().unwrap();
        places(dir.path());
        let link = |title: &str, url: &str| ImportNode::Link {
            title: title.into(),
            url: url.into(),
        };
        assert_eq!(
            bookmarks(dir.path()).unwrap(),
            vec![
                link("News", "https://news.example/"),
                ImportNode::Folder {
                    title: "Reading".into(),
                    children: vec![link("Docs", "https://docs.example/")]
                },
                ImportNode::Folder {
                    title: "Bookmarks Menu".into(),
                    children: vec![link("Docs in menu", "https://docs.example/")]
                },
            ]
        );
        let visits = history(dir.path(), 1_600_000_000, 10).unwrap();
        assert_eq!(visits[0].visited_at, 1_700_000_600);
        assert_eq!(visits[1].title, "News");
    }

    #[test]
    fn profiles_ini_names_profiles_and_the_installed_default_comes_first() {
        for (os, tree) in [
            (Os::Mac, "Library/Application Support/Firefox"),
            (Os::Windows, "Roaming/Mozilla/Firefox"),
            (Os::Linux, ".mozilla/firefox"),
        ] {
            let home = tempfile::tempdir().unwrap();
            let root = home.path().join(tree);
            for profile in ["Profiles/old.default", "Profiles/new.default-release"] {
                std::fs::create_dir_all(root.join(profile)).unwrap();
                places(&root.join(profile));
            }
            std::fs::write(
                root.join("profiles.ini"),
                "[Profile1]\nName=default\nIsRelative=1\nPath=Profiles/old.default\n\n\
                 [Profile0]\nName=default-release\nIsRelative=1\nPath=Profiles/new.default-release\n\n\
                 [Install4F96D1932A9F858E]\nDefault=Profiles/new.default-release\n",
            )
            .unwrap();
            let locations = Locations::scratch(os, home.path());
            let found = profiles(&locations, Browser::Firefox);
            assert_eq!(
                found.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
                vec!["Profiles/new.default-release", "Profiles/old.default"],
                "{os:?}"
            );
            assert!(profile_dir(&locations, Browser::Firefox, "Profiles/old.default").is_ok());
            assert!(profile_dir(&locations, Browser::Firefox, "../../etc").is_err());
            assert!(profiles(&locations, Browser::Zen).is_empty());
        }
    }

    /// Where each browser's profile tree sits under the home, spelled out
    /// apart from the code that looks there.
    const LAYOUTS: &[(Os, Browser, &str)] = &[
        (
            Os::Mac,
            Browser::Firefox,
            "Library/Application Support/Firefox",
        ),
        (Os::Mac, Browser::Zen, "Library/Application Support/zen"),
        (
            Os::Mac,
            Browser::Floorp,
            "Library/Application Support/Floorp",
        ),
        (
            Os::Mac,
            Browser::LibreWolf,
            "Library/Application Support/LibreWolf",
        ),
        (Os::Mac, Browser::LibreWolf, ".librewolf"),
        (
            Os::Mac,
            Browser::Waterfox,
            "Library/Application Support/Waterfox",
        ),
        (Os::Windows, Browser::Firefox, "Roaming/Mozilla/Firefox"),
        (
            Os::Windows,
            Browser::Firefox,
            "Local/Packages/Mozilla.Firefox_n80bbvh6b1yt2/LocalCache/Roaming/Mozilla/Firefox",
        ),
        (Os::Windows, Browser::Zen, "Roaming/zen"),
        (Os::Windows, Browser::Floorp, "Roaming/Floorp"),
        (Os::Windows, Browser::LibreWolf, "Roaming/LibreWolf"),
        (Os::Windows, Browser::Waterfox, "Roaming/Waterfox"),
        (Os::Linux, Browser::Firefox, ".config/mozilla/firefox"),
        (Os::Linux, Browser::Firefox, ".mozilla/firefox"),
        (
            Os::Linux,
            Browser::Firefox,
            ".var/app/org.mozilla.firefox/config/mozilla/firefox",
        ),
        (
            Os::Linux,
            Browser::Firefox,
            ".var/app/org.mozilla.firefox/.mozilla/firefox",
        ),
        (
            Os::Linux,
            Browser::Firefox,
            "snap/firefox/common/.mozilla/firefox",
        ),
        (Os::Linux, Browser::Zen, ".config/zen"),
        (Os::Linux, Browser::Zen, ".zen"),
        (Os::Linux, Browser::Zen, ".var/app/app.zen_browser.zen/.zen"),
        (
            Os::Linux,
            Browser::Zen,
            ".var/app/io.github.zen_browser.zen/.zen",
        ),
        (Os::Linux, Browser::Floorp, ".floorp"),
        (
            Os::Linux,
            Browser::Floorp,
            ".var/app/one.ablaze.floorp/.floorp",
        ),
        (Os::Linux, Browser::LibreWolf, ".config/librewolf/librewolf"),
        (Os::Linux, Browser::LibreWolf, ".librewolf"),
        (
            Os::Linux,
            Browser::LibreWolf,
            ".var/app/io.gitlab.librewolf-community/.librewolf",
        ),
        (Os::Linux, Browser::Waterfox, ".waterfox"),
        (
            Os::Linux,
            Browser::Waterfox,
            ".var/app/net.waterfox.waterfox/.waterfox",
        ),
    ];

    const FIREFOX_BASED: [Browser; 5] = [
        Browser::Firefox,
        Browser::Zen,
        Browser::Floorp,
        Browser::LibreWolf,
        Browser::Waterfox,
    ];

    #[test]
    fn each_browser_is_found_where_it_keeps_its_tree_and_nowhere_else() {
        for (os, browser, tree) in LAYOUTS {
            let home = tempfile::tempdir().unwrap();
            let root = home.path().join(tree);
            // Zen's XDG tree keeps profiles beside profiles.ini, not in Profiles/.
            let profile = "k3x9q7ab.Default (release)";
            std::fs::create_dir_all(root.join(profile)).unwrap();
            places(&root.join(profile));
            std::fs::write(
                root.join("profiles.ini"),
                format!(
                    "[Profile0]\nName=Default (release)\nIsRelative=1\nPath={profile}\nDefault=1\n"
                ),
            )
            .unwrap();
            let locations = Locations::scratch(*os, home.path());
            let found = profiles(&locations, *browser);
            assert_eq!(
                found.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
                [profile],
                "{} on {os:?} at {tree}",
                browser.name()
            );
            assert_eq!(
                profile_dir(&locations, *browser, profile).unwrap(),
                root.join(profile)
            );
            for other in FIREFOX_BASED.into_iter().filter(|other| other != browser) {
                assert!(
                    profiles(&locations, other).is_empty(),
                    "{} on {os:?} sees {}'s profiles",
                    other.name(),
                    browser.name()
                );
            }
        }
    }

    #[test]
    fn trees_in_both_the_xdg_and_the_older_place_are_read_together() {
        let home = tempfile::tempdir().unwrap();
        for (tree, profile) in [
            (".config/mozilla/firefox", "new1.default-release"),
            (".mozilla/firefox", "old1.default"),
        ] {
            let root = home.path().join(tree);
            std::fs::create_dir_all(root.join(profile)).unwrap();
            places(&root.join(profile));
            std::fs::write(
                root.join("profiles.ini"),
                format!("[Profile0]\nName=main\nIsRelative=1\nPath={profile}\n"),
            )
            .unwrap();
        }
        let locations = Locations::scratch(Os::Linux, home.path());
        let found = profiles(&locations, Browser::Firefox);
        assert_eq!(
            found.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
            ["new1.default-release", "old1.default"]
        );
        assert!(profile_dir(&locations, Browser::Firefox, "old1.default").is_ok());
    }

    fn group_store(root: &Path, rows: &[(&str, &str)]) {
        std::fs::create_dir_all(root.join("Profile Groups")).unwrap();
        let conn = rusqlite::Connection::open(root.join("Profile Groups").join("6f2a0c1b.sqlite"))
            .unwrap();
        conn.execute_batch(
            "CREATE TABLE Profiles(id INTEGER NOT NULL, path TEXT NOT NULL UNIQUE,
                 name TEXT NOT NULL, avatar TEXT NOT NULL, themeId TEXT NOT NULL,
                 themeFg TEXT NOT NULL, themeBg TEXT NOT NULL, PRIMARY KEY(id));",
        )
        .unwrap();
        for (path, name) in rows {
            conn.execute(
                "INSERT INTO Profiles(path, name, avatar, themeId, themeFg, themeBg)
                 VALUES (?1, ?2, 'book', 'default', 'x', 'y')",
                [path, name],
            )
            .unwrap();
        }
    }

    #[test]
    fn profiles_made_in_a_group_are_listed_though_profiles_ini_names_only_one() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("Library/Application Support/zen");
        for profile in [
            "Profiles/aaa.Work",
            "Profiles/bbb.Home",
            "Profiles/ccc.Gone",
        ] {
            std::fs::create_dir_all(root.join(profile)).unwrap();
            places(&root.join(profile));
        }
        // Only Work is in profiles.ini; the group's database knows them all.
        std::fs::write(
            root.join("profiles.ini"),
            "[Profile0]\nName=Work\nIsRelative=1\nPath=Profiles/aaa.Work\nStoreID=6f2a0c1b\n",
        )
        .unwrap();
        std::fs::remove_file(root.join("Profiles/ccc.Gone/places.sqlite")).unwrap();
        group_store(
            &root,
            &[
                ("Profiles/aaa.Work", "Work"),
                ("Profiles/bbb.Home", "Home"),
                ("Profiles/ccc.Gone", "Gone"),
                // The database is another program's file: nothing may leave the tree.
                ("../../etc", "Up"),
                ("/etc", "Absolute"),
                ("", "Nowhere"),
            ],
        );
        let locations = Locations::scratch(Os::Mac, home.path());
        let found = profiles(&locations, Browser::Zen);
        assert_eq!(
            found
                .iter()
                .map(|p| (p.id.as_str(), p.name.as_str()))
                .collect::<Vec<_>>(),
            [("Profiles/aaa.Work", "Work"), ("Profiles/bbb.Home", "Home")]
        );
        assert_eq!(
            profile_dir(&locations, Browser::Zen, "Profiles/bbb.Home").unwrap(),
            root.join("Profiles/bbb.Home")
        );
        for refused in ["../../etc", "/etc", ""] {
            assert!(
                profile_dir(&locations, Browser::Zen, refused).is_err(),
                "{refused}"
            );
        }
    }

    #[test]
    fn a_damaged_group_database_leaves_profiles_ini_alone() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join(".mozilla/firefox");
        std::fs::create_dir_all(root.join("Profile Groups")).unwrap();
        std::fs::write(
            root.join("Profile Groups/6f2a0c1b.sqlite"),
            b"not a database",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("a.default")).unwrap();
        places(&root.join("a.default"));
        std::fs::write(
            root.join("profiles.ini"),
            "[Profile0]\nName=main\nIsRelative=1\nPath=a.default\n",
        )
        .unwrap();
        let locations = Locations::scratch(Os::Linux, home.path());
        assert_eq!(profiles(&locations, Browser::Firefox).len(), 1);
    }
}
