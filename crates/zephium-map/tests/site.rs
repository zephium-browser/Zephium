use url::Url;
use zephium_map::{results::Link, robots::*, site::*, *};
const BASE: &str = "https://workshop.example/";
fn url(path: &str) -> Url {
    Url::parse(BASE).unwrap().join(path).unwrap()
}
fn doc(path: &str, html: &str) -> Doc {
    Doc::from_html(&url(path), html.as_bytes(), None, DocLimits::SITE_PAGE).unwrap()
}
fn key(path: &str) -> PageKey {
    PageKey::from_url(&url(path)).unwrap()
}
fn labels(children: &[Entry]) -> Vec<&str> {
    children.iter().map(|e| e.label.as_str()).collect()
}
#[test]
fn ordered_site_with_all_evidence() {
    use SectionRole::*;
    use Source::*;
    let home = doc("/", include_str!("fixtures/site-menu.html"));
    let docs = doc(
        "https://docs.workshop.example/guide",
        include_str!("fixtures/section-docs.html"),
    );
    let pricing = doc("/pricing", include_str!("fixtures/section-pricing.html"));
    let changes = doc("/changes", include_str!("fixtures/section-changes.html"));
    let links = [Link {
        url: url("/pricing"),
        title: "Plans and costs".into(),
    }];
    let mut inputs = SiteInputs {
        sitelinks: &links,
        llms: Some(include_str!("fixtures/llms.txt")),
        ..Default::default()
    };
    for d in [&changes, &docs, &pricing] {
        inputs.pages.insert(PageKey::from_url(d.page()).unwrap(), d);
    }
    let place = read_site(&home, &inputs).unwrap();
    let expected = [
        ("Product", Some(Product), PrimaryNav, false),
        ("Documentation", Some(Documentation), PrimaryNav, false),
        ("Pricing", Some(Pricing), PrimaryNav, false),
        ("Changelog", Some(Changelog), PrimaryNav, false),
        ("Status", Some(Status), PrimaryNav, true),
        ("Courses", None, Llms, false),
        ("Careers", Some(Careers), Footer, false),
        ("Security", Some(Security), Footer, false),
        ("Examples", Some(Examples), LinkedData, false),
        ("Home", Some(Home), LinkedData, false),
        ("Privacy", Some(Legal), Footer, false),
    ];
    assert_eq!(
        place
            .sections
            .iter()
            .map(|e| (e.label.as_str(), e.role, e.sources[0], e.off_site))
            .collect::<Vec<_>>(),
        expected
    );
    let product = &place.sections[0];
    assert_eq!(labels(&product.children), ["Editor", "Reader"]);
    assert!(product
        .children
        .iter()
        .all(|e| e.sources[0] == MenuGroup && !e.off_site));
    let guide = &place.sections[1];
    assert_eq!(
        labels(&guide.children),
        ["Setup", "Types", "Overview", "Syntax"]
    );
    assert_eq!(
        guide
            .children
            .iter()
            .map(|e| (e.sources[0], e.group.as_deref(), e.off_site))
            .collect::<Vec<_>>(),
        [
            (SectionSidebar, Some("Getting started"), false),
            (SectionSidebar, Some("Reference"), false),
            (Heading, None, false),
            (Llms, None, false)
        ]
    );
    let prices = &place.sections[2];
    assert!(prices.sources.contains(&EngineSitelink));
    assert_eq!(labels(&prices.children), ["Plan tiers", "Questions"]);
    assert_eq!(
        prices.children[0].locator.fragment().as_deref(),
        Some("#tiers")
    );
    assert!(prices
        .children
        .iter()
        .all(|c| c.key == prices.key && c.sources[0] == Heading && !c.off_site));
    assert_eq!(
        labels(&place.sections[3].children),
        ["v3.2.1", "2026-06-10"]
    );
    assert!(matches!(
        place.sections[3].children[1].locator,
        Locator { quote: Some(_), .. }
    ));
    assert_eq!(labels(&place.sections[5].children), ["Field guide"]);
    assert_eq!(place.social.len(), 1);
    assert!(place.social[0].basis == results::ProfileBasis::SameAs);
    assert!(
        place.sections[3].children[0].locator.quote.is_some()
            && place.sections[3].children[0].locator.element.is_some()
    );
    assert_eq!(place.social[0].url.host_str(), Some("github.com"));

    inputs.pages.clear();
    for d in [&pricing, &docs, &changes] {
        inputs.pages.insert(PageKey::from_url(d.page()).unwrap(), d);
    }
    assert!(place == read_site(&home, &inputs).unwrap());
}

