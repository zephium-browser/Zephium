//! Clears and lists the website data of one persistent profile's own
//! `WKWebsiteDataStore`. Work's store, another profile's and a private
//! window's are other objects this module never addresses.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::ptr::NonNull;
use std::rc::Rc;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2_foundation::{MainThreadMarker, NSArray, NSDate, NSSet, NSString, NSUUID};
use objc2_web_kit::{
    WKWebsiteDataRecord, WKWebsiteDataStore, WKWebsiteDataTypeCookies, WKWebsiteDataTypeDiskCache,
    WKWebsiteDataTypeFetchCache, WKWebsiteDataTypeFileSystem, WKWebsiteDataTypeIndexedDBDatabases,
    WKWebsiteDataTypeLocalStorage, WKWebsiteDataTypeMemoryCache,
    WKWebsiteDataTypeServiceWorkerRegistrations, WKWebsiteDataTypeSessionStorage,
    WKWebsiteDataTypeWebSQLDatabases,
};
use zephium_core::browsing_data::{
    host_is_on_site, EngineDataCompletion, EngineDataFailure, EngineDataKinds, EngineDataReply,
};
use zephium_core::ids::ProfileId;

use crate::data_kinds::{
    merge_sites, webkit_plan, WebKitDataType, WebKitStep, WEBKIT_CACHE, WEBKIT_SITE_DATA,
};

/// Records scanned from one listing before the rest are left out.
const MAX_RECORDS: usize = 16_384;

/// The native identifier of the profile's own store. Work's store is named by
/// a different, derived identifier and never by this one.
pub(crate) fn profile_store_identifier(profile: ProfileId) -> [u8; 16] {
    profile.bytes()
}

fn profile_store(profile: ProfileId) -> Option<Retained<WKWebsiteDataStore>> {
    let mtm = MainThreadMarker::new()?;
    let identifier = NSUUID::from_bytes(profile_store_identifier(profile));
    // SAFETY: main thread (`mtm`); the retained store outlives these calls.
    let store = unsafe { WKWebsiteDataStore::dataStoreForIdentifier(&identifier, mtm) };
    // SAFETY: as above.
    let exact = unsafe {
        store.isPersistent()
            && store.identifier().map(|value| value.as_bytes())
                == Some(profile_store_identifier(profile))
    };
    exact.then_some(store)
}

fn data_type(kind: WebKitDataType) -> &'static NSString {
    // SAFETY: WebKit defines these constants for the process lifetime.
    unsafe {
        match kind {
            WebKitDataType::Cookies => WKWebsiteDataTypeCookies,
            WebKitDataType::LocalStorage => WKWebsiteDataTypeLocalStorage,
            WebKitDataType::SessionStorage => WKWebsiteDataTypeSessionStorage,
            WebKitDataType::IndexedDb => WKWebsiteDataTypeIndexedDBDatabases,
            WebKitDataType::WebSql => WKWebsiteDataTypeWebSQLDatabases,
            WebKitDataType::ServiceWorkers => WKWebsiteDataTypeServiceWorkerRegistrations,
            WebKitDataType::FetchCache => WKWebsiteDataTypeFetchCache,
            WebKitDataType::FileSystem => WKWebsiteDataTypeFileSystem,
            WebKitDataType::DiskCache => WKWebsiteDataTypeDiskCache,
            WebKitDataType::MemoryCache => WKWebsiteDataTypeMemoryCache,
        }
    }
}

fn data_types(kinds: &[WebKitDataType]) -> Retained<NSSet<NSString>> {
    let names: Vec<&NSString> = kinds.iter().copied().map(data_type).collect();
    NSSet::from_slice(&names)
}

fn finish_once(
    done: &Rc<RefCell<Option<EngineDataCompletion>>>,
    reply: impl FnOnce() -> EngineDataReply,
) {
    if let Some(done) = done.borrow_mut().take() {
        done.finish(reply());
    }
}

/// Removes `kinds` modified at or after `since`, or all of them. Open pages
/// keep running: nothing here reloads or closes a view.
pub(crate) fn clear(
    profile: ProfileId,
    kinds: EngineDataKinds,
    since: Option<i64>,
    done: EngineDataCompletion,
) {
    let plan = webkit_plan(kinds, since);
    if plan.is_empty() {
        done.finish(EngineDataReply::Cleared);
        return;
    }
    let Some(store) = profile_store(profile) else {
        done.finish(EngineDataReply::Failed(EngineDataFailure::Unavailable));
        return;
    };
    run(store, plan.into(), Rc::new(RefCell::new(Some(done))));
}

fn run(
    store: Retained<WKWebsiteDataStore>,
    mut steps: VecDeque<WebKitStep>,
    done: Rc<RefCell<Option<EngineDataCompletion>>>,
) {
    let Some(step) = steps.pop_front() else {
        finish_once(&done, || EngineDataReply::Cleared);
        return;
    };
    let next_store = store.clone();
    let steps = Rc::new(RefCell::new(Some(steps)));
    let next_done = done.clone();
    let proceed: Rc<dyn Fn()> = Rc::new(move || {
        if let Some(steps) = steps.borrow_mut().take() {
            run(next_store.clone(), steps, next_done.clone());
        }
    });
    match step {
        WebKitStep::ModifiedSince { types, since } => {
            let date = NSDate::dateWithTimeIntervalSince1970(since as f64);
            let proceed = RcBlock::new(move || proceed());
            // SAFETY: main thread; WebKit copies the block, which owns what it
            // needs, and the set and date are alive for the call.
            unsafe {
                store.removeDataOfTypes_modifiedSince_completionHandler(
                    &data_types(&types),
                    &date,
                    &proceed,
                );
            }
        }
        WebKitStep::WebRecords { types } => {
            remove_records(&store, &types, |name| !name.is_empty(), proceed);
        }
    }
}

