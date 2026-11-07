//! Exercise the shipped corpus through the public worker/cache boundary.

#![cfg(any(
    all(target_os = "windows", feature = "runtime"),
    all(not(target_os = "windows"), feature = "webkit")
))]

use std::io::Read;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};
use zephium_core::blocker::DocumentStyleProvider;

use zephium_blocker::Sha256;
use zephium_blocker::{
    BlockerCompileFailure, CompiledArtifactCacheConfig, PolicyCatalog, PolicySource, SourceFormat,
    SourceId, StaticPolicyCatalog, WorkerBlocker,
};
use zephium_core::blocker::{BlockerConfig, ContentPolicyGeneration, ContentRules};
use zephium_core::ids::ProfileId;
use zephium_core::ports::blocker::{
    BlockerCompileOutcome, BlockerCompiler, BlockerDispatch, BlockerShutdownOutcome,
};

const MANIFEST: &[u8] = include_bytes!("../../../assets/blocker-seed/v1/catalog.json");
const SOURCES: [(&str, &[u8]); 9] = [
    (
        "easylist",
        include_bytes!("../../../assets/blocker-seed/v1/easylist.txt.gz"),
    ),
    (
        "easyprivacy",
        include_bytes!("../../../assets/blocker-seed/v1/easyprivacy.txt.gz"),
    ),
    (
        "ublock-filters",
        include_bytes!("../../../assets/blocker-seed/v1/ublock-filters.txt.gz"),
    ),
    (
        "ublock-quick-fixes",
        include_bytes!("../../../assets/blocker-seed/v1/ublock-quick-fixes.txt.gz"),
    ),
    (
        "ublock-link-shorteners",
        include_bytes!("../../../assets/blocker-seed/v1/ublock-link-shorteners.txt.gz"),
    ),
    (
        "ublock-privacy",
        include_bytes!("../../../assets/blocker-seed/v1/ublock-privacy.txt.gz"),
    ),
    (
        "ublock-resource-abuse",
        include_bytes!("../../../assets/blocker-seed/v1/ublock-resource-abuse.txt.gz"),
    ),
    (
        "ublock-unbreak",
        include_bytes!("../../../assets/blocker-seed/v1/ublock-unbreak.txt.gz"),
    ),
    (
        "ublock-badware",
        include_bytes!("../../../assets/blocker-seed/v1/ublock-badware.txt.gz"),
    ),
];

#[test]
fn shipped_cosmetics_are_validated_and_round_trip_independently_of_network_rules() {
    let sources: Vec<_> = SOURCES
        .iter()
        .map(|(_, bytes)| {
            let mut source = String::new();
            flate2::read::GzDecoder::new(*bytes)
                .read_to_string(&mut source)
                .unwrap();
            source
        })
        .collect();
    let policy =
        zephium_blocker::CosmeticPolicy::compile(sources.iter().map(String::as_str)).unwrap();
    assert!(policy.report().accepted > 20_000, "{:?}", policy.report());
    assert!(policy.report().generic_controls > 100);
    let bytes = policy.encode().unwrap();
    let loaded = zephium_blocker::CosmeticPolicy::decode(&bytes).unwrap();
    for url in [
        "https://example.com/",
        "https://howtogeek.com/",
        "https://amazon.com/",
    ] {
        assert_eq!(
            policy.document_plan(url).unwrap(),
            loaded.document_plan(url).unwrap()
        );
    }
    // This subscription explicitly exempts the domain from generic hiding.
    let generic = policy.document_plan("https://example.com/").unwrap();
    assert!(generic.generic_index.len() > 100_000);
    assert_eq!(
        policy
            .document_plan("https://howtogeek.com/")
            .unwrap()
            .generic_index
            .as_ref(),
        "[]"
    );
}

fn compile(worker: &WorkerBlocker, profile: u128) -> Arc<ContentRules> {
    let (send, receive) = mpsc::sync_channel(1);
    assert_eq!(
        worker.compile(
            ProfileId::from(profile),
            ContentPolicyGeneration::new(1).unwrap(),
            BlockerConfig { enabled: true },
            Box::new(move |outcome| send.send(outcome).unwrap()),
        ),
        BlockerDispatch::Scheduled
    );
    let rules = match receive.recv_timeout(Duration::from_secs(60)).unwrap() {
        BlockerCompileOutcome::Compiled(rules) => rules,
        other => {
            panic!("the release artifact must be recoverable without its source loader: {other:?}")
        }
    };
    assert!(rules.enabled());
    assert!(rules.coverage().has_blocking_entries());
    // Loading cached text runs valid_cached_webkit_rule over every rule,
    // including on macOS where the native consumer normally defers this load.
    if let zephium_core::blocker::ContentRulesPayload::Declarative { encoded, .. } = rules.payload()
    {
        assert!(encoded.load().is_some(), "cached WebKit text must validate");
    }
    rules
}

