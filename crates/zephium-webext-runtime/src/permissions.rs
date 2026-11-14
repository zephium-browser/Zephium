//! What one install may do: API permissions, host permissions, optional
//! grants, `activeTab`, site access and file access, and the [`Gate`] that
//! checks a call against them.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use url::Url;
use zephium_core::ids::{ItemId, ProfileId};
use zephium_webext::manifest::Manifest;

use crate::context::{DocumentGeneration, FrameId, Sender};
use crate::errors::ApiError;
use crate::pattern::{is_extension_gallery, DocumentOrigin, HostPattern, TupleOrigin};

/// Longest API permission name.
pub const MAX_PERMISSION_NAME_BYTES: usize = 64;

const ACTIVE_TAB: &str = "activeTab";
const NATIVE_MESSAGING: &str = "nativeMessaging";

/// A syntactically valid API permission name such as `tabs` or
/// `identity.email`.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ApiPermission(Box<str>);

impl ApiPermission {
    /// Validates a name.
    pub fn parse(name: &str) -> Option<Self> {
        let valid = name.len() <= MAX_PERMISSION_NAME_BYTES
            && name.split('.').all(|segment| {
                let mut bytes = segment.bytes();
                bytes
                    .next()
                    .is_some_and(|first| first.is_ascii_alphabetic())
                    && bytes.all(|byte| byte.is_ascii_alphanumeric())
            });
        valid.then(|| Self(name.into()))
    }

    /// The name.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ApiPermission {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// An entry the manifest declared that cannot take effect.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeclarationIssue {
    /// An API permission name that is not well formed.
    InvalidPermission(String),
    /// A host pattern that does not parse.
    InvalidHost(String),
}

/// What a manifest declares, required and optional.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PermissionDeclaration {
    required_api: BTreeSet<ApiPermission>,
    optional_api: BTreeSet<ApiPermission>,
    required_hosts: Vec<HostPattern>,
    optional_hosts: Vec<HostPattern>,
}

impl PermissionDeclaration {
    /// Reads the declaration. Entries Chrome would ignore with a warning
    /// are returned as issues rather than failing the install.
    pub fn from_manifest(manifest: &Manifest) -> (Self, Vec<DeclarationIssue>) {
        let mut issues = Vec::new();
        let mut api = |names: Vec<String>| {
            names
                .into_iter()
                .filter_map(|name| {
                    ApiPermission::parse(&name).or_else(|| {
                        issues.push(DeclarationIssue::InvalidPermission(name));
                        None
                    })
                })
                .collect::<BTreeSet<_>>()
        };
        let required_api = api(manifest.permissions());
        let optional_api = api(manifest.optional_permissions());
        let mut hosts = |patterns: Vec<String>| {
            let mut parsed: Vec<HostPattern> = Vec::new();
            for source in patterns {
                match HostPattern::parse(&source) {
                    Ok(pattern) if !parsed.contains(&pattern) => parsed.push(pattern),
                    Ok(_) => {}
                    Err(_) => issues.push(DeclarationIssue::InvalidHost(source)),
                }
            }
            parsed
        };
        let declaration = Self {
            required_api,
            optional_api,
            required_hosts: hosts(manifest.host_permissions()),
            optional_hosts: hosts(manifest.optional_host_permissions()),
        };
        (declaration, issues)
    }
}

/// A `permissions` API argument: `{permissions, origins}`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PermissionRequest {
    /// API permissions.
    pub permissions: Vec<ApiPermission>,
    /// Origin patterns.
    pub origins: Vec<HostPattern>,
}

impl PermissionRequest {
    /// Parses the strings an extension passed, with Chrome's errors.
    pub fn parse<S: AsRef<str>>(permissions: &[S], origins: &[S]) -> Result<Self, ApiError> {
        let permissions = permissions
            .iter()
            .map(|name| {
                ApiPermission::parse(name.as_ref())
                    .ok_or_else(|| ApiError::unknown_permission(name.as_ref()))
            })
            .collect::<Result<_, _>>()?;
        let origins = origins
            .iter()
            .map(|source| {
                HostPattern::parse(source.as_ref())
                    .map_err(|error| ApiError::invalid_origin(source.as_ref(), &error.to_string()))
            })
            .collect::<Result<_, _>>()?;
        Ok(Self {
            permissions,
            origins,
        })
    }