/// Removes `types` from the records of `store` whose name `keep` admits.
fn remove_records(
    store: &Retained<WKWebsiteDataStore>,
    types: &[WebKitDataType],
    keep: impl Fn(&str) -> bool + 'static,
    proceed: Rc<dyn Fn()>,
) {
    let set = data_types(types);
    let removal_store = store.clone();
    let removal_set = set.clone();
    let fetched = RcBlock::new(move |records: NonNull<NSArray<WKWebsiteDataRecord>>| {
        // SAFETY: WebKit owns the array for the callback's duration.
        let records = unsafe { records.as_ref() };
        let chosen: Vec<Retained<WKWebsiteDataRecord>> = records
            .iter()
            .take(MAX_RECORDS)
            .filter(|record| keep(&unsafe { record.displayName() }.to_string()))
            .collect();
        if chosen.is_empty() {
            proceed();
            return;
        }
        let chosen = NSArray::from_retained_slice(&chosen);
        let proceed = proceed.clone();
        let removed = RcBlock::new(move || proceed());
        // SAFETY: main thread (WebKit calls back on it); the block, set and
        // array are alive for the call and WebKit copies the block.
        unsafe {
            removal_store.removeDataOfTypes_forDataRecords_completionHandler(
                &removal_set,
                &chosen,
                &removed,
            );
        }
    });
    // SAFETY: main thread; the set and block are alive for the call.
    unsafe { store.fetchDataRecordsOfTypes_completionHandler(&set, &fetched) };
}

/// The hosts the profile holds cookies and site data for.
pub(crate) fn sites(profile: ProfileId, done: EngineDataCompletion) {
    let Some(store) = profile_store(profile) else {
        done.finish(EngineDataReply::Failed(EngineDataFailure::Unavailable));
        return;
    };
    let done = Rc::new(RefCell::new(Some(done)));
    let fetched = RcBlock::new(move |records: NonNull<NSArray<WKWebsiteDataRecord>>| {
        // SAFETY: WebKit owns the array for the callback's duration.
        let records = unsafe { records.as_ref() };
        let cookies = data_type(WebKitDataType::Cookies);
        let raw: Vec<(String, bool, bool)> = records
            .iter()
            .take(MAX_RECORDS)
            .map(|record| {
                // SAFETY: the record is alive for the callback.
                let (name, types) = unsafe { (record.displayName(), record.dataTypes()) };
                let has_cookies = types.containsObject(cookies);
                let has_storage = types.iter().any(|kind| !kind.isEqualToString(cookies));
                (name.to_string(), has_cookies, has_storage)
            })
            .collect();
        finish_once(&done, || EngineDataReply::Sites {
            sites: merge_sites(raw),
            partial: false,
        });
    });
    // SAFETY: main thread; the set and block are alive for the call.
    unsafe {
        store.fetchDataRecordsOfTypes_completionHandler(&data_types(&WEBKIT_SITE_DATA), &fetched)
    };
}

/// Removes cookies, storage and cache for every record on `site`.
pub(crate) fn forget_site(profile: ProfileId, site: String, done: EngineDataCompletion) {
    let Some(store) = profile_store(profile) else {
        done.finish(EngineDataReply::Failed(EngineDataFailure::Unavailable));
        return;
    };
    let done = Rc::new(RefCell::new(Some(done)));
    let proceed: Rc<dyn Fn()> = Rc::new(move || finish_once(&done, || EngineDataReply::Cleared));
    let types: Vec<WebKitDataType> = WEBKIT_SITE_DATA.into_iter().chain(WEBKIT_CACHE).collect();
    remove_records(
        &store,
        &types,
        move |name| !name.is_empty() && host_is_on_site(name, &site),
        proceed,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_profile_store_is_never_the_work_store_or_another_profiles() {
        let first = ProfileId::generate();
        let second = ProfileId::generate();
        assert_eq!(profile_store_identifier(first), first.bytes());
        assert_ne!(
            profile_store_identifier(first),
            crate::work_website_data::identifier(first)
        );
        assert_ne!(
            profile_store_identifier(first),
            profile_store_identifier(second)
        );
        assert_ne!(
            profile_store_identifier(first),
            crate::work_website_data::identifier(second)
        );
    }

    #[test]
    fn every_data_type_maps_to_its_webkit_constant() {
        let expected = [
            (WebKitDataType::Cookies, "WKWebsiteDataTypeCookies"),
            (
                WebKitDataType::LocalStorage,
                "WKWebsiteDataTypeLocalStorage",
            ),
            (
                WebKitDataType::SessionStorage,
                "WKWebsiteDataTypeSessionStorage",
            ),
            (
                WebKitDataType::IndexedDb,
                "WKWebsiteDataTypeIndexedDBDatabases",
            ),
            (WebKitDataType::WebSql, "WKWebsiteDataTypeWebSQLDatabases"),
            (
                WebKitDataType::ServiceWorkers,
                "WKWebsiteDataTypeServiceWorkerRegistrations",
            ),
            (WebKitDataType::FetchCache, "WKWebsiteDataTypeFetchCache"),
            (WebKitDataType::FileSystem, "WKWebsiteDataTypeFileSystem"),
            (WebKitDataType::DiskCache, "WKWebsiteDataTypeDiskCache"),
            (WebKitDataType::MemoryCache, "WKWebsiteDataTypeMemoryCache"),
        ];
        for (kind, name) in expected {
            assert_eq!(data_type(kind).to_string(), name, "{kind:?}");
        }
    }
}
