use std::collections::HashSet;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use thiserror::Error;
use tough::{
    ExpirationEnforcement, IntoVec, Limits, Repository, RepositoryLoader, TargetName, Transport,
};
use zephium_blocker::PolicyCatalog;
use zephium_blocker::Sha256;

use crate::manifest::{decode_sha256, CatalogManifest, ManifestError, CATALOG_MANIFEST_TARGET};
use crate::storage::{seal_tuf_stage, CatalogStore, PreparedTufDatastore};
#[cfg(test)]
use crate::transport::FixedFilesystemTransport;
use crate::transport::FixedOriginTransport;
use crate::types::{ActivatedCatalog, CatalogIdentity, FailureKind, RepositoryConfig};

#[derive(Clone, Debug)]
pub(crate) struct VerifiedCatalog {
    pub(crate) repository_identity: [u8; 32],
    pub(crate) tuf_descriptor_sha256: [u8; 32],
    pub(crate) verified_at_unix: u64,
    pub(crate) manifest: CatalogManifest,
    pub(crate) payload: VerifiedCatalogPayload,
}

#[derive(Clone, Debug)]
pub(crate) enum VerifiedCatalogPayload {
    Candidate {
        activated: ActivatedCatalog,
        source_contents: Vec<Arc<str>>,
    },
    CandidateRepair {
        identity: CatalogIdentity,
        source_contents: Vec<Arc<str>>,
    },
    Unchanged {
        identity: CatalogIdentity,
        repaired_objects: Vec<([u8; 32], Vec<u8>)>,
    },
    RejectedUnchanged(CatalogIdentity),
}

impl VerifiedCatalog {
    pub(crate) fn identity(&self) -> &CatalogIdentity {
        match &self.payload {
            VerifiedCatalogPayload::Candidate { activated, .. } => &activated.identity,
            VerifiedCatalogPayload::CandidateRepair { identity, .. } => identity,
            VerifiedCatalogPayload::Unchanged { identity, .. }
            | VerifiedCatalogPayload::RejectedUnchanged(identity) => identity,
        }
    }
}

pub(crate) async fn verify_repository(
    config: &RepositoryConfig,
    prepared: &PreparedTufDatastore,
    store: &CatalogStore,
) -> Result<VerifiedCatalog, VerificationError> {
    verify_repository_with_time(config, prepared, store, None, None).await
}

pub(crate) async fn verify_candidate_repair(
    config: &RepositoryConfig,
    prepared: &PreparedTufDatastore,
    store: &CatalogStore,
    expected: &CatalogIdentity,
) -> Result<VerifiedCatalog, VerificationError> {
    verify_repository_with_time(config, prepared, store, None, Some(expected)).await
}

async fn verify_repository_with_time(
    config: &RepositoryConfig,
    prepared: &PreparedTufDatastore,
    store: &CatalogStore,
    now_override: Option<u64>,
    repair_candidate: Option<&CatalogIdentity>,
) -> Result<VerifiedCatalog, VerificationError> {
    if prepared.repository_identity != config.repository_identity {
        return Err(VerificationError::Metadata);
    }
    let transport: Box<dyn Transport + Send + Sync> =
        if config.metadata_base_url.scheme() == "https" {
            Box::new(
                FixedOriginTransport::new(
                    config.metadata_base_url.clone(),
                    config.targets_base_url.clone(),
                    config.limits.request_timeout,
                    config.limits.connect_timeout,
                    "Zephium-Filter-Updater/1",
                )
                .map_err(|_| VerificationError::Transport)?,
            )
        } else {
            #[cfg(test)]
            {
                Box::new(FixedFilesystemTransport::new(
                    config.metadata_base_url.clone(),
                    config.targets_base_url.clone(),
                ))
            }
            #[cfg(not(test))]
            {
                return Err(VerificationError::Transport);
            }
        };

    let limits = Limits {
        max_root_size: config.limits.max_root_bytes,
        max_targets_size: config.limits.max_targets_metadata_bytes,
        max_timestamp_size: config.limits.max_timestamp_metadata_bytes,
        max_snapshot_size: config.limits.max_snapshot_metadata_bytes,
        max_root_updates: config.limits.max_root_updates,
    };
    let repository = RepositoryLoader::new(
        &prepared.trusted_root,
        config.metadata_base_url.clone(),
        config.targets_base_url.clone(),
    )
    .transport(DynamicTransport(transport))
    .limits(limits)
    .datastore(&prepared.path)
    .expiration_enforcement(ExpirationEnforcement::Safe)
    .load()
    .await
    .map_err(|_| VerificationError::Metadata)?;

    let mut verified =
        verify_loaded_repository(&repository, config, store, now_override, repair_candidate)
            .await?;
    // Tough advances latest_known_time.json on each Safe-mode target read.
    // Seal only after every target is fully consumed.
    verified.tuf_descriptor_sha256 =
        seal_tuf_stage(&prepared.path, config).map_err(|_| VerificationError::Metadata)?;
    Ok(verified)
}

