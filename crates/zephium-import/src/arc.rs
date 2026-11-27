//! Arc keeps what its people think of as bookmarks in its sidebar, not in
//! Chromium's bookmarks file: each space's pinned tabs (with folders) and the
//! favorites above them, in `StorableSidebar.json`. Favorites come over as
//! Essentials, pinned tabs as bookmarks. History is Chromium's.
//!
//! Arc's profiles are Chromium profiles: every space and every row of
//! favorites belongs to one, and each has its own history.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use serde_json::Value;

use crate::{child, chromium, Essential, ImportError, ImportNode, Locations, Os, Profile};

/// The profile of a space that names none, and of Arc's first.
const DEFAULT: &str = "Default";
/// Arc runs its own pages in a profile the person never made or sees.
const SYSTEM_PROFILE: &str = "__ARC_SYSTEM_PROFILE";

fn root(locations: &Locations) -> Option<PathBuf> {
    match locations.os {
        Os::Mac => Some(locations.support().join("Arc")).filter(|root| root.is_dir()),
        // Arc for Windows is a Store package.
        Os::Windows => locations
            .store_packages("TheBrowserCompany.Arc")
            .into_iter()
            .map(|package| package.join("LocalCache").join("Local").join("Arc"))
            .find(|root| root.is_dir()),
        Os::Linux => None,
    }
}

pub(crate) fn profiles(locations: &Locations) -> Vec<Profile> {
    let Some(root) = root(locations) else {
        return Vec::new();
    };
    let sidebar = root.join("StorableSidebar.json").is_file();
    let mut profiles: Vec<Profile> = chromium::profiles(&root.join("User Data"))
        .into_iter()
        .filter(|profile| profile.name != SYSTEM_PROFILE)
        // Chromium's bookmarks file is not where Arc keeps them.
        .map(|profile| Profile {
            bookmarks: sidebar,
            essentials: sidebar,
            ..profile
        })
        .collect();
    // A sidebar before any history: the first profile is still there.
    if profiles.is_empty() && sidebar {
        profiles.push(Profile {
            id: DEFAULT.into(),
            name: "Arc".into(),
            bookmarks: true,
            history: false,
            essentials: true,
        });
    }
    profiles
}

/// Only a profile Arc still lists may be read.
fn listed(locations: &Locations, profile: &str) -> Result<PathBuf, ImportError> {
    if !profiles(locations).iter().any(|known| known.id == profile) {
        return Err(ImportError::Missing);
    }
    root(locations).ok_or(ImportError::Missing)
}

pub(crate) fn profile_dir(locations: &Locations, profile: &str) -> Result<PathBuf, ImportError> {
    let root = listed(locations, profile)?;
    child(&root.join("User Data"), profile).ok_or(ImportError::Missing)
}

fn sidebar(locations: &Locations, profile: &str) -> Result<Sidebar, ImportError> {
    let root = listed(locations, profile)?;
    parse_sidebar(&std::fs::read(root.join("StorableSidebar.json"))?, profile)
}

pub(crate) fn bookmarks(
    locations: &Locations,
    profile: &str,
) -> Result<Vec<ImportNode>, ImportError> {
    Ok(sidebar(locations, profile)?.spaces)
}

pub(crate) fn essentials(
    locations: &Locations,
    profile: &str,
) -> Result<Vec<Essential>, ImportError> {
    Ok(sidebar(locations, profile)?.favorites)
}

/// What one profile's sidebar keeps: its favorites, and one folder per space
/// holding the space's pinned tabs.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Sidebar {
    favorites: Vec<Essential>,
    spaces: Vec<ImportNode>,
}

