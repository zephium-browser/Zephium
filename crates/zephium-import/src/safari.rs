//! Safari keeps bookmarks in a property list and history in a database, both
//! inside a directory macOS protects: reading them needs Full Disk Access.

use std::io::Read;
use std::path::PathBuf;

use plist::Value;

use crate::{snapshot, ImportError, ImportNode, ImportedVisit, Locations, Os, Profile, Source};

/// Seconds between 1970-01-01 and 2001-01-01, Core Foundation's epoch.
const EPOCH_OFFSET_SECONDS: f64 = 978_307_200.0;
const PROFILE: &str = "Default";

fn root(locations: &Locations) -> PathBuf {
    locations.home.join("Library").join("Safari")
}

/// Safari ships with every Mac. Its files are opened only once the person
/// imports from it: touching them while listing sources makes macOS warn that
/// Zephium tried to read another app's data. Missing access surfaces then, as
/// `ImportError::Permission`.
pub(crate) fn source(locations: &Locations) -> Option<Source> {
    if locations.os != Os::Mac {
        return None;
    }
    Some(Source {
        browser: crate::Browser::Safari,
        profiles: vec![Profile {
            id: PROFILE.into(),
            name: "Safari".into(),
            bookmarks: true,
            history: true,
            essentials: false,
        }],
        needs_permission: false,
    })
}