    /// The install-style warnings to show when asking the person.
    pub fn warnings(&self) -> Vec<String> {
        let permissions: Vec<String> = self
            .permissions
            .iter()
            .map(|permission| permission.as_str().to_owned())
            .collect();
        let origins: Vec<String> = self
            .origins
            .iter()
            .map(|origin| origin.as_str().to_owned())
            .collect();
        zephium_webext::permissions::access_warnings(&permissions, &origins)
    }
}

/// The person's host-access choice for an install, as in Chrome's site
/// access menu.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum SiteAccess {
    /// Every host the install was granted.
    #[default]
    OnAllSites,
    /// Only granted hosts within these sites.
    OnSpecificSites(Vec<HostPattern>),
    /// Only a tab the person invoked the extension on.
    OnClick,
}

/// `permissions.getAll`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PermissionsSnapshot {
    /// Granted API permissions, sorted.
    pub permissions: Vec<String>,
    /// Effective origin patterns.
    pub origins: Vec<String>,
}

/// What the adapter must tear down after access shrank.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Teardown {
    /// Host access shrank. Engine CORS allowlists cannot be narrowed in
    /// place, so background, offscreen and popup restart and extension tabs
    /// reload.
    RestartContexts,
    /// Rebuild content-script sets; documents loaded later see the change.
    RefreshContentScripts,
    /// Republish network rules whose effect depends on host access.
    RepublishNetworkRules,
    /// Close the install's native-messaging hosts.
    DisconnectNativeHosts,
    /// Fire `permissions.onRemoved`.
    NotifyRemoved,
}

/// The result of narrowing an install's access.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[must_use = "the adapter must apply every teardown"]
pub struct RevocationEffect {
    /// API permissions no longer granted.
    pub removed_permissions: Vec<ApiPermission>,
    /// Origins no longer effective.
    pub removed_origins: Vec<HostPattern>,
    /// Tabs whose `activeTab` grant ended.
    pub ended_active_tabs: Vec<ItemId>,
    /// What to tear down.
    pub teardown: BTreeSet<Teardown>,
}

impl RevocationEffect {
    /// Whether nothing changed.
    pub fn is_empty(&self) -> bool {
        self.removed_permissions.is_empty()
            && self.removed_origins.is_empty()
            && self.ended_active_tabs.is_empty()
            && self.teardown.is_empty()
    }
}

/// The result of widening an install's access.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GrantEffect {
    /// Newly granted API permissions.
    pub added_permissions: Vec<ApiPermission>,
    /// Newly effective origins.
    pub added_origins: Vec<HostPattern>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ActiveTabGrant {
    main_document: DocumentGeneration,
    origin: Option<TupleOrigin>,
    file: bool,
}

/// A tab or frame a call wants to act on, from the host's tab state.
#[derive(Clone, Copy, Debug)]
pub struct Target<'a> {
    /// The tab's profile.
    pub profile: ProfileId,
    /// The tab.
    pub tab: ItemId,
    /// The frame.
    pub frame: FrameId,
    /// The tab's current main-frame document.
    pub main_document: DocumentGeneration,
    /// The frame's committed URL.
    pub url: &'a Url,
    /// The frame's committed origin.
    pub origin: &'a DocumentOrigin,
}

/// One install's permission state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstallPermissions {
    declaration: PermissionDeclaration,
    optional_api: BTreeSet<ApiPermission>,
    optional_hosts: Vec<HostPattern>,
    site_access: SiteAccess,
    file_access: bool,
    active_tabs: BTreeMap<ItemId, ActiveTabGrant>,
}

impl InstallPermissions {
    /// State for an install whose required permissions the person accepted,
    /// with the optional grants it already holds.
    pub fn new(
        declaration: PermissionDeclaration,
        granted: &PermissionRequest,
        site_access: SiteAccess,
        file_access: bool,
    ) -> Self {
        let optional_api = granted
            .permissions
            .iter()
            .filter(|permission| declaration.optional_api.contains(permission))
            .cloned()
            .collect();
        let mut optional_hosts: Vec<HostPattern> = Vec::new();
        for origin in &granted.origins {
            let declared = declaration
                .optional_hosts
                .iter()
                .any(|pattern| pattern.contains(origin));
            if declared && !optional_hosts.contains(origin) {
                optional_hosts.push(origin.clone());
            }
        }
        Self {
            declaration,
            optional_api,
            optional_hosts,
            site_access,
            file_access,
            active_tabs: BTreeMap::new(),
        }
    }

