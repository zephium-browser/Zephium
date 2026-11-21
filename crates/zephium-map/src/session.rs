use crate::{
    compare,
    doc::DocLimits,
    passage::{self, Passage, MAX_PASSAGES, MAX_RESULT_PAGES},
    query::{Confidence, QueryError},
    results::{Link, Organic},
    robots::{Robots, RobotsOutcome, MAX_SITEMAPS},
    site::{read_site, SiteInputs, Sitemap, MAX_SECTION_PAGES},
    *,
};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    time::Duration,
};
use url::{Position, Url};
use zephium_core::search::SearchEngine;
mod memo;
mod types;
pub use memo::*;
pub use types::*;

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Work {
    Robots,
    Anonymous(usize, String),
    Entry(usize),
    Section(usize, PageKey),
    Llms(usize),
    Sitemap(usize),
    Passage(usize),
    Render(usize),
}
impl Work {
    fn phase(&self) -> Phase {
        match self {
            Self::Robots => Phase::Robots,
            Self::Anonymous(..) => Phase::AnonymousResults,
            Self::Entry(_) => Phase::Entry,
            Self::Section(..) => Phase::Section,
            Self::Llms(_) => Phase::Llms,
            Self::Sitemap(_) => Phase::Sitemap,
            Self::Passage(_) => Phase::Passage,
            Self::Render(_) => Phase::Render,
        }
    }
    fn expect(&self) -> Expect {
        match self {
            Self::Robots | Self::Llms(_) => Expect::Text,
            Self::Sitemap(_) => Expect::Xml,
            _ => Expect::Html,
        }
    }
    fn side(&self) -> Option<usize> {
        match self {
            Self::Entry(i)
            | Self::Section(i, _)
            | Self::Llms(i)
            | Self::Sitemap(i)
            | Self::Render(i) => Some(*i),
            _ => None,
        }
    }
}
#[derive(Clone)]
struct Job {
    work: Work,
    url: Url,
}
enum Pending {
    Results,
    Cache(usize),
    Job(Job),
}
struct Seed {
    site: SiteKey,
    url: Url,
    links: Vec<Link>,
    confidence: Confidence,
}
impl Seed {
    fn new(hit: &Organic, confidence: Confidence) -> Option<Self> {
        Some(Self {
            site: SiteKey::from_url(&hit.url)?,
            url: hit.url.clone(),
            links: hit.sitelinks.clone(),
            confidence,
        })
    }
    fn official(layer: &FirstLayer) -> Option<Self> {
        let o = layer.official.as_ref()?;
        Self::new(layer.organic.get(o.organic_index)?, o.confidence)
    }
}
struct SiteState {
    seed: Seed,
    shared_passage: Option<usize>,
    doc: Option<Doc>,
    pages: BTreeMap<PageKey, Doc>,
    maps: BTreeMap<PageKey, Sitemap>,
    llms: Option<String>,
    place: Option<Place>,
    denied: bool,
    cut: BTreeSet<Phase>,
    maps_wanted: usize,
    rendered: bool,
    clean: bool,
    stale: bool,
    external: BTreeMap<PageKey, PageKey>,
}
impl SiteState {
    fn new(seed: Seed) -> Self {
        Self {
            seed,
            shared_passage: None,
            doc: None,
            pages: BTreeMap::new(),
            maps: BTreeMap::new(),
            llms: None,
            place: None,
            denied: false,
            cut: BTreeSet::new(),
            maps_wanted: 0,
            rendered: false,
            clean: true,
            stale: false,
            external: BTreeMap::new(),
        }
    }
}
pub struct Session {
    cfg: Config,
    query: QueryRead,
    sequence: u32,
    elapsed: Duration,
    finished: bool,
    stats: Stats,
    pending: BTreeMap<Id, Pending>,
    queue: VecDeque<Job>,
    seen: BTreeSet<(Work, PageKey)>,
    robots: BTreeMap<String, Robots>,
    hosts: BTreeMap<String, usize>,
    started: usize,
    sections: usize,
    sites: [Option<SiteState>; 2],
    articles: BTreeMap<usize, Doc>,
    first_passage: Option<Passage>,
    passages: Vec<Passage>,
}
impl Session {
    pub fn start(mut cfg: Config, query_text: &str) -> Result<(Self, Vec<Effect>), QueryError> {
        let query = read_query(query_text)?;
        if cfg.custom_template.len() > crate::key::MAX_URL_BYTES {
            return Err(QueryError::Search);
        }
        let url = cfg
            .engine
            .configured_search(query_text, &cfg.custom_template)
            .ok_or(QueryError::Search)?;
        cfg.tighten();
        let kind = query.kind().clone();
        let mut s = Self {
            cfg,
            query,
            sequence: 0,
            elapsed: Duration::ZERO,
            finished: false,
            stats: Stats::default(),
            pending: BTreeMap::new(),
            queue: VecDeque::new(),
            seen: BTreeSet::new(),
            robots: BTreeMap::new(),
            hosts: BTreeMap::new(),
            started: 0,
            sections: 0,
            sites: [None, None],
            articles: BTreeMap::new(),
            first_passage: None,
            passages: Vec::new(),
        };
        let mut out = Vec::new();
        s.emit(MapEvent::Kind(kind), &mut out);
        let id = s.id();
        s.pending.insert(id, Pending::Results);
        s.counts(Phase::Results).started += 1;
        out.push(Effect::LoadResults { id, url });
        Ok((s, out))
    }
    fn id(&mut self) -> Id {
        self.sequence += 1;
        Id {
            session: self.cfg.session_id,
            sequence: self.sequence,
        }
    }
    fn emit(&mut self, event: MapEvent, out: &mut Vec<Effect>) {
        out.push(Effect::Emit {
            id: self.id(),
            event,
        });
    }
    fn counts(&mut self, p: Phase) -> &mut Counts {
        self.stats.phases.entry(p).or_default()
    }
    fn refused(&mut self, p: Phase, f: Failure) {
        *self.counts(p).refused.entry(f).or_default() += 1;
    }
    fn budget(&mut self, b: Budget) {
        self.stats.budgets.insert(b);
    }
    fn enqueue(&mut self, work: Work, url: Url) {
        let Ok(key) = PageKey::from_url(&url) else {
            return;
        };
        if self.seen.contains(&(work.clone(), key.clone())) {
            return;
        }
        if let Work::Sitemap(i) = work {
            let Some(site) = self.sites[i].as_mut() else {
                return;
            };
            if site.maps_wanted >= MAX_SITEMAPS
                || SiteKey::from_url(&url).as_ref() != Some(&site.seed.site)
            {
                return;
            }
            site.maps_wanted += 1;
        }
        self.seen.insert((work.clone(), key));
        self.queue.push_back(Job { work, url });
    }
    fn lookup(&mut self, side: usize, seed: Seed, out: &mut Vec<Effect>) {
        if self.elapsed >= self.cfg.last_start {
            self.budget(Budget::LastStart);
            return;
        }
        let site = seed.site.clone();
        let mut state = SiteState::new(seed);
        if let Some(at) = self
            .queue
            .iter()
            .position(|j| matches!(j.work, Work::Passage(_)) && j.url == state.seed.url)
        {
            if let Some(Job {
                work: Work::Passage(pos),
                ..
            }) = self.queue.remove(at)
            {
                state.shared_passage = Some(pos);
            }
        }
        self.sites[side] = Some(state);
        let id = self.id();
        self.pending.insert(id, Pending::Cache(side));
        out.push(Effect::LookupSite { id, site });
    }
    fn read(&self, doc: &Doc, q: &QueryRead) -> Result<FirstLayer, Failure> {
        match read_results(self.cfg.engine, None, q, doc) {
            ResultsRead::Wall(w) => Err(w.into()),
            ResultsRead::FirstLayer(l) => Ok(l),
        }
    }
    fn side_seed(&self, doc: &Doc, text: &str) -> Option<Seed> {
        let read = read_query(text).ok()?;
        Seed::official(&self.read(doc, &read).ok()?)
    }
    fn results(&mut self, doc: Doc, out: &mut Vec<Effect>) -> Result<(), Failure> {
        let layer = self.read(&doc, &self.query)?;
        let mut seeds = Vec::new();
        let mut missing = Vec::new();
        match self.query.kind() {
            QueryKind::Instant | QueryKind::List => {}
            QueryKind::Place { .. } => {
                let seed = Seed::official(&layer).or_else(|| {
                    let hit = layer.organic.first()?;
                    (self.query.confidence() == Confidence::High && !hit.sitelinks.is_empty())
                        .then(|| Seed::new(hit, Confidence::Low))
                        .flatten()
                });
                if let Some(seed) = seed {
                    seeds.push((0, seed));
                }
            }
            QueryKind::Compare { left, right, .. } => {
                for (side, words) in [left, right].iter().enumerate() {
                    let text = words.join(" ");
                    if let Some(seed) = self.side_seed(&doc, &text) {
                        seeds.push((side, seed));
                    } else {
                        missing.push((side, text));
                    }
                }
            }
            QueryKind::Passage(_) => {
                if let Some(f) = &layer.featured {
                    self.first_passage = layer
                        .organic
                        .get(f.organic_index)
                        .and_then(|r| passage::featured(&self.query, f, r));
                }
                let count = self.cfg.result_pages;
                if layer.organic.len() > count {
                    self.budget(Budget::ResultPages);
                }
                for (i, r) in layer.organic.iter().take(count).enumerate() {
                    self.enqueue(Work::Passage(i), r.url.clone());
                }
                if let Some(hit) = layer.organic.first() {
                    if let Some(site) = SiteKey::from_url(&hit.url) {
                        let name = site.as_str().split('.').next().unwrap_or_default();
                        if self.query.terms().iter().any(|w| {
                            w.trim_end_matches('?') == site.as_str()
                                || !hit.sitelinks.is_empty() && w == name
                        }) {
                            if let Some(seed) = self.side_seed(&doc, site.as_str()) {
                                seeds.push((0, seed));
                            }
                        }
                    }
                }
            }
        }
        self.emit(MapEvent::FirstLayer(layer), out);
        if self.first_passage.is_some() {
            self.update_passages(out);
        }
        for (side, seed) in seeds {
            self.lookup(side, seed, out);
        }
        for (side, query) in missing {
            if let Some(url) = self
                .cfg
                .engine
                .configured_search(&query, &self.cfg.custom_template)
            {
                self.enqueue(Work::Anonymous(side, query), url);
            }
        }
        Ok(())
    }
    fn cache(&mut self, i: usize, raw: Option<Vec<u8>>, out: &mut Vec<Effect>) {
        let Some(state) = self.sites[i].as_mut() else {
            return;
        };
        let url = state.seed.url.clone();
        let shared_passage = state.shared_passage;
        let supplied = raw.is_some();
        if let Some(m) = raw
            .and_then(|bytes| SiteMemo::decode(&bytes, &state.seed.site).ok())
            .filter(|m| m.read_at <= self.cfg.now)
        {
            let stale = self.cfg.now - m.read_at >= MEMO_TTL;
            let confidence = state.seed.confidence;
            if stale {
                self.stats.stale_memos += 1;
            } else {
                self.stats.fresh_memos += 1;
            }
            state.stale = stale;
            state.place = Some(m.place.clone());
            self.emit_place(m.place, stale, confidence, out);
            if !stale {
                if let Some(pos) = shared_passage {
                    self.queue.push_back(Job {
                        work: Work::Passage(pos),
                        url,
                    });
                }
                for (host, rules) in m.robots {
                    if !self.pending.values().any(|p|matches!(p,Pending::Job(j) if j.work==Work::Robots && origin(&j.url)==host)){
                        self.robots.entry(host).or_insert(rules);
                    }
                }
                return;
            }
        } else if supplied {
            self.stats.rejected_memos += 1;
        }
        self.enqueue(Work::Entry(i), url);
    }
    fn update_passages(&mut self, out: &mut Vec<Effect>) {
        let mut ordered: BTreeMap<_, _> =
            self.articles.iter().map(|(pos, doc)| (*pos, doc)).collect();
        for site in self.sites.iter().flatten() {
            if let Some((pos, doc)) = site.shared_passage.zip(site.doc.as_ref()) {
                ordered.insert(pos, doc);
            }
        }
        let pages: Vec<_> = ordered.into_values().collect();
        let mut passages: Vec<_> = self.first_passage.clone().into_iter().collect();
        for p in passage::read_passages(&self.query, &pages) {
            if !passages.iter().any(|f| f.source == p.source) {
                passages.push(p);
            }
        }
        passages.truncate(MAX_PASSAGES);
        if self.passages != passages {
            self.passages = passages.clone();
            self.emit(MapEvent::Passages(passages), out);
        }
    }
    fn emit_place(
        &mut self,
        place: Place,
        stale: bool,
        confidence: Confidence,
        out: &mut Vec<Effect>,
    ) {
        for section in &place.sections {
            for e in std::iter::once(section).chain(&section.children) {
                self.stats.sources.extend(&e.sources);
            }
        }
        self.emit(
            MapEvent::Place {
                place,
                stale,
                confidence,
            },
            out,
        );
    }
    fn rebuild(&mut self, i: usize, out: &mut Vec<Effect>) {
        let Some(state) = self.sites[i].as_mut() else {
            return;
        };
        let Some(doc) = &state.doc else { return };
        let inputs = SiteInputs {
            sitelinks: &state.seed.links,
            robots: self.robots.get(&origin(doc.page())),
            llms: state.llms.as_deref(),
            sitemaps: state.maps.iter().map(|(k, v)| (k.clone(), v)).collect(),
            pages: state.pages.iter().map(|(k, v)| (k.clone(), v)).collect(),
        };
        let Ok(mut place) = read_site(doc, &inputs) else {
            return;
        };
        place.notes.robots_disallowed |= state.denied;
        place.notes.entry_truncated |= state.cut.contains(&Phase::Entry);
        place.notes.section_page_truncated |= state.cut.contains(&Phase::Section);
        place.notes.sitemap_truncated |= state.cut.contains(&Phase::Sitemap);
        place.notes.llms_truncated |= state.cut.contains(&Phase::Llms);
        for entry in &mut place.sections {
            if let Some(key) = state.external.get(&entry.key) {
                entry.external = true;
                entry.key = key.clone();
                entry.locator = Locator::None;
            }
        }
        if !std::mem::take(&mut state.stale) && state.place.as_ref() == Some(&place) {
            return;
        }
        let confidence = state.seed.confidence;
        state.place = Some(place.clone());
        self.emit_place(place, false, confidence, out);
    }
    fn plan_sections(&mut self, i: usize) {
        let Some(state) = self.sites[i].as_ref() else {
            return;
        };
        let Some(place) = state.place.as_ref() else {
            return;
        };
        let fresh = matches!(self.query.kind(), QueryKind::Place { fresh: true, .. });
        let keys: Vec<_> = place
            .fetch_sections(fresh)
            .into_iter()
            .filter(|e| {
                !state.external.values().any(|k| k == &e.key)
                    && !state
                        .pages
                        .values()
                        .any(|d| PageKey::from_url(d.page()).ok().as_ref() == Some(&e.key))
            })
            .map(|e| e.key.clone())
            .collect();
        for key in keys {
            self.enqueue(Work::Section(i, key.clone()), key.as_url().clone());
        }
    }
    fn entry(&mut self, i: usize, doc: Doc, out: &mut Vec<Effect>) {
        let url = doc.page().clone();
        let Some(state) = self.sites[i].as_mut() else {
            return;
        };
        state.doc = Some(doc);
        self.rebuild(i, out);
        let Some(state) = self.sites[i].as_mut() else {
            return;
        };
        let Some(place) = &state.place else { return };
        if place.notes.wall {
            self.stats.walls += 1;
            state.clean = false;
        }
        if place.notes.shell && !state.rendered {
            self.stats.shells += 1;
            state.rendered = true;
            self.enqueue(Work::Render(i), url.clone());
        }
        self.plan_sections(i);
        if let Ok(llms) = url.join("/llms.txt") {
            self.enqueue(Work::Llms(i), llms);
        }
        let mut urls = self
            .robots
            .get(&origin(&url))
            .map(|r| r.sitemaps.clone())
            .unwrap_or_default();
        if urls.is_empty() {
            urls.extend(url.join("/sitemap.xml"));
        }
        for url in urls {
            self.enqueue(Work::Sitemap(i), url);
        }
    }
    pub fn handle(&mut self, input: Input) -> Vec<Effect> {
        let mut out = Vec::new();
        if self.finished {
            return out;
        }
        match input {
            Input::Cancel => {
                self.stats.cancelled = true;
                self.done(&mut out);
                return out;
            }
            Input::Elapsed(time) => {
                self.elapsed = self.elapsed.max(time);
                if self.elapsed >= self.cfg.elapsed {
                    self.budget(Budget::Elapsed);
                    self.finish(&mut out);
                    return out;
                }
            }
            Input::Results { id, result } => {
                if !matches!(
                    self.pending.get(&id),
                    Some(
                        Pending::Results
                            | Pending::Job(Job {
                                work: Work::Anonymous(..),
                                ..
                            })
                    )
                ) {
                    return out;
                }
                match self.pending.remove(&id) {
                    Some(Pending::Results) => {
                        self.counts(Phase::Results).completed += 1;
                        if let Err(f) = result.and_then(|doc| self.results(doc, &mut out)) {
                            self.finished = true;
                            self.emit(MapEvent::Failed(f), &mut out);
                            return out;
                        }
                    }
                    Some(Pending::Job(Job {
                        work: Work::Anonymous(i, text),
                        ..
                    })) => {
                        self.counts(Phase::AnonymousResults).completed += 1;
                        match result {
                            Ok(doc) => {
                                if let Some(seed) = self.side_seed(&doc, &text) {
                                    self.lookup(i, seed, &mut out);
                                }
                            }
                            Err(f) => self.refused(Phase::AnonymousResults, f),
                        }
                    }
                    _ => {}
                }
            }
            Input::Cache { id, memo } => {
                if !matches!(self.pending.get(&id), Some(Pending::Cache(_))) {
                    return out;
                }
                if let Some(Pending::Cache(i)) = self.pending.remove(&id) {
                    self.cache(i, memo, &mut out);
                }
            }
            Input::Fetch { id, result } => {
                if !matches!(self.pending.get(&id),Some(Pending::Job(j)) if !matches!(j.work,Work::Render(_)|Work::Anonymous(..)))
                {
                    return out;
                }
                if let Some(Pending::Job(job)) = self.pending.remove(&id) {
                    self.receive(job, result, &mut out);
                }
            }
            Input::Render { id, result } => {
                if !matches!(
                    self.pending.get(&id),
                    Some(Pending::Job(Job {
                        work: Work::Render(_),
                        ..
                    }))
                ) {
                    return out;
                }
                if let Some(Pending::Job(Job {
                    work: Work::Render(i),
                    url,
                })) = self.pending.remove(&id)
                {
                    self.counts(Phase::Render).completed += 1;
                    match result {
                        Ok(doc) if PageKey::from_url(doc.page()) == PageKey::from_url(&url) => {
                            self.entry(i, doc, &mut out)
                        }
                        Ok(_) => self.refused(Phase::Render, Failure::External),
                        Err(f) => self.refused(Phase::Render, f),
                    }
                }
            }
        }
        self.schedule(&mut out);
        out
    }
    fn done(&mut self, out: &mut Vec<Effect>) {
        self.finished = true;
        self.pending.clear();
        self.queue.clear();
        self.emit(MapEvent::Done(self.stats.clone()), out);
    }
    fn finish(&mut self, out: &mut Vec<Effect>) {
        if matches!(self.query.kind(), QueryKind::Passage(_))
            && self.sites.iter().any(Option::is_some)
        {
            self.update_passages(out);
        }
        if matches!(self.query.kind(), QueryKind::Passage(_))
            && self.passages.is_empty()
            && self
                .stats
                .phases
                .get(&Phase::Results)
                .is_some_and(|c| c.completed > 0)
        {
            self.emit(MapEvent::Passages(Vec::new()), out);
        }
        for i in 0..self.sites.len() {
            let Some(s) = &self.sites[i] else { continue };
            if let Some(p) = s.place.as_ref().filter(|p| {
                s.doc.is_some()
                    && s.clean
                    && !p.notes.shell
                    && !p.notes.wall
                    && self.stats.budgets.is_empty()
            }) {
                let site = p.site.clone();
                let robots = self
                    .robots
                    .iter()
                    .filter(|(o, _)| {
                        Url::parse(o)
                            .ok()
                            .and_then(|u| SiteKey::from_url(&u))
                            .as_ref()
                            == Some(&site)
                    })
                    .map(|(o, r)| (o.clone(), r.clone()))
                    .collect();
                let memo = SiteMemo {
                    version: MEMO_VERSION,
                    place: p.clone(),
                    robots,
                    read_at: self.cfg.now,
                };
                if let Ok(memo) = memo.encode() {
                    let id = self.id();
                    out.push(Effect::RememberSite { id, site, memo });
                }
            }
        }
        if let QueryKind::Compare { shared, .. } = self.query.kind() {
            let place = |i: usize| self.sites[i].as_ref()?.place.as_ref();
            if let Some((l, r)) = place(0).zip(place(1)) {
                self.emit(
                    MapEvent::Compare(compare::compare(l, r, shared.map(|s| s.role))),
                    out,
                );
            }
        }
        self.done(out);
    }
}
fn origin(url: &Url) -> String {
    url.origin().ascii_serialization()
}
mod scheduler;
