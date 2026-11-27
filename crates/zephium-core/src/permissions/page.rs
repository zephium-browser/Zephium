//! Pure, bounded page-permission aggregate and CAS patch contracts.

use std::collections::HashSet;
use std::error::Error;
use std::fmt;
use std::net::IpAddr;
use std::num::NonZeroU64;

use url::Url;

use crate::ids::PagePermissionGrantId;
use crate::navigation;

pub const MAX_PAGE_ORIGIN_BYTES: usize = 512;
pub const MAX_PAGE_PERMISSION_GRANTS_PER_PROFILE: usize = 512;
pub const MAX_PAGE_PERMISSION_PATCH_CHANGES: usize = 4;
pub const MAX_PAGE_PERMISSION_CATALOG_RETAINED_BYTES: usize = 512 * 1024;
/// Process-wide native permission completions retained concurrently by the
/// engine. Shell currently serializes their user-visible presentation.
pub const MAX_PENDING_PAGE_PERMISSION_REQUESTS: usize = 8;
const MAX_DURABLE_REVISION: u64 = i64::MAX as u64;
const ALLOCATION_MARGIN_BYTES: usize = 2 * std::mem::size_of::<usize>();

macro_rules! durable_revision {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(u64);

        impl $name {
            pub const INITIAL: Self = Self(1);

            pub const fn new(value: u64) -> Option<Self> {
                if value == 0 || value > MAX_DURABLE_REVISION {
                    None
                } else {
                    Some(Self(value))
                }
            }

            pub const fn get(self) -> u64 {
                self.0
            }

            pub const fn next(self) -> Option<Self> {
                match self.0.checked_add(1) {
                    Some(next) => Self::new(next),
                    None => None,
                }
            }
        }
    };
}

durable_revision!(PagePermissionGrantRevision);
durable_revision!(PagePermissionCatalogRevision);

/// A canonical ASCII HTTP(S) tuple origin such as `https://example.com`.
///
/// Paths, credentials, opaque origins, internal pseudo-hosts, and unsupported
/// schemes cannot be represented. The private allocation prevents callers
/// from constructing a non-canonical durable authority key.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PageOrigin(Box<str>);

impl PageOrigin {
    /// Derives the canonical tuple origin for an already parsed page URL.
    pub fn from_url(url: &Url) -> Result<Self, PageOriginError> {
        if !matches!(url.scheme(), "http" | "https") || !navigation::is_allowed(url) {
            return Err(PageOriginError::Unsupported);
        }
        let url::Origin::Tuple(..) = url.origin() else {
            return Err(PageOriginError::Opaque);
        };
        let canonical = url.origin().ascii_serialization();
        if canonical.is_empty() || canonical.len() > MAX_PAGE_ORIGIN_BYTES {
            return Err(PageOriginError::TooLong {
                length: canonical.len(),
                max: MAX_PAGE_ORIGIN_BYTES,
            });
        }
        Ok(Self(canonical.into_boxed_str()))
    }

    /// Parses a durable origin key and requires the input itself to already be
    /// the canonical ASCII tuple serialization.
    pub fn parse_exact(value: &str) -> Result<Self, PageOriginError> {
        if value.is_empty() {
            return Err(PageOriginError::Empty);
        }
        if value.len() > MAX_PAGE_ORIGIN_BYTES {
            return Err(PageOriginError::TooLong {
                length: value.len(),
                max: MAX_PAGE_ORIGIN_BYTES,
            });
        }
        let url = Url::parse(value).map_err(|_| PageOriginError::Malformed)?;
        let origin = Self::from_url(&url)?;
        if origin.as_str() != value {
            return Err(PageOriginError::NotCanonical);
        }
        Ok(origin)
    }

