use super::*;
use std::time::{Duration, Instant};
use zephium_core::ports::engine::WebExtensionAccessRequest;
use zephium_webext::windows::permissions::{Grants, PermissionRequests, Permissions};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(25);
pub(super) const REQUEST_URL_PREFIX: &str = "about:blank#zephium-permissions=";
const MAX_MESSAGE_BYTES: usize = 32_768;
const PRIVACY_SETTINGS: [&str; 3] = [
    "passwordSavingEnabled",
    "autofillAddressEnabled",
    "autofillCreditCardEnabled",
];

#[derive(Default)]
pub(super) struct OptionalPermissions {
    requests: PermissionRequests<ExtensionRuntimeInstance>,
    timer: Option<crate::platform::imp::ContentPolicyTimeout>,
}

pub(super) struct PermissionState {
    declared: Permissions,
    grants: Grants,
    path: PathBuf,
}

impl PermissionState {
    pub(super) fn new(
        profile_path: &Path,
        install: ExtensionInstallId,
        manifest: &zephium_webext::manifest::Manifest,
    ) -> Self {
        let declared = Permissions {
            permissions: manifest.optional_permissions(),
            origins: manifest.optional_host_permissions(),
        };
        let path = grant_path(profile_path, install);
        let grants = Grants::restore(&path, &declared);
        Self {
            declared,
            grants,
            path,
        }
    }

    fn replace(&mut self, grants: Grants) -> bool {
        if self.grants == grants {
            return true;
        }
        if grants.save(&self.path).is_err() {
            eprintln!("extensions: Windows optional permission state could not be saved");
            return false;
        }
        self.grants = grants;
        true
    }
}

fn grant_path(profile_path: &Path, install: ExtensionInstallId) -> PathBuf {
    profile_path.join(format!("zephium-optional-permissions-{install}.json"))
}

pub(super) fn erase_grants(profile_path: &Path, install: ExtensionInstallId) {
    match std::fs::remove_file(grant_path(profile_path, install)) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => eprintln!("extensions: Windows optional permission state could not be removed"),
    }
}

fn parse_message(mut value: Value) -> Option<(u64, Permissions)> {
    let reply = reply_id(&value)?;
    let spec = serde_json::from_value::<Permissions>(value["spec"].take()).ok()?;
    spec.bounded_request().then_some((reply, spec))
}

fn reply_id(value: &Value) -> Option<u64> {
    value["id"]
        .as_u64()
        .filter(|id| (1..=(1 << 53) - 1).contains(id))
}

fn send_result(install: &Install, reply: u64, result: Value) {
    if !install.bridge.alive.get() {
        return;
    }
    let message = json!({"kind": "permissions-result", "id": reply, "result": result}).to_string();
    let message = webview2_com::CoTaskMemPWSTR::from(message.as_str());
    // The bridge belongs to this exact install's management controller.
    if unsafe {
        install
            .bridge
            .view
            .webview()
            .PostWebMessageAsJson(*message.as_ref().as_pcwstr())
    }
    .is_err()
    {
        eprintln!("extensions: Windows optional permission response could not be sent");
    }
}

impl WindowsExtensions {
    fn autofill_settings(
        &self,
        profile: ProfileId,
        replacement: Option<(ExtensionRuntimeInstance, &Grants)>,
    ) -> [Option<bool>; 3] {
        let mut settings = [None; 3];
        for install in self
            .installs
            .values()
            .filter(|install| install.runtime.profile() == profile)
        {
            let grants = replacement
                .filter(|(owner, _)| *owner == install.runtime)
                .map_or(&install.permissions.grants, |(_, grants)| grants);
            for (setting, candidate) in settings.iter_mut().zip(grants.autofill) {
                if let Some(value) = candidate {
                    *setting = Some(setting.unwrap_or(true) && value);
                }
            }
        }
        settings
    }

    pub(in crate::host) fn apply_autofill(
        &self,
        profile: ProfileId,
        core: &ICoreWebView2,
    ) -> windows::core::Result<()> {
        apply_autofill(core, self.autofill_settings(profile, None))
    }

