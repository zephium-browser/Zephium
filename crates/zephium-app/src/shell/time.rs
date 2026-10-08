//! Time on the web and focus sessions. Attention is re-derived from shell
//! state after every command and only changes produce work; tallies reach the
//! store on the existing maintenance tick, and focus wakes once per phase.

use super::*;

use crate::api::TimeCompletion;
use zephium_core::time::{
    site_covers, Attention, FocusEvent, FocusGate, FocusPhase, FocusPlan, FocusRecord,
    FocusSession, Ledger, Place, TimeQuery, TimeReport, Tracker, HOUR_MS, MAX_BLOCKED_SITES,
};
use zephium_ipc::{
    FocusControl, FocusDayView, FocusPhaseView, FocusStatus, FocusView, IconSurface, ShutSiteView,
    SiteTimeView, TimeBucketView, TimeCall, TimeError, TimeResponse,
};

/// The running session, kept so a relaunch picks it up where it was.
const SESSION_KEY: &str = "focus.session";
const DEFAULT_RETENTION_DAYS: i64 = 90;
/// Fresh tabs whose first load focus shut, remembered so they can open once
/// the site is let through.
const MAX_SHUT_LOADS: usize = 64;
const MAX_PENDING_FOCUS_RECORDS: usize = 8;

pub(super) struct TimeState {
    tracker: Tracker,
    ledger: Ledger,
    app_active: bool,
    awake: bool,
    enabled: bool,
    retention_days: i64,
    focus: Option<FocusSession>,
    blocked: Vec<String>,
    /// The tab the focus surface covers, and the site it would show.
    covered: Option<(ItemId, String)>,
    /// Tabs on shut sites, their media held still while the round runs.
    stilled: std::collections::HashSet<ItemId>,
    /// The gate the engine holds, so an unchanged one is not sent again.
    gate: Option<FocusGate>,
    shut_loads: std::collections::HashMap<ItemId, String>,
    pending_focus_records: std::collections::VecDeque<FocusRecord>,
    focus_retry_at: Option<std::time::Instant>,
    focus_retry_failures: u32,
}

impl TimeState {
    pub(super) fn load(store: &SharedStore) -> Self {
        let setting = |key: &str| store.app_setting(key).unwrap_or_default();
        let mut state = Self {
            tracker: Tracker::default(),
            ledger: Ledger::default(),
            app_active: false,
            awake: true,
            enabled: true,
            retention_days: DEFAULT_RETENTION_DAYS,
            focus: serde_json::from_str::<FocusSession>(&setting(SESSION_KEY))
                .ok()
                .filter(FocusSession::valid),
            blocked: Vec::new(),
            covered: None,
            stilled: std::collections::HashSet::new(),
            gate: None,
            shut_loads: std::collections::HashMap::new(),
            pending_focus_records: std::collections::VecDeque::new(),
            focus_retry_at: None,
            focus_retry_failures: 0,
        };
        for key in ["time.track", "time.retention", "focus.blocked"] {
            state.apply_setting(key, &setting(key));
        }
        state
    }

    fn apply_setting(&mut self, key: &str, value: &str) {
        match key {
            "time.track" => self.enabled = value != "false",
            "time.retention" => {
                self.retention_days = value.parse().unwrap_or(DEFAULT_RETENTION_DAYS);
            }
            "focus.blocked" => self.blocked = blocked_sites(value),
            _ => {}
        }
    }
}

/// The block list as stored: one normalized site per line.
fn blocked_sites(value: &str) -> Vec<String> {
    value
        .lines()
        .filter(|line| !line.is_empty())
        .take(MAX_BLOCKED_SITES)
        .map(str::to_owned)
        .collect()
}

/// Wall-clock milliseconds as the local zone reads them.
fn local_ms(utc_ms: i64) -> i64 {
    use chrono::TimeZone;
    let offset = chrono::Local
        .timestamp_millis_opt(utc_ms)
        .single()
        .map_or(0, |local| i64::from(local.offset().local_minus_utc()));
    utc_ms + offset * 1000
}

