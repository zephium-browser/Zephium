//! Clearing browsing data and managing the sites that hold it, for the
//! focused persistent profile only.
//!
//! One clear fans out to the store (history, the download list) and to the
//! engine's store for the same profile (cookies and site data, the cache),
//! and answers once every part has. Nothing here names another profile, a
//! private window, Work's own website data or an extension's storage.

use std::sync::{Arc, Mutex};

use super::*;

use crate::api::BrowsingDataCompletion;
use zephium_core::browsing_data::{
    EngineDataCall, EngineDataCompletion, EngineDataFailure, EngineDataKinds, EngineDataReply,
    EngineSite,
};
use zephium_core::downloads::{DownloadCall, DownloadCompletion, DownloadResponse};
use zephium_ipc::{
    BrowsingDataCall, BrowsingDataError, BrowsingDataRange, BrowsingDataResponse, HistoryCall,
    HistoryRange, HistoryResponse, SiteDataView,
};

/// The parts of one clear still running. The last to settle answers.
struct Join {
    remaining: usize,
    failure: Option<BrowsingDataError>,
    done: Option<BrowsingDataCompletion>,
}

impl Join {
    fn new(parts: usize, done: BrowsingDataCompletion) -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self {
            remaining: parts,
            failure: None,
            done: Some(done),
        }))
    }

    fn settle(join: &Arc<Mutex<Self>>, cleared: bool) {
        Self::settle_result(join, cleared.then_some(()).ok_or(BrowsingDataError::Failed));
    }

    fn settle_result(join: &Arc<Mutex<Self>>, result: Result<(), BrowsingDataError>) {
        let mut state = join.lock().unwrap_or_else(|error| error.into_inner());
        if let Err(error) = result {
            state.failure.get_or_insert(error);
        }
        state.remaining = state.remaining.saturating_sub(1);
        if state.remaining > 0 {
            return;
        }
        if let Some(done) = state.done.take() {
            done.finish(
                state
                    .failure
                    .take()
                    .map_or(BrowsingDataResponse::Cleared, error),
            );
        }
    }
}

fn error(error: BrowsingDataError) -> BrowsingDataResponse {
    BrowsingDataResponse::Error { error }
}

fn failure(failure: EngineDataFailure) -> BrowsingDataError {
    match failure {
        EngineDataFailure::Failed => BrowsingDataError::Failed,
        EngineDataFailure::Unavailable | EngineDataFailure::Unsupported => {
            BrowsingDataError::Unavailable
        }
    }
}

/// The site a host belongs to: its registrable domain, or the host itself
/// for an address or a name with no known suffix.
fn site_of_host(host: &str) -> String {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host.parse::<std::net::IpAddr>().is_ok() || host.starts_with('[') {
        return host;
    }
    psl::domain(host.as_bytes())
        .filter(|domain| domain.suffix().is_known())
        .and_then(|domain| std::str::from_utf8(domain.as_bytes()).ok())
        .map_or(host.clone(), str::to_owned)
}

/// The hosts the engine named, as the sites a person recognises: one row per
/// registrable domain, with what any of its hosts holds.
fn sites_view(hosts: Vec<EngineSite>) -> Vec<SiteDataView> {
    let mut sites: std::collections::BTreeMap<String, SiteDataView> =
        std::collections::BTreeMap::new();
    for host in hosts {
        let site = site_of_host(&host.name);
        let view = sites.entry(site.clone()).or_insert(SiteDataView {
            site,
            cookies: false,
            storage: false,
        });
        view.cookies |= host.cookies;
        view.storage |= host.storage;
    }
    sites.into_values().collect()
}

fn range_start(range: BrowsingDataRange) -> Option<i64> {
    range
        .window_seconds()
        .map(|window| chrono::Utc::now().timestamp() - window)
}