    pub(super) fn restore_autofill(&self, runtime: ExtensionRuntimeInstance) {
        let profile = runtime.profile();
        let settings = self.autofill_settings(profile, None);
        if settings.iter().any(Option::is_some) {
            if let Some(install) = self.installs.get(&(profile, runtime.install_id())) {
                if apply_autofill(&install.bridge.view.webview(), settings).is_err() {
                    eprintln!("extensions: Windows autofill overrides could not be restored");
                }
            }
        }
    }

    pub(super) fn release_autofill(&self, install: &Install) {
        if install
            .permissions
            .grants
            .autofill
            .iter()
            .any(Option::is_some)
            && self
                .apply_autofill(install.runtime.profile(), &install.bridge.view.webview())
                .is_err()
        {
            eprintln!("extensions: Windows autofill override could not be released");
        }
    }

    fn replace_grants(&mut self, runtime: ExtensionRuntimeInstance, next: Grants) -> bool {
        let profile = runtime.profile();
        let key = (profile, runtime.install_id());
        let previous = self.autofill_settings(profile, None);
        let desired = self.autofill_settings(profile, Some((runtime, &next)));
        let Some(install) = self
            .installs
            .get_mut(&key)
            .filter(|item| item.runtime == runtime)
        else {
            return false;
        };
        let core = install.bridge.view.webview();
        if desired != previous && apply_autofill(&core, desired).is_err() {
            let _ = apply_autofill(&core, previous);
            return false;
        }
        if install.permissions.replace(next) {
            return true;
        }
        if desired != previous {
            let _ = apply_autofill(&core, previous);
        }
        false
    }
}

fn apply_autofill(core: &ICoreWebView2, settings: [Option<bool>; 3]) -> windows::core::Result<()> {
    let native_settings = unsafe { core.Settings()? }.cast::<ICoreWebView2Settings4>()?;
    // WebView2 has a profile-wide general autofill setting shared by addresses
    // and cards, and a separate profile-wide password saving setting.
    unsafe {
        native_settings.SetIsPasswordAutosaveEnabled(settings[0].unwrap_or(true))?;
        native_settings
            .SetIsGeneralAutofillEnabled(settings[1..].iter().all(|value| value.unwrap_or(true)))
    }
}

fn read_autofill(core: &ICoreWebView2, setting: usize) -> windows::core::Result<bool> {
    let settings = unsafe { core.Settings()? }.cast::<ICoreWebView2Settings4>()?;
    let mut value = windows::core::BOOL::default();
    unsafe {
        if setting == 0 {
            settings.IsPasswordAutosaveEnabled(&mut value)?;
        } else {
            settings.IsGeneralAutofillEnabled(&mut value)?;
        }
    }
    Ok(value.as_bool())
}

