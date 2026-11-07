//! Bookmarks: folders and links a person keeps, apart from the sidebar.
//! Pinned tabs stay live tabs; a bookmark is an address to come back to, and
//! an import from another browser lands here as one folder.

/// Bookmarks one profile may hold, folders included.
pub const MAX_BOOKMARKS: u32 = 50_000;
/// Folders nest at most this deep, counting the top level as one.
pub const MAX_DEPTH: usize = 32;
pub const MAX_TITLE_BYTES: usize = 512;
/// Rows one folder listing or search returns.
pub const MAX_LISTING: u32 = 5_000;
pub const MAX_SEARCH_RESULTS: u32 = 100;
pub const MAX_QUERY_BYTES: usize = 512;
/// The longest address a bookmark keeps, as the store enforces it.
pub const MAX_URL_BYTES: usize = 8 * 1024;
/// Nodes one subtree read returns, for extensions reading the whole tree.
pub const MAX_SUBTREE: u32 = 10_000;
/// Bookmarks one extension lookup or search names or returns.
pub const MAX_ENTRIES: u32 = 1_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BookmarkNode {
    pub id: i64,
    pub parent: Option<i64>,
    /// None for a folder.
    pub url: Option<String>,
    pub title: String,
    /// Direct children, for a folder.
    pub children: u32,
}

impl BookmarkNode {
    pub fn is_folder(&self) -> bool {
        self.url.is_none()
    }
}

/// One bookmark operation. Writes are validated again by the store, which
/// alone sees the tree they apply to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BookmarkRequest {
    /// A folder's children in order; `None` is the top level.
    Children {
        parent: Option<i64>,
    },
    /// The folders from the top level down to `id`, `id` last.
    Path {
        id: i64,
    },
    Search {
        query: String,
    },
    /// Every folder and link with siblings in order, for an export. Child
    /// counts are not filled in.
    All,
    /// `if_absent` returns the existing bookmark for the same address instead
    /// of adding a second one.
    AddLink {
        parent: Option<i64>,
        title: String,
        url: String,
        if_absent: bool,
    },
    AddFolder {
        parent: Option<i64>,
        title: String,
    },
    Rename {
        id: i64,
        title: String,
    },
    /// Moves `id` into `parent` at `index` among its new siblings, clamped.
    Move {
        id: i64,
        parent: Option<i64>,
        index: u32,
    },
    /// Removes `id` and, for a folder, everything in it.
    Remove {
        id: i64,
    },
    /// Merges a tree from another browser into the top-level folder named
    /// `folder`: subfolders are matched by title, and an address already in
    /// the folder it would land in is skipped, so importing again adds only
    /// what is new.
    Import {
        folder: String,
        nodes: Vec<ImportNode>,
    },
    /// Each of `ids`; refused when any is missing.
    Entries {
        ids: Vec<i64>,
    },
    /// A folder's children in order, `None` being the top level.
    ChildEntries {
        parent: Option<i64>,
    },
    /// `root` and everything below it, each folder before its contents;
    /// `None` is the whole tree. Refused as `Full` past [`MAX_SUBTREE`].
    Subtree {
        root: Option<i64>,
    },
    /// Links, newest first.
    Recent {
        limit: u32,
    },
    /// Folders and links whose title or address holds every word of `query`,
    /// and whose address and title equal `url` and `title` when given.
    Find {
        query: Option<String>,
        url: Option<String>,
        title: Option<String>,
    },
    /// A link when `url` is given, else a folder, at `index` among its new
    /// siblings or last.
    Create {
        parent: Option<i64>,
        index: Option<u32>,
        title: String,
        url: Option<String>,
    },
    /// Only a link has an address to change.
    Update {
        id: i64,
        title: Option<String>,
        url: Option<String>,
    },
    /// Moves as Chrome counts: `index` is a place among the siblings before
    /// the move, so moving later in the same folder lands one place earlier.
    /// An absent `parent` keeps the folder; an absent `index` goes last.
    Relocate {
        id: i64,
        parent: Option<Option<i64>>,
        index: Option<u32>,
    },
    /// Refused as `Invalid` for a folder that still holds anything unless
    /// `recursive`.
    Delete {
        id: i64,
        recursive: bool,
    },
}

/// A bookmark with its place among its siblings and when it was added.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BookmarkEntry {
    pub id: i64,
    pub parent: Option<i64>,
    pub index: u32,
    /// None for a folder.
    pub url: Option<String>,
    pub title: String,
    /// Unix seconds.
    pub added_at: i64,
}

/// A change to the tree as Chrome's bookmark events describe it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BookmarkChange {
    Created(BookmarkEntry),
    Changed {
        id: i64,
        title: String,
        url: Option<String>,
    },
    Moved {
        id: i64,
        parent: Option<i64>,
        index: u32,
        old_parent: Option<i64>,
        old_index: u32,
    },
    /// The node as it was; a folder's contents went with it.
    Removed(BookmarkEntry),
    /// Another browser's bookmarks were merged in.
    Imported,
}

/// A bookmark as another browser kept it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImportNode {
    Folder {
        title: String,
        children: Vec<ImportNode>,
    },
    Link {
        title: String,
        url: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BookmarkReply {
    Nodes(Vec<BookmarkNode>),
    /// The bookmark added, or the existing one an `if_absent` add found.
    Added(i64),
    /// Links an import added; folders are not counted.
    Imported(u32),
    Entries(Vec<BookmarkEntry>),
    Done,
    Failed(BookmarkFailure),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BookmarkFailure {
    /// The bookmark or folder named no longer exists.
    Missing,
    /// The profile holds the maximum, or the folder is nested too deep.
    Full,
    /// A folder cannot move into itself or its own contents.
    Cycle,
    Invalid,
    Unavailable,
}

/// A title as stored: control characters removed, whitespace collapsed,
/// bounded on a character boundary.
pub fn clean_title(title: &str) -> String {
    let collapsed = title
        .split(|c: char| c.is_whitespace() || c.is_control())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let mut end = collapsed.len().min(MAX_TITLE_BYTES);
    while !collapsed.is_char_boundary(end) {
        end -= 1;
    }
    collapsed[..end].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_are_single_line_and_bounded() {
        assert_eq!(clean_title("  Read\n\tlater \u{7}now "), "Read later now");
        let long = "é".repeat(MAX_TITLE_BYTES);
        let cleaned = clean_title(&long);
        assert!(cleaned.len() <= MAX_TITLE_BYTES);
        assert!(cleaned.chars().all(|c| c == 'é'));
    }
}