#[test]
fn shells_and_challenges_use_independent_evidence() {
    let links = [Link {
        url: url("/about"),
        title: "About".into(),
    }];
    for (html, shell, wall) in [
        (include_str!("fixtures/shell.html"), true, false),
        (include_str!("fixtures/wall.html"), false, true),
    ] {
        let p = read_site(
            &doc("/", html),
            &SiteInputs {
                sitelinks: &links,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!((p.notes.shell, p.notes.wall), (shell, wall));
        assert_eq!(labels(&p.sections), ["About"]);
    }
}
#[test]
fn wide_navigation_and_menu_children_stay_bounded() {
    let links = (0..600)
        .map(|i| include_str!("fixtures/wide-nav-link.html").replace('N', &i.to_string()))
        .collect::<String>();
    let (p, stats) = read_site_with_stats(
        &doc("/", &format!("<nav><ul>{links}</ul></nav>")),
        &Default::default(),
    )
    .unwrap();
    assert_eq!(stats.links_seen, 600);
    assert_eq!(
        labels(&p.sections),
        (0..MAX_PRIMARY_SECTIONS)
            .map(|i| format!("Section {i}"))
            .collect::<Vec<_>>()
    );
    assert!(p.notes.truncated);
    let p = read_site(
        &doc(
            "/",
            &format!("<nav><ul><li><a href='/docs'>Docs</a><ul>{links}</ul></li></ul></nav>"),
        ),
        &Default::default(),
    )
    .unwrap();
    assert_eq!(p.sections.len(), 1);
    assert_eq!(p.sections[0].children.len(), MAX_CHILDREN);
    assert_eq!(p.sections[0].more, 600 - MAX_CHILDREN as u32);
}
#[test]
fn sitemap_sampling_and_index_bounds() {
    let source = url("/index.xml");
    let index = Sitemap::parse(include_bytes!("fixtures/sitemap-index.xml"), &source).unwrap();
    assert_eq!(index.index, [url("/one.xml"), url("/two.xml")]);
    let mut xml = String::from("<urlset>");
    for n in 0..MAX_SITEMAP_URLS / 5 {
        for prefix in ["downloads", "docs", "examples", "news", "products"] {
            xml.push_str(
                &include_str!("fixtures/sitemap-record.xml")
                    .replace("PREFIX", prefix)
                    .replace('N', &n.to_string()),
            );
        }
    }
    let extra = format!("{xml}<url><loc>https://workshop.example/extra</loc></url></urlset>");
    assert!(matches!(
        Sitemap::parse(extra.as_bytes(), &source),
        Err(XmlError::Limit)
    ));
    xml.push_str("</urlset>");
    let map = Sitemap::parse(xml.as_bytes(), &source).unwrap();
    let home = doc("/", "<title>Workshop</title>");
    let (p, stats) = read_site_with_stats(
        &home,
        &SiteInputs {
            sitemaps: [(key("/index.xml"), &map)].into(),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        labels(&p.sections),
        ["downloads", "docs", "examples", "news", "products"]
    );
    assert_eq!(stats.sitemap_urls, 50_000);
    let count = p.sections.len();
    assert_eq!(
        p.sections.iter().map(|e| e.children.len()).sum::<usize>(),
        MAX_PLACE_CHILDREN
    );
    for (index, e) in p.sections.into_iter().enumerate() {
        let expected = MAX_PLACE_CHILDREN / count + usize::from(index < MAX_PLACE_CHILDREN % count);
        assert_eq!(e.more, 10_000 - 1 - expected as u32);
        assert_eq!(e.children.len(), expected);
        assert_eq!(e.sources[0], Source::Sitemap);
        assert_eq!(e.key.as_url().path(), format!("/{}/0", e.label));
        assert_eq!(
            labels(&e.children),
            (1..=expected).map(|n| n.to_string()).collect::<Vec<_>>()
        );
    }
}
#[test]
fn hostile_inputs_fail_closed_or_keep_bounded_independent_evidence() {
    let source = url("/index.xml");
    assert!(matches!(
        Sitemap::parse(&vec![b' '; 50 * 1024 * 1024], &source),
        Err(XmlError::Limit)
    ));
    assert!(matches!(
        Sitemap::parse(
            b"<!DOCTYPE urlset [<!ENTITY a 'aaaa'><!ENTITY b '&a;&a;'>]><urlset/>",
            &source
        ),
        Err(XmlError::Malformed)
    ));
    let recursive=b"<sitemapindex><sitemap><loc>https://workshop.example/index.xml</loc></sitemap></sitemapindex>";
    assert!(Sitemap::parse(recursive, &source).unwrap().index.is_empty());
    let xml = format!(
        "<urlset>{}{}</urlset>",
        "<n>".repeat(MAX_XML_DEPTH),
        "</n>".repeat(MAX_XML_DEPTH)
    );
    assert!(matches!(
        Sitemap::parse(xml.as_bytes(), &source),
        Err(XmlError::Limit)
    ));
    let home=doc("/",&format!("<script type='application/ld+json'>{}0{}</script><nav><a href='/about'>About</a></nav>","[".repeat(10_000),"]".repeat(10_000)));
    let llms = "- [Repeated](/same)\n".repeat(100_000);
    let (p, stats) = read_site_with_stats(
        &home,
        &SiteInputs {
            llms: Some(&llms),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(stats.llms_truncated && stats.linked_data_truncated);
    assert_eq!(labels(&p.sections), ["About"]);
    assert_eq!(
        Robots::parse(&"\n".repeat(1024 * 1024)).outcome,
        RobotsOutcome::Unreadable
    );
}
#[test]
fn robots_specific_groups_wildcards_end_anchor_and_ties() {
    let policy = Robots::parse(include_str!("fixtures/robots.txt"));
    assert_eq!(policy.sitemaps, [url("/index.xml")]);
    for (path, allowed) in [
        ("/", true),
        ("/draft", false),
        ("/draft/1", false),
        ("/draft/public/a", true),
        ("/draft/same", true),
        ("/more", false),
        ("/manual.pdf", false),
        ("/manual.pdf?q=1", true),
        ("/admin/a/b/edit", false),
        ("/admin/a/edit/", true),
    ] {
        assert_eq!(policy.allows(path), allowed, "{path}");
    }
    let wild =
        Robots::parse("User-agent: *\nDisallow: /private\nUser-agent: Other\nAllow: /private");
    assert!(!wild.allows("/private"));
    assert!(Robots::unavailable(true).allows("/private"));
    assert!(!Robots::unavailable(false).allows("/public"));
    for n in [MAX_RULES, MAX_RULES + 1] {
        let p = Robots::parse(&format!("User-agent: *\n{}", "Disallow: /p\n".repeat(n)));
        assert_eq!(p.allows("/ok"), n == MAX_RULES);
    }
    for n in [MAX_PATTERN_BYTES, MAX_PATTERN_BYTES + 1] {
        let p = Robots::parse(&format!("User-agent: *\nDisallow: /{}", "x".repeat(n - 1)));
        assert_eq!(p.allows("/ok"), n == MAX_PATTERN_BYTES);
    }
}
#[test]
fn compare_reserves_role_pairs_before_labels_and_keeps_unpaired() {
    let make = |links: &str| {
        read_site(
            &doc("/", &format!("<nav>{links}</nav>")),
            &Default::default(),
        )
        .unwrap()
    };
    let left=make("<a href='/docs'>Manual</a><a href='/pricing'>Plans</a><a href='/lab'>Field-Lab</a><a href='/tools'>Tools</a>");
    let right=make("<a href='/help-center'>Manual</a><a href='/reference/docs'>Guide</a><a href='/lab'>Field Lab</a><a href='/pricing'>Billing</a>");
    let rows = compare::compare(&left, &right, Some(SectionRole::Pricing));
    assert_eq!(
        rows.iter()
            .map(|r| (
                left.sections
                    .iter()
                    .find(|e| Some(&e.id) == r.left.as_ref())
                    .map(|e| e.label.as_str()),
                right
                    .sections
                    .iter()
                    .find(|e| Some(&e.id) == r.right.as_ref())
                    .map(|e| e.label.as_str())
            ))
            .collect::<Vec<_>>(),
        [
            (Some("Plans"), Some("Billing")),
            (Some("Manual"), Some("Guide")),
            (Some("Field-Lab"), Some("Field Lab")),
            (Some("Tools"), None),
            (None, Some("Manual"))
        ]
    );
}

#[test]
fn labelled_menus_and_deep_landings_use_observed_destinations() {
    let home=doc("/","<header><nav><div><button>Products</button><ul><li><a href='/editor'>Editor</a></li><li><a href='/reader'>Reader</a></li></ul></div><div><a href='/a'>Alpha</a><a href='/b'>Beta</a></div></nav></header>");
    let place = read_site(&home, &Default::default()).unwrap();
    assert_eq!(labels(&place.sections), ["Products", "Alpha", "Beta"]);
    assert_eq!(place.sections[0].key.as_url().path(), "/editor");
    assert_eq!(labels(&place.sections[0].children), ["Reader"]);
    let deep = doc("/pricing", include_str!("fixtures/section-pricing.html"));
    let p = read_site(&deep, &Default::default()).unwrap();
    assert_eq!(p.sections[0].key.as_url(), deep.page());
    assert_eq!(labels(&p.sections[0].children), ["Plan tiers", "Questions"]);
    assert_eq!(p.entry.as_url().path(), "/");
}
#[test]
fn utility_entries_cannot_consume_every_slot() {
    let noisy = (0..MAX_SECTIONS + 2)
        .map(|i| format!("<a href='/privacy/{i}'>Privacy {i}</a>"))
        .collect::<String>();
    let home = doc(
        "/",
        &format!(
            "<footer><a href='/docs'>Manual</a></footer><nav>{noisy}<a href='/docs'>Docs</a></nav>"
        ),
    );
    let p = read_site(&home, &Default::default()).unwrap();
    assert_eq!(p.sections.len(), 1 + MAX_SECONDARY_SECTIONS);
    assert_eq!(p.sections[0].label, "Docs");
    assert_eq!(p.sections[0].sources, [Source::PrimaryNav, Source::Footer]);
    assert!(p.notes.truncated);
}
#[test]
fn separate_sitemaps_merge_one_cluster_regardless_of_arrival() {
    let a = Sitemap::parse(
        b"<urlset><url><loc>https://workshop.example/docs/print</loc></url></urlset>",
        &url("/a.xml"),
    )
    .unwrap();
    let b = Sitemap::parse(
        b"<urlset><url><loc>https://workshop.example/docs/read</loc></url></urlset>",
        &url("/b.xml"),
    )
    .unwrap();
    let run = |order| {
        read_site(
            &doc("/", "<title>Workshop</title>"),
            &SiteInputs {
                sitemaps: order,
                ..Default::default()
            },
        )
        .unwrap()
    };
    let p = run([(key("/b.xml"), &b), (key("/a.xml"), &a)].into());
    assert!(p == run([(key("/a.xml"), &a), (key("/b.xml"), &b)].into()));
    assert_eq!(labels(&p.sections), ["docs"]);
    assert_eq!(p.sections[0].key.as_url().path(), "/docs/read");
    assert_eq!(labels(&p.sections[0].children), ["print"]);
}

#[test]
fn metadata_and_section_page_boundaries() {
    let home = doc("/", "<title>Workshop</title>");
    let llms = |n| format!("#{}", "x".repeat(n - 1));
    for n in [MAX_LLMS_BYTES, MAX_LLMS_BYTES + 1] {
        let (_, stats) = read_site_with_stats(
            &home,
            &SiteInputs {
                llms: Some(&llms(n)),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(stats.llms_truncated, n > MAX_LLMS_BYTES);
    }
    let header = "User-agent: *\nDisallow: /private\n#";
    let mut prefix = format!(
        "{header}{}",
        "x".repeat(MAX_ROBOTS_BYTES - header.len() - 13)
    );
    prefix.push_str("\nDisallow: /x");
    assert_eq!(prefix.len(), MAX_ROBOTS_BYTES);
    assert!(!Robots::parse(&prefix).allows("/x"));
    prefix.push('y');
    let parsed = Robots::parse(&prefix);
    assert!(!parsed.allows("/private"));
    assert!(parsed.allows("/x"));
    for n in [MAX_SITEMAPS, MAX_SITEMAPS + 1] {
        let lines = (0..n)
            .map(|i| format!("Sitemap: https://workshop.example/{i}.xml\n"))
            .collect::<String>();
        assert_eq!(Robots::parse(&lines).sitemaps.len(), MAX_SITEMAPS);
        let records = (0..n)
            .map(|i| format!("<sitemap><loc>https://workshop.example/{i}.xml</loc></sitemap>"))
            .collect::<String>();
        let map = Sitemap::parse(
            format!("<sitemapindex>{records}</sitemapindex>").as_bytes(),
            &url("/map.xml"),
        )
        .unwrap();
        assert_eq!(map.index.len(), MAX_SITEMAPS);
        assert_eq!(map.truncated, n > MAX_SITEMAPS);
    }
    let docs = (0..MAX_SECTION_PAGES + 1)
        .map(|i| doc(&format!("/page/{i}"), "<h2 id='a'>A</h2>"))
        .collect::<Vec<_>>();
    let links = docs
        .iter()
        .map(|d| Link {
            url: d.page().clone(),
            title: "Guide".into(),
        })
        .collect::<Vec<_>>();
    let (p, stats) = read_site_with_stats(
        &home,
        &SiteInputs {
            sitelinks: &links,
            pages: docs
                .iter()
                .map(|d| (PageKey::from_url(d.page()).unwrap(), d))
                .collect(),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(stats.section_pages, MAX_SECTION_PAGES as u32);
    assert!(stats.section_page_truncated);
    assert_eq!(
        p.sections.iter().filter(|e| !e.children.is_empty()).count(),
        MAX_SECTION_PAGES
    );
    for n in [MAX_XML_BYTES, MAX_XML_BYTES + 1] {
        let xml = format!("<urlset/>{}", " ".repeat(n - 9));
        assert_eq!(
            Sitemap::parse(xml.as_bytes(), &url("/map.xml")).is_ok(),
            n == MAX_XML_BYTES
        );
    }
    let p = Robots::parse(
        "User-agent: Zephium\nDisallow: /draft\nDisallow: /café\nDisallow: /%2fprivate",
    );
    for path in ["/%64raft", "/caf%C3%A9", "/%2Fprivate"] {
        assert!(!p.allows(path));
    }
    assert!(!Robots::parse("<html>try again</html>").allows("/"));
}

#[test]
fn routed_pages_and_element_locators_keep_their_separate_destinations() {
    let page=read_site(&doc("/","<nav><a href='/app#/inbox'>Inbox</a><a href='/app#!/draft'>Draft</a><a href='/app#standard-pricing.v2_é'>Prices</a></nav>"),&Default::default()).unwrap();
    assert_eq!(page.sections.len(), 3);
    for (e, route) in page.sections.iter().zip(["/inbox", "!/draft"]) {
        assert_eq!(e.key.as_url().fragment(), Some(route));
        assert!(e.locator.fragment().is_none());
    }
    let e = &page.sections[2];
    assert!(e.key.as_url().fragment().is_none());
    assert_eq!(
        e.locator.fragment().as_deref(),
        Some("#standard-pricing.v2_%C3%A9")
    );
}

#[test]
fn wall_evidence_depends_on_the_whole_document() {
    for (text, wall) in [
        ("CAPTCHA is a test for automated clients.", false),
        ("Please verify you are human to continue.", true),
    ] {
        let page = doc("/", &format!("<main><p>{text}</p></main>"));
        assert_eq!(
            read_site(&page, &Default::default()).unwrap().notes.wall,
            wall
        );
    }
    for (len, wall) in [
        (MAX_WALL_TEXT_BYTES, true),
        (MAX_WALL_TEXT_BYTES + 1, false),
    ] {
        let page = doc(
            "/",
            &format!(
                "<title>Checking your browser</title><p>{}</p>",
                "x".repeat(len)
            ),
        );
        assert_eq!(
            read_site(&page, &Default::default()).unwrap().notes.wall,
            wall
        );
    }
    let page = doc("/","<main><p>HTTP status 472 is reserved.</p><p>Other errors may require a captcha.</p></main>");
    assert!(!read_site(&page, &Default::default()).unwrap().notes.wall);
}

#[test]
fn navigation_repeated_in_a_section_does_not_become_a_sidebar() {
    let home = doc(
        "/",
        "<nav><a href='/docs'>Docs</a><a href='/plans'>Plans</a><a href='/news'>News</a></nav>",
    );
    let page = doc("/docs","<nav><a href='/docs'>Docs</a><a href='/'>Home</a><a href='/plans/'>Plans</a><a href='/news'>News</a><a href='/junk'>Global extra</a></nav><aside><a href='/docs'>Docs</a><a href='/edit'>Edit</a><a href='/save'>Save</a></aside>");
    let p = read_site(
        &home,
        &SiteInputs {
            pages: [(key("/docs"), &page)].into(),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(labels(&p.sections[0].children), ["Edit", "Save"]);
}

#[test]
fn landmark_free_link_rows_require_short_labels_and_little_intervening_text() {
    let links = |n| {
        (0..n)
            .map(|i| format!("<td><a href='/p{i}'>Page {i}</a></td>"))
            .collect::<String>()
    };
    for (n, count) in [(3, 0), (4, 4)] {
        let page = doc(
            "/",
            &format!("<table><tr>{}</tr></table><p>Article text.</p>", links(n)),
        );
        let place = read_site(&page, &Default::default()).unwrap();
        assert_eq!(place.sections.len(), count);
        assert!(place
            .sections
            .iter()
            .all(|e| e.tier == Tier::Primary && e.sources == [Source::LinkCluster]));
    }
    for size in [48, 49] {
        let page = doc("/",&format!("<div><a href='/a'>A</a><a href='/b'>B</a><a href='/c'>C</a><a href='/d'>{}</a></div><p>Body.</p>", "x".repeat(size)));
        assert_eq!(
            read_site(&page, &Default::default())
                .unwrap()
                .sections
                .len(),
            if size == 48 { 4 } else { 0 }
        );
    }
    let page = doc("/", "<p>Read <a href='/a'>A</a>, <a href='/b'>B</a>, <a href='/c'>C</a> and <a href='/d'>D</a> together.</p>");
    assert!(read_site(&page, &Default::default())
        .unwrap()
        .sections
        .is_empty());
    let page = doc("/",&format!("<table><tr>{}</tr></table><p>Body text.</p><div><a href='/a'>A</a><a href='/b'>B</a><a href='/c'>C</a><a href='/d'>D</a></div>",links(4)));
    let p = read_site(
        &page,
        &SiteInputs {
            llms: Some("## Manual\n- [Read](/manual)"),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        p.sections
            .iter()
            .filter(|e| e.tier == Tier::Secondary)
            .count(),
        8
    );
}

#[test]
fn networks_own_navigation_is_not_an_external_profile() {
    for root in ["https://github.com/", "https://instagram.com/"] {
        let home = Doc::from_html(
            &Url::parse(root).unwrap(),
            b"<header><nav><a href='/pricing'>Pricing</a></nav></header>",
            None,
            DocLimits::SITE_PAGE,
        )
        .unwrap();
        let p = read_site(&home, &SiteInputs::default()).unwrap();
        assert_eq!(labels(&p.sections), ["Pricing"]);
        assert!(p.social.is_empty());
    }
}

#[test]
fn capped_tiers_preserve_site_order_and_align_late_roles() {
    let menu = |tag, cap, last: &str, label: &str| {
        let items: String = (0..cap)
            .map(|i| format!("<a href='/item-{i}'>Item {i}</a>"))
            .collect();
        let page = doc(
            &format!("https://{tag}.example/"),
            &format!("<header><h1>Catalogue</h1></header><{tag}>{items}<a href='/{last}'>{label}</a></{tag}>"),
        );
        read_site_with_stats(&page, &SiteInputs::default()).unwrap()
    };

    let mut places = Vec::new();
    for (tag, cap, tier, label) in [
        ("nav", MAX_PRIMARY_SECTIONS, Tier::Primary, "Plans"),
        ("footer", MAX_SECONDARY_SECTIONS, Tier::Secondary, "Rates"),
    ] {
        for (last, name) in [("pricing", label), ("extra", "Extra")] {
            let (place, stats) = menu(tag, cap, last, name);
            assert!(stats.sections_truncated);
            assert!(place.sections.iter().all(|entry| entry.tier == tier));
            let mut expected: Vec<_> = (0..cap).map(|i| format!("Item {i}")).collect();
            if last == "pricing" {
                expected[cap - 1] = label.into();
            }
            assert_eq!(labels(&place.sections), expected);
            if last == "pricing" {
                places.push(place);
            }
        }
    }
    let rows = compare::compare(&places[0], &places[1], None);
    let pricing = rows
        .iter()
        .find(|row| row.role == Some(SectionRole::Pricing))
        .unwrap();
    assert_eq!(
        pricing.left.as_ref(),
        Some(&places[0].sections.last().unwrap().id)
    );
    assert_eq!(
        pricing.right.as_ref(),
        Some(&places[1].sections.last().unwrap().id)
    );
}

#[test]
fn surplus_role_entries_keep_the_first_in_site_order() {
    let links: String = (0..=MAX_PRIMARY_SECTIONS)
        .map(|i| format!("<a href='/p{i}'>Features {i}</a>"))
        .collect();
    let page = doc("/", &format!("<nav><a href='/item'>Item</a>{links}</nav>"));
    let place = read_site(&page, &SiteInputs::default()).unwrap();
    let expected: Vec<_> = (0..MAX_PRIMARY_SECTIONS)
        .map(|i| format!("Features {i}"))
        .collect();
    assert_eq!(labels(&place.sections), expected);
}

#[test]
fn footer_navigation_does_not_hide_primary_link_clusters() {
    let home = doc("/", "<main><ul><li><a href='/docs'>Docs</a></li><li><a href='/pricing'>Pricing</a></li><li><a href='/download'>Download</a></li><li><a href='/about'>About</a></li></ul></main><footer><nav><a href='/privacy'>Privacy</a></nav></footer>");
    let place = read_site(&home, &SiteInputs::default()).unwrap();
    assert!(place.sections.iter().any(|e| e.label == "Docs"
        && e.sources.contains(&Source::LinkCluster)
        && e.tier == Tier::Primary));
    assert!(place
        .sections
        .iter()
        .any(|e| e.label == "Privacy" && e.sources.contains(&Source::Footer)));
}

#[test]
fn nested_menu_links_keep_distinct_anchors_on_the_parent_page() {
    let home = doc("/", "<nav><ul><li><a href='/pricing'>Pricing</a><ul><li><a href='/pricing#students'>Students</a></li><li><a href='/pricing'>Same page</a></li></ul></li></ul></nav>");
    let place = read_site(&home, &SiteInputs::default()).unwrap();
    assert_eq!(labels(&place.sections[0].children), ["Students"]);
    assert_eq!(
        place.sections[0].children[0].locator.fragment().as_deref(),
        Some("#students")
    );
}

#[test]
fn delegated_sidebar_links_keep_their_real_destination_and_require_a_label() {
    let home = doc(
        "/",
        "<nav><a href='https://guides.example/docs'>Documentation</a></nav>",
    );
    let docs = doc(
        "https://guides.example/docs",
        "<aside><a href='/setup#linux'>Linux setup</a><a href='/empty'></a></aside>",
    );
    let place = read_site(
        &home,
        &SiteInputs {
            pages: [(PageKey::from_url(docs.page()).unwrap(), &docs)].into(),
            ..SiteInputs::default()
        },
    )
    .unwrap();
    let section = &place.sections[0];
    let child = section
        .children
        .iter()
        .find(|e| e.label == "Linux setup")
        .unwrap();
    assert!(child.off_site);
    assert_eq!(child.key.as_url().path(), "/setup");
    assert_eq!(child.locator.fragment().as_deref(), Some("#linux"));
    assert!(section.children.iter().all(|e| !e.label.is_empty()));
}

#[test]
fn section_landing_updates_the_key_and_locator_together() {
    let home = doc("/", "<nav><a href='/docs#install'>Documentation</a></nav>");
    for (landing, expected) in [
        ("/docs", Some("#install")),
        ("/guide#setup", Some("#setup")),
        ("/guide", None),
    ] {
        let docs = doc(landing, "<p>Guide</p>");
        let place = read_site(
            &home,
            &SiteInputs {
                pages: [(key("/docs"), &docs)].into(),
                ..SiteInputs::default()
            },
        )
        .unwrap();
        let section = &place.sections[0];
        assert_eq!(section.key.as_url().path(), docs.page().path());
        assert_eq!(section.locator.fragment().as_deref(), expected);
    }
}

#[test]
fn site_profiles_ignore_known_service_routes() {
    let doc = doc("/", "<nav><a href='https://github.com/pricing'>Pricing</a><a href='https://github.com/login'>Login</a><a href='https://x.com/search'>Search</a><a href='https://github.com/acme'>Acme</a></nav>");
    let place = read_site(&doc, &SiteInputs::default()).unwrap();
    assert_eq!(
        place
            .social
            .iter()
            .map(|profile| profile.url.path())
            .collect::<Vec<_>>(),
        ["/acme"]
    );
}

#[test]
fn sitemap_rejects_character_data_outside_the_root() {
    let source = url("/sitemap.xml");
    for text in [
        "<![CDATA[extra]]><urlset/>",
        "<urlset/><![CDATA[extra]]>",
        "<urlset/>&#32;",
    ] {
        assert!(matches!(
            Sitemap::parse(text.as_bytes(), &source),
            Err(XmlError::Malformed)
        ));
    }
    assert!(Sitemap::parse(
        b"<urlset><url><loc><![CDATA[https://workshop.example/docs]]></loc></url></urlset>",
        &source
    )
    .is_ok());
}
