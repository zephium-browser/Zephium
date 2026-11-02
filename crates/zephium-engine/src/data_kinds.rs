//! The native data types a clear names, per engine. The mappings are pure so
//! what a person's choices remove is checked without a running engine.

use zephium_core::browsing_data::{EngineDataKinds, EngineSite, MAX_ENGINE_SITES};

/// The WebKit website data types Zephium removes, named after the
/// `WKWebsiteDataType` constants.
#[cfg(any(target_os = "macos", test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WebKitDataType {
    Cookies,
    LocalStorage,
    SessionStorage,
    IndexedDb,
    WebSql,
    ServiceWorkers,
    FetchCache,
    FileSystem,
    DiskCache,
    MemoryCache,
}

/// Cookies, and the storage a page keeps for itself.
#[cfg(any(target_os = "macos", test))]
pub(crate) const WEBKIT_SITE_DATA: [WebKitDataType; 8] = [
    WebKitDataType::Cookies,
    WebKitDataType::LocalStorage,
    WebKitDataType::SessionStorage,
    WebKitDataType::IndexedDb,
    WebKitDataType::WebSql,
    WebKitDataType::ServiceWorkers,
    WebKitDataType::FetchCache,
    WebKitDataType::FileSystem,
];

/// The HTTP cache.
#[cfg(any(target_os = "macos", test))]
pub(crate) const WEBKIT_CACHE: [WebKitDataType; 2] =
    [WebKitDataType::DiskCache, WebKitDataType::MemoryCache];

/// One removal WebKit performs.
#[cfg(any(target_os = "macos", test))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum WebKitStep {
    /// Everything of these types modified at or after `since` (unix seconds),
    /// whatever origin holds it.
    ModifiedSince {
        types: Vec<WebKitDataType>,
        since: i64,
    },
    /// These types for the records of web sites the store lists. A record
    /// without a host name, which is how an extension's or an internal
    /// page's own origin would appear, is left alone.
    WebRecords { types: Vec<WebKitDataType> },
}

/// The removals that clear `kinds` back to `since`. WebKit has no visited
/// link store to clear, so history is Zephium's own and adds no step.
#[cfg(any(target_os = "macos", test))]
pub(crate) fn webkit_plan(kinds: EngineDataKinds, since: Option<i64>) -> Vec<WebKitStep> {
    let mut types = Vec::new();
    if kinds.site_data {
        types.extend(WEBKIT_SITE_DATA);
    }
    if kinds.cache {
        types.extend(WEBKIT_CACHE);
    }
    if types.is_empty() {
        return Vec::new();
    }
    match since {
        // WebKit dates a removal but not a record, so a time range can only
        // be removed by date, across every origin.
        Some(since) => vec![WebKitStep::ModifiedSince { types, since }],
        None => {
            let mut steps = Vec::new();
            if kinds.site_data {
                steps.push(WebKitStep::WebRecords {
                    types: WEBKIT_SITE_DATA.to_vec(),
                });
            }
            if kinds.cache {
                steps.push(WebKitStep::ModifiedSince {
                    types: WEBKIT_CACHE.to_vec(),
                    since: 0,
                });
            }
            steps
        }
    }
}

/// The `COREWEBVIEW2_BROWSING_DATA_KINDS` bits Zephium removes.
#[cfg(any(target_os = "windows", test))]
pub(crate) mod webview2 {
    pub(crate) const FILE_SYSTEMS: i32 = 1;
    pub(crate) const INDEXED_DB: i32 = 2;
    pub(crate) const LOCAL_STORAGE: i32 = 4;
    pub(crate) const WEB_SQL: i32 = 8;
    pub(crate) const CACHE_STORAGE: i32 = 16;
    pub(crate) const COOKIES: i32 = 64;
    pub(crate) const DISK_CACHE: i32 = 256;
    pub(crate) const BROWSING_HISTORY: i32 = 4096;
    pub(crate) const SERVICE_WORKERS: i32 = 32768;
    /// Cookies, and the storage a page keeps for itself.
    pub(crate) const SITE_DATA: i32 = FILE_SYSTEMS
        | INDEXED_DB
        | LOCAL_STORAGE
        | WEB_SQL
        | CACHE_STORAGE
        | COOKIES
        | SERVICE_WORKERS;
}

/// The WebView2 kinds that clear `kinds`. Session storage has no kind of its
/// own: WebView2 keeps it in memory with the page that owns it.
#[cfg(any(target_os = "windows", test))]
pub(crate) fn webview2_kinds(kinds: EngineDataKinds) -> i32 {
    let mut bits = 0;
    if kinds.history {
        bits |= webview2::BROWSING_HISTORY;
    }
    if kinds.site_data {
        bits |= webview2::SITE_DATA;
    }
    if kinds.cache {
        bits |= webview2::DISK_CACHE;
    }
    bits
}

