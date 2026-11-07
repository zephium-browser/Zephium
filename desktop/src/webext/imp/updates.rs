//! Keeping installed extensions current with the Chrome Web Store.

use std::io::Read as _;
use std::path::Path;
use std::time::Duration;

use zephium_app::Handle;
use zephium_core::ids::{ExtensionInstallId, ProfileId};
use zephium_webext::{archive, crx, manifest::Manifest, store, ExtensionId};

use super::install::{discard, download, publish, stage, store_client, Source};
use crate::webext::{Entry, WebExtensions};

/// The first check waits so launch stays free of network work.
const FIRST_UPDATE_CHECK: Duration = Duration::from_secs(5 * 60);
const UPDATE_CHECK_INTERVAL: Duration = Duration::from_secs(5 * 60 * 60);
const MAX_UPDATE_RESPONSE_BYTES: u64 = 1024 * 1024;

/// Keeps installed extensions current with the Chrome Web Store.
pub(in crate::webext) fn start_updates(root: &Path, shell: &Handle) {
    let extensions = WebExtensions {
        root: root.to_path_buf(),
        state: std::sync::Mutex::new(None),
    };
    let shell = shell.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_UPDATE_CHECK).await;
        loop {
            if let Err(error) = check_updates(&extensions, &shell).await {
                eprintln!("extensions: update check failed: {error}");
            }
            tokio::time::sleep(UPDATE_CHECK_INTERVAL).await;
        }
    });
}

async fn check_updates(extensions: &WebExtensions, shell: &Handle) -> Result<(), String> {
    let mut installed: std::collections::BTreeMap<String, String> = Default::default();
    for profile in extensions.profiles() {
        for entry in extensions.registry(profile).extensions {
            if !entry.sideloaded {
                installed.entry(entry.id).or_insert(entry.version);
            }
        }
    }
    let entries: Vec<(ExtensionId, String)> = installed
        .iter()
        .filter_map(|(id, version)| Some((ExtensionId::parse(id)?, version.clone())))
        .collect();
    if entries.is_empty() {
        return Ok(());
    }
    let url = store::update_check_url(&entries, store::CHROME_VERSION);
    let response = store_client()?
        .get(url)
        .send()
        .await
        .map_err(|_| "The Chrome Web Store could not be reached.")?;
    if !response.status().is_success() {
        return Err(format!(
            "The Chrome Web Store answered {}.",
            response.status()
        ));
    }
    let body = super::response::read_body(
        response,
        MAX_UPDATE_RESPONSE_BYTES,
        "The update check answered too much.",
        "The update check was interrupted.",
    )
    .await?;
    for info in store::parse_update_response(&String::from_utf8_lossy(&body)) {
        let newer = info.status == "ok"
            && info.version.as_deref().is_some_and(|version| {
                installed
                    .get(info.id.as_str())
                    .is_some_and(|current| store::compare_versions(version, current).is_gt())
            });
        if newer {
            if let Err(error) = update(extensions, shell, &info).await {
                eprintln!("extensions: could not update {}: {error}", info.id);
            }
        }
    }
    Ok(())
}

/// Downloads and verifies one update, then applies it everywhere the
/// extension is installed, unless it asks for access the user never
/// granted: then it waits for approval, as in Chrome.
async fn update(
    extensions: &WebExtensions,
    shell: &Handle,
    info: &store::UpdateInfo,
) -> Result<(), String> {
    let bytes = download(&info.id).await?;
    if let Some(hash) = &info.hash_sha256 {
        if !store::matches_sha256(&bytes, hash) {
            return Err("the package doesn't match the published hash".into());
        }
    }
    let holders: Vec<(ProfileId, Entry)> = extensions
        .profiles()
        .into_iter()
        .filter_map(|profile| {
            let entry = extensions
                .registry(profile)
                .extensions
                .into_iter()
                .find(|entry| entry.id == info.id.as_str())?;
            (!entry.sideloaded).then_some((profile, entry))
        })
        .collect();
    let Some((profile, existing)) = holders.first().cloned() else {
        return Ok(());
    };
    let root = extensions.root.clone();
    let source = Source::Store(info.id.clone(), bytes);
    let baseline_holders = holders.clone();
    let (staged, needs_review) = tauri::async_runtime::spawn_blocking(move || {
        let staged = stage(&root, source, profile, &[existing])?;
        // Legacy baseline recovery reads and verifies the original package;
        // keep that filesystem and signature work off the async executor too.
        let needs_review = baseline_holders
            .iter()
            .any(|(profile, old)| requires_review(&root, *profile, old, &staged.pending.entry));
        Ok::<_, String>((staged, needs_review))
    })
    .await
    .map_err(|_| "staging failed")??;
    let pending = staged.pending;
    let fresh = &pending.entry;
    if needs_review {
        let mut changed = Vec::new();
        discard(&pending);
        let _publication = super::PUBLICATION
            .lock()
            .map_err(|_| "update publication failed")?;
        for (profile, old) in &holders {
            let mut registry = extensions.registry(*profile);
            if let Some(entry) = registry.extensions.iter_mut().find(|entry| {
                entry.id == fresh.id
                    && entry.install == old.install
                    && entry.source == old.source
                    && !entry.sideloaded
            }) {
                entry.held_update = Some(fresh.version.clone());
                changed.push((*profile, registry));
            }
        }
        save_holders(extensions, changed)?;
        return Ok(());
    }

    let publication = super::PUBLICATION
        .lock()
        .map_err(|_| "update publication failed")?;
    let packages = extensions.packages(&fresh.id);
    publish(&packages, &pending)?;
    let mut changed = Vec::new();
    for (profile, old) in &holders {
        let mut registry = extensions.registry(*profile);
        if let Some(entry) = registry.extensions.iter_mut().find(|entry| {
            entry.id == fresh.id
                && entry.install == old.install
                && entry.source == old.source
                && !entry.sideloaded
        }) {
            *entry = Entry {
                install: entry.install.clone(),
                enabled: entry.enabled,
                access: entry.access.clone(),
                pinned: entry.pinned,
                native_messaging_granted: fresh.publisher_verified
                    && entry.native_messaging_granted,
                ..fresh.clone()
            };
            changed.push((*profile, registry));
        }
    }
    let changed = save_holders(extensions, changed)?;
    drop(publication);
    for (profile, mut registry) in changed {
        extensions.apply(shell, profile, &mut registry);
    }
    Ok(())
}