    /// Canonicalizes bounded components obtained from a native WebView
    /// security-origin object. JavaScript strings must never use this path.
    ///
    /// The component form avoids interpolating an attacker-controlled host
    /// into a URL string. `url` performs host parsing (including IPv6 and IDN
    /// canonicalization), and this aggregate still admits only HTTP(S) tuple
    /// origins under Zephium's navigation policy.
    pub fn from_native_components(
        scheme: &str,
        host: &str,
        port: Option<u16>,
    ) -> Result<Self, PageOriginError> {
        if !matches!(scheme, "http" | "https") {
            return Err(PageOriginError::Unsupported);
        }
        if host.is_empty() {
            return Err(PageOriginError::Empty);
        }
        if scheme.len().saturating_add(host.len()) > MAX_PAGE_ORIGIN_BYTES {
            return Err(PageOriginError::TooLong {
                length: scheme.len().saturating_add(host.len()),
                max: MAX_PAGE_ORIGIN_BYTES,
            });
        }
        let mut url =
            Url::parse("http://native-origin.invalid").map_err(|_| PageOriginError::Malformed)?;
        url.set_scheme(scheme)
            .map_err(|_| PageOriginError::Malformed)?;
        let ip_literal = host
            .strip_prefix('[')
            .and_then(|host| host.strip_suffix(']'))
            .unwrap_or(host)
            .parse::<IpAddr>();
        if let Ok(address) = ip_literal {
            url.set_ip_host(address)
                .map_err(|_| PageOriginError::Malformed)?;
        } else {
            url.set_host(Some(host))
                .map_err(|_| PageOriginError::Malformed)?;
        }
        url.set_port(port).map_err(|_| PageOriginError::Malformed)?;
        Self::from_url(&url)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl AsRef<str> for PageOrigin {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for PageOrigin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl fmt::Debug for PageOrigin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("PageOrigin").field(&self.0).finish()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageOriginError {
    Empty,
    Malformed,
    Unsupported,
    Opaque,
    NotCanonical,
    TooLong { length: usize, max: usize },
}

impl fmt::Display for PageOriginError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("page origin is empty"),
            Self::Malformed => formatter.write_str("page origin is malformed"),
            Self::Unsupported => {
                formatter.write_str("page origin is not an allowed HTTP(S) origin")
            }
            Self::Opaque => formatter.write_str("page origin is opaque"),
            Self::NotCanonical => formatter.write_str("page origin is not canonical"),
            Self::TooLong { length, max } => {
                write!(formatter, "page origin is {length} bytes; limit is {max}")
            }
        }
    }
}

impl Error for PageOriginError {}

/// Closed page-capability vocabulary shared by native events and persistence.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PagePermissionKind {
    Geolocation,
    Camera,
    Microphone,
    Notifications,
    ClipboardRead,
}