impl super::super::EngineHost {
    pub(super) fn windows_permission_message(
        &mut self,
        runtime: ExtensionRuntimeInstance,
        value: Value,
    ) {
        let Some(reply) = reply_id(&value) else {
            return;
        };
        let profile = runtime.profile();
        let key = (profile, runtime.install_id());
        if self.windows_view_admission_blocked(profile)
            || self.erasure_tombstones.contains(&profile)
        {
            return;
        }
        if value["kind"] == "permissions-toolbar-request" {
            if let Some((reply, spec)) = parse_message(value) {
                let gesture = self.take_extension_permission_toolbar_gesture(runtime);
                self.open_windows_permission_prompt(runtime, reply, spec, gesture);
            }
            return;
        }
        let Some(install) = self
            .windows_extensions
            .installs
            .get(&key)
            .filter(|install| install.runtime == runtime && install.bridge.alive.get())
        else {
            return;
        };
        let grants = &install.permissions.grants;
        let result = match value["kind"].as_str() {
            Some("permissions-state") => json!(grants.permissions),
            Some("permissions-remove") => {
                let Some((_, spec)) = parse_message(value) else {
                    return;
                };
                let mut next = grants.clone();
                next.permissions = grants.permissions.difference(&spec);
                next.clear_revoked_privacy();
                json!(self.windows_extensions.replace_grants(runtime, next))
            }
            Some("permissions-privacy") if grants.has_privacy() => {
                let Some(setting) = PRIVACY_SETTINGS
                    .iter()
                    .position(|setting| value["setting"] == *setting)
                else {
                    send_result(install, reply, json!(false));
                    return;
                };
                match value["op"].as_str() {
                    Some("get") => {
                        let effective =
                            self.windows_extensions.autofill_settings(profile, None)[setting];
                        let level = match effective {
                            None => "controllable_by_this_extension",
                            owner if owner == grants.autofill[setting] => {
                                "controlled_by_this_extension"
                            }
                            _ => "controlled_by_other_extensions",
                        };
                        match read_autofill(&install.bridge.view.webview(), setting) {
                            Ok(value) => json!({ "value": value, "levelOfControl": level }),
                            Err(_) => json!(false),
                        }
                    }
                    Some("set" | "clear") => {
                        let mut next = grants.clone();
                        next.autofill[setting] = if value["op"] == "set" {
                            let Some(enabled) = value["value"].as_bool() else {
                                send_result(install, reply, json!(false));
                                return;
                            };
                            Some(enabled)
                        } else {
                            None
                        };
                        json!(self.windows_extensions.replace_grants(runtime, next))
                    }
                    _ => json!(false),
                }
            }
            _ => json!(false),
        };
        if let Some(install) = self.windows_extensions.installs.get(&key) {
            send_result(install, reply, result);
        }
    }

    pub(super) fn request_windows_permissions(
        &mut self,
        runtime: ExtensionRuntimeInstance,
        target: &str,
        features: wry::NewWindowFeatures,
    ) {
        if target.len() > MAX_MESSAGE_BYTES {
            return;
        }
        // Parse one fixed fragment field. This URL is never used for navigation.
        let Some(fragment) = target.strip_prefix("about:blank#") else {
            return;
        };
        let Some((_, payload)) = url::form_urlencoded::parse(fragment.as_bytes()).next() else {
            return;
        };
        let Some((reply, spec)) = serde_json::from_str(&payload).ok().and_then(parse_message)
        else {
            return;
        };
        let has_gesture = features.user_initiated
            && self
                .windows_extensions
                .installs
                .get(&(runtime.profile(), runtime.install_id()))
                .is_some_and(|install| {
                    let core = install.bridge.view.webview();
                    if core.as_raw() != features.opener.webview.as_raw() {
                        return false;
                    }
                    let mut source = PWSTR::null();
                    unsafe { core.Source(&mut source) }.is_ok()
                        && take_pwstr(source)
                            == format!(
                                "chrome-extension://{}/{}/host.html",
                                install.id,
                                zephium_webext::windows::HOST_DIRECTORY,
                            )
                });
        self.open_windows_permission_prompt(runtime, reply, spec, has_gesture);
    }

    fn take_extension_permission_toolbar_gesture(&self, runtime: ExtensionRuntimeInstance) -> bool {
        use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
        let profile = runtime.profile();
        let Some(install) = self
            .windows_extensions
            .installs
            .get(&(profile, runtime.install_id()))
            .filter(|install| install.runtime == runtime)
        else {
            return false;
        };
        let mut action = install.action.borrow_mut();
        let Some((tab, expires, _)) = action.open_request else {
            return false;
        };
        if !action.permission_gesture_available || expires <= Instant::now() {
            return false;
        }
        let owner = self
            .extension_browser_surfaces
            .get(&profile)
            .and_then(|surface| {
                surface
                    .windows()
                    .iter()
                    .find(|window| window.active() == Some(tab))
            })
            .and_then(|window| self.stages.get(&window.id()))
            .and_then(|stage| stage.parent_window());
        if owner != Some(unsafe { GetForegroundWindow() }) {
            return false;
        }
        action.permission_gesture_available = false;
        true
    }

