 Serialize, Deserialize, Type)]
pub struct PersonalHideView {
    pub id: String,
    pub label: String,
    pub enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct BlockerSiteView {
    pub context: BlockerSiteContext,
    pub paused: bool,
    pub private_session: bool,
    pub ready: bool,
    pub busy: bool,
    pub hides: Vec<PersonalHideView>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BlockerSiteAction {
    SaveSelection { session: String, selection: String },
    Retry,
    Pause { paused: bool },
    SetHideEnabled { id: String, enabled: bool },
    RemoveHide { id: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct BlockerStatusView {
    pub site: Option<Box<BlockerSiteView>>,
    pub projection_revision: String,
    pub protection: BlockerProtection,
    pub phase: BlockerPhase,
    pub preference: BlockerPreferenceState,
    pub config_revision: Option<String>,
    pub desired_enabled: Option<bool>,
    pub applied_enabled: Option<bool>,
    pub desired_generation: Option<String>,
    pub retained_generation: Option<String>,
    pub failure: Option<BlockerFailure>,
    pub retryable: bool,
    pub retries_remaining: u8,
    /// Boxed with the other diagnostic-only payloads so ordinary projection
    /// queue entries do not carry the full coverage report inline.
    pub applied_coverage: Option<Box<BlockerRuleCoverage>>,
    /// Boxed because the six decimal counters are diagnostic-only and should
    /// not inflate every projection enum value on the actor/UI hot path.
    pub runtime_diagnostics: Option<Box<BlockerRuntimeDiagnostics>>,
    pub source_phase: BlockerSourcePhase,
    pub source_failure: Option<BlockerSourceFailure>,
    pub source_package_revision: Option<String>,
    pub source_installed_revision: Option<String>,
    pub source_package_provenance: Option<BlockerSourceProvenance>,
    pub source_installed_provenance: Option<BlockerSourceProvenance>,
    pub source_identities: Option<Box<BlockerSourceIdentities>>,
    pub source_package_created_unix: Option<String>,
    pub source_package_expires_unix: Option<String>,
    pub source_package_stale: Option<bool>,
    /// Advisory source update cadence. This never downgrades a healthy
    /// release-bundled policy.
    pub source_refresh_due: bool,
    pub source_count: Option<u32>,
    pub source_bytes: Option<u32>,
    pub source_activation_pending: bool,
    pub source_material_repair_pending: bool,
    pub source_material_repair_retry_pending: bool,
    pub source_repair_retry_pending: bool,
    pub source_last_refresh_attempt_unix: Option<String>,
    pub source_refresh_operation: Option<String>,
    /// Authoritative source-policy capability for the focused profile.
    pub can_enable: bool,
    /// Authoritative refresh admission capability for the active supply mode.
    pub can_refresh_sources: bool,
}

impl BlockerStatusView {
    /// Static reconciliation result used only when no actor-owned revision can
    /// be obtained. Revision zero cannot overwrite a real actor projection.
    pub fn unavailable() -> Self {
        Self {
            site: None,
            projection_revision: "00000000000000000000000000000000".into(),
            protection: BlockerProtection::Unavailable,
            phase: BlockerPhase::Unavailable,
            preference: BlockerPreferenceState::Unavailable,
            config_revision: None,
            desired_enabled: None,
            applied_enabled: None,
            desired_generation: None,
            retained_generation: None,
            failure: None,
            retryable: false,
            retries_remaining: 0,
            applied_coverage: None,
            runtime_diagnostics: None,
            source_phase: BlockerSourcePhase::NotConfigured,
            source_failure: None,
            source_package_revision: None,
            source_installed_revision: None,
            source_package_provenance: None,
            source_installed_provenance: None,
            source_identities: None,
            source_package_created_unix: None,
            source_package_expires_unix: None,
            source_package_stale: None,
            source_refresh_due: false,
            source_count: None,
            source_bytes: None,
            source_activation_pending: false,
            source_material_repair_pending: false,
            source_material_repair_retry_pending: false,
            source_repair_retry_pending: false,
            source_last_refresh_attempt_unix: None,
            source_refresh_operation: None,
            can_enable: false,
            can_refresh_sources: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct PanelOwner {
    pub private: bool,
    pub window_id: String,
    pub profile_id: String,
    pub profile_name: String,
    pub space_id: String,
}

/// Snapshots for structural changes, single-row deltas for per-tab churn.
#[derive(Clone, Debug)]
pub enum Projection {
    WorkEnvironmentChanged(work::WorkEnvironmentChangedV1),
    WorkChanged(work::WorkChangedV1),
    PanelOwner(PanelOwner),
    TabSwitcher {
        session: u64,
        state: Option<TabSwitcherState>,
    },
    Items(ItemsState),
    Tab(TabView),
    Favicons(FaviconsView),
    ExtensionActions(ExtensionActionsView),
    ExtensionActionFailed(ExtensionActionFailedView),
    ExtensionActionShortcut(ExtensionActionShortcutView),
    PagePermissionPrompt(PagePermissionPromptView),
    WebExtensionAccessRequest(WebExtensionAccessRequestView),
    WebExtensionAccessRemoved(WebExtensionAccessRemovalView),
    WebExtensionStoreClick(WebExtensionStoreClickView),
    UiCommand(String),
    FindResult(FindResultView),
    Search(SearchResults),
    OpenNote {
        profile: String,
        id: String,
    },
    Layout(LayoutState),
    /// The page on screen is fullscreen and fills the browser window, which
    /// should itself be fullscreen until this turns false. Only engines that
    /// present fullscreen inside the browser window send it.
    HostFullscreen(bool),
    /// The browser window should come to the front, as the person asked for
    /// one of its pages from outside it.
    RaiseWindow,
    RuntimeStatus(RuntimeStatus),
    BlockerStatus(BlockerStatusView),
    Focus(FocusStatus),
    OperationProcessed(OperationDisposition),
    Profiles(ProfilesView),
    /// The newest tabs the focused profile can reopen by name, for the menus.
    /// Sent only when that list changes; icons are not part of it.
    RecentlyClosed(Vec<ClosedTabView>),
    /// The sidebar's player, sent when it changes; `None` hides it.
    MediaPlayer(Option<MediaPlayerView>),
    MiniOpened(MiniOpenedView),
    Mini(MiniView),
    MiniClosed(MiniClosedView),
    /// Links another application handed over opened as tabs, so the main
    /// window comes forward. A link that opened a mini window does not.
    ExternalTabsOpened,
}

/// Shared Rust-owned Notes/Tasks wire model.
pub use zephium_core::resources::{ResourceCall, ResourceReply, ResourceResponse};

#[cfg(test)]
mod blocker_status_tests {
    use super::*;

    #[test]
    fn unavailable_status_is_bounded_and_cannot_supersede_actor_state() {
        let status = BlockerStatusView::unavailable();
        assert_eq!(
            status.projection_revision,
            "00000000000000000000000000000000"
        );
        assert_eq!(status.protection, BlockerProtection::Unavailable);
        assert_eq!(status.phase, BlockerPhase::Unavailable);
        assert_eq!(status.preference, BlockerPreferenceState::Unavailable);
        assert!(status.config_revision.is_none());
        assert!(status.desired_generation.is_none());
        assert!(status.retained_generation.is_none());
        assert!(status.failure.is_none());
        assert!(status.applied_coverage.is_none());
        assert_eq!(status.source_phase, BlockerSourcePhase::NotConfigured);
        assert!(status.source_failure.is_none());
        assert!(status.source_package_revision.is_none());
        assert!(status.source_installed_revision.is_none());
        assert!(status.source_identities.is_none());
        assert!(!status.source_activation_pending);
        assert!(!status.source_material_repair_pending);
        assert!(!status.source_material_repair_retry_pending);
        assert!(!status.source_repair_retry_pending);
        assert!(!status.retryable);
        assert_eq!(status.retries_remaining, 0);
    }
}

#[cfg(test)]
mod browsing_data_tests {
    use super::*;

    #[test]
    fn ranges_cover_the_windows_a_person_picks_and_all_time_has_none() {
        let hour = 3600;
        assert_eq!(BrowsingDataRange::Hour.window_seconds(), Some(hour));
        assert_eq!(BrowsingDataRange::Day.window_seconds(), Some(24 * hour));
        assert_eq!(
            BrowsingDataRange::Week.window_seconds(),
            Some(7 * 24 * hour)
        );
        assert_eq!(
            BrowsingDataRange::FourWeeks.window_seconds(),
            Some(28 * 24 * hour)
        );
        assert_eq!(BrowsingDataRange::Everything.window_seconds(), None);
    }

    #[test]
    fn a_clear_needs_a_kind_and_a_forget_needs_one_bare_site() {
        let clear = |history, cookies, cache, downloads| BrowsingDataCall::Clear {
            range: BrowsingDataRange::Hour,
            history,
            cookies,
            cache,
            downloads,
        };
        assert!(!clear(false, false, false, false).validate());
        for call in [
            clear(true, false, false, false),
            clear(false, true, false, false),
            clear(false, false, true, false),
            clear(false, false, false, true),
            BrowsingDataCall::Sites,
            BrowsingDataCall::ForgetSite {
                site: "example.com".into(),
            },
            BrowsingDataCall::ForgetPage {
                origin: "https://www.example.com:8443".into(),
            },
        ] {
            assert!(call.validate(), "{call:?}");
        }
        for site in [
            "",
            "https://example.com",
            "example.com/a",
            "*.example.com",
            " ",
        ] {
            assert!(
                !BrowsingDataCall::ForgetSite { site: site.into() }.validate(),
                "{site:?}"
            );
        }
        for origin in [
            "",
            "example.com",
            "https://example.com/a",
            "file:///tmp",
            "about:blank",
        ] {
            assert!(
                !BrowsingDataCall::ForgetPage {
                    origin: origin.into()
                }
                .validate(),
                "{origin:?}"
            );
        }
    }

    #[test]
    fn a_frame_cannot_send_a_dated_history_or_download_clear() {
        assert!(
            serde_json::from_str::<HistoryCall>(r#"{"kind":"clear_since","since":1}"#).is_err()
        );
        assert!(serde_json::from_str::<HistoryCall>(r#"{"kind":"clear","range":"hour"}"#).is_ok());
        assert!(serde_json::from_str::<BrowsingDataCall>(
            r#"{"kind":"clear","range":"four_weeks","history":true,"cookies":false,"cache":false,"downloads":false}"#
        )
        .is_ok());
        assert!(serde_json::from_str::<BrowsingDataCall>(
            r#"{"kind":"clear","range":"forever","history":true,"cookies":false,"cache":false,"downloads":false}"#
        )
        .is_err());
        assert!(
            serde_json::from_str::<zephium_core::downloads::DownloadCall>(
                r#"{"kind":"clear_since","since":1}"#
            )
            .is_err()
        );
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct BlockerStatsView {
    #[specta(type = f64)]
    pub today: u64,
    #[serde(rename = "last7Days")]
    #[specta(type = f64)]
    pub last_seven_days: u64,
    #[specta(type = Vec<f64>)]
    pub days: [u64; 7],
}

#[cfg(test)]
mod bookmark_tests {
    use super::*;

    #[test]
    fn bookmark_calls_admit_only_canonical_ids_and_bounded_text() {
        assert_eq!(bookmark_id("42"), Some(42));
        for id in ["0", "-1", "042", "4.2", "", "x"] {
            assert_eq!(bookmark_id(id), None, "{id}");
        }
        let valid = [
            BookmarkCall::List { folder: None },
            BookmarkCall::Reveal { id: "3".into() },
            BookmarkCall::Search {
                query: "docs".into(),
            },
            BookmarkCall::AddFolder {
                parent: Some("3".into()),
                title: "Reading".into(),
            },
            BookmarkCall::Move {
                id: "4".into(),
                parent: None,
                index: 9,
            },
            BookmarkCall::AddLink {
                parent: None,
                title: String::new(),
                url: "example.com".into(),
            },
        ];
        assert!(valid.iter().all(BookmarkCall::validate));
        let invalid = [
            BookmarkCall::List {
                folder: Some("top".into()),
            },
            BookmarkCall::Search { query: "  ".into() },
            BookmarkCall::Rename {
                id: "3".into(),
                title: "a".repeat(MAX_BOOKMARK_TITLE_INPUT_BYTES + 1),
            },
            BookmarkCall::AddFolder {
                parent: None,
                title: " ".into(),
            },
            BookmarkCall::Remove { id: "0".into() },
            BookmarkCall::AddLink {
                parent: None,
                title: "Docs".into(),
                url: " ".into(),
            },
        ];
        assert!(!invalid.iter().any(BookmarkCall::validate));
    }
}

#[cfg(test)]
mod history_bounds_tests {
    use super::*;

    #[test]
    fn history_forget_retains_its_per_address_and_row_budgets() {
        let prefix = "https://example.test/";
        let at_bound = format!(
            "{prefix}{}",
            "x".repeat(MAX_HISTORY_URL_BYTES - prefix.len())
        );
        let forget = |url| HistoryCall::Forget { urls: vec![url] };
        assert!(forget(at_bound.clone()).validate());
        assert!(HistoryCall::Forget {
            urls: vec![at_bound.clone(); MAX_HISTORY_FORGET_URLS]
        }
        .validate());
        assert!(!HistoryCall::Forget {
            urls: vec![at_bound.clone(); MAX_HISTORY_FORGET_URLS + 1]
        }
        .validate());
        assert!(!forget(format!("{at_bound}x")).validate());
        assert!(!forget(format!("{at_bound}\n")).validate());
        let expanded = format!(
            "{prefix}{}",
            "é".repeat((MAX_HISTORY_URL_BYTES - prefix.len()) / 2)
        );
        assert!(expanded.len() <= MAX_HISTORY_URL_BYTES);
        assert!(zephium_core::navigation::is_allowed_str(&expanded));
        assert!(!forget(expanded).validate());
    }
}
