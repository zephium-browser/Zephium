use proptest::prelude::*;
use std::{
    collections::{BTreeSet, VecDeque},
    time::Duration,
};
use url::Url;
use zephium_map::{query::Confidence, robots::RobotsOutcome, session::*, *};
const ENGINE: &str = "https://duckduckgo.com/";
const HOME: &str = "https://acme.com/";
const RESULT: &str =
    "<main><article><h2><a href='https://acme.com/'>Acme workshop</a></h2></article></main>";
const NAV: &str =
    "<title>Acme Workshop</title><nav><a href='/pricing'>Pricing</a><a href='/news'>News</a></nav>";
fn doc(url: &str, markup: &str) -> Doc {
    Doc::from_html(
        &Url::parse(url).unwrap(),
        markup.as_bytes(),
        None,
        DocLimits::SITE_PAGE,
    )
    .unwrap()
}
fn cfg() -> Config {
    Config {
        session_id: 42,
        now: 100,
        concurrent: 1,
        host_concurrent: 1,
        ..Default::default()
    }
}
fn response(id: Id, url: &Url, media: &str, body: &str) -> Input {
    Input::Fetch {
        id,
        result: Ok(FetchResult {
            final_url: url.clone(),
            content_type: media.to_string(),
            charset: None,
            bytes: body.as_bytes().to_vec(),
            truncated: false,
        }),
    }
}
fn name(e: &Effect) -> String {
    match e {
        Effect::LoadResults { .. } => "load".into(),
        Effect::Fetch { url, .. } => format!("fetch {}{}", url.host_str().unwrap(), url.path()),
        Effect::Render { .. } => "render".into(),
        Effect::ReadResultsAnonymously { query, .. } => format!("anon {query}"),
        Effect::LookupSite { site, .. } => format!(
            "lookup {}",
            format_args!(
                "{}{}",
                site.host,
                site.tenant.as_deref().unwrap_or_default()
            )
        ),
        Effect::RememberSite { site, .. } => format!(
            "remember {}",
            format_args!(
                "{}{}",
                site.host,
                site.tenant.as_deref().unwrap_or_default()
            )
        ),
        Effect::Emit { event, .. } => match event {
            MapEvent::Kind { kind: _, .. } => "kind",
            MapEvent::FirstLayer { layer: _, .. } => "layer",
            MapEvent::Place { complete: true, .. } => "complete",
            MapEvent::Place { stale: true, .. } => "stale",
            MapEvent::Place { .. } => "place",
            MapEvent::Passages { passages: _, .. } => "passages",
            MapEvent::Compare { rows: _, .. } => "compare",
            MapEvent::End { stats: _, .. } => "done",
            MapEvent::PlaceFailed { .. } => "place_failed",
        }
        .into(),
    }
}
struct Run {
    trace: Vec<String>,
    stats: Stats,
    places: Vec<Place>,
    conf: Vec<Confidence>,
    memo: Option<Vec<u8>>,
    passages: Vec<Vec<String>>,
}
fn run(
    c: Config,
    query: &str,
    results: &str,
    mut reply: impl FnMut(&Effect) -> Option<Input>,
) -> Run {
    let (mut session, effects) = Session::start(c, query).unwrap();
    let mut queue: VecDeque<_> = effects.into();
    let mut run = Run {
        trace: Vec::new(),
        stats: Stats::default(),
        places: Vec::new(),
        conf: Vec::new(),
        memo: None,
        passages: Vec::new(),
    };
    let mut ids = BTreeSet::new();
    while let Some(effect) = queue.pop_front() {
        let id = effect.id();
        assert!(ids.insert(id));
        run.trace.push(name(&effect));
        let input = reply(&effect).or_else(|| {
            Some(match &effect {
                Effect::LoadResults { .. } => Input::Results {
                    id,
                    result: Ok(doc(ENGINE, results)),
                },
                Effect::Fetch { url, .. } => match url.path() {
                    "/robots.txt" => Input::Fetch {
                        id,
                        result: Err(Failure::Missing),
                    },
                    "/" => response(id, url, "text/html", NAV),
                    "/pricing" => response(
                        id,
                        url,
                        "text/html",
                        include_str!("fixtures/section-pricing.html"),
                    ),
                    "/news" => response(
                        id,
                        url,
                        "text/html",
                        include_str!("fixtures/section-changes.html"),
                    ),
                    _ => Input::Fetch {
                        id,
                        result: Err(Failure::Missing),
                    },
                },
                Effect::Render { .. } => Input::Render {
                    id,
                    result: Err(Failure::Unavailable),
                },
                Effect::ReadResultsAnonymously { .. } => Input::Results {
                    id,
                    result: Err(Failure::Unavailable),
                },
                Effect::LookupSite { .. } => Input::Cache { id, memo: None },
                _ => return None,
            })
        });
        match effect {
            Effect::Emit {
                event: MapEvent::End { stats: s, .. },
                ..
            } => run.stats = s,
            Effect::Emit {
                event: MapEvent::Place {
                    place, confidence, ..
                },
                ..
            } => {
                run.places.push(place);
                run.conf.push(confidence);
            }
            Effect::Emit {
                event: MapEvent::Passages { passages: p, .. },
                ..
            } => run.passages.push(p.into_iter().map(|p| p.text).collect()),
            Effect::RememberSite { memo, .. } => run.memo = Some(memo),
            _ => {}
        }
        if let Some(input) = input {
            queue.extend(session.handle(input));
        }
    }
    assert_eq!(run.trace.last().unwrap(), "done");
    run
}
#[test]
fn engine_only_and_no_site_evidence() {
    for q in ["san francisco time", "pizza near me", "unknown person"] {
        assert_eq!(
            run(cfg(), q, RESULT, |_| None).trace,
            ["kind", "load", "layer", "done"]
        );
    }
    let low = run(
        cfg(),
        "unrelated",
        include_str!("fixtures/heading-sitelinks.html"),
        |_| None,
    );
    assert!(low.conf.iter().all(|c| *c == Confidence::Low) && !low.conf.is_empty());
}
#[test]
fn fresh_place_streams_complete_replacements() {
    let r = run(cfg(), "acme news", RESULT, |_| None);
    assert_eq!(
        r.trace,
        [
            "kind",
            "load",
            "layer",
            "lookup acme.com",
            "fetch acme.com/robots.txt",
            "fetch acme.com/",
            "place",
            "fetch acme.com/news",
            "place",
            "fetch acme.com/pricing",
            "place",
            "fetch acme.com/llms.txt",
            "fetch acme.com/sitemap.xml",
            "complete",
            "remember acme.com",
            "done"
        ]
    );
    assert!(r.places[0].sections.iter().all(|e| e.children.is_empty()));
    assert_eq!(r.places.last().unwrap().sections[0].children.len(), 2);
    assert_eq!(r.stats.phases[&Phase::Section].started, 2);
    assert_eq!(r.stats.robots[&RobotsOutcome::Missing], 1);
    assert!(r.stats.budgets.is_empty());
}
#[test]
fn passage_first_citation_then_ranked_pages() {
    let r = run(
        Config {
            result_pages: 1,
            ..cfg()
        },
        "how to reset timer",
        include_str!("fixtures/engine-groups.html"),
        |e| {
            if let Effect::Fetch { id, url, .. } = e {
                if url.path() == "/guide" {
                    return Some(response(
                        *id,
                        url,
                        "text/html",
                        "<main><p>Reset the timer using the side button.</p></main>",
                    ));
                }
            }
            None
        },
    );
    assert_eq!(
        r.trace,
        [
            "kind",
            "load",
            "layer",
            "passages",
            "fetch sample.example/robots.txt",
            "fetch sample.example/guide",
            "done"
        ]
    );
    assert_eq!(r.passages[0], ["The reset switch clears the saved timer."]);
}
#[test]
fn compare_resolves_one_missing_side_and_handles_unavailable() {
    let comparator = "acme vs other";
    let r = run(cfg(), comparator, RESULT, |e| match e {
        Effect::ReadResultsAnonymously { id, .. } => Some(Input::Results {
            id: *id,
            result: Ok(doc(ENGINE, &RESULT.replace("acme.com", "other.com"))),
        }),
        Effect::Fetch { id, url, .. } if url.path() == "/" => Some(response(
            *id,
            url,
            "text/html",
            "<nav><a href='/about'>About</a></nav>",
        )),
        _ => None,
    });
    assert_eq!(
        r.trace,
        [
            "kind",
            "load",
            "layer",
            "lookup acme.com",
            "fetch duckduckgo.com/robots.txt",
            "anon other",
            "lookup other.com",
            "fetch acme.com/robots.txt",
            "fetch acme.com/",
            "place",
            "fetch other.com/robots.txt",
            "fetch other.com/",
            "place",
            "fetch acme.com/llms.txt",
            "fetch other.com/llms.txt",
            "fetch acme.com/sitemap.xml",
            "fetch other.com/sitemap.xml",
            "complete",
            "remember acme.com",
            "complete",
            "remember other.com",
            "compare",
            "done"
        ]
    );
    let r = run(cfg(), comparator, RESULT, |_| None);
    assert!(!r.trace.iter().any(|e| e == "compare"));
    assert_eq!(
        r.stats.phases[&Phase::AnonymousResults].refused[&Failure::Unavailable],
        1
    );
}
#[test]
fn memo_replay_stale_and_invalid_versions() {
    let base = run(cfg(), "acme", RESULT, |_| None);
    let memo = base.memo.unwrap();
    let answer = |c, bytes: &Vec<u8>| {
        run(c, "acme", RESULT, |e| {
            if let Effect::LookupSite { id, .. } = e {
                Some(Input::Cache {
                    id: *id,
                    memo: Some(bytes.clone()),
                })
            } else {
                None
            }
        })
    };
    assert_eq!(
        answer(cfg(), &memo).trace,
        [
            "kind",
            "load",
            "layer",
            "lookup acme.com",
            "complete",
            "done"
        ]
    );
    let stale = answer(
        Config {
            now: cfg().now + MEMO_TTL,
            ..cfg()
        },
        &memo,
    );
    assert_eq!(
        &stale.trace[4..7],
        ["stale", "fetch acme.com/robots.txt", "fetch acme.com/"]
    );
    for invalid in [
        vec![b' '; MAX_MEMO_BYTES + 1],
        br#"{"version":0}"#.to_vec(),
        br#"{"version":2}"#.to_vec(),
        b"null".to_vec(),
    ] {
        assert_eq!(answer(cfg(), &invalid).trace, base.trace);
    }
}
#[test]
fn disallowed_paths_robots_failures_and_redirects() {
    for failure in [Failure::Server, Failure::Unreadable, Failure::Timeout] {
        let r = run(cfg(), "acme", RESULT, |e| match e {
            Effect::Fetch { id, url, .. } if url.path() == "/robots.txt" => Some(Input::Fetch {
                id: *id,
                result: Err(failure),
            }),
            _ => None,
        });
        assert_eq!(
            r.trace,
            [
                "kind",
                "load",
                "layer",
                "lookup acme.com",
                "fetch acme.com/robots.txt",
                "place_failed",
                "done"
            ]
        );
        assert_eq!(r.stats.phases[&Phase::Entry].refused[&Failure::Robots], 1);
    }
    let r = run(cfg(), "acme", RESULT, |e| match e {
        Effect::Fetch { id, url, .. } if url.path() == "/robots.txt" => Some(response(
            *id,
            url,
            "text/plain",
            "User-agent: *\nDisallow: /pricing\nDisallow: /llms.txt\nDisallow: /sitemap.xml",
        )),
        _ => None,
    });
    for phase in [Phase::Section, Phase::Llms, Phase::Sitemap] {
        assert_eq!(r.stats.phases[&phase].refused[&Failure::Robots], 1);
    }
    assert!(r.places.last().unwrap().notes.robots_disallowed);
    let r = run(cfg(), "acme", RESULT, |e| match e {
        Effect::Fetch { id, url, .. } if url.as_str() == HOME => Some(response(
            *id,
            &Url::parse("https://www.acme.com/start").unwrap(),
            "text/html",
            NAV,
        )),
        _ => None,
    });
    assert_eq!(r.places[0].entry.as_str(), "https://www.acme.com/");
    assert!(
        r.trace
            .iter()
            .position(|s| s == "fetch www.acme.com/robots.txt")
            < r.trace
                .iter()
                .position(|s| s == "fetch www.acme.com/pricing")
    );
    let r = run(cfg(), "acme", RESULT, |e| match e {
        Effect::Fetch { id, url, .. } if url.as_str() == HOME => Some(response(
            *id,
            &Url::parse("https://unrelated.example/").unwrap(),
            "text/html",
            NAV,
        )),
        _ => None,
    });
    assert!(r.places.is_empty());
    assert_eq!(r.stats.phases[&Phase::Entry].refused[&Failure::External], 1);
}
#[test]
fn unavailable_render_settles_and_preserves_the_shell() {
    let r = run(cfg(), "acme", RESULT, |e| match e {
        Effect::Fetch { id, url, .. } if url.as_str() == HOME => Some(response(
            *id,
            url,
            "text/html",
            include_str!("fixtures/shell.html"),
        )),
        _ => None,
    });
    assert_eq!(
        r.trace,
        [
            "kind",
            "load",
            "layer",
            "lookup acme.com",
            "fetch acme.com/robots.txt",
            "fetch acme.com/",
            "place",
            "render",
            "fetch acme.com/llms.txt",
            "fetch acme.com/sitemap.xml",
            "complete",
            "done"
        ]
    );
    assert_eq!(
        r.stats.phases[&Phase::Render].refused[&Failure::Unavailable],
        1
    );
}
#[test]
fn every_budget_settles() {
    use Budget::*;
    let cases = [
        (
            Config {
                fetches: 1,
                ..cfg()
            },
            Fetches,
        ),
        (
            Config {
                host_fetches: 1,
                ..cfg()
            },
            HostFetches,
        ),
        (
            Config {
                concurrent: 0,
                ..cfg()
            },
            Concurrent,
        ),
        (
            Config {
                host_concurrent: 0,
                ..cfg()
            },
            HostConcurrent,
        ),
        (
            Config {
                html_bytes: 1,
                ..cfg()
            },
            HtmlBytes,
        ),
        (
            Config {
                xml_bytes: 0,
                ..cfg()
            },
            XmlBytes,
        ),
        (
            Config {
                text_bytes: 0,
                ..cfg()
            },
            TextBytes,
        ),
        (
            Config {
                section_pages: 1,
                ..cfg()
            },
            SectionPages,
        ),
        (
            Config {
                result_pages: 0,
                ..cfg()
            },
            ResultPages,
        ),
        (
            Config {
                elapsed: Duration::ZERO,
                ..cfg()
            },
            Elapsed,
        ),
        (
            Config {
                last_start: Duration::ZERO,
                ..cfg()
            },
            LastStart,
        ),
        (
            Config {
                timeout: Duration::ZERO,
                ..cfg()
            },
            Timeout,
        ),
    ];
    for (c, limit) in cases {
        let q = if limit == ResultPages {
            "how does acme work"
        } else {
            "acme"
        };
        assert!(
            run(c, q, RESULT, |_| None).stats.budgets.contains(&limit),
            "{limit:?}"
        );
    }
}
#[test]
fn cancel_and_duplicate_inputs_at_every_stage() {
    for cutoff in 0..9 {
        let (mut s, start) = Session::start(cfg(), "acme").unwrap();
        let mut events: VecDeque<_> = start.into();
        let mut delivered = 0;
        while let Some(effect) = events.pop_front() {
            let id = effect.id();
            let wrong = Input::Cache {
                id: Id {
                    session: id.session + 1,
                    ..id
                },
                memo: None,
            };
            assert!(s.handle(wrong).is_empty());
            let input = match effect {
                Effect::LoadResults { .. } => Input::Results {
                    id,
                    result: Ok(doc(ENGINE, RESULT)),
                },
                Effect::LookupSite { .. } => Input::Cache { id, memo: None },
                Effect::Fetch { url, .. } if url.path() == "/robots.txt" => {
                    response(id, &url, "text/plain", "")
                }
                Effect::Fetch { url, .. } if url.as_str() == HOME => {
                    response(id, &url, "text/html", NAV)
                }
                Effect::Fetch { .. } => Input::Fetch {
                    id,
                    result: Err(Failure::Missing),
                },
                _ => continue,
            };
            if delivered == cutoff {
                let stop = s.handle(Input::Cancel);
                assert!(matches!(
                    stop.last(),
                    Some(Effect::Emit {
                        event: MapEvent::End {
                            outcome: EndOutcome::Cancelled,
                            stats: Stats {
                                cancelled: true,
                                ..
                            }
                        },
                        ..
                    })
                ));
                assert!(stop[..stop.len() - 1].iter().all(|e| matches!(
                    e,
                    Effect::Emit {
                        event: MapEvent::Place { .. } | MapEvent::PlaceFailed { .. },
                        ..
                    }
                )));
                assert!(s.handle(input).is_empty());
                break;
            }
            delivered += 1;
            events.extend(s.handle(input));
            assert!(s
                .handle(Input::Fetch {
                    id,
                    result: Err(Failure::Server)
                })
                .is_empty());
            assert!(s
                .handle(Input::Results {
                    id,
                    result: Err(Failure::Server)
                })
                .is_empty());
        }
        assert!(s.handle(Input::Cancel).is_empty());
    }
}
proptest! {
    #[test]
    fn arbitrary_delivery_respects_policy_identity_and_caps(ops in proptest::collection::vec(any::<u8>(),1..70), max in 0usize..14, host_max in 0usize..9, slots in 1usize..5){
        let c=Config{fetches:max as u32,host_fetches:host_max as u32,concurrent:slots as u32,host_concurrent:slots as u32,..cfg()};
        let (mut s,start)=Session::start(c,"acme").unwrap();
        let mut emitted=start;let mut pending=Vec::new();
        let mut requests=0usize;let mut done=0;let mut robots_read=false;
        for op in ops.into_iter().chain([255,255]) {
            for e in emitted.drain(..){
                if let Effect::Fetch{ref url,..}=e{
                    requests+=1;
                    prop_assert!(requests<=max.min(host_max));
                    prop_assert!(url.path()=="/robots.txt" || robots_read);
                    prop_assert_ne!(url.path(),"/pricing");
                }
                match e{Effect::Emit{event:MapEvent::End{stats:_,..},..}=>done+=1,Effect::Emit{..}|Effect::RememberSite{..}=>{},_=>pending.push(e)}
            }
            prop_assert!(done<=1);
            prop_assert!(pending.iter().filter(|e|matches!(e,Effect::Fetch{..})).count()<=slots,"too many simultaneous fetches");
            let input=if op==255{Input::Cancel}else if op%17==0{Input::Elapsed{time:Duration::from_secs(u64::from(op%20))}}else if op%7==0 || pending.is_empty(){Input::Fetch{id:Id{session:42,sequence:1000+u32::from(op)},result:Err(Failure::Unreadable)}}else{
                let i=usize::from(op)%pending.len();let e=pending.swap_remove(i);let id=e.id();
                match e{
                    Effect::LoadResults{..}=>Input::Results{id,result:Ok(doc(ENGINE,RESULT))},
                    Effect::LookupSite{..}=>Input::Cache{id,memo:Some(vec![op;usize::from(op)])},
                    Effect::Fetch{url,..} if url.path()=="/robots.txt"=>{robots_read=true;response(id,&url,"text/plain","User-agent: *\nDisallow: /pricing")},
                    Effect::Fetch{url,..} if url.path()=="/"=>response(id,&url,"text/html",NAV),
                    _=>Input::Fetch{id,result:Err(Failure::Missing)},
                }
            };
            emitted=s.handle(input);
        }
        prop_assert!(emitted.is_empty());
        prop_assert_eq!(done,1);
    }
}

