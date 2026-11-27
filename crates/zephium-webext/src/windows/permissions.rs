//! Bounded permission data and one-shot request bookkeeping shared by the
//! Windows adapter and host-independent tests.
use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};
use std::path::Path;
use std::time::Instant;

const MAX_REQUEST_ITEMS: usize = 16;
const MAX_PERMISSION_BYTES: usize = 1024; // 256 JS UTF-16 units
const MAX_GRANT_ITEMS: usize = 256;
const MAX_GRANT_FILE_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Permissions {
    pub permissions: Vec<String>,
    pub origins: Vec<String>,
}

impl Permissions {
    pub fn bounded_request(&self) -> bool {
        self.bounded(MAX_REQUEST_ITEMS)
    }

    fn bounded(&self, max: usize) -> bool {
        [&self.permissions, &self.origins]
            .into_iter()
            .all(|values| {
                values.len() <= max
                    && values.iter().all(|value| {
                        !value.is_empty()
                            && value.len() <= MAX_PERMISSION_BYTES
                            && value.encode_utf16().count() <= 256
                    })
            })
    }

    pub fn is_empty(&self) -> bool {
        self.permissions.is_empty() && self.origins.is_empty()
    }

    pub fn difference(&self, other: &Self) -> Self {
        Self {
            permissions: excluding(&self.permissions, &other.permissions),
            origins: excluding(&self.origins, &other.origins),
        }
    }

    pub fn declared_by(&self, declared: &Self) -> bool {
        self.permissions
            .iter()
            .all(|item| declared.permissions.contains(item))
            && self
                .origins
                .iter()
                .all(|item| declared.origins.contains(item))
    }

