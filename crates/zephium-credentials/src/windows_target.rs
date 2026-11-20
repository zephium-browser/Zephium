//! Credential Manager names, kept separate from the native calls for testing.

use crate::{isolated_identity, VaultError};

pub(super) const MAX_TARGET_UNITS: usize = 32767;
const TARGET_PREFIX: &str = "app.zephium.";
// Keep scoped names outside the legacy family accepted from callers.
const BUILD_PREFIX: &str = "app.zephium-build/";
// Leave room for the identity delimiter and the shortest enumeration filter.
const MAX_IDENTITY_BYTES: usize =
    (MAX_TARGET_UNITS - BUILD_PREFIX.len() - TARGET_PREFIX.len() - 2) / 2;

pub(super) struct TargetNamespace {
    prefix: String,
}

impl TargetNamespace {
    pub(super) fn new(identifier: &str) -> Result<Self, VaultError> {
        if identifier.is_empty()
            || identifier.len() > MAX_IDENTITY_BYTES
            || !identifier
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
        {
            return Err(VaultError::Invalid);
        }
        let prefix = match isolated_identity(identifier) {
            None => String::new(),
            Some(identity) => {
                // Encode ASCII bytes so case-insensitive native matching still
                // gives distinct identifiers distinct target namespaces.
                let encoded: String = identity.bytes().map(|byte| format!("{byte:02x}")).collect();
                format!("{BUILD_PREFIX}{encoded}/")
            }
        };
        Ok(Self { prefix })
    }

    pub(super) fn wide(&self, target: &str, filter: bool) -> Result<Vec<u16>, VaultError> {
        if !target.starts_with(TARGET_PREFIX) || target.contains('\0') || target.contains('*') {
            return Err(VaultError::Invalid);
        }
        let max_units = MAX_TARGET_UNITS - usize::from(filter);
        let mut value: Vec<u16> = self
            .prefix
            .encode_utf16()
            .chain(target.encode_utf16())
            .take(max_units + 1)
            .collect();
        if value.len() > max_units {
            return Err(VaultError::Invalid);
        }
        if filter {
            value.push(u16::from(b'*'));
        }
        value.push(0);
        Ok(value)
    }

    pub(super) fn listed_target<'a>(&self, prefix: &str, name: &'a str) -> Option<&'a str> {
        let target = name.strip_prefix(&self.prefix)?;
        target.starts_with(prefix).then_some(target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terminated(name: &str) -> Vec<u16> {
        name.encode_utf16().chain([0]).collect()
    }

    #[test]
    fn windows_targets_share_legacy_names_only_for_shipped_and_dev() {
        let target = "app.zephium.account|https://api.zephium.app";
        for identity in ["app.zephium", "app.zephium.dev"] {
            let namespace = TargetNamespace::new(identity).unwrap();
            assert_eq!(namespace.wide(target, false).unwrap(), terminated(target));
        }

        let qa = TargetNamespace::new("qa").unwrap();
        let name = qa.wide(target, false).unwrap();
        assert_eq!(
            name,
            terminated("app.zephium-build/7161/app.zephium.account|https://api.zephium.app")
        );
        for identity in ["QA", "qa-other", "zephium", "app.zephium.work-integration"] {
            let other = TargetNamespace::new(identity).unwrap();
            let other_name = other.wide(target, false).unwrap();
            assert_ne!(other_name, name);
            assert_ne!(other_name, terminated(target));
        }
    }

    #[test]
    fn windows_list_filters_by_identity_and_returns_logical_targets() {
        let prefix = "app.zephium.connection.profile.server/";
        let target = format!("{prefix}ab/manifest");
        let qa = TargetNamespace::new("qa").unwrap();
        let shipped = TargetNamespace::new("app.zephium").unwrap();
        let qa_name = format!("app.zephium-build/7161/{target}");
        assert_eq!(
            qa.wide(prefix, true).unwrap(),
            terminated(&format!("app.zephium-build/7161/{prefix}*"))
        );
        assert_eq!(
            shipped.wide(prefix, true).unwrap(),
            terminated(&format!("{prefix}*"))
        );
        assert_eq!(qa.listed_target(prefix, &qa_name), Some(target.as_str()));
        assert_eq!(
            shipped.listed_target(prefix, &target),
            Some(target.as_str())
        );
        assert_eq!(shipped.listed_target(prefix, &qa_name), None);
        for name in [
            target,
            format!("app.zephium-build/71612d6f74686572/{prefix}ab/manifest"),
            "app.zephium-build/7161/app.zephium.connection.profile.server2/ab/manifest".into(),
        ] {
            assert_eq!(qa.listed_target(prefix, &name), None);
        }
    }

    #[test]
    fn windows_target_validation_prevents_wildcards_nuls_and_external_namespaces() {
        let qa = TargetNamespace::new("qa").unwrap();
        let shipped = TargetNamespace::new("app.zephium").unwrap();
        let scoped_name = "app.zephium-build/7161/app.zephium.account";
        // A physical target cannot be supplied back as a logical target.
        assert_eq!(
            qa.wide("app.zephium.account", false).unwrap(),
            terminated(scoped_name)
        );
        for namespace in [qa, shipped] {
            for name in [
                "other.application",
                "app.zephium.a*",
                "app.zephium.a\0b",
                scoped_name,
            ] {
                assert_eq!(namespace.wide(name, false), Err(VaultError::Invalid));
                assert_eq!(namespace.wide(name, true), Err(VaultError::Invalid));
            }
        }
    }

    #[test]
    fn windows_target_limit_includes_scope_and_enumeration_wildcard() {
        let qa = TargetNamespace::new("qa").unwrap();
        let shipped = TargetNamespace::new("app.zephium").unwrap();
        let target = format!(
            "{TARGET_PREFIX}{}",
            "é".repeat(MAX_TARGET_UNITS - TARGET_PREFIX.len())
        );
        assert_eq!(
            shipped.wide(&target, false).unwrap().len(),
            MAX_TARGET_UNITS + 1
        );
        assert_eq!(
            qa.wide(&target, false).map(|name| name.len()),
            Err(VaultError::Invalid)
        );
        let target = format!(
            "{TARGET_PREFIX}{}",
            "é".repeat(MAX_TARGET_UNITS - TARGET_PREFIX.len() - "app.zephium-build/7161/".len())
        );
        assert_eq!(qa.wide(&target, false).unwrap().len(), MAX_TARGET_UNITS + 1);
        assert_eq!(qa.wide(&target, true), Err(VaultError::Invalid));
    }

    #[test]
    fn windows_rejects_missing_or_malformed_identity() {
        for identity in ["", "app.zephium*", "app.zephium\0", "app.zephium/qa"] {
            assert!(matches!(
                TargetNamespace::new(identity),
                Err(VaultError::Invalid)
            ));
        }
    }
}
