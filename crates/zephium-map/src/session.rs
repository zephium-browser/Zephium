use crate::{
    compare,
    doc::DocLimits,
    passage::{self, Passage, MAX_PASSAGES, MAX_RESULT_PAGES},
    query::{Confidence, QueryError},
    results::{Link, Organic},
    robots::{Robots, RobotsOutcome},
    site::{read_site_with_stats, SiteInputs, Sitemap, MAX_SECTION_PAGES},
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
    Section(usize, PageMatch),
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
    fn byte_cap(&self, cfg: &Config) -> u32 {
        if *self == Self::Robots && cfg.text_bytes == MAX_TEXT_BYTES as u32 {
            crate::robots::MAX_ROBOTS_BYTES as u32
        } else {
            cfg.bytes(self.expect())
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
    site: PlaceKey,
    result: Link,
    url: Url,
    links: Vec<Link>,
    confidence: Confidence,
}
impl Seed {
    fn new(hit: &Organic, confidence: Confidence) -> Option<Self> {
        let site = PlaceKey::from_url(&hit.url)?;
        Some(Self {
            url: site.root(&hit.url).ok()?.as_url().clone(),
            site,
            result: Link {
                url: hit.url.clone(),
                title: hit.title.clone(),
            },
            links: hit.sitelinks.clone(),
            confidence,
        })
    }
    fn official(layer: &FirstLayer) -> Option<Self> {
        layer
            .organic
            .iter()
            .find_map(|hit| Self::new(hit, hit.official.as_ref()?.confidence))
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
    reads: BTreeMap<PageMatch, ReadState>,
    discovered: BTreeMap<site::EntryId, PageMatch>,
    entry_failure: Option<Failure>,
    complete: bool,
    section_hint: Option<Link>,
    requested: Option<site::Focus>,
    rendered: bool,
    clean: bool,
    stale: bool,
    external: BTreeMap<PageMatch, PageKey>,
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
            reads: Default::default(),
            discovered: Default::default(),
            entry_failure: None,
            complete: false,
            section_hint: None,
            requested: None,
            rendered: false,
            clean: true,
            stale: false,
            external: BTreeMap::new(),
        }
    }
}
impl SiteState {
    fn payload_place(&self, robots: &BTreeMap<String, Robots>) -> Option<Place> {
        let doc = self.doc.as_ref()?;
        let inputs = SiteInputs {
            sitelinks: &[],
            result_section: None,
            robots: robots.get(&origin(doc.page())),
            llms: self.llms.as_deref(),
            sitemaps: self.maps.iter().map(|(k, v)| (k.clone(), v)).collect(),
            pages: self.pages.iter().map(|(k, v)| (k.clone(), v)).collect(),
        };
        let (mut place, _) = read_site_with_stats(doc, &inputs).ok()?;
        if let Some(previous) = &self.place {
            place.keep_discovery(previous);
        }
        Some(place)
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
    seen: BTreeSet<(Work, PageMatch)>,
    robots: BTreeMap<String, Robots>,
    hosts: BTreeMap<String, u32>,
    started: u32,
    sections: u32,
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
        s.emit(MapEvent::Kind { kind }, &mut out);
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
        let Ok(key) = PageKey::from_url(&url).map(|key| key.page_match()) else {
            return;
        };
        if self.seen.contains(&(work.clone(), key.clone())) {
            return;
        }
        if let Work::Sitemap(i) = work {
            let Some(site) = self.sites[i].as_mut() else {
                return;
            };
            if site.maps_wanted >= MAX_SITEMAP_READS
                || SiteKey::from_url(&url) != SiteKey::from_url(&site.seed.url)
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
            self.emit(
                MapEvent::PlaceFailed {
                    slot: side as u8,
                    failure: Failure::Limit,
                },
                out,
            );
            return;
        }
        let site = seed.site.clone();
        let role = match self.query.kind() {
            QueryKind::Place { section, .. } => section.map(|s| s.role),
            QueryKind::Compare { shared, .. } => shared.map(|s| s.role),
            _ => None,
        };
        let mut state = SiteState::new(seed);
        state.requested = role.map(|role| {
            let mut modifiers = Vec::new();
            let mut fresh = false;
            if let QueryKind::Place {
                entity,
                fresh: is_fresh,
                ..
            } = self.query.kind()
            {
                fresh = *is_fresh;
                if let Some((brand, rest)) = entity.split_first() {
                    let domain = SiteKey::from_url(&state.seed.url);
                    if !fresh
                        && domain.as_ref().and_then(|s| s.as_str().split('.').next())
                            == Some(brand.as_str())
                    {
                        modifiers = rest.to_vec();
                    }
                }
            }
            site::Focus {
                role,
                modifiers,
                fresh,
            }
        });
        if role.is_some()
            && role == site::section_role(&state.seed.result.title, &state.seed.result.url)
            && !key::same_page(&state.seed.url, &state.seed.result.url)
        {
            state.section_hint = Some(state.seed.result.clone());
        }
        if let Some(at) = self.queue.iter().position(|j| {
            matches!(j.work, Work::Passage(_)) && key::same_page(&j.url, &state.seed.url)
        }) {
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
            ResultsRead::Wall { wall: w } => Err(w.into()),
            ResultsRead::FirstLayer { layer: l } => Ok(l),
        }
    }
    fn side_seed(&self, doc: &Doc, text: &str) -> Option<Seed> {
        let read = read_query(text).ok()?;
        Seed::official(&self.read(doc, &read).ok()?)
    }
    fn results(&mut self, doc: Doc, out: &mut Vec<Effect>) -> Result<(), Failure> {
        let layer = match results::read_results_with_stats(
            self.cfg.engine,
            None,
            &self.query,
            &doc,
            &mut self.stats.results,
        ) {
            ResultsRead::Wall { wall } => return Err(wall.into()),
            ResultsRead::FirstLayer { layer } => layer,
        };
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
            QueryKind::Passage { kind: _ } => {
                if let Some(f) = &layer.featured {
                    self.first_passage = layer
                        .organic
                        .iter()
                        .find(|hit| hit.key.page_match() == f.cites.page_match())
                        .and_then(|r| passage::featured(&self.query, f, r));
                }
                let count = self.cfg.result_pages as usize;
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
        self.emit(MapEvent::FirstLayer { layer }, out);
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
            let Ok(place) = site::with_results(
                m.payload.place,
                &state.seed.links,
                state.section_hint.as_ref(),
                state.requested.as_ref(),
            ) else {
                self.stats.rejected_memos += 1;
                self.enqueue(Work::Entry(i), url);
                return;
            };
            state.place = Some(place.clone());
            let complete = !stale
                && state.section_hint.is_none()
                && (state.requested.is_none() || place.focus.is_some());
            state.complete = complete;
            self.emit_place(i, place, stale, confidence, complete, out);
            if complete {
                if let Some(pos) = shared_passage {
                    self.queue.push_back(Job {
                        work: Work::Passage(pos),
                        url,
                    });
                }
                for (host, rules) in m.payload.robots {
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
    pub fn accepted_passage_pages(&self) -> usize {
        self.articles.len()
            + self
                .sites
                .iter()
                .flatten()
                .filter(|site| {
                    site.shared_passage.is_some()
                        && site.doc.is_some()
                        && site.place.as_ref().is_some_and(|place| !place.notes.wall)
                })
                .count()
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
            if !passages
                .iter()
                .any(|f| f.source.page_match() == p.source.page_match())
            {
                passages.push(p);
            }
        }
        passages.truncate(MAX_PASSAGES);
        if self.passages != passages {
            self.passages = passages.clone();
            self.emit(MapEvent::Passages { passages }, out);
        }
    }
    fn emit_place(
        &mut self,
        i: usize,
        place: Place,
        stale: bool,
        confidence: Confidence,
        complete: bool,
        out: &mut Vec<Effect>,
    ) {
        for section in &place.sections {
            for e in std::iter::once(section).chain(&section.children) {
                self.stats.sources.extend(&e.sources);
            }
        }
        self.emit(
            MapEvent::Place {
                slot: i as u8,
                complete,
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
            result_section: state.section_hint.as_ref(),
            robots: self.robots.get(&origin(doc.page())),
            llms: state.llms.as_deref(),
            sitemaps: state.maps.iter().map(|(k, v)| (k.clone(), v)).collect(),
            pages: state.pages.iter().map(|(k, v)| (k.clone(), v)).collect(),
        };
        let Ok((mut place, mut detail)) =
            site::read_site_for(doc, &inputs, state.requested.as_ref())
        else {
            state.entry_failure = Some(Failure::Unreadable);
            state.clean = false;
            return;
        };
        place.notes.robots_disallowed |= state.denied;
        detail.entry_truncated |= state.cut.contains(&Phase::Entry);
        detail.section_page_truncated |= state.cut.contains(&Phase::Section);
        detail.sitemap_truncated |= state.cut.contains(&Phase::Sitemap);
        detail.llms_truncated |= state.cut.contains(&Phase::Llms);
        place.notes.truncated |= detail.truncated();
        self.stats.places.insert(i as u8, detail);
        if let Some(previous) = &state.place {
            place.keep_discovery(previous);
            if place.check_ids().is_err() {
                state.entry_failure = Some(Failure::Unreadable);
                state.clean = false;
                return;
            }
        }
        let candidates: BTreeSet<_> = place
            .fetch_sections()
            .into_iter()
            .map(|e| e.id.clone())
            .collect();
        for entry in &mut place.sections {
            let discovered = state
                .discovered
                .entry(entry.id.clone())
                .or_insert_with(|| entry.key.page_match());
            entry.read = state.reads.get(discovered).copied();
            if entry.read.is_none() && candidates.contains(&entry.id) {
                state.reads.insert(discovered.clone(), ReadState::Queued);
                entry.read = Some(ReadState::Queued);
            }
            if let Some(key) = state.external.get(discovered) {
                entry.off_site = SiteKey::from_url(key.as_url()).as_ref() != Some(&place.site);
                entry.key = key.clone();
                entry.locator = Locator::default();
            }
        }
        if !std::mem::take(&mut state.stale) && state.place.as_ref() == Some(&place) {
            return;
        }
        let confidence = state.seed.confidence;
        state.place = Some(place.clone());
        self.emit_place(i, place, false, confidence, false, out);
    }
    fn plan_sections(&mut self, i: usize) {
        let Some(state) = self.sites[i].as_ref() else {
            return;
        };
        let Some(place) = state.place.as_ref() else {
            return;
        };
        let keys: Vec<_> = place
            .fetch_sections()
            .into_iter()
            .filter(|e| e.read == Some(ReadState::Queued))
            .map(|e| e.key.clone())
            .collect();
        for key in keys {
            self.enqueue(Work::Section(i, key.page_match()), key.as_url().clone());
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
    }
    fn next_sitemap(&mut self) -> bool {
        for i in 0..self.sites.len() {
            let Some(state) = &self.sites[i] else {
                continue;
            };
            let Some((doc, place)) = state.doc.as_ref().zip(state.place.as_ref()) else {
                continue;
            };
            if state.maps_wanted >= MAX_SITEMAP_READS
                || place
                    .sections
                    .iter()
                    .filter(|e| {
                        e.sources.iter().any(|s| {
                            matches!(
                                s,
                                Source::PrimaryNav
                                    | Source::Footer
                                    | Source::MenuGroup
                                    | Source::Llms
                            )
                        })
                    })
                    .count()
                    >= SITEMAP_SECTION_THRESHOLD
            {
                continue;
            }
            let mut candidates: Vec<_> = state
                .maps
                .values()
                .flat_map(|map| map.index.iter().cloned().map(|u| (false, u)))
                .collect();
            let roots = self
                .robots
                .get(&origin(doc.page()))
                .map(|r| r.sitemaps.as_slice())
                .unwrap_or_default();
            if roots.is_empty() {
                candidates.extend(doc.page().join("/sitemap.xml").map(|u| (true, u)));
            } else {
                candidates.extend(roots.iter().cloned().map(|u| (true, u)));
            }
            candidates.sort_by_cached_key(|(root, u)| {
                (*root, site::sitemap_priority(u, doc.page()), u.clone())
            });
            for (_, url) in candidates {
                self.enqueue(Work::Sitemap(i), url);
                if !self.queue.is_empty() {
                    return true;
                }
            }
        }
        false
    }
    pub fn handle(&mut self, input: Input) -> Vec<Effect> {
        let mut out = Vec::new();
        if self.finished {
            return out;
        }
        match input {
            Input::Cancel => {
                self.stats.cancelled = true;
                for i in 0..self.sites.len() {
                    let Some(state) = self.sites[i].as_mut() else {
                        continue;
                    };
                    if state.complete {
                        continue;
                    }
                    for read in state.reads.values_mut() {
                        if *read == ReadState::Queued {
                            *read = ReadState::Refused {
                                failure: Failure::Cancelled,
                            };
                        }
                    }
                    self.rebuild(i, &mut out);
                    self.close_slot(i, Failure::Cancelled, &mut out);
                }
                self.done(&mut out);
                return out;
            }
            Input::Elapsed { time } => {
                self.elapsed = self.elapsed.max(time);
                if self.elapsed >= self.cfg.elapsed {
                    self.budget(Budget::Elapsed);
                    if self
                        .pending
                        .values()
                        .any(|pending| matches!(pending, Pending::Results))
                    {
                        self.fail_initial_results(Failure::Timeout, &mut out);
                    } else {
                        self.finish(&mut out);
                    }
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
                            self.fail_initial_results(f, &mut out);
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
                                } else {
                                    self.emit(
                                        MapEvent::PlaceFailed {
                                            slot: i as u8,
                                            failure: Failure::Missing,
                                        },
                                        &mut out,
                                    );
                                }
                            }
                            Err(failure) => {
                                self.refused(Phase::AnonymousResults, failure);
                                self.emit(
                                    MapEvent::PlaceFailed {
                                        slot: i as u8,
                                        failure,
                                    },
                                    &mut out,
                                );
                            }
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
                        Ok(doc)
                            if doc.page().origin() == url.origin()
                                && key::same_page(doc.page(), &url) =>
                        {
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
    fn fail_initial_results(&mut self, failure: Failure, out: &mut Vec<Effect>) {
        self.finished = true;
        self.pending.clear();
        self.refused(Phase::Results, failure);
        self.emit(
            MapEvent::End {
                outcome: EndOutcome::Failed { failure },
                stats: self.stats.clone(),
            },
            out,
        );
    }

    fn done(&mut self, out: &mut Vec<Effect>) {
        self.finished = true;
        self.pending.clear();
        self.queue.clear();
        self.emit(
            MapEvent::End {
                outcome: if self.stats.cancelled {
                    EndOutcome::Cancelled
                } else {
                    EndOutcome::Done
                },
                stats: self.stats.clone(),
            },
            out,
        );
    }
    fn close_slot(&mut self, i: usize, missing: Failure, out: &mut Vec<Effect>) {
        let Some(s) = &self.sites[i] else {
            return;
        };
        if s.complete {
            return;
        }
        if let Some(failure) = s
            .entry_failure
            .or_else(|| s.place.is_none().then_some(missing))
        {
            self.emit(
                MapEvent::PlaceFailed {
                    slot: i as u8,
                    failure,
                },
                out,
            );
        } else if let Some(place) = s.place.clone() {
            self.emit_place(i, place, s.stale, s.seed.confidence, true, out);
        }
    }
    fn finish(&mut self, out: &mut Vec<Effect>) {
        if self.elapsed >= self.cfg.elapsed {
            let jobs: Vec<_> = self
                .pending
                .values()
                .filter_map(|v| match v {
                    Pending::Job(j) => Some(j.clone()),
                    _ => None,
                })
                .chain(self.queue.drain(..))
                .collect();
            for job in jobs {
                self.set_read(
                    &job,
                    ReadState::Skipped {
                        budget: Budget::Elapsed,
                    },
                    out,
                );
            }
        }
        if matches!(self.query.kind(), QueryKind::Passage { kind: _ })
            && self.sites.iter().any(Option::is_some)
        {
            self.update_passages(out);
        }
        if matches!(self.query.kind(), QueryKind::Passage { kind: _ })
            && self.passages.is_empty()
            && self
                .stats
                .phases
                .get(&Phase::Results)
                .is_some_and(|c| c.completed > 0)
        {
            self.emit(
                MapEvent::Passages {
                    passages: Vec::new(),
                },
                out,
            );
        }
        for i in 0..self.sites.len() {
            self.close_slot(i, Failure::Limit, out);
            let Some(s) = &self.sites[i] else { continue };
            if s.clean
                && self.stats.budgets.is_empty()
                && s.place
                    .as_ref()
                    .is_some_and(|p| p.key == s.seed.site && !p.notes.shell && !p.notes.wall)
            {
                let Some(place) = s.payload_place(&self.robots) else {
                    continue;
                };
                let mut origins =
                    BTreeSet::from([origin(&s.seed.url), origin(place.entry.as_url())]);
                for section in &place.sections {
                    for entry in std::iter::once(section)
                        .chain(&section.children)
                        .filter(|e| !e.off_site)
                    {
                        origins.insert(origin(entry.key.as_url()));
                    }
                }
                let robots = origins
                    .into_iter()
                    .filter_map(|o| Some((o.clone(), self.robots.get(&o)?.clone())))
                    .collect();
                let site = s.seed.site.clone();
                let memo = SiteMemo {
                    version: MEMO_VERSION,
                    read_at: self.cfg.now,
                    payload: SitePayload { place, robots },
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
                    MapEvent::Compare {
                        rows: compare::compare(l, r, shared.map(|s| s.role)),
                    },
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

#[cfg(test)]
mod arrival_tests {
    use super::*;
    use proptest::prelude::*;
    fn page(path: &str, markup: &str) -> Doc {
        Doc::from_html(
            &Url::parse(&format!("https://example.com/{path}")).unwrap(),
            markup.as_bytes(),
            None,
            DocLimits::SITE_PAGE,
        )
        .unwrap()
    }
    proptest! {
        #[test]
        fn place_updates_keep_discovery_across_arrival_orders(weights in any::<[u32;5]>()){
            let (mut session,_)=Session::start(Config::default(),"example").unwrap();
            let root=page("","<nav><a href='/docs'>Docs</a><a href='/pricing'>Prices</a></nav><footer><a href='/news'>News</a></footer>");
            let seed=Seed{site:PlaceKey::from_url(root.page()).unwrap(),url:root.page().clone(),result:Link{url:root.page().clone(),title:"Example".into()},links:Vec::new(),confidence:Confidence::High};
            let mut state=SiteState::new(seed);
            state.doc=Some(root);
            session.sites[0]=Some(state);
            let mut effects=Vec::new();
            session.rebuild(0,&mut effects);
            let mut order=[0,1,2,3,4];
            order.sort_by_key(|&i|weights[i]);
            for part in order{
                let state=session.sites[0].as_mut().unwrap();
                match part{
                    0|1=>{
                        let (requested,landed)=if part==0{("docs","guide")}else{("pricing","plans")};
                        let doc=page(landed,"<h2 id='tiers'>Tiers</h2>");
                        let request=state.seed.url.join(requested).unwrap();
                        state.pages.insert(PageKey::from_url(&request).unwrap(),doc);
                    }
                    2=>state.llms=Some("## Library\n- [Books](/books)".into()),
                    _=>{
                        let urls=if part==3{["/learn/reference","/learn/introduction"]}else{["/learn","/learn/guide"]};
                        let xml=format!("<urlset>{}</urlset>",urls.map(|path|format!("<url><loc>https://example.com{path}</loc></url>")).join(""));
                        let url=state.seed.url.join(&format!("map-{part}.xml")).unwrap();
                        state.maps.insert(PageKey::from_url(&url).unwrap(),Sitemap::parse(xml.as_bytes(),&url).unwrap());
                    }
                }
                session.rebuild(0,&mut effects);
            }
            let mut ids=BTreeMap::new();
            let mut previous=Vec::new();
            for effect in effects{
                let Effect::Emit{event:MapEvent::Place{slot:0,place,..},..}=effect else{panic!("expected slot zero")};
                let current:Vec<_>=place.sections.iter().map(|e|e.id.clone()).collect();
                let surviving:Vec<_>=previous.iter().filter(|id|current.contains(id)).collect();
                let observed:Vec<_>=current.iter().filter(|id|previous.contains(id)).collect();
                prop_assert_eq!(surviving,observed);
                let mut distinct=BTreeSet::new();
                for e in &place.sections{
                    if let Some(prior)=ids.insert(e.label.clone(),e.id.clone()){prop_assert_eq!(&prior,&e.id);}
                    for item in std::iter::once(e).chain(&e.children){prop_assert!(distinct.insert(item.id.clone()));}
                }
                previous=current;
            }
        }
    }
}