#[test]
fn malformed_memo_cannot_supply_an_unrelated_destination() {
    let original = run(cfg(), "acme", RESULT, |_| None).memo.unwrap();
    let site = PlaceKey::from_url(&Url::parse(HOME).unwrap()).unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&original).unwrap();
    for raw in [
        "javascript:alert(1)",
        "https://other.example/unrelated",
        "https://user@acme.com/",
    ] {
        value["payload"]["place"]["sections"][0]["key"] = raw.into();
        assert!(SiteMemo::decode(&serde_json::to_vec(&value).unwrap(), &site).is_err());
    }
    assert!(matches!(
        SiteMemo::decode(&vec![0; MAX_MEMO_BYTES + 1], &site),
        Err(MemoError::Limit)
    ));
}
#[test]
fn media_headers_and_truncation_have_typed_outcomes() {
    for (length, accepted) in [
        (MAX_MEDIA_TYPE_BYTES, true),
        (MAX_MEDIA_TYPE_BYTES + 1, false),
    ] {
        let media = format!("text/html;{}", "x".repeat(length - 10));
        let r = run(cfg(), "acme", RESULT, |e| match e {
            Effect::Fetch { id, url, .. } if url.as_str() == HOME => {
                Some(response(*id, url, &media, NAV))
            }
            _ => None,
        });
        assert_eq!(!r.places.is_empty(), accepted);
        if !accepted {
            assert_eq!(
                r.stats.phases[&Phase::Entry].refused[&Failure::ContentType],
                1
            );
        }
    }
    let r = run(cfg(), "acme", RESULT, |e| match e {
        Effect::Fetch { id, url, .. } if url.path() == "/llms.txt" => Some(Input::Fetch {
            id: *id,
            result: Ok(FetchResult {
                final_url: url.clone(),
                content_type: "text/plain".into(),
                charset: None,
                bytes: vec![],
                truncated: true,
            }),
        }),
        _ => None,
    });
    assert!(r.stats.places[&0].llms_truncated);
    assert!(r.stats.budgets.contains(&Budget::TextBytes));
}
#[test]
fn late_time_limits_allow_only_completions_and_one_terminal_event() {
    let (mut session, start) = Session::start(cfg(), "acme vs other").unwrap();
    let id = start[1].id();
    assert!(session
        .handle(Input::Elapsed { time: LAST_START })
        .is_empty());
    let end = session.handle(Input::Results {
        id,
        result: Ok(doc(ENGINE, RESULT)),
    });
    assert_eq!(
        end.iter().map(name).collect::<Vec<_>>(),
        ["layer", "place_failed", "done"]
    );
    assert!(
        matches!(end.last(),Some(Effect::Emit{event:MapEvent::End{stats:s,..},..}) if s.budgets.contains(&Budget::LastStart))
    );
    let (mut session, start) = Session::start(cfg(), "acme").unwrap();
    let id = start[1].id();
    assert!(matches!(
        session
            .handle(Input::Elapsed { time: MAX_ELAPSED })
            .as_slice(),
        [Effect::Emit {
            event: MapEvent::End { stats: _, .. },
            ..
        }]
    ));
    assert!(session
        .handle(Input::Results {
            id,
            result: Ok(doc(ENGINE, RESULT))
        })
        .is_empty());
}
#[test]
fn cross_domain_section_redirect_only_changes_the_landing() {
    let r = run(cfg(), "acme", RESULT, |e| match e {
        Effect::Fetch { id, url, .. } if url.path() == "/pricing" => Some(response(
            *id,
            &Url::parse("https://other.example/new").unwrap(),
            "text/html",
            "<nav><a href='/injected'>Injected</a></nav>",
        )),
        _ => None,
    });
    let p = &r.places.last().unwrap().sections[0];
    assert!(p.off_site && p.children.is_empty());
    assert_eq!(p.key.as_str(), "https://other.example/new");
    assert!(!r.trace.iter().any(|e| e.contains("other.example")));
}

