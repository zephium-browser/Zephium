//! One startup choice for every Windows WebView2 controller. WebView2 joins
//! per-controller requirements across a user-data folder. Its default is ON;
//! only an explicit OFF choice needs the optional Settings8 API.

use std::sync::atomic::{AtomicBool, Ordering};

struct Policy {
    required: bool,
    managed: AtomicBool,
}
impl Policy {
    fn new(required: bool) -> Self {
        Self {
            required,
            managed: AtomicBool::new(false),
        }
    }
    fn apply<E: std::fmt::Display>(&self, disable: impl FnOnce() -> Result<(), E>) -> bool {
        if self.required || self.managed.load(Ordering::Acquire) {
            return false;
        }
        if let Err(error) = disable() {
            eprintln!("Windows reputation checking is managed; OFF could not be applied: {error}");
            return !self.managed.swap(true, Ordering::AcqRel);
        }
        false
    }
}

#[cfg(target_os = "windows")]
static POLICY: std::sync::OnceLock<Policy> = std::sync::OnceLock::new();
#[cfg(target_os = "windows")]
static MANAGED: std::sync::OnceLock<Box<dyn Fn() + Send + Sync>> = std::sync::OnceLock::new();

/// Install before constructing Windows WebViews. Preference edits apply on restart.
#[cfg(target_os = "windows")]
pub fn initialize(required: bool) -> bool {
    POLICY.set(Policy::new(required)).is_ok()
}

#[cfg(target_os = "windows")]
pub fn on_managed(handler: impl Fn() + Send + Sync + 'static) {
    let _ = MANAGED.set(Box::new(handler));
}

#[cfg(target_os = "windows")]
pub fn managed() -> bool {
    POLICY
        .get()
        .is_some_and(|policy| policy.managed.load(Ordering::Acquire))
}

#[cfg(target_os = "windows")]
pub fn configure(
    core: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2,
) -> windows_core::Result<()> {
    use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Settings8;
    use windows_core::Interface;
    let policy = POLICY.get_or_init(|| Policy::new(true));
    if policy.apply(|| -> windows_core::Result<()> {
        let settings = unsafe { core.Settings()? }.cast::<ICoreWebView2Settings8>()?;
        unsafe { settings.SetIsReputationCheckingRequired(false)? };
        let mut actual = windows_core::BOOL::default();
        unsafe { settings.IsReputationCheckingRequired(&mut actual)? };
        if actual.as_bool() {
            return Err(windows::Win32::Foundation::E_ACCESSDENIED.into());
        }
        Ok(())
    }) {
        if let Some(handler) = MANAGED.get() {
            handler();
        }
    }
    // Missing Settings8 and policy refusal are managed states, never view failures.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn default_on_never_queries_optional_settings() {
        let policy = Policy::new(true);
        assert!(!policy.apply(|| -> Result<(), &str> { panic!("default ON queried Settings8") }));
        assert!(!policy.managed.load(Ordering::Acquire));
    }
    #[test]
    fn off_failure_projects_managed_and_does_not_fail_view_creation() {
        let policy = Policy::new(false);
        assert!(policy.apply(|| Err("Settings8 unavailable or enterprise policy refused OFF")));
        assert!(policy.managed.load(Ordering::Acquire));
        assert!(!policy.apply(|| -> Result<(), &str> { panic!("managed policy was retried") }));
    }
    #[test]
    fn successful_off_preserves_persons_control() {
        let policy = Policy::new(false);
        assert!(!policy.apply(|| Ok::<(), &str>(())));
        assert!(!policy.managed.load(Ordering::Acquire));
    }
}