pub(crate) fn parse_sidebar(bytes: &[u8], profile: &str) -> Result<Sidebar, ImportError> {
    let file: Value = serde_json::from_slice(bytes).map_err(|_| ImportError::Unreadable)?;
    let container = file
        .get("sidebar")
        .and_then(|sidebar| sidebar.get("containers"))
        .and_then(Value::as_array)
        .and_then(|containers| {
            containers.iter().find(|container| {
                container.get("spaces").is_some() && container.get("items").is_some()
            })
        })
        .ok_or(ImportError::Unreadable)?;
    let items: HashMap<&str, &Value> = entries(container.get("items"))
        .filter_map(|item| Some((item.get("id")?.as_str()?, item)))
        .collect();
    let tree = Tree { items: &items };
    let mut nodes = Vec::new();
    let mut seen = HashSet::new();
    let mut favorites = Vec::new();
    // Some files keep the favorites only in the sync mirror.
    let mirrored = file
        .get("sidebarSyncState")
        .and_then(|state| state.get("container"))
        .and_then(|container| container.get("value"))
        .and_then(|value| value.get("topAppsContainerIDs"));
    let top_apps = container
        .get("topAppsContainerIDs")
        .filter(|list| list.as_array().is_some_and(|list| !list.is_empty()))
        .or(mirrored);
    let kept = favorite_containers(top_apps, profile)
        .into_iter()
        .flat_map(|id| tree.children(id, &mut HashSet::new()));
    flatten(kept, &mut |title, url| {
        if favorites.len() < crate::MAX_ESSENTIALS && seen.insert(url.clone()) {
            favorites.push(Essential { title, url });
        }
    });
    for space in entries(container.get("spaces")) {
        if space.get("profile").and_then(profile_of).unwrap_or(DEFAULT) != profile {
            continue;
        }
        let ids = space
            .get("newContainerIDs")
            .or_else(|| space.get("containerIDs"));
        let pinned: Vec<ImportNode> = marked_ids(ids, "pinned")
            .into_iter()
            .flat_map(|id| tree.children(id, &mut HashSet::new()))
            .collect();
        if pinned.is_empty() {
            continue;
        }
        let title = space
            .get("title")
            .and_then(Value::as_str)
            .filter(|title| !title.trim().is_empty())
            .unwrap_or("Space");
        nodes.push(ImportNode::Folder {
            title: title.to_owned(),
            children: pinned,
        });
    }
    Ok(Sidebar {
        favorites,
        spaces: nodes,
    })
}

/// Essentials are a flat row, so a folder among the favorites gives up its
/// links in order.
fn flatten(nodes: impl IntoIterator<Item = ImportNode>, keep: &mut impl FnMut(String, String)) {
    for node in nodes {
        match node {
            ImportNode::Link { title, url } => keep(title, url),
            ImportNode::Folder { children, .. } => flatten(children, keep),
        }
    }
}

/// Arc stores its lists as alternating ids and objects; the objects carry
/// their own id, so only they are read.
fn entries(list: Option<&Value>) -> impl Iterator<Item = &Value> {
    list.and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|entry| entry.is_object())
}

/// Ids that follow a marker in a container list. The marker is a string
/// ("pinned") in older files and an object (`{"pinned": {}}`) in newer ones.
fn marked_ids<'a>(list: Option<&'a Value>, marker: &str) -> Vec<&'a str> {
    let Some(list) = list.and_then(Value::as_array) else {
        return Vec::new();
    };
    list.windows(2)
        .filter(|pair| pair[0].as_str() == Some(marker) || pair[0].get(marker).is_some())
        .filter_map(|pair| pair[1].as_str())
        .collect()
}

/// The profile directory a marker names: `{"default": true}` is the first
/// profile, `{"custom": {"_0": {"directoryBasename": "Profile 2"}}}` another.
fn profile_of(marker: &Value) -> Option<&str> {
    if marker.get("default").is_some() {
        return Some(DEFAULT);
    }
    marker
        .get("custom")?
        .get("_0")?
        .get("directoryBasename")?
        .as_str()
}

/// The containers of favorites that belong to `profile`. Each profile's id
/// follows an object naming it; ids before any such object are the first
/// profile's.
fn favorite_containers<'a>(list: Option<&'a Value>, profile: &str) -> Vec<&'a str> {
    let mut owner = DEFAULT;
    let mut found = Vec::new();
    for entry in list.and_then(Value::as_array).into_iter().flatten() {
        match entry.as_str() {
            Some(id) if owner == profile => found.push(id),
            Some(_) => {}
            None => owner = profile_of(entry).unwrap_or(DEFAULT),
        }
    }
    found
}

struct Tree<'a> {
    items: &'a HashMap<&'a str, &'a Value>,
}

