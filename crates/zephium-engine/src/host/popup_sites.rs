//! Sites a person let open new tabs without a click. WebView2 says whether a
//! request came from a click; WebKit is kept from asking at all unless the
//! view's committed site is allowed, so every request it makes while that
//! gate is open is checked against the committed site here.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use zephium_core::ids::{ItemId, ProfileId};
use zephium_core::permissions::PageOrigin;
use zephium_core::ports::store::MAX_POPUP_SITES;

use super::EngineHost;

#[derive(Default)]
pub(super) struct PopupSites(RefCell<HashSet<PageOrigin>>);

impl PopupSites {
    fn allows(&self, url: &str) -> bool {
        let Some(origin) = url::Url::parse(url)
            .ok()
            .and_then(|url| PageOrigin::from_url(&url).ok())
        else {
            return false;
        };
        self.0
            .try_borrow()
            .is_ok_and(|sites| sites.contains(&origin))
    }

    fn replace(&self, origins: Vec<PageOrigin>) {
        if let Ok(mut sites) = self.0.try_borrow_mut() {
            *sites = origins.into_iter().take(MAX_POPUP_SITES).collect();
        }
    }
}

/// One view's share of its profile's list. On macOS it also reads whether
/// WebKit's automatic-window gate is open on the view, from WebKit itself:
/// a native popup shares its opener's preferences, so a cached flag could be
/// stale.
pub(super) struct ViewPopupPolicy {
    sites: Rc<PopupSites>,
    #[cfg(target_os = "macos")]
    webview: RefCell<Option<objc2::rc::Weak<objc2_web_kit::WKWebView>>>,
}

impl ViewPopupPolicy {
    pub(super) fn new(sites: Rc<PopupSites>) -> Self {
        Self {
            sites,
            #[cfg(target_os = "macos")]
            webview: RefCell::new(None),
        }
    }

    #[cfg(target_os = "macos")]
    pub(super) fn bind(&self, view: &wry::WebView) {
        let native = crate::platform::macos::native_webview(view);
        if let Ok(mut slot) = self.webview.try_borrow_mut() {
            *slot = Some(objc2::rc::Weak::from_retained(&native));
        }
    }

    /// Whether a new-tab request from the document committed at `committed`
    /// may open as though it came from a click.
    pub(super) fn admits(&self, user_initiated: bool, committed: Option<&str>) -> bool {
        admits(
            user_initiated,
            committed.is_some_and(|url| self.sites.allows(url)),
            self.automatic(),
        )
    }

    #[cfg(target_os = "macos")]
    fn automatic(&self) -> bool {
        // Unknown counts as open, so WebKit's word is not taken for a click.
        self.webview
            .try_borrow()
            .ok()
            .and_then(|slot| slot.as_ref()?.load())
            .is_none_or(|view| unsafe {
                view.configuration()
                    .preferences()
                    .javaScriptCanOpenWindowsAutomatically()
            })
    }
    #[cfg(not(target_os = "macos"))]
    fn automatic(&self) -> bool {
        false
    }
}

/// While WebKit's automatic gate is open it reports every request as a
/// click, so only the allowed site's own document is believed then.
fn admits(user_initiated: bool, site_allowed: bool, automatic: bool) -> bool {
    site_allowed || (user_initiated && !automatic)
}

impl EngineHost {
    pub(crate) fn set_popup_sites(&mut self, profile: ProfileId, origins: Vec<PageOrigin>) {
        if self.erasure_tombstones.contains(&profile) {
            return;
        }
        self.popup_sites
            .entry(profile)
            .or_default()
            .replace(origins);
        #[cfg(target_os = "macos")]
        {
            let items: Vec<ItemId> = self
                .views
                .keys()
                .copied()
                .filter(|item| {
                    self.partitions
                        .get(item)
                        .is_some_and(|partition| partition.profile() == profile)
                })
                .collect();
            for item in items {
                self.apply_popup_policy(item);
            }
        }
    }

    pub(super) fn popup_sites_for(&mut self, profile: ProfileId) -> Rc<PopupSites> {
        self.popup_sites.entry(profile).or_default().clone()
    }

    /// Opens WebKit's automatic-window gate only while the view shows an
    /// allowed site.
    #[cfg(target_os = "macos")]
    pub(super) fn apply_popup_policy(&self, item: ItemId) {
        let Some(view) = self.views.get(&item) else {
            return;
        };
        let allowed = view
            .navigation
            .committed_snapshot()
            .is_some_and(|(_, url)| view.popup_policy.sites.allows(&url));
        // SAFETY: the host owns this live WKWebView on the main thread; the
        // configuration copy shares the view's WKPreferences.
        unsafe {
            crate::platform::macos::native_webview(&view.view)
                .configuration()
                .preferences()
                .setJavaScriptCanOpenWindowsAutomatically(allowed);
        }
    }
    #[cfg(not(target_os = "macos"))]
    pub(super) fn apply_popup_policy(&self, _item: ItemId) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sites(origins: &[&str]) -> Rc<PopupSites> {
        let sites = Rc::new(PopupSites::default());
        sites.replace(
            origins
                .iter()
                .map(|origin| PageOrigin::parse_exact(origin).unwrap())
                .collect(),
        );
        sites
    }

    #[test]
    fn an_allowed_site_needs_no_click() {
        let policy = ViewPopupPolicy::new(sites(&["https://mail.example"]));
        assert_eq!(
            policy.admits(true, Some("https://news.example/story")),
            cfg!(not(target_os = "macos"))
        );
        assert!(!policy.admits(false, Some("https://news.example/story")));
        assert!(policy.admits(false, Some("https://mail.example/inbox?id=4")));
        assert!(!policy.admits(false, Some("http://mail.example/")));
        assert!(!policy.admits(false, Some("https://sub.mail.example/")));
        assert!(!policy.admits(false, None));
    }

    #[test]
    fn while_webkit_asks_without_a_click_only_the_allowed_document_is_believed() {
        assert!(admits(true, true, true));
        assert!(!admits(true, false, true));
        assert!(admits(true, false, false));
        assert!(!admits(false, false, false));
    }

    #[test]
    fn the_list_is_bounded_and_replaced_whole() {
        let list = sites(&["https://a.example", "https://b.example"]);
        list.replace(vec![PageOrigin::parse_exact("https://c.example").unwrap()]);
        assert!(!list.allows("https://a.example/"));
        assert!(list.allows("https://c.example/"));
        list.replace(
            (0..MAX_POPUP_SITES + 5)
                .map(|index| PageOrigin::parse_exact(&format!("https://s{index}.example")).unwrap())
                .collect(),
        );
        assert_eq!(list.0.borrow().len(), MAX_POPUP_SITES);
    }
}
