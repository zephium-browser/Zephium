//! Document-lifetime programmatic styles owned by a tab's native content manager.
use std::cell::RefCell;
use std::collections::HashMap;

use glib::prelude::*;
use webkit2gtk::{UserContentManagerExt, WebViewExt};
use zephium_core::ids::{ExtensionInstallId, ItemId};
use zephium_webext_runtime::api::scripting::{CssSource, StyleOrigin};
use zephium_webext_runtime::ApiError;

const MAX_SHEETS: usize = 128;
const MAX_BYTES: usize = 8 * 1024 * 1024;

struct Sheet {
    install: ExtensionInstallId,
    source: CssSource,
    origin: StyleOrigin,
    all_frames: bool,
    bytes: usize,
    native: webkit2gtk::UserStyleSheet,
}

struct Tab {
    view: glib::WeakRef<webkit2gtk::WebView>,
    sheets: Vec<Sheet>,
}

#[derive(Default)]
pub(crate) struct Sheets(RefCell<HashMap<ItemId, Tab>>);

pub(crate) fn check_all_frame_grants(
    request: &zephium_webext_runtime::api::scripting::Request,
    permissions: &zephium_webext_runtime::permissions::InstallPermissions,
) -> Result<(), ApiError> {
    use zephium_webext_runtime::api::scripting::Request;
    use zephium_webext_runtime::pattern::HostPattern;
    if let Request::Css {
        all_frames: true,
        target,
        ..
    } = request
    {
        let grants: Vec<_> = permissions
            .snapshot()
            .origins
            .iter()
            .filter_map(|raw| HostPattern::parse(raw).ok())
            .collect();
        for frame in &target.frames {
            if !matches!(frame.url.scheme(), "http" | "https" | "file") {
                continue;
            }
            if !covers_origin(&frame.url, &grants) {
                return Err(ApiError::other("All-frame CSS requires host grants covering every port; activeTab-only and port-specific grants are not supported on Linux."));
            }
        }
    }
    Ok(())
}

fn covers_origin(url: &url::Url, grants: &[zephium_webext_runtime::pattern::HostPattern]) -> bool {
    use zephium_webext_runtime::pattern::HostPattern;
    let origin = format!("{}://{}/*", url.scheme(), url.host_str().unwrap_or("*"));
    HostPattern::parse(&origin)
        .ok()
        .is_some_and(|origin| grants.iter().any(|grant| grant.contains(&origin)))
}

impl Sheets {
    pub(crate) fn attach(&self, tab: ItemId, view: &webkit2gtk::WebView) {
        self.detach(tab);
        self.0.borrow_mut().insert(
            tab,
            Tab {
                view: view.downgrade(),
                sheets: Vec::new(),
            },
        );
    }

    pub(crate) fn clear(&self, tab: ItemId) {
        if let Some(tab) = self.0.borrow_mut().get_mut(&tab) {
            Self::remove(tab, |_| true);
        }
    }

    pub(crate) fn detach(&self, tab: ItemId) {
        if let Some(mut tab) = self.0.borrow_mut().remove(&tab) {
            Self::remove(&mut tab, |_| true);
        }
    }

    pub(crate) fn remove_install(&self, install: ExtensionInstallId) {
        for tab in self.0.borrow_mut().values_mut() {
            Self::remove(tab, |sheet| sheet.install == install);
        }
    }

    fn remove(tab: &mut Tab, matches: impl Fn(&Sheet) -> bool) {
        let manager = tab
            .view
            .upgrade()
            .and_then(|view| view.user_content_manager());
        tab.sheets.retain(|sheet| {
            if !matches(sheet) {
                return true;
            }
            if let Some(manager) = &manager {
                manager.remove_style_sheet(&sheet.native);
            }
            false
        });
    }