    fn open_windows_permission_prompt(
        &mut self,
        runtime: ExtensionRuntimeInstance,
        reply: u64,
        spec: Permissions,
        has_gesture: bool,
    ) {
        let profile = runtime.profile();
        let Some(install) = self
            .windows_extensions
            .installs
            .get(&(profile, runtime.install_id()))
            .filter(|install| install.runtime == runtime && install.bridge.alive.get())
        else {
            return;
        };
        if !has_gesture
            || self.windows_view_admission_blocked(profile)
            || self.erasure_tombstones.contains(&profile)
            || !spec.declared_by(&install.permissions.declared)
        {
            send_result(install, reply, json!(false));
            return;
        }
        let requested = spec.difference(&install.permissions.grants.permissions);
        if requested.is_empty() {
            send_result(install, reply, json!(true));
            return;
        }
        if install
            .permissions
            .grants
            .permissions
            .with_granted(&requested)
            .is_none()
        {
            send_result(install, reply, json!(false));
            return;
        }
        let access = &mut self.windows_extensions.optional_permissions;
        let Some(pending) =
            access
                .requests
                .begin(runtime, reply, requested, Instant::now() + REQUEST_TIMEOUT)
        else {
            send_result(install, reply, json!(false));
            return;
        };
        let number = pending.number;
        let Some(timer) =
            crate::platform::imp::schedule_browser_timeout(REQUEST_TIMEOUT, move || {
                super::super::dispatch::with_extension_permission_timeout(number, move |host| {
                    host.finish_windows_permission(|pending| pending.number == number, false);
                });
            })
        else {
            access.requests.take_if(|pending| pending.number == number);
            send_result(install, reply, json!(false));
            return;
        };
        access.timer = Some(timer);
        let projection = WebExtensionAccessRequest {
            profile,
            request: number,
            extension_id: install.id.clone(),
            warnings: zephium_webext::permissions::access_warnings(
                &pending.requested.permissions,
                &pending.requested.origins,
            ),
            permissions: pending.requested.permissions.clone(),
            patterns: pending.requested.origins.clone(),
        };
        if let Some(popup) = self
            .windows_extensions
            .popup
            .as_ref()
            .filter(|popup| popup.runtime == runtime)
        {
            popup.window.hide_for_permission();
        }
        self.sink
            .emit(EngineEvent::WebExtensionAccessRequested(Box::new(
                projection,
            )));
    }

    pub(crate) fn answer_web_extension_access(
        &mut self,
        profile: ProfileId,
        request: u64,
        allowed: bool,
    ) {
        self.finish_windows_permission(
            |pending| pending.owner.profile() == profile && pending.number == request,
            allowed,
        );
    }

    pub(super) fn finish_windows_permission(
        &mut self,
        matches: impl FnOnce(
            &zephium_webext::windows::permissions::PendingPermission<ExtensionRuntimeInstance>,
        ) -> bool,
        allowed: bool,
    ) {
        let access = &mut self.windows_extensions.optional_permissions;
        let Some(pending) = access.requests.take_if(matches) else {
            return;
        };
        access.timer.take();
        let runtime = pending.owner;
        let profile = runtime.profile();
        let allowed = allowed
            && Instant::now() < pending.deadline
            && !self.windows_view_admission_blocked(profile)
            && !self.erasure_tombstones.contains(&profile);
        if let Some(install) = self
            .windows_extensions
            .installs
            .get_mut(&(profile, runtime.install_id()))
            .filter(|install| install.runtime == runtime && install.bridge.alive.get())
        {
            let mut next = install.permissions.grants.clone();
            let accepted = allowed
                && next
                    .permissions
                    .with_granted(&pending.requested)
                    .is_some_and(|grants| {
                        next.permissions = grants;
                        install.permissions.replace(next)
                    });
            send_result(install, pending.reply, json!(accepted));
        }
        self.sink.emit(EngineEvent::WebExtensionAccessDismissed {
            profile,
            request: pending.number,
        });
        if let Some(popup) = self
            .windows_extensions
            .popup
            .as_ref()
            .filter(|popup| popup.runtime == runtime && popup.window.permission_pending())
        {
            if popup.window.resume_after_permission() {
                let _ = popup.view.view.focus();
            } else {
                self.close_windows_extension_popup();
            }
        }
    }
}
