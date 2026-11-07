//! Profile-scoped bookmark reads and writes for the Bookmarks panel, and
//! bookmarking the page in front. Every call runs on the store-read worker;
//! the shell only admits, routes and decorates with icons.

use super::*;

use crate::api::{BookmarkCompletion, ExportCompletion, ImportCompletion};
use crate::store_reads::{BookmarkSurfaceReply, BookmarkWork, ImportWork};
use zephium_core::bookmarks::{BookmarkFailure, BookmarkNode};
use zephium_ipc::{
    BookmarkCall, BookmarkCrumb, BookmarkError, BookmarkResponse, BookmarkView, IconSurface,
};

/// Completions outlive one actor turn; the read queue bounds how many exist.
const MAX_PENDING_BOOKMARK_CALLS: usize = 8;
/// An export reads a whole table, so only one runs at a time.
const MAX_PENDING_EXPORTS: usize = 1;

enum Pending {
    Surface(BookmarkCompletion),
    /// Bookmark This Page; chrome learns the outcome as a UI command.
    AddPage,
}

#[derive(Default)]
pub(super) struct BookmarkState {
    pending: std::collections::HashMap<u64, Pending>,
    imports: std::collections::HashMap<u64, ImportCompletion>,
    exports: std::collections::HashMap<u64, ExportCompletion>,
    /// Icon imports by token, with the profile whose sidebar reads them back.
    icon_imports: std::collections::HashMap<u64, ProfileId>,
    next_token: u64,
}

fn error(failure: BookmarkFailure) -> BookmarkError {
    match failure {
        BookmarkFailure::Missing => BookmarkError::Missing,
        BookmarkFailure::Full => BookmarkError::Full,
        BookmarkFailure::Cycle => BookmarkError::Cycle,
        BookmarkFailure::Invalid => BookmarkError::Invalid,
        BookmarkFailure::Unavailable => BookmarkError::Unavailable,
    }
}

impl Shell {
    /// Private profiles keep nothing, and a degraded profile cannot answer.
    fn bookmarks_writable(&self, profile: ProfileId) -> bool {
        self.profiles
            .get(profile)
            .is_some_and(|p| p.kind != zephium_core::profiles::ProfileKind::Incognito)
            && !self.degraded_storage_profiles.contains(&profile)
            && !self.profile_deletion_quarantines(profile)
    }

    fn queue_bookmarks(
        &mut self,
        profile: ProfileId,
        work: BookmarkWork,
        pending: Pending,
    ) -> Result<(), Pending> {
        if self.bookmarks.pending.len() >= MAX_PENDING_BOOKMARK_CALLS {
            return Err(pending);
        }
        let Some(reads) = &self.store_reads else {
            return Err(pending);
        };
        self.bookmarks.next_token = self.bookmarks.next_token.wrapping_add(1);
        let token = self.bookmarks.next_token;
        if !reads.request_bookmarks(token, profile, work) {
            return Err(pending);
        }
        self.bookmarks.pending.insert(token, pending);
        Ok(())
    }

    pub(super) fn bookmark_call(
        &mut self,
        expected_profile: ProfileId,
        call: BookmarkCall,
        done: BookmarkCompletion,
    ) {
        let focused = self
            .windows
            .focused()
            .is_some_and(|window| window.profile == expected_profile);
        if !focused || !call.validate() {
            done.finish(BookmarkResponse::Error {
                error: BookmarkError::Invalid,
            });
            return;
        }
        if !self.bookmarks_writable(expected_profile) {
            done.finish(BookmarkResponse::Error {
                error: BookmarkError::Unavailable,
            });
            return;
        }
        let capacity = self.bookmarks.pending.len() >= MAX_PENDING_BOOKMARK_CALLS;
        if let Err(Pending::Surface(done)) = self.queue_bookmarks(
            expected_profile,
            BookmarkWork::Surface(call),
            Pending::Surface(done),
        ) {
            done.finish(BookmarkResponse::Error {
                error: if capacity {
                    BookmarkError::Capacity
                } else {
                    BookmarkError::Unavailable
                },
            });
        }
    }

