//! The parts of the native harness the runtime tests use.
use std::{
    cell::RefCell,
    collections::BTreeSet,
    fs::Permissions,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

thread_local! {
    static SEEN_RENDERERS: RefCell<BTreeSet<u32>> = const { RefCell::new(BTreeSet::new()) };
}

pub fn init() -> NativeSession {
    assert!(
        std::env::var_os("ZEPHIUM_WEBEXT_PROBE_PRIVATE_SESSION").is_some(),
        "run with timeout -k 10 420 scripts/ci/linux-test.sh"
    );
    for name in [
        "WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS",
        "WEBKIT_FORCE_SANDBOX",
    ] {
        assert!(std::env::var_os(name).is_none(), "sandbox override: {name}");
    }
    gtk::init().expect("isolated GTK display");
    SEEN_RENDERERS.with_borrow_mut(BTreeSet::clear);
    NativeSession
}

#[must_use = "declare before all native owners so shutdown drains last"]
pub struct NativeSession;

impl Drop for NativeSession {
    fn drop(&mut self) {
        let start = Instant::now();
        let deadline = start + Duration::from_secs(10);
        let mut quiet = None;
        let mut drained = false;
        while Instant::now() < deadline {
            while gtk::events_pending() && Instant::now() < deadline {
                gtk::main_iteration_do(false);
            }
            if web_processes().is_empty() && !gtk::events_pending() {
                let since = quiet.get_or_insert_with(Instant::now);
                if since.elapsed() >= Duration::from_millis(100) {
                    drained = true;
                    break;
                }
            } else {
                quiet = None;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        println!(
            "WEBEXT native_shutdown drained={drained} elapsed_ms={:.3} remaining={:?}",
            start.elapsed().as_secs_f64() * 1000.0,
            web_processes()
        );
        if !std::thread::panicking() {
            assert!(drained, "native shutdown did not settle before thread exit");
        }
    }
}

pub fn pump_until(mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !done() {
        while gtk::events_pending() {
            assert!(Instant::now() < deadline, "native queue did not drain");
            gtk::main_iteration_do(false);
        }
        assert!(Instant::now() < deadline, "native test timed out");
        std::thread::sleep(Duration::from_millis(1));
    }
}

pub fn pump_for(duration: Duration) {
    let until = Instant::now() + duration;
    pump_until(|| Instant::now() >= until);
}

pub struct ModuleDirectory(tempfile::TempDir);

impl ModuleDirectory {
    pub fn new() -> Self {
        let artifact = std::env::var_os("ZEPHIUM_WEBEXT_MODULE_PATH")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("ZEPHIUM_WEBEXT_MODULE_DIR").map(|directory| {
                    PathBuf::from(directory).join("libzephium_webkit_web_process.so")
                })
            })
            .expect("set ZEPHIUM_WEBEXT_MODULE_PATH to the built cdylib");
        let artifact = artifact.canonicalize().expect("built web-process cdylib");
        let directory = tempfile::tempdir().expect("isolated module directory");
        let module = directory.path().join("libzephium_webkit_web_process.so");
        std::fs::copy(artifact, &module).expect("copy only the intended module");
        std::fs::set_permissions(module, Permissions::from_mode(0o444)).unwrap();
        std::fs::set_permissions(directory.path(), Permissions::from_mode(0o555)).unwrap();
        Self(directory)
    }
}

impl ModuleDirectory {
    pub fn path(&self) -> &Path {
        self.0.path()
    }
}

impl Drop for ModuleDirectory {
    fn drop(&mut self) {
        let _ = std::fs::set_permissions(self.path(), Permissions::from_mode(0o700));
    }
}

pub fn web_processes() -> BTreeSet<u32> {
    let mut pending = vec![std::process::id()];
    let mut renderers = BTreeSet::new();
    let mut visited = BTreeSet::new();
    while let Some(pid) = pending.pop() {
        if !visited.insert(pid) {
            continue;
        }
        if let Ok(tasks) = std::fs::read_dir(format!("/proc/{pid}/task")) {
            for task in tasks.flatten() {
                if let Ok(children) = std::fs::read_to_string(task.path().join("children")) {
                    pending.extend(
                        children
                            .split_whitespace()
                            .filter_map(|pid| pid.parse::<u32>().ok()),
                    );
                }
            }
        }
        if is_renderer(pid) {
            renderers.insert(pid);
        }
    }
    SEEN_RENDERERS.with_borrow_mut(|seen| {
        seen.extend(renderers);
        seen.retain(|pid| is_renderer(*pid));
        seen.clone()
    })
}

fn is_renderer(pid: u32) -> bool {
    std::fs::read(format!("/proc/{pid}/cmdline"))
        .unwrap_or_default()
        .split(|byte| *byte == 0)
        .next()
        .is_some_and(|arg| arg == b"WebKitWebProcess" || arg.ends_with(b"/WebKitWebProcess"))
}