fn utc_now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn local_day(utc_ms: i64) -> i64 {
    local_ms(utc_ms).div_euclid(24 * HOUR_MS)
}

/// A page's registrable domain, so `m.youtube.com` and `www.youtube.com`
/// are one site. Hosts without a known suffix, such as `localhost`, stay
/// whole.
fn site_of(url: &url::Url) -> Option<String> {
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let host = url.host_str()?.trim_end_matches('.').to_ascii_lowercase();
    let site = psl::domain(host.as_bytes())
        .filter(|domain| domain.suffix().is_known())
        .and_then(|domain| std::str::from_utf8(domain.as_bytes()).ok())
        .map_or_else(|| host.clone(), str::to_owned);
    (!site.is_empty() && site.len() <= zephium_core::time::MAX_SITE_BYTES).then_some(site)
}

fn seconds(ms: i64) -> u32 {
    u32::try_from(ms.max(0) / 1000).unwrap_or(u32::MAX)
}

fn phase_view(phase: FocusPhase) -> FocusPhaseView {
    match phase {
        FocusPhase::Focus => FocusPhaseView::Focus,
        FocusPhase::Break => FocusPhaseView::Break,
        FocusPhase::LongBreak => FocusPhaseView::LongBreak,
    }
}

impl Shell {
    fn time_attention(&self) -> Option<Attention> {
        let time = &self.time;
        if !time.enabled || !time.app_active || !time.awake || !self.window_visible {
            return None;
        }
        let window = self.windows.focused()?;
        let profile = window.profile;
        let counted = self
            .profiles
            .get(profile)
            .is_some_and(|p| p.kind != ProfileKind::Incognito)
            && !self.degraded_storage_profiles.contains(&profile)
            && !self.profile_deletion_quarantines(profile);
        if !counted {
            return None;
        }
        let place = match self.active_browser_page() {
            Some(crate::BrowserPage::Work) => Place::Work,
            Some(_) => return None,
            None => {
                let tab = self.items.tab(window.active?)?;
                if tab.content != zephium_core::item::TabContent::Web {
                    return None;
                }
                Place::Site(site_of(tab.url.as_ref()?)?)
            }
        };
        Some(Attention { profile, place })
    }

    /// Follows attention after a command. Unchanged attention costs one
    /// comparison and no write.
    pub(super) fn refresh_time(&mut self) {
        let next = self.time_attention();
        if self.time.tracker.current() == next.as_ref() {
            return;
        }
        let wall = local_ms(utc_now_ms());
        let transition = self.time.tracker.set(next, std::time::Instant::now(), wall);
        if let Some(segment) = transition.closed {
            self.time.ledger.spend(&segment);
        }
        if let Some((profile, site)) = transition.opened {
            self.time.ledger.open(profile, site, wall);
        }
    }

    /// Writes what has been counted, up to now, for every profile or one.
    pub(super) fn flush_time(&mut self, only: Option<ProfileId>) {
        self.flush_pending_focus_records(false);
        let wall = local_ms(utc_now_ms());
        if let Some(segment) = self
            .time
            .tracker
            .checkpoint(std::time::Instant::now(), wall)
        {
            self.time.ledger.spend(&segment);
        }
        let keep_from_hour = wall.div_euclid(HOUR_MS) - self.time.retention_days * 24;
        for profile in self.time.ledger.profiles() {
            if only.is_some_and(|only| only != profile) {
                continue;
            }
            let tallies = self.time.ledger.take(profile);
            if !self
                .store
                .record_time(profile, tallies.clone(), keep_from_hour)
            {
                self.time.ledger.restore(profile, tallies);
            }
        }
    }

    pub(super) fn set_app_active(&mut self, active: bool) {
        self.time.app_active = active;
    }

