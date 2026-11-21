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
        Effect::LookupSite { site, .. } => format!("lookup {}", site.as_str()),
        Effect::RememberSite { site, .. } => format!("remember {}", site.as_str()),
        Effect::Emit { event, .. } => match event {
            MapEvent::Kind(_) => "kind",
            MapEvent::FirstLayer(_) => "layer",
            MapEvent::Place { stale: true, .. } => "stale",
            MapEvent::Place { .. } => "place",
            MapEvent::Passages(_) => "passages",
            MapEvent::Compare(_) => "compare",
            MapEvent::Done(_) => "done",
            MapEvent::Failed(_) => "failed",
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
                event: MapEvent::Done(s),
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
                event: MapEvent::Passages(p),
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
            "fetch acme.com/sitemap.xml",
            "fetch other.com/llms.txt",
            "fetch other.com/sitemap.xml",
            "remember acme.com",
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
        ["kind", "load", "layer", "lookup acme.com", "place", "done"]
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
    assert_eq!(r.places[0].entry.as_str(), "https://www.acme.com/start");
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
                assert!(matches!(
                    s.handle(Input::Cancel).as_slice(),
                    [Effect::Emit {
                        event: MapEvent::Done(Stats {
                            cancelled: true,
                            ..
                        }),
                        ..
                    }]
                ));
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
        let c=Config{fetches:max,host_fetches:host_max,concurrent:slots,host_concurrent:slots,..cfg()};
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
                match e{Effect::Emit{event:MapEvent::Done(_),..}=>done+=1,Effect::Emit{..}|Effect::RememberSite{..}=>{},_=>pending.push(e)}
            }
            prop_assert!(done<=1);
            prop_assert!(pending.iter().filter(|e|matches!(e,Effect::Fetch{..})).count()<=slots,"too many simultaneous fetches");
            let input=if op==255{Input::Cancel}else if op%17==0{Input::Elapsed(Duration::from_secs(u64::from(op%20)))}else if op%7==0 || pending.is_empty(){Input::Fetch{id:Id{session:42,sequence:1000+u32::from(op)},result:Err(Failure::Unreadable)}}else{
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
    let site = SiteKey::from_url(&Url::parse(HOME).unwrap()).unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&original).unwrap();
    for raw in [
        "javascript:alert(1)",
        "https://other.example/unrelated",
        "https://user@acme.com/",
    ] {
        value["place"]["sections"][0]["key"] = raw.into();
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
    assert!(r.places.last().unwrap().notes.llms_truncated);
    assert!(r.stats.budgets.contains(&Budget::TextBytes));
}
#[test]
fn late_time_limits_allow_only_completions_and_one_terminal_event() {
    let (mut session, start) = Session::start(cfg(), "acme vs other").unwrap();
    let id = start[1].id();
    assert!(session.handle(Input::Elapsed(LAST_START)).is_empty());
    let end = session.handle(Input::Results {
        id,
        result: Ok(doc(ENGINE, RESULT)),
    });
    assert_eq!(end.iter().map(name).collect::<Vec<_>>(), ["layer", "done"]);
    assert!(
        matches!(end.last(),Some(Effect::Emit{event:MapEvent::Done(s),..}) if s.budgets.contains(&Budget::LastStart))
    );
    let (mut session, start) = Session::start(cfg(), "acme").unwrap();
    let id = start[1].id();
    assert!(matches!(
        session.handle(Input::Elapsed(MAX_ELAPSED)).as_slice(),
        [Effect::Emit {
            event: MapEvent::Done(_),
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
    assert!(p.external && p.children.is_empty());
    assert_eq!(p.key.as_str(), "https://other.example/new");
    assert!(!r.trace.iter().any(|e| e.contains("other.example")));
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
    assert_eq!(second.places.len(), 2);
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
                event: MapEvent::Failed(Failure::Unavailable | Failure::Challenge),
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
