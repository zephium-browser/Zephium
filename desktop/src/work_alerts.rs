//! A Work page waiting on the person says so outside the window: a count on
//! the Dock or taskbar while it waits and, while Zephium is not in front, one
//! system notification per wait. Nothing leaves the computer.
use super::WorkProductState;
use std::{collections::BTreeSet, sync::Mutex};
use tauri::Manager;
use zephium_ipc::work::WorkHumanReasonV1;
use zephium_work_composition::WaitingHumanPage;

#[derive(Default)]
pub(crate) struct WorkAlerts {
    /// Waits already announced, so one wait is announced once.
    told: Mutex<BTreeSet<String>>,
}

impl WorkAlerts {
    /// Called on every held-page change: the Dock count follows, and a new
    /// wait is announced if nobody is looking at Zephium.
    pub(crate) fn refresh(&self, app: &tauri::AppHandle) {
        let Some(owner) = app.try_state::<WorkProductState>() else {
            return;
        };
        let waiting = owner.providers.browser.pages_waiting_for_person();
        if let Some(window) = app.get_webview_window(crate::MAIN_LABEL) {
            #[cfg(target_os = "windows")]
            crate::taskbar_badge::show(&window, waiting.len());
            #[cfg(not(target_os = "windows"))]
            {
                let count = i64::try_from(waiting.len()).unwrap_or(i64::MAX);
                let _ = window.set_badge_count((count > 0).then_some(count));
            }
        }
        let Ok(mut told) = self.told.lock() else {
            return;
        };
        told.retain(|key| waiting.iter().any(|page| page.key == *key));
        if waiting.is_empty() {
            return;
        }
        crate::focus_alerts::ask_to_notify();
        if crate::focus_alerts::in_front(app) {
            return;
        }
        for page in &waiting {
            if !told.insert(page.key.clone()) {
                continue;
            }
            let (title, body) = words(page);
            crate::focus_alerts::notify(&format!("app.zephium.work.{}", page.key), &title, &body);
            #[cfg(target_os = "windows")]
            if let Some(window) = app.get_webview_window(crate::MAIN_LABEL) {
                let _ =
                    window.request_user_attention(Some(tauri::UserAttentionType::Informational));
            }
        }
    }
}

/// "Work needs you", then what to do and where, as the canvas says it.
fn words(page: &WaitingHumanPage) -> (String, String) {
    let site = page.site.as_deref();
    let body = match (page.reason, site) {
        (WorkHumanReasonV1::Challenge | WorkHumanReasonV1::Verification, Some(site)) => {
            format!("Solve the check on {site} so Work can continue.")
        }
        (WorkHumanReasonV1::SignIn, Some(site)) => {
            format!("Sign in to {site} so Work can continue.")
        }
        (_, Some(site)) => format!("{site} needs you so Work can continue."),
        (_, None) => "A page needs you so Work can continue.".into(),
    };
    ("Work needs you".into(), body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_alert_says_what_to_do_and_where() {
        let page = |reason, site: Option<&str>| WaitingHumanPage {
            key: "k".into(),
            reason,
            site: site.map(str::to_owned),
        };
        assert_eq!(
            words(&page(WorkHumanReasonV1::Challenge, Some("allegro.pl"))).1,
            "Solve the check on allegro.pl so Work can continue."
        );
        assert_eq!(
            words(&page(WorkHumanReasonV1::SignIn, Some("github.com"))).1,
            "Sign in to github.com so Work can continue."
        );
        assert_eq!(
            words(&page(WorkHumanReasonV1::Permission, None)).1,
            "A page needs you so Work can continue."
        );
    }
}
