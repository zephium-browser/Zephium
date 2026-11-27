//! Host side of the Ask panel's Browse reader: the shell's switch, each
//! view's navigation hooks, and the main-thread entry points the engine
//! handle dispatches to.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;
use std::time::{Duration, Instant};

use zephium_core::ids::ItemId;

use super::EngineHost;
use crate::page_reader::{
    PageActFailure, PageGlow, PageMark, PageOutline, PagePick, PagePickFailure, PageReadFailure,
    PageReaderPolicy, PageReadout,
};
use crate::platform::imp::{schedule_content_policy_timeout, ContentPolicyTimeout};
use crate::platform::page_recipe::{
    mark_outcome, parse_outline, pick_state, placed_marks, MarkOutcome, MarkTarget, PageRecipe,
    PickState, MAX_MARKS,
};

#[cfg(target_os = "macos")]
pub(super) type Reader = crate::platform::macos::browse_reader::BrowseReader;
#[cfg(target_os = "windows")]
pub(super) type Reader = crate::platform::windows::browse_reader::BrowseReader;

/// A view's reader. The view owns the only strong handle; its native
/// callbacks hold weak ones, so the reader goes with the view.
pub(super) type ReaderSlot = Rc<RefCell<Option<Reader>>>;

static POLICY: RwLock<Option<PageReaderPolicy>> = RwLock::new(None);

/// The reader's probe sets the policy the shell would.
#[cfg(feature = "page-reader-probe")]
pub(crate) fn set_policy_for_probe(next: Option<PageReaderPolicy>) {
    *POLICY
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = next;
}

