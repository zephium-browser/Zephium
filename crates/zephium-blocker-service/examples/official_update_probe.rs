//! Nonshipping, explicit live-source/native-compiler qualification tool.
//! Run with: ROOT_DIRECTORY NATIVE_VALIDATOR_EXECUTABLE (macOS validator must
//! exit successfully only after a real WKContentRuleListStore compilation).
use std::sync::Arc;
use std::time::{Duration, Instant};
use zephium_blocker::CompiledArtifactCacheConfig;
use zephium_blocker_service::{
    EmbeddedReleaseAsset, LicensePolicy, ManagedBlocker, ReleaseCatalogSeed, UpdateLimits,
};
use zephium_core::blocker::{ContentRuleApplyFailure, ContentRulesPayload};
use zephium_core::ports::blocker::{
    BlockerCatalog, BlockerCatalogPhase, BlockerCatalogProvenance, BlockerCompiler,
};
use zephium_core::ports::engine::ContentRuleValidationOutcome;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let root = std::path::PathBuf::from(args.next().ok_or("ROOT_DIRECTORY required")?);
    let verifier =
        std::path::PathBuf::from(args.next().ok_or("NATIVE_VALIDATOR_EXECUTABLE required")?);
    if !root.is_absolute() || !verifier.is_absolute() || args.next().is_some() {
        return Err("absolute paths required".into());
    }
    std::fs::create_dir_all(&root)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))?;
    }
    let seed = ReleaseCatalogSeed::from_embedded_gzip(
        include_bytes!("../../../assets/blocker-seed/v1/catalog.json"),
        include_bytes!("../../../assets/blocker-seed/v1/release-seed.json"),
        vec![
            EmbeddedReleaseAsset::new(
                "easylist.txt",
                include_bytes!("../../../assets/blocker-seed/v1/easylist.txt.gz"),
            ),
            EmbeddedReleaseAsset::new(
                "easyprivacy.txt",
                include_bytes!("../../../assets/blocker-seed/v1/easyprivacy.txt.gz"),
            ),
            EmbeddedReleaseAsset::new(
                "ublock-filters.txt",
                include_bytes!("../../../assets/blocker-seed/v1/ublock-filters.txt.gz"),
            ),
            EmbeddedReleaseAsset::new(
                "ublock-quick-fixes.txt",
                include_bytes!("../../../assets/blocker-seed/v1/ublock-quick-fixes.txt.gz"),
            ),
            EmbeddedReleaseAsset::new(
                "ublock-link-shorteners.txt",
                include_bytes!("../../../assets/blocker-seed/v1/ublock-link-shorteners.txt.gz"),
            ),
            EmbeddedReleaseAsset::new(
                "ublock-privacy.txt",
                include_bytes!("../../../assets/blocker-seed/v1/ublock-privacy.txt.gz"),
            ),
            EmbeddedReleaseAsset::new(
                "ublock-resource-abuse.txt",
                include_bytes!("../../../assets/blocker-seed/v1/ublock-resource-abuse.txt.gz"),
            ),
            EmbeddedReleaseAsset::new(
                "ublock-unbreak.txt",
                include_bytes!("../../../assets/blocker-seed/v1/ublock-unbreak.txt.gz"),
            ),
            EmbeddedReleaseAsset::new(
                "ublock-badware.txt",
                include_bytes!("../../../assets/blocker-seed/v1/ublock-badware.txt.gz"),
            ),
        ],
        UpdateLimits::default(),
        LicensePolicy::new(["CC-BY-SA-3.0", "GPL-3.0-only"])?,
    )?;
    let artifacts = root.clone();
    let validator: zephium_blocker_service::NativeRuleValidator =
        Arc::new(move |rules, completion| {
            let verifier = verifier.clone();
            let root = artifacts.clone();
            std::thread::spawn(move || {
                let result = (|| -> std::io::Result<bool> {
                    let ContentRulesPayload::Declarative { encoded, .. } = rules.payload() else {
                        return Ok(false);
                    };
                    let Some(encoded) = encoded.load() else {
                        return Ok(false);
                    };
                    let path = root.join("native-network.json");
                    std::fs::write(&path, encoded.as_bytes())?;
                    if !std::process::Command::new(&verifier)
                        .arg(&path)
                        .status()?
                        .success()
                    {
                        return Ok(false);
                    }
                    Ok(true)
                })();
                completion.finish(if matches!(result, Ok(true)) {
                    ContentRuleValidationOutcome::Valid
                } else {
                    ContentRuleValidationOutcome::Rejected(
                        ContentRuleApplyFailure::NativeCompilation,
                    )
                });
            });
        });
    let service = ManagedBlocker::with_official_updates(
        seed,
        CompiledArtifactCacheConfig::new(root.join("compiled"))?,
        root.join("official"),
        validator,
    )?;
    println!("refresh_admission={:?}", service.request_refresh());
    let started = Instant::now();
    let mut revision = 0;
    let outcome = loop {
        let status = service.maintain();
        if status.revision != revision {
            revision = status.revision;
            println!(
                "elapsed={:.3} phase={:?} candidate={} installed={:?} provenance={:?}",
                started.elapsed().as_secs_f64(),
                status.phase,
                status.activation_pending,
                status.installed_revision,
                status.installed_provenance
            );
        }
        if status.installed_provenance == Some(BlockerCatalogProvenance::OfficialHttps)
            && !status.activation_pending
            && matches!(status.phase, BlockerCatalogPhase::Fresh)
        {
            break true;
        }
        if matches!(
            status.phase,
            BlockerCatalogPhase::Failed(_)
                | BlockerCatalogPhase::Unavailable(_)
                | BlockerCatalogPhase::Shutdown
        ) || started.elapsed() > Duration::from_secs(240)
        {
            break false;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let shutdown = service.shutdown_until(Instant::now() + Duration::from_secs(5));
    println!("shutdown={shutdown:?}");
    if !outcome {
        return Err("source update or native validation failed".into());
    }
    Ok(())
}