    /// Whether an API permission is granted.
    pub fn has_api(&self, name: &str) -> bool {
        let granted = |set: &BTreeSet<ApiPermission>| set.iter().any(|p| p.as_str() == name);
        granted(&self.declaration.required_api) || granted(&self.optional_api)
    }

    /// The person allowed `file:` access.
    pub fn file_access(&self) -> bool {
        self.file_access
    }

    /// The site-access choice.
    pub fn site_access(&self) -> &SiteAccess {
        &self.site_access
    }

    fn granted_hosts(&self) -> impl Iterator<Item = &HostPattern> {
        self.declaration
            .required_hosts
            .iter()
            .chain(&self.optional_hosts)
    }

    fn site_allows(&self, url: &Url) -> bool {
        match &self.site_access {
            SiteAccess::OnAllSites => true,
            SiteAccess::OnSpecificSites(sites) => sites.iter().any(|site| site.matches_url(url)),
            SiteAccess::OnClick => false,
        }
    }

    /// Whether host permissions alone cover `url`.
    pub fn has_host_access(&self, url: &Url) -> bool {
        (url.scheme() != "file" || self.file_access)
            && self.granted_hosts().any(|pattern| pattern.matches_url(url))
            && self.site_allows(url)
    }

    /// Whether site access lets the install's content scripts run in a
    /// document at `url` in `tab`. The scripts' own patterns decide the rest.
    pub fn may_run_content_scripts(
        &self,
        url: &Url,
        tab: ItemId,
        main_document: DocumentGeneration,
    ) -> bool {
        (url.scheme() != "file" || self.file_access)
            && !is_extension_gallery(url)
            && (self.site_allows(url) || self.active_tab_covers(tab, main_document, url))
    }

    fn active_tab_covers(&self, tab: ItemId, main_document: DocumentGeneration, url: &Url) -> bool {
        self.active_tabs.get(&tab).is_some_and(|grant| {
            grant.main_document == main_document
                && match &grant.origin {
                    Some(origin) => TupleOrigin::of_url(url).as_ref() == Some(origin),
                    None => grant.file && url.scheme() == "file",
                }
        })
    }

    /// The person invoked the extension on `tab`, whose main frame shows
    /// `url`. Grants `activeTab` when declared, or one-tab access to a
    /// requested host that site access withholds. Returns whether a grant
    /// was made.
    pub fn grant_active_tab(
        &mut self,
        tab: ItemId,
        main_document: DocumentGeneration,
        url: &Url,
    ) -> bool {
        let file = url.scheme() == "file";
        let eligible = matches!(url.scheme(), "http" | "https") || (file && self.file_access);
        let withheld = self.granted_hosts().any(|pattern| pattern.matches_url(url));
        if !eligible || is_extension_gallery(url) || !(self.has_api(ACTIVE_TAB) || withheld) {
            return false;
        }
        self.active_tabs.insert(
            tab,
            ActiveTabGrant {
                main_document,
                origin: TupleOrigin::of_url(url),
                file,
            },
        );
        true
    }

    /// The tab's main frame committed another document.
    pub fn main_document_changed(
        &mut self,
        tab: ItemId,
        main_document: DocumentGeneration,
    ) -> RevocationEffect {
        if self
            .active_tabs
            .get(&tab)
            .is_some_and(|grant| grant.main_document != main_document)
        {
            return self.tab_closed(tab);
        }
        RevocationEffect::default()
    }

    /// The tab is gone.
    pub fn tab_closed(&mut self, tab: ItemId) -> RevocationEffect {
        let mut effect = RevocationEffect::default();
        if self.active_tabs.remove(&tab).is_some() {
            effect.ended_active_tabs.push(tab);
        }
        effect
    }

    /// Whether the install may act on `target`, as Chrome decides for
    /// `scripting`, `tabs.captureVisibleTab` and similar calls.
    pub fn check_target(&self, sender: &Sender, target: &Target<'_>) -> Result<(), ApiError> {
        if target.profile != sender.profile() {
            return Err(ApiError::cannot_access_page());
        }
        let resolved;
        let url = if matches!(
            target.url.scheme(),
            "about" | "data" | "blob" | "filesystem"
        ) {
            resolved = target
                .origin
                .as_match_url()
                .ok_or_else(ApiError::cannot_access_page)?;
            &resolved
        } else {
            target.url
        };
        if let Some(extension) = sender.scheme().extension_of(url) {
            return if &extension == sender.extension_id() {
                Ok(())
            } else {
                Err(ApiError::cannot_access_other_extension())
            };
        }
        if is_extension_gallery(url) {
            return Err(ApiError::cannot_script_gallery());
        }
        if !matches!(url.scheme(), "http" | "https" | "file")
            || (url.scheme() == "file" && !self.file_access)
        {
            return Err(ApiError::cannot_access_page());
        }
        if self.has_host_access(url)
            || self.active_tab_covers(target.tab, target.main_document, url)
        {
            Ok(())
        } else {
            Err(ApiError::cannot_access_page())
        }
    }

