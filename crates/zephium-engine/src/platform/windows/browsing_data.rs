//! Clears and lists the website data of one persistent profile's WebView2
//! profile. Every call goes through a live view of that profile, so another
//! profile's folder and Work's own are never reachable from here.

use std::time::{SystemTime, UNIX_EPOCH};

use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2, ICoreWebView2CookieList, ICoreWebView2CookieManager, ICoreWebView2Profile2,
    ICoreWebView2_2, COREWEBVIEW2_BROWSING_DATA_KINDS,
};
use webview2_com::{
    CallDevToolsProtocolMethodCompletedHandler, ClearBrowsingDataCompletedHandler,
    GetCookiesCompletedHandler,
};
use windows_core::{w, Interface as _, HSTRING, PWSTR};
use zephium_core::browsing_data::{
    host_is_on_site, page_origin_host, EngineDataCompletion, EngineDataFailure, EngineDataKinds,
    EngineDataReply,
};

use crate::data_kinds::{merge_sites, webview2_kinds};

/// Cookies one listing reads before the rest are left out.
const MAX_COOKIES: u32 = 50_000;

/// Most origins one site removal reaches into storage for; past it, the
/// removal says some data may remain.
const MAX_ORIGINS: usize = 128;

/// What the storage of a single origin holds, except cookies, which the
/// cookie manager owns.
const ORIGIN_STORAGE: &str =
    "local_storage,indexeddb,cache_storage,service_workers,file_systems,websql";

/// A live view of the profile, which is how WebView2 reaches its data.
pub(crate) struct DataTarget {
    core: ICoreWebView2,
    profile: ICoreWebView2Profile2,
}

impl DataTarget {
    pub(crate) fn of(view: &wry::WebView) -> windows_core::Result<Self> {
        use wry::WebViewExtWindows as _;
        Ok(Self {
            core: view.webview(),
            profile: super::profile_for_erasure(view)?,
        })
    }

    fn cookies(&self) -> windows_core::Result<ICoreWebView2CookieManager> {
        // SAFETY: the exact STA-owned view; the manager is reference counted.
        unsafe { self.core.cast::<ICoreWebView2_2>()?.CookieManager() }
    }
}

fn failed() -> EngineDataReply {
    EngineDataReply::Failed(EngineDataFailure::Failed)
}

fn unix_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |elapsed| elapsed.as_secs_f64())
}

/// Removes `kinds` modified at or after `since` (unix seconds), or all of
/// them, from the profile. Open pages keep running.
pub(crate) fn clear(
    target: &DataTarget,
    kinds: EngineDataKinds,
    since: Option<i64>,
    done: EngineDataCompletion,
) {
    let bits = webview2_kinds(kinds);
    if bits == 0 {
        done.finish(EngineDataReply::Cleared);
        return;
    }
    let handler = ClearBrowsingDataCompletedHandler::create(Box::new(move |result| {
        done.finish(if result.is_ok() {
            EngineDataReply::Cleared
        } else {
            failed()
        });
        Ok(())
    }));
    let kinds = COREWEBVIEW2_BROWSING_DATA_KINDS(bits);
    // SAFETY: the STA-owned live profile and handler are reference counted;
    // a refused call drops the handler, which answers `Unavailable`.
    let _ = unsafe {
        match since {
            Some(since) => target.profile.ClearBrowsingDataInTimeRange(
                kinds,
                since as f64,
                // The range ends a day ahead so clock skew cannot spare data.
                unix_seconds() + 86_400.0,
                &handler,
            ),
            None => target.profile.ClearBrowsingData(kinds, &handler),
        }
    };
}

fn domain_of(
    cookie: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Cookie,
) -> String {
    let mut raw = PWSTR::null();
    // SAFETY: the cookie is live; the getter returns an owned out-string.
    let result = unsafe { cookie.Domain(&mut raw) };
    let domain = webview2_com::take_pwstr(raw);
    if result.is_err() {
        return String::new();
    }
    domain.trim_start_matches('.').to_ascii_lowercase()
}

/// Reads every cookie of the profile: WebView2 lists no other kind of data
/// by site, so this is the only list a profile can give.
fn all_cookies(
    manager: &ICoreWebView2CookieManager,
    then: impl FnOnce(ICoreWebView2CookieList) -> windows_core::Result<()> + 'static,
) -> windows_core::Result<()> {
    let handler = GetCookiesCompletedHandler::create(Box::new(move |result, cookies| {
        match (result, cookies) {
            (Ok(()), Some(cookies)) => then(cookies),
            _ => Ok(()),
        }
    }));
    // SAFETY: the manager and handler are reference counted; an empty address
    // asks for every cookie of the profile.
    unsafe { manager.GetCookies(w!(""), &handler) }
}

