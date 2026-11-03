//! Windows WebView2 runtime-generation ownership and crash reclamation.
//!
//! A successful filesystem delete is not process-exit proof. Every fresh
//! generation receives a durable, bounded ownership marker and a matching
//! volatile HKCU registry subkey. The registry subkey survives process death
//! but not a Windows reboot, so marked data is reclaimed only after either an
//! exact native process-exit proof or an intervening boot boundary.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use winreg::enums::{
    HKEY_CURRENT_USER, KEY_CREATE_SUB_KEY, KEY_READ, REG_CREATED_NEW_KEY, REG_OPTION_VOLATILE,
};
use winreg::RegKey;

use crate::ids::ProfileId;

const REGISTRY_PARENT: &str = r"Software\Zephium";
const LEASE_FILE: &str = ".zephium-lifecycle.lock";
const GENERATION_MARKER: &str = ".zephium-webview2-generation-v1";
const MARKER_PREFIX: &str = "zephium-webview2-runtime-generation-v1\n";
const MAX_MARKER_BYTES: u64 = 128;

// These runtime roots contain private/ephemeral engine state, not an ordinary
// persistent website profile. The budgets bound crash-loop startup work and
// disk consumption. Hitting a same-boot bound is recoverable by fully
// restarting Windows, after which valid marked generations become reclaimable.
const MAX_SAME_BOOT_GENERATIONS: usize = 32;
const MAX_TOTAL_GENERATIONS: usize = 128;
const MAX_QUARANTINE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_SCANNED_ENTRIES: usize = 250_000;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeGenerationKind {
    RawPrivate,
    Privileged,
}

impl RuntimeGenerationKind {
    fn tag(self) -> &'static str {
        match self {
            Self::RawPrivate => "raw-private",
            Self::Privileged => "privileged",
        }
    }

    fn registry_boot_key(self) -> &'static str {
        match self {
            Self::RawPrivate => "RawPrivateRuntimeGenerationsV1",
            Self::Privileged => "PrivilegedRuntimeGenerationsV1",
        }
    }
}

pub struct RuntimeGeneration {
    root: PathBuf,
    generation: String,
    kind: RuntimeGenerationKind,
    // Deny all sharing modes. This independently prevents two Zephium parent
    // processes from classifying/removing one root concurrently while the
    // outer single-instance plugin is still settling.
    _lease: File,
}

#[derive(Clone, Debug)]
pub struct RuntimeCleanupTicket {
    root: PathBuf,
    generation: String,
    kind: RuntimeGenerationKind,
}

#[derive(Default)]
struct QuarantineUsage {
    generations: usize,
    same_boot_generations: usize,
    entries: usize,
    bytes: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MarkerStatus {
    Missing,
    Matches,
    Invalid,
}

fn is_reparse(metadata: &fs::Metadata) -> bool {
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

fn direct_directory(path: &Path) -> io::Result<PathBuf> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() || is_reparse(&metadata) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("runtime path is not a direct directory: {}", path.display()),
        ));
    }
    super::canonical_user_data_directory(path)
}

fn open_boot_registry(kind: RuntimeGenerationKind) -> io::Result<RegKey> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (parent, _) =
        hkcu.create_subkey_with_flags(REGISTRY_PARENT, KEY_READ | KEY_CREATE_SUB_KEY)?;
    let (boot, _) = parent.create_subkey_with_options_flags(
        kind.registry_boot_key(),
        REG_OPTION_VOLATILE,
        KEY_READ | KEY_CREATE_SUB_KEY,
    )?;
    Ok(boot)
}

fn current_boot_contains(boot: &RegKey, generation: &str) -> io::Result<bool> {
    match boot.open_subkey_with_flags(generation, KEY_READ) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn mark_current_boot(boot: &RegKey, generation: &str) -> io::Result<()> {
    let (_, disposition) =
        boot.create_subkey_with_options_flags(generation, REG_OPTION_VOLATILE, KEY_READ)?;
    if disposition != REG_CREATED_NEW_KEY {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "WebView2 generation already has a current-boot provenance key",
        ));
    }
    Ok(())
}

