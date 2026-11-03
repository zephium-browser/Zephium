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
// The browser processes a generation started, one `pid creation-time` line
// each. Versioned by name: a generation older than this file has none and
// keeps the reboot-only rule.
const PROCESS_RECORD: &str = ".zephium-webview2-processes-v1";
const MAX_RECORDED_PROCESSES: usize = 8;
const MAX_RECORD_BYTES: u64 = 512;

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
    report: GenerationReport,
    // Deny all sharing modes. This independently prevents two Zephium parent
    // processes from classifying/removing one root concurrently while the
    // outer single-instance plugin is still settling.
    _lease: File,
}

/// One browser process a generation started: its id and its creation time
/// (FILETIME ticks), which together name it even after Windows reuses the id.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BrowserProcessRecord {
    pub id: u32,
    pub created: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecordedProcessState {
    /// No process with this id and creation time exists.
    Gone,
    Running,
}

/// Native checks on the exact browser processes an earlier run recorded.
pub trait LeftoverProcesses {
    fn state(&self, process: BrowserProcessRecord) -> RecordedProcessState;
    /// Ends the exact process and its child processes. True once its exit is
    /// proven.
    fn terminate(&self, process: BrowserProcessRecord) -> bool;
}

/// What preparing a root did with the generations earlier runs left behind,
/// for the caller to log and surface.
#[derive(Debug, Default)]
pub struct GenerationReport {
    /// Every recorded browser process had already exited.
    pub reclaimed_after_exit: Vec<String>,
    /// Orphaned browser processes were ended first.
    pub reclaimed_after_termination: Vec<String>,
    /// The generation held no browsing data, only its own bookkeeping.
    pub reclaimed_empty: Vec<String>,
    /// An orphan could not be ended, so its data stays until a reboot.
    pub kept_running: Vec<String>,
    /// Over a startup budget: the launch went ahead and this says why the
    /// leftovers still need a reboot.
    pub overdue: Option<String>,
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
    over_budget: bool,
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

/// Walks a tree, refusing redirecting objects. Measuring what stays
/// (`budgeted`) stops at the startup budgets and marks the usage over them;
/// the walk before a deletion has no such way out and fails instead.
fn measure_tree(root: &Path, usage: &mut QuarantineUsage, budgeted: bool) -> io::Result<()> {
    let mut pending = vec![root.to_owned()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            usage.entries = usage
                .entries
                .checked_add(1)
                .ok_or_else(|| io::Error::other("WebView2 quarantine entry counter overflowed"))?;
            if usage.entries > MAX_SCANNED_ENTRIES {
                if budgeted {
                    usage.over_budget = true;
                    return Ok(());
                }
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
                if budgeted && usage.bytes > MAX_QUARANTINE_BYTES {
                    usage.over_budget = true;
                    return Ok(());
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

/// Reclaims what earlier runs left in this root and measures what must stay.
/// A leftover that cannot be inspected or removed stays put and makes the
/// root overdue; it never stops this launch, whose own generation is new.
fn maintain_generations(
    generations: &Path,
    boot: &RegKey,
    kind: RuntimeGenerationKind,
    leftovers: &dyn LeftoverProcesses,
    report: &mut GenerationReport,
) -> io::Result<()> {
    let mut usage = QuarantineUsage::default();
    let mut top_level_entries = 0_usize;
    for entry in fs::read_dir(generations)? {
        let entry = entry?;
        top_level_entries = top_level_entries.saturating_add(1);
        if top_level_entries > MAX_TOTAL_GENERATIONS + 1 {
            note_overdue(
                report,
                "the runtime folder holds more entries than one launch inspects",
            );
            break;
        }
        let name = entry.file_name();
        if name == LEASE_FILE {
            let metadata = fs::symlink_metadata(entry.path())?;
            if !metadata.is_file() || metadata.file_type().is_symlink() || is_reparse(&metadata) {
                return Err(remediation_error(
                    "the WebView2 runtime lifecycle lease is not a direct file",
                ));
            }
            usage.bytes = usage.bytes.saturating_add(metadata.len());
            continue;
        }
        if let Err(error) = maintain_generation(
            &entry.path(),
            &name,
            boot,
            kind,
            leftovers,
            &mut usage,
            report,
        ) {
            note_overdue(
                report,
                &format!("a leftover WebView2 generation could not be cleared ({error})"),
            );
        }
    }
    if usage.generations >= MAX_TOTAL_GENERATIONS {
        note_overdue(report, "128 leftover WebView2 generations are waiting");
    }
    if usage.same_boot_generations >= MAX_SAME_BOOT_GENERATIONS {
        note_overdue(
            report,
            "32 WebView2 generations from runs that did not close cleanly are waiting",
        );
    }
    if usage.over_budget || usage.bytes > MAX_QUARANTINE_BYTES {
        note_overdue(report, "leftover WebView2 data is over its 2 GiB budget");
    }
    Ok(())
}

fn maintain_generation(
    path: &Path,
    name: &std::ffi::OsStr,
    boot: &RegKey,
    kind: RuntimeGenerationKind,
    leftovers: &dyn LeftoverProcesses,
    usage: &mut QuarantineUsage,
    report: &mut GenerationReport,
) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() || is_reparse(&metadata) {
        usage.generations = usage.generations.saturating_add(1);
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unowned object at {}", path.display()),
        ));
    }
    let canonical = name.to_str().filter(|generation| {
        ProfileId::parse(generation).is_some_and(|id| id.to_string() == *generation)
    });
    let Some(generation) = canonical else {
        usage.generations = usage.generations.saturating_add(1);
        return measure_tree(path, usage, true);
    };
    match marker_status(path, kind, generation)? {
        MarkerStatus::Invalid => {
            usage.generations = usage.generations.saturating_add(1);
            measure_tree(path, usage, true)
        }
        MarkerStatus::Missing => {
            // Migrate a bounded direct generation created by pre-provenance
            // Zephium into the safe baseline. It is marked current-boot and
            // cannot be reclaimed until a later reboot, so an old child still
            // terminating during this launch remains protected.
            usage.generations = usage.generations.saturating_add(1);
            usage.same_boot_generations = usage.same_boot_generations.saturating_add(1);
            measure_tree(path, usage, true)?;
            ensure_current_boot(boot, generation)?;
            write_generation_marker(path, kind, generation)
        }
        MarkerStatus::Matches if !current_boot_contains(boot, generation)? => {
            reclaim_generation(path, kind, generation)
        }
        MarkerStatus::Matches => {
            if let Some(reclaimed) = reclaim_same_boot(path, kind, generation, leftovers, report)? {
                forget_current_boot(boot, generation)?;
                match reclaimed {
                    SameBootReclaim::AfterExit => &mut report.reclaimed_after_exit,
                    SameBootReclaim::AfterTermination => &mut report.reclaimed_after_termination,
                    SameBootReclaim::Empty => &mut report.reclaimed_empty,
                }
                .push(generation.to_owned());
                return Ok(());
            }
            usage.generations = usage.generations.saturating_add(1);
            usage.same_boot_generations = usage.same_boot_generations.saturating_add(1);
            measure_tree(path, usage, true)
        }
    }
}

enum SameBootReclaim {
    AfterExit,
    AfterTermination,
    Empty,
}

/// A same-boot leftover is a run that ended without proving its browser
/// processes gone. Its own record now proves it: every recorded process has
/// exited, or is an orphan this launch may end. The caller holds the root's
/// exclusive lease, so no other Zephium instance can own any process recorded
/// here. Without a record, only a generation holding no browsing data at all
/// is cleared, since a running browser always keeps files open in its folder.
fn reclaim_same_boot(
    path: &Path,
    kind: RuntimeGenerationKind,
    generation: &str,
    leftovers: &dyn LeftoverProcesses,
    report: &mut GenerationReport,
) -> io::Result<Option<SameBootReclaim>> {
    let reclaimed = match read_process_record(path)? {
        Some(processes) if !processes.is_empty() => {
            let running = processes
                .into_iter()
                .filter(|process| leftovers.state(*process) == RecordedProcessState::Running)
                .collect::<Vec<_>>();
            if running.is_empty() {
                SameBootReclaim::AfterExit
            } else if running.iter().all(|process| leftovers.terminate(*process)) {
                SameBootReclaim::AfterTermination
            } else {
                report.kept_running.push(generation.to_owned());
                return Ok(None);
            }
        }
        _ if holds_only_bookkeeping(path)? => SameBootReclaim::Empty,
        _ => return Ok(None),
    };
    reclaim_generation(path, kind, generation)?;
    Ok(Some(reclaimed))
}

fn note_overdue(report: &mut GenerationReport, reason: &str) {
    report.overdue.get_or_insert_with(|| {
        format!("{reason}. Zephium started anyway; restarting Windows lets it clear them")
    });
}

fn forget_current_boot(boot: &RegKey, generation: &str) -> io::Result<()> {
    match boot.delete_subkey(generation) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn create_process_record(root: &Path) -> io::Result<()> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(root.join(PROCESS_RECORD))?
        .sync_all()
}

/// The recorded processes, or None when the record is missing or malformed,
/// which leaves the generation to the reboot-only rule.
fn read_process_record(root: &Path) -> io::Result<Option<Vec<BrowserProcessRecord>>> {
    let path = root.join(PROCESS_RECORD);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || is_reparse(&metadata)
        || metadata.len() > MAX_RECORD_BYTES
    {
        return Ok(None);
    }
    let mut text = String::new();
    if File::open(&path)?
        .take(MAX_RECORD_BYTES + 1)
        .read_to_string(&mut text)
        .is_err()
    {
        return Ok(None);
    }
    Ok(parse_process_record(&text))
}

fn parse_process_record(text: &str) -> Option<Vec<BrowserProcessRecord>> {
    let processes = text
        .lines()
        .map(|line| {
            let (id, created) = line.split_once(' ')?;
            Some(BrowserProcessRecord {
                id: id.parse().ok().filter(|id| *id != 0)?,
                created: created.parse().ok().filter(|created| *created != 0)?,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    (processes.len() <= MAX_RECORDED_PROCESSES).then_some(processes)
}

fn holds_only_bookkeeping(root: &Path) -> io::Result<bool> {
    let mut pending = vec![root.to_owned()];
    let mut entries = 0_usize;
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            entries += 1;
            if entries > MAX_SCANNED_ENTRIES {
                return Ok(false);
            }
            let metadata = fs::symlink_metadata(entry.path())?;
            if metadata.file_type().is_symlink() || is_reparse(&metadata) {
                return Ok(false);
            }
            if metadata.is_dir() {
                pending.push(entry.path());
                continue;
            }
            let own = directory == root
                && (entry.file_name() == GENERATION_MARKER || entry.file_name() == PROCESS_RECORD);
            if !own {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

impl RuntimeGeneration {
    /// Clears what earlier runs left behind, then creates this run's
    /// generation. `leftovers` checks the browser processes those runs
    /// recorded; see [`GenerationReport`] for what happened to each.
    pub fn prepare(
        configured_root: &Path,
        kind: RuntimeGenerationKind,
        leftovers: &dyn LeftoverProcesses,
    ) -> io::Result<Self> {
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
        // Only an exact cleanup ticket or this run's own process proof removes
        // a marker; otherwise Windows retires the volatile registry at reboot.
        let mut report = GenerationReport::default();
        maintain_generations(&generations, &boot, kind, leftovers, &mut report)?;

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
        create_process_record(&root)?;
        Ok(Self {
            root,
            generation,
            kind,
            report,
            _lease: lease,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn report(&self) -> &GenerationReport {
        &self.report
    }

    /// Records a browser process this generation's data belongs to, so a
    /// later launch can prove it gone, or end it, if this run never does.
    pub fn record_browser_process(&self, process: BrowserProcessRecord) -> io::Result<()> {
        let recorded = read_process_record(&self.root)?.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "WebView2 generation process record is missing or malformed",
            )
        })?;
        if recorded.contains(&process) {
            return Ok(());
        }
        if recorded.len() >= MAX_RECORDED_PROCESSES {
            return Err(io::Error::new(
                io::ErrorKind::StorageFull,
                "WebView2 generation process record is full",
            ));
        }
        let mut record = OpenOptions::new()
            .append(true)
            .open(self.root.join(PROCESS_RECORD))?;
        writeln!(record, "{} {}", process.id, process.created)?;
        record.sync_all()
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
    use std::cell::RefCell;

    struct NoLeftovers;

    impl LeftoverProcesses for NoLeftovers {
        fn state(&self, _: BrowserProcessRecord) -> RecordedProcessState {
            RecordedProcessState::Gone
        }
        fn terminate(&self, _: BrowserProcessRecord) -> bool {
            true
        }
    }

    /// Exactly these processes still run; ending them succeeds or fails.
    struct Running {
        processes: Vec<BrowserProcessRecord>,
        ends: bool,
        ended: RefCell<Vec<BrowserProcessRecord>>,
    }

    impl Running {
        fn new(processes: &[BrowserProcessRecord], ends: bool) -> Self {
            Self {
                processes: processes.to_vec(),
                ends,
                ended: RefCell::new(Vec::new()),
            }
        }
    }

    impl LeftoverProcesses for Running {
        fn state(&self, process: BrowserProcessRecord) -> RecordedProcessState {
            if self.processes.contains(&process) {
                RecordedProcessState::Running
            } else {
                RecordedProcessState::Gone
            }
        }
        fn terminate(&self, process: BrowserProcessRecord) -> bool {
            self.ended.borrow_mut().push(process);
            self.ends
        }
    }

    const BROWSER: BrowserProcessRecord = BrowserProcessRecord {
        id: 4242,
        created: 133_000_000_000_000_000,
    };

    /// A run that ended without proving its browser gone: its generation,
    /// its private data and its current-boot key all stay behind.
    fn unclean_run(
        root: &Path,
        kind: RuntimeGenerationKind,
        record: Option<BrowserProcessRecord>,
    ) -> RuntimeCleanupTicket {
        let run = RuntimeGeneration::prepare(root, kind, &NoLeftovers).unwrap();
        fs::write(run.root().join("private-data"), b"private").unwrap();
        if let Some(process) = record {
            run.record_browser_process(process).unwrap();
        }
        run.cleanup_ticket()
    }

    fn boot_knows(kind: RuntimeGenerationKind, generation: &str) -> bool {
        current_boot_contains(&open_boot_registry(kind).unwrap(), generation).unwrap()
    }

    #[test]
    fn a_failed_quit_is_wiped_at_the_next_launch_once_its_browser_is_gone() {
        for kind in [
            RuntimeGenerationKind::RawPrivate,
            RuntimeGenerationKind::Privileged,
        ] {
            let temp = tempfile::tempdir().unwrap();
            let left = unclean_run(temp.path(), kind, Some(BROWSER));
            let next = RuntimeGeneration::prepare(temp.path(), kind, &NoLeftovers).unwrap();
            assert!(!left.root().exists());
            assert!(!boot_knows(kind, &left.generation));
            assert_eq!(
                next.report().reclaimed_after_exit,
                std::slice::from_ref(&left.generation)
            );
            next.cleanup_ticket().cleanup_after_proven_exit().unwrap();
        }
    }

    #[test]
    fn a_reused_process_id_does_not_keep_a_generation() {
        let temp = tempfile::tempdir().unwrap();
        let kind = RuntimeGenerationKind::RawPrivate;
        let left = unclean_run(temp.path(), kind, Some(BROWSER));
        let stranger = BrowserProcessRecord {
            created: BROWSER.created + 1,
            ..BROWSER
        };
        let running = Running::new(&[stranger], false);
        let next = RuntimeGeneration::prepare(temp.path(), kind, &running).unwrap();
        assert!(!left.root().exists());
        assert!(running.ended.borrow().is_empty());
        assert_eq!(
            next.report().reclaimed_after_exit,
            std::slice::from_ref(&left.generation)
        );
        next.cleanup_ticket().cleanup_after_proven_exit().unwrap();
    }

    #[test]
    fn an_orphaned_browser_is_ended_and_its_data_wiped() {
        let temp = tempfile::tempdir().unwrap();
        let kind = RuntimeGenerationKind::RawPrivate;
        let left = unclean_run(temp.path(), kind, Some(BROWSER));
        let running = Running::new(&[BROWSER], true);
        let next = RuntimeGeneration::prepare(temp.path(), kind, &running).unwrap();
        assert_eq!(*running.ended.borrow(), [BROWSER]);
        assert!(!left.root().exists());
        assert!(!boot_knows(kind, &left.generation));
        assert_eq!(
            next.report().reclaimed_after_termination,
            std::slice::from_ref(&left.generation)
        );
        next.cleanup_ticket().cleanup_after_proven_exit().unwrap();
    }

    #[test]
    fn an_orphan_that_cannot_be_ended_keeps_its_generation_until_reboot() {
        let temp = tempfile::tempdir().unwrap();
        let kind = RuntimeGenerationKind::RawPrivate;
        let left = unclean_run(temp.path(), kind, Some(BROWSER));
        let running = Running::new(&[BROWSER], false);
        let next = RuntimeGeneration::prepare(temp.path(), kind, &running).unwrap();
        assert_eq!(
            fs::read(left.root().join("private-data")).unwrap(),
            b"private"
        );
        assert!(boot_knows(kind, &left.generation));
        assert_eq!(
            next.report().kept_running,
            std::slice::from_ref(&left.generation)
        );
        left.cleanup_after_proven_exit().unwrap();
        next.cleanup_ticket().cleanup_after_proven_exit().unwrap();
    }

    #[test]
    fn a_generation_without_a_process_record_keeps_the_reboot_rule() {
        let temp = tempfile::tempdir().unwrap();
        let kind = RuntimeGenerationKind::RawPrivate;
        let left = unclean_run(temp.path(), kind, Some(BROWSER));
        // As a generation from a build that predates the record leaves it.
        fs::remove_file(left.root().join(PROCESS_RECORD)).unwrap();
        let next = RuntimeGeneration::prepare(temp.path(), kind, &NoLeftovers).unwrap();
        assert!(left.root().join("private-data").exists());
        assert!(boot_knows(kind, &left.generation));
        assert!(next.report().reclaimed_after_exit.is_empty());
        left.cleanup_after_proven_exit().unwrap();
        next.cleanup_ticket().cleanup_after_proven_exit().unwrap();
    }

    #[test]
    fn a_generation_holding_only_its_bookkeeping_is_cleared() {
        let temp = tempfile::tempdir().unwrap();
        let kind = RuntimeGenerationKind::Privileged;
        let run = RuntimeGeneration::prepare(temp.path(), kind, &NoLeftovers).unwrap();
        fs::create_dir(run.root().join("main")).unwrap();
        let left = run.cleanup_ticket();
        drop(run);
        let next = RuntimeGeneration::prepare(temp.path(), kind, &NoLeftovers).unwrap();
        assert!(!left.root().exists());
        assert_eq!(
            next.report().reclaimed_empty,
            std::slice::from_ref(&left.generation)
        );
        next.cleanup_ticket().cleanup_after_proven_exit().unwrap();
    }

    #[test]
    fn too_many_leftovers_never_stop_a_launch() {
        let temp = tempfile::tempdir().unwrap();
        let kind = RuntimeGenerationKind::RawPrivate;
        let left = (0..MAX_SAME_BOOT_GENERATIONS)
            .map(|_| unclean_run(temp.path(), kind, None))
            .collect::<Vec<_>>();
        let next = RuntimeGeneration::prepare(temp.path(), kind, &NoLeftovers).unwrap();
        assert!(next
            .report()
            .overdue
            .as_deref()
            .is_some_and(|reason| reason.contains("32 WebView2 generations")));
        for ticket in left {
            ticket.cleanup_after_proven_exit().unwrap();
        }
        next.cleanup_ticket().cleanup_after_proven_exit().unwrap();
    }

    #[test]
    fn the_process_record_is_exact_and_bounded() {
        let temp = tempfile::tempdir().unwrap();
        let run = RuntimeGeneration::prepare(
            temp.path(),
            RuntimeGenerationKind::RawPrivate,
            &NoLeftovers,
        )
        .unwrap();
        assert_eq!(read_process_record(run.root()).unwrap(), Some(Vec::new()));
        run.record_browser_process(BROWSER).unwrap();
        run.record_browser_process(BROWSER).unwrap();
        assert_eq!(
            read_process_record(run.root()).unwrap(),
            Some(vec![BROWSER])
        );
        for id in 1..MAX_RECORDED_PROCESSES as u32 {
            run.record_browser_process(BrowserProcessRecord { id, ..BROWSER })
                .unwrap();
        }
        let full = run
            .record_browser_process(BrowserProcessRecord { id: 99, ..BROWSER })
            .unwrap_err();
        assert_eq!(full.kind(), io::ErrorKind::StorageFull);
        for malformed in ["4242", "0 5", "4242 x", "4242 5\n\n", "-1 5"] {
            assert_eq!(parse_process_record(malformed), None, "{malformed:?}");
        }
        run.cleanup_ticket().cleanup_after_proven_exit().unwrap();
    }

    #[test]
    fn distinct_runtime_roots_preserve_each_others_same_boot_generations() {
        for kind in [
            RuntimeGenerationKind::RawPrivate,
            RuntimeGenerationKind::Privileged,
        ] {
            let temp = tempfile::tempdir().unwrap();
            let first_root = temp.path().join("first");
            let first = RuntimeGeneration::prepare(&first_root, kind, &NoLeftovers).unwrap();
            fs::write(first.root().join("owned-data"), b"keep").unwrap();
            let first_cleanup = first.cleanup_ticket();
            let second =
                RuntimeGeneration::prepare(&temp.path().join("second"), kind, &NoLeftovers)
                    .unwrap();
            let boot = open_boot_registry(kind).unwrap();
            assert!(current_boot_contains(&boot, &first.generation).unwrap());
            assert!(current_boot_contains(&boot, &second.generation).unwrap());

            // A parent may exit before its WebView2 process. A launch using
            // another root must not turn that same-boot residue into exit proof.
            drop(first);
            let restarted = RuntimeGeneration::prepare(&first_root, kind, &NoLeftovers).unwrap();
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

        let runtime = RuntimeGeneration::prepare(
            &generations,
            RuntimeGenerationKind::RawPrivate,
            &NoLeftovers,
        )
        .unwrap();
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

        let first = RuntimeGeneration::prepare(
            &generations,
            RuntimeGenerationKind::Privileged,
            &NoLeftovers,
        )
        .unwrap();
        let boot = open_boot_registry(RuntimeGenerationKind::Privileged).unwrap();
        boot.delete_subkey(&legacy).unwrap();
        let first_cleanup = first.cleanup_ticket();
        first_cleanup.cleanup_after_proven_exit().unwrap();
        drop(first);

        let second = RuntimeGeneration::prepare(
            &generations,
            RuntimeGenerationKind::Privileged,
            &NoLeftovers,
        )
        .unwrap();
        assert!(!legacy_root.exists());
        let second_cleanup = second.cleanup_ticket();
        second_cleanup.cleanup_after_proven_exit().unwrap();
    }

    #[test]
    fn exact_cleanup_removes_current_boot_registry_entry() {
        let temp = tempfile::tempdir().unwrap();
        let generations = temp.path().join("private-runtime");
        let runtime = RuntimeGeneration::prepare(
            &generations,
            RuntimeGenerationKind::RawPrivate,
            &NoLeftovers,
        )
        .unwrap();
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
            &NoLeftovers,
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
            &NoLeftovers,
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
            &NoLeftovers,
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
