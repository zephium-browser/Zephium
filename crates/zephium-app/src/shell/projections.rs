rue,
            });
        }
        view.certificate_exception = self.under_certificate_exception(id, tab);
        if let Some(zephium_ipc::PageRequestView::Popup { site, .. }) = view.page_request.as_mut() {
            *site = self.popup_site(id).and_then(|(_, origin)| {
                Some(
                    url::Url::parse(origin.as_str())
                        .ok()?
                        .host_str()?
                        .to_owned(),
                )
            });
        }
        view.availability = self.capacity_presentation(id).or_else(|| {
            (!tab.has_view()
                && tab.url.is_some()
                && tab.content == zephium_core::item::TabContent::Web)
                .then_some(zephium_ipc::TabAvailability::Sleeping)
        });
        view
    }

    pub(super) fn next_projection_revision(&self) -> u128 {
        // Saturation is fail-closed: subsequent equal revisions are ignored
        // by privileged chrome, so no older projection can become current.
        let next = self
            .presentation
            .projection_sequence
            .get()
            .saturating_add(1);
        self.presentation.projection_sequence.set(next);
        next
    }

    pub(super) fn icon_ref(
        &self,
        surface: zephium_ipc::IconSurface,
        tab: &TabState,
        profile: Option<ProfileId>,
    ) -> Option<zephium_ipc::IconRef> {
        let url = tab.url.as_ref()?;
        let origin = origin_of(url)?;
        let profile = profile?;
        self.icon_ref_for_page(surface, profile, url).or_else(|| {
            // The other spelling of the host stands in until this one has its
            // own; no other subdomain does.
            self.icon_ref_for(surface, profile, &super::favicons::apex_pair(&origin)?)
        })
    }

    /// The icon a live tab wears. A tab loading another page of the site it
    /// was just on keeps that icon until the new page's is known, and a tab
    /// that moved to another site shows no stale mark at all, only that it
    /// is loading.
    pub(super) fn tab_icon(
        &self,
        id: ItemId,
        tab: &TabState,
        profile: Option<ProfileId>,
    ) -> Option<zephium_ipc::IconRef> {
        // A failure filling the pane is not the page whose icon the tab had;
        // that icon returns with the page when the person leaves.
        if tab.failure_covers_page() {
            return None;
        }
        let icon = self.icon_ref(zephium_ipc::IconSurface::Chrome, tab, profile);
        self.keep_or_hold_icon(id, tab, profile?, icon)
    }

    /// The icon a page wears: its own when it has one, else its origin's.
    fn icon_ref_for_page(
        &self,
        surface: zephium_ipc::IconSurface,
        profile: ProfileId,
        url: &url::Url,
    ) -> Option<zephium_ipc::IconRef> {
        zephium_core::icon::page_key(url)
            .and_then(|page| self.icon_ref_for(surface, profile, &page))
            .or_else(|| self.icon_ref_for(surface, profile, &origin_of(url)?))
    }

    pub(super) fn icon_ref_for_url(
        &self,
        surface: zephium_ipc::IconSurface,
        profile: ProfileId,
        url: &str,
    ) -> Option<zephium_ipc::IconRef> {
        let parsed = url::Url::parse(url).ok()?;
        self.icon_ref_for_page(surface, profile, &parsed)
    }
}

/// The site as cards name it: host and any non-default port, no scheme.
fn origin_label(origin: &str) -> String {
    origin
        .split_once("://")
        .map_or(origin, |(_, host)| host)
        .to_owned()
}

fn page_permission_kind_view(
    kind: zephium_core::permissions::PagePermissionKind,
) -> Option<PagePermissionKindView> {
    match kind {
        zephium_core::permissions::PagePermissionKind::Camera => {
            Some(PagePermissionKindView::Camera)
        }
        zephium_core::permissions::PagePermissionKind::Microphone => {
            Some(PagePermissionKindView::Microphone)
        }
        zephium_core::permissions::PagePermissionKind::Geolocation => {
            Some(PagePermissionKindView::Geolocation)
        }
        // The native broker never admits these capabilities. Keep the
        // projection total while preserving a closed UI vocabulary.
        zephium_core::permissions::PagePermissionKind::Notifications
        | zephium_core::permissions::PagePermissionKind::ClipboardRead => None,
    }
}