impl PagePermissionKind {
    pub const fn as_persisted(self) -> &'static str {
        match self {
            Self::Geolocation => "geolocation",
            Self::Camera => "camera",
            Self::Microphone => "microphone",
            Self::Notifications => "notifications",
            Self::ClipboardRead => "clipboard_read",
        }
    }

    pub fn from_persisted(value: &str) -> Option<Self> {
        match value {
            "geolocation" => Some(Self::Geolocation),
            "camera" => Some(Self::Camera),
            "microphone" => Some(Self::Microphone),
            "notifications" => Some(Self::Notifications),
            "clipboard_read" => Some(Self::ClipboardRead),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RememberedPagePermission {
    Allow,
    Deny,
}

/// Process-local correlation identity for one retained native permission
/// completion. The engine mints it; page content and privileged UI cannot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PagePermissionRequestId(NonZeroU64);

impl PagePermissionRequestId {
    pub const fn new(value: u64) -> Option<Self> {
        match NonZeroU64::new(value) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }

    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

/// Atomic capability cohort represented by one native completion.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PagePermissionRequestKind {
    Single(PagePermissionKind),
    CameraAndMicrophone,
}

impl PagePermissionRequestKind {
    pub fn contains(self, kind: PagePermissionKind) -> bool {
        match self {
            Self::Single(candidate) => candidate == kind,
            Self::CameraAndMicrophone => matches!(
                kind,
                PagePermissionKind::Camera | PagePermissionKind::Microphone
            ),
        }
    }

    pub const fn len(self) -> usize {
        match self {
            Self::Single(_) => 1,
            Self::CameraAndMicrophone => 2,
        }
    }

    pub const fn is_empty(self) -> bool {
        false
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PagePermissionRequest {
    pub id: PagePermissionRequestId,
    pub origin: PageOrigin,
    pub kind: PagePermissionRequestKind,
    /// The operating system refuses the browser this capability, so an
    /// Allow reaches nothing until the person changes it there.
    pub blocked_by_system: bool,
}

/// Explicit settlement for one exact retained native completion.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PagePermissionRequestSettlement {
    Allow,
    Deny,
}

impl RememberedPagePermission {
    pub const fn as_persisted(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Deny => "deny",
        }
    }

    pub fn from_persisted(value: &str) -> Option<Self> {
        match value {
            "allow" => Some(Self::Allow),
            "deny" => Some(Self::Deny),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PagePermissionGrant {
    pub id: PagePermissionGrantId,
    pub revision: PagePermissionGrantRevision,
    pub origin: PageOrigin,
    pub kind: PagePermissionKind,
    pub decision: RememberedPagePermission,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PagePermissionCatalog {
    revision: PagePermissionCatalogRevision,
    grants: Vec<PagePermissionGrant>,
    retained_bytes: usize,
}

impl PagePermissionCatalog {
    pub fn new(
        revision: PagePermissionCatalogRevision,
        grants: Vec<PagePermissionGrant>,
    ) -> Result<Self, PagePermissionCatalogError> {
        if grants.len() > MAX_PAGE_PERMISSION_GRANTS_PER_PROFILE {
            return Err(PagePermissionCatalogError::TooManyGrants {
                count: grants.len(),
                max: MAX_PAGE_PERMISSION_GRANTS_PER_PROFILE,
            });
        }
        // Never retain attacker-inflated spare capacity from a caller-owned
        // request. Reallocation makes the accepted allocation depend only on
        // the bounded logical length; the budget below still charges the
        // allocator's actual resulting capacity.
        let mut compact = Vec::with_capacity(grants.len());
        compact.extend(grants);
        compact.shrink_to_fit();
        let mut grants = compact;
        grants.sort_unstable_by_key(|grant| grant.id);
        let mut ids = HashSet::with_capacity(grants.len());
        let mut authorities: HashSet<(&str, PagePermissionKind)> =
            HashSet::with_capacity(grants.len());
        let mut retained_bytes = grants
            .capacity()
            .checked_mul(std::mem::size_of::<PagePermissionGrant>())
            .ok_or(PagePermissionCatalogError::RetainedBytesExceeded {
                bytes: usize::MAX,
                max: MAX_PAGE_PERMISSION_CATALOG_RETAINED_BYTES,
            })?;
        if grants.capacity() != 0 {
            retained_bytes = retained_bytes.checked_add(ALLOCATION_MARGIN_BYTES).ok_or(
                PagePermissionCatalogError::RetainedBytesExceeded {
                    bytes: usize::MAX,
                    max: MAX_PAGE_PERMISSION_CATALOG_RETAINED_BYTES,
                },
            )?;
        }
        for grant in &grants {
            if !ids.insert(grant.id) {
                return Err(PagePermissionCatalogError::DuplicateId(grant.id));
            }
            if !authorities.insert((grant.origin.as_str(), grant.kind)) {
                return Err(PagePermissionCatalogError::DuplicateAuthority {
                    origin: grant.origin.clone(),
                    kind: grant.kind,
                });
            }
            retained_bytes = retained_bytes
                .checked_add(grant.origin.len())
                .and_then(|bytes| bytes.checked_add(ALLOCATION_MARGIN_BYTES))
                .ok_or(PagePermissionCatalogError::RetainedBytesExceeded {
                    bytes: usize::MAX,
                    max: MAX_PAGE_PERMISSION_CATALOG_RETAINED_BYTES,
                })?;
            if retained_bytes > MAX_PAGE_PERMISSION_CATALOG_RETAINED_BYTES {
                return Err(PagePermissionCatalogError::RetainedBytesExceeded {
                    bytes: retained_bytes,
                    max: MAX_PAGE_PERMISSION_CATALOG_RETAINED_BYTES,
                });
            }
        }
        Ok(Self {
            revision,
            grants,
            retained_bytes,
        })
    }

    pub const fn revision(&self) -> PagePermissionCatalogRevision {
        self.revision
    }

    pub fn grants(&self) -> &[PagePermissionGrant] {
        &self.grants
    }

    pub const fn retained_bytes(&self) -> usize {
        // Conservative dynamic allocation charge: actual Vec capacity plus a
        // fixed allocator margin for the Vec block and every boxed origin.
        self.retained_bytes
    }

    #[cfg(test)]
    fn grant_capacity(&self) -> usize {
        self.grants.capacity()
    }

    pub fn get(&self, id: PagePermissionGrantId) -> Option<&PagePermissionGrant> {
        self.grants
            .binary_search_by_key(&id, |grant| grant.id)
            .ok()
            .map(|index| &self.grants[index])
    }

    pub fn decision(
        &self,
        origin: &PageOrigin,
        kind: PagePermissionKind,
    ) -> Option<RememberedPagePermission> {
        self.grants
            .iter()
            .find(|grant| grant.origin == *origin && grant.kind == kind)
            .map(|grant| grant.decision)
    }

    /// Applies a bounded patch as one aggregate transition. The catalog is
    /// consumed so the common mutation path does not clone every origin.
    pub fn apply_patch(
        mut self,
        patch: &PagePermissionPatch,
    ) -> Result<PagePermissionPatchApplication, PagePermissionApplyError> {
        for change in patch.changes() {
            match change {
                PagePermissionChange::Create { id, .. } => {
                    if self.get(*id).is_some() {
                        return Err(PagePermissionApplyError::Invalid);
                    }
                }
                PagePermissionChange::Update { id, expected, .. }
                | PagePermissionChange::Delete { id, expected } => {
                    let Some(current) = self.get(*id) else {
                        return Err(PagePermissionApplyError::Invalid);
                    };
                    if current.revision != *expected {
                        return Err(PagePermissionApplyError::Invalid);
                    }
                }
            }
        }

        let changed = patch.changes().iter().any(|change| match change {
            PagePermissionChange::Create { .. } | PagePermissionChange::Delete { .. } => true,
            PagePermissionChange::Update { id, decision, .. } => self
                .get(*id)
                .is_some_and(|current| current.decision != *decision),
        });
        let next_catalog_revision = if changed {
            self.revision
                .next()
                .ok_or(PagePermissionApplyError::RevisionExhausted)?
        } else {
            self.revision
        };

        let deleted: HashSet<_> = patch
            .changes()
            .iter()
            .filter_map(|change| match change {
                PagePermissionChange::Delete { id, .. } => Some(*id),
                _ => None,
            })
            .collect();
        self.grants.retain(|grant| !deleted.contains(&grant.id));

        for change in patch.changes() {
            if let PagePermissionChange::Update { id, decision, .. } = change {
                let grant = self
                    .grants
                    .iter_mut()
                    .find(|grant| grant.id == *id)
                    .ok_or(PagePermissionApplyError::Invalid)?;
                if grant.decision != *decision {
                    grant.revision = grant
                        .revision
                        .next()
                        .ok_or(PagePermissionApplyError::RevisionExhausted)?;
                    grant.decision = *decision;
                }
            }
        }
        for change in patch.changes() {
            if let PagePermissionChange::Create {
                id,
                origin,
                kind,
                decision,
            } = change
            {
                self.grants.push(PagePermissionGrant {
                    id: *id,
                    revision: PagePermissionGrantRevision::INITIAL,
                    origin: origin.clone(),
                    kind: *kind,
                    decision: *decision,
                });
            }
        }

        let catalog =
            Self::new(next_catalog_revision, self.grants).map_err(|error| match error {
                PagePermissionCatalogError::TooManyGrants { .. }
                | PagePermissionCatalogError::RetainedBytesExceeded { .. } => {
                    PagePermissionApplyError::LimitReached
                }
                PagePermissionCatalogError::DuplicateId(_)
                | PagePermissionCatalogError::DuplicateAuthority { .. } => {
                    PagePermissionApplyError::Invalid
                }
            })?;
        let results = patch
            .changes()
            .iter()
            .map(|change| match change {
                PagePermissionChange::Create { id, .. }
                | PagePermissionChange::Update { id, .. } => PagePermissionChangeResult {
                    id: *id,
                    grant: catalog.get(*id).cloned().map(Box::new),
                },
                PagePermissionChange::Delete { id, .. } => PagePermissionChangeResult {
                    id: *id,
                    grant: None,
                },
            })
            .collect();
        let results = PagePermissionPatchResults::new(results)
            .map_err(|_| PagePermissionApplyError::Invalid)?;
        Ok(PagePermissionPatchApplication {
            catalog,
            results,
            changed,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PagePermissionCatalogError {
    TooManyGrants {
        count: usize,
        max: usize,
    },
    RetainedBytesExceeded {
        bytes: usize,
        max: usize,
    },
    DuplicateId(PagePermissionGrantId),
    DuplicateAuthority {
        origin: PageOrigin,
        kind: PagePermissionKind,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PagePermissionChange {
    Create {
        id: PagePermissionGrantId,
        origin: PageOrigin,
        kind: PagePermissionKind,
        decision: RememberedPagePermission,
    },
    Update {
        id: PagePermissionGrantId,
        expected: PagePermissionGrantRevision,
        decision: RememberedPagePermission,
    },
    Delete {
        id: PagePermissionGrantId,
        expected: PagePermissionGrantRevision,
    },
}

impl PagePermissionChange {
    pub const fn id(&self) -> PagePermissionGrantId {
        match self {
            Self::Create { id, .. } | Self::Update { id, .. } | Self::Delete { id, .. } => *id,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PagePermissionPatch {
    changes: Vec<PagePermissionChange>,
}

impl PagePermissionPatch {
    pub fn new(changes: Vec<PagePermissionChange>) -> Result<Self, PagePermissionPatchError> {
        if changes.is_empty() {
            return Err(PagePermissionPatchError::Empty);
        }
        if changes.len() > MAX_PAGE_PERMISSION_PATCH_CHANGES {
            return Err(PagePermissionPatchError::TooManyChanges {
                count: changes.len(),
                max: MAX_PAGE_PERMISSION_PATCH_CHANGES,
            });
        }
        let mut compact = Vec::with_capacity(changes.len());
        compact.extend(changes);
        compact.shrink_to_fit();
        let changes = compact;
        let mut ids = HashSet::with_capacity(changes.len());
        let mut creates: HashSet<(&str, PagePermissionKind)> =
            HashSet::with_capacity(changes.len());
        for change in &changes {
            if !ids.insert(change.id()) {
                return Err(PagePermissionPatchError::DuplicateId(change.id()));
            }
            if let PagePermissionChange::Create { origin, kind, .. } = change {
                if !creates.insert((origin.as_str(), *kind)) {
                    return Err(PagePermissionPatchError::DuplicateCreateAuthority {
                        origin: origin.clone(),
                        kind: *kind,
                    });
                }
            }
        }
        Ok(Self { changes })
    }

    pub fn changes(&self) -> &[PagePermissionChange] {
        &self.changes
    }

    #[cfg(test)]
    fn change_capacity(&self) -> usize {
        self.changes.capacity()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PagePermissionPatchError {
    Empty,
    TooManyChanges {
        count: usize,
        max: usize,
    },
    DuplicateId(PagePermissionGrantId),
    DuplicateCreateAuthority {
        origin: PageOrigin,
        kind: PagePermissionKind,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PagePermissionApplyError {
    Invalid,
    LimitReached,
    RevisionExhausted,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PagePermissionChangeResult {
    pub id: PagePermissionGrantId,
    /// Exact durable row after create/update; deletion is `None`.
    pub grant: Option<Box<PagePermissionGrant>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PagePermissionPatchResults {
    results: Vec<PagePermissionChangeResult>,
}

impl PagePermissionPatchResults {
    fn new(results: Vec<PagePermissionChangeResult>) -> Result<Self, PagePermissionPatchError> {
        if results.is_empty() {
            return Err(PagePermissionPatchError::Empty);
        }
        if results.len() > MAX_PAGE_PERMISSION_PATCH_CHANGES {
            return Err(PagePermissionPatchError::TooManyChanges {
                count: results.len(),
                max: MAX_PAGE_PERMISSION_PATCH_CHANGES,
            });
        }
        let mut compact = Vec::with_capacity(results.len());
        compact.extend(results);
        compact.shrink_to_fit();
        let results = compact;
        let mut ids = HashSet::with_capacity(results.len());
        for result in &results {
            if !ids.insert(result.id) {
                return Err(PagePermissionPatchError::DuplicateId(result.id));
            }
        }
        Ok(Self { results })
    }

    pub fn as_slice(&self) -> &[PagePermissionChangeResult] {
        &self.results
    }

    #[cfg(test)]
    fn result_capacity(&self) -> usize {
        self.results.capacity()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PagePermissionPatchApplication {
    catalog: PagePermissionCatalog,
    results: PagePermissionPatchResults,
    changed: bool,
}

impl PagePermissionPatchApplication {
    pub fn catalog(&self) -> &PagePermissionCatalog {
        &self.catalog
    }

    pub fn results(&self) -> &PagePermissionPatchResults {
        &self.results
    }

    pub const fn changed(&self) -> bool {
        self.changed
    }

    pub fn into_parts(self) -> (PagePermissionCatalog, PagePermissionPatchResults, bool) {
        (self.catalog, self.results, self.changed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn origin(value: &str) -> PageOrigin {
        PageOrigin::parse_exact(value).unwrap()
    }

    fn grant(id: u128, value: &str, kind: PagePermissionKind) -> PagePermissionGrant {
        PagePermissionGrant {
            id: PagePermissionGrantId::from(id),
            revision: PagePermissionGrantRevision::INITIAL,
            origin: origin(value),
            kind,
            decision: RememberedPagePermission::Allow,
        }
    }

    #[test]
    fn origin_derivation_is_canonical_and_exact_persistence_rejects_aliases() {
        let parsed = Url::parse("HTTPS://BÜCHER.example:443/path?q=1#fragment").unwrap();
        assert_eq!(
            PageOrigin::from_url(&parsed).unwrap().as_str(),
            "https://xn--bcher-kva.example"
        );
        assert_eq!(
            PageOrigin::from_url(&Url::parse("http://EXAMPLE.com:80/a").unwrap())
                .unwrap()
                .as_str(),
            "http://example.com"
        );
        for invalid in [
            "https://example.com/",
            "https://EXAMPLE.com",
            "https://example.com:443",
            "https://user:secret@example.com",
            "file:///tmp/private",
            "about:blank",
            "data:text/html,hi",
            "blob:https://example.com/id",
            "http://tauri.localhost",
            "https://example.com/path",
            "",
        ] {
            assert!(
                PageOrigin::parse_exact(invalid).is_err(),
                "accepted {invalid}"
            );
        }
    }

    #[test]
    fn native_origin_components_are_structured_canonical_and_bounded() {
        assert_eq!(
            PageOrigin::from_native_components("https", "BÜCHER.example", Some(443))
                .unwrap()
                .as_str(),
            "https://xn--bcher-kva.example"
        );
        assert_eq!(
            PageOrigin::from_native_components("http", "::1", Some(8080))
                .unwrap()
                .as_str(),
            "http://[::1]:8080"
        );
        for (scheme, host, port) in [
            ("file", "example.com", None),
            ("https", "", None),
            ("https", "user@example.com", None),
            ("https", "example.com/path", None),
        ] {
            assert!(PageOrigin::from_native_components(scheme, host, port).is_err());
        }
        assert!(PageOrigin::from_native_components("https", &"x".repeat(513), None).is_err());

        let pair = PagePermissionRequestKind::CameraAndMicrophone;
        assert_eq!(pair.len(), 2);
        assert!(pair.contains(PagePermissionKind::Camera));
        assert!(pair.contains(PagePermissionKind::Microphone));
        assert!(!pair.contains(PagePermissionKind::Notifications));
        assert!(PagePermissionRequestId::new(0).is_none());
        assert_eq!(PagePermissionRequestId::new(9).unwrap().get(), 9);
    }

    #[test]
    fn catalog_is_sorted_and_rejects_duplicate_ids_or_authority() {
        let high = grant(2, "https://two.example", PagePermissionKind::Camera);
        let low = grant(1, "https://one.example", PagePermissionKind::Camera);
        let catalog = PagePermissionCatalog::new(
            PagePermissionCatalogRevision::INITIAL,
            vec![high.clone(), low.clone()],
        )
        .unwrap();
        assert_eq!(catalog.grants()[0].id, low.id);
        assert_eq!(catalog.grants()[1].id, high.id);
        assert!(catalog.retained_bytes() > low.origin.len() + high.origin.len());

        assert!(matches!(
            PagePermissionCatalog::new(
                PagePermissionCatalogRevision::INITIAL,
                vec![low.clone(), low]
            ),
            Err(PagePermissionCatalogError::DuplicateId(_))
        ));
        let first = grant(3, "https://same.example", PagePermissionKind::Camera);
        let second = grant(4, "https://same.example", PagePermissionKind::Camera);
        assert!(matches!(
            PagePermissionCatalog::new(PagePermissionCatalogRevision::INITIAL, vec![first, second]),
            Err(PagePermissionCatalogError::DuplicateAuthority { .. })
        ));
    }

    #[test]
    fn catalog_and_revision_limits_are_exact() {
        let grants = (0..=MAX_PAGE_PERMISSION_GRANTS_PER_PROFILE)
            .map(|index| {
                grant(
                    index as u128 + 1,
                    &format!("https://p{index}.example"),
                    PagePermissionKind::Camera,
                )
            })
            .collect();
        assert!(matches!(
            PagePermissionCatalog::new(PagePermissionCatalogRevision::INITIAL, grants),
            Err(PagePermissionCatalogError::TooManyGrants { count, max })
                if count == MAX_PAGE_PERMISSION_GRANTS_PER_PROFILE + 1
                    && max == MAX_PAGE_PERMISSION_GRANTS_PER_PROFILE
        ));

        let at_catalog_limit = PagePermissionCatalog::new(
            PagePermissionCatalogRevision::new(i64::MAX as u64).unwrap(),
            vec![grant(
                1,
                "https://catalog-limit.example",
                PagePermissionKind::Camera,
            )],
        )
        .unwrap();
        let create = PagePermissionPatch::new(vec![PagePermissionChange::Create {
            id: PagePermissionGrantId::from(2),
            origin: origin("https://new.example"),
            kind: PagePermissionKind::Camera,
            decision: RememberedPagePermission::Allow,
        }])
        .unwrap();
        assert_eq!(
            at_catalog_limit.apply_patch(&create),
            Err(PagePermissionApplyError::RevisionExhausted)
        );

        let mut row_at_limit = grant(
            3,
            "https://row-limit.example",
            PagePermissionKind::Microphone,
        );
        row_at_limit.revision = PagePermissionGrantRevision::new(i64::MAX as u64).unwrap();
        let row_id = row_at_limit.id;
        let catalog =
            PagePermissionCatalog::new(PagePermissionCatalogRevision::INITIAL, vec![row_at_limit])
                .unwrap();
        let update = PagePermissionPatch::new(vec![PagePermissionChange::Update {
            id: row_id,
            expected: PagePermissionGrantRevision::new(i64::MAX as u64).unwrap(),
            decision: RememberedPagePermission::Deny,
        }])
        .unwrap();
        assert_eq!(
            catalog.apply_patch(&update),
            Err(PagePermissionApplyError::RevisionExhausted)
        );
    }

    #[test]
    fn accepted_boundaries_discard_caller_inflated_vector_capacity() {
        let mut grants = Vec::with_capacity(100_000);
        grants.push(grant(1, "https://example.com", PagePermissionKind::Camera));
        let catalog =
            PagePermissionCatalog::new(PagePermissionCatalogRevision::INITIAL, grants).unwrap();
        assert!(catalog.grant_capacity() <= MAX_PAGE_PERMISSION_GRANTS_PER_PROFILE);
        assert!(catalog.retained_bytes() >= std::mem::size_of::<PagePermissionGrant>());
        assert!(catalog.retained_bytes() <= MAX_PAGE_PERMISSION_CATALOG_RETAINED_BYTES);

        let mut changes = Vec::with_capacity(100_000);
        changes.push(PagePermissionChange::Update {
            id: PagePermissionGrantId::from(1),
            expected: PagePermissionGrantRevision::INITIAL,
            decision: RememberedPagePermission::Deny,
        });
        let patch = PagePermissionPatch::new(changes).unwrap();
        assert!(patch.change_capacity() <= MAX_PAGE_PERMISSION_PATCH_CHANGES);
        let application = catalog.apply_patch(&patch).unwrap();
        assert!(application.results().result_capacity() <= MAX_PAGE_PERMISSION_PATCH_CHANGES);
    }

    #[test]
    fn patch_is_bounded_unique_atomic_and_noop_preserves_revisions() {
        let camera = grant(1, "https://example.com", PagePermissionKind::Camera);
        let microphone = grant(2, "https://example.com", PagePermissionKind::Microphone);
        let catalog = PagePermissionCatalog::new(
            PagePermissionCatalogRevision::INITIAL,
            vec![camera.clone(), microphone.clone()],
        )
        .unwrap();
        let patch = PagePermissionPatch::new(vec![
            PagePermissionChange::Update {
                id: camera.id,
                expected: camera.revision,
                decision: RememberedPagePermission::Deny,
            },
            PagePermissionChange::Update {
                id: microphone.id,
                expected: microphone.revision,
                decision: RememberedPagePermission::Deny,
            },
        ])
        .unwrap();
        let applied = catalog.apply_patch(&patch).unwrap();
        assert!(applied.changed());
        assert_eq!(applied.catalog().revision().get(), 2);
        assert_eq!(applied.results().as_slice().len(), 2);
        assert!(applied.catalog().grants().iter().all(|grant| {
            grant.revision.get() == 2 && grant.decision == RememberedPagePermission::Deny
        }));

        let no_op = PagePermissionPatch::new(vec![PagePermissionChange::Update {
            id: camera.id,
            expected: PagePermissionGrantRevision::new(2).unwrap(),
            decision: RememberedPagePermission::Deny,
        }])
        .unwrap();
        let applied = applied.into_parts().0.apply_patch(&no_op).unwrap();
        assert!(!applied.changed());
        assert_eq!(applied.catalog().revision().get(), 2);
        assert_eq!(
            applied.results().as_slice()[0]
                .grant
                .as_ref()
                .unwrap()
                .revision
                .get(),
            2
        );
    }

    #[test]
    fn stale_or_invalid_patch_is_rejected_before_any_transition() {
        let camera = grant(1, "https://example.com", PagePermissionKind::Camera);
        let catalog = PagePermissionCatalog::new(
            PagePermissionCatalogRevision::INITIAL,
            vec![camera.clone()],
        )
        .unwrap();
        let patch = PagePermissionPatch::new(vec![
            PagePermissionChange::Update {
                id: camera.id,
                expected: camera.revision,
                decision: RememberedPagePermission::Deny,
            },
            PagePermissionChange::Delete {
                id: PagePermissionGrantId::from(999),
                expected: PagePermissionGrantRevision::INITIAL,
            },
        ])
        .unwrap();
        assert_eq!(
            catalog.apply_patch(&patch),
            Err(PagePermissionApplyError::Invalid)
        );
    }

    #[test]
    fn patch_constructor_rejects_empty_oversized_and_duplicate_targets() {
        assert_eq!(
            PagePermissionPatch::new(Vec::new()),
            Err(PagePermissionPatchError::Empty)
        );
        let changes = (1..=MAX_PAGE_PERMISSION_PATCH_CHANGES + 1)
            .map(|id| PagePermissionChange::Delete {
                id: PagePermissionGrantId::from(id as u128),
                expected: PagePermissionGrantRevision::INITIAL,
            })
            .collect();
        assert!(matches!(
            PagePermissionPatch::new(changes),
            Err(PagePermissionPatchError::TooManyChanges { .. })
        ));
        let duplicate_id = PagePermissionGrantId::from(1);
        assert!(matches!(
            PagePermissionPatch::new(vec![
                PagePermissionChange::Delete {
                    id: duplicate_id,
                    expected: PagePermissionGrantRevision::INITIAL,
                },
                PagePermissionChange::Update {
                    id: duplicate_id,
                    expected: PagePermissionGrantRevision::INITIAL,
                    decision: RememberedPagePermission::Deny,
                },
            ]),
            Err(PagePermissionPatchError::DuplicateId(_))
        ));
        let duplicate_origin = origin("https://duplicate.example");
        assert!(matches!(
            PagePermissionPatch::new(vec![
                PagePermissionChange::Create {
                    id: PagePermissionGrantId::from(2),
                    origin: duplicate_origin.clone(),
                    kind: PagePermissionKind::Camera,
                    decision: RememberedPagePermission::Allow,
                },
                PagePermissionChange::Create {
                    id: PagePermissionGrantId::from(3),
                    origin: duplicate_origin,
                    kind: PagePermissionKind::Camera,
                    decision: RememberedPagePermission::Deny,
                },
            ]),
            Err(PagePermissionPatchError::DuplicateCreateAuthority { .. })
        ));
    }

    #[test]
    fn revisions_match_the_signed_durable_domain_and_never_wrap() {
        assert!(PagePermissionGrantRevision::new(0).is_none());
        assert!(PagePermissionCatalogRevision::new(0).is_none());
        assert!(PagePermissionGrantRevision::new(i64::MAX as u64)
            .unwrap()
            .next()
            .is_none());
        assert!(PagePermissionCatalogRevision::new(i64::MAX as u64 + 1).is_none());
    }

    proptest! {
        #[test]
        fn origin_parsing_never_panics(value in "\\PC*") {
            let _ = PageOrigin::parse_exact(&value);
            if let Ok(url) = Url::parse(&value) {
                let _ = PageOrigin::from_url(&url);
            }
        }
    }
}