    /// Sleep, a locked screen or a dark display stop the clock; waking runs
    /// the focus clock forward, since a timer may have slept through its end.
    pub(super) fn set_system_awake(&mut self, awake: bool) {
        self.time.awake = awake;
        if awake {
            self.focus_wake();
        }
    }

    pub(super) fn apply_time_setting(&mut self, key: &str, value: &str) {
        let was_enabled = self.time.enabled;
        let was_blocked = self.time.blocked.clone();
        self.time.apply_setting(key, value);
        if was_enabled && !self.time.enabled {
            self.flush_time(None);
        }
        if was_blocked != self.time.blocked {
            self.sync_focus_gate();
            self.project_focus();
        }
    }

    /// Clearing history clears the time spent over the same span.
    pub(super) fn clear_time(&mut self, profile: ProfileId, since_seconds: Option<i64>) -> bool {
        let since_hour =
            since_seconds.map(|since| local_ms(since.saturating_mul(1000)).div_euclid(HOUR_MS));
        self.flush_time(Some(profile));
        if !self.store.clear_time(profile, since_hour) {
            return false;
        }
        self.time.tracker.forget(profile);
        self.time
            .ledger
            .forget(profile, since_hour.map(|hour| hour * HOUR_MS));
        true
    }

    pub(super) fn time_call(
        &mut self,
        expected_profile: ProfileId,
        call: TimeCall,
        done: TimeCompletion,
    ) {
        let focused = self
            .windows
            .focused()
            .is_some_and(|window| window.profile == expected_profile);
        if !focused || !call.validate() {
            done.finish(TimeResponse::Error {
                error: TimeError::Invalid,
            });
            return;
        }
        let unavailable = |done: TimeCompletion| {
            done.finish(TimeResponse::Error {
                error: TimeError::Unavailable,
            })
        };
        match call {
            TimeCall::Report {
                from_hour,
                bucket_hours,
                buckets,
                site,
            } => {
                let private = self
                    .profiles
                    .get(expected_profile)
                    .is_none_or(|p| p.kind == ProfileKind::Incognito);
                if private || self.degraded_storage_profiles.contains(&expected_profile) {
                    unavailable(done);
                    return;
                }
                // The store runs writes and reads in one queue, so this report
                // sees everything counted up to now.
                self.flush_time(Some(expected_profile));
                let callback = self.self_queue.as_ref().map(|queue| CallbackHandle {
                    queue: Arc::downgrade(&queue.inner),
                });
                let returned = done.clone();
                let accepted = self.store.time_report(
                    expected_profile,
                    TimeQuery {
                        from_hour: i64::from(from_hour),
                        bucket_hours,
                        buckets,
                        site,
                    },
                    Box::new(move |report| {
                        let delivered = callback.is_some_and(|callback| {
                            callback.dispatch(Command::TimeReportRead {
                                profile: expected_profile,
                                report,
                                done: returned.clone(),
                            })
                        });
                        if !delivered {
                            unavailable(returned);
                        }
                    }),
                );
                if !accepted {
                    unavailable(done);
                }
            }
            TimeCall::FocusDays { from_day, days } => {
                let returned = done.clone();
                let accepted = self.store.focus_days(
                    i64::from(from_day),
                    days,
                    Box::new(move |days| match days {
                        Some(days) => returned.finish(TimeResponse::FocusDays {
                            days: days
                                .into_iter()
                                .filter_map(|day| {
                                    Some(FocusDayView {
                                        day: i32::try_from(day.day).ok()?,
                                        seconds: seconds(day.focused_ms),
                                        sessions: day.sessions,
                                        completed: day.completed,
                                    })
                                })
                                .collect(),
                        }),
                        None => unavailable(returned),
                    }),
                );
                if !accepted {
                    unavailable(done);
                }
            }
        }
    }