    /// Whether the install may read a tab's URL and title: the `tabs`
    /// permission, host access to it, or an `activeTab` grant.
    pub fn may_read_tab(&self, tab: ItemId, main_document: DocumentGeneration, url: &Url) -> bool {
        self.has_api("tabs")
            || self.has_host_access(url)
            || self.active_tab_covers(tab, main_document, url)
    }

    /// `permissions.contains`.
    pub fn contains(&self, request: &PermissionRequest) -> bool {
        request
            .permissions
            .iter()
            .all(|permission| self.has_api(permission.as_str()))
            && request
                .origins
                .iter()
                .all(|origin| self.effective_contains(origin))
    }

    fn effective_contains(&self, origin: &HostPattern) -> bool {
        (!origin.includes_file() || self.file_access)
            && self.granted_hosts().any(|granted| granted.contains(origin))
            && match &self.site_access {
                SiteAccess::OnAllSites => true,
                SiteAccess::OnSpecificSites(sites) => {
                    sites.iter().any(|site| site.contains(origin))
                }
                SiteAccess::OnClick => false,
            }
    }

    fn effective_origins(&self) -> Vec<HostPattern> {
        let mut origins: Vec<HostPattern> = Vec::new();
        let mut push = |pattern: &HostPattern| {
            if !origins.contains(pattern) {
                origins.push(pattern.clone());
            }
        };
        match &self.site_access {
            SiteAccess::OnAllSites => self.granted_hosts().for_each(&mut push),
            SiteAccess::OnSpecificSites(sites) => {
                for granted in self.granted_hosts() {
                    if sites.iter().any(|site| site.contains(granted)) {
                        push(granted);
                    }
                }
                for site in sites {
                    if self.granted_hosts().any(|granted| granted.contains(site)) {
                        push(site);
                    }
                }
            }
            SiteAccess::OnClick => {}
        }
        origins
    }

    /// `permissions.getAll`.
    pub fn snapshot(&self) -> PermissionsSnapshot {
        let mut permissions: Vec<String> = self
            .declaration
            .required_api
            .iter()
            .chain(&self.optional_api)
            .map(|permission| permission.as_str().to_owned())
            .collect();
        permissions.sort();
        permissions.dedup();
        PermissionsSnapshot {
            permissions,
            origins: self
                .effective_origins()
                .iter()
                .map(|origin| origin.as_str().to_owned())
                .collect(),
        }
    }

    /// Checks a `permissions.request` before the person is asked: everything
    /// not already granted must be declared.
    pub fn check_requestable(&self, request: &PermissionRequest) -> Result<(), ApiError> {
        let declared_api = request.permissions.iter().all(|permission| {
            self.has_api(permission.as_str()) || self.declaration.optional_api.contains(permission)
        });
        let declared_hosts = request.origins.iter().all(|origin| {
            self.declaration
                .optional_hosts
                .iter()
                .chain(&self.declaration.required_hosts)
                .any(|declared| declared.contains(origin))
        });
        if declared_api && declared_hosts {
            Ok(())
        } else {
            Err(ApiError::not_in_manifest())
        }
    }

    /// Grants a request the person accepted. Required hosts the site-access
    /// choice withholds are granted by adding them as specific sites.
    pub fn grant(&mut self, request: &PermissionRequest) -> Result<GrantEffect, ApiError> {
        self.check_requestable(request)?;
        let before = self.effective_origins();
        let mut effect = GrantEffect::default();
        for permission in &request.permissions {
            if !self.has_api(permission.as_str()) && self.optional_api.insert(permission.clone()) {
                effect.added_permissions.push(permission.clone());
            }
        }
        for origin in &request.origins {
            let optional = self
                .declaration
                .optional_hosts
                .iter()
                .any(|declared| declared.contains(origin));
            if optional
                && !self.granted_hosts().any(|granted| granted.contains(origin))
                && !self.optional_hosts.contains(origin)
            {
                self.optional_hosts.push(origin.clone());
            }
            if !self.effective_contains(origin) {
                match &mut self.site_access {
                    SiteAccess::OnAllSites => {}
                    SiteAccess::OnSpecificSites(sites) => sites.push(origin.clone()),
                    SiteAccess::OnClick => {
                        self.site_access = SiteAccess::OnSpecificSites(vec![origin.clone()]);
                    }
                }
            }
        }
        effect.added_origins = self
            .effective_origins()
            .into_iter()
            .filter(|origin| !before.contains(origin))
            .collect();
        Ok(effect)
    }