fn save_holders(
    extensions: &WebExtensions,
    holders: Vec<(ProfileId, crate::webext::Registry)>,
) -> Result<Vec<(ProfileId, crate::webext::Registry)>, String> {
    let mut saved = Vec::new();
    for (profile, registry) in holders {
        if extensions.save_holder(profile, &registry)? {
            saved.push((profile, registry));
        }
    }
    Ok(saved)
}

fn requires_review(root: &Path, profile: ProfileId, old: &Entry, fresh: &Entry) -> bool {
    let Some(baseline) = permission_baseline(root, profile, old) else {
        return true;
    };
    fresh
        .reviewed_permissions
        .iter()
        .any(|name| !baseline.contains(name))
        || fresh.hosts.iter().any(|host| !old.hosts.contains(host))
}

fn permission_baseline(root: &Path, profile: ProfileId, old: &Entry) -> Option<Vec<String>> {
    if !old.reviewed_permissions.is_empty() {
        let native = zephium_webext::permissions::external_native_messaging_allowed(
            &old.reviewed_permissions,
            old.publisher_verified,
        );
        return Some(
            old.reviewed_permissions
                .iter()
                .filter(|name| name.as_str() != "nativeMessaging" || native)
                .cloned()
                .collect(),
        );
    }
    if old.sideloaded {
        return None;
    }
    let id = ExtensionId::parse(&old.id)?;
    let key = if old.source.is_empty() {
        if old.version.is_empty()
            || !old
                .version
                .bytes()
                .all(|byte| byte.is_ascii_digit() || byte == b'.')
        {
            return None;
        }
        &old.version
    } else {
        let digest = old.source.strip_prefix("crx-")?;
        if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return None;
        }
        &old.source
    };
    let path = root
        .join("packages")
        .join(id.as_str())
        .join(format!("{key}.crx"));
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(super::MAX_PACKAGE_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > super::MAX_PACKAGE_BYTES
        || (!old.source.is_empty()
            && old.source != format!("crx-{}", zephium_webext::package::bytes_digest(&bytes)))
    {
        return None;
    }
    let verified = crx::verify(&bytes, Some(&id)).ok()?;
    let staged = root
        .join("staging")
        .join(profile.to_string())
        .join(ExtensionInstallId::generate().to_string());
    let baseline = (|| {
        archive::extract(verified.zip, &staged, &archive::Limits::default()).ok()?;
        let manifest = Manifest::load(&staged).ok()?;
        if manifest.version() != Some(old.version.as_str()) {
            return None;
        }
        // Old registries mix declared grants with injected transport. Only
        // declarations present in both sources retain their baseline, including
        // native-app access explicitly requested by the verified original.
        Some(
            manifest
                .permissions()
                .into_iter()
                .filter(|name| old.permissions.contains(name))
                .collect(),
        )
    })();
    let _ = std::fs::remove_dir_all(staged);
    baseline
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deleted_update_holder_does_not_abort_other_profiles() {
        let root = tempfile::tempdir().unwrap();
        let extensions = WebExtensions {
            root: root.path().to_owned(),
            state: Default::default(),
        };
        let deleted = ProfileId::generate();
        let survivor = ProfileId::generate();
        let signed = include_bytes!("../../../tests/fixtures/webext-native.crx");
        let verified = crx::verify(signed, None).unwrap();
        let staged = stage(
            root.path(),
            Source::Store(verified.id, signed.to_vec()),
            survivor,
            &[],
        )
        .unwrap();
        let mut registry = crate::webext::Registry {
            extensions: vec![staged.pending.entry],
        };
        extensions.save(deleted, &registry).unwrap();
        extensions.save(survivor, &registry).unwrap();
        registry.extensions[0].version = "2.0".into();
        registry.extensions[0].held_update = Some("3.0".into());
        // Both registries were read for this update; deletion overtakes publication.
        std::fs::remove_file(extensions.registry_path(deleted)).unwrap();
        let updated = save_holders(
            &extensions,
            vec![(deleted, registry.clone()), (survivor, registry)],
        )
        .unwrap();
        assert_eq!(updated.len(), 1);
        assert_eq!(updated[0].0, survivor);
        let surviving = extensions.registry(survivor);
        assert_eq!(surviving.extensions[0].version, "2.0");
        assert_eq!(surviving.extensions[0].held_update.as_deref(), Some("3.0"));
        assert!(!extensions.registry_path(deleted).exists());
    }

    #[test]
    fn legacy_updates_keep_proven_ordinary_grants_without_inheriting_native_transport() {
        let root = tempfile::tempdir().unwrap();
        let signed = include_bytes!("../../../tests/fixtures/webext-native.crx");
        let verified = crx::verify(signed, None).unwrap();
        let staged = stage(
            root.path(),
            Source::Store(verified.id, signed.to_vec()),
            ProfileId::from(1),
            &[],
        )
        .unwrap();
        let fresh = staged.pending.entry.clone();
        let packages = root.path().join("packages").join(&fresh.id);
        publish(&packages, &staged.pending).unwrap();
        let mut old = fresh.clone();
        old.reviewed_permissions.clear();
        old.publisher_verified = false;
        old.permissions = vec![
            "storage".into(),
            "nativeMessaging".into(),
            "activeTab".into(),
        ];
        old.source.clear();
        let original = packages.join(format!("{}.crx", old.version));
        std::fs::write(&original, signed).unwrap();

        assert!(
            !requires_review(root.path(), ProfileId::from(1), &old, &fresh),
            "the signed original and stored grant prove ordinary storage access"
        );
        assert!(old.reviewed_permissions.is_empty() && !old.publisher_verified);
        let mut increased = fresh.clone();
        increased
            .reviewed_permissions
            .push("nativeMessaging".into());
        assert!(
            requires_review(root.path(), ProfileId::from(1), &old, &increased),
            "injected native transport is never an original native-app grant"
        );
        increased.reviewed_permissions = vec!["storage".into(), "activeTab".into()];
        assert!(
            requires_review(root.path(), ProfileId::from(1), &old, &increased),
            "the old compatibility layer also injected activeTab"
        );
        increased = fresh.clone();
        increased.hosts.push("https://new.example/*".into());
        assert!(requires_review(
            root.path(),
            ProfileId::from(1),
            &old,
            &increased
        ));
        old.permissions.retain(|name| name != "storage");
        assert!(
            requires_review(root.path(), ProfileId::from(1), &old, &fresh),
            "an original declaration alone does not prove a stored grant"
        );
        old.permissions.push("storage".into());

        std::fs::remove_file(&original).unwrap();
        assert!(requires_review(
            root.path(),
            ProfileId::from(1),
            &old,
            &fresh
        ));
        let mut corrupted = signed.to_vec();
        *corrupted.last_mut().unwrap() ^= 1;
        std::fs::write(&original, corrupted).unwrap();
        assert!(requires_review(
            root.path(),
            ProfileId::from(1),
            &old,
            &fresh
        ));
        std::fs::write(&original, signed).unwrap();
        old.version = "9.9.9".into();
        std::fs::write(packages.join("9.9.9.crx"), signed).unwrap();
        assert!(
            requires_review(root.path(), ProfileId::from(1), &old, &fresh),
            "the signed original must match the installed version"
        );
        old.version = fresh.version.clone();
        old.source = fresh.source.clone();
        assert!(!requires_review(
            root.path(),
            ProfileId::from(1),
            &old,
            &fresh
        ));
        old.source = format!("crx-{}", "0".repeat(64));
        std::fs::write(packages.join(format!("{}.crx", old.source)), signed).unwrap();
        assert!(
            requires_review(root.path(), ProfileId::from(1), &old, &fresh),
            "an existing source identity must match the actual bytes"
        );
    }

    #[test]
    fn native_update_authority_requires_a_verified_original_review() {
        let root = tempfile::tempdir().unwrap();
        let signed = include_bytes!("../../../tests/fixtures/webext-native.crx");
        let verified = crx::verify(signed, None).unwrap();
        let staged = stage(
            root.path(),
            Source::Store(verified.id, signed.to_vec()),
            ProfileId::from(1),
            &[],
        )
        .unwrap();
        let mut old = staged.pending.entry.clone();
        old.reviewed_permissions.push("nativeMessaging".into());
        let fresh = old.clone();
        old.publisher_verified = false;
        assert!(requires_review(
            root.path(),
            ProfileId::from(1),
            &old,
            &fresh
        ));
        old.publisher_verified = true;
        assert!(!requires_review(
            root.path(),
            ProfileId::from(1),
            &old,
            &fresh
        ));
        discard(&staged.pending);
    }
}