    pub(super) fn on_time_report(
        &mut self,
        profile: ProfileId,
        report: Option<TimeReport>,
        done: TimeCompletion,
    ) {
        let Some(report) = report else {
            done.finish(TimeResponse::Error {
                error: TimeError::Unavailable,
            });
            return;
        };
        let bucket = |bucket: zephium_core::time::BucketTime| TimeBucketView {
            browse: seconds(bucket.browse_ms),
            work: seconds(bucket.work_ms),
        };
        let pages: Vec<String> = report
            .sites
            .iter()
            .map(|entry| format!("https://{}/", entry.site))
            .collect();
        let sites = report
            .sites
            .into_iter()
            .zip(&pages)
            .map(|(entry, page)| SiteTimeView {
                icon: self.icon_ref_for_url(IconSurface::Chrome, profile, page),
                site: entry.site,
                seconds: seconds(entry.spent_ms),
                opens: entry.opens,
                series: entry.series.into_iter().map(seconds).collect(),
            })
            .collect::<Vec<_>>();
        self.want_icons(
            IconSurface::Chrome,
            profile,
            sites
                .iter()
                .zip(&pages)
                .filter(|(site, _)| site.icon.is_none())
                .map(|(_, page)| page.as_str()),
        );
        self.publish_icons();
        done.finish(TimeResponse::Report {
            buckets: report.buckets.into_iter().map(bucket).collect(),
            previous: bucket(report.previous),
            sites,
        });
    }

    pub(super) fn operation_focus(&mut self, control: FocusControl) -> OperationDisposition {
        if !control.validate() {
            return operation_result(OperationOutcome::Rejected, OperationReason::InvalidInput);
        }
        let now = utc_now_ms();
        match control {
            FocusControl::Start { minutes, breaks } => {
                self.flush_pending_focus_records(false);
                // Reserve capacity for both a replaced running session and
                // the new session's eventual completion before changing it.
                if self.time.pending_focus_records.len() + usize::from(self.time.focus.is_some())
                    >= MAX_PENDING_FOCUS_RECORDS
                {
                    return operation_result(
                        OperationOutcome::NativeAdmissionFailed,
                        OperationReason::StoreAdmissionRejected,
                    );
                }
                if let Some(running) = self.time.focus.take() {
                    if !self.record_focus(running.clone().stop(now)) {
                        self.time.focus = Some(running);
                        return operation_result(
                            OperationOutcome::NativeAdmissionFailed,
                            OperationReason::StoreAdmissionRejected,
                        );
                    }
                }
                let plan = FocusPlan {
                    minutes: u16::try_from(minutes).unwrap_or(25),
                    breaks,
                };
                self.time.focus = Some(FocusSession::start(plan, now));
            }
            FocusControl::Stop => {
                let Some(running) = self.time.focus.take() else {
                    return operation_result(
                        OperationOutcome::NoOp,
                        OperationReason::StateUnchanged,
                    );
                };
                if !self.record_focus(running.clone().stop(now)) {
                    self.time.focus = Some(running);
                    return operation_result(
                        OperationOutcome::NativeAdmissionFailed,
                        OperationReason::StoreAdmissionRejected,
                    );
                }
            }
            FocusControl::Allow { site } => {
                let Some(site) = zephium_core::time::normalize_site(&site) else {
                    return operation_result(
                        OperationOutcome::Rejected,
                        OperationReason::InvalidInput,
                    );
                };
                let allowed = self
                    .time
                    .focus
                    .as_mut()
                    .is_some_and(|session| session.allow(site.clone(), now));
                if !allowed {
                    return operation_result(
                        OperationOutcome::Rejected,
                        OperationReason::InvalidInput,
                    );
                }
                self.focus_changed();
                self.open_shut_loads(Some(&site));
                return operation_result(
                    OperationOutcome::Applied,
                    OperationReason::MutationApplied,
                );
            }
            FocusControl::Skip => {
                let Some(session) = self.time.focus.as_mut() else {
                    return operation_result(
                        OperationOutcome::NoOp,
                        OperationReason::StateUnchanged,
                    );
                };
                session.phase_ends_ms = now.max(session.phase_started_ms + 1);
                self.focus_wake();
                return operation_result(
                    OperationOutcome::Applied,
                    OperationReason::MutationApplied,
                );
            }
        }
        self.focus_changed();
        if self.time.pending_focus_records.is_empty() {
            operation_result(OperationOutcome::Applied, OperationReason::MutationApplied)
        } else {
            operation_result(
                OperationOutcome::Deferred,
                OperationReason::StoreWorkPending,
            )
        }
    }