    /// Bookmarks the page in front, once: an address already kept answers
    /// with the bookmark that holds it.
    /// Bookmarks `tab`, or the page in front when there is none.
    pub(super) fn operation_bookmark_page(&mut self, tab: Option<ItemId>) -> OperationDisposition {
        let page = if tab.is_some() {
            tab.filter(|id| self.item_in_focused_scope(*id))
        } else if self.active_browser_page().is_some() {
            self.work_pane_tab()
        } else {
            self.windows.focused().and_then(|window| window.active)
        };
        let Some(profile) = self.windows.focused().map(|window| window.profile) else {
            return operation_result(OperationOutcome::NoOp, OperationReason::NoFocusedWindow);
        };
        let target = page.and_then(|id| self.items.tab(id)).and_then(|tab| {
            let url = tab.url.as_ref()?;
            (tab.content == zephium_core::item::TabContent::Web && navigation::is_allowed(url))
                .then(|| (url.to_string(), tab.title.clone()))
        });
        let Some((url, title)) = target else {
            return operation_result(OperationOutcome::NoOp, OperationReason::InvalidScope);
        };
        if !self.bookmarks_writable(profile) {
            return operation_result(OperationOutcome::Rejected, OperationReason::InvalidScope);
        }
        match self.queue_bookmarks(
            profile,
            BookmarkWork::AddPage { url, title },
            Pending::AddPage,
        ) {
            Ok(()) => operation_result(OperationOutcome::Applied, OperationReason::MutationApplied),
            Err(_) => operation_result(OperationOutcome::Rejected, OperationReason::InvalidScope),
        }
    }

    pub(super) fn on_bookmarks_read(
        &mut self,
        token: u64,
        profile: ProfileId,
        reply: BookmarkSurfaceReply,
    ) {
        let Some(pending) = self.bookmarks.pending.remove(&token) else {
            return;
        };
        let done = match pending {
            Pending::AddPage => {
                let command = match reply {
                    BookmarkSurfaceReply::Saved(Some(id)) => format!("bookmark.added={id}"),
                    _ => "bookmark.failed".to_owned(),
                };
                (self.emit)(Projection::UiCommand(command));
                return;
            }
            Pending::Surface(done) => done,
        };
        let response = match reply {
            BookmarkSurfaceReply::Listing {
                folder,
                path,
                nodes,
            } => BookmarkResponse::Listing {
                folder: folder.map(|id| id.to_string()),
                path: path
                    .into_iter()
                    .map(|node| BookmarkCrumb {
                        id: node.id.to_string(),
                        title: node.title,
                    })
                    .collect(),
                items: self.bookmark_views(profile, nodes),
            },
            BookmarkSurfaceReply::Results(nodes) => BookmarkResponse::Results {
                items: self.bookmark_views(profile, nodes),
            },
            BookmarkSurfaceReply::Saved(id) => BookmarkResponse::Saved {
                id: id.map(|id| id.to_string()),
            },
            BookmarkSurfaceReply::Failed(failure) => BookmarkResponse::Error {
                error: error(failure),
            },
        };
        self.publish_icons();
        done.finish(response);
    }

    fn bookmark_views(
        &mut self,
        profile: ProfileId,
        nodes: Vec<BookmarkNode>,
    ) -> Vec<BookmarkView> {
        let views: Vec<BookmarkView> = nodes
            .into_iter()
            .map(|node| BookmarkView {
                id: node.id.to_string(),
                icon: node
                    .url
                    .as_deref()
                    .and_then(|url| self.icon_ref_for_url(IconSurface::Chrome, profile, url)),
                url: node.url,
                title: node.title,
                children: node.children,
                parent: node.parent.map(|id| id.to_string()),
            })
            .collect();
        self.want_icons(
            IconSurface::Chrome,
            profile,
            views
                .iter()
                .filter(|view| view.icon.is_none())
                .filter_map(|view| view.url.as_deref()),
        );
        views
    }

    pub(super) fn import_destination(&mut self) -> Option<crate::ImportDestination> {
        if !self.bootstrapped {
            self.bootstrap();
        }
        self.windows
            .focused()
            .map(|window| crate::ImportDestination {
                profile: window.profile,
                space: window.space,
            })
            .filter(|destination| {
                self.bookmarks_writable(destination.profile)
                    && self
                        .spaces
                        .get(destination.space)
                        .is_some_and(|space| space.profile == destination.profile)
            })
    }

