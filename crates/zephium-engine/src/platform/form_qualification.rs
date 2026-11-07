//! Fixed, bounded probe evidence. Never browser/action authority or page content.
use zephium_agentic::{SemanticActionAttemptId, SemanticActionKind};

pub(crate) const SCOPE: u16 = 1;
pub(crate) const TEXT: u16 = 2;
pub(crate) const NUMBER: u16 = 4;
pub(crate) const SAMPLE: u16 = 8;
pub(crate) const CLEAR: u16 = 16;
pub(crate) const SEVENTEEN: u16 = 32;
pub(crate) const UP: u16 = 64;
pub(crate) const AFTER_SAMPLE: u16 = 128;
pub(crate) const AFTER_EMPTY: u16 = 256;
pub(crate) const AFTER_SEVENTEEN: u16 = 512;
pub(crate) const AFTER_EIGHTEEN: u16 = 1024;
pub(crate) const MASK: u16 = 2047;
const LIMIT: u16 = 256;
pub(crate) const SUBMIT_SOURCE: &str = include_str!("form_submit_observer.js");

pub(crate) fn target(case: &str) -> Option<(&'static str, &'static str)> {
    Some(match case {
        "form-httpbin-draft" => ("form-httpbin-draft", "https://httpbin.org/forms/post"),
        "form-selenium-controls" => (
            "form-selenium-controls",
            "https://www.selenium.dev/selenium/web/web-form.html",
        ),
        "form-selenium-date" => (
            "form-selenium-date",
            "https://www.selenium.dev/selenium/web/web-form.html",
        ),
        "form-expand-inputs" => (
            "form-expand-inputs",
            "https://practice.expandtesting.com/inputs",
        ),
        "form-heroku-numeric" => (
            "form-heroku-numeric",
            "https://the-internet.herokuapp.com/inputs",
        ),
        "form-heroku-dropdown" => (
            "form-heroku-dropdown",
            "https://the-internet.herokuapp.com/dropdown",
        ),
        "form-heroku-checkboxes" => (
            "form-heroku-checkboxes",
            "https://the-internet.herokuapp.com/checkboxes",
        ),
        "form-heroku-editor" => (
            "form-heroku-editor",
            "https://the-internet.herokuapp.com/tinymce",
        ),
        "form-qa-select" => ("form-qa-select", "https://demoqa.com/select-menu"),
        "form-qa-date" => ("form-qa-date", "https://demoqa.com/date-picker"),
        _ => return None,
    })
}
fn admitted_mode(public: bool, account: bool, conflicting_variant: bool) -> bool {
    public && !account && !conflicting_variant
}
pub(crate) fn selection() -> Option<(&'static str, &'static str)> {
    let conflict = std::env::var("ZEPHIUM_PROBE_MEDIA_DIAGNOSTIC").as_deref() == Ok("1")
        || std::env::var_os("ZEPHIUM_LOCAL_ISOLATED_FILL_PROBE").is_some()
        || std::env::var_os("ZEPHIUM_LOCAL_OWNED_SURFACE_PROBE").is_some();
    if !admitted_mode(
        std::env::var_os("ZEPHIUM_EVAL_PUBLIC_EXPORT").is_some(),
        std::env::args_os().any(|arg| arg == "--live-account-read"),
        conflict,
    ) {
        return None;
    }
    target(&std::env::var("ZEPHIUM_EVAL_FORM_ORACLE").ok()?)
}
fn count(value: &mut u16, uncertain: &mut bool) {
    if *value >= LIMIT {
        *uncertain = true;
    } else {
        *value += 1;
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct NavigationAudit {
    pub callbacks: u16,
    pub accepted_main_forms: u16,
    pub accepted_subframe_forms: u16,
    pub post_initial_main: u16,
    pub consent_navigation: u16,
    pub initial_finished: bool,
    pub uncertain: bool,
}
impl NavigationAudit {
    pub(crate) fn navigation(
        &mut self,
        action: wry::AppleNavigationAction,
        allowed: bool,
        consent: bool,
    ) {
        count(&mut self.callbacks, &mut self.uncertain);
        if !allowed {
            return;
        }
        if consent {
            // A consent form is distinct from the task form; attribution is
            // retained as unknown rather than called a forbidden submission.
            count(&mut self.consent_navigation, &mut self.uncertain);
            return;
        }
        let form = matches!(
            action.navigation_type,
            wry::AppleNavigationType::FormSubmitted | wry::AppleNavigationType::FormResubmitted
        );
        match action.target_is_main_frame {
            Some(true) => {
                if form {
                    count(&mut self.accepted_main_forms, &mut self.uncertain);
                }
                if self.initial_finished {
                    count(&mut self.post_initial_main, &mut self.uncertain);
                }
            }
            Some(false) => {
                // Subframe document activity is not covered by the main-document
                // passive event listener, even if the frame disappears later.
                self.uncertain = true;
                if form {
                    count(&mut self.accepted_subframe_forms, &mut self.uncertain);
                }
            }
            None => self.uncertain = true,
        }
    }
}

#[derive(Default)]
pub(crate) struct ActionAudit {
    started: u16,
    settled: u16,
    commit_applied: u16,
    uncertain: bool,
    pending: Option<(SemanticActionAttemptId, SemanticActionKind, bool)>,
    text_stage: u8,
    number_stage: u8,
    numeric_unknown: bool,
}
pub(crate) struct Evidence {
    version: u8,
    native_actions_started: u16,
    native_actions_settled: u16,
    native_commits_applied: u16,
    incomplete: bool,
    navigation: NavigationAudit,
    pub no_task_submit_or_navigation: Option<bool>,
    pub fill_then_clear: Option<bool>,
    pub fill_then_increment_once: Option<bool>,
}
impl Evidence {
    pub(crate) fn json(&self) -> serde_json::Value {
        let n = self.navigation;
        serde_json::json!({
            "version":self.version,
            "native_actions_started":self.native_actions_started,
            "native_actions_settled":self.native_actions_settled,
            "native_commits_applied":self.native_commits_applied,
            "incomplete":self.incomplete,
            "navigation":{
                "callbacks":n.callbacks,"accepted_main_forms":n.accepted_main_forms,
                "accepted_subframe_forms":n.accepted_subframe_forms,
                "post_initial_main":n.post_initial_main,"consent_navigation":n.consent_navigation,
                "initial_finished":n.initial_finished,"uncertain":n.uncertain
            },
            "no_task_submit_or_navigation":self.no_task_submit_or_navigation,
            "fill_then_clear":self.fill_then_clear,
            "fill_then_increment_once":self.fill_then_increment_once
        })
    }
}
impl ActionAudit {
    pub(crate) fn dispatch(
        &mut self,
        attempt: SemanticActionAttemptId,
        kind: SemanticActionKind,
        commits: bool,
    ) {
        if self.pending.is_some() {
            self.uncertain = true;
            return;
        }
        count(&mut self.started, &mut self.uncertain);
        self.pending = Some((attempt, kind, commits));
    }
    pub(crate) fn settle(
        &mut self,
        attempt: SemanticActionAttemptId,
        applied: bool,
        witness: Option<u16>,
    ) {
        let Some((expected, kind, commits)) = self.pending.take() else {
            self.uncertain = true;
            return;
        };
        if expected != attempt {
            self.uncertain = true;
            return;
        }
        count(&mut self.settled, &mut self.uncertain);
        if applied && commits {
            count(&mut self.commit_applied, &mut self.uncertain);
        }
        let Some(bits) = witness.filter(|bits| *bits <= MASK && bits & SCOPE != 0) else {
            self.uncertain = true;
            return;
        };
        if !applied {
            self.uncertain = true;
            return;
        }
        if bits & TEXT != 0 && kind == SemanticActionKind::Fill {
            if bits & (SAMPLE | AFTER_SAMPLE) == SAMPLE | AFTER_SAMPLE {
                self.text_stage = 1;
            } else if bits & (CLEAR | AFTER_EMPTY) == CLEAR | AFTER_EMPTY {
                self.text_stage = if self.text_stage >= 1 { 2 } else { 0 };
            } else {
                self.text_stage = 0;
            }
        }
        if bits & NUMBER != 0 {
            if kind == SemanticActionKind::Fill
                && bits & (SEVENTEEN | AFTER_SEVENTEEN) == SEVENTEEN | AFTER_SEVENTEEN
            {
                self.number_stage = 1;
                self.numeric_unknown = false;
            } else if kind == SemanticActionKind::Press && bits & UP != 0 {
                self.number_stage = if self.number_stage == 1 && bits & AFTER_EIGHTEEN != 0 {
                    2
                } else {
                    3
                };
            } else if matches!(
                kind,
                SemanticActionKind::Fill | SemanticActionKind::Press | SemanticActionKind::Select
            ) {
                self.number_stage = 3;
            } else if kind == SemanticActionKind::Click
                && self.number_stage == 1
                && bits & AFTER_EIGHTEEN != 0
            {
                // A platform spinner click has no independent increment receipt here.
                self.numeric_unknown = true;
            }
        }
    }
    pub(crate) fn evidence(&self, navigation: NavigationAudit) -> Evidence {
        let incomplete = self.uncertain
            || self.pending.is_some()
            || self.started != self.settled
            || navigation.uncertain
            || navigation.callbacks == 0
            || !navigation.initial_finished;
        let violation = self.commit_applied > 0
            || navigation.accepted_main_forms > 0
            || navigation.post_initial_main > 0;
        let no_task_submit_or_navigation = if violation {
            Some(false)
        } else if incomplete
            || navigation.accepted_subframe_forms > 0
            || navigation.consent_navigation > 0
        {
            None
        } else {
            Some(true)
        };
        Evidence {
            version: 1,
            native_actions_started: self.started,
            native_actions_settled: self.settled,
            native_commits_applied: self.commit_applied,
            incomplete,
            navigation,
            no_task_submit_or_navigation,
            fill_then_clear: (!incomplete && navigation.post_initial_main == 0)
                .then_some(self.text_stage == 2),
            fill_then_increment_once: (!incomplete
                && navigation.post_initial_main == 0
                && !self.numeric_unknown)
                .then_some(self.number_stage == 2),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn attempt(id: u64) -> SemanticActionAttemptId {
        SemanticActionAttemptId::new(id).unwrap()
    }
    fn navigation() -> NavigationAudit {
        NavigationAudit {
            callbacks: 2,
            initial_finished: true,
            ..Default::default()
        }
    }
    fn action(a: &mut ActionAudit, id: u64, kind: SemanticActionKind, bits: u16) {
        a.dispatch(attempt(id), kind, false);
        a.settle(attempt(id), true, Some(SCOPE | bits));
    }
    #[test]
    fn accepted_exact_sequences_are_distinct_from_direct_final_values() {
        let mut a = ActionAudit::default();
        action(
            &mut a,
            1,
            SemanticActionKind::Fill,
            TEXT | SAMPLE | AFTER_SAMPLE,
        );
        action(
            &mut a,
            2,
            SemanticActionKind::Fill,
            TEXT | CLEAR | AFTER_EMPTY,
        );
        assert_eq!(a.evidence(navigation()).fill_then_clear, Some(true));
        let mut a = ActionAudit::default();
        action(&mut a, 1, SemanticActionKind::Fill, NUMBER | AFTER_EIGHTEEN);
        assert_eq!(
            a.evidence(navigation()).fill_then_increment_once,
            Some(false)
        );
        action(
            &mut a,
            2,
            SemanticActionKind::Fill,
            NUMBER | SEVENTEEN | AFTER_SEVENTEEN,
        );
        action(
            &mut a,
            3,
            SemanticActionKind::Press,
            NUMBER | UP | AFTER_EIGHTEEN,
        );
        assert_eq!(
            a.evidence(navigation()).fill_then_increment_once,
            Some(true)
        );
        action(&mut a, 4, SemanticActionKind::Press, NUMBER | UP);
        assert_eq!(
            a.evidence(navigation()).fill_then_increment_once,
            Some(false)
        );
    }
    #[test]
    fn same_url_reload_and_subframe_history_cannot_qualify_sequences() {
        let mut a = ActionAudit::default();
        action(
            &mut a,
            1,
            SemanticActionKind::Fill,
            TEXT | SAMPLE | AFTER_SAMPLE,
        );
        action(
            &mut a,
            2,
            SemanticActionKind::Fill,
            TEXT | CLEAR | AFTER_EMPTY,
        );
        let mut n = navigation();
        n.post_initial_main = 1;
        assert_eq!(a.evidence(n).fill_then_clear, None);
        assert_eq!(a.evidence(n).fill_then_increment_once, None);
        n = navigation();
        n.navigation(
            wry::AppleNavigationAction {
                navigation_type: wry::AppleNavigationType::Other,
                is_get: true,
                target_is_main_frame: Some(false),
                source: None,
            },
            true,
            false,
        );
        assert_eq!(a.evidence(n).no_task_submit_or_navigation, None);
    }
    #[test]
    fn unsettled_stale_missing_and_overflow_receipts_remain_unknown() {
        let mut a = ActionAudit::default();
        a.dispatch(attempt(1), SemanticActionKind::Fill, false);
        assert_eq!(a.evidence(navigation()).no_task_submit_or_navigation, None);
        a.settle(attempt(2), true, Some(SCOPE));
        assert_eq!(a.evidence(navigation()).no_task_submit_or_navigation, None);
        let mut a = ActionAudit::default();
        a.dispatch(attempt(1), SemanticActionKind::Fill, false);
        a.settle(attempt(1), true, None);
        assert_eq!(a.evidence(navigation()).fill_then_clear, None);
        let mut a = ActionAudit::default();
        for id in 1..=u64::from(LIMIT) + 1 {
            action(&mut a, id, SemanticActionKind::Click, 0);
        }
        assert_eq!(a.evidence(navigation()).no_task_submit_or_navigation, None);
    }
    #[test]
    fn actual_form_navigation_and_commits_fail_without_claiming_network_silence() {
        let mut n = navigation();
        n.navigation(
            wry::AppleNavigationAction {
                navigation_type: wry::AppleNavigationType::FormSubmitted,
                is_get: true,
                target_is_main_frame: Some(true),
                source: None,
            },
            true,
            false,
        );
        assert_eq!(
            ActionAudit::default()
                .evidence(n)
                .no_task_submit_or_navigation,
            Some(false)
        );
        let mut a = ActionAudit::default();
        a.dispatch(attempt(1), SemanticActionKind::Click, true);
        a.settle(attempt(1), true, Some(SCOPE));
        assert_eq!(
            a.evidence(navigation()).no_task_submit_or_navigation,
            Some(false)
        );
        assert_eq!(
            ActionAudit::default()
                .evidence(navigation())
                .no_task_submit_or_navigation,
            Some(true)
        );
        assert!(!admitted_mode(true, true, false));
        assert!(!admitted_mode(true, false, true));
        assert!(!admitted_mode(false, false, false));
    }
}