    /// Runs the focus clock to now: phases that ended move on, and a single
    /// round that ran out is kept and closed.
    pub(super) fn focus_wake(&mut self) {
        self.flush_pending_focus_records(false);
        let Some(session) = self.time.focus.as_mut() else {
            self.schedule_focus_wake();
            self.sync_focus_gate();
            return;
        };
        let original = session.clone();
        let (events, finished) = session.advance(utc_now_ms());
        if events.is_empty() {
            // An allowance may have run out.
            self.schedule_focus_wake();
            self.sync_focus_gate();
            return;
        }
        if let Some(record) = finished {
            self.time.focus = None;
            if !self.record_focus(record) {
                self.time.focus = Some(original);
                self.time.focus_retry_at =
                    Some(std::time::Instant::now() + std::time::Duration::from_secs(1));
                self.schedule_focus_wake();
                return;
            }
        }
        if let Some(last) = events.last() {
            let alert = match last {
                FocusEvent::Finished => "finished",
                FocusEvent::Phase {
                    began: FocusPhase::Focus,
                    ..
                } => "focus",
                FocusEvent::Phase { .. } => "break",
            };
            (self.emit)(Projection::UiCommand(format!("focus.alert={alert}")));
        }
        self.focus_changed();
    }

    fn record_focus(&mut self, record: FocusRecord) -> bool {
        if record.focused_ms <= 0 {
            return true;
        }
        if self.time.pending_focus_records.len() >= MAX_PENDING_FOCUS_RECORDS {
            return false;
        }
        self.time.pending_focus_records.push_back(record);
        self.flush_pending_focus_records(false);
        true
    }

    pub(super) fn flush_pending_focus_records(&mut self, force: bool) {
        if !force
            && self
                .time
                .focus_retry_at
                .is_some_and(|at| std::time::Instant::now() < at)
        {
            return;
        }
        while let Some(record) = self.time.pending_focus_records.front().copied() {
            if !self
                .store
                .record_focus(record, local_day(record.started_ms))
            {
                let delay =
                    std::time::Duration::from_secs(1_u64 << self.time.focus_retry_failures.min(5))
                        .min(std::time::Duration::from_secs(30));
                self.time.focus_retry_failures = self.time.focus_retry_failures.saturating_add(1);
                self.time.focus_retry_at = Some(std::time::Instant::now() + delay);
                self.schedule_focus_wake();
                return;
            }
            self.time.pending_focus_records.pop_front();
        }
        self.time.focus_retry_at = None;
        self.time.focus_retry_failures = 0;
    }

    pub(super) fn has_unadmitted_time_writes(&self) -> bool {
        !self.time.pending_focus_records.is_empty() || !self.time.ledger.profiles().is_empty()
    }

    fn focus_changed(&mut self) {
        let saved = self
            .time
            .focus
            .as_ref()
            .and_then(|session| serde_json::to_string(session).ok())
            .unwrap_or_default();
        let _ = self.store.set_app_setting(SESSION_KEY.to_owned(), saved);
        self.schedule_focus_wake();
        self.sync_focus_gate();
        self.project_focus();
    }

    fn focus_gate_now(&self) -> Option<FocusGate> {
        self.time
            .focus
            .as_ref()?
            .gate(&self.time.blocked, utc_now_ms())
    }

    pub(super) fn focus_covers(&self) -> bool {
        self.time.covered.is_some()
    }

