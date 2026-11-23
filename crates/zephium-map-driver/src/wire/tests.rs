use super::*;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use zephium_map::{site::SiteInputs, Doc, DocLimits};

fn long_host() -> String {
    format!("{}example.com", "ab.".repeat(MAX_HOST_BYTES / 3))
}

fn base() -> Value {
    let url = Url::parse("https://acme.com/").unwrap();
    let doc = Doc::from_html(
        &url,
        b"<title>Acme</title><nav><a href='/pricing'>Pricing</a><a href='/news'>News</a></nav>",
        None,
        DocLimits::SITE_PAGE,
    )
    .unwrap();
    let inputs = SiteInputs {
        result_section: None,
        sitelinks: &[],
        robots: None,
        llms: None,
        sitemaps: BTreeMap::new(),
        pages: BTreeMap::new(),
    };
    serde_json::to_value(site::read_site(&doc, &inputs).unwrap()).unwrap()
}

fn id(n: usize) -> String {
    format!("{n:016x}")
}

fn section(id: &str, label: &str, key: &str, tier: &str) -> Value {
    json!({
        "children": [],
        "group": null,
        "id": id,
        "key": key,
        "label": label,
        "locator": {},
        "more": 0,
        "off_site": false,
        "role": null,
        "sources": ["Footer"],
        "tier": tier,
    })
}

fn engine(value: Value) -> site::Place {
    serde_json::from_value(value).unwrap()
}

fn hostile() -> site::Place {
    let mut place = base();
    place["title"] = json!(format!("\u{202e}Acme\u{7}\n{}", "T".repeat(1000)));
    place["focus"] = json!("not-an-id");
    place["social"] = json!([
        {"basis": "Navigation", "network": "GitHub", "url": "https://github.com/acme"},
        {"basis": "Navigation", "network": "GitHub", "url": "https://github.com/other"},
    ]);
    let mut sections = Vec::new();
    let mut long = section(
        &id(1),
        &format!("{}\u{0}\u{200b}{}", "L".repeat(200), "é".repeat(400)),
        "https://acme.com/long?token=secret#frag",
        "Secondary",
    );
    long["group"] = json!("G".repeat(1000));
    sections.push(long);
    sections.push(section(
        &id(2),
        "https://evil.example/path?x=1",
        "https://acme.com/a",
        "Primary",
    ));
    sections.push(section(
        &id(3),
        "visit www.evil.example now",
        "https://acme.com/b",
        "Primary",
    ));
    sections.push(section(
        &id(8),
        "https://evil.example/root",
        "https://acme.com/",
        "Primary",
    ));
    sections.push(section("ZZZZ", "Bad", "https://acme.com/c", "Primary"));
    sections.push(section(
        &id(4).to_uppercase().replace('0', "A"),
        "Upper",
        "https://acme.com/d",
        "Primary",
    ));
    sections.push(section(
        &id(5),
        "Redirect",
        "https://acme.com/go/https://evil.example/x?u=1",
        "Primary",
    ));
    sections.push(section(
        &id(6),
        "Deep",
        &format!("https://acme.com/{}", "p".repeat(300)),
        "Primary",
    ));
    sections.push(section(
        &id(9),
        "Guide",
        "https://docs.acme.com/guide",
        "Primary",
    ));
    sections.push(section(
        &id(10),
        "Oversized host",
        &format!("https://{}/guide", long_host()),
        "Primary",
    ));
    let mut parent = section(&id(7), "Parent", "https://acme.com/parent", "Primary");
    let mut children = Vec::new();
    for n in 0..20 {
        let mut child = section(
            &id(100 + n),
            &format!("Child {n}"),
            &format!("https://acme.com/parent/{n}?q=hidden"),
            "Primary",
        );
        if n == 0 {
            child["children"] = json!([section(
                &id(999),
                "Grandchild",
                "https://acme.com/grand",
                "Primary"
            )]);
        }
        children.push(child);
    }
    parent["children"] = Value::Array(children);
    sections.push(parent);
    sections.push(section(
        &id(7),
        "Duplicate",
        "https://acme.com/dup",
        "Primary",
    ));
    for n in 0..40 {
        sections.push(section(
            &id(200 + n),
            &format!("Section {n}"),
            &format!("https://acme.com/s/{n}?session=hidden"),
            if n % 2 == 0 { "Secondary" } else { "Primary" },
        ));
    }
    place["sections"] = Value::Array(sections);
    engine(place)
}

