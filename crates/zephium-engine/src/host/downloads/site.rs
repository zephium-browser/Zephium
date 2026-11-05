//! Whether a site may save files. A site the person has not answered for
//! waits on a card under the address field of the tab that shows it; one
//! that is allowed still asks about a download nobody clicked; one that is
//! blocked is refused before anything is recorded. Downloads the person
//! starts from browser menus and extensions never come here.

use std::cell::Cell;

use zephium_core::ids::ItemId;
use zephium_core::permissions::{
    download_answer_allows, download_gate, DownloadGate, DownloadPermissionPrompt,
    MAX_DOWNLOAD_SITES,
};
use zephium_core::ports::engine::EngineEvent;

use super::*;
use crate::host::permits::Sink;

/// The tab a page's download is asked about on, and the site it shows.
#[derive(Clone)]
pub(in crate::host) struct SiteAsk {
    pub item: Rc<Cell<ItemId>>,
    pub permit: EventPermit,
    pub sink: Sink,
    pub site: Option<PageOrigin>,
}

/// A transfer that may not choose its destination until the person answers.
pub(super) struct SiteHold {
    ask: SiteAsk,
    site: PageOrigin,
    allowed: bool,
    clicked: bool,
    preferences: Option<DownloadPreferences>,
    request: Option<u32>,
}

pub(super) struct Prompt {
    item: Rc<Cell<ItemId>>,
    shown_on: ItemId,
    permit: EventPermit,
    sink: Sink,
    site: PageOrigin,
    /// Only the site question gathers further downloads; a confirmation of
    /// one unclicked download stands alone.
    gathers: bool,
    transfers: Vec<DownloadId>,
}

/// What admission does with a page's download.
pub(super) enum SiteAdmission {
    Open,
    Hold(SiteHold),
    Refuse,
}

impl Downloads {
    /// Replaces what `profile` keeps per site.
    pub(in crate::host) fn set_sites(&self, profile: ProfileId, sites: Vec<(PageOrigin, bool)>) {
        self.sites.borrow_mut().insert(
            profile,
            sites.into_iter().take(MAX_DOWNLOAD_SITES).collect(),
        );
    }

    /// `ask` is None for a download the person started from the browser
    /// itself. `fallback` names the site when the tab has not committed one.
    pub(super) fn site_admission(
        &self,
        profile: ProfileId,
        ask: Option<SiteAsk>,
        fallback: &PageOrigin,
        clicked: bool,
    ) -> SiteAdmission {
        let Some(ask) = ask else {
            return SiteAdmission::Open;
        };
        let site = ask.site.clone().unwrap_or_else(|| fallback.clone());
        let remembered = self
            .sites
            .borrow()
            .get(&profile)
            .and_then(|sites| sites.get(&site).copied());
        match download_gate(remembered, clicked) {
            DownloadGate::Open => SiteAdmission::Open,
            DownloadGate::Refuse => SiteAdmission::Refuse,
            DownloadGate::Ask => SiteAdmission::Hold(SiteHold {
                ask,
                site,
                allowed: remembered == Some(true),
                clicked,
                preferences: None,
                request: None,
            }),
        }
    }

    /// Chooses the destination, unless the transfer waits for the person.
    pub(super) fn decide(self: &Rc<Self>, id: DownloadId, preferences: DownloadPreferences) {
        if !self.hold_for_site(id, preferences.clone()) {
            self.choose_destination(id, preferences);
        }
    }

    fn hold_for_site(&self, id: DownloadId, preferences: DownloadPreferences) -> bool {
        let mut active = self.active.borrow_mut();
        let Some(transfer) = active.get_mut(&id) else {
            return false;
        };
        let filename = transfer.record.filename.clone();
        let profile = transfer.partition.profile();
        let Some(hold) = transfer.site.as_mut() else {
            return false;
        };
        if hold.request.is_some() {
            return true;
        }
        hold.preferences = Some(preferences);
        let shown_on = hold.ask.item.get();
        let gathers = !hold.allowed;
        let mut prompts = self.prompts.borrow_mut();
        let joined = prompts.iter_mut().find(|(_, prompt)| {
            prompt.gathers
                && gathers
                && prompt.shown_on == shown_on
                && prompt.item.get() == shown_on
                && prompt.site == hold.site
        });
        if let Some((request, prompt)) = joined {
            prompt.transfers.push(id);
            hold.request = Some(*request);
            transfer.deadline =
                DecisionDeadline::new(DecisionPhase::SitePermission, Instant::now());
            drop(prompts);
            drop(active);
            // The list shows the file's own name while it waits.
            self.changed(profile);
            return true;
        }
        if prompts.values().any(|prompt| prompt.shown_on == shown_on) {
            // One question per tab: a page that keeps asking while one waits
            // is not heard.
            hold.request = None;
            drop(prompts);
            drop(active);
            self.cancel(id, None);
            return true;
        }
        let request = self.next_prompt.get().max(1);
        self.next_prompt.set(request.wrapping_add(1).max(1));
        let prompt = DownloadPermissionPrompt {
            request,
            site: hold.site.clone(),
            filename,
            allowed: hold.allowed,
            unclicked: !hold.clicked,
        };
        prompts.insert(
            request,
            Prompt {
                item: hold.ask.item.clone(),
                shown_on,
                permit: hold.ask.permit.clone(),
                sink: hold.ask.sink.clone(),
                site: hold.site.clone(),
                gathers,
                transfers: vec![id],
            },
        );
        hold.request = Some(request);
        transfer.deadline = DecisionDeadline::new(DecisionPhase::SitePermission, Instant::now());
        let (permit, sink) = (hold.ask.permit.clone(), hold.ask.sink.clone());
        drop(prompts);
        drop(active);
        self.changed(profile);
        permit.emit(
            &sink,
            EngineEvent::DownloadPermissionRequested {
                id: shown_on,
                prompt,
            },
        );
        true
    }