    /// The active tab and the site it shows, when a focus round shuts it.
    fn focus_cover_target(&self) -> Option<(ItemId, String)> {
        if self.active_browser_page().is_some() || self.time.focus.is_none() {
            return None;
        }
        let gate = self.focus_gate_now()?;
        let id = self.windows.focused()?.active?;
        let tab = self.items.tab(id)?;
        let host = match tab.url.as_ref() {
            Some(url) if tab.content == zephium_core::item::TabContent::Web => {
                url.host_str()?.to_owned()
            }
            Some(_) => return None,
            None => url::Url::parse(self.time.shut_loads.get(&id)?)
                .ok()?
                .host_str()?
                .to_owned(),
        };
        let site = host.strip_prefix("www.").unwrap_or(&host).to_owned();
        gate.blocks(&host, utc_now_ms()).then_some((id, site))
    }

    /// Covers the active tab while it shows a shut site. Chrome draws the
    /// focus surface in the content area the page gives up.
    pub(super) fn refresh_focus_cover(&mut self) {
        let next = self.focus_cover_target();
        if next == self.time.covered {
            return;
        }
        let site = next
            .as_ref()
            .map(|(_, site)| site.clone())
            .unwrap_or_default();
        self.time.covered = next;
        let _ = self.relayout();
        (self.emit)(Projection::UiCommand(format!("focus.cover={site}")));
    }

    /// Hands the engine the gate for this moment and holds the media of every
    /// tab already on a shut site; a break or the end lets all of it go.
    fn sync_focus_gate(&mut self) {
        let gate = self.focus_gate_now();
        let now = utc_now_ms();
        let stilled: std::collections::HashSet<ItemId> = gate
            .as_ref()
            .map(|gate| {
                self.items
                    .view_ids()
                    .into_iter()
                    .filter(|id| {
                        self.items
                            .tab(*id)
                            .and_then(|tab| tab.url.as_ref())
                            .and_then(|url| url.host_str())
                            .is_some_and(|host| gate.blocks(host, now))
                    })
                    .collect()
            })
            .unwrap_or_default();
        for id in self.time.stilled.difference(&stilled) {
            self.engine.set_media_suspended(*id, false);
        }
        for id in stilled.difference(&self.time.stilled) {
            self.engine.set_media_suspended(*id, true);
        }
        self.time.stilled = stilled;
        let open = gate.is_none();
        if gate != self.time.gate {
            self.time.gate.clone_from(&gate);
            self.engine.set_focus_gate(gate);
        }
        if open {
            self.open_shut_loads(None);
        }
    }

    /// Opens fresh tabs whose first load focus shut: those on one site once it
    /// is let through, or all of them once the round is over.
    fn open_shut_loads(&mut self, site: Option<&str>) {
        let ready: Vec<(ItemId, String)> = self
            .time
            .shut_loads
            .iter()
            .filter(|(_, url)| {
                site.is_none_or(|site| {
                    url::Url::parse(url)
                        .ok()
                        .and_then(|url| url.host_str().map(|host| site_covers(site, host)))
                        .unwrap_or(false)
                })
            })
            .map(|(id, url)| (*id, url.clone()))
            .collect();
        for (id, url) in ready {
            self.time.shut_loads.remove(&id);
            if self.items.tab(id).is_some_and(|tab| tab.url.is_none()) {
                let _ = self.operation_navigate(id, url);
            }
        }
    }

    pub(super) fn on_focus_blocked(&mut self, id: ItemId, url: String) {
        let Some(tab) = self.items.tab(id) else {
            return;
        };
        if tab.url.is_none() {
            self.time
                .shut_loads
                .retain(|kept, _| self.items.tab(*kept).is_some());
            if self.time.shut_loads.len() < MAX_SHUT_LOADS || self.time.shut_loads.contains_key(&id)
            {
                self.time.shut_loads.insert(id, url);
            }
            return;
        }
        // A page already open tried to leave for a shut site; it stays where
        // it is, and chrome says why nothing happened.
        if let Some(host) = url::Url::parse(&url).ok().and_then(|url| {
            url.host_str()
                .map(|host| host.strip_prefix("www.").unwrap_or(host).to_owned())
        }) {
            (self.emit)(Projection::UiCommand(format!("focus.shut={host}")));
        }
    }