fn entries(place: &Place) -> impl Iterator<Item = &Entry> {
    place
        .sections
        .iter()
        .flat_map(|s| std::iter::once(s).chain(&s.children))
}

#[test]
fn the_projection_bounds_a_hostile_place() {
    let source = hostile();
    let rules = BTreeMap::from([(
        "https://acme.com".to_owned(),
        Robots::parse("User-agent: *\nDisallow: /parent"),
    )]);
    let (projected, targets) = place(&source, Some(&rules));
    let json = serde_json::to_string(&projected).unwrap();
    for banned in [
        "http", "://", "?", "token", "hidden", "evil", "frag", "secret",
    ] {
        assert!(!json.contains(banned), "{banned} in {json}");
    }
    assert!(!json
        .chars()
        .any(|c| c.is_control() || c == '\u{202e}' || c == '\u{200b}'));
    assert_eq!(projected.sections.len(), MAX_SECTIONS);
    let tiers: Vec<_> = projected.sections.iter().map(|s| s.tier).collect();
    let mut sorted = tiers.clone();
    sorted.sort();
    assert_eq!(tiers, sorted);
    assert!(projected.title.len() <= MAX_TEXT_BYTES && projected.title.starts_with("Acme T"));
    assert_eq!(projected.site, "acme.com");
    assert_eq!(projected.host, "acme.com");
    assert_eq!(projected.focus, None);
    assert_eq!(projected.social, [Network::GitHub]);
    let mut ids = BTreeSet::new();
    for e in entries(&projected) {
        assert!(valid_id(&e.id) && ids.insert(e.id.clone()));
        assert!(e.label.len() <= MAX_TEXT_BYTES && !e.label.is_empty());
        assert!(e.group.as_ref().is_none_or(|g| g.len() <= MAX_TEXT_BYTES));
        assert!(e.path.starts_with('/') && e.path.chars().count() <= MAX_PATH_CHARS);
        assert!(e.children.len() <= MAX_CHILDREN);
        assert!(e.children.iter().all(|c| c.children.is_empty()));
    }
    let parent = projected
        .sections
        .iter()
        .find(|s| s.label == "Parent")
        .unwrap();
    assert_eq!(parent.children.len(), MAX_CHILDREN);
    assert!(parent.more && parent.children[0].more);
    assert!(!parent.open && parent.children.iter().all(|child| !child.open));
    assert!(!ids.contains(&id(999)) && !ids.contains(&id(8)));
    let label = |n: usize| {
        entries(&projected)
            .find(|e| e.id == id(n))
            .map(|e| e.label.clone())
    };
    assert_eq!(label(2).as_deref(), Some("/a"));
    assert_eq!(label(3).as_deref(), Some("/b"));
    assert!(!json.contains("Duplicate") && !json.contains("Upper") && !json.contains("Bad"));
    let long = entries(&projected).find(|e| e.id == id(1)).unwrap();
    assert!(long.open && long.host.is_none());
    let guide = entries(&projected).find(|e| e.id == id(9)).unwrap();
    assert!(guide.open);
    assert_eq!(guide.host.as_deref(), Some("docs.acme.com"));
    let oversized = entries(&projected).find(|e| e.id == id(10)).unwrap();
    assert!(!oversized.open && oversized.host.is_none());
    assert!(long.label.len() > MAX_TEXT_BYTES - 2);
    assert_eq!(long.group.as_ref().map(String::len), Some(MAX_TEXT_BYTES));
    let redirect = entries(&projected).find(|e| e.id == id(5)).unwrap();
    assert_eq!(redirect.path, "/go");
    let deep = entries(&projected).find(|e| e.id == id(6)).unwrap();
    assert_eq!(deep.path.chars().count(), MAX_PATH_CHARS);
    assert_eq!(
        targets
            .resolve(&Target::Entry { id: id(1) })
            .map(|u| u.to_string()),
        Some("https://acme.com/long?token=secret#frag".into())
    );
    assert_eq!(targets.resolve(&Target::Entry { id: id(999) }), None);

    let organic = |raw: &str, title: String, discussion: bool, rank: u32| {
        let url = Url::parse(raw).unwrap();
        results::Organic {
            key: PageKey::from_url(&url).unwrap(),
            url,
            title,
            rank,
            discussion,
            snippet: Some("secret snippet".into()),
            official: None,
            sitelinks: Vec::new(),
        }
    };
    let mut layer = FirstLayer {
        social: source.social.clone(),
        organic: vec![
            organic("https://acme.com/official", "Official".into(), false, 1),
            organic("https://github.com/acme", "Social profile".into(), false, 2),
            organic(
                &format!("https://{}/oversized", long_host()),
                "Oversized".into(),
                false,
                3,
            ),
            organic(
                "http://plain.example/hidden",
                "https://abuse.example/secret".into(),
                false,
                4,
            ),
            organic(
                "https://FORUM.example/hidden",
                format!("Line\u{7}\nHere {}", "é".repeat(MAX_TEXT_BYTES)),
                true,
                5,
            ),
        ],
        video: vec![results::Link {
            url: Url::parse("https://video.example/movie").unwrap(),
            title: "Film".into(),
        }],
        ..FirstLayer::default()
    };
    let places = [(&source.key, source.site.clone())];
    let preview = first_layer(&layer, &places);
    assert_eq!(
        preview
            .also
            .iter()
            .map(|result| result.kind)
            .collect::<Vec<_>>(),
        [AlsoKind::Page, AlsoKind::Discussion, AlsoKind::Video]
    );
    for rank in 6..6 + MAX_ALSO {
        layer.organic.push(organic(
            &format!("https://result.example/{rank}"),
            "Result".into(),
            false,
            rank as u32,
        ));
    }
    let projected = first_layer(&layer, &places);
    assert_eq!(projected.also.len(), MAX_ALSO);
    assert_eq!(projected.also[0].title, "plain.example");
    assert_eq!(projected.also[1].host, "forum.example");
    assert!(projected.also[1].title.starts_with("Line Here "));
    assert!(projected.also[1].title.len() <= MAX_TEXT_BYTES);
    assert_eq!(also_target(&projected.targets, 0), None);
    assert_eq!(
        also_target(&projected.targets, 1).unwrap().host_str(),
        Some("forum.example")
    );
    assert_eq!(also_target(&projected.targets, MAX_ALSO as u8), None);
    let json = serde_json::to_string(&projected.also).unwrap();
    for banned in [
        "http",
        "://",
        "hidden",
        "secret",
        "video.example",
        "github.com",
        "acme.com",
    ] {
        assert!(!json.contains(banned));
    }
    assert!(!json.contains("\\u0007") && !json.contains("\\n"));
}