    /// `permissions.remove`. Required permissions cannot be removed.
    pub fn remove(&mut self, request: &PermissionRequest) -> Result<RevocationEffect, ApiError> {
        let required = request
            .permissions
            .iter()
            .any(|permission| self.declaration.required_api.contains(permission))
            || request
                .origins
                .iter()
                .any(|origin| self.declaration.required_hosts.contains(origin));
        if required {
            return Err(ApiError::cannot_remove_required());
        }
        Ok(self.narrow(|state| {
            for permission in &request.permissions {
                state.optional_api.remove(permission);
            }
            state
                .optional_hosts
                .retain(|granted| !request.origins.contains(granted));
        }))
    }

    /// The person changed site access.
    pub fn set_site_access(&mut self, site_access: SiteAccess) -> RevocationEffect {
        self.narrow(|state| state.site_access = site_access)
    }

    /// The person changed "Allow access to file URLs".
    pub fn set_file_access(&mut self, allowed: bool) -> RevocationEffect {
        let mut effect = self.narrow(|state| state.file_access = allowed);
        if !allowed {
            let ended: Vec<ItemId> = self
                .active_tabs
                .iter()
                .filter(|(_, grant)| grant.file)
                .map(|(tab, _)| *tab)
                .collect();
            for tab in ended {
                self.active_tabs.remove(&tab);
                effect.ended_active_tabs.push(tab);
            }
            effect.teardown.insert(Teardown::RestartContexts);
            effect.teardown.insert(Teardown::RefreshContentScripts);
        }
        effect
    }

    fn narrow(&mut self, change: impl FnOnce(&mut Self)) -> RevocationEffect {
        let api_before: BTreeSet<ApiPermission> = self.optional_api.clone();
        let origins_before = self.effective_origins();
        let site_before = self.site_access.clone();
        change(self);
        let mut effect = RevocationEffect {
            removed_permissions: api_before.difference(&self.optional_api).cloned().collect(),
            ..RevocationEffect::default()
        };
        let origins_after = self.effective_origins();
        effect.removed_origins = origins_before
            .into_iter()
            .filter(|origin| !origins_after.iter().any(|kept| kept.contains(origin)))
            .collect();
        let hosts_narrowed =
            !effect.removed_origins.is_empty() || site_narrowed(&site_before, &self.site_access);
        if hosts_narrowed {
            effect.teardown.extend([
                Teardown::RestartContexts,
                Teardown::RefreshContentScripts,
                Teardown::RepublishNetworkRules,
            ]);
        }
        if effect
            .removed_permissions
            .iter()
            .any(|permission| permission.as_str() == NATIVE_MESSAGING)
        {
            effect.teardown.insert(Teardown::DisconnectNativeHosts);
        }
        if effect
            .removed_permissions
            .iter()
            .any(|permission| permission.as_str().starts_with("declarativeNetRequest"))
        {
            effect.teardown.insert(Teardown::RepublishNetworkRules);
        }
        if !effect.removed_permissions.is_empty() || !effect.removed_origins.is_empty() {
            effect.teardown.insert(Teardown::NotifyRemoved);
        }
        effect
    }
}

fn site_narrowed(before: &SiteAccess, after: &SiteAccess) -> bool {
    match (before, after) {
        (_, SiteAccess::OnAllSites) | (SiteAccess::OnClick, _) => false,
        (SiteAccess::OnAllSites, _) | (SiteAccess::OnSpecificSites(_), SiteAccess::OnClick) => true,
        (SiteAccess::OnSpecificSites(old), SiteAccess::OnSpecificSites(new)) => old
            .iter()
            .any(|site| !new.iter().any(|kept| kept.contains(site))),
    }
}

/// The API permission a method requires.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gate {
    /// Available without a permission.
    Open,
    /// Requires this permission.
    Requires(&'static str),
    /// Requires any one of these.
    RequiresAny(&'static [&'static str]),
}

