//! Explicit data calls on the host-attested persistent profile manager.
use crate::data_kinds::{
    merge_sites, webkit_plan, WebKitDataType, WebKitStep, WEBKIT_CACHE, WEBKIT_SITE_DATA,
};
use gtk::glib::prelude::ObjectExt;
use gtk::{gio, glib};
use std::collections::VecDeque;
use webkit2gtk::{
    WebsiteDataManager, WebsiteDataManagerExt, WebsiteDataManagerExtManual,
    WebsiteDataTypes as Types,
};
use zephium_core::browsing_data::{
    host_is_on_site, is_site_name, EngineDataCall, EngineDataCompletion, EngineDataFailure,
    EngineDataReply,
};

fn native_types(kinds: &[WebKitDataType]) -> Types {
    kinds.iter().fold(Types::empty(), |types, kind| {
        types
            | match kind {
                WebKitDataType::Cookies => Types::COOKIES,
                WebKitDataType::LocalStorage => Types::LOCAL_STORAGE,
                WebKitDataType::SessionStorage => Types::SESSION_STORAGE,
                WebKitDataType::IndexedDb => Types::INDEXEDDB_DATABASES,
                WebKitDataType::WebSql => Types::WEBSQL_DATABASES,
                WebKitDataType::ServiceWorkers => Types::SERVICE_WORKER_REGISTRATIONS,
                WebKitDataType::FetchCache => Types::DOM_CACHE,
                // GTK exposes no FileSystem data bit, even through ALL.
                WebKitDataType::FileSystem => Types::empty(),
                WebKitDataType::DiskCache => Types::DISK_CACHE,
                WebKitDataType::MemoryCache => Types::MEMORY_CACHE,
            }
    })
}

pub(crate) fn call(manager: &WebsiteDataManager, call: EngineDataCall, done: EngineDataCompletion) {
    match call {
        EngineDataCall::Clear { kinds, since } => {
            // GTK silently leaves cookies behind for a nonzero timespan.
            if kinds.site_data && since.is_some() {
                done.finish(EngineDataReply::Failed(EngineDataFailure::Unsupported));
                return;
            }
            run(
                manager,
                webkit_plan(kinds, since).into(),
                kinds.site_data,
                done,
            );
        }
        EngineDataCall::Sites => {
            manager.fetch(
                native_types(&WEBKIT_SITE_DATA),
                None::<&gio::Cancellable>,
                move |result| {
                    let Ok(records) = result else {
                        done.finish(EngineDataReply::Failed(EngineDataFailure::Failed));
                        return;
                    };
                    let raw = records.iter().filter_map(|record| {
                        record.name().map(|name| {
                            (
                                name.to_string(),
                                record.types().contains(Types::COOKIES),
                                record
                                    .types()
                                    .intersects(native_types(&WEBKIT_SITE_DATA) - Types::COOKIES),
                            )
                        })
                    });
                    let (sites, _) = merge_sites(raw);
                    // File-system-only sites cannot be enumerated by this API.
                    done.finish(EngineDataReply::Sites {
                        sites,
                        partial: true,
                    });
                },
            );
        }
        EngineDataCall::ForgetSite { site, .. } => {
            if !is_site_name(&site) {
                done.finish(EngineDataReply::Failed(EngineDataFailure::Unsupported));
                return;
            }
            let types = native_types(&WEBKIT_SITE_DATA) | native_types(&WEBKIT_CACHE);
            remove_records(
                manager,
                types,
                move |name| host_is_on_site(name, &site),
                move |result| {
                    finish(result, true, done);
                },
            );
        }
    }
}

fn finish(result: Result<(), glib::Error>, partial: bool, done: EngineDataCompletion) {
    done.finish(match result {
        Err(_) => EngineDataReply::Failed(EngineDataFailure::Failed),
        Ok(()) if partial => EngineDataReply::PartlyCleared,
        Ok(()) => EngineDataReply::Cleared,
    });
}

fn run(
    manager: &WebsiteDataManager,
    mut steps: VecDeque<WebKitStep>,
    partial: bool,
    done: EngineDataCompletion,
) {
    let Some(step) = steps.pop_front() else {
        finish(Ok(()), partial, done);
        return;
    };
    let weak: glib::SendWeakRef<WebsiteDataManager> = manager.downgrade().into();
    // GTask delivers on the initiating main context. SendWeakRef satisfies the
    // binding's Send requirement without moving a GTK object between threads.
    let next = move |result: Result<(), glib::Error>| {
        if result.is_err() {
            finish(result, partial, done);
        } else if let Some(manager) = weak.upgrade() {
            run(&manager, steps, partial, done);
        }
    };
    match step {
        WebKitStep::ModifiedSince { types, since } => {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_micros()
                .min(i64::MAX as u128) as i64;
            let Some(span) = clear_span(since, now) else {
                next(Ok(()));
                return;
            };
            manager.clear(
                native_types(&types),
                glib::TimeSpan(span),
                None::<&gio::Cancellable>,
                next,
            );
        }
        WebKitStep::WebRecords { types } => {
            remove_records(manager, native_types(&types), is_site_name, next);
        }
    }
}

fn clear_span(since: i64, now_micros: i64) -> Option<i64> {
    if since <= 0 {
        return Some(0);
    }
    let since = since.saturating_mul(1_000_000);
    (since < now_micros).then(|| now_micros.saturating_sub(since))
}

fn remove_records(
    manager: &WebsiteDataManager,
    types: Types,
    keep: impl Fn(&str) -> bool + 'static,
    done: impl FnOnce(Result<(), glib::Error>) + Send + 'static,
) {
    let target = manager.clone();
    manager.fetch(types, None::<&gio::Cancellable>, move |result| {
        let records = match result {
            Ok(records) => records,
            Err(error) => {
                done(Err(error));
                return;
            }
        };
        let chosen: Vec<_> = records
            .iter()
            .filter(|record| record.name().is_some_and(|name| keep(&name)))
            .collect();
        if chosen.is_empty() {
            done(Ok(()));
            return;
        }
        target.remove(types, &chosen, None::<&gio::Cancellable>, done);
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn future_or_overflowing_cutoffs_never_become_all_time() {
        assert_eq!(super::clear_span(2, 1_000_000), None);
        assert_eq!(super::clear_span(i64::MAX, 1_000_000), None);
        assert_eq!(super::clear_span(1, 2_000_000), Some(1_000_000));
        assert_eq!(super::clear_span(0, 2_000_000), Some(0));
    }
}