fn ensure_current_boot(boot: &RegKey, generation: &str) -> io::Result<()> {
    if current_boot_contains(boot, generation)? {
        Ok(())
    } else {
        mark_current_boot(boot, generation)
    }
}

fn marker_bytes(kind: RuntimeGenerationKind, generation: &str) -> Vec<u8> {
    format!("{MARKER_PREFIX}{}\n{generation}\n", kind.tag()).into_bytes()
}

fn write_generation_marker(
    root: &Path,
    kind: RuntimeGenerationKind,
    generation: &str,
) -> io::Result<()> {
    let path = root.join(GENERATION_MARKER);
    let mut marker = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    marker.write_all(&marker_bytes(kind, generation))?;
    marker.sync_all()?;
    let metadata = marker.metadata()?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || is_reparse(&metadata)
        || metadata.len() > MAX_MARKER_BYTES
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "WebView2 generation marker is not a bounded direct file",
        ));
    }
    Ok(())
}

fn marker_status(
    root: &Path,
    kind: RuntimeGenerationKind,
    generation: &str,
) -> io::Result<MarkerStatus> {
    let path = root.join(GENERATION_MARKER);
    let link_metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(MarkerStatus::Missing);
        }
        Err(error) => return Err(error),
    };
    if !link_metadata.is_file()
        || link_metadata.file_type().is_symlink()
        || is_reparse(&link_metadata)
        || link_metadata.len() > MAX_MARKER_BYTES
    {
        return Ok(MarkerStatus::Invalid);
    }
    let marker = File::open(path)?;
    let metadata = marker.metadata()?;
    if !metadata.is_file() || is_reparse(&metadata) || metadata.len() > MAX_MARKER_BYTES {
        return Ok(MarkerStatus::Invalid);
    }
    let mut bytes = Vec::with_capacity(MAX_MARKER_BYTES as usize + 1);
    marker.take(MAX_MARKER_BYTES + 1).read_to_end(&mut bytes)?;
    Ok(if bytes == marker_bytes(kind, generation) {
        MarkerStatus::Matches
    } else {
        MarkerStatus::Invalid
    })
}

fn measure_tree(
    root: &Path,
    usage: &mut QuarantineUsage,
    enforce_byte_budget: bool,
) -> io::Result<()> {
    let mut pending = vec![root.to_owned()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            usage.entries = usage
                .entries
                .checked_add(1)
                .ok_or_else(|| io::Error::other("WebView2 quarantine entry counter overflowed"))?;
            if usage.entries > MAX_SCANNED_ENTRIES {
                return Err(remediation_error(
                    "the quarantine contains too many filesystem entries to inspect safely",
                ));
            }
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.file_type().is_symlink() || is_reparse(&metadata) {
                return Err(remediation_error(&format!(
                    "the quarantine contains a redirecting filesystem object at {}",
                    path.display()
                )));
            }
            if metadata.is_dir() {
                pending.push(path);
            } else if metadata.is_file() {
                usage.bytes = usage
                    .bytes
                    .checked_add(metadata.len())
                    .ok_or_else(|| remediation_error("the quarantine byte counter overflowed"))?;
                if enforce_byte_budget && usage.bytes > MAX_QUARANTINE_BYTES {
                    return Err(remediation_error(
                        "the quarantine exceeds its 2 GiB startup disk budget",
                    ));
                }
            } else {
                return Err(remediation_error(&format!(
                    "the quarantine contains an unsupported filesystem object at {}",
                    path.display()
                )));
            }
        }
    }
    Ok(())
}

fn remediation_error(reason: &str) -> io::Error {
    io::Error::other(format!(
        "{reason}. Fully restart Windows and start Zephium again so prior-boot WebView2 generations can be reclaimed. If this persists, keep Zephium closed and move the affected runtime directory aside for manual recovery"
    ))
}