impl Tree<'_> {
    /// The children of `id` in sidebar order. `seen` breaks cycles a damaged
    /// file could contain.
    fn children(&self, id: &str, seen: &mut HashSet<String>) -> Vec<ImportNode> {
        if !seen.insert(id.to_owned()) || seen.len() > crate::MAX_NODES {
            return Vec::new();
        }
        let Some(item) = self.items.get(id) else {
            return Vec::new();
        };
        item.get("childrenIds")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .flat_map(|child| self.node(child, seen))
            .collect()
    }

    /// What one item becomes: a link for a tab, a folder for a list. Anything
    /// else (a split view, say) holds tabs but is not a place of its own, so
    /// they stay where they are.
    fn node(&self, id: &str, seen: &mut HashSet<String>) -> Vec<ImportNode> {
        let Some(item) = self.items.get(id) else {
            return Vec::new();
        };
        let title = item
            .get("title")
            .and_then(Value::as_str)
            .filter(|title| !title.is_empty());
        let Some(data) = item.get("data") else {
            return Vec::new();
        };
        if let Some(tab) = data.get("tab") {
            let Some(url) = tab.get("savedURL").and_then(Value::as_str) else {
                return Vec::new();
            };
            let title = title
                .or_else(|| tab.get("savedTitle").and_then(Value::as_str))
                .unwrap_or_default();
            return vec![ImportNode::Link {
                title: title.to_owned(),
                url: url.to_owned(),
            }];
        }
        if data.get("list").is_some() {
            return vec![ImportNode::Folder {
                title: title.unwrap_or("Folder").to_owned(),
                children: self.children(id, seen),
            }];
        }
        self.children(id, seen)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn favorites_are_essentials_and_spaces_become_folders_of_their_pinned_tabs() {
        let file = br#"{"sidebar":{"containers":[
          {"global":{}},
          {
            "topAppsContainerIDs":[{"default":true},"top"],
            "spaces":[
              "s1",{"id":"s1","title":"Work","newContainerIDs":[{"pinned":{}},"p1",{"unpinned":{}},"u1"]},
              "s2",{"id":"s2","title":"Empty","containerIDs":["pinned","p2","unpinned","u2"]}
            ],
            "items":[
              "top",{"id":"top","childrenIds":["fav"],"data":{"itemContainer":{}}},
              "fav",{"id":"fav","parentID":"top","data":{"tab":{"savedURL":"https://mail.example/","savedTitle":"Mail"}}},
              "p1",{"id":"p1","childrenIds":["f1","t2"],"data":{"itemContainer":{}}},
              "f1",{"id":"f1","title":"Docs","parentID":"p1","childrenIds":["t1"],"data":{"list":{}}},
              "t1",{"id":"t1","parentID":"f1","title":"Renamed","data":{"tab":{"savedURL":"https://docs.example/","savedTitle":"Docs home"}}},
              "t2",{"id":"t2","parentID":"p1","data":{"tab":{"savedURL":"https://board.example/","savedTitle":"Board"}}},
              "u1",{"id":"u1","childrenIds":["t3"],"data":{"itemContainer":{}}},
              "t3",{"id":"t3","parentID":"u1","data":{"tab":{"savedURL":"https://today.example/"}}},
              "p2",{"id":"p2","childrenIds":[],"data":{"itemContainer":{}}}
            ]
          }
        ]}}"#;
        let link = |title: &str, url: &str| ImportNode::Link {
            title: title.into(),
            url: url.into(),
        };
        let sidebar = parse_sidebar(file, DEFAULT).unwrap();
        assert_eq!(
            sidebar.favorites,
            vec![Essential {
                title: "Mail".into(),
                url: "https://mail.example/".into()
            }]
        );
        assert_eq!(
            sidebar.spaces,
            vec![ImportNode::Folder {
                title: "Work".into(),
                children: vec![
                    ImportNode::Folder {
                        title: "Docs".into(),
                        children: vec![link("Renamed", "https://docs.example/")]
                    },
                    link("Board", "https://board.example/"),
                ]
            },]
        );
    }

    /// Two profiles' worth of sidebar, shaped like Arc's file: the first
    /// profile's space and favorites, and a second profile's.
    const TWO_PROFILES: &[u8] = br#"{"sidebar":{"containers":[{
        "topAppsContainerIDs":[{"default":true},"top-a",{"custom":{"_0":{"directoryBasename":"Profile 2","machineID":"m"}}},"top-b"],
        "spaces":[
          "s1",{"id":"s1","title":"Personal","profile":{"default":true},"newContainerIDs":[{"pinned":{}},"pa"]},
          "s2",{"id":"s2","title":"Studio","profile":{"custom":{"_0":{"directoryBasename":"Profile 2","machineID":"m"}}},"newContainerIDs":[{"pinned":{}},"pb"]}
        ],
        "items":[
          "top-a",{"id":"top-a","childrenIds":["fa"],"data":{"itemContainer":{}}},
          "fa",{"id":"fa","title":null,"childrenIds":[],"data":{"tab":{"savedURL":"https://mail.example/","savedTitle":"Mail"}}},
          "top-b",{"id":"top-b","childrenIds":["fb"],"data":{"itemContainer":{}}},
          "fb",{"id":"fb","title":null,"childrenIds":[],"data":{"tab":{"savedURL":"https://figma.example/","savedTitle":"Figma"}}},
          "pa",{"id":"pa","childrenIds":["ta"],"data":{"itemContainer":{}}},
          "ta",{"id":"ta","title":null,"childrenIds":[],"data":{"tab":{"savedURL":"https://notes.example/","savedTitle":"Notes"}}},
          "pb",{"id":"pb","childrenIds":["tb"],"data":{"itemContainer":{}}},
          "tb",{"id":"tb","title":"","childrenIds":[],"data":{"tab":{"savedURL":"https://board.example/","savedTitle":"Board"}}}
        ]
    }]}}"#;

    #[test]
    fn each_profile_gets_its_own_spaces_and_favorites() {
        let personal = parse_sidebar(TWO_PROFILES, "Default").unwrap();
        assert_eq!(
            personal
                .favorites
                .iter()
                .map(|f| f.title.as_str())
                .collect::<Vec<_>>(),
            ["Mail"]
        );
        assert_eq!(
            personal.spaces,
            vec![ImportNode::Folder {
                title: "Personal".into(),
                children: vec![ImportNode::Link {
                    title: "Notes".into(),
                    url: "https://notes.example/".into()
                }]
            }]
        );
        let studio = parse_sidebar(TWO_PROFILES, "Profile 2").unwrap();
        assert_eq!(
            studio
                .favorites
                .iter()
                .map(|f| f.title.as_str())
                .collect::<Vec<_>>(),
            ["Figma"]
        );
        // A tab with an empty name shows the title it was saved with.
        assert_eq!(
            studio.spaces,
            vec![ImportNode::Folder {
                title: "Studio".into(),
                children: vec![ImportNode::Link {
                    title: "Board".into(),
                    url: "https://board.example/".into()
                }]
            }]
        );
        let none = parse_sidebar(TWO_PROFILES, "Profile 9").unwrap();
        assert!(none.favorites.is_empty() && none.spaces.is_empty());
    }

    #[test]
    fn favorites_kept_only_in_the_sync_mirror_are_found_there() {
        let file = br#"{
          "sidebarSyncState":{"container":{"value":{"topAppsContainerIDs":[{"default":true},"top"]}}},
          "sidebar":{"containers":[{
            "topAppsContainerIDs":[],
            "spaces":[],
            "items":[
              "top",{"id":"top","childrenIds":["fav"],"data":{"itemContainer":{}}},
              "fav",{"id":"fav","childrenIds":[],"data":{"tab":{"savedURL":"https://mail.example/","savedTitle":"Mail"}}}
            ]
        }]}}"#;
        let sidebar = parse_sidebar(file, DEFAULT).unwrap();
        assert_eq!(sidebar.favorites.len(), 1);
    }

    #[test]
    fn a_split_view_is_not_a_folder_and_an_easel_is_nothing() {
        let file = br#"{"sidebar":{"containers":[{
            "spaces":[{"id":"s","title":"Work","containerIDs":["pinned","p"]}],
            "items":[
              {"id":"p","childrenIds":["split","easel","t3"],"data":{"itemContainer":{}}},
              {"id":"split","childrenIds":["t1","t2"],"data":{"splitView":{}}},
              {"id":"t1","childrenIds":[],"data":{"tab":{"savedURL":"https://one.example/","savedTitle":"One"}}},
              {"id":"t2","childrenIds":[],"data":{"tab":{"savedURL":"https://two.example/","savedTitle":"Two"}}},
              {"id":"easel","title":"Sketch","childrenIds":[],"data":{"easel":{}}},
              {"id":"t3","childrenIds":[],"data":{"tab":{"savedURL":"https://three.example/","savedTitle":"Three"}}}
            ]
        }]}}"#;
        let titles: Vec<String> = match &parse_sidebar(file, DEFAULT).unwrap().spaces[..] {
            [ImportNode::Folder { children, .. }] => children
                .iter()
                .map(|node| match node {
                    ImportNode::Link { title, .. } => title.clone(),
                    ImportNode::Folder { title, .. } => format!("[{title}]"),
                })
                .collect(),
            other => panic!("{other:?}"),
        };
        assert_eq!(titles, ["One", "Two", "Three"]);
    }

    #[test]
    fn every_profile_is_offered_but_not_the_one_arc_keeps_for_itself() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("Library/Application Support/Arc");
        let data = root.join("User Data");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(root.join("StorableSidebar.json"), TWO_PROFILES).unwrap();
        // As Arc writes it: the first profile, one made later, and the
        // profile Arc runs its own pages in.
        std::fs::write(
            data.join("Local State"),
            br#"{"profile":{"info_cache":{
                "Default":{"name":"Your Arc"},
                "Profile 1":{"name":"__ARC_SYSTEM_PROFILE"},
                "Profile 2":{"name":"Development"}
            }}}"#,
        )
        .unwrap();
        for profile in ["Default", "Profile 1", "Profile 2"] {
            std::fs::create_dir_all(data.join(profile)).unwrap();
            std::fs::write(data.join(profile).join("History"), b"").unwrap();
        }
        let locations = Locations::scratch(Os::Mac, home.path());
        let found = profiles(&locations);
        assert_eq!(
            found
                .iter()
                .map(|p| (
                    p.id.as_str(),
                    p.name.as_str(),
                    p.bookmarks,
                    p.history,
                    p.essentials
                ))
                .collect::<Vec<_>>(),
            [
                ("Default", "Your Arc", true, true, true),
                ("Profile 2", "Development", true, true, true)
            ]
        );
        assert_eq!(
            profile_dir(&locations, "Profile 2").unwrap(),
            data.join("Profile 2")
        );
        assert!(profile_dir(&locations, "Profile 1").is_err());
        assert!(profile_dir(&locations, "../Default").is_err());
        assert_eq!(bookmarks(&locations, "Profile 2").unwrap().len(), 1);
        assert_eq!(essentials(&locations, "Default").unwrap().len(), 1);
    }

    #[test]
    fn arc_for_windows_is_a_store_package_and_arc_is_not_on_linux() {
        let home = tempfile::tempdir().unwrap();
        let root = home
            .path()
            .join("Local/Packages/TheBrowserCompany.Arc_ttt1ap7aakyb4/LocalCache/Local/Arc");
        std::fs::create_dir_all(root.join("User Data/Default")).unwrap();
        std::fs::write(root.join("User Data/Default/History"), b"").unwrap();
        let windows = Locations::scratch(Os::Windows, home.path());
        let found = profiles(&windows);
        assert_eq!(found.len(), 1);
        assert!(found[0].history && !found[0].bookmarks);
        assert!(profiles(&Locations::scratch(Os::Linux, home.path())).is_empty());
        assert!(profiles(&Locations::scratch(Os::Mac, home.path())).is_empty());
    }

    #[test]
    fn a_cycle_in_a_damaged_file_ends() {
        let file = br#"{"sidebar":{"containers":[{
            "spaces":[{"id":"s","title":"Loop","containerIDs":["pinned","a"]}],
            "items":[
              {"id":"a","childrenIds":["b"],"data":{"itemContainer":{}}},
              {"id":"b","title":"B","childrenIds":["a"],"data":{"list":{}}}
            ]
        }]}}"#;
        let sidebar = parse_sidebar(file, DEFAULT).unwrap();
        assert_eq!(sidebar.spaces.len(), 1);
        assert!(parse_sidebar(b"{\"sidebar\":{}}", DEFAULT).is_err());
    }
}
