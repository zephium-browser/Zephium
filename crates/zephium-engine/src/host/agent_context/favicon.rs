//! A Work page the agent read discovers its icon the way a tab does: one
//! watch of the document's icon links, then the page decodes one same-origin
//! address. Only a fixed 32x32 raster leaves the page, named by the committed
//! URL. Nothing is polled.

use zephium_agentic::{ContextId, ContextNavigationTarget};
use zephium_core::icon::{rank, CandidateReport};
use zephium_core::ports::engine::EngineEvent;

use super::super::favicon::{rasterize, same_address, watch, MAX_WATCH_RUNS};
use super::super::EngineHost;

/// Same-origin addresses tried before giving up; a canvas can read only those.
const MAX_ADDRESSES: usize = 3;

/// One discovery for one committed document. Dropping it with the context
/// abandons the watch: its reports find no matching binding.
pub(super) struct AgentFaviconWatch {
    target: ContextNavigationTarget,
    generation: u64,
}

fn discoverable(target: &ContextNavigationTarget) -> bool {
    matches!(target.as_url().scheme(), "http" | "https")
}

impl EngineHost {
    pub(super) fn start_agent_favicon(&mut self, id: ContextId) {
        let Some(binding) = self.agent_contexts.get_mut(&id) else {
            return;
        };
        let generation = binding
            .favicon
            .as_ref()
            .map_or(0, |watch| watch.generation)
            .wrapping_add(1)
            .max(1);
        binding.favicon = binding
            .committed_target
            .clone()
            .filter(discoverable)
            .map(|target| AgentFaviconWatch { target, generation });
        self.agent_favicon_run(id, generation, None, 1);
    }

    /// The watch still belongs to the committed, settled document.
    fn current_agent_favicon(
        &self,
        id: ContextId,
        generation: u64,
    ) -> Option<&ContextNavigationTarget> {
        let binding = self.agent_contexts.get(&id)?;
        let watch = binding.favicon.as_ref()?;
        (!binding.renderer_lost
            && binding.pending_navigation.is_none()
            && binding.committed_target.as_ref() == Some(&watch.target)
            && watch.generation == generation)
            .then_some(&watch.target)
    }

    fn agent_favicon_run(&mut self, id: ContextId, generation: u64, last: Option<String>, run: u8) {
        if self.current_agent_favicon(id, generation).is_none() {
            if let Some(binding) = self.agent_contexts.get_mut(&id) {
                binding.favicon = None;
            }
            return;
        }
        let Some(binding) = self.agent_contexts.get(&id) else {
            return;
        };
        let queued = last.clone();
        let started = watch(binding.view.view(), last.as_deref(), move |report| {
            let _ = crate::host::try_with(move |host| {
                host.agent_favicon_report(id, generation, run, queued, report);
            });
        });
        if !started {
            if let Some(binding) = self.agent_contexts.get_mut(&id) {
                binding.favicon = None;
            }
        }
    }

    fn agent_favicon_report(
        &mut self,
        id: ContextId,
        generation: u64,
        run: u8,
        last: Option<String>,
        report: Option<CandidateReport>,
    ) {
        let Some(target) = self.current_agent_favicon(id, generation).cloned() else {
            return;
        };
        let Some(report) = report else {
            return;
        };
        let url = target.as_url();
        if !same_address(&report.page_url, url.as_str()) {
            if run < MAX_WATCH_RUNS {
                self.agent_favicon_run(id, generation, last, run + 1);
            }
            return;
        }
        let origin = url.origin();
        let addresses: Vec<String> = rank(url, &report.candidates)
            .into_iter()
            .filter(|candidate| {
                url::Url::parse(&candidate.url).is_ok_and(|candidate| candidate.origin() == origin)
            })
            .take(MAX_ADDRESSES)
            .map(|candidate| candidate.url)
            .collect();
        if !report.candidates.is_empty() || report.complete || run >= MAX_WATCH_RUNS {
            self.agent_favicon_decode(id, generation, addresses);
        } else {
            self.agent_favicon_run(id, generation, Some(report.token), run + 1);
        }
    }

    fn agent_favicon_decode(&mut self, id: ContextId, generation: u64, mut addresses: Vec<String>) {
        if self.current_agent_favicon(id, generation).is_none() || addresses.is_empty() {
            if let Some(binding) = self.agent_contexts.get_mut(&id) {
                binding.favicon = None;
            }
            return;
        }
        let address = addresses.remove(0);
        let Some(binding) = self.agent_contexts.get(&id) else {
            return;
        };
        let started = rasterize(binding.view.view(), &address, move |rgba| {
            let _ = crate::host::try_with(move |host| {
                host.agent_favicon_decoded(id, generation, addresses, rgba);
            });
        });
        if !started {
            if let Some(binding) = self.agent_contexts.get_mut(&id) {
                binding.favicon = None;
            }
        }
    }

    fn agent_favicon_decoded(
        &mut self,
        id: ContextId,
        generation: u64,
        remaining: Vec<String>,
        rgba: Option<Vec<u8>>,
    ) {
        let Some(target) = self.current_agent_favicon(id, generation).cloned() else {
            return;
        };
        match rgba {
            Some(rgba) => self.accept_agent_favicon(id, &target, rgba),
            None => self.agent_favicon_decode(id, generation, remaining),
        }
    }

    /// The document that answered must still be the committed one, so a
    /// late raster never lands under a later page's origin.
    fn accept_agent_favicon(
        &mut self,
        id: ContextId,
        target: &ContextNavigationTarget,
        rgba: Vec<u8>,
    ) {
        let Some(binding) = self.agent_contexts.get_mut(&id) else {
            return;
        };
        if binding.committed_target.as_ref() != Some(target) {
            return;
        }
        binding.favicon = None;
        let profile = binding.profile();
        self.sink.emit(EngineEvent::WorkPageFavicon {
            profile,
            page_url: target.as_url().as_str().to_owned(),
            rgba,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_is_only_for_web_documents() {
        assert!(discoverable(
            &ContextNavigationTarget::parse("https://example.com/a").unwrap()
        ));
    }
}