fn reclaim_generation(
    root: &Path,
    kind: RuntimeGenerationKind,
    generation: &str,
) -> io::Result<()> {
    reclaim_generation_with_scan(root, kind, generation, |root, usage| {
        measure_tree(root, usage, false)
    })
}

fn reclaim_generation_with_scan(
    root: &Path,
    kind: RuntimeGenerationKind,
    generation: &str,
    mut scan: impl FnMut(&Path, &mut QuarantineUsage) -> io::Result<()>,
) -> io::Result<()> {
    let mut last_error = None;
    for _ in 0..8 {
        let canonical = match direct_directory(root) {
            Ok(canonical) if canonical == root => canonical,
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "WebView2 generation identity changed before deletion",
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        if marker_status(&canonical, kind, generation)? != MarkerStatus::Matches {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "WebView2 generation lost its exact ownership marker before deletion",
            ));
        }
        // Reject every known reparse point immediately before recursive
        // deletion. Same-UID path replacement remains a documented residual
        // until descriptor-relative Windows tree deletion is implemented.
        let mut measured = QuarantineUsage::default();
        // An exact-exit or prior-boot generation is already authorized for
        // removal. Its size must not make the cleanup path self-defeating;
        // still bound entry traversal and reject every reparse point.
        match scan(&canonical, &mut measured) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                // A concurrent exact-process-gated profile erasure can remove
                // a child queued by this scan. Partial absence is not cleanup
                // proof: retry the whole original identity, marker and tree
                // validation within the existing removal attempt bound.
                last_error = Some(error);
                std::thread::sleep(std::time::Duration::from_millis(50));
                continue;
            }
            Err(error) => return Err(error),
        }
        match fs::remove_dir_all(&canonical) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => last_error = Some(error),
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    Err(last_error
        .unwrap_or_else(|| io::Error::other("WebView2 generation deletion did not complete")))
}

fn maintain_generations(
    generations: &Path,
    boot: &RegKey,
    kind: RuntimeGenerationKind,
) -> io::Result<()> {
    let mut usage = QuarantineUsage::default();
    let mut top_level_entries = 0_usize;
    for entry in fs::read_dir(generations)? {
        let entry = entry?;
        top_level_entries = top_level_entries
            .checked_add(1)
            .ok_or_else(|| remediation_error("the generation counter overflowed"))?;
        if top_level_entries > MAX_TOTAL_GENERATIONS + 1 {
            return Err(remediation_error(
                "the runtime root exceeds its bounded generation scan",
            ));
        }
        let name = entry.file_name();
        if name == LEASE_FILE {
            let metadata = fs::symlink_metadata(entry.path())?;
            if !metadata.is_file() || metadata.file_type().is_symlink() || is_reparse(&metadata) {
                return Err(remediation_error(
                    "the WebView2 runtime lifecycle lease is not a direct file",
                ));
            }
            usage.bytes = usage
                .bytes
                .checked_add(metadata.len())
                .ok_or_else(|| remediation_error("the quarantine byte counter overflowed"))?;
            continue;
        }

        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        usage.generations = usage
            .generations
            .checked_add(1)
            .ok_or_else(|| remediation_error("the quarantine generation counter overflowed"))?;
        // Reserve one slot for the generation this successful preparation is
        // about to create; never momentarily exceed the advertised ceiling.
        if usage.generations >= MAX_TOTAL_GENERATIONS {
            return Err(remediation_error(
                "creating a fresh generation would exceed the 128-generation startup budget",
            ));
        }
        if !metadata.is_dir() || metadata.file_type().is_symlink() || is_reparse(&metadata) {
            return Err(remediation_error(&format!(
                "the runtime root contains an unowned object at {}",
                path.display()
            )));
        }
        let Some(generation) = name.to_str() else {
            measure_tree(&path, &mut usage, true)?;
            continue;
        };
        let canonical_name = ProfileId::parse(generation)
            .filter(|id| id.to_string() == generation)
            .is_some();
        if !canonical_name {
            measure_tree(&path, &mut usage, true)?;
            continue;
        }
        let status = marker_status(&path, kind, generation)?;
        if status == MarkerStatus::Invalid {
            measure_tree(&path, &mut usage, true)?;
            continue;
        }
        if status == MarkerStatus::Missing {
            // Migrate a bounded direct generation created by pre-provenance
            // Zephium into the safe baseline. It is marked current-boot and
            // cannot be reclaimed until a later reboot, so an old child still
            // terminating during this launch remains protected.
            measure_tree(&path, &mut usage, true)?;
            ensure_current_boot(boot, generation)?;
            write_generation_marker(&path, kind, generation)?;
        }
        if current_boot_contains(boot, generation)? {
            usage.same_boot_generations = usage
                .same_boot_generations
                .checked_add(1)
                .ok_or_else(|| remediation_error("the same-boot counter overflowed"))?;
            if usage.same_boot_generations >= MAX_SAME_BOOT_GENERATIONS {
                return Err(remediation_error(
                    "32 same-boot WebView2 generations are quarantined",
                ));
            }
            if status == MarkerStatus::Matches {
                measure_tree(&path, &mut usage, true)?;
            }
        } else {
            reclaim_generation(&path, kind, generation)?;
        }
    }
    if usage.bytes > MAX_QUARANTINE_BYTES {
        return Err(remediation_error(
            "the WebView2 runtime quarantine exceeds 2 GiB",
        ));
    }
    Ok(())
}

