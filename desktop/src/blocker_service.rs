//! Release-owned composition for authenticated blocker sources and caches.

use std::path::Path;
use std::sync::Arc;

use zephium_blocker::CompiledArtifactCacheConfig;
use zephium_blocker_service::{
    EmbeddedReleaseAsset, LicensePolicy, ManagedBlocker, ReleaseCatalogSeed, UpdateLimits,
};

const COMPILED_CACHE_DIRECTORY: &str = "blocker-compiled-v1";
const RELEASE_SEED_LICENSE: &str = "CC-BY-SA-3.0";
const RELEASE_SEED_MAX_MANIFEST_BYTES: usize = 16 * 1024;
const RELEASE_SEED_MAX_SOURCE_BYTES: u64 = 4 * 1024 * 1024;
const RELEASE_SEED_MAX_TOTAL_SOURCE_BYTES: u64 = 4 * 1024 * 1024;

const RELEASE_SEED_CATALOG: &[u8] = include_bytes!("../../assets/blocker-seed/v1/catalog.json");
const RELEASE_SEED_ENVELOPE: &[u8] =
    include_bytes!("../../assets/blocker-seed/v1/release-seed.json");
const RELEASE_SEED_EASYLIST: &[u8] = include_bytes!("../../assets/blocker-seed/v1/easylist.txt.gz");
const RELEASE_SEED_EASYPRIVACY: &[u8] =
    include_bytes!("../../assets/blocker-seed/v1/easyprivacy.txt.gz");

pub(crate) fn start_with_updates(
    data_dir: &Path,
    validate: zephium_blocker_service::NativeRuleValidator,
) -> std::io::Result<Arc<ManagedBlocker>> {
    let artifact_cache = CompiledArtifactCacheConfig::new(data_dir.join(COMPILED_CACHE_DIRECTORY))
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    ManagedBlocker::with_official_updates(
        bundled_release_seed()?,
        artifact_cache,
        data_dir.join("blocker-official-v1"),
        validate,
    )
}

#[cfg(test)]
pub(crate) fn start_seed_only(data_dir: &Path) -> std::io::Result<Arc<ManagedBlocker>> {
    let artifact_cache = CompiledArtifactCacheConfig::new(data_dir.join(COMPILED_CACHE_DIRECTORY))
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    ManagedBlocker::with_release_seed(bundled_release_seed()?, artifact_cache)
}

fn bundled_release_seed() -> std::io::Result<ReleaseCatalogSeed> {
    release_seed_from_embedded(
        RELEASE_SEED_CATALOG,
        RELEASE_SEED_ENVELOPE,
        embedded_release_assets(),
    )
}

fn release_seed_from_embedded(
    catalog: &'static [u8],
    envelope: &'static [u8],
    assets: Vec<EmbeddedReleaseAsset>,
) -> std::io::Result<ReleaseCatalogSeed> {
    let limits = UpdateLimits {
        max_manifest_bytes: RELEASE_SEED_MAX_MANIFEST_BYTES,
        max_sources: 2,
        max_source_bytes: RELEASE_SEED_MAX_SOURCE_BYTES,
        max_total_source_bytes: RELEASE_SEED_MAX_TOTAL_SOURCE_BYTES,
        ..UpdateLimits::default()
    };
    let licenses = LicensePolicy::new([RELEASE_SEED_LICENSE])
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    ReleaseCatalogSeed::from_embedded_gzip(catalog, envelope, assets, limits, licenses)
        .map_err(|error| std::io::Error::other(format!("invalid embedded blocker seed: {error}")))
}