#[test]
fn a_section_landing_in_another_tenant_contributes_nothing() {
    let results = "<main><article><h2><a href='https://github.com/org/first'>GitHub first</a></h2></article></main>";
    let r = run(cfg(), "github", results, |e| {
        match e {
        Effect::Fetch { id, url, .. } if url.path() == "/org/first" => Some(response(
            *id,
            url,
            "text/html",
            "<title>first</title><nav><a href='/org/first/pricing'>Pricing</a><a href='/org/first/news'>News</a></nav>",
        )),
        Effect::Fetch { id, url, .. } if url.path() == "/org/first/pricing" => Some(response(
            *id,
            &Url::parse("https://github.com/org/second/pricing").unwrap(),
            "text/html",
            include_str!("fixtures/section-pricing.html"),
        )),
        Effect::Fetch { id, url, .. } if url.path() == "/org/first/news" => Some(response(
            *id,
            url,
            "text/html",
            include_str!("fixtures/section-changes.html"),
        )),
        _ => None,
    }
    });
    let place = r.places.last().unwrap();
    assert_eq!(place.key.tenant.as_deref(), Some("/org/first"));
    let entry = |label: &str| place.sections.iter().find(|e| e.label == label).unwrap();
    let moved = entry("Pricing");
    assert_eq!(moved.key.as_str(), "https://github.com/org/second/pricing");
    assert!(moved.children.is_empty() && !moved.off_site);
    assert!(moved.locator == Locator::default());
    assert_eq!(
        moved.read,
        Some(ReadState::Refused {
            failure: Failure::External
        })
    );
    assert!(!entry("News").children.is_empty());
    assert_eq!(
        r.stats.phases[&Phase::Section].refused[&Failure::External],
        1
    );
}