    pub(crate) fn apply(
        &self,
        tab: ItemId,
        install: ExtensionInstallId,
        request: &zephium_webext_runtime::api::scripting::Request,
        text: &str,
    ) -> Result<(), ApiError> {
        let zephium_webext_runtime::api::scripting::Request::Css {
            source,
            origin,
            remove,
            all_frames,
            target,
        } = request
        else {
            return Err(ApiError::malformed_arguments());
        };
        if !all_frames && target.frames.iter().any(|frame| frame.frame.get() != 0) {
            return Err(ApiError::other(
                "Exact non-main-frame CSS targets are not supported on Linux.",
            ));
        }
        if !remove
            && *all_frames
            && target
                .frames
                .iter()
                .any(|frame| !matches!(frame.url.scheme(), "http" | "https" | "file"))
        {
            return Err(ApiError::other(
                "All-frame CSS for inherited-origin documents is not supported on Linux.",
            ));
        }
        let mut tabs = self.0.borrow_mut();
        let tab = tabs
            .get_mut(&tab)
            .ok_or_else(ApiError::context_invalidated)?;
        let manager = tab
            .view
            .upgrade()
            .and_then(|view| view.user_content_manager())
            .ok_or_else(ApiError::context_invalidated)?;
        let matches = |sheet: &Sheet| {
            sheet.install == install
                && sheet.source == *source
                && sheet.origin == *origin
                && sheet.all_frames == *all_frames
        };
        if *remove {
            Self::remove(tab, matches);
            return Ok(());
        }
        if tab.sheets.len() >= MAX_SHEETS
            || tab.sheets.iter().map(|sheet| sheet.bytes).sum::<usize>() + text.len() > MAX_BYTES
        {
            return Err(ApiError::too_many_pending());
        }
        if text.contains('\0') {
            return Err(ApiError::malformed_arguments());
        }
        let patterns: Vec<String> = if *all_frames {
            target
                .frames
                .iter()
                .map(|frame| {
                    let url = &frame.url;
                    format!(
                        "{}://{}{}",
                        url.scheme(),
                        url.host_str().unwrap_or("*"),
                        url.path()
                    )
                })
                .collect()
        } else {
            Vec::new()
        };
        let urls: Vec<&str> = patterns.iter().map(String::as_str).collect();
        let native = webkit2gtk::UserStyleSheet::new(
            text,
            if *all_frames {
                webkit2gtk::UserContentInjectedFrames::AllFrames
            } else {
                webkit2gtk::UserContentInjectedFrames::TopFrame
            },
            match origin {
                StyleOrigin::Author => webkit2gtk::UserStyleLevel::Author,
                StyleOrigin::User => webkit2gtk::UserStyleLevel::User,
            },
            &urls,
            &[],
        );
        manager.add_style_sheet(&native);
        tab.sheets.push(Sheet {
            install,
            source: source.clone(),
            origin: *origin,
            all_frames: *all_frames,
            bytes: text.len(),
            native,
        });
        Ok(())
    }
}

#[cfg(all(test, feature = "native-tests"))]
mod tests {
    use super::*;
    use std::rc::Rc;
    use std::time::{Duration, Instant};
    use zephium_webext_runtime::api::scripting::{InjectionTarget, Request};
    use zephium_webext_runtime::context::DocumentGeneration;