    fn schedule_focus_wake(&self) {
        let Some(queue) = &self.self_queue else {
            return;
        };
        let deadline = self
            .time
            .focus
            .as_ref()
            .filter(|_| self.time.pending_focus_records.len() < MAX_PENDING_FOCUS_RECORDS)
            .map(|session| {
                let wait = (session.next_change_ms() - utc_now_ms()).max(0);
                std::time::Instant::now()
                    + std::time::Duration::from_millis(u64::try_from(wait).unwrap_or(0))
            });
        queue.schedule_focus(deadline.into_iter().chain(self.time.focus_retry_at).min());
    }

    /// A shut site's icon, as held for the site or its `www.` host. Anything
    /// missing is fetched, and arrives with the next projection.
    fn shut_site_views(&mut self) -> Vec<ShutSiteView> {
        let Some(profile) = self.windows.focused().map(|window| window.profile) else {
            return Vec::new();
        };
        let pages = |site: &str| [format!("https://{site}/"), format!("https://www.{site}/")];
        let views: Vec<ShutSiteView> = self
            .time
            .blocked
            .iter()
            .map(|site| ShutSiteView {
                icon: pages(site)
                    .iter()
                    .find_map(|page| self.icon_ref_for_url(IconSurface::Chrome, profile, page)),
                site: site.clone(),
            })
            .collect();
        let missing: Vec<String> = views
            .iter()
            .filter(|view| view.icon.is_none())
            .flat_map(|view| pages(&view.site))
            .collect();
        self.want_icons(
            IconSurface::Chrome,
            profile,
            missing.iter().map(String::as_str),
        );
        self.publish_icons();
        views
    }

    pub(super) fn project_focus(&mut self) {
        let shut = self.shut_site_views();
        let session = self.time.focus.as_ref().map(|session| {
            let (short, long) = session.plan.break_minutes();
            FocusView {
                phase: phase_view(session.phase),
                started_at: session.started_ms,
                phase_started_at: session.phase_started_ms,
                phase_ends_at: session.phase_ends_ms,
                minutes: u32::from(session.plan.minutes),
                breaks: session.plan.breaks,
                break_minutes: u32::from(short),
                long_break_minutes: u32::from(long),
                rounds: session.rounds,
                focused: seconds(session.focused_ms),
                allowed: session
                    .allowances
                    .iter()
                    .map(|(site, _)| site.clone())
                    .collect(),
            }
        });
        (self.emit)(Projection::Focus(FocusStatus { session, shut }));
    }

    /// Picks a restored session back up once the actor can schedule it.
    pub(super) fn resume_focus(&mut self) {
        if self.time.focus.is_some() {
            self.focus_wake();
            self.schedule_focus_wake();
        }
        self.sync_focus_gate();
        self.project_focus();
        let site = self
            .time
            .covered
            .as_ref()
            .map(|(_, site)| site.clone())
            .unwrap_or_default();
        (self.emit)(Projection::UiCommand(format!("focus.cover={site}")));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sites_group_by_registrable_domain() {
        let site = |url: &str| site_of(&url::Url::parse(url).unwrap());
        assert_eq!(
            site("https://m.youtube.com/watch").as_deref(),
            Some("youtube.com")
        );
        assert_eq!(
            site("https://www.bbc.co.uk/news").as_deref(),
            Some("bbc.co.uk")
        );
        assert_eq!(
            site("https://docs.google.com/").as_deref(),
            Some("google.com")
        );
        assert_eq!(site("http://localhost:5173/").as_deref(), Some("localhost"));
        assert_eq!(site("about:blank"), None);
    }

    #[test]
    fn the_block_list_is_one_site_per_line() {
        assert_eq!(
            blocked_sites("x.com\n\nyoutube.com\n"),
            vec!["x.com".to_owned(), "youtube.com".to_owned()]
        );
    }
}