#[test]
fn named_place_passage_shares_one_entry_read() {
    let r = run(cfg(), "how do acme.com timers work", RESULT, |e| match e {
        Effect::Fetch { id, url, .. } if url.as_str() == HOME => Some(response(
            *id,
            url,
            "text/html",
            "<main><p>Acme.com timers work with a daily reset.</p></main>",
        )),
        _ => None,
    });
    assert_eq!(
        r.trace,
        [
            "kind",
            "load",
            "layer",
            "lookup acme.com",
            "fetch acme.com/robots.txt",
            "fetch acme.com/",
            "place",
            "fetch acme.com/llms.txt",
            "fetch acme.com/sitemap.xml",
            "passages",
            "complete",
            "remember acme.com",
            "done"
        ]
    );
    assert_eq!(r.passages[0], ["Acme.com timers work with a daily reset."]);
}
#[test]
fn stale_identical_place_still_emits_fresh_replacement() {
    let html = "<title>Acme</title><nav><a href='/about'>About</a></nav>";
    let first = run(cfg(), "acme", RESULT, |e| match e {
        Effect::Fetch { id, url, .. } if url.as_str() == HOME => {
            Some(response(*id, url, "text/html", html))
        }
        _ => None,
    });
    let second = run(
        Config {
            now: cfg().now + MEMO_TTL,
            ..cfg()
        },
        "acme",
        RESULT,
        |e| match e {
            Effect::LookupSite { id, .. } => Some(Input::Cache {
                id: *id,
                memo: first.memo.clone(),
            }),
            Effect::Fetch { id, url, .. } if url.as_str() == HOME => {
                Some(response(*id, url, "text/html", html))
            }
            _ => None,
        },
    );
    assert_eq!(second.places.len(), 3);
    assert!(second.places[0] == second.places[1]);
    assert_eq!(second.stats.stale_memos, 1);
    assert!(
        second.trace.iter().position(|e| e == "stale")
            < second.trace.iter().position(|e| e == "place")
    );
}