    pub(super) fn import_into_profile(
        &mut self,
        destination: crate::ImportDestination,
        work: ImportWork,
        done: ImportCompletion,
    ) {
        let crate::ImportDestination { profile, space } = destination;
        if !self.bookmarks_writable(profile)
            || self
                .spaces
                .get(space)
                .is_none_or(|space| space.profile != profile)
        {
            done.finish(None);
            return;
        }
        if let ImportWork::Essentials(sites) = work {
            done.finish(Some(self.keep_imported_sites(space, sites)));
            return;
        }
        if self.bookmarks.imports.len() >= MAX_PENDING_BOOKMARK_CALLS {
            done.finish(None);
            return;
        }
        let Some(reads) = &self.store_reads else {
            done.finish(None);
            return;
        };
        self.bookmarks.next_token = self.bookmarks.next_token.wrapping_add(1);
        let token = self.bookmarks.next_token;
        let icons = matches!(work, ImportWork::Icons(_));
        if reads.request_import(token, profile, work) {
            self.bookmarks.imports.insert(token, done);
            if icons {
                self.bookmarks.icon_imports.insert(token, profile);
            }
        } else {
            done.finish(None);
        }
    }

    /// Reads the focused profile's bookmarks or history out as the text of a
    /// file. A private profile keeps neither, so it has nothing to give.
    pub(super) fn export_from_focused(
        &mut self,
        kind: zephium_core::export::ExportKind,
        done: ExportCompletion,
    ) {
        use zephium_core::export::ExportRefusal;
        let Some(profile) = self.windows.focused().map(|window| window.profile) else {
            done.finish(Err(ExportRefusal::Unavailable));
            return;
        };
        if self
            .profiles
            .get(profile)
            .is_some_and(|p| p.kind == zephium_core::profiles::ProfileKind::Incognito)
        {
            done.finish(Err(ExportRefusal::Private));
            return;
        }
        if !self.bookmarks_writable(profile) || self.bookmarks.exports.len() >= MAX_PENDING_EXPORTS
        {
            done.finish(Err(ExportRefusal::Unavailable));
            return;
        }
        let Some(reads) = &self.store_reads else {
            done.finish(Err(ExportRefusal::Unavailable));
            return;
        };
        self.bookmarks.next_token = self.bookmarks.next_token.wrapping_add(1);
        let token = self.bookmarks.next_token;
        if reads.request_export(token, profile, kind) {
            self.bookmarks.exports.insert(token, done);
        } else {
            done.finish(Err(ExportRefusal::Unavailable));
        }
    }

    pub(super) fn on_export_read(&mut self, token: u64, text: Option<String>) {
        if let Some(done) = self.bookmarks.exports.remove(&token) {
            done.finish(text.ok_or(zephium_core::export::ExportRefusal::Unavailable));
        }
    }

    pub(super) fn on_import_read(&mut self, token: u64, added: Option<u32>) {
        if let Some(done) = self.bookmarks.imports.remove(&token) {
            done.finish(added);
        }
        // Imported Essentials are unloaded tabs that read their icons from the
        // cache; the icons just stored reach them through the same read the
        // sidebar makes at startup.
        if let Some(profile) = self.bookmarks.icon_imports.remove(&token) {
            if let Some(space) = self
                .windows
                .focused()
                .filter(|window| window.profile == profile)
                .map(|window| window.space)
            {
                self.hydrate_favicon_cache(profile, space);
            }
        }
    }

    /// Every completion is answered, including when the shell is shutting down
    /// and the read queue will never deliver.
    pub(super) fn fail_pending_bookmark_calls(&mut self) {
        self.bookmarks.icon_imports.clear();
        for (_, done) in std::mem::take(&mut self.bookmarks.imports) {
            done.finish(None);
        }
        for (_, done) in std::mem::take(&mut self.bookmarks.exports) {
            done.finish(Err(zephium_core::export::ExportRefusal::Unavailable));
        }
        for (_, pending) in std::mem::take(&mut self.bookmarks.pending) {
            if let Pending::Surface(done) = pending {
                done.finish(BookmarkResponse::Error {
                    error: BookmarkError::Unavailable,
                });
            }
        }
    }
}
