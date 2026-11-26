//! Persistent entities use ULIDs (sortable, globally unique, sync-safe).
//! Runtime-only entities (windows) use local u64.

use std::fmt;

use ulid::Ulid;

pub use crate::work::{WorkId, WorkPlanId, WorkPlanNodeId, WorkQuestionId};

macro_rules! ulid_id {
    ($name:ident) => {
        #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        #[cfg_attr(feature = "ipc-types", derive(specta::Type))]
        #[cfg_attr(feature = "ipc-types", specta(type = String))]
        pub struct $name(Ulid);

        impl $name {
            /// Minted at the shell edge; aggregates receive ids, never create them.
            pub fn generate() -> Self {
                Self(Ulid::new())
            }

            pub fn parse(s: &str) -> Option<Self> {
                Ulid::from_string(s).ok().map(Self)
            }

            pub fn bytes(self) -> [u8; 16] {
                self.0 .0.to_be_bytes()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!(stringify!($name), "({})"), self.0)
            }
        }

        impl serde::Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                serializer.serialize_str(&self.to_string())
            }
        }

        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                let value = <String as serde::Deserialize>::deserialize(deserializer)?;
                Self::parse(&value)
                    .ok_or_else(|| serde::de::Error::custom(concat!("invalid ", stringify!($name))))
            }
        }

        impl From<u128> for $name {
            fn from(n: u128) -> Self {
                Self(Ulid(n))
            }
        }
    };
}

ulid_id!(DownloadId);
ulid_id!(ClosedSessionId);
ulid_id!(ItemId);
ulid_id!(SpaceId);
ulid_id!(ContainerId);
ulid_id!(ProfileId);
ulid_id!(ExtensionInstallId);
ulid_id!(PagePermissionGrantId);
ulid_id!(ScriptId);
ulid_id!(UserscriptId);

impl ExtensionInstallId {
    /// Mints a shell-edge identity strictly above the profile's durable
    /// install-id floor.
    ///
    /// Ordinary randomized ULIDs are not monotonic within one millisecond and
    /// can also sort below a future floor after clock rollback. This allocator
    /// preserves a fresh ULID when it is already newer; otherwise it advances
    /// the durable floor by exactly one. `None` reports the only exhausted
    /// state, where the floor already occupies the complete 128-bit range.
    pub fn generate_after(high_water: Option<Self>) -> Option<Self> {
        Self::candidate_after(Self::generate(), high_water)
    }

    fn candidate_after(candidate: Self, high_water: Option<Self>) -> Option<Self> {
        let Some(high_water) = high_water else {
            return Some(candidate);
        };
        if candidate > high_water {
            return Some(candidate);
        }
        high_water.0 .0.checked_add(1).map(Self::from)
    }
}

pub type WindowId = u64;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_install_allocator_advances_same_millisecond_randomness() {
        let timestamp = 0x0123_4567_89ab_u128 << 80;
        let candidate = ExtensionInstallId::from(timestamp | 5);
        let high_water = ExtensionInstallId::from(timestamp | 7);

        assert_eq!(
            ExtensionInstallId::candidate_after(candidate, Some(high_water)),
            Some(ExtensionInstallId::from(timestamp | 8))
        );
    }

    #[test]
    fn extension_install_allocator_advances_a_future_clock_floor() {
        let candidate = ExtensionInstallId::from(0x0123_4567_89ab_u128 << 80);
        let future_floor = ExtensionInstallId::from((0x1123_4567_89ab_u128 << 80) | 99);

        assert_eq!(
            ExtensionInstallId::candidate_after(candidate, Some(future_floor)),
            Some(ExtensionInstallId::from(
                (0x1123_4567_89ab_u128 << 80) | 100
            ))
        );
    }

    #[test]
    fn extension_install_allocator_preserves_newer_freshness_and_detects_exhaustion() {
        let high_water = ExtensionInstallId::from(10);
        let fresh = ExtensionInstallId::from(20);
        assert_eq!(
            ExtensionInstallId::candidate_after(fresh, Some(high_water)),
            Some(fresh)
        );
        assert_eq!(
            ExtensionInstallId::candidate_after(
                ExtensionInstallId::from(u128::MAX - 1),
                Some(ExtensionInstallId::from(u128::MAX)),
            ),
            None
        );
    }
}

ulid_id!(ResourceId);