fn embedded_release_assets() -> Vec<EmbeddedReleaseAsset> {
    vec![
        EmbeddedReleaseAsset::new("easylist.txt", RELEASE_SEED_EASYLIST),
        EmbeddedReleaseAsset::new("easyprivacy.txt", RELEASE_SEED_EASYPRIVACY),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    use zephium_core::blocker::{BlockerConfig, ContentPolicyGeneration};
    use zephium_core::ids::ProfileId;
    use zephium_core::ports::blocker::{
        BlockerCatalog, BlockerCatalogPhase, BlockerCatalogProvenance,
        BlockerCatalogRefreshDispatch, BlockerCompileOutcome, BlockerCompiler, BlockerDispatch,
        BlockerShutdownOutcome,
    };

    #[test]
    fn exact_production_release_seed_is_admitted_by_the_runtime_loader() {
        let seed = bundled_release_seed().unwrap();
        let identity = seed.identity();
        assert_eq!(identity.revision, 202_610_281_352);
        assert_eq!(identity.source_count, 2);
        assert_eq!(identity.source_bytes, 3_515_852);
        assert_eq!(
            identity.manifest_sha256,
            [
                0x8b, 0xea, 0xb1, 0x10, 0x5e, 0x15, 0xd0, 0xb1, 0xd0, 0x74, 0xc6, 0xb1, 0xcd, 0x78,
                0x69, 0x40, 0xcf, 0x76, 0xa4, 0xb9, 0xc4, 0xaf, 0x32, 0xe0, 0x07, 0xd8, 0x42, 0xb2,
                0x82, 0x2e, 0xc0, 0xa9
            ]
        );
    }

    #[test]
    fn malformed_embedded_seed_is_a_startup_error() {
        static MALFORMED_ENVELOPE: &[u8] = b"{}";
        let error = release_seed_from_embedded(
            RELEASE_SEED_CATALOG,
            MALFORMED_ENVELOPE,
            embedded_release_assets(),
        )
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "invalid embedded blocker seed: release seed envelope is invalid"
        );
    }

    #[test]
    fn release_seed_is_authoritative_but_not_network_refreshable() {
        let root = tempfile::tempdir().unwrap();
        let service = start_seed_only(root.path()).unwrap();
        let snapshot = service.maintain();

        assert_eq!(snapshot.phase, BlockerCatalogPhase::Fresh);
        assert_eq!(snapshot.package_stale, Some(false));
        assert_eq!(snapshot.package_revision, Some(202_610_281_352));
        assert_eq!(
            snapshot.package_provenance,
            Some(BlockerCatalogProvenance::ReleaseBundle)
        );
        assert_eq!(snapshot.installed_revision, snapshot.package_revision);
        assert_eq!(
            snapshot.installed_provenance,
            Some(BlockerCatalogProvenance::ReleaseBundle)
        );
        assert!(!snapshot.refresh_supported);
        assert_eq!(
            service.request_refresh(),
            BlockerCatalogRefreshDispatch::Unsupported
        );
        assert_eq!(
            service.shutdown_until(Instant::now() + Duration::from_secs(2)),
            BlockerShutdownOutcome::Clean
        );
    }

    #[test]
    fn exact_production_release_seed_materializes_through_the_worker() {
        let root = tempfile::tempdir().unwrap();
        let service = start_seed_only(root.path()).unwrap();
        let (completed, completion) = std::sync::mpsc::sync_channel(1);

        assert_eq!(
            service.compile(
                ProfileId::from(1),
                ContentPolicyGeneration::new(1).unwrap(),
                BlockerConfig { enabled: true },
                Box::new(move |outcome| completed.send(outcome).unwrap()),
            ),
            BlockerDispatch::Scheduled
        );
        let BlockerCompileOutcome::Compiled(rules) = completion
            .recv_timeout(Duration::from_secs(60))
            .expect("the exact bundled lists must compile within the bounded test deadline")
        else {
            panic!("the exact bundled lists were rejected by the production loader");
        };
        assert!(rules.enabled());
        assert!(rules.coverage().has_blocking_entries());
        assert_eq!(rules.coverage().source_rules, 131_724);
        assert_eq!(rules.coverage().accepted_rules, 103_789);
        assert_eq!(
            service.shutdown_until(Instant::now() + Duration::from_secs(10)),
            BlockerShutdownOutcome::Clean
        );
    }
}