fn extension_action_failure_view(
    reason: zephium_core::extensions::ExtensionActionRejection,
) -> ExtensionActionFailure {
    use zephium_core::extensions::ExtensionActionRejection as Core;
    match reason {
        Core::InvalidRequest => ExtensionActionFailure::InvalidRequest,
        Core::RuntimeUnavailable => ExtensionActionFailure::RuntimeUnavailable,
        Core::RuntimeSuperseded => ExtensionActionFailure::RuntimeSuperseded,
        Core::TabUnavailable => ExtensionActionFailure::TabUnavailable,
        Core::TabDiscarded => ExtensionActionFailure::TabDiscarded,
        Core::ActionUnavailable => ExtensionActionFailure::ActionUnavailable,
        Core::ActionDisabled => ExtensionActionFailure::ActionDisabled,
        Core::CapacityExceeded => ExtensionActionFailure::CapacityExceeded,
        Core::PopupUnavailable => ExtensionActionFailure::PopupUnavailable,
        Core::PopupCapacityExceeded => ExtensionActionFailure::PopupCapacityExceeded,
        Core::NativeAdmissionFailed => ExtensionActionFailure::NativeAdmissionFailed,
        Core::ShuttingDown => ExtensionActionFailure::ShuttingDown,
        Core::UnsupportedPlatform => ExtensionActionFailure::UnsupportedPlatform,
    }
}

fn runtime_security_advisory_view(
    advisory: zephium_core::runtime_security::RuntimeSecurityAdvisory,
) -> RuntimeSecurityAdvisory {
    RuntimeSecurityAdvisory {
        kind: match advisory.kind() {
            zephium_core::runtime_security::RuntimeSecurityAdvisoryKind::ReviewOverdue => {
                RuntimeSecurityAdvisoryKind::ReviewOverdue
            }
            zephium_core::runtime_security::RuntimeSecurityAdvisoryKind::UpdateRecommended => {
                RuntimeSecurityAdvisoryKind::UpdateRecommended
            }
            zephium_core::runtime_security::RuntimeSecurityAdvisoryKind::UnreviewedRuntime => {
                RuntimeSecurityAdvisoryKind::UnreviewedRuntime
            }
            zephium_core::runtime_security::RuntimeSecurityAdvisoryKind::CleanupOverdue => {
                RuntimeSecurityAdvisoryKind::CleanupOverdue
            }
        },
        update_target: match advisory.update_target() {
            zephium_core::runtime_security::RuntimeSecurityUpdateTarget::Zephium => {
                RuntimeSecurityUpdateTarget::Zephium
            }
            zephium_core::runtime_security::RuntimeSecurityUpdateTarget::OperatingSystem => {
                RuntimeSecurityUpdateTarget::OperatingSystem
            }
            zephium_core::runtime_security::RuntimeSecurityUpdateTarget::BrowserRuntime => {
                RuntimeSecurityUpdateTarget::BrowserRuntime
            }
        },
    }
}

/// A failure that fills the tab's pane (a refused page, or one that never
/// opened) titles the tab with its host; the page behind it keeps its own
/// title for when the person leaves.
fn failed_pane_title(tab: &TabState) -> Option<String> {
    let failure = tab.failure.as_deref()?;
    (failure.covers_page || (!tab.loading && tab.url.is_none()))
        .then(|| failure.url.host_str().map(str::to_owned))
        .flatten()
}

fn space_icon_view(icon: &zephium_core::spaces::SpaceIcon) -> zephium_ipc::SpaceIconView {
    match icon {
        zephium_core::spaces::SpaceIcon::Symbol(index) => {
            zephium_ipc::SpaceIconView::Symbol { index: *index }
        }
        zephium_core::spaces::SpaceIcon::Emoji(text) => {
            zephium_ipc::SpaceIconView::Emoji { text: text.clone() }
        }
    }
}