impl RuntimeGeneration {
    pub fn prepare(configured_root: &Path, kind: RuntimeGenerationKind) -> io::Result<Self> {
        fs::create_dir_all(configured_root)?;
        let generations = direct_directory(configured_root)?;
        let lease = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .share_mode(0)
            .open(generations.join(LEASE_FILE))?;
        let boot = open_boot_registry(kind)?;
        // Boot markers are shared across app identities and runtime roots.
        // Absence from this root does not prove another root's generation dead.
        // Only an exact cleanup ticket removes a marker; otherwise Windows
        // retires the volatile registry at reboot.
        maintain_generations(&generations, &boot, kind)?;

        let generation = ProfileId::generate().to_string();
        mark_current_boot(&boot, &generation)?;
        let root = generations.join(&generation);
        fs::create_dir(&root)?;
        let root = direct_directory(&root)?;
        if root.parent() != Some(generations.as_path()) {
            return Err(io::Error::other(
                "WebView2 generation escaped its owned runtime root",
            ));
        }
        write_generation_marker(&root, kind, &generation)?;
        Ok(Self {
            root,
            generation,
            kind,
            _lease: lease,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn cleanup_ticket(&self) -> RuntimeCleanupTicket {
        RuntimeCleanupTicket {
            root: self.root.clone(),
            generation: self.generation.clone(),
            kind: self.kind,
        }
    }
}

impl RuntimeCleanupTicket {
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Remove a current generation only after the caller has established the
    /// relevant exact native process-group exit proof. Successful filesystem
    /// removal is followed by volatile-key removal so clean launches do not
    /// grow HKCU for the rest of the boot.
    pub fn cleanup_after_proven_exit(&self) -> io::Result<()> {
        reclaim_generation(&self.root, self.kind, &self.generation)?;
        let boot = open_boot_registry(self.kind)?;
        match boot.delete_subkey(&self.generation) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distinct_runtime_roots_preserve_each_others_same_boot_generations() {
        for kind in [
            RuntimeGenerationKind::RawPrivate,
            RuntimeGenerationKind::Privileged,
        ] {
            let temp = tempfile::tempdir().unwrap();
            let first_root = temp.path().join("first");
            let first = RuntimeGeneration::prepare(&first_root, kind).unwrap();
            fs::write(first.root().join("owned-data"), b"keep").unwrap();
            let first_cleanup = first.cleanup_ticket();
            let second = RuntimeGeneration::prepare(&temp.path().join("second"), kind).unwrap();
            let boot = open_boot_registry(kind).unwrap();
            assert!(current_boot_contains(&boot, &first.generation).unwrap());
            assert!(current_boot_contains(&boot, &second.generation).unwrap());

            // A parent may exit before its WebView2 process. A launch using
            // another root must not turn that same-boot residue into exit proof.
            drop(first);
            let restarted = RuntimeGeneration::prepare(&first_root, kind).unwrap();
            assert_eq!(
                fs::read(first_cleanup.root().join("owned-data")).unwrap(),
                b"keep"
            );
            first_cleanup.cleanup_after_proven_exit().unwrap();
            restarted
                .cleanup_ticket()
                .cleanup_after_proven_exit()
                .unwrap();
            second.cleanup_ticket().cleanup_after_proven_exit().unwrap();
        }
    }

    #[test]
    fn runtime_kinds_have_non_aliasing_native_provenance() {
        let generation = ProfileId::generate().to_string();
        assert_ne!(
            marker_bytes(RuntimeGenerationKind::RawPrivate, &generation),
            marker_bytes(RuntimeGenerationKind::Privileged, &generation)
        );
        assert_ne!(
            RuntimeGenerationKind::RawPrivate.registry_boot_key(),
            RuntimeGenerationKind::Privileged.registry_boot_key()
        );
    }

    #[test]
    fn same_boot_legacy_generation_is_baselined_not_deleted() {
        let temp = tempfile::tempdir().unwrap();
        let generations = temp.path().join("private-runtime");
        fs::create_dir(&generations).unwrap();
        let legacy = ProfileId::generate().to_string();
        let legacy_root = generations.join(&legacy);
        fs::create_dir(&legacy_root).unwrap();
        fs::write(legacy_root.join("legacy-data"), b"private").unwrap();

        let runtime =
            RuntimeGeneration::prepare(&generations, RuntimeGenerationKind::RawPrivate).unwrap();
        assert!(legacy_root.exists());
        assert_eq!(
            marker_status(&legacy_root, RuntimeGenerationKind::RawPrivate, &legacy).unwrap(),
            MarkerStatus::Matches
        );
        let boot = open_boot_registry(RuntimeGenerationKind::RawPrivate).unwrap();
        assert!(current_boot_contains(&boot, &legacy).unwrap());

        let cleanup = runtime.cleanup_ticket();
        cleanup.cleanup_after_proven_exit().unwrap();
        assert!(!cleanup.root().exists());
    }

    #[test]
    fn missing_boot_provenance_reclaims_only_exact_marked_generation() {
        let temp = tempfile::tempdir().unwrap();
        let generations = temp.path().join("privileged-runtime");
        fs::create_dir(&generations).unwrap();
        let legacy = ProfileId::generate().to_string();
        let legacy_root = generations.join(&legacy);
        fs::create_dir(&legacy_root).unwrap();
        fs::write(legacy_root.join("legacy-data"), b"private").unwrap();

        let first =
            RuntimeGeneration::prepare(&generations, RuntimeGenerationKind::Privileged).unwrap();
        let boot = open_boot_registry(RuntimeGenerationKind::Privileged).unwrap();
        boot.delete_subkey(&legacy).unwrap();
        let first_cleanup = first.cleanup_ticket();
        first_cleanup.cleanup_after_proven_exit().unwrap();
        drop(first);

        let second =
            RuntimeGeneration::prepare(&generations, RuntimeGenerationKind::Privileged).unwrap();
        assert!(!legacy_root.exists());
        let second_cleanup = second.cleanup_ticket();
        second_cleanup.cleanup_after_proven_exit().unwrap();
    }

    #[test]
    fn exact_cleanup_removes_current_boot_registry_entry() {
        let temp = tempfile::tempdir().unwrap();
        let generations = temp.path().join("private-runtime");
        let runtime =
            RuntimeGeneration::prepare(&generations, RuntimeGenerationKind::RawPrivate).unwrap();
        let generation = runtime.generation.clone();
        let cleanup = runtime.cleanup_ticket();
        cleanup.cleanup_after_proven_exit().unwrap();
        let boot = open_boot_registry(RuntimeGenerationKind::RawPrivate).unwrap();
        assert!(!current_boot_contains(&boot, &generation).unwrap());
    }

    #[test]
    fn concurrent_profile_removal_revalidates_generation_before_cleanup() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = RuntimeGeneration::prepare(
            &temp.path().join("private-runtime"),
            RuntimeGenerationKind::RawPrivate,
        )
        .unwrap();
        let profile = runtime.root().join(ProfileId::generate().to_string());
        fs::create_dir(&profile).unwrap();
        fs::write(profile.join("private-data"), b"private").unwrap();
        let sibling = temp.path().join("sibling");
        fs::create_dir(&sibling).unwrap();
        fs::write(sibling.join("untouched"), b"keep").unwrap();

        let mut scans = 0;
        reclaim_generation_with_scan(
            runtime.root(),
            runtime.kind,
            &runtime.generation,
            |root, usage| {
                scans += 1;
                if scans == 1 {
                    // A previously queued profile directory disappears after the
                    // exact generation marker and root were validated.
                    fs::remove_dir_all(&profile).unwrap();
                    fs::read_dir(&profile).map(|_| ())
                } else {
                    measure_tree(root, usage, false)
                }
            },
        )
        .unwrap();
        assert_eq!(scans, 2);
        assert!(!runtime.root().exists());
        assert_eq!(fs::read(sibling.join("untouched")).unwrap(), b"keep");
        // A repeated exact ticket also removes its matching volatile key.
        runtime
            .cleanup_ticket()
            .cleanup_after_proven_exit()
            .unwrap();
        let boot = open_boot_registry(runtime.kind).unwrap();
        assert!(!current_boot_contains(&boot, &runtime.generation).unwrap());
    }

    #[test]
    fn concurrent_profile_removal_cannot_skip_changed_generation_marker() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = RuntimeGeneration::prepare(
            &temp.path().join("private-runtime"),
            RuntimeGenerationKind::RawPrivate,
        )
        .unwrap();
        let profile = runtime.root().join(ProfileId::generate().to_string());
        fs::create_dir(&profile).unwrap();
        let mut scans = 0;
        let error = reclaim_generation_with_scan(
            runtime.root(),
            runtime.kind,
            &runtime.generation,
            |_, _| {
                scans += 1;
                fs::remove_dir_all(&profile).unwrap();
                fs::write(runtime.root().join(GENERATION_MARKER), b"different owner").unwrap();
                fs::read_dir(&profile).map(|_| ())
            },
        )
        .unwrap_err();
        assert_eq!(
            scans, 1,
            "changed authority must fail before another scan or delete"
        );
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(runtime.root().exists());
        fs::remove_file(runtime.root().join(GENERATION_MARKER)).unwrap();
        write_generation_marker(runtime.root(), runtime.kind, &runtime.generation).unwrap();
        runtime
            .cleanup_ticket()
            .cleanup_after_proven_exit()
            .unwrap();
    }

    #[test]
    fn partial_absence_never_proves_generation_cleanup_or_extends_attempt_bound() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = RuntimeGeneration::prepare(
            &temp.path().join("private-runtime"),
            RuntimeGenerationKind::RawPrivate,
        )
        .unwrap();
        let missing_child = runtime.root().join("missing-profile");
        let mut scans = 0;
        let error = reclaim_generation_with_scan(
            runtime.root(),
            runtime.kind,
            &runtime.generation,
            |_, _| {
                scans += 1;
                fs::read_dir(&missing_child).map(|_| ())
            },
        )
        .unwrap_err();
        assert_eq!(scans, 8);
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert!(runtime.root().exists());
        let boot = open_boot_registry(runtime.kind).unwrap();
        assert!(current_boot_contains(&boot, &runtime.generation).unwrap());
        runtime
            .cleanup_ticket()
            .cleanup_after_proven_exit()
            .unwrap();
    }
}