/// The hosts the profile holds cookies for. Sites with only other storage
/// are missing, so the listing says it is partial.
pub(crate) fn sites(target: &DataTarget, done: EngineDataCompletion) {
    let Ok(manager) = target.cookies() else {
        done.finish(EngineDataReply::Failed(EngineDataFailure::Unavailable));
        return;
    };
    let listed = std::rc::Rc::new(std::cell::RefCell::new(Some(done)));
    let list_done = listed.clone();
    let started = all_cookies(&manager, move |cookies| {
        let Some(done) = list_done.borrow_mut().take() else {
            return Ok(());
        };
        let mut count = 0;
        // SAFETY: the live callback list and an initialized output.
        if unsafe { cookies.Count(&mut count) }.is_err() {
            done.finish(failed());
            return Ok(());
        }
        let hosts = (0..count.min(MAX_COOKIES))
            // SAFETY: the index is inside this list.
            .filter_map(|index| unsafe { cookies.GetValueAtIndex(index) }.ok())
            .map(|cookie| (domain_of(&cookie), true, false));
        done.finish(EngineDataReply::Sites {
            sites: merge_sites(hosts),
            partial: true,
        });
        Ok(())
    });
    if started.is_err() {
        if let Some(done) = listed.borrow_mut().take() {
            done.finish(EngineDataReply::Failed(EngineDataFailure::Unavailable));
        }
    }
}

/// The origins to clear storage for: those Zephium knows of on the site,
/// with their ports, then the site's own and every host a cookie of it names,
/// over https and http. True when some had to be left out.
fn origins_of(site: &str, hosts: &[String], known: &[String]) -> (Vec<String>, bool) {
    let mut origins: Vec<String> = Vec::new();
    let mut left_out = false;
    let mut add = |origin: String| {
        if !origins.contains(&origin) {
            if origins.len() < MAX_ORIGINS {
                origins.push(origin);
            } else {
                left_out = true;
            }
        }
    };
    for origin in known {
        if page_origin_host(origin).is_some_and(|host| host_is_on_site(&host, site)) {
            add(origin.clone());
        }
    }
    for host in std::iter::once(site).chain(hosts.iter().map(String::as_str)) {
        let host = if host.contains(':') && !host.starts_with('[') {
            format!("[{host}]")
        } else {
            host.to_owned()
        };
        add(format!("https://{host}"));
        add(format!("http://{host}"));
    }
    (origins, left_out)
}

/// Removes every cookie of `site`, then the storage of its origins. Data the
/// engine would not remove, or origins left out, make it a partial removal.
pub(crate) fn forget_site(
    target: &DataTarget,
    site: String,
    known: Vec<String>,
    done: EngineDataCompletion,
) {
    let Ok(manager) = target.cookies() else {
        done.finish(EngineDataReply::Failed(EngineDataFailure::Unavailable));
        return;
    };
    let core = target.core.clone();
    let pending = std::rc::Rc::new(std::cell::RefCell::new(Some(done)));
    let list_pending = pending.clone();
    let list_manager = manager.clone();
    let started = all_cookies(&manager, move |cookies| {
        let Some(done) = list_pending.borrow_mut().take() else {
            return Ok(());
        };
        let mut count = 0;
        // SAFETY: the live callback list and an initialized output.
        if unsafe { cookies.Count(&mut count) }.is_err() {
            done.finish(failed());
            return Ok(());
        }
        let mut hosts = Vec::new();
        let mut partial = count > MAX_COOKIES;
        for index in 0..count.min(MAX_COOKIES) {
            // SAFETY: the index is inside this list.
            let Ok(cookie) = (unsafe { cookies.GetValueAtIndex(index) }) else {
                partial = true;
                continue;
            };
            let domain = domain_of(&cookie);
            if !domain.is_empty() && host_is_on_site(&domain, &site) {
                // SAFETY: the manager and cookie are live objects of this profile.
                partial |= unsafe { list_manager.DeleteCookie(&cookie) }.is_err();
                if !hosts.contains(&domain) {
                    hosts.push(domain);
                }
            }
        }
        let (origins, left_out) = origins_of(&site, &hosts, &known);
        clear_origins(core, origins, partial || left_out, done);
        Ok(())
    });
    if started.is_err() {
        if let Some(done) = pending.borrow_mut().take() {
            done.finish(EngineDataReply::Failed(EngineDataFailure::Unavailable));
        }
    }
}