    /// An install never acquires unbounded state through sequential requests.
    pub fn with_granted(&self, requested: &Self) -> Option<Self> {
        let mut grants = self.clone();
        grants
            .permissions
            .extend(requested.permissions.iter().cloned());
        grants.origins.extend(requested.origins.iter().cloned());
        grants.permissions.sort_unstable();
        grants.permissions.dedup();
        grants.origins.sort_unstable();
        grants.origins.dedup();
        grants.bounded(MAX_GRANT_ITEMS).then_some(grants)
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Grants {
    pub permissions: Permissions,
    /// Password saving, address autofill, and card autofill, respectively.
    pub autofill: [Option<bool>; 3],
}

impl Grants {
    pub fn restore(path: &Path, declared: &Permissions) -> Self {
        let mut bytes = Vec::new();
        let Ok(file) = std::fs::File::open(path) else {
            return Self::default();
        };
        if file
            .take(MAX_GRANT_FILE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .is_err()
            || bytes.len() > MAX_GRANT_FILE_BYTES
        {
            return Self::default();
        }
        let Ok(mut grants) = serde_json::from_slice::<Self>(&bytes) else {
            return Self::default();
        };
        if !grants.permissions.bounded(MAX_GRANT_ITEMS) {
            return Self::default();
        }
        grants
            .permissions
            .permissions
            .retain(|item| declared.permissions.contains(item));
        grants
            .permissions
            .origins
            .retain(|item| declared.origins.contains(item));
        grants.clear_revoked_privacy();
        grants
    }

    pub fn has_privacy(&self) -> bool {
        self.permissions
            .permissions
            .iter()
            .any(|name| name == "privacy")
    }

    pub fn clear_revoked_privacy(&mut self) {
        if !self.has_privacy() {
            self.autofill = [None; 3];
        }
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        let staged = path.with_extension("tmp");
        // A process stopped mid-write may have left the previous staging file.
        match std::fs::remove_file(&staged) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let mut file = std::fs::File::options()
            .write(true)
            .create_new(true)
            .open(&staged)?;
        let result = (|| {
            serde_json::to_writer(&mut file, self)?;
            file.flush()?;
            file.sync_all()?;
            drop(file);
            std::fs::rename(&staged, path)
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(staged);
        }
        result
    }
}

fn excluding(values: &[String], removed: &[String]) -> Vec<String> {
    values
        .iter()
        .filter(|value| !removed.contains(value))
        .cloned()
        .collect()
}

pub struct PendingPermission<Owner> {
    pub owner: Owner,
    pub number: u64,
    pub reply: u64,
    pub requested: Permissions,
    pub deadline: Instant,
}

pub struct PermissionRequests<Owner> {
    number: u64,
    pending: Option<PendingPermission<Owner>>,
}

impl<Owner> Default for PermissionRequests<Owner> {
    fn default() -> Self {
        Self {
            number: 0,
            pending: None,
        }
    }
}

impl<Owner> PermissionRequests<Owner> {
    pub fn begin(
        &mut self,
        owner: Owner,
        reply: u64,
        requested: Permissions,
        deadline: Instant,
    ) -> Option<&PendingPermission<Owner>> {
        if self.pending.is_some() {
            return None;
        }
        self.number = self.number.checked_add(1)?;
        self.pending = Some(PendingPermission {
            owner,
            number: self.number,
            reply,
            requested,
            deadline,
        });
        self.pending.as_ref()
    }

    pub fn take_if(
        &mut self,
        matches: impl FnOnce(&PendingPermission<Owner>) -> bool,
    ) -> Option<PendingPermission<Owner>> {
        if !self.pending.as_ref().is_some_and(matches) {
            return None;
        }
        self.pending.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn spec(name: &str) -> Permissions {
        Permissions {
            permissions: vec![name.into()],
            origins: vec![],
        }
    }

    #[test]
    fn allow_or_deny_can_claim_a_request_only_once() {
        let mut requests = PermissionRequests::default();
        let now = Instant::now();
        let number = requests
            .begin(("profile", 1), 41, spec("privacy"), now)
            .unwrap()
            .number;
        assert!(requests
            .begin(("profile", 2), 42, spec("privacy"), now)
            .is_none());
        assert!(requests
            .take_if(|pending| pending.number == number + 1)
            .is_none());
        assert_eq!(
            requests
                .take_if(|pending| pending.number == number)
                .unwrap()
                .reply,
            41
        );
        assert!(requests
            .take_if(|pending| pending.number == number)
            .is_none());
    }

    #[test]
    fn removing_an_install_or_closing_its_profile_cancels_only_the_owner() {
        let mut requests = PermissionRequests::default();
        let now = Instant::now();
        for close_profile in [false, true] {
            requests
                .begin(("profile-a", 1), 42, spec("privacy"), now)
                .unwrap();
            assert!(requests
                .take_if(|pending| pending.owner.0 == "profile-b")
                .is_none());
            assert!(requests
                .take_if(|pending| pending.owner == ("profile-a", 2))
                .is_none());
            assert!(requests
                .take_if(|pending| {
                    if close_profile {
                        pending.owner.0 == "profile-a"
                    } else {
                        pending.owner == ("profile-a", 1)
                    }
                })
                .is_some());
            assert!(requests.take_if(|_| true).is_none());
        }
    }

    #[test]
    fn timeout_is_bound_to_the_original_request() {
        let mut requests = PermissionRequests::default();
        let start = Instant::now();
        let deadline = start + Duration::from_secs(25);
        let first = requests
            .begin(1, 41, spec("privacy"), deadline)
            .unwrap()
            .number;
        assert!(requests
            .take_if(|pending| pending.deadline <= start)
            .is_none());
        assert!(requests
            .take_if(|pending| pending.deadline <= deadline)
            .is_some());
        requests.begin(1, 42, spec("privacy"), deadline).unwrap();
        assert!(requests
            .take_if(|pending| pending.number == first)
            .is_none());
    }

    #[test]
    fn grant_state_restores_only_current_declarations_and_is_install_local() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("install-a.json");
        let privacy = spec("privacy");
        let mut grants = Grants {
            permissions: Permissions::default().with_granted(&privacy).unwrap(),
            autofill: [Some(false), None, None],
        };
        grants.save(&path).unwrap();
        assert_eq!(Grants::restore(&path, &privacy), grants);
        assert_eq!(Grants::restore(&path, &spec("tabs")), Grants::default());
        assert_eq!(
            Grants::restore(&temp.path().join("install-b.json"), &privacy),
            Grants::default()
        );
        grants.permissions = grants.permissions.difference(&privacy);
        grants.clear_revoked_privacy();
        grants.save(&path).unwrap();
        assert_eq!(Grants::restore(&path, &privacy), Grants::default());
    }

    #[test]
    fn unlisted_names_and_oversized_requests_are_refused() {
        assert!(!spec("tabs").declared_by(&spec("privacy")));
        assert!(!spec(&"x".repeat(MAX_PERMISSION_BYTES + 1)).bounded_request());
        assert!(!Permissions {
            permissions: vec!["tabs".into(); 17],
            origins: vec![]
        }
        .bounded_request());
    }
}