#[test]
fn text_is_cleaned_then_clipped_at_a_character_boundary() {
    assert_eq!(text("  a\u{0}\tb\u{feff}c \n "), "a bc");
    let clipped = text(&"é".repeat(MAX_TEXT_BYTES));
    assert_eq!(clipped.len(), MAX_TEXT_BYTES);
    let odd = text(&format!("a{}", "é".repeat(MAX_TEXT_BYTES)));
    assert_eq!(odd.len(), MAX_TEXT_BYTES - 1);
    assert_eq!(text(&"a".repeat(MAX_TEXT_BYTES + 1)).len(), MAX_TEXT_BYTES);
    assert!(address_shaped("HTTPS:example") && address_shaped("x WWW.a.b"));
    assert!(!address_shaped("HTTP API") && !address_shaped("Pricing"));
}

#[test]
fn targets_open_only_known_https_addresses() {
    let mut value = base();
    value["entry"] = json!("http://acme.com/");
    let mut fragment = section(&id(1), "Pricing", "https://acme.com/pricing", "Primary");
    fragment["locator"] = json!({"element": "plans"});
    let mut route = section(&id(2), "Route", "https://acme.com/#/app", "Primary");
    route["locator"] = json!({"element": "ignored"});
    value["sections"] = json!([
        fragment,
        route,
        section(&id(3), "Plain", "http://acme.com/plain", "Primary"),
    ]);
    value["social"] = json!([
        {"basis": "Navigation", "network": "GitHub", "url": "https://github.com/acme"},
        {"basis": "Navigation", "network": "Reddit", "url": "http://reddit.com/r/acme"},
    ]);
    let (_, targets) = place(&engine(value), None);
    let open = |t: Target| targets.resolve(&t).map(|u| u.to_string());
    assert_eq!(
        open(Target::Entry { id: id(1) }),
        Some("https://acme.com/pricing#plans".into())
    );
    assert_eq!(
        open(Target::Entry { id: id(2) }),
        Some("https://acme.com/#/app".into())
    );
    assert_eq!(open(Target::Entry { id: id(3) }), None);
    assert_eq!(open(Target::Entry { id: id(4) }), None);
    assert_eq!(open(Target::Site), None);
    assert_eq!(
        open(Target::Social {
            network: Network::GitHub
        }),
        Some("https://github.com/acme".into())
    );
    for network in [Network::Reddit, Network::Discord] {
        assert_eq!(open(Target::Social { network }), None);
    }
    let (_, targets) = place(&engine(base()), None);
    assert_eq!(
        targets.resolve(&Target::Site).map(|u| u.to_string()),
        Some("https://acme.com/".into())
    );
}