#[test]
fn results_failures_and_wall_events_are_terminal() {
    for result in [
        Err(Failure::Unavailable),
        Ok(doc(ENGINE, "<h1>Verify you are human</h1>")),
    ] {
        let (mut s, start) = Session::start(cfg(), "acme").unwrap();
        let id = start[1].id();
        assert!(matches!(
            s.handle(Input::Results { id, result }).as_slice(),
            [Effect::Emit {
                event: MapEvent::End {
                    outcome: EndOutcome::Failed {
                        failure: Failure::Unavailable | Failure::Challenge
                    },
                    ..
                },
                ..
            }]
        ));
        assert!(s.handle(Input::Cancel).is_empty());
    }
}
#[test]
fn passage_updates_include_answers_from_other_cited_pages() {
    let r = run(
        Config {
            result_pages: 2,
            ..cfg()
        },
        "how to reset timer",
        include_str!("fixtures/engine-groups.html"),
        |e| match e {
            Effect::Fetch { id, url, .. } if url.path() == "/item" => Some(response(
                *id,
                url,
                "text/html",
                "<main><p>Reset the timer using the side button.</p></main>",
            )),
            _ => None,
        },
    );
    assert_eq!(r.passages.len(), 2);
    assert_eq!(
        r.passages[1],
        [
            "The reset switch clears the saved timer.",
            "Reset the timer using the side button."
        ]
    );
}

#[test]
fn repository_memo_cannot_cross_tenants_or_store_result_evidence() {
    let search = |repo: &str, memo: Option<Vec<u8>>| {
        let root = format!("/org/{repo}");
        let html = include_str!("fixtures/heading-sitelinks.html")
            .replace("https://acme.com/", &format!("https://github.com{root}/"))
            .replace("Acme tools", "GitHub repository")
            .replace(
                &format!("{root}/\""),
                &format!("{root}/tree/main?person=private\""),
            )
            .replace("/products", "/private-plans");
        run(cfg(), "github", &html, |e| match e {
            Effect::LookupSite { id, site } => {
                assert_eq!(site.tenant.as_deref(), Some(root.as_str()));
                Some(Input::Cache {
                    id: *id,
                    memo: memo.clone(),
                })
            }
            Effect::Fetch { id, url, .. } if url.path() == root => {
                assert!(url.query().is_none());
                Some(response(
                    *id,
                    url,
                    "text/html",
                    &format!("<title>{repo}</title><nav><a href='{root}/about'>About</a></nav>"),
                ))
            }
            _ => None,
        })
    };
    let first = search("first", None);
    assert!(first.places.last().unwrap().sections.iter().any(|e| e
        .key
        .as_url()
        .path()
        .ends_with("private-plans")));
    let memo = first.memo.unwrap();
    let stored = std::str::from_utf8(&memo).unwrap();
    for private in ["tree/main", "person=", "private-plans", "EngineSitelink"] {
        assert!(!stored.contains(private));
    }
    let json: serde_json::Value = serde_json::from_slice(&memo).unwrap();
    assert_eq!(
        json["payload"]["place"]["entry"],
        "https://github.com/org/first"
    );
    assert!(json["payload"].get("read_at").is_none());
    let next = search("second", Some(memo));
    assert_eq!(next.stats.rejected_memos, 1);
    assert_eq!(next.stats.fresh_memos, 0);
    assert!(next.places.iter().all(|p| p.title == "second"));
}

#[test]
fn a_redirected_robots_file_governs_the_landed_origin_only_as_its_own_file() {
    for (landing, allowed, reads) in [
        ("https://www.acme.com/robots.txt", true, 1),
        ("https://www.acme.com/en/robots.txt", true, 2),
        ("https://www.acme.com/robots.txt?lang=en", true, 2),
        ("https://other.com/robots.txt", false, 1),
        ("https://acme.com/consent", false, 1),
    ] {
        let r = run(cfg(), "acme", RESULT, |e| match e {
            Effect::Fetch { id, url, .. } if url.path() == "/robots.txt" => Some(response(
                *id,
                &Url::parse(landing).unwrap(),
                "text/plain",
                "User-agent: *\nDisallow: /pricing",
            )),
            Effect::Fetch { id, url, .. } if url.as_str() == HOME => Some(response(
                *id,
                &Url::parse("https://www.acme.com/").unwrap(),
                "text/html",
                NAV,
            )),
            _ => None,
        });
        assert_eq!(r.stats.phases[&Phase::Robots].started, reads, "{landing}");
        assert_eq!(
            r.trace
                .iter()
                .filter(|s| s.as_str() == "fetch www.acme.com/robots.txt")
                .count() as u32,
            reads - 1
        );
        assert_eq!(!r.places.is_empty(), allowed);
        assert!(!r.trace.iter().any(|s| s.ends_with("/pricing")));
        if allowed {
            assert_eq!(r.stats.phases[&Phase::Section].refused[&Failure::Robots], 1);
        } else {
            assert_eq!(
                r.stats.phases[&Phase::Robots].refused[&Failure::External],
                1
            );
        }
    }
}