async fn verify_loaded_repository(
    repository: &Repository,
    config: &RepositoryConfig,
    store: &CatalogStore,
    now_override: Option<u64>,
    repair_candidate: Option<&CatalogIdentity>,
) -> Result<VerifiedCatalog, VerificationError> {
    if repository
        .targets()
        .signed
        .delegations
        .as_ref()
        .is_some_and(|delegations| !delegations.keys.is_empty() || !delegations.roles.is_empty())
    {
        return Err(VerificationError::Metadata);
    }
    let manifest_name =
        exact_target_name(CATALOG_MANIFEST_TARGET).ok_or(VerificationError::Manifest)?;
    let manifest_metadata = repository
        .all_targets()
        .find_map(|(name, target)| (name == &manifest_name).then_some(target))
        .ok_or(VerificationError::ManifestMissing)?;
    if manifest_metadata.length == 0
        || manifest_metadata.length > config.limits.max_manifest_bytes as u64
    {
        return Err(VerificationError::Manifest);
    }

    let manifest_digest: [u8; 32] = manifest_metadata
        .hashes
        .sha256
        .as_ref()
        .try_into()
        .map_err(|_| VerificationError::Metadata)?;
    let manifest_bytes = read_target_cached(
        repository,
        &manifest_name,
        &manifest_digest,
        manifest_metadata.length,
        store,
    )
    .await?
    .ok_or(VerificationError::ManifestMissing)?;
    if manifest_bytes.len() as u64 != manifest_metadata.length
        || Sha256::digest(&manifest_bytes).as_slice() != manifest_digest
    {
        return Err(VerificationError::Manifest);
    }
    let verification_started_unix = match now_override {
        Some(now) => now,
        None => SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| VerificationError::Clock)?
            .as_secs(),
    };
    let manifest = CatalogManifest::parse_canonical(
        &manifest_bytes,
        verification_started_unix,
        config.limits,
        &config.license_policy,
    )
    .map_err(|error| match error {
        ManifestError::License => VerificationError::License,
        _ => VerificationError::Manifest,
    })?;

    let declared_targets: HashSet<&str> = std::iter::once(CATALOG_MANIFEST_TARGET)
        .chain(manifest.sources.iter().map(|source| source.target.as_str()))
        .collect();
    let repository_targets: HashSet<&str> = repository
        .all_targets()
        .map(|(name, _)| name.raw())
        .collect();
    if repository_targets != declared_targets {
        return Err(VerificationError::TargetSet);
    }

    let mut total_bytes = 0u64;
    for source in &manifest.sources {
        let target_name = exact_target_name(&source.target).ok_or(VerificationError::Target)?;
        let metadata = repository
            .all_targets()
            .find_map(|(name, target)| (name == &target_name).then_some(target))
            .ok_or(VerificationError::Target)?;
        let expected_digest =
            decode_sha256(&source.sha256).map_err(|_| VerificationError::Target)?;
        if metadata.length != source.length
            || metadata.hashes.sha256.as_ref() != expected_digest.as_slice()
        {
            return Err(VerificationError::TargetMetadata);
        }
        total_bytes = total_bytes
            .checked_add(source.length)
            .ok_or(VerificationError::Target)?;
        if total_bytes > config.limits.max_total_source_bytes {
            return Err(VerificationError::Target);
        }
    }

    let verified_at_unix = match now_override {
        Some(now) => now,
        None => SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| VerificationError::Clock)?
            .as_secs(),
    };
    if verified_at_unix < verification_started_unix {
        return Err(VerificationError::Clock);
    }
    if manifest.expires_unix <= verified_at_unix {
        return Err(VerificationError::Manifest);
    }
    let manifest_sha256: [u8; 32] = Sha256::digest(&manifest_bytes);
    let source_count =
        u32::try_from(manifest.sources.len()).map_err(|_| VerificationError::Catalog)?;
    let identity = CatalogIdentity {
        revision: manifest.revision,
        manifest_sha256,
        created_unix: manifest.created_unix,
        expires_unix: manifest.expires_unix,
        source_count,
        source_bytes: total_bytes,
    };
    let payload = match store
        .classify_identity(&identity)
        .map_err(|error| match error {
            crate::storage::StoreError::Rollback | crate::storage::StoreError::Equivocation => {
                VerificationError::Rollback
            }
            _ => VerificationError::Storage,
        })? {
        crate::storage::CatalogIdentityDisposition::Current => {
            if repair_candidate.is_some() {
                return Err(VerificationError::CandidateMismatch);
            }
            let mut repaired_objects = Vec::new();
            for source in &manifest.sources {
                let digest =
                    decode_sha256(&source.sha256).map_err(|_| VerificationError::Target)?;
                if store
                    .validate_cached_object(&digest, source.length)
                    .map_err(|_| VerificationError::Storage)?
                    == crate::storage::CachedObjectState::Present
                {
                    continue;
                }
                let target_name =
                    exact_target_name(&source.target).ok_or(VerificationError::Target)?;
                let bytes = read_target(repository, &target_name)
                    .await?
                    .ok_or(VerificationError::Target)?;
                if bytes.len() as u64 != source.length
                    || Sha256::digest(&bytes).as_slice() != digest
                {
                    return Err(VerificationError::TargetMetadata);
                }
                repaired_objects.push((digest, bytes));
            }
            VerifiedCatalogPayload::Unchanged {
                identity,
                repaired_objects,
            }
        }
        crate::storage::CatalogIdentityDisposition::New => {
            let mut source_contents = Vec::with_capacity(manifest.sources.len());
            for source in &manifest.sources {
                let target_name =
                    exact_target_name(&source.target).ok_or(VerificationError::Target)?;
                let expected_digest =
                    decode_sha256(&source.sha256).map_err(|_| VerificationError::Target)?;
                let bytes = read_target_cached(
                    repository,
                    &target_name,
                    &expected_digest,
                    source.length,
                    store,
                )
                .await?
                .ok_or(VerificationError::Target)?;
                if bytes.len() as u64 != source.length
                    || Sha256::digest(&bytes).as_slice() != expected_digest
                {
                    return Err(VerificationError::TargetMetadata);
                }
                let contents = String::from_utf8(bytes).map_err(|_| VerificationError::NotUtf8)?;
                source_contents.push(Arc::<str>::from(contents));
            }
            let catalog = manifest
                .build_verified_catalog(&source_contents)
                .map_err(|_| VerificationError::Catalog)?;
            VerifiedCatalogPayload::Candidate {
                activated: ActivatedCatalog {
                    identity,
                    catalog: PolicyCatalog::authenticated(manifest_sha256, catalog),
                },
                source_contents,
            }
        }
        crate::storage::CatalogIdentityDisposition::Rejected => {
            if repair_candidate.is_some() {
                return Err(VerificationError::CandidateMismatch);
            }
            VerifiedCatalogPayload::RejectedUnchanged(identity)
        }
        crate::storage::CatalogIdentityDisposition::Candidate => {
            if repair_candidate != Some(&identity) {
                return Err(VerificationError::CandidateMismatch);
            }
            let mut source_contents = Vec::with_capacity(manifest.sources.len());
            for source in &manifest.sources {
                let target_name =
                    exact_target_name(&source.target).ok_or(VerificationError::Target)?;
                let expected_digest =
                    decode_sha256(&source.sha256).map_err(|_| VerificationError::Target)?;
                // Repair is deliberately a one-shot full package read. It
                // does not trust any candidate source object which survived
                // locally alongside the object that triggered repair.
                let bytes = read_target(repository, &target_name)
                    .await?
                    .ok_or(VerificationError::Target)?;
                if bytes.len() as u64 != source.length
                    || Sha256::digest(&bytes).as_slice() != expected_digest
                {
                    return Err(VerificationError::TargetMetadata);
                }
                let contents = String::from_utf8(bytes).map_err(|_| VerificationError::NotUtf8)?;
                source_contents.push(Arc::<str>::from(contents));
            }
            VerifiedCatalogPayload::CandidateRepair {
                identity,
                source_contents,
            }
        }
    };
    Ok(VerifiedCatalog {
        repository_identity: config.repository_identity,
        tuf_descriptor_sha256: [0; 32],
        verified_at_unix,
        manifest,
        payload,
    })
}