/// The favorites bar opens at the top of the import; the menu and the
/// reading list keep their own folder.
pub(crate) fn bookmarks(locations: &Locations) -> Result<Vec<ImportNode>, ImportError> {
    let path = root(locations).join("Bookmarks.plist");
    if std::fs::metadata(&path)?.len() > 32 * 1024 * 1024 {
        return Err(ImportError::Unreadable);
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(32 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 32 * 1024 * 1024 {
        return Err(ImportError::Unreadable);
    }
    parse_bookmarks(&bytes)
}

pub(crate) fn parse_bookmarks(bytes: &[u8]) -> Result<Vec<ImportNode>, ImportError> {
    let file =
        Value::from_reader(std::io::Cursor::new(bytes)).map_err(|_| ImportError::Unreadable)?;
    let mut nodes = Vec::new();
    for top in children(&file, 0) {
        match top {
            Top::Folder { name, children } if name == "BookmarksBar" => nodes.extend(children),
            Top::Folder { name, children } => {
                let title = match name.as_str() {
                    "BookmarksMenu" => "Bookmarks Menu".to_owned(),
                    "com.apple.ReadingList" => "Reading List".to_owned(),
                    _ => name,
                };
                if !children.is_empty() {
                    nodes.push(ImportNode::Folder { title, children });
                }
            }
            Top::Link(link) => nodes.push(link),
        }
    }
    Ok(nodes)
}

enum Top {
    Folder {
        name: String,
        children: Vec<ImportNode>,
    },
    Link(ImportNode),
}

fn children(folder: &Value, depth: usize) -> Vec<Top> {
    if depth > crate::MAX_TREE_DEPTH {
        return Vec::new();
    }
    folder
        .as_dictionary()
        .and_then(|folder| folder.get("Children"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|child| {
            let child = child.as_dictionary()?;
            let text = |key: &str| child.get(key).and_then(Value::as_string);
            match text("WebBookmarkType")? {
                "WebBookmarkTypeLeaf" => {
                    let title = child
                        .get("URIDictionary")
                        .and_then(Value::as_dictionary)
                        .and_then(|uri| uri.get("title"))
                        .and_then(Value::as_string)
                        .unwrap_or_default();
                    Some(Top::Link(ImportNode::Link {
                        title: title.to_owned(),
                        url: text("URLString")?.to_owned(),
                    }))
                }
                "WebBookmarkTypeList" => Some(Top::Folder {
                    name: text("Title").unwrap_or("Folder").to_owned(),
                    children: children(&Value::Dictionary(child.clone()), depth + 1)
                        .into_iter()
                        .map(|node| match node {
                            Top::Link(link) => link,
                            Top::Folder { name, children } => ImportNode::Folder {
                                title: name,
                                children,
                            },
                        })
                        .collect(),
                }),
                // History and other proxies are not bookmarks.
                _ => None,
            }
        })
        .collect()
}

pub(crate) fn history(
    locations: &Locations,
    since: i64,
    limit: usize,
) -> Result<Vec<ImportedVisit>, ImportError> {
    let snapshot = snapshot::open(&root(locations).join("History.db"))?;
    let mut rows = snapshot.conn.prepare(
        "SELECT i.url, coalesce(v.title, ''), v.visit_time
         FROM history_visits v JOIN history_items i ON i.id = v.history_item
         WHERE v.visit_time >= ?1 ORDER BY v.visit_time DESC LIMIT ?2",
    )?;
    let visits = rows
        .query_map(
            rusqlite::params![since as f64 - EPOCH_OFFSET_SECONDS, limit as i64],
            |row| {
                Ok(ImportedVisit {
                    url: row.get(0)?,
                    title: row.get(1)?,
                    visited_at: (row.get::<_, f64>(2)? + EPOCH_OFFSET_SECONDS) as i64,
                })
            },
        )?
        .collect::<rusqlite::Result<_>>()?;
    Ok(visits)
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    #[test]
    fn bookmark_stream_cannot_exceed_the_read_limit() {
        use std::io::Write;
        let directory = tempfile::tempdir().unwrap();
        let locations = Locations::scratch(Os::Mac, directory.path());
        std::fs::create_dir_all(root(&locations)).unwrap();
        let path = root(&locations).join("Bookmarks.plist");
        assert!(std::process::Command::new("/usr/bin/mkfifo")
            .arg(&path)
            .status()
            .unwrap()
            .success());
        // A pipe reports zero bytes of metadata but supplies more than the limit.
        let writer = std::thread::spawn(move || {
            let mut file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
            let mut bytes =
                br#"<?xml version="1.0"?><plist version="1.0"><dict/></plist>"#.to_vec();
            bytes.resize(32 * 1024 * 1024 + 1, b' ');
            file.write_all(&bytes).unwrap();
            // A bounded reader closes without consuming the rest of the stream.
            bytes.fill(b' ');
            file.write_all(&bytes)
        });
        let result = bookmarks(&locations);
        let tail = writer.join().unwrap();
        assert!(matches!(result, Err(ImportError::Unreadable)));
        assert_eq!(tail.unwrap_err().kind(), std::io::ErrorKind::BrokenPipe);
    }

    #[test]
    fn oversized_bookmarks_are_refused_before_parsing() {
        let directory = tempfile::tempdir().unwrap();
        let locations = Locations::scratch(Os::Mac, directory.path());
        std::fs::create_dir_all(root(&locations)).unwrap();
        let path = root(&locations).join("Bookmarks.plist");
        let mut bytes = br#"<?xml version="1.0"?><plist version="1.0"><dict/></plist>"#.to_vec();
        bytes.resize(32 * 1024 * 1024 + 1, b' ');
        std::fs::write(&path, bytes).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
            // Metadata remains readable, but opening the contents would fail
            // with Permission rather than the size refusal below.
            // A privileged runner can still open it; the size check holds either way.
            if let Err(error) = std::fs::File::open(&path) {
                assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
            }
        }
        assert!(matches!(
            bookmarks(&locations),
            Err(ImportError::Unreadable)
        ));
    }

    use super::*;

    #[test]
    fn the_favorites_bar_opens_at_the_top_and_the_reading_list_keeps_a_folder() {
        let file = br#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Children</key><array>
    <dict><key>Title</key><string>History</string><key>WebBookmarkType</key><string>WebBookmarkTypeProxy</string></dict>
    <dict><key>Title</key><string>BookmarksBar</string><key>WebBookmarkType</key><string>WebBookmarkTypeList</string>
      <key>Children</key><array>
        <dict><key>URLString</key><string>https://apple.example/</string>
          <key>URIDictionary</key><dict><key>title</key><string>Apple</string></dict>
          <key>WebBookmarkType</key><string>WebBookmarkTypeLeaf</string></dict>
      </array></dict>
    <dict><key>Title</key><string>com.apple.ReadingList</string><key>WebBookmarkType</key><string>WebBookmarkTypeList</string>
      <key>Children</key><array>
        <dict><key>URLString</key><string>https://later.example/</string>
          <key>URIDictionary</key><dict><key>title</key><string>Later</string></dict>
          <key>WebBookmarkType</key><string>WebBookmarkTypeLeaf</string></dict>
      </array></dict>
    <dict><key>Title</key><string>BookmarksMenu</string><key>WebBookmarkType</key><string>WebBookmarkTypeList</string>
      <key>Children</key><array/></dict>
  </array>
</dict></plist>"#;
        assert_eq!(
            parse_bookmarks(file).unwrap(),
            vec![
                ImportNode::Link {
                    title: "Apple".into(),
                    url: "https://apple.example/".into()
                },
                ImportNode::Folder {
                    title: "Reading List".into(),
                    children: vec![ImportNode::Link {
                        title: "Later".into(),
                        url: "https://later.example/".into()
                    }]
                },
            ]
        );
        assert!(parse_bookmarks(b"not a plist").is_err());
    }
}