impl Shell {
    pub(super) fn browsing_data_call(
        &mut self,
        expected_profile: ProfileId,
        call: BrowsingDataCall,
        done: BrowsingDataCompletion,
    ) {
        let focused = self
            .windows
            .focused()
            .is_some_and(|window| window.profile == expected_profile);
        if !focused || !call.validate() || self.profile_deletion_quarantines(expected_profile) {
            done.finish(error(BrowsingDataError::Invalid));
            return;
        }
        let partition = match self
            .profiles
            .get(expected_profile)
            .map(|profile| profile.kind)
        {
            Some(ProfileKind::Named) => Partition::Persistent(expected_profile),
            Some(ProfileKind::Default) => Partition::Default(expected_profile),
            // A private window keeps nothing to clear, and its engine store is
            // never reachable from here.
            Some(ProfileKind::Incognito) => {
                done.finish(error(BrowsingDataError::Private));
                return;
            }
            None => {
                done.finish(error(BrowsingDataError::Invalid));
                return;
            }
        };
        match call {
            BrowsingDataCall::Clear {
                range,
                history,
                cookies,
                cache,
                downloads,
            } => self.clear_browsing_data(
                expected_profile,
                partition,
                range,
                [history, cookies, cache, downloads],
                done,
            ),
            BrowsingDataCall::Sites => {
                self.engine.browsing_data_call(
                    partition,
                    EngineDataCall::Sites,
                    EngineDataCompletion::new(move |reply| {
                        done.finish(match reply {
                            EngineDataReply::Sites { sites, partial } => {
                                BrowsingDataResponse::Sites {
                                    sites: sites_view(sites),
                                    partial,
                                }
                            }
                            EngineDataReply::Failed(why) => error(failure(why)),
                            EngineDataReply::Cleared => error(BrowsingDataError::Failed),
                        })
                    }),
                );
            }
            BrowsingDataCall::ForgetSite { site } => self.forget_site(partition, site, None, done),
            BrowsingDataCall::ForgetPage { origin } => {
                let Some(host) = zephium_core::browsing_data::page_origin_host(&origin) else {
                    done.finish(error(BrowsingDataError::Invalid));
                    return;
                };
                self.forget_site(partition, site_of_host(&host), Some(origin), done);
            }
        }
    }

    fn forget_site(
        &mut self,
        partition: Partition,
        site: String,
        origin: Option<String>,
        done: BrowsingDataCompletion,
    ) {
        let join = Join::new(2, done);
        self.discard_browsing_backup(partition.profile(), &join);
        let settled = join.clone();
        self.engine.browsing_data_call(
            partition,
            EngineDataCall::ForgetSite { site, origin },
            EngineDataCompletion::new(move |reply| {
                Join::settle_result(
                    &settled,
                    match reply {
                        EngineDataReply::Cleared => Ok(()),
                        EngineDataReply::Failed(why) => Err(failure(why)),
                        EngineDataReply::Sites { .. } => Err(BrowsingDataError::Failed),
                    },
                );
            }),
        );
    }

    fn discard_browsing_backup(&self, profile: ProfileId, join: &Arc<Mutex<Join>>) {
        let settled = join.clone();
        if !self.store.discard_session_backup(
            profile,
            Box::new(move |cleared| Join::settle(&settled, cleared)),
        ) {
            Join::settle(join, false);
        }
    }

    fn clear_browsing_data(
        &mut self,
        profile: ProfileId,
        partition: Partition,
        range: BrowsingDataRange,
        [history, cookies, cache, downloads]: [bool; 4],
        done: BrowsingDataCompletion,
    ) {
        let since = range_start(range);
        let kinds = EngineDataKinds {
            history,
            site_data: cookies,
            cache,
        };
        let parts = usize::from(!kinds.is_empty())
            + usize::from(downloads)
            + usize::from(history)
            + usize::from(cookies);
        let join = Join::new(parts, done);
        if cookies {
            self.discard_browsing_backup(profile, &join);
        }

        if !kinds.is_empty() {
            let settled = join.clone();
            self.engine.browsing_data_call(
                partition,
                EngineDataCall::Clear { kinds, since },
                EngineDataCompletion::new(move |reply| {
                    Join::settle(&settled, reply == EngineDataReply::Cleared)
                }),
            );
        }
        if downloads {
            let settled = join.clone();
            let call = since.map_or(DownloadCall::Clear, |since| DownloadCall::ClearSince {
                since,
            });
            self.engine.download_call(
                partition,
                call,
                DownloadCompletion::new(move |response| {
                    Join::settle(&settled, matches!(response, DownloadResponse::Applied))
                }),
            );
        }
        if history {
            let settled = join.clone();
            self.history_call(
                profile,
                since.map_or(
                    HistoryCall::Clear {
                        range: HistoryRange::Everything,
                    },
                    |since| HistoryCall::ClearSince { since },
                ),
                crate::api::HistoryCompletion::new(move |response| {
                    Join::settle(
                        &settled,
                        matches!(response, HistoryResponse::Removed { .. }),
                    )
                }),
            );
        }
    }
}