/// Clears each origin in turn; one the engine refuses is passed over and
/// makes the removal partial.
fn clear_origins(
    core: ICoreWebView2,
    mut origins: Vec<String>,
    partial: bool,
    done: EngineDataCompletion,
) {
    if origins.is_empty() {
        done.finish(if partial {
            EngineDataReply::PartlyCleared
        } else {
            EngineDataReply::Cleared
        });
        return;
    }
    let origin = origins.remove(0);
    let parameters = HSTRING::from(
        serde_json::json!({ "origin": origin, "storageTypes": ORIGIN_STORAGE }).to_string(),
    );
    let next = core.clone();
    let handler =
        CallDevToolsProtocolMethodCompletedHandler::create(Box::new(move |result, _response| {
            clear_origins(next, origins, partial || result.is_err(), done);
            Ok(())
        }));
    // SAFETY: the exact STA-owned view; the strings outlive the call and the
    // handler is reference counted. A refused call drops the handler, which
    // answers `Unavailable`.
    let _ = unsafe {
        core.CallDevToolsProtocolMethod(
            &HSTRING::from("Storage.clearDataForOrigin"),
            &parameters,
            &handler,
        )
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use webview2_com::Microsoft::Web::WebView2::Win32::*;

    #[test]
    fn the_kind_bits_are_the_webview2_constants() {
        use crate::data_kinds::webview2 as bits;
        assert_eq!(
            bits::FILE_SYSTEMS,
            COREWEBVIEW2_BROWSING_DATA_KINDS_FILE_SYSTEMS.0
        );
        assert_eq!(
            bits::INDEXED_DB,
            COREWEBVIEW2_BROWSING_DATA_KINDS_INDEXED_DB.0
        );
        assert_eq!(
            bits::LOCAL_STORAGE,
            COREWEBVIEW2_BROWSING_DATA_KINDS_LOCAL_STORAGE.0
        );
        assert_eq!(bits::WEB_SQL, COREWEBVIEW2_BROWSING_DATA_KINDS_WEB_SQL.0);
        assert_eq!(
            bits::CACHE_STORAGE,
            COREWEBVIEW2_BROWSING_DATA_KINDS_CACHE_STORAGE.0
        );
        assert_eq!(bits::COOKIES, COREWEBVIEW2_BROWSING_DATA_KINDS_COOKIES.0);
        assert_eq!(
            bits::DISK_CACHE,
            COREWEBVIEW2_BROWSING_DATA_KINDS_DISK_CACHE.0
        );
        assert_eq!(
            bits::BROWSING_HISTORY,
            COREWEBVIEW2_BROWSING_DATA_KINDS_BROWSING_HISTORY.0
        );
        assert_eq!(
            bits::SERVICE_WORKERS,
            COREWEBVIEW2_BROWSING_DATA_KINDS_SERVICE_WORKERS.0
        );
    }

    #[test]
    fn a_site_removal_reaches_its_own_hosts_over_both_schemes_and_nothing_else() {
        let (origins, left_out) = origins_of(
            "example.com",
            &["www.example.com".to_owned()],
            &["https://app.example.com:8443".to_owned()],
        );
        assert_eq!(
            origins,
            [
                "https://app.example.com:8443",
                "https://example.com",
                "http://example.com",
                "https://www.example.com",
                "http://www.example.com",
            ]
        );
        assert!(!left_out);
        assert_eq!(origins_of("example.com", &[], &[]).0.len(), 2);
    }

    #[test]
    fn a_site_removal_reaches_known_origins_with_their_ports_and_only_on_the_site() {
        // Storage on www with every cookie on the parent domain.
        let (origins, _) = origins_of(
            "example.com",
            &["example.com".to_owned()],
            &[
                "https://www.example.com".to_owned(),
                "http://localhost:3000".to_owned(),
                "https://example.com.evil.test".to_owned(),
                "https://www.example.com/path".to_owned(),
            ],
        );
        assert_eq!(
            origins,
            [
                "https://www.example.com",
                "https://example.com",
                "http://example.com",
            ]
        );
        let (local, _) = origins_of(
            "localhost",
            &[],
            &[
                "http://localhost:3000".to_owned(),
                "http://localhost:8080".to_owned(),
            ],
        );
        assert_eq!(
            local[..2],
            ["http://localhost:3000", "http://localhost:8080"]
        );
        let (v6, _) = origins_of("[::1]", &[], &["http://[::1]:5000".to_owned()]);
        assert_eq!(v6, ["http://[::1]:5000", "https://[::1]", "http://[::1]"]);
    }

    #[test]
    fn a_site_removal_is_bounded_and_says_so() {
        let hosts: Vec<String> = (0..100).map(|n| format!("h{n}.example.com")).collect();
        let (origins, left_out) = origins_of("example.com", &hosts, &[]);
        assert_eq!(origins.len(), MAX_ORIGINS);
        assert!(left_out);
        let (_, within) = origins_of("example.com", &hosts[..10], &[]);
        assert!(!within);
    }
}
