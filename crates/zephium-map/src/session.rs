      if let Some(unseeded) = self.unseeded[i].take() {
                let failure = match unseeded {
                    Unseeded::Searching => missing,
                    Unseeded::Failed(failure) => failure,
                };
                self.emit(
                    MapEvent::PlaceFailed {
                        slot: i as u8,
                        failure,
                    },
                    out,
                );
            }
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

    fn finish_open(&mut self, out: &mut Vec<Effect>) {
        let Some(open) = self.opening.as_ref() else {
            return;
        };
        let robots = self.place_robots(open.held.place());
        let Some(open) = self.opening.as_mut() else {
            return;
        };
        let mut updated = open.held.clone();
        updated.payload.robots = robots;
        if let Some(updated) = updated.bounded() {
            open.held = updated;
        }
        let remember = if open.clean && self.stats.budgets.is_empty() {
            open.held.memo.clone().and_then(|mut memo| {
                let mut updated = open.held.clone();
                memo.payload.robots.clone_from(&open.held.payload.robots);
                let bytes = memo.encode().ok()?;
                let site = memo.payload.place.key.clone();
                updated.memo = Some(memo);
                let retained = updated.bounded()?;
                retained.memo.as_ref()?;
                open.held = retained;
                Some((site, bytes))
            })
        } else {
            None
        };
        let (slot, place, stale, confidence) = (
            open.slot,
            open.held.place().clone(),
            open.stale,
            open.confidence,
        );
        self.emit_place(usize::from(slot), place, stale, confidence, true, out);
        if let Some((site, memo)) = remember {
            let id = self.id();
            out.push(Effect::RememberSite { id, site, memo });
        }
        self.done(out);
    }
    fn finish(&mut self, out: &mut Vec<Effect>) {
        if self.elapsed >= self.cfg.elapsed {
            let waiting = std::mem::take(&mut self.landings)
                .into_values()
                .flatten()
                .map(|(job, _)| job);
            let jobs: Vec<_> = self
                .pending
                .values()
                .filter_map(|v| match v {
                    Pending::Job(j) => Some(j.clone()),
                    _ => None,
                })
                .chain(self.queue.drain(..))
                .chain(waiting)
                .collect();
            for job in jobs {
                if self.opening.is_some() && matches!(job.work, Work::Section(..)) {
                    self.refused(Phase::Section, Failure::Timeout);
                }
                self.set_read(
                    &job,
                    ReadState::Skipped {
                        budget: Budget::Elapsed,
                    },
                    out,
                );
            }
        }
        if self.opening.is_some() {
            self.finish_open(out);
            return;
        }
        let kind = self.query.as_ref().map(|q| q.kind().clone());
        if matches!(kind, Some(QueryKind::Passage { .. })) && self.sites.iter().any(Option::is_some)
        {
            self.update_passages(out);
        }
        if matches!(kind, Some(QueryKind::Passage { .. }))
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
            let Some(s) = &mut self.sites[i] else {
                continue;
            };
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
                    eager: self.cfg.sections == Sections::All,
                    read_at: self.cfg.now,
                    payload: SitePayload { place, robots },
                };
                if let Ok(bytes) = memo.encode() {
                    s.memo = Some(memo);
                    let id = self.id();
                    out.push(Effect::RememberSite {
                        id,
                        site,
                        memo: bytes,
                    });
                }
            }
        }
        if let Some(QueryKind::Compare { shared, .. }) = kind {
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
pub(crate) fn origin(url: &Url) -> String {
    url.origin().ascii_serialization()
}
mod scheduler;

#[cfg(test)]
mod slot_tests {
    use super::*;
    const RESULT: &str =
        "<main><article><h2><a href='https://acme.com/'>Acme workshop</a></h2></article></main>";
    fn doc(url: &str, markup: &str) -> Doc {
        Doc::from_html(
            &Url::parse(url).unwrap(),
            markup.as_bytes(),
            None,
            DocLimits::RESULTS,
        )
        .unwrap()
    }
    fn closing(
        anonymous: Option<Result<&str, Failure>>,
        cancel_at_anonymous: bool,
        engine_robots: Option<&str>,
    ) -> Vec<String> {
        let (mut session, effects) = Session::start(Config::default(), "acme vs other").unwrap();
        let mut queue: VecDeque<_> = effects.into();
        let mut last: [Option<String>; 2] = [None, None];
        let mut ended = false;
        while let Some(effect) = queue.pop_front() {
            assert!(!ended);
            let input = match effect {
                Effect::LoadResults { id, .. } => Some(Input::Results {
                    id,
                    result: Ok(doc("https://duckduckgo.com/", RESULT)),
                }),
                Effect::Fetch { id, url, .. } => Some(Input::Fetch {
                    id,
                    result: match (url.host_str(), url.path(), engine_robots) {
                        (_, "/", _) => Ok(FetchResult {
                            final_url: url,
                            content_type: "text/html".into(),
                            charset: None,
                            bytes: b"<nav><a href='/about'>About</a></nav>".to_vec(),
                            truncated: false,
                        }),
                        (Some("duckduckgo.com"), "/robots.txt", Some(rules)) => Ok(FetchResult {
                            final_url: url,
                            content_type: "text/plain".into(),
                            charset: None,
                            bytes: rules.as_bytes().to_vec(),
                            truncated: false,
                        }),
                        _ => Err(Failure::Missing),
                    },
                }),
                Effect::ReadResultsAnonymously { id, .. } => {
                    if cancel_at_anonymous {
                        Some(Input::Cancel)
                    } else {
                        anonymous.map(|answer| Input::Results {
                            id,
                            result: answer.map(|markup| doc("https://duckduckgo.com/", markup)),
                        })
                    }
                }
                Effect::LookupSite { id, .. } => Some(Input::Cache { id, memo: None }),
                Effect::Emit { event, .. } => {
                    match event {
                        MapEvent::Place { slot, complete, .. } => {
                            last[slot as usize] = Some(format!("place {complete}"))
                        }
                        MapEvent::PlaceFailed { slot, failure } => {
                            last[slot as usize] = Some(format!("failed {failure:?}"))
                        }
                        MapEvent::End { .. } => ended = true,
                        _ => {}
                    }
                    None
                }
                Effect::Render { id, .. } => Some(Input::Render {
                    id,
                    result: Err(Failure::Unavailable),
                }),
                Effect::RememberSite { .. } => None,
            };
            if let Some(input) = input {
                queue.extend(session.handle(input));
            }
            if queue.is_empty() && !ended {
                queue.extend(session.handle(Input::Elapsed { time: MAX_ELAPSED }));
            }
        }
        assert!(ended);
        last.into_iter().map(Option::unwrap_or_default).collect()
    }
    #[test]
    fn every_opened_comparison_side_closes_before_the_end() {
        let other = RESULT.replace("acme.com", "other.com");
        assert_eq!(
            closing(Some(Ok(&other)), false, None),
            ["place true", "place true"]
        );
        assert_eq!(
            closing(Some(Err(Failure::Unavailable)), false, None),
            ["place true", "failed Unavailable"]
        );
        assert_eq!(
            closing(Some(Ok("<main></main>")), false, None),
            ["place true", "failed Missing"]
        );
        assert_eq!(closing(None, false, None), ["place true", "failed Limit"]);
        assert_eq!(
            closing(None, true, None),
            ["failed Cancelled", "failed Cancelled"]
        );
    }
    #[test]
    fn a_refused_comparison_search_reports_its_own_reason() {
        let other = RESULT.replace("acme.com", "other.com");
        assert_eq!(
            closing(Some(Ok(&other)), false, Some("User-agent: *\nDisallow: /")),
            ["place true", "failed Robots"]
        );
    }
}

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
