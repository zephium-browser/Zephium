//! One attempt's screenshot questions. Only the step driver may resolve them.
use super::sites::OriginEpoch;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
#[cfg(any(target_os = "macos", windows, test))]
use zephium_core::work::WorkError;
use zephium_core::{ids::ProfileId, work::model::WorkModelImageFence, work::WorkStepId};

#[cfg(any(target_os = "macos", windows, test))]
const MAX_RUN_ORIGINS: usize = 16;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScreenshotChoice {
    Checking,
    Asking,
    Allowed,
    Denied,
}
#[derive(Clone, Copy)]
pub(crate) enum ScreenshotAnswer {
    Allow,
    Always,
    NotNow,
}
struct State {
    choice: ScreenshotChoice,
    claimed: bool,
    remembering: bool,
    question: Option<WorkStepId>,
    epoch: OriginEpoch,
}
struct Choice {
    origin: String,
    profile: ProfileId,
    page: WorkStepId,
    global: Arc<dyn WorkModelImageFence>,
    state: Mutex<State>,
}
/// Private ticket: neither model-written Ask steps nor an arbitrary step ID can
/// create or settle a runtime screenshot choice.
#[derive(Clone)]
pub(crate) struct QuestionTicket(Arc<Choice>);
pub(crate) struct RunScreenshots {
    #[cfg(any(target_os = "macos", windows, test))]
    profile: ProfileId,
    #[cfg(any(target_os = "macos", windows, test))]
    global: zephium_agentic::lead::ModelImageGate,
    entries: Mutex<BTreeMap<String, Arc<Choice>>>,
    /// Sites the person let this run work in as them: what the run sees
    /// there is theirs to show it, screenshots included, without asking again.
    granted: Mutex<Vec<String>>,
}
impl RunScreenshots {
    pub(crate) fn new(profile: ProfileId) -> Self {
        #[cfg(not(any(target_os = "macos", windows, test)))]
        let _ = profile;
        Self {
            #[cfg(any(target_os = "macos", windows, test))]
            profile,
            #[cfg(any(target_os = "macos", windows, test))]
            global: super::gate(),
            entries: Mutex::new(BTreeMap::new()),
            granted: Mutex::new(Vec::new()),
        }
    }
    pub(crate) fn grant_site(&self, site: &str) {
        if let Ok(mut granted) = self.granted.lock() {
            if !granted.iter().any(|known| known == site) {
                granted.push(site.to_owned());
            }
        }
    }
    #[cfg(any(target_os = "macos", windows, test))]
    fn granted(&self, origin: &str) -> bool {
        crate::work_sites::site_of(origin).is_some_and(|site| {
            self.granted
                .lock()
                .is_ok_and(|granted| granted.contains(&site))
        })
    }
    #[cfg(any(target_os = "macos", windows, test))]
    pub(crate) fn request(
        &self,
        origin: &str,
        page: WorkStepId,
    ) -> Result<ScreenshotChoice, WorkError> {
        zephium_core::work::screenshots::validate_origin(origin)?;
        let mut entries = self.entries.lock().map_err(|_| WorkError::Unavailable)?;
        if let Some(choice) = entries.get(origin) {
            return Ok(choice.current());
        }
        if entries.len() >= MAX_RUN_ORIGINS {
            return Err(WorkError::Capacity);
        }
        let global = self.global.fence().ok_or(WorkError::Unavailable)?;
        let epoch = super::sites::origin_epoch(self.profile, origin)?;
        let choice = if self.granted(origin) {
            ScreenshotChoice::Allowed
        } else {
            ScreenshotChoice::Checking
        };
        entries.insert(
            origin.into(),
            Arc::new(Choice {
                origin: origin.into(),
                profile: self.profile,
                page,
                global,
                state: Mutex::new(State {
                    choice,
                    claimed: false,
                    remembering: false,
                    question: None,
                    epoch,
                }),
            }),
        );
        Ok(choice)
    }
    #[cfg(any(target_os = "macos", windows, test))]
    pub(crate) fn choice(&self, origin: &str) -> Option<ScreenshotChoice> {
        self.entries
            .lock()
            .ok()?
            .get(origin)
            .map(|choice| choice.current())
    }
    /// Claims each Store lookup once, before awaiting any queue or user input.
    pub(crate) fn take_check(&self, pages: &[WorkStepId]) -> Option<QuestionTicket> {
        self.take(ScreenshotChoice::Checking, pages)
    }
    /// Claims the one durable question for this origin before BeginStep.
    pub(crate) fn take_question(&self, pages: &[WorkStepId]) -> Option<QuestionTicket> {
        self.take(ScreenshotChoice::Asking, pages)
    }
    fn take(&self, desired: ScreenshotChoice, pages: &[WorkStepId]) -> Option<QuestionTicket> {
        let entries = self.entries.lock().ok()?;
        for choice in entries
            .values()
            .filter(|choice| pages.contains(&choice.page))
        {
            let mut state = choice.state.lock().ok()?;
            if state.choice == desired && !state.claimed {
                state.claimed = true;
                return Some(QuestionTicket(choice.clone()));
            }
        }
        None
    }
    pub(crate) fn questions(&self, pages: &[WorkStepId]) -> Vec<QuestionTicket> {
        self.entries
            .lock()
            .map(|entries| {
                entries
                    .values()
                    .filter(|choice| pages.contains(&choice.page))
                    .filter(|choice| {
                        choice
                            .state
                            .lock()
                            .is_ok_and(|state| state.question.is_some())
                    })
                    .cloned()
                    .map(QuestionTicket)
                    .collect()
            })
            .unwrap_or_default()
    }
    pub(crate) fn cancel_page(&self, page: WorkStepId) {
        if let Ok(entries) = self.entries.lock() {
            for choice in entries.values().filter(|choice| choice.page == page) {
                if let Ok(mut state) = choice.state.lock() {
                    if matches!(
                        state.choice,
                        ScreenshotChoice::Checking | ScreenshotChoice::Asking
                    ) {
                        state.choice = ScreenshotChoice::Denied;
                    }
                }
            }
        }
    }
    #[cfg(any(target_os = "macos", windows, test))]
    pub(super) fn permission(
        &self,
        origin: &str,
    ) -> Option<(Arc<dyn WorkModelImageFence>, OriginEpoch)> {
        let choice = self.entries.lock().ok()?.get(origin)?.clone();
        if choice.current() != ScreenshotChoice::Allowed {
            return None;
        }
        let epoch = choice.state.lock().ok()?.epoch.clone();
        Some((choice.global.clone(), epoch))
    }
}
impl Choice {
    fn current(&self) -> ScreenshotChoice {
        let Ok(mut state) = self.state.lock() else {
            return ScreenshotChoice::Denied;
        };
        if !self.global.while_current(&mut || {}) || (!state.remembering && !state.epoch.current())
        {
            state.choice = ScreenshotChoice::Denied;
        }
        state.choice
    }
}
impl QuestionTicket {
    pub(crate) fn current(&self) -> ScreenshotChoice {
        self.0.current()
    }
    pub(crate) fn question_closed(&self) {
        if let Ok(mut state) = self.0.state.lock() {
            state.question = None;
        }
    }
    pub(crate) fn origin(&self) -> &str {
        &self.0.origin
    }
    #[cfg(test)]
    pub(crate) fn page(&self) -> WorkStepId {
        self.0.page
    }
    pub(crate) fn step(&self) -> Option<WorkStepId> {
        self.0.state.lock().ok()?.question
    }
    pub(crate) fn bind(&self, step: WorkStepId) -> bool {
        let Ok(mut state) = self.0.state.lock() else {
            return false;
        };
        if state.choice != ScreenshotChoice::Asking || !state.claimed || state.question.is_some() {
            return false;
        }
        state.question = Some(step);
        true
    }
    pub(crate) fn deny(&self) {
        if let Ok(mut state) = self.0.state.lock() {
            state.choice = ScreenshotChoice::Denied;
        }
    }
    pub(crate) async fn check(&self, handle: &crate::Handle) {
        let records = super::sites::read_records(handle, self.0.profile).await;
        let Ok(mut state) = self.0.state.lock() else {
            return;
        };
        if state.choice != ScreenshotChoice::Checking || !state.claimed || state.question.is_some()
        {
            return;
        }
        state.choice = match records {
            Ok(records) if self.0.global.while_current(&mut || {}) && state.epoch.current() => {
                if records.iter().any(|record| record.origin == self.0.origin) {
                    ScreenshotChoice::Allowed
                } else {
                    ScreenshotChoice::Asking
                }
            }
            _ => ScreenshotChoice::Denied,
        };
        state.claimed = false;
    }
    pub(crate) async fn answer(
        &self,
        handle: &crate::Handle,
        step: WorkStepId,
        answer: ScreenshotAnswer,
    ) {
        {
            let Ok(mut state) = self.0.state.lock() else {
                return;
            };
            if state.choice != ScreenshotChoice::Asking || state.question != Some(step) {
                return;
            }
            if !self.0.global.while_current(&mut || {})
                || !state.epoch.current()
                || matches!(answer, ScreenshotAnswer::NotNow)
            {
                state.choice = ScreenshotChoice::Denied;
                return;
            }
            // No duplicate/late answer can take this original pending ticket.
            state.choice = ScreenshotChoice::Checking;
            state.remembering = matches!(answer, ScreenshotAnswer::Always);
        }
        let epoch = if matches!(answer, ScreenshotAnswer::Always) {
            let expected = self.0.state.lock().ok().map(|state| state.epoch.clone());
            match expected {
                Some(expected) => super::sites::remember_for_question(
                    handle,
                    self.0.profile,
                    self.0.origin.clone(),
                    expected,
                )
                .await
                .ok(),
                None => None,
            }
        } else {
            self.0.state.lock().ok().map(|state| state.epoch.clone())
        };
        let Ok(mut state) = self.0.state.lock() else {
            return;
        };
        if state.choice != ScreenshotChoice::Checking || state.question != Some(step) {
            return;
        }
        state.remembering = false;
        if let Some(epoch) =
            epoch.filter(|epoch| epoch.current() && self.0.global.while_current(&mut || {}))
        {
            state.epoch = epoch;
            state.choice = ScreenshotChoice::Allowed;
        } else {
            state.choice = ScreenshotChoice::Denied;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn run(id: u128) -> RunScreenshots {
        RunScreenshots {
            profile: id.into(),
            global: zephium_agentic::lead::ModelImageGate::new(Some(true)),
            entries: Mutex::new(BTreeMap::new()),
            granted: Mutex::new(Vec::new()),
        }
    }
    #[test]
    fn a_site_the_person_let_the_run_work_in_needs_no_screenshot_question() {
        let run = run(99002);
        run.grant_site("linear.app");
        assert_eq!(
            run.request("https://linear.app", 1u128.into()).unwrap(),
            ScreenshotChoice::Allowed
        );
        assert_eq!(
            run.request("https://example.test", 2u128.into()).unwrap(),
            ScreenshotChoice::Checking
        );
    }
    #[test]
    fn parallel_same_origin_has_one_lookup_and_one_host_question() {
        let run = run(99001);
        let origin = "https://app.example.test";
        assert_eq!(
            run.request(origin, 1u128.into()).unwrap(),
            ScreenshotChoice::Checking
        );
        assert_eq!(
            run.request(origin, 2u128.into()).unwrap(),
            ScreenshotChoice::Checking
        );
        let check = run.take_check(&[1u128.into(), 2u128.into()]).unwrap();
        assert!(run.take_check(&[1u128.into(), 2u128.into()]).is_none());
        {
            let mut state = check.0.state.lock().unwrap();
            state.choice = ScreenshotChoice::Asking;
            state.claimed = false;
        }
        let question = run.take_question(&[1u128.into(), 2u128.into()]).unwrap();
        assert!(run.take_question(&[1u128.into(), 2u128.into()]).is_none());
        assert_eq!(question.page(), WorkStepId::from(1u128));
        assert!(question.bind(3u128.into()));
        assert!(!question.bind(4u128.into()));
        assert!(run.permission(origin).is_none());
        run.cancel_page(1u128.into());
        assert_eq!(
            run.request(origin, 2u128.into()).unwrap(),
            ScreenshotChoice::Denied
        );
    }
    #[test]
    fn global_off_on_never_revives_a_pending_origin_choice() {
        let run = run(99002);
        let origin = "https://example.test";
        run.request(origin, 1u128.into()).unwrap();
        let old = run.take_check(&[1u128.into(), 2u128.into()]).unwrap();
        let epoch = run.global.begin_change().unwrap();
        run.global.confirm(epoch, true);
        assert_eq!(run.choice(origin), Some(ScreenshotChoice::Denied));
        assert_eq!(old.0.current(), ScreenshotChoice::Denied);
        assert_eq!(
            run.request(origin, 2u128.into()).unwrap(),
            ScreenshotChoice::Denied
        );
        assert!(run.permission(origin).is_none());
    }
    #[test]
    fn different_origins_are_separate_and_bounded() {
        let run = run(99003);
        for index in 0..MAX_RUN_ORIGINS {
            run.request(&format!("https://site{index}.example.test"), 1u128.into())
                .unwrap();
        }
        assert!(matches!(
            run.request("https://overflow.example.test", 1u128.into()),
            Err(WorkError::Capacity)
        ));
        assert!(run.permission("https://unseen.example.test").is_none());
    }
}