impl Gate {
    /// Whether `permissions` let `sender` call the method and, when given,
    /// act on `target`.
    pub fn check(
        &self,
        permissions: &InstallPermissions,
        sender: &Sender,
        target: Option<&Target<'_>>,
    ) -> Result<(), ApiError> {
        let granted = match self {
            Self::Open => true,
            Self::Requires(name) => permissions.has_api(name),
            Self::RequiresAny(names) => names.iter().any(|name| permissions.has_api(name)),
        };
        if !granted {
            return Err(ApiError::missing_permission());
        }
        match target {
            Some(target) => permissions.check_target(sender, target),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::context::tests::{background_sender, doc, profile, EXTENSION, OTHER};
    use crate::errors::ApiErrorKind;

    fn url(value: &str) -> Url {
        Url::parse(value).expect("URL")
    }

    fn state(manifest: serde_json::Value) -> InstallPermissions {
        let (declaration, issues) =
            PermissionDeclaration::from_manifest(&Manifest::from_value(manifest));
        assert!(issues.is_empty(), "{issues:?}");
        InstallPermissions::new(
            declaration,
            &PermissionRequest::default(),
            SiteAccess::OnAllSites,
            false,
        )
    }

    fn request(permissions: &[&str], origins: &[&str]) -> PermissionRequest {
        let permissions: Vec<&str> = permissions.to_vec();
        PermissionRequest {
            permissions: permissions
                .iter()
                .map(|name| ApiPermission::parse(name).expect("name"))
                .collect(),
            origins: origins
                .iter()
                .map(|origin| HostPattern::parse(origin).expect("origin"))
                .collect(),
        }
    }

    fn target<'a>(url: &'a Url, origin: &'a DocumentOrigin, main: u64) -> Target<'a> {
        Target {
            profile: profile(),
            tab: ItemId::from(5),
            frame: FrameId::MAIN,
            main_document: doc(main),
            url,
            origin,
        }
    }

    fn check(permissions: &InstallPermissions, at: &str, main: u64) -> Result<(), ApiErrorKind> {
        let at = url(at);
        let origin = DocumentOrigin::of_url(&at);
        permissions
            .check_target(&background_sender(), &target(&at, &origin, main))
            .map_err(|error| error.kind())
    }

    #[test]
    fn manifest_declarations_split_and_report_issues() {
        let (declaration, issues) =
            PermissionDeclaration::from_manifest(&Manifest::from_value(json!({
                "manifest_version": 3,
                "permissions": ["tabs", "bad name", "storage"],
                "optional_permissions": ["history"],
                "host_permissions": ["https://*.a.test/*", "notaurl", "https://*.a.test/x"],
                "optional_host_permissions": ["<all_urls>"]
            })));
        assert_eq!(
            issues,
            [
                DeclarationIssue::InvalidPermission("bad name".into()),
                DeclarationIssue::InvalidHost("notaurl".into())
            ]
        );
        assert_eq!(declaration.required_hosts.len(), 1);
        assert_eq!(declaration.optional_api.len(), 1);
    }

    #[test]
    fn host_access_gates_targets() {
        let permissions = state(json!({"host_permissions": ["https://*.a.test/*"]}));
        assert_eq!(check(&permissions, "https://www.a.test/x", 1), Ok(()));
        assert_eq!(
            check(&permissions, "https://b.test/", 1),
            Err(ApiErrorKind::TargetDenied)
        );
        assert_eq!(
            check(&permissions, "chrome://settings/", 1),
            Err(ApiErrorKind::TargetDenied)
        );
        assert_eq!(
            check(
                &permissions,
                &format!("chrome-extension://{EXTENSION}/a.html"),
                1
            ),
            Ok(())
        );
        let other = format!("chrome-extension://{OTHER}/a.html");
        let other = url(&other);
        let origin = DocumentOrigin::of_url(&other);
        assert_eq!(
            permissions.check_target(&background_sender(), &target(&other, &origin, 1)),
            Err(ApiError::cannot_access_other_extension())
        );
    }

    #[test]
    fn the_web_store_and_file_urls_need_more_than_all_urls() {
        let mut permissions = state(json!({"host_permissions": ["<all_urls>"]}));
        let gallery = url("https://chromewebstore.google.com/detail/x");
        let origin = DocumentOrigin::of_url(&gallery);
        assert_eq!(
            permissions.check_target(&background_sender(), &target(&gallery, &origin, 1)),
            Err(ApiError::cannot_script_gallery())
        );
        assert_eq!(
            check(&permissions, "file:///etc/hosts", 1),
            Err(ApiErrorKind::TargetDenied)
        );
        let _ = permissions.set_file_access(true);
        assert_eq!(check(&permissions, "file:///etc/hosts", 1), Ok(()));
    }

    #[test]
    fn opaque_frames_are_checked_by_their_precursor() {
        let permissions = state(json!({"host_permissions": ["https://a.test/*"]}));
        let data = url("data:text/html,x");
        let creator = url("https://a.test/");
        let from_granted = DocumentOrigin::opaque(Some(&creator));
        assert_eq!(
            permissions.check_target(&background_sender(), &target(&data, &from_granted, 1)),
            Ok(())
        );
        let lost = DocumentOrigin::opaque(None);
        assert_eq!(
            permissions.check_target(&background_sender(), &target(&data, &lost, 1)),
            Err(ApiError::cannot_access_page())
        );
    }

    #[test]
    fn targets_in_other_profiles_are_refused() {
        let permissions = state(json!({"host_permissions": ["<all_urls>"]}));
        let at = url("https://a.test/");
        let origin = DocumentOrigin::of_url(&at);
        let mut elsewhere = target(&at, &origin, 1);
        elsewhere.profile = ProfileId::from(99);
        assert_eq!(
            permissions.check_target(&background_sender(), &elsewhere),
            Err(ApiError::cannot_access_page())
        );
    }

    #[test]
    fn active_tab_is_scoped_to_tab_document_and_origin() {
        let mut permissions = state(json!({"permissions": ["activeTab"]}));
        let tab = ItemId::from(5);
        let page = url("https://a.test/page");
        assert_eq!(
            check(&permissions, "https://a.test/page", 1),
            Err(ApiErrorKind::TargetDenied)
        );
        assert!(permissions.grant_active_tab(tab, doc(1), &page));
        assert_eq!(check(&permissions, "https://a.test/other", 1), Ok(()));
        assert_eq!(
            check(&permissions, "https://b.test/", 1),
            Err(ApiErrorKind::TargetDenied)
        );
        assert_eq!(
            check(&permissions, "https://a.test/page", 2),
            Err(ApiErrorKind::TargetDenied)
        );
        assert!(permissions.may_read_tab(tab, doc(1), &page));
        assert!(permissions.main_document_changed(tab, doc(1)).is_empty());
        let effect = permissions.main_document_changed(tab, doc(2));
        assert_eq!(effect.ended_active_tabs, [tab]);
        assert_eq!(
            check(&permissions, "https://a.test/page", 1),
            Err(ApiErrorKind::TargetDenied)
        );
        assert!(!permissions.grant_active_tab(tab, doc(3), &url("chrome://newtab/")));
        assert!(!permissions.grant_active_tab(tab, doc(3), &url("file:///x")));

        let mut undeclared = state(json!({}));
        assert!(!undeclared.grant_active_tab(tab, doc(1), &page));
    }

    #[test]
    fn site_access_withholds_and_a_click_grants_one_tab() {
        let mut permissions = state(json!({"host_permissions": ["https://*/*"]}));
        let effect = permissions.set_site_access(SiteAccess::OnClick);
        assert!(effect.teardown.contains(&Teardown::RestartContexts));
        assert_eq!(effect.removed_origins.len(), 1);
        assert_eq!(
            check(&permissions, "https://a.test/", 1),
            Err(ApiErrorKind::TargetDenied)
        );
        assert!(permissions.snapshot().origins.is_empty());
        let tab = ItemId::from(5);
        let page = url("https://a.test/");
        assert!(!permissions.may_run_content_scripts(&page, tab, doc(1)));
        assert!(permissions.grant_active_tab(tab, doc(1), &page));
        assert_eq!(check(&permissions, "https://a.test/", 1), Ok(()));
        assert!(permissions.may_run_content_scripts(&page, tab, doc(1)));

        let effect =
            permissions.set_site_access(SiteAccess::OnSpecificSites(vec![HostPattern::parse(
                "https://b.test/*",
            )
            .expect("site")]));
        assert!(effect.is_empty(), "{effect:?}");
        assert_eq!(check(&permissions, "https://b.test/", 9), Ok(()));
        assert_eq!(permissions.snapshot().origins, ["https://b.test/*"]);
        assert!(permissions
            .set_site_access(SiteAccess::OnAllSites)
            .is_empty());
    }

    #[test]
    fn optional_grants_follow_the_declaration() {
        let mut permissions = state(json!({
            "permissions": ["storage"],
            "optional_permissions": ["history", "nativeMessaging"],
            "optional_host_permissions": ["https://*.a.test/*"]
        }));
        assert_eq!(
            permissions.check_requestable(&request(&["bookmarks"], &[])),
            Err(ApiError::not_in_manifest())
        );
        assert_eq!(
            permissions.check_requestable(&request(&[], &["https://b.test/*"])),
            Err(ApiError::not_in_manifest())
        );
        assert_eq!(
            permissions.check_requestable(&request(&["storage"], &[])),
            Ok(())
        );

        let effect = permissions
            .grant(&request(
                &["history", "nativeMessaging"],
                &["https://x.a.test/*"],
            ))
            .expect("grant");
        assert_eq!(effect.added_permissions.len(), 2);
        assert_eq!(effect.added_origins.len(), 1);
        assert!(permissions.contains(&request(&["history"], &["https://x.a.test/*"])));
        assert!(!permissions.contains(&request(&[], &["https://y.a.test/*"])));
        assert_eq!(check(&permissions, "https://x.a.test/", 1), Ok(()));
        assert_eq!(
            permissions.snapshot(),
            PermissionsSnapshot {
                permissions: vec!["history".into(), "nativeMessaging".into(), "storage".into()],
                origins: vec!["https://x.a.test/*".into()],
            }
        );

        assert_eq!(
            permissions.remove(&request(&["storage"], &[])),
            Err(ApiError::cannot_remove_required())
        );
        let effect = permissions
            .remove(&request(&["nativeMessaging"], &["https://x.a.test/*"]))
            .expect("remove");
        assert!(effect.teardown.contains(&Teardown::DisconnectNativeHosts));
        assert!(effect.teardown.contains(&Teardown::RestartContexts));
        assert!(effect.teardown.contains(&Teardown::NotifyRemoved));
        assert_eq!(
            check(&permissions, "https://x.a.test/", 1),
            Err(ApiErrorKind::TargetDenied)
        );
        assert!(permissions.has_api("history"));
    }

    #[test]
    fn requesting_a_withheld_required_host_adds_a_site() {
        let mut permissions = state(json!({"host_permissions": ["https://*/*"]}));
        let _ = permissions.set_site_access(SiteAccess::OnClick);
        permissions
            .grant(&request(&[], &["https://a.test/*"]))
            .expect("grant");
        assert_eq!(check(&permissions, "https://a.test/", 1), Ok(()));
        assert_eq!(
            check(&permissions, "https://b.test/", 1),
            Err(ApiErrorKind::TargetDenied)
        );
    }

    #[test]
    fn gate_checks_the_api_permission_before_the_target() {
        let permissions = state(json!({"permissions": ["scripting"]}));
        let sender = background_sender();
        assert_eq!(Gate::Open.check(&permissions, &sender, None), Ok(()));
        assert_eq!(
            Gate::Requires("scripting").check(&permissions, &sender, None),
            Ok(())
        );
        assert_eq!(
            Gate::Requires("history").check(&permissions, &sender, None),
            Err(ApiError::missing_permission())
        );
        assert_eq!(
            Gate::RequiresAny(&["declarativeNetRequest", "scripting"]).check(
                &permissions,
                &sender,
                None
            ),
            Ok(())
        );
        let at = url("https://a.test/");
        let origin = DocumentOrigin::of_url(&at);
        assert_eq!(
            Gate::Requires("scripting").check(
                &permissions,
                &sender,
                Some(&target(&at, &origin, 1))
            ),
            Err(ApiError::cannot_access_page())
        );
    }

    #[test]
    fn requests_parse_with_chromes_errors() {
        assert_eq!(
            PermissionRequest::parse(&["bad name"], &[]),
            Err(ApiError::unknown_permission("bad name"))
        );
        let error = PermissionRequest::parse(&[], &["nope"]).expect_err("invalid");
        assert!(error
            .message()
            .starts_with("Invalid value for origin pattern nope: "));
        let parsed = PermissionRequest::parse(&["history"], &["https://*/*"]).expect("parse");
        assert_eq!(
            parsed.warnings(),
            [
                "Read and change all your data on all websites",
                "Read and change your browsing history"
            ]
        );
    }
}