#[cfg(test)]
pub(crate) async fn verify_repository_at(
    config: &RepositoryConfig,
    prepared: &PreparedTufDatastore,
    store: &CatalogStore,
    now_unix: u64,
) -> Result<VerifiedCatalog, VerificationError> {
    verify_repository_with_time(config, prepared, store, Some(now_unix), None).await
}

#[cfg(test)]
pub(crate) async fn verify_candidate_repair_at(
    config: &RepositoryConfig,
    prepared: &PreparedTufDatastore,
    store: &CatalogStore,
    expected: &CatalogIdentity,
    now_unix: u64,
) -> Result<VerifiedCatalog, VerificationError> {
    verify_repository_with_time(config, prepared, store, Some(now_unix), Some(expected)).await
}

fn exact_target_name(value: &str) -> Option<TargetName> {
    let name = TargetName::new(value).ok()?;
    (name.raw() == value && name.resolved() == value).then_some(name)
}

async fn read_target_cached(
    repository: &Repository,
    name: &TargetName,
    digest: &[u8; 32],
    exact_bytes: u64,
    store: &CatalogStore,
) -> Result<Option<Vec<u8>>, VerificationError> {
    match store
        .validate_cached_object(digest, exact_bytes)
        .map_err(|_| VerificationError::Storage)?
    {
        crate::storage::CachedObjectState::Present => {
            let bytes = store
                .load_cached_object(digest, exact_bytes)
                .map_err(|_| VerificationError::Storage)?
                .ok_or(VerificationError::Storage)?;
            if bytes.len() as u64 != exact_bytes {
                return Err(VerificationError::TargetMetadata);
            }
            return Ok(Some(bytes));
        }
        crate::storage::CachedObjectState::Missing | crate::storage::CachedObjectState::Corrupt => {
        }
    }
    read_target(repository, name).await
}