#[test]
fn rows_keep_only_projected_ids_and_are_capped() {
    let (_, left) = place(&engine(base()), None);
    let known = base()["sections"][0]["id"].as_str().unwrap().to_owned();
    let entry_id = |s: &str| -> Option<site::EntryId> { serde_json::from_value(json!(s)).ok() };
    let row = |label: &str, left: &str, right: &str| compare::Row {
        role: None,
        label: label.into(),
        left: entry_id(left),
        right: entry_id(right),
    };
    let mut source = vec![
        compare::Row {
            role: Some(SectionRole::Pricing),
            ..row("Pricing", &known, &id(9))
        },
        row("Gone", &id(9), &id(9)),
        row("https://evil.example/", &known, &known),
    ];
    for _ in 0..MAX_ROWS + 5 {
        source.push(row("Row", &known, &id(9)));
    }
    let rows = rows(&source, &left, &Targets::default());
    assert_eq!(rows.len(), MAX_ROWS);
    assert_eq!(
        rows[0],
        Row {
            role: Some(Role::Pricing),
            label: "Pricing".into(),
            left: Some(known),
            right: None,
        }
    );
    assert!(rows
        .iter()
        .all(|r| r.label != "Gone" && !r.label.contains("evil")));
}

#[test]
fn every_engine_failure_has_a_wire_class() {
    use session::Failure as F;
    let all = [
        (F::Cancelled, Failure::Cancelled),
        (F::Missing, Failure::Missing),
        (F::Client, Failure::Missing),
        (F::Server, Failure::Unavailable),
        (F::Unreadable, Failure::Unreadable),
        (F::Unavailable, Failure::Unavailable),
        (F::Timeout, Failure::Timeout),
        (F::External, Failure::Blocked),
        (F::Robots, Failure::Blocked),
        (F::Challenge, Failure::Challenge),
        (F::Consent, Failure::Consent),
        (F::NoResults, Failure::NoResults),
        (F::Limit, Failure::Limit),
        (F::ContentType, Failure::Unreadable),
    ];
    for (engine, wire) in all {
        assert_eq!(Failure::from(engine), wire);
    }
    assert_eq!(
        serde_json::to_value(Change::End {
            seq: 3,
            outcome: Outcome::Failed {
                failure: Failure::NoResults
            },
            elapsed_ms: 7,
        })
        .unwrap(),
        json!({"type": "end", "seq": 3, "outcome": {"kind": "failed", "failure": "no_results"}, "elapsed_ms": 7})
    );
}

#[test]
fn action_tier_is_bounded_and_round_trips_with_its_target() {
    let mut value = base();
    value["sections"] = Value::Array(
        (0..=site::MAX_ACTIONS)
            .map(|i| {
                section(
                    &id(i),
                    "Enter",
                    &format!("https://acme.com/flow/{i}"),
                    "Action",
                )
            })
            .collect(),
    );
    let (card, targets) = place(&engine(value), None);
    assert_eq!(card.sections.len(), site::MAX_ACTIONS);
    for e in card.sections {
        assert_eq!(serde_json::to_value(e.tier).unwrap(), "action");
        assert!(targets.has(&e.id));
    }
}

#[test]
fn a_menu_only_row_projects_children_without_registering_a_page_target() {
    let url = Url::parse("https://acme.com/").unwrap();
    let doc = Doc::from_html(
        &url,
        b"<nav><div><button>Tools</button><a href='/a'>Alpha</a><a href='/b'>Beta</a></div></nav>",
        None,
        DocLimits::SITE_PAGE,
    )
    .unwrap();
    let source = site::read_site(&doc, &Default::default()).unwrap();
    let (card, targets) = place(&source, Some(&Default::default()));
    let menu = &card.sections[0];
    assert!(card.notes.thin && menu.menu_only && !menu.open && menu.path.is_empty());
    assert!(targets.has(&menu.id));
    assert!(targets
        .resolve(&Target::Entry {
            id: menu.id.clone()
        })
        .is_none());
    assert_eq!(menu.children.len(), 2);
    for child in &menu.children {
        assert!(targets
            .resolve(&Target::Entry {
                id: child.id.clone()
            })
            .is_some());
    }
}
