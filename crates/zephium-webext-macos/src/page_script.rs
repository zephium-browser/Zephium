//! A program the browser runs in every page's main world, from an extension
//! context of its own. The host never lists it with installed extensions, so
//! it has no toolbar action, storage or background. Whatever must vary by
//! site, such as a paused site, the program decides itself: a match pattern
//! sees each frame's own address, never the page it is embedded in.

use std::cell::RefCell;
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::{MainThreadMarker, Message};
use objc2_foundation::{NSError, NSOperatingSystemVersion, NSProcessInfo, NSString, NSURL};
use objc2_web_kit::{
    WKWebExtension, WKWebExtensionContext, WKWebExtensionContextPermissionStatus,
    WKWebExtensionController, WKWebExtensionMatchPattern,
};

use crate::runtime::describe;

const ID: &str = "zephium-page-script";
const PROGRAM: &str = "program.js";
const MANIFEST: &str = r#"{
  "manifest_version": 3,
  "name": "Zephium",
  "version": "1.0",
  "host_permissions": ["<all_urls>"],
  "content_scripts": [{
    "matches": ["<all_urls>"],
    "js": ["program.js"],
    "run_at": "document_start",
    "all_frames": true,
    "match_about_blank": true,
    "world": "MAIN"
  }]
}
"#;

pub struct PageScript {
    controller: Retained<WKWebExtensionController>,
    private_data: bool,
    state: Rc<RefCell<State>>,
}

#[derive(Default)]
struct State {
    generation: u64,
    root: Option<PathBuf>,
    context: Option<Retained<WKWebExtensionContext>>,
}

impl PageScript {
    /// Main-world content scripts need macOS 15.4.
    pub fn supported() -> bool {
        NSProcessInfo::processInfo().isOperatingSystemAtLeastVersion(NSOperatingSystemVersion {
            majorVersion: 15,
            minorVersion: 4,
            patchVersion: 0,
        })
    }

    /// `private_data` lets the program run in private browsing views, and
    /// belongs only on a controller that private views alone use.
    pub fn new(controller: &WKWebExtensionController, private_data: bool) -> Self {
        Self {
            controller: controller.retain(),
            private_data,
            state: Rc::default(),
        }
    }

    /// Writes a package for `program` into `dir`, which must not exist yet.
    /// The package is complete before `dir` appears.
    pub fn write(dir: &Path, program: &str) -> io::Result<()> {
        let parent = dir.parent().ok_or(io::ErrorKind::InvalidInput)?;
        let name = dir.file_name().ok_or(io::ErrorKind::InvalidInput)?;
        let staging = parent.join(format!(".{}.partial", name.to_string_lossy()));
        if staging.exists() {
            std::fs::remove_dir_all(&staging)?;
        }
        std::fs::create_dir_all(&staging)?;
        std::fs::write(staging.join("manifest.json"), MANIFEST)?;
        std::fs::write(staging.join(PROGRAM), program)?;
        std::fs::rename(&staging, dir)
    }

    /// The package now loaded, if any.
    pub fn root(&self) -> Option<PathBuf> {
        let state = self.state.borrow();
        state.context.as_ref().and(state.root.clone())
    }

    /// Runs the package in `root` from the next document on, or nothing with
    /// `None`. A replacement takes over in one main-thread turn, so no page
    /// starts with both programs or with neither. True when WebKit is still
    /// reading the package; `done` runs once it is in place.
    pub fn set(
        &self,
        root: Option<&Path>,
        done: impl FnOnce(Result<(), String>) + 'static,
    ) -> bool {
        let generation = {
            let mut state = self.state.borrow_mut();
            if state.root.as_deref() == root && (root.is_none() || state.context.is_some()) {
                drop(state);
                done(Ok(()));
                return false;
            }
            state.generation += 1;
            state.root = root.map(Path::to_path_buf);
            state.generation
        };
        let Some(root) = root else {
            let old = self.state.borrow_mut().context.take();
            if let Some(old) = old {
                let _ = unsafe { self.controller.unloadExtensionContext_error(&old) };
            }
            done(Ok(()));
            return false;
        };
        if !root.is_dir() {
            done(Err(format!("{} is not a directory", root.display())));
            return false;
        }
        let url =
            NSURL::fileURLWithPath_isDirectory(&NSString::from_str(&root.to_string_lossy()), true);
        let state = Rc::downgrade(&self.state);
        let controller = self.controller.clone();
        let private_data = self.private_data;
        let done = RefCell::new(Some(done));
        let completion =
            RcBlock::new(move |extension: *mut WKWebExtension, error: *mut NSError| {
                let Some(done) = done.take() else {
                    return;
                };
                let Some(state) = state.upgrade() else {
                    return done(Err("the page script was closed".to_owned()));
                };
                if state.borrow().generation != generation {
                    return done(Ok(()));
                }
                let Some(extension) = (unsafe { extension.as_ref() }) else {
                    return done(Err(describe(unsafe { error.as_ref() })));
                };
                done(activate(&state, &controller, extension, private_data));
            });
        unsafe {
            WKWebExtension::extensionWithResourceBaseURL_completionHandler(
                &url,
                &completion,
                MainThreadMarker::from(&*self.controller),
            )
        };
        true
    }
}

impl Drop for PageScript {
    fn drop(&mut self) {
        if let Some(context) = self.state.borrow_mut().context.take() {
            let _ = unsafe { self.controller.unloadExtensionContext_error(&context) };
        }
    }
}

fn activate(
    state: &Rc<RefCell<State>>,
    controller: &WKWebExtensionController,
    extension: &WKWebExtension,
    private_data: bool,
) -> Result<(), String> {
    let mtm = MainThreadMarker::from(controller);
    let context = unsafe { WKWebExtensionContext::contextForExtension(extension) };
    unsafe {
        context.setUniqueIdentifier(&NSString::from_str(ID));
        context.setInspectable(false);
        context.setHasAccessToPrivateData(private_data);
        context.setPermissionStatus_forMatchPattern(
            WKWebExtensionContextPermissionStatus::GrantedExplicitly,
            &WKWebExtensionMatchPattern::allURLsMatchPattern(mtm),
        );
    }
    let mut state = state.borrow_mut();
    if let Some(old) = state.context.take() {
        let _ = unsafe { controller.unloadExtensionContext_error(&old) };
    }
    unsafe { controller.loadExtensionContext_error(&context) }
        .map_err(|error| describe(Some(&error)))?;
    state.context = Some(context);
    Ok(())
}