    fn pump(mut done: impl FnMut() -> bool) {
        let until = Instant::now() + Duration::from_secs(15);
        while !done() {
            assert!(Instant::now() < until, "native CSS timeout");
            while gtk::events_pending() {
                gtk::main_iteration_do(false);
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn read(view: &webkit2gtk::WebView, script: &str) -> String {
        let result = Rc::new(RefCell::new(None));
        let capture = result.clone();
        view.evaluate_javascript(
            script,
            None,
            None,
            None::<&gio::Cancellable>,
            move |value| {
                *capture.borrow_mut() = Some(value.unwrap().to_string());
            },
        );
        pump(|| result.borrow().is_some());
        let value = result.borrow_mut().take().unwrap();
        value
    }

    #[test]
    #[ignore = "private GTK session with sandboxed WebKit"]
    fn native_css_live_invisible_origin_removal_and_view_isolation() {
        use webkit2gtk::WebContextExt;
        gtk::init().unwrap();
        let context = webkit2gtk::WebContext::new_ephemeral();
        context.set_sandbox_enabled(true);
        let first = webkit2gtk::WebView::builder()
            .web_context(&context)
            .user_content_manager(&webkit2gtk::UserContentManager::new())
            .build();
        let second = webkit2gtk::WebView::builder()
            .web_context(&context)
            .user_content_manager(&webkit2gtk::UserContentManager::new())
            .build();
        assert_ne!(first.user_content_manager(), second.user_content_manager());
        for view in [&first, &second] {
            view.load_html("<!doctype html><style>body { color: rgb(1, 2, 3) !important; }</style><body>test<iframe srcdoc='<body>child</body>'></iframe>", Some("http://localhost/"));
        }
        pump(|| !first.is_loading() && !second.is_loading());
        let sheets = Sheets::default();
        let tab = ItemId::from(1);
        let install = ExtensionInstallId::from(1);
        sheets.attach(tab, &first);
        let mut request = Request::Css {
            all_frames: false,
            target: InjectionTarget {
                tab_id: 1,
                main_document: DocumentGeneration::new(1).unwrap(),
                frames: Vec::new(),
            },
            source: CssSource::Code("body { color: rgb(9, 8, 7) !important; }".into()),
            origin: StyleOrigin::User,
            remove: false,
        };
        let inspect = "JSON.stringify([getComputedStyle(document.body).color, document.styleSheets.length, document.adoptedStyleSheets.length])";
        assert_eq!(read(&first, inspect), "[\"rgb(1, 2, 3)\",1,0]");
        sheets
            .apply(
                tab,
                install,
                &request,
                "body { color: rgb(9, 8, 7) !important; }",
            )
            .unwrap();
        assert_eq!(read(&first, inspect), "[\"rgb(9, 8, 7)\",1,0]");
        assert_eq!(read(&second, inspect), "[\"rgb(1, 2, 3)\",1,0]");
        assert_eq!(
            read(
                &first,
                "getComputedStyle(document.querySelector('iframe').contentDocument.body).color"
            ),
            "rgb(0, 0, 0)"
        );
        if let Request::Css { remove, .. } = &mut request {
            *remove = true;
        }
        sheets.apply(tab, install, &request, "").unwrap();
        assert_eq!(read(&first, inspect), "[\"rgb(1, 2, 3)\",1,0]");
        if let Request::Css { remove, origin, .. } = &mut request {
            *remove = false;
            *origin = StyleOrigin::Author;
        }
        sheets
            .apply(
                tab,
                install,
                &request,
                "body { color: rgb(9, 8, 7) !important; }",
            )
            .unwrap();
        assert_eq!(read(&first, inspect), "[\"rgb(1, 2, 3)\",1,0]");
        sheets.clear(tab);
        assert_eq!(read(&first, inspect), "[\"rgb(1, 2, 3)\",1,0]");
        first.terminate_web_process();
        second.terminate_web_process();
    }
}

#[cfg(test)]
mod grant_tests {
    use super::*;
    use zephium_webext_runtime::pattern::HostPattern;
    #[test]
    fn native_portless_patterns_require_port_independent_grants() {
        let url = url::Url::parse("https://example.test:8443/path").unwrap();
        assert!(!covers_origin(&url, &[]));
        assert!(!covers_origin(
            &url,
            &[HostPattern::parse("https://example.test:8443/*").unwrap()]
        ));
        assert!(!covers_origin(
            &url,
            &[HostPattern::parse("https://foreign.test/*").unwrap()]
        ));
        assert!(covers_origin(
            &url,
            &[HostPattern::parse("https://example.test/*").unwrap()]
        ));
        assert!(covers_origin(&url, &[HostPattern::all_urls()]));
    }
}
