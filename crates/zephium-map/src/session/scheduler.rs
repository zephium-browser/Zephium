use super::*;

impl Session {
    pub(super) fn schedule(&mut self, out: &mut Vec<Effect>) {
        if self.elapsed >= self.cfg.elapsed {
            self.budget(Budget::Elapsed);
            self.finish(out);
            return;
        }
        if self.elapsed >= self.cfg.last_start && !self.queue.is_empty() {
            self.budget(Budget::LastStart);
            self.queue.clear();
        }
        loop {
            let jobs: Vec<_> = self
                .pending
                .values()
                .filter_map(|p| match p {
                    Pending::Job(j) => Some(j),
                    _ => None,
                })
                .collect();
            if self.queue.is_empty() || !jobs.is_empty() && jobs.len() >= self.cfg.concurrent {
                break;
            }
            let active: Vec<_> = jobs
                .iter()
                .map(|j| (origin(&j.url), j.work == Work::Robots))
                .collect();
            let fresh = matches!(self.query.kind(), QueryKind::Place { fresh: true, .. });
            let news_job = |job: &Job| match &job.work {
                Work::Section(i, key) => self.sites[*i]
                    .as_ref()
                    .and_then(|s| s.place.as_ref())
                    .is_some_and(|p| {
                        p.sections.iter().any(|e| {
                            &e.key == key
                                && matches!(
                                    e.role,
                                    Some(
                                        SectionRole::Changelog
                                            | SectionRole::News
                                            | SectionRole::Releases
                                    )
                                )
                        })
                    }),
                _ => false,
            };
            let priority = if fresh {
                self.queue
                    .iter()
                    .find(|j| news_job(j))
                    .map(|j| j.work.clone())
            } else {
                None
            };
            let mut progressed = false;
            for _ in 0..self.queue.len() {
                let Some(job) = self.queue.pop_front() else {
                    break;
                };
                if matches!(job.work, Work::Section(..))
                    && priority.as_ref().is_some_and(|p| p != &job.work)
                {
                    self.queue.push_back(job);
                    continue;
                }
                let host = origin(&job.url);
                let hostname = job.url.host_str().unwrap_or_default();
                let phase = job.work.phase();
                let blocked = if self.started >= self.cfg.fetches {
                    Some(Budget::Fetches)
                } else if self.hosts.get(hostname).copied().unwrap_or_default()
                    >= self.cfg.host_fetches
                {
                    Some(Budget::HostFetches)
                } else if self.cfg.concurrent == 0 {
                    Some(Budget::Concurrent)
                } else if self.cfg.host_concurrent == 0 {
                    Some(Budget::HostConcurrent)
                } else if self.cfg.timeout.is_zero() {
                    Some(Budget::Timeout)
                } else if self.cfg.bytes(job.work.expect()) == 0 {
                    Some(job.work.expect().budget())
                } else if matches!(job.work, Work::Section(..))
                    && self.sections >= self.cfg.section_pages
                {
                    Some(Budget::SectionPages)
                } else {
                    None
                };
                if let Some(b) = blocked {
                    self.budget(b);
                    self.refused(phase, Failure::Limit);
                    progressed = true;
                    continue;
                }
                let busy = self
                    .pending
                    .values()
                    .filter(|p| matches!(p,Pending::Job(j) if j.url.host_str()==job.url.host_str()))
                    .count();
                if busy >= self.cfg.host_concurrent {
                    self.queue.push_back(job);
                    continue;
                }
                if let Some(policy) = self.robots.get(&host) {
                    if !policy.allows(&job.url[Position::BeforePath..Position::AfterQuery]) {
                        self.refused(phase, Failure::Robots);
                        if let Some(i) = job.work.side() {
                            if let Some(s) = self.sites[i].as_mut() {
                                s.denied = true;
                                if !matches!(
                                    self.robots.get(&host).map(|r| r.outcome),
                                    Some(RobotsOutcome::Parsed | RobotsOutcome::Missing)
                                ) {
                                    s.clean = false;
                                }
                            }
                            self.rebuild(i, out);
                        }
                        progressed = true;
                        continue;
                    }
                } else {
                    if active.iter().any(|(h, robots)| h == &host && *robots) {
                        self.queue.push_back(job);
                        continue;
                    }
                    if self.cfg.text_bytes == 0 {
                        self.budget(Budget::TextBytes);
                        self.refused(phase, Failure::Limit);
                        progressed = true;
                        continue;
                    }
                    let Ok(url) = job.url.join("/robots.txt") else {
                        self.refused(phase, Failure::Unreadable);
                        progressed = true;
                        continue;
                    };
                    self.queue.push_front(job);
                    self.start_job(
                        Job {
                            work: Work::Robots,
                            url,
                        },
                        out,
                    );
                    progressed = true;
                    break;
                }
                self.start_job(job, out);
                progressed = true;
                break;
            }
            if !progressed {
                break;
            }
        }
        if self.pending.is_empty() && self.queue.is_empty() {
            self.finish(out);
        }
    }
    fn start_job(&mut self, job: Job, out: &mut Vec<Effect>) {
        let id = self.id();
        let url = job.url.clone();
        let expect = job.work.expect();
        self.started += 1;
        *self
            .hosts
            .entry(url.host_str().unwrap_or_default().to_owned())
            .or_default() += 1;
        self.counts(job.work.phase()).started += 1;
        if matches!(job.work, Work::Section(..)) {
            self.sections += 1;
        }
        let effect = if let Work::Anonymous(_, query) = &job.work {
            Effect::ReadResultsAnonymously {
                id,
                query: query.clone(),
            }
        } else if matches!(job.work, Work::Render(_)) {
            Effect::Render { id, url }
        } else {
            Effect::Fetch {
                id,
                url,
                expect,
                max_bytes: self.cfg.bytes(expect),
                timeout: self
                    .cfg
                    .timeout
                    .min(self.cfg.elapsed.saturating_sub(self.elapsed)),
            }
        };
        self.pending.insert(id, Pending::Job(job));
        out.push(effect);
    }
    pub(super) fn receive(
        &mut self,
        job: Job,
        result: Result<FetchResult, Failure>,
        out: &mut Vec<Effect>,
    ) {
        let phase = job.work.phase();
        let expect = job.work.expect();
        let count = self.counts(phase);
        count.completed += 1;
        if let Ok(r) = &result {
            count.bytes = count.bytes.saturating_add(r.bytes.len() as u64);
        }
        let result = result.and_then(|r| {
            if r.bytes.len() > self.cfg.bytes(expect) || r.truncated {
                self.budget(expect.budget());
                return Err(Failure::Limit);
            }
            if !r.media(expect) {
                return Err(Failure::ContentType);
            }
            if let Work::Section(i, ref key) = job.work {
                if SiteKey::from_url(&job.url) != SiteKey::from_url(&r.final_url) {
                    if let Some((s, landing)) = self.sites[i]
                        .as_mut()
                        .zip(PageKey::from_url(&r.final_url).ok())
                    {
                        s.external.insert(key.clone(), landing);
                    }
                }
            }
            if PageKey::from_url(&r.final_url).is_err()
                || SiteKey::from_url(&job.url).is_none()
                || SiteKey::from_url(&r.final_url) != SiteKey::from_url(&job.url)
            {
                return Err(Failure::External);
            }
            if job.work == Work::Robots && origin(&job.url) != origin(&r.final_url) {
                return Err(Failure::External);
            }
            if job.work != Work::Robots
                && self.robots.get(&origin(&r.final_url)).is_some_and(|p| {
                    !p.allows(&r.final_url[Position::BeforePath..Position::AfterQuery])
                })
            {
                return Err(Failure::Robots);
            }
            Ok(r)
        });
        if job.work == Work::Robots {
            let policy = match result {
                Ok(r) => match std::str::from_utf8(&r.bytes) {
                    Ok(text) => Robots::parse(text),
                    Err(_) => Robots::unavailable(false),
                },
                Err(f) => {
                    self.refused(phase, f);
                    if f == Failure::Limit {
                        Robots::limited()
                    } else {
                        Robots::unavailable(matches!(f, Failure::Missing | Failure::Client))
                    }
                }
            };
            *self.stats.robots.entry(policy.outcome).or_default() += 1;
            self.robots.insert(origin(&job.url), policy);
            return;
        }
        let side = job.work.side();
        let applied = result.and_then(|r| self.apply(job, r, out));
        if let Err(f) = applied {
            self.refused(phase, f);
            if let Some(i) = side {
                if let Some(s) = self.sites[i].as_mut() {
                    if f == Failure::Limit {
                        s.cut.insert(phase);
                    }
                    s.denied |= f == Failure::Robots;
                    s.clean &= matches!(f, Failure::Missing | Failure::Client);
                }
                self.rebuild(i, out);
            }
        }
    }
    fn apply(&mut self, job: Job, r: FetchResult, out: &mut Vec<Effect>) -> Result<(), Failure> {
        match job.work {
            Work::Entry(i) => self.entry(i, html(&r, DocLimits::SITE_PAGE)?, out),
            Work::Section(i, key) => {
                let doc = html(&r, DocLimits::SITE_PAGE)?;
                if crate::site::challenge(&doc) {
                    self.stats.walls += 1;
                    return Err(Failure::Challenge);
                }
                if let Some(s) = self.sites[i].as_mut() {
                    s.pages.insert(key, doc);
                }
                self.rebuild(i, out);
            }
            Work::Passage(pos) => {
                let doc = html(&r, DocLimits::ARTICLE)?;
                if crate::site::challenge(&doc) {
                    self.stats.walls += 1;
                    return Err(Failure::Challenge);
                }
                self.articles.insert(pos, doc);
                if self.sites.iter().all(Option::is_none) {
                    self.update_passages(out);
                }
            }
            Work::Llms(i) => {
                let text = String::from_utf8(r.bytes).map_err(|_| Failure::Unreadable)?;
                if let Some(s) = self.sites[i].as_mut() {
                    s.llms = Some(text);
                }
                self.rebuild(i, out);
                self.plan_sections(i);
            }
            Work::Sitemap(i) => {
                let map = Sitemap::parse(&r.bytes, &r.final_url).map_err(|e| match e {
                    site::XmlError::Limit => Failure::Limit,
                    site::XmlError::Malformed => Failure::Unreadable,
                })?;
                for child in &map.index {
                    self.enqueue(Work::Sitemap(i), child.clone());
                }
                let key = PageKey::from_url(&r.final_url).map_err(|_| Failure::Unreadable)?;
                if let Some(s) = self.sites[i].as_mut() {
                    s.maps.insert(key, map);
                }
                self.rebuild(i, out);
                self.plan_sections(i);
            }
            Work::Render(_) | Work::Robots | Work::Anonymous(..) => {}
        }
        Ok(())
    }
}
fn html(r: &FetchResult, limits: DocLimits) -> Result<Doc, Failure> {
    Doc::from_html(&r.final_url, &r.bytes, r.charset.as_deref(), limits)
        .map_err(|_| Failure::Unreadable)
}