/// Hosts a listing names, one entry per host, in the order a person reads
/// them. A name that is not a host is dropped.
pub(crate) fn merge_sites(raw: impl IntoIterator<Item = (String, bool, bool)>) -> Vec<EngineSite> {
    let mut sites: Vec<EngineSite> = Vec::new();
    for (name, cookies, storage) in raw {
        let name = name.trim().trim_start_matches('.').to_ascii_lowercase();
        if !zephium_core::browsing_data::is_site_name(&name) {
            continue;
        }
        match sites.iter_mut().find(|site| site.name == name) {
            Some(site) => {
                site.cookies |= cookies;
                site.storage |= storage;
            }
            None => sites.push(EngineSite {
                name,
                cookies,
                storage,
            }),
        }
    }
    sites.sort_by(|a, b| a.name.cmp(&b.name));
    sites.truncate(MAX_ENGINE_SITES);
    sites
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn kinds(history: bool, site_data: bool, cache: bool) -> EngineDataKinds {
        EngineDataKinds {
            history,
            site_data,
            cache,
        }
    }

    #[test]
    fn site_data_is_cookies_and_every_storage_a_page_keeps_but_no_cache() {
        let cookies_and_storage = webkit_plan(kinds(false, true, false), None);
        assert_eq!(
            cookies_and_storage,
            vec![WebKitStep::WebRecords {
                types: WEBKIT_SITE_DATA.to_vec()
            }]
        );
        for kept in [
            WebKitDataType::Cookies,
            WebKitDataType::LocalStorage,
            WebKitDataType::SessionStorage,
            WebKitDataType::IndexedDb,
            WebKitDataType::ServiceWorkers,
            WebKitDataType::FetchCache,
        ] {
            assert!(WEBKIT_SITE_DATA.contains(&kept), "{kept:?}");
        }
        assert!(!WEBKIT_SITE_DATA.contains(&WebKitDataType::DiskCache));
        assert!(!WEBKIT_SITE_DATA.contains(&WebKitDataType::MemoryCache));
    }

    #[test]
    fn the_http_cache_is_the_disk_and_memory_cache_only() {
        assert_eq!(
            webkit_plan(kinds(false, false, true), None),
            vec![WebKitStep::ModifiedSince {
                types: WEBKIT_CACHE.to_vec(),
                since: 0
            }]
        );
        assert!(!WEBKIT_CACHE.contains(&WebKitDataType::Cookies));
    }

    #[test]
    fn history_alone_removes_nothing_from_webkit() {
        assert!(webkit_plan(kinds(true, false, false), None).is_empty());
        assert!(webkit_plan(kinds(true, false, false), Some(1)).is_empty());
        assert!(webkit_plan(EngineDataKinds::default(), None).is_empty());
    }

    #[test]
    fn a_time_range_removes_by_date_in_one_step() {
        let plan = webkit_plan(kinds(true, true, true), Some(1_700_000_000));
        assert_eq!(plan.len(), 1);
        let WebKitStep::ModifiedSince { types, since } = &plan[0] else {
            panic!("a bounded clear removes by date");
        };
        assert_eq!(*since, 1_700_000_000);
        assert_eq!(types.len(), WEBKIT_SITE_DATA.len() + WEBKIT_CACHE.len());
    }

    #[test]
    fn all_time_site_data_never_goes_by_date_so_it_stays_on_web_sites() {
        let plan = webkit_plan(kinds(false, true, true), None);
        assert!(matches!(plan[0], WebKitStep::WebRecords { .. }));
        assert!(plan[1..].iter().all(
            |step| matches!(step, WebKitStep::ModifiedSince { types, .. } if types == &WEBKIT_CACHE)
        ));
    }

    #[test]
    fn webview2_bits_follow_the_documented_kinds() {
        assert_eq!(webview2_kinds(EngineDataKinds::default()), 0);
        assert_eq!(webview2_kinds(kinds(true, false, false)), 4096);
        assert_eq!(webview2_kinds(kinds(false, false, true)), 256);
        let site = webview2_kinds(kinds(false, true, false));
        for bit in [1, 2, 4, 8, 16, 64, 32768] {
            assert_eq!(site & bit, bit, "{bit}");
        }
        // Neither the download list, autofill, passwords nor settings.
        for bit in [512, 1024, 2048, 8192, 16384] {
            assert_eq!(site & bit, 0, "{bit}");
        }
        assert_eq!(site & 256, 0, "site data leaves the HTTP cache");
        assert_eq!(webview2_kinds(kinds(true, true, true)), site | 4096 | 256);
    }

    #[test]
    fn a_listing_names_each_host_once_sorted_and_only_hosts() {
        let sites = merge_sites([
            ("Zed.example".to_owned(), true, false),
            (".example.com".to_owned(), false, true),
            ("example.com".to_owned(), true, false),
            (String::new(), true, true),
            ("Local Files".to_owned(), false, true),
            ("a.example".to_owned(), false, true),
        ]);
        let names: Vec<_> = sites.iter().map(|site| site.name.as_str()).collect();
        assert_eq!(names, ["a.example", "example.com", "zed.example"]);
        assert!(sites[1].cookies && sites[1].storage);
    }
}