fn tab_view(
    id: ItemId,
    tab: &TabState,
    icon: Option<zephium_ipc::IconRef>,
    revision: u128,
) -> TabView {
    TabView {
        id: id.to_string(),
        projection_revision: format!("{revision:032x}"),
        title: failed_pane_title(tab).unwrap_or_else(|| tab.title.clone()),
        url: tab.url.as_ref().map(ToString::to_string),
        content: match tab.content {
            zephium_core::item::TabContent::Web => zephium_ipc::TabContentView::Web,
            zephium_core::item::TabContent::BrowserOwned(
                zephium_core::item::BrowserOwnedTab::Settings,
            ) => zephium_ipc::TabContentView::Settings,
            zephium_core::item::TabContent::BrowserOwned(
                zephium_core::item::BrowserOwnedTab::Extensions,
            ) => zephium_ipc::TabContentView::Extensions,
            zephium_core::item::TabContent::BrowserOwned(
                zephium_core::item::BrowserOwnedTab::Source,
            ) => zephium_ipc::TabContentView::Source,
            zephium_core::item::TabContent::ExtensionOwned => {
                zephium_ipc::TabContentView::ExtensionOwned
            }
        },
        loading: tab.loading,
        page_request: tab.page_request.as_deref().map(|request| match request {
            zephium_core::item::PageRequest::ExternalApp { url, app, site } => {
                zephium_ipc::PageRequestView::ExternalApp {
                    site: site.as_ref().map(|site| origin_label(site.as_str())),
                    scheme: url.scheme().to_owned(),
                    app: app.clone(),
                }
            }
            zephium_core::item::PageRequest::Popup { url } => zephium_ipc::PageRequestView::Popup {
                host: url
                    .as_ref()
                    .and_then(|url| url.host_str())
                    .map(str::to_owned),
                site: None,
            },
            zephium_core::item::PageRequest::HttpAuth(prompt) => {
                zephium_ipc::PageRequestView::HttpAuth {
                    request: prompt.request,
                    host: prompt.host.clone(),
                    insecure: prompt.insecure,
                    proxy: prompt.proxy,
                    retry: prompt.retry,
                }
            }
            zephium_core::item::PageRequest::Download(prompt) => {
                zephium_ipc::PageRequestView::Download {
                    request: prompt.request,
                    site: origin_label(prompt.site.as_str()),
                    filename: prompt.filename.clone(),
                    allowed: prompt.allowed,
                    unclicked: prompt.unclicked,
                }
            }
            zephium_core::item::PageRequest::InsecureAutofill => {
                zephium_ipc::PageRequestView::InsecureAutofill {
                    host: tab
                        .url
                        .as_ref()
                        .and_then(|url| url.host_str())
                        .unwrap_or_default()
                        .to_owned(),
                }
            }
            zephium_core::item::PageRequest::NavigationRefused { embedded } => {
                zephium_ipc::PageRequestView::NavigationRefused {
                    embedded: *embedded,
                }
            }
            zephium_core::item::PageRequest::Unresponsive => {
                zephium_ipc::PageRequestView::Unresponsive
            }
        }),
        page_request_serial: tab.page_request.as_ref().map(|_| tab.page_request_serial),
        availability: None,
        failure: tab.failure.as_ref().map(|failure| {
            use zephium_core::certificates::CertificateProblem as Problem;
            use zephium_core::ports::engine::NavigationFailureReason as Reason;
            use zephium_ipc::TabCertificateProblem as View;
            let certificate = match failure.reason {
                Reason::Certificate(problem) => Some(match problem {
                    Problem::Expired => View::Expired,
                    Problem::Untrusted => View::Untrusted,
                    Problem::NameMismatch => View::NameMismatch,
                    Problem::Revoked => View::Revoked,
                    Problem::Other => View::Other,
                }),
                _ => None,
            };
            zephium_ipc::TabFailure {
                url: failure.url.to_string(),
                reason: match failure.reason {
                    Reason::Offline => zephium_ipc::TabFailureReason::Offline,
                    Reason::HostNotFound => zephium_ipc::TabFailureReason::HostNotFound,
                    Reason::Unreachable => zephium_ipc::TabFailureReason::Unreachable,
                    Reason::TimedOut => zephium_ipc::TabFailureReason::TimedOut,
                    Reason::Insecure | Reason::Certificate(_) => {
                        zephium_ipc::TabFailureReason::Insecure
                    }
                    Reason::Blocked(_) => zephium_ipc::TabFailureReason::Blocked,
                    Reason::Other => zephium_ipc::TabFailureReason::Other,
                },
                can_continue: (certificate.is_some()
                    && zephium_core::certificates::may_continue(&failure.url))
                    || failure.can_continue,
                certificate,
                blocked: match failure.reason {
                    Reason::Blocked(
                        zephium_core::ports::engine::NavigationBlockReason::Badware,
                    ) => Some(zephium_ipc::TabBlockReason::Badware),
                    Reason::Blocked(
                        zephium_core::ports::engine::NavigationBlockReason::FilterLists,
                    ) => Some(zephium_ipc::TabBlockReason::FilterLists),
                    _ => None,
                },
                covers_page: failure.covers_page,
            }
        }),
        certificate_exception: false,
        can_go_back: tab.can_go_back,
        can_go_forward: tab.can_go_forward,
        icon,
        custom_icon: tab.custom_icon.as_ref().map(space_icon_view),
        capture: tab.capture.filter(|(_, state)| state.is_capturing()).map(
            |(navigation, state)| {
                let device = |value| match value {
                    zephium_core::ports::engine::CaptureDeviceState::None => {
                        zephium_ipc::CaptureDeviceStateView::None
                    }
                    zephium_core::ports::engine::CaptureDeviceState::Active => {
                        zephium_ipc::CaptureDeviceStateView::Active
                    }
                    zephium_core::ports::engine::CaptureDeviceState::Muted => {
                        zephium_ipc::CaptureDeviceStateView::Muted
                    }
                };
                zephium_ipc::MediaCaptureView {
                    navigation_id: format!("{:016x}", navigation.into_raw()),
                    camera: device(state.camera),
                    microphone: device(state.microphone),
                }
            },
        ),
        zoom: None,
        audio: None,
    }
}