#[test]
fn sitemap_samples_obey_read_byte_and_section_bounds() {
    let robots = (0..10)
        .map(|n| format!("Sitemap: https://acme.com/map-{n}.xml"))
        .collect::<Vec<_>>()
        .join("\n");
    for n in [SITEMAP_SECTION_THRESHOLD - 1, SITEMAP_SECTION_THRESHOLD] {
        let nav = (0..n - 1)
            .map(|i| format!("<a href='/s{i}'>S{i}</a>"))
            .collect::<String>();
        let mut maps = Vec::new();
        let r = run(cfg(), "acme", RESULT, |e| match e {
            Effect::Fetch { id, url, .. } if url.path() == "/robots.txt" => {
                Some(response(*id, url, "text/plain", &robots))
            }
            Effect::Fetch { id, url, .. } if url.path() == "/" => Some(response(
                *id,
                url,
                "text/html",
                &format!("<nav>{nav}</nav>"),
            )),
            Effect::Fetch { id, url, .. } if url.path() == "/llms.txt" => {
                Some(response(*id, url, "text/plain", "## More\n- [Item](/more)"))
            }
            Effect::Fetch {
                id,
                url,
                expect: Expect::Xml,
                max_bytes,
                ..
            } => {
                assert_eq!(*max_bytes, 512 * 1024);
                maps.push(url.path().to_owned());
                if maps.len() == 1 {
                    let records = ["posts", "fr-pages", "en-docs"]
                        .map(|name| {
                            format!("<sitemap><loc>https://acme.com/{name}.xml</loc></sitemap>")
                        })
                        .join("");
                    Some(response(
                        *id,
                        url,
                        "application/xml",
                        &format!("<sitemapindex>{records}</sitemapindex>"),
                    ))
                } else {
                    let mut bytes =
                        b"<urlset><url><loc>https://acme.com/sampled</loc></url>".to_vec();
                    bytes.resize(*max_bytes as usize - 1, b' ');
                    bytes.push(b'<');
                    Some(Input::Fetch {
                        id: *id,
                        result: Ok(FetchResult {
                            final_url: url.clone(),
                            content_type: "application/xml".into(),
                            charset: None,
                            bytes,
                            truncated: true,
                        }),
                    })
                }
            }
            _ => None,
        });
        if n == SITEMAP_SECTION_THRESHOLD {
            assert!(maps.is_empty());
        } else {
            assert_eq!(maps, ["/map-0.xml", "/en-docs.xml"]);
            assert_eq!(
                r.stats.phases[&Phase::Sitemap].started,
                MAX_SITEMAP_READS as u32
            );
            assert!(r.stats.places[&0].sitemap_truncated);
            assert!(r
                .places
                .last()
                .unwrap()
                .sections
                .iter()
                .any(|e| e.label == "sampled"));
        }
    }
}

#[test]
fn section_ids_and_reads_survive_landing_refusal_and_budget() {
    let r = run(
        Config {
            section_pages: 2,
            ..cfg()
        },
        "acme",
        RESULT,
        |e| {
            match e{
        Effect::Fetch{id,url,..} if url.path()=="/"=>Some(response(*id,url,"text/html","<nav><a href='/docs'>Docs</a><a href='/news'>News</a><a href='/pricing'>Prices</a></nav>")),
        Effect::Fetch{id,url,..} if url.path()=="/docs"=>Some(response(*id,&url.join("/guide").unwrap(),"text/html","<h2 id='guide'>Guide</h2>")),
        Effect::Fetch{id,url,..} if url.path()=="/news"=>Some(Input::Fetch{id:*id,result:Err(Failure::Server)}),
        _=>None
    }
        },
    );
    let first = &r.places[0];
    let last = r.places.last().unwrap();
    assert!(first
        .sections
        .iter()
        .all(|e| e.read == Some(ReadState::Queued)));
    assert_eq!(
        first.sections.iter().map(|e| &e.id).collect::<Vec<_>>(),
        last.sections.iter().map(|e| &e.id).collect::<Vec<_>>()
    );
    assert_eq!(last.sections[0].key.as_url().path(), "/guide");
    assert_eq!(last.sections[0].read, Some(ReadState::Read));
    assert_eq!(
        last.sections[1].read,
        Some(ReadState::Refused {
            failure: Failure::Server
        })
    );
    assert_eq!(
        last.sections[2].read,
        Some(ReadState::Skipped {
            budget: Budget::SectionPages
        })
    );
}

#[test]
fn deep_seed_and_root_are_separate_reads_when_the_plan_names_the_section_or_passage() {
    let results = RESULT
        .replace("acme.com/", "acme.com/pricing?plan=one")
        .replace("Acme workshop", "Acme pricing");
    for query in ["acme pricing", "how do acme.com timers work"] {
        let r = run(cfg(), query, &results, |e| {
            match e{
            Effect::Fetch{id,url,..} if url.path()=="/"=>Some(response(*id,url,"text/html","<title>Workshop</title>")),
            Effect::Fetch{id,url,..} if url.path()=="/pricing"=>Some(response(*id,url,"text/html","<main><h2 id='plans'>Plans</h2><p>Acme.com timers work with a daily reset.</p></main>")),
            _=>None
        }
        });
        assert!(r.trace.iter().any(|s| s == "fetch acme.com/"));
        assert!(r.trace.iter().any(|s| s == "fetch acme.com/pricing"));
        assert_eq!(r.places.last().unwrap().entry.as_url().as_str(), HOME);
        assert!(!std::str::from_utf8(r.memo.as_ref().unwrap())
            .unwrap()
            .contains("plan=one"));
    }
}

#[test]
fn duplicate_ids_and_session_evidence_are_invalid_in_a_memo() {
    let memo = run(cfg(), "acme", RESULT, |_| None).memo.unwrap();
    let key = PlaceKey::from_url(&Url::parse(HOME).unwrap()).unwrap();
    let original: serde_json::Value = serde_json::from_slice(&memo).unwrap();
    for kind in ["id", "short_id", "long_id", "source", "read"] {
        let mut value = original.clone();
        let entries = &mut value["payload"]["place"]["sections"];
        match kind {
            "id" => entries[0]["children"][0]["id"] = entries[1]["id"].clone(),
            "short_id" => entries[0]["id"] = "f".repeat(site::ENTRY_ID_BYTES - 1).into(),
            "long_id" => entries[0]["id"] = "f".repeat(site::ENTRY_ID_BYTES + 1).into(),
            "source" => entries[0]["sources"] = serde_json::json!(["EngineSitelink"]),
            _ => entries[0]["read"] = "Queued".into(),
        }
        assert!(matches!(
            SiteMemo::decode(&serde_json::to_vec(&value).unwrap(), &key),
            Err(MemoError::Invalid)
        ));
    }
}