    /// The person's answer to `request` on `item`. False when nothing waits
    /// on it there.
    pub(in crate::host) fn answer_site(
        self: &Rc<Self>,
        item: ItemId,
        request: u32,
        allow: bool,
    ) -> bool {
        let prompt = {
            let mut prompts = self.prompts.borrow_mut();
            match prompts.get(&request) {
                Some(prompt) if prompt.shown_on == item && prompt.item.get() == item => {
                    prompts.remove(&request)
                }
                _ => None,
            }
        };
        let Some(prompt) = prompt else {
            return false;
        };
        for id in prompt.transfers {
            let preferences = {
                let mut active = self.active.borrow_mut();
                let Some(transfer) = active.get_mut(&id) else {
                    continue;
                };
                let Some(hold) = transfer.site.take() else {
                    continue;
                };
                let remembered = self
                    .sites
                    .borrow()
                    .get(&transfer.partition.profile())
                    .and_then(|sites| sites.get(&hold.site).copied());
                let expired = transfer.deadline.expired(Instant::now());
                if !download_answer_allows(allow, remembered, expired) {
                    None
                } else {
                    transfer.confirmed = true;
                    transfer.deadline =
                        DecisionDeadline::new(DecisionPhase::Admission, Instant::now());
                    hold.preferences
                }
            };
            match preferences {
                Some(preferences) => self.choose_destination(id, preferences),
                None => self.cancel(id, None),
            }
        }
        true
    }

    /// The transfer stops waiting on its prompt; a prompt nothing waits on
    /// any more is taken down.
    pub(super) fn release_site(&self, id: DownloadId) {
        let request = self
            .active
            .borrow_mut()
            .get_mut(&id)
            .and_then(|transfer| transfer.site.take())
            .and_then(|hold| hold.request);
        let Some(request) = request else {
            return;
        };
        let ended = {
            let mut prompts = self.prompts.borrow_mut();
            let Some(prompt) = prompts.get_mut(&request) else {
                return;
            };
            prompt.transfers.retain(|transfer| *transfer != id);
            if !prompt.transfers.is_empty() {
                return;
            }
            prompts.remove(&request)
        };
        if let Some(prompt) = ended {
            prompt.permit.emit(
                &prompt.sink,
                EngineEvent::DownloadPermissionEnded {
                    id: prompt.shown_on,
                    request,
                },
            );
        }
    }

    /// Whether a transfer waits for the person, which only its page going
    /// away or its deadline ends.
    pub(super) fn waits_for_site(transfer: &Transfer) -> bool {
        transfer
            .site
            .as_ref()
            .is_some_and(|hold| hold.request.is_some())
    }

    /// The page a waiting transfer came from is still the one it was.
    pub(super) fn site_source_current(transfer: &Transfer) -> bool {
        transfer
            .site
            .as_ref()
            .is_some_and(|hold| hold.ask.permit.active_token().is_some())
            && transfer.source.as_ref().is_some_and(Source::current)
    }
}

impl crate::host::EngineHost {
    pub(crate) fn set_download_sites(
        &mut self,
        profile: ProfileId,
        sites: Vec<(PageOrigin, bool)>,
    ) {
        if self.erasure_tombstones.contains(&profile) {
            return;
        }
        if let Some(downloads) = &self.downloads {
            downloads.set_sites(profile, sites);
        }
    }

    pub(crate) fn answer_download_permission(&mut self, item: ItemId, request: u32, allow: bool) {
        if let Some(downloads) = self.downloads.clone() {
            downloads.answer_site(item, request, allow);
        }
    }
}