#[test]
#[cfg_attr(
    windows,
    ignore = "the Windows cache admits only a directory whose DACL grants the current user alone; hosted runner workspaces inherit broader entries"
)]
fn release_seed_recovers_after_byte_release_and_restart_without_source_loading() {
    // Avoid OS-owned /var -> /private/var symlinks: production intentionally
    // refuses symlinked cache path components.
    let root = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let identity: [u8; 32] = Sha256::digest(MANIFEST);
    let loads = Arc::new(AtomicUsize::new(0));
    let observed = loads.clone();
    let cold_catalog = PolicyCatalog::deferred_reloadable(identity, move || {
        // No second inflate can mask a cache miss in this process.
        if observed.fetch_add(1, Ordering::SeqCst) != 0 {
            return Err(BlockerCompileFailure::SourceUnavailable);
        }
        let sources = SOURCES
            .iter()
            .map(|(id, gzip)| {
                let mut text = String::new();
                flate2::read::GzDecoder::new(*gzip)
                    .read_to_string(&mut text)
                    .unwrap();
                PolicySource::new(
                    SourceId::new(*id).unwrap(),
                    SourceFormat::Standard,
                    text.into(),
                )
            })
            .collect();
        Ok(StaticPolicyCatalog::new(sources).unwrap())
    });
    let config = CompiledArtifactCacheConfig::new(root.path()).unwrap();
    let cold = WorkerBlocker::start_with_policy_catalog(cold_catalog, config.clone()).unwrap();
    let first = compile(&cold, 1);
    let digest = first.digest();
    let coverage = first.coverage();
    let style = first
        .cosmetics()
        .expect("the shipped static cosmetics must survive adaptation")
        .document_plan("https://example.com/")
        .unwrap();
    drop(first);
    let recovered = compile(&cold, 2);
    assert_eq!(recovered.digest(), digest);
    assert_eq!(recovered.coverage(), coverage);
    assert_eq!(
        recovered
            .cosmetics()
            .unwrap()
            .document_plan("https://example.com/")
            .unwrap(),
        style
    );
    drop(recovered);
    assert_eq!(loads.load(Ordering::SeqCst), 1);
    assert_eq!(
        cold.shutdown_until(Instant::now() + Duration::from_secs(5)),
        BlockerShutdownOutcome::Clean
    );
    drop(cold);

    let observed = loads.clone();
    let warm_catalog = PolicyCatalog::deferred_reloadable(identity, move || {
        observed.fetch_add(1, Ordering::SeqCst);
        Err(BlockerCompileFailure::SourceUnavailable)
    });
    let warm = WorkerBlocker::start_with_policy_catalog(warm_catalog, config).unwrap();
    let recovered = compile(&warm, 3);
    assert_eq!(recovered.digest(), digest);
    assert_eq!(recovered.coverage(), coverage);
    assert_eq!(
        recovered
            .cosmetics()
            .unwrap()
            .document_plan("https://example.com/")
            .unwrap(),
        style
    );
    assert_eq!(loads.load(Ordering::SeqCst), 1);
    assert_eq!(
        warm.shutdown_until(Instant::now() + Duration::from_secs(5)),
        BlockerShutdownOutcome::Clean
    );
}

#[cfg(all(feature = "webkit", not(target_os = "windows")))]
#[test]
fn duplicate_source_domains_survive_cache_restart_without_source_loading() {
    use zephium_core::blocker::ContentRulesPayload;

    const SOURCE: &str = concat!(
        "||ads.invalid^$script,domain=first.invalid|second.invalid|first.invalid\n",
        "||excluded.invalid^$script,domain=~first.invalid|~second.invalid|~first.invalid\n",
        "@@||allowed.invalid^$script,domain=first.invalid|second.invalid|first.invalid\n",
    );
    let root = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let identity = Sha256::digest(SOURCE.as_bytes());
    let catalog = PolicyCatalog::deferred_reloadable(identity, || {
        Ok(StaticPolicyCatalog::new(vec![PolicySource::new(
            SourceId::new("duplicate-domains").unwrap(),
            SourceFormat::Standard,
            SOURCE.into(),
        )])
        .unwrap())
    });
    let config = CompiledArtifactCacheConfig::new(root.path()).unwrap();
    let cold = WorkerBlocker::start_with_policy_catalog(catalog, config.clone()).unwrap();
    let first = compile(&cold, 1);
    let ContentRulesPayload::Declarative { encoded, .. } = first.payload() else {
        panic!("WebKit artifact expected");
    };
    let expected = encoded.load().unwrap();
    let rules: serde_json::Value = serde_json::from_str(&expected).unwrap();
    for (host, field) in [
        ("ads\\.invalid", "if-domain"),
        ("excluded\\.invalid", "unless-domain"),
        ("allowed\\.invalid", "if-domain"),
    ] {
        let mut observed = std::collections::BTreeSet::new();
        for rule in rules.as_array().unwrap().iter().filter(|rule| {
            rule["trigger"]["url-filter"]
                .as_str()
                .unwrap()
                .contains(host)
        }) {
            let domains = rule["trigger"][field].as_array().unwrap();
            let unique: std::collections::BTreeSet<_> = domains
                .iter()
                .map(|domain| domain.as_str().unwrap())
                .collect();
            assert_eq!(unique.len(), domains.len(), "duplicate {field} for {host}");
            observed.extend(unique);
        }
        assert_eq!(
            observed,
            std::collections::BTreeSet::from(["*first.invalid", "*second.invalid"]),
            "lost or broadened {field} coverage for {host}"
        );
    }
    drop(first);
    assert_eq!(
        cold.shutdown_until(Instant::now() + Duration::from_secs(5)),
        BlockerShutdownOutcome::Clean
    );
    drop(cold);

    let loads = Arc::new(AtomicUsize::new(0));
    let observed = loads.clone();
    let catalog = PolicyCatalog::deferred_reloadable(identity, move || {
        observed.fetch_add(1, Ordering::SeqCst);
        Err(BlockerCompileFailure::SourceUnavailable)
    });
    let warm = WorkerBlocker::start_with_policy_catalog(catalog, config).unwrap();
    let recovered = compile(&warm, 2);
    let ContentRulesPayload::Declarative { encoded, .. } = recovered.payload() else {
        panic!("WebKit artifact expected");
    };
    assert_eq!(encoded.load().unwrap(), expected);
    assert_eq!(loads.load(Ordering::SeqCst), 0);
    assert_eq!(
        warm.shutdown_until(Instant::now() + Duration::from_secs(5)),
        BlockerShutdownOutcome::Clean
    );
}