#[test]
fn stats_keep_engine_repeat_and_engine_owned_drop_counts() {
    let markup = include_str!(
        "../../zephium-agentic/src/provider_transport/decision/fixtures/search/google.html"
    );
    let r = run(
        Config {
            engine: zephium_core::search::SearchEngine::Google,
            ..cfg()
        },
        "rust async runtime",
        markup,
        |e| match e {
            Effect::LoadResults { id, .. } => Some(Input::Results {
                id: *id,
                result: Ok(doc("https://google.com/search", markup)),
            }),
            _ => None,
        },
    );
    assert_eq!((r.stats.results.own, r.stats.results.repeat), (1, 1));
}

#[test]
fn redirected_host_can_supply_a_live_place_without_writing_the_seeds_memo() {
    let r = run(cfg(), "acme", RESULT, |e| match e {
        Effect::Fetch { id, url, .. } if url.as_str() == HOME => Some(response(
            *id,
            &Url::parse("https://docs.acme.com/intro").unwrap(),
            "text/html",
            "<title>Manual</title>",
        )),
        _ => None,
    });
    assert_eq!(r.places.last().unwrap().key.host, "docs.acme.com");
    assert!(r.memo.is_none());
}

#[test]
fn robots_uses_its_own_ceiling_and_enforces_complete_rules_from_a_prefix() {
    let run = run(cfg(), "acme", RESULT, |effect| {
        let Effect::Fetch {
            id, url, max_bytes, ..
        } = effect
        else {
            return None;
        };
        match url.path() {
            "/robots.txt" => {
                assert_eq!(*max_bytes as usize, zephium_map::robots::MAX_ROBOTS_BYTES);
                let mut bytes = b"User-agent: *\nDisallow: /pricing\n#".to_vec();
                bytes.resize(*max_bytes as usize - b"\nDisallow: /".len(), b' ');
                bytes.extend_from_slice(b"\nDisallow: /");
                Some(Input::Fetch {
                    id: *id,
                    result: Ok(FetchResult {
                        final_url: url.clone(),
                        content_type: "text/plain".into(),
                        charset: None,
                        bytes,
                        truncated: true,
                    }),
                })
            }
            "/llms.txt" => {
                assert_eq!(*max_bytes as usize, MAX_TEXT_BYTES);
                None
            }
            "/pricing" => panic!("complete disallow line was ignored"),
            _ => None,
        }
    });
    assert!(run.stats.places[&0].robots_truncated);
    assert!(run.places.last().unwrap().notes.robots_disallowed);
}

#[test]
fn a_robots_sample_cut_before_any_rule_denies_the_site() {
    let r = run(
        Config {
            text_bytes: 1,
            ..cfg()
        },
        "acme",
        RESULT,
        |effect| match effect {
            Effect::Fetch {
                id, url, max_bytes, ..
            } if url.path() == "/robots.txt" => {
                assert_eq!(*max_bytes, 1);
                Some(response(
                    *id,
                    url,
                    "text/plain",
                    "User-agent: *\nAllow: /\n",
                ))
            }
            Effect::Fetch { .. } => panic!("read without robots evidence"),
            _ => None,
        },
    );
    assert_eq!(r.stats.robots[&RobotsOutcome::Unreadable], 1);
    assert_eq!(r.stats.phases[&Phase::Entry].refused[&Failure::Robots], 1);
    assert!(r.places.is_empty());
}

#[test]
fn cached_place_recomputes_focus_for_the_current_query() {
    let first = run(cfg(), "acme pricing", RESULT, |_| None);
    let memo = first.memo.unwrap();
    let first = first.places.last().unwrap();
    assert!(first
        .sections
        .iter()
        .any(|e| e.key.as_url().path() == "/pricing" && Some(&e.id) == first.focus.as_ref()));
    let next = run(cfg(), "acme news", RESULT, |e| match e {
        Effect::LookupSite { id, .. } => Some(Input::Cache {
            id: *id,
            memo: Some(memo.clone()),
        }),
        _ => None,
    });
    assert_eq!(next.stats.fresh_memos, 1);
    let place = next.places.last().unwrap();
    assert!(place
        .sections
        .iter()
        .any(|e| e.key.as_url().path() == "/news" && Some(&e.id) == place.focus.as_ref()));
}

#[test]
fn qualified_section_focus_requires_the_named_subject_and_fresh_focus_uses_editorial_sections() {
    let home = "<nav><a href='/docs'>Docs</a><a href='/guides/storage/docs'>Storage docs</a><a href='/blog'>Blog</a></nav>";
    for (q, expected) in [
        ("acme storage docs", Some("/guides/storage/docs")),
        ("acme rendering docs", None),
        ("acme news", Some("/blog")),
    ] {
        let run = run(cfg(), q, RESULT, |effect| match effect {
            Effect::Fetch { id, url, .. } if url.path() == "/" => {
                Some(response(*id, url, "text/html", home))
            }
            _ => None,
        });
        let place = run.places.last().unwrap();
        let entry = place
            .sections
            .iter()
            .find(|e| Some(&e.id) == place.focus.as_ref());
        assert_eq!(entry.map(|e| e.key.as_url().path()), expected, "{q}");
    }
}

#[test]
fn total_bytes_stops_new_reads_at_the_boundary_and_preserves_focus_order() {
    let query = "acme news";
    let small = run(cfg(), query, RESULT, |_| None);
    assert!(!small.stats.budgets.contains(&Budget::TotalBytes));
    let section_bytes = include_str!("fixtures/section-changes.html").len();
    let boundary = (NAV.len() + section_bytes) as u64;
    let capped = run(
        Config {
            total_bytes: boundary,
            ..cfg()
        },
        query,
        RESULT,
        |_| None,
    );
    assert_eq!(capped.stats.phases[&Phase::Section].started, 1);
    assert!(capped.trace.contains(&"fetch acme.com/news".into()));
    assert!(!capped.trace.contains(&"fetch acme.com/pricing".into()));
    assert!(capped.places.last().unwrap().sections.iter().any(|e| e.read
        == Some(ReadState::Skipped {
            budget: Budget::TotalBytes
        })));
    assert!(capped.stats.budgets.contains(&Budget::TotalBytes));
    let above = run(
        Config {
            total_bytes: boundary + 1,
            ..cfg()
        },
        query,
        RESULT,
        |_| None,
    );
    assert_eq!(above.stats.phases[&Phase::Section].started, 2);
    let zero = run(
        Config {
            total_bytes: 0,
            ..cfg()
        },
        query,
        RESULT,
        |_| None,
    );
    assert_eq!(zero.stats.phases[&Phase::Entry].completed, 1);
    assert_eq!(zero.stats.phases[&Phase::Robots].completed, 1);
    for phase in [Phase::Section, Phase::Llms, Phase::Sitemap] {
        assert_eq!(zero.stats.phases[&phase].started, 0);
    }
}