async fn read_target(
    repository: &Repository,
    name: &TargetName,
) -> Result<Option<Vec<u8>>, VerificationError> {
    let Some(stream) = repository
        .read_target(name)
        .await
        .map_err(|_| VerificationError::Target)?
    else {
        return Ok(None);
    };
    stream
        .into_vec()
        .await
        .map(Some)
        .map_err(|_| VerificationError::Target)
}

#[derive(Clone)]
struct DynamicTransport(Box<dyn Transport + Send + Sync>);

impl std::fmt::Debug for DynamicTransport {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("DynamicTransport(..)")
    }
}

#[async_trait::async_trait]
impl Transport for DynamicTransport {
    async fn fetch(&self, url: url::Url) -> Result<tough::TransportStream, tough::TransportError> {
        self.0.fetch(url).await
    }
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub(crate) enum VerificationError {
    #[error("fixed-origin transport failed")]
    Transport,
    #[error("TUF metadata verification failed")]
    Metadata,
    #[error("catalog manifest target is absent")]
    ManifestMissing,
    #[error("catalog manifest is invalid")]
    Manifest,
    #[error("catalog license metadata is invalid")]
    License,
    #[error("repository target set differs from the catalog")]
    TargetSet,
    #[error("catalog target is invalid")]
    Target,
    #[error("catalog target metadata differs from the manifest")]
    TargetMetadata,
    #[error("catalog target is not UTF-8")]
    NotUtf8,
    #[error("verified sources exceed the compiler boundary")]
    Catalog,
    #[error("verified repository no longer names the exact repair candidate")]
    CandidateMismatch,
    #[error("system clock predates the Unix epoch")]
    Clock,
    #[error("verified local package cache is unavailable")]
    Storage,
    #[error("signed package rollback or equivocation was detected")]
    Rollback,
}

impl VerificationError {
    pub(crate) const fn kind(self) -> FailureKind {
        match self {
            Self::Transport => FailureKind::Transport,
            Self::Metadata => FailureKind::Metadata,
            Self::ManifestMissing | Self::Manifest => FailureKind::Manifest,
            Self::Clock => FailureKind::Clock,
            Self::Storage => FailureKind::Storage,
            Self::Rollback => FailureKind::Rollback,
            Self::License => FailureKind::License,
            Self::TargetSet | Self::Target | Self::TargetMetadata | Self::NotUtf8 => {
                FailureKind::Target
            }
            Self::Catalog | Self::CandidateMismatch => FailureKind::Catalog,
        }
    }
}