fn policy() -> Option<PageReaderPolicy> {
    POLICY
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

/// Whether the reader is switched on.
pub(super) fn enabled() -> bool {
    policy().is_some()
}

/// Whether a document at `target` may receive the reader's script.
pub(super) fn admits(target: &str) -> bool {
    policy().is_some_and(|policy| !(policy.sensitive)(target))
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(super) fn back_forward(slot: &Weak<RefCell<Option<Reader>>>, back_forward: bool) {
    let Some(slot) = slot.upgrade() else {
        return;
    };
    if let Ok(reader) = slot.try_borrow() {
        if let Some(reader) = reader.as_ref() {
            reader.back_forward(back_forward);
        }
    };
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(super) fn attempt(slot: &Weak<RefCell<Option<Reader>>>, target: &str) {
    let Some(slot) = slot.upgrade() else {
        return;
    };
    let admitted = admits(target);
    if let Ok(reader) = slot.try_borrow() {
        if let Some(reader) = reader.as_ref() {
            reader.attempt(admitted, target);
        }
    };
}

/// A main-frame load begins; macOS learns it from the attempt instead.
#[cfg(target_os = "windows")]
pub(super) fn begin_load(slot: &Weak<RefCell<Option<Reader>>>) {
    let Some(slot) = slot.upgrade() else {
        return;
    };
    if let Ok(reader) = slot.try_borrow() {
        if let Some(reader) = reader.as_ref() {
            reader.begin_load();
        }
    };
}

pub(super) fn commit(slot: &Weak<RefCell<Option<Reader>>>, epoch: u64, url: &str) {
    let Some(slot) = slot.upgrade() else {
        return;
    };
    if let Ok(reader) = slot.try_borrow() {
        if let Some(reader) = reader.as_ref() {
            reader.commit(epoch, url);
        }
    };
}

/// The person used `tab` while steps held it.
pub(crate) fn report_human_input(tab: ItemId) {
    if let Some(policy) = policy() {
        (policy.human_input)(tab);
    }
}

type ReadDone = Box<dyn FnOnce(Result<PageReadout, PageReadFailure>) + Send>;
type RecipeDone = Box<dyn FnOnce(Option<String>) + Send>;
type PickDone = Box<dyn FnOnce(Result<PagePick, PagePickFailure>) + Send>;

/// How often a running pick is asked whether the person picked, and how
/// long it may run at most whatever the page says.
const PICK_POLL: Duration = Duration::from_millis(150);
const PICK_LIMIT: Duration = Duration::from_secs(10 * 60);
const PICK_MISSES: u8 = 3;

/// A pick the person is making on one tab.
struct PickSession {
    pick: u64,
    epoch: u64,
    done: PickDone,
    started: Instant,
    misses: u8,
    timer: Option<ContentPolicyTimeout>,
}

thread_local! {
    static PICKS: RefCell<HashMap<ItemId, PickSession>> = RefCell::new(HashMap::new());
}

static NEXT_PICK: AtomicU64 = AtomicU64::new(1);

/// Ends `tab`'s pick `pick` with `outcome`, when it is still the running one.
fn finish_pick(tab: ItemId, pick: u64, outcome: Result<PagePick, PagePickFailure>) {
    let session = PICKS.with(|picks| {
        let mut picks = picks.borrow_mut();
        picks
            .get(&tab)
            .is_some_and(|session| session.pick == pick)
            .then(|| picks.remove(&tab))
            .flatten()
    });
    if let Some(session) = session {
        (session.done)(outcome);
    }
}

/// Asks again after a short wait, while the pick runs.
fn poll_pick_later(tab: ItemId, pick: u64) {
    let timer = schedule_content_policy_timeout(PICK_POLL, move || {
        let _ = super::try_with(move |host| host.page_pick_poll(tab, pick));
    });
    let Some(timer) = timer else {
        return finish_pick(tab, pick, Err(PagePickFailure::Unavailable));
    };
    PICKS.with(|picks| {
        if let Some(session) = picks
            .borrow_mut()
            .get_mut(&tab)
            .filter(|session| session.pick == pick)
        {
            session.timer = Some(timer);
        }
    });
}

/// The marks recipe for `marks` on read `read` of `tab`'s document at
/// `epoch`, and what it asks the page to find. A read that is gone, or a
/// passage it never had, marks nothing.
pub(crate) fn marks_recipe(
    tab: ItemId,
    epoch: u64,
    read: u64,
    marks: &[PageMark],
    reduce_motion: bool,
) -> Option<(PageRecipe, Vec<MarkTarget>)> {
    let mut targets: Vec<MarkTarget> = Vec::new();
    if let Some(stored) =
        crate::page_reader::planned_read(tab, read).filter(|stored| stored.epoch() == epoch)
    {
        for mark in marks.iter().take(MAX_MARKS) {
            if mark.n == 0 || targets.iter().any(|target| target.n == mark.n) {
                continue;
            }
            if let Some(target) =
                crate::page_reader::read::mark_target(&stored, mark.n, mark.passage)
            {
                targets.push(target);
            }
        }
    }
    let recipe = PageRecipe::marks(&targets, reduce_motion)?;
    Some((recipe, targets))
}

/// One answer about the running pick.
fn pick_answered(tab: ItemId, pick: u64, answer: Option<String>) {
    let Some((epoch, started, misses)) = PICKS.with(|picks| {
        picks
            .borrow()
            .get(&tab)
            .filter(|session| session.pick == pick)
            .map(|session| (session.epoch, session.started, session.misses))
    }) else {
        return;
    };
    let changed = crate::page_reader::committed_epoch(tab) != Some(epoch);
    let state = answer
        .as_deref()
        .and_then(|answer| pick_state(answer, pick));
    match state {
        Some(PickState::Picked(picked)) => finish_pick(tab, pick, Ok(picked)),
        Some(PickState::Cancelled) => finish_pick(tab, pick, Err(PagePickFailure::Cancelled)),
        _ if changed => finish_pick(tab, pick, Err(PagePickFailure::PageChanged)),
        Some(PickState::Gone) => finish_pick(tab, pick, Err(PagePickFailure::Cancelled)),
        Some(PickState::Waiting) if started.elapsed() >= PICK_LIMIT => {
            finish_pick(tab, pick, Err(PagePickFailure::Cancelled));
        }
        Some(PickState::Waiting) => poll_pick_later(tab, pick),
        None if misses + 1 >= PICK_MISSES => {
            finish_pick(tab, pick, Err(PagePickFailure::Unavailable));
        }
        None => {
            PICKS.with(|picks| {
                if let Some(session) = picks.borrow_mut().get_mut(&tab) {
                    session.misses += 1;
                }
            });
            poll_pick_later(tab, pick);
        }
    }
}

impl EngineHost {
    fn with_reader(&self, id: ItemId, apply: impl FnOnce(&Reader)) -> bool {
        let Some(slot) = self.views.get(&id).map(|view| view.page_reader.clone()) else {
            return false;
        };
        let Ok(reader) = slot.try_borrow() else {
            return false;
        };
        match reader.as_ref() {
            Some(reader) => {
                apply(reader);
                true
            }
            None => false,
        }
    }

    /// Switches the reader on or off for every view. Off removes every
    /// script, handler and channel; on prepares the next document of each
    /// view, while documents already open stay unread.
    pub(crate) fn set_page_reader(&mut self, next: Option<PageReaderPolicy>) {
        let on = next.is_some();
        *POLICY
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = next;
        for view in self.views.values() {
            if let Ok(reader) = view.page_reader.try_borrow() {
                if let Some(reader) = reader.as_ref() {
                    reader.set_enabled(on);
                }
            }
        }
        if !on {
            crate::platform::imp::page_glow::clear();
        }
    }

    /// The working glow on `id`'s page: only where the assistant may act
    /// (AI on, not a private window); anything else turns it off.
    pub(crate) fn page_glow(&mut self, id: ItemId, glow: PageGlow, reduce_motion: bool) {
        let state = glow.name();
        let Some(view) = self.views.get(&id) else {
            crate::page_reader::note(format_args!(
                "page_glow refused tab={id} state={state} reason=no-view"
            ));
            crate::platform::imp::page_glow::forget(id);
            return;
        };
        let refused = if !enabled() {
            Some("ai-off")
        } else {
            match view.page_reader.try_borrow() {
                Ok(reader) if reader.is_some() => None,
                Ok(_) => Some("no-reader"),
                Err(_) => Some("reader-busy"),
            }
        };
        if let Some(reason) = refused.filter(|_| glow != PageGlow::Off) {
            crate::page_reader::note(format_args!(
                "page_glow refused tab={id} state={state} reason={reason}"
            ));
        } else {
            crate::page_reader::note(format_args!(
                "page_glow requested tab={id} state={state} reduce_motion={reduce_motion}"
            ));
        }
        let glow = if refused.is_none() {
            glow
        } else {
            PageGlow::Off
        };
        crate::platform::imp::page_glow::set(id, &view.view, glow, reduce_motion);
    }

    /// The committed address of `id`'s document at `epoch`.
    fn reader_target(&self, id: ItemId, epoch: u64) -> Result<String, PageReadFailure> {
        let view = self.views.get(&id).ok_or(PageReadFailure::PageChanged)?;
        let (committed, url) = view
            .navigation
            .committed_snapshot()
            .ok_or(PageReadFailure::NotReady)?;
        if committed.presentation_id().into_raw() != epoch {
            return Err(PageReadFailure::PageChanged);
        }
        if !admits(&url) {
            return Err(PageReadFailure::Unavailable);
        }
        Ok(url)
    }

    pub(crate) fn page_read(&mut self, id: ItemId, epoch: u64, done: ReadDone) {
        let url = match self.reader_target(id, epoch) {
            Ok(url) => url,
            Err(failure) => return done(Err(failure)),
        };
        let mut done = Some(done);
        let found = self.with_reader(id, |reader| {
            if let Some(done) = done.take() {
                reader.read(epoch, url, done);
            }
        });
        if let (false, Some(done)) = (found, done) {
            done(Err(PageReadFailure::Unavailable));
        }
    }

    fn page_recipe(&mut self, id: ItemId, epoch: u64, recipe: PageRecipe, done: RecipeDone) {
        if let Err(_failure) = self.reader_target(id, epoch) {
            return done(None);
        }
        let mut done = Some(done);
        let found = self.with_reader(id, |reader| {
            if let Some(done) = done.take() {
                reader.recipe(epoch, recipe, done);
            }
        });
        if let (false, Some(done)) = (found, done) {
            done(None);
        }
    }

    pub(crate) fn page_selection(
        &mut self,
        id: ItemId,
        epoch: u64,
        done: Box<dyn FnOnce(Option<String>) + Send>,
    ) {
        self.page_recipe(
            id,
            epoch,
            PageRecipe::selection(),
            Box::new(move |answer| done(answer.filter(|text| !text.is_empty()))),
        );
    }

    pub(crate) fn page_outline(
        &mut self,
        id: ItemId,
        epoch: u64,
        done: Box<dyn FnOnce(Result<PageOutline, PageReadFailure>) + Send>,
    ) {
        self.page_recipe(
            id,
            epoch,
            PageRecipe::outline(),
            Box::new(move |answer| {
                done(
                    answer
                        .as_deref()
                        .and_then(parse_outline)
                        .ok_or(PageReadFailure::Unavailable),
                );
            }),
        );
    }

    pub(crate) fn page_highlight(
        &mut self,
        id: ItemId,
        epoch: u64,
        quote: &str,
        done: Box<dyn FnOnce(bool) + Send>,
    ) {
        let Some(recipe) = PageRecipe::mark(quote) else {
            return done(false);
        };
        self.page_recipe(
            id,
            epoch,
            recipe,
            Box::new(move |answer| {
                done(matches!(
                    answer.as_deref().and_then(mark_outcome),
                    Some(MarkOutcome::Marked | MarkOutcome::Scrolled)
                ));
            }),
        );
    }

    /// Runs one person-directed step on `id`'s document at `epoch`.
    pub(crate) fn page_act(
        &mut self,
        id: ItemId,
        epoch: u64,
        read: u64,
        proposal: zephium_agentic::SemanticActionProposal,
        done: Box<dyn FnOnce(Result<(), PageActFailure>) + Send>,
    ) {
        let url = match self.reader_target(id, epoch) {
            Ok(url) => url,
            Err(PageReadFailure::PageChanged) => return done(Err(PageActFailure::PageChanged)),
            Err(_) => return done(Err(PageActFailure::Refused)),
        };
        // The action may mutate before its result can be verified, including
        // on a timeout/refusal after beforeinput. Keep that document resident.
        if matches!(
            proposal.intent().kind(),
            zephium_agentic::SemanticActionKind::Fill
                | zephium_agentic::SemanticActionKind::Select
                | zephium_agentic::SemanticActionKind::Press
        ) && !self
            .views
            .get(&id)
            .is_some_and(|view| view.navigation.protect_assistant_edit(epoch))
        {
            return done(Err(PageActFailure::PageChanged));
        }
        let mut done = Some(done);
        let found = self.with_reader(id, |reader| {
            if let Some(done) = done.take() {
                reader.act(epoch, read, url, proposal, done);
            }
        });
        if let (false, Some(done)) = (found, done) {
            done(Err(PageActFailure::Failed));
        }
    }

    /// Replaces `id`'s marks with `marks`, each found again through read
    /// `read` of the document at `epoch`. A read that is gone places none.
    pub(crate) fn page_marks(
        &mut self,
        id: ItemId,
        epoch: u64,
        read: u64,
        marks: Vec<PageMark>,
        reduce_motion: bool,
        done: Box<dyn FnOnce(Vec<u8>) + Send>,
    ) {
        if self.reader_target(id, epoch).is_err() {
            return done(Vec::new());
        }
        let Some((recipe, targets)) = marks_recipe(id, epoch, read, &marks, reduce_motion) else {
            return done(Vec::new());
        };
        self.page_recipe(
            id,
            epoch,
            recipe,
            Box::new(move |answer| {
                done(
                    answer
                        .map(|answer| placed_marks(&answer, &targets))
                        .unwrap_or_default(),
                );
            }),
        );
    }

    pub(crate) fn page_mark_focus(&mut self, id: ItemId, epoch: u64, n: Option<u8>, scroll: bool) {
        self.page_recipe(
            id,
            epoch,
            PageRecipe::mark_focus(n, scroll),
            Box::new(|_| {}),
        );
    }

    pub(crate) fn page_marks_clear(&mut self, id: ItemId) {
        let Some(epoch) = crate::page_reader::committed_epoch(id) else {
            return;
        };
        if let Some(recipe) = PageRecipe::marks(&[], false) {
            self.page_recipe(id, epoch, recipe, Box::new(|_| {}));
        }
    }

    /// The person picks one element of `id`'s document at `epoch`; a pick
    /// already running there ends as cancelled.
    pub(crate) fn page_pick(
        &mut self,
        id: ItemId,
        epoch: u64,
        reduce_motion: bool,
        done: PickDone,
    ) {
        if let Err(failure) = self.reader_target(id, epoch) {
            return done(Err(match failure {
                PageReadFailure::PageChanged => PagePickFailure::PageChanged,
                PageReadFailure::NotReady | PageReadFailure::Unavailable => {
                    PagePickFailure::Unavailable
                }
            }));
        }
        let pick = NEXT_PICK.fetch_add(1, Ordering::Relaxed);
        let Some(recipe) = PageRecipe::pick(pick, reduce_motion) else {
            return done(Err(PagePickFailure::Unavailable));
        };
        let previous = PICKS.with(|picks| {
            picks.borrow_mut().insert(
                id,
                PickSession {
                    pick,
                    epoch,
                    done,
                    started: Instant::now(),
                    misses: 0,
                    timer: None,
                },
            )
        });
        if let Some(previous) = previous {
            (previous.done)(Err(PagePickFailure::Cancelled));
        }
        self.page_recipe(
            id,
            epoch,
            recipe,
            Box::new(move |answer| match answer.as_deref() {
                Some("1") => poll_pick_later(id, pick),
                _ if crate::page_reader::committed_epoch(id) != Some(epoch) => {
                    finish_pick(id, pick, Err(PagePickFailure::PageChanged));
                }
                _ => finish_pick(id, pick, Err(PagePickFailure::Unavailable)),
            }),
        );
    }

    pub(crate) fn page_pick_poll(&mut self, id: ItemId, pick: u64) {
        let Some(epoch) = PICKS.with(|picks| {
            picks
                .borrow()
                .get(&id)
                .filter(|session| session.pick == pick)
                .map(|session| session.epoch)
        }) else {
            return;
        };
        self.page_recipe(
            id,
            epoch,
            PageRecipe::picked(),
            Box::new(move |answer| pick_answered(id, pick, answer)),
        );
    }

    pub(crate) fn page_pick_cancel(&mut self, id: ItemId) {
        let Some(session) = PICKS.with(|picks| picks.borrow_mut().remove(&id)) else {
            return;
        };
        let epoch = session.epoch;
        (session.done)(Err(PagePickFailure::Cancelled));
        self.page_recipe(id, epoch, PageRecipe::pick_cancel(), Box::new(|_| {}));
    }

    pub(crate) fn page_pick_read(
        &mut self,
        id: ItemId,
        epoch: u64,
        pick: u64,
        done: Box<dyn FnOnce(Option<PagePick>) + Send>,
    ) {
        let Some(recipe) = PageRecipe::pick_read(pick) else {
            return done(None);
        };
        self.page_recipe(
            id,
            epoch,
            recipe,
            Box::new(move |answer| {
                done(
                    match answer
                        .as_deref()
                        .and_then(|answer| pick_state(answer, pick))
                    {
                        Some(PickState::Picked(picked)) => Some(picked),
                        _ => None,
                    },
                );
            }),
        );
    }

    pub(crate) fn page_pick_clear(&mut self, id: ItemId, pick: u64) {
        let (Some(epoch), Some(recipe)) = (
            crate::page_reader::committed_epoch(id),
            PageRecipe::pick_clear(pick),
        ) else {
            return;
        };
        self.page_recipe(id, epoch, recipe, Box::new(|_| {}));
    }

    pub(crate) fn page_reader_poll(&mut self, id: ItemId) {
        self.with_reader(id, Reader::poll);
    }

    pub(crate) fn page_reader_pump(&mut self, id: ItemId) {
        self.with_reader(id, Reader::pump);
    }

    pub(crate) fn page_reader_timeout(&mut self, id: ItemId, operation: u64) {
        self.with_reader(id, |reader| reader.timeout(operation));
    }

    /// WebView2 keeps nothing running between asks, so only macOS idles.
    #[cfg(target_os = "macos")]
    pub(crate) fn page_reader_idle(&mut self, id: ItemId) {
        self.with_reader(id, Reader::idle);
    }
}