#[test]
fn reads_already_in_flight_finish_after_the_total_byte_limit() {
    let result = run(
        Config {
            total_bytes: NAV.len() as u64 + 1,
            concurrent: 2,
            host_concurrent: 2,
            ..cfg()
        },
        "acme news",
        RESULT,
        |_| None,
    );
    assert_eq!(result.stats.phases[&Phase::Section].completed, 2);
    assert_eq!(result.stats.phases[&Phase::Llms].started, 0);
    assert!(result.stats.budgets.contains(&Budget::TotalBytes));
}

#[test]
fn explicit_focus_precedes_earlier_role_sections_even_for_a_nonfresh_search() {
    let result = run(
        Config {
            section_pages: 1,
            ..cfg()
        },
        "acme pricing",
        RESULT,
        |effect| {
            if let Effect::Fetch { id, url, .. } = effect {
                if url.path() == "/" {
                    return Some(response(*id,url,"text/html", "<title>Acme</title><nav><a href='/docs'>Documentation</a><a href='/pricing'>Pricing</a></nav>"));
                }
            }
            None
        },
    );
    assert!(result.trace.contains(&"fetch acme.com/pricing".into()));
    assert!(!result.trace.contains(&"fetch acme.com/docs".into()));
}

#[test]
fn a_passage_read_observes_the_decoded_size_of_robots() {
    let result = run(
        Config {
            total_bytes: 1,
            ..cfg()
        },
        "how to reset timer",
        include_str!("fixtures/engine-groups.html"),
        |effect| {
            if let Effect::Fetch { id, url, .. } = effect {
                if url.path() == "/robots.txt" {
                    return Some(response(*id, url, "text/plain", "\n"));
                }
            }
            None
        },
    );
    assert_eq!(result.stats.phases[&Phase::Passage].started, 0);
    assert!(result.stats.budgets.contains(&Budget::TotalBytes));
}

#[test]
fn unusable_bodies_still_spend_the_default_six_mib() {
    assert_eq!(cfg().total_bytes, MAX_TOTAL_BYTES);
    let mut remaining = MAX_TOTAL_BYTES as usize - NAV.len();
    let result = run(cfg(), "acme news", RESULT, |effect| {
        if let Effect::Fetch { id, url, .. } = effect {
            if matches!(url.path(), "/news" | "/pricing") {
                let bytes = remaining.min(MAX_HTML_BYTES);
                remaining -= bytes;
                return Some(response(
                    *id,
                    url,
                    "application/octet-stream",
                    &" ".repeat(bytes),
                ));
            }
        }
        None
    });
    assert_eq!(remaining, 0);
    assert_eq!(result.stats.phases[&Phase::Section].completed, 2);
    assert_eq!(result.stats.phases[&Phase::Llms].started, 0);
    assert!(result.stats.budgets.contains(&Budget::TotalBytes));
}

#[test]
fn elapsed_before_initial_results_ends_in_a_typed_timeout() {
    let (mut session, _) = Session::start(cfg(), "acme").unwrap();
    let effects = session.handle(Input::Elapsed { time: MAX_ELAPSED });
    assert!(matches!(
        effects.as_slice(),
        [Effect::Emit { event: MapEvent::End { outcome: EndOutcome::Failed { failure: Failure::Timeout }, stats }, .. }]
            if stats.phases[&Phase::Results].refused[&Failure::Timeout] == 1
    ));
}

#[test]
fn missing_comparison_side_reports_result_refusals_and_absent_seeds() {
    for (reply, expected) in [
        (Err(Failure::Unavailable), Failure::Unavailable),
        (Ok(doc(ENGINE, "<p>No results found</p>")), Failure::Missing),
    ] {
        let (mut session, start) = Session::start(cfg(), "acme vs other").unwrap();
        let mut effects: VecDeque<_> = session
            .handle(Input::Results {
                id: start[1].id(),
                result: Ok(doc(ENGINE, RESULT)),
            })
            .into();
        let id = loop {
            match effects.pop_front().unwrap() {
                Effect::ReadResultsAnonymously { id, .. } => break id,
                Effect::LookupSite { id, .. } => {
                    effects.extend(session.handle(Input::Cache { id, memo: None }))
                }
                Effect::Fetch { id, url, .. } => effects.extend(session.handle(response(
                    id,
                    &url,
                    "text/plain",
                    "User-agent: *\n",
                ))),
                _ => {}
            }
        };
        assert!(session.handle(Input::Results { id, result: reply }).iter().any(|effect| matches!(
            effect, Effect::Emit { event: MapEvent::PlaceFailed { slot: 1, failure }, .. } if *failure == expected
        )));
    }
}

#[test]
fn shell_and_wall_memos_cannot_short_circuit_entry_reads() {
    let base = run(cfg(), "acme", RESULT, |_| None);
    let memo = base.memo.unwrap();
    for field in ["shell", "wall"] {
        let mut data: serde_json::Value = serde_json::from_slice(&memo).unwrap();
        data["payload"]["place"]["notes"][field] = serde_json::Value::Bool(true);
        let raw = serde_json::to_vec(&data).unwrap();
        let next = run(cfg(), "acme", RESULT, |e| match e {
            Effect::LookupSite { id, .. } => Some(Input::Cache {
                id: *id,
                memo: Some(raw.clone()),
            }),
            _ => None,
        });
        assert_eq!(next.stats.rejected_memos, 1);
        assert!(next.trace.iter().any(|item| item == "fetch acme.com/"));
    }
}

#[test]
fn sitemap_fetch_requires_a_valid_media_type_essence() {
    for media in [
        "not-a-media-type+xml",
        "/rdf+xml",
        "application/xml/rdf+xml",
    ] {
        let r = run(cfg(), "acme", RESULT, |e| match e {
            Effect::Fetch { id, url, .. } if url.path() == "/sitemap.xml" => {
                Some(response(*id, url, media, "<urlset/>"))
            }
            _ => None,
        });
        assert_eq!(
            r.stats.phases[&Phase::Sitemap]
                .refused
                .get(&Failure::ContentType),
            Some(&1),
            "{media}"
        );
    }
}
