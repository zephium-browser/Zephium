use url::Url;
use zephium_map::{
    site::{read_site_with_stats, PlaceStats, MAX_SITE_NAME_BYTES},
    Doc, DocLimits, Entry, Place, Tier,
};

fn read(html: &str) -> (Place, PlaceStats) {
    let base = Url::parse("https://workshop.example/").unwrap();
    let doc = Doc::from_html(&base, html.as_bytes(), None, DocLimits::SITE_PAGE).unwrap();
    read_site_with_stats(&doc, &Default::default()).unwrap()
}

fn row<'a>(p: &'a Place, label: &str) -> &'a Entry {
    p.sections
        .iter()
        .find(|e| e.label == label && e.tier == Tier::Primary)
        .unwrap()
}

fn labels(e: &Entry) -> Vec<&str> {
    e.children.iter().map(|c| c.label.as_str()).collect()
}

#[test]
fn short_topical_paths_are_independent_destinations() {
    let (p, _) =
        read("<nav><a href='/go/install'>Install Go</a><a href='/js/install'>Install JS</a></nav>");
    assert_eq!(row(&p, "Install Go").key.as_url().path(), "/go/install");
    assert_eq!(row(&p, "Install JS").key.as_url().path(), "/js/install");
    let (p, _) = read("<nav><div><button>Install</button><a lang='en' href='/go/install'>Go</a><a lang='en' href='/js/install'>JS</a></div></nav>");
    assert_eq!(labels(row(&p, "Install")), ["Go", "JS"]);
}

#[test]
fn topical_root_links_do_not_make_an_edition_picker() {
    let (p, _) = read(
        "<nav><div><button>Topics</button><a href='/ai'>AI</a><a href='/ml'>ML</a></div></nav>",
    );
    assert_eq!(labels(row(&p, "Topics")), ["AI", "ML"]);
}

#[test]
fn a_counterpart_under_the_entry_locale_confirms_only_its_own_remainder() {
    let url = Url::parse("https://workshop.example/en/").unwrap();
    let doc = Doc::from_html(
        &url,
        b"<nav><div><button>Guides</button>
        <a href='/en/install'>Setup</a><a href='/de/install'>Installation</a>
        <a href='/go/guide'>Go</a><a href='/js/guide'>JS</a></div></nav>",
        None,
        DocLimits::SITE_PAGE,
    )
    .unwrap();
    let (p, _) = read_site_with_stats(&doc, &Default::default()).unwrap();
    assert_eq!(labels(row(&p, "Guides")), ["Setup", "Go", "JS"]);
}

#[test]
fn a_menu_and_a_direct_page_can_share_a_label() {
    let (place, stats) = read(
        "<nav><a href='/catalog'>Products</a><div><button>Products</button>
        <a href='/one'>First</a><a href='/two'>Second</a></div></nav>",
    );
    let rows: Vec<_> = place
        .sections
        .iter()
        .filter(|e| e.label == "Products")
        .collect();
    assert_eq!(rows.len(), 2);
    assert!(rows
        .iter()
        .any(|e| e.menu_only && labels(e) == ["First", "Second"]));
    assert!(rows
        .iter()
        .any(|e| !e.menu_only && e.key.as_url().path() == "/catalog"));
    assert_eq!(
        (stats.missing_header_links, stats.invalid_group_pages),
        (0, 0)
    );
}

#[test]
fn disclosure_actions_leave_the_named_single_item_menu_intact() {
    let (place, _) = read("<nav><button aria-controls='items'>Resources</button>
        <div aria-hidden='true' id='items'><a href='/signin'>Sign in</a><a href='/guides'>Guides</a></div></nav>");
    assert_eq!(labels(row(&place, "Resources")), ["Guides"]);
    assert!(place
        .sections
        .iter()
        .any(|e| e.label == "Sign in" && e.tier == Tier::Action));
}

#[test]
fn marked_disclosure_panels_supply_no_destinations() {
    for mark in ["data-type='ad'", "aria-label='AI-generated overview'"] {
        let (place, _) = read(&format!("<nav><button aria-controls='items'>Resources</button>
            <div {mark} aria-hidden='true' id='items'><a href='/a'>First</a><a href='/b'>Second</a></div></nav>"));
        assert!(place.sections.is_empty(), "{mark}");
    }
}

#[test]
fn menu_members_with_the_same_label_keep_their_fragment_targets() {
    let (p, _) = read(
        "<nav><div><button>Resources</button><a href='/docs#api'>Overview</a>
        <a href='/docs#cli'>Overview</a><a href='/docs#api'>Overview</a></div></nav>",
    );
    assert_eq!(labels(row(&p, "Resources")), ["Overview", "Overview"]);
}

#[test]
fn a_mixed_menu_keeps_its_unrelated_language_tagged_guide() {
    for guide in ["/guide", "/fr#guide"] {
        let (p, _) = read(&format!(
            "<link rel='alternate' hreflang='fr' href='/fr'><nav><div><button>Resources</button>
            <a href='/fr'>French</a><a href='{guide}' hreflang='de'>Guide</a></div></nav>"
        ));
        assert!(labels(row(&p, "Resources")).contains(&"Guide"));
    }
}

#[test]
fn the_document_language_does_not_declare_a_link_target_language() {
    let (p, _) = read(
        "<html lang='en'><nav><a href='https://en.workshop.example/docs'>Docs</a></nav></html>",
    );
    assert_eq!(row(&p, "Docs").tier, Tier::Primary);
}

#[test]
fn cleaned_link_prefix_keeps_its_byte_boundary() {
    let (p, _) = read(
        "<nav><a href='/guide'><span>Docs\u{200b}</span><span> API reference</span></a></nav>",
    );
    assert_eq!(p.sections[0].label, "Docs");
}

#[test]
fn site_title_uses_only_owned_top_level_entities() {
    for (json, expected) in [
        (
            r#"{"@type":"JobPosting","hiringOrganization":{"@type":"Organization","name":"Recruiter"}}"#,
            "Workshop",
        ),
        (
            r#"{"@type":"WebSite","url":"https://other.example","name":"Other"}"#,
            "Workshop",
        ),
        (
            r#"{"@graph":[{"@type":"WebSite","url":"https://www.workshop.example","name":"Workshop tools"}]}"#,
            "Workshop tools",
        ),
    ] {
        assert_eq!(
            read(&format!(
                "<title>Home | Workshop</title><script type='application/ld+json'>{json}</script>"
            ))
            .0
            .title,
            expected
        );
    }
}

#[test]
fn a_control_that_repeats_an_item_cannot_supply_the_group_page() {
    for (href, index, expected) in [
        ("/one", "", None),
        ("/topics", "", Some("/topics")),
        (
            "/one",
            "<li><a href='/index'>View all topics</a></li>",
            Some("/index"),
        ),
    ] {
        let (p, stats) = read(&format!(
            "<nav><ul><li><a href='{href}'>Topics</a><ul>
            <li><a href='/one'>One</a></li><li><a href='/two'>Two</a></li>{index}
            </ul></li></ul></nav>"
        ));
        let group = row(&p, "Topics");
        assert_eq!(labels(group), ["One", "Two"]);
        assert_eq!(
            (!group.menu_only).then(|| group.key.as_url().path()),
            expected
        );
        assert_eq!(
            p.entry_openable(&group.id, &Default::default()),
            expected.is_some()
        );
        assert_eq!(stats.invalid_group_pages, 0);
    }
}

#[test]
fn an_observed_redirect_to_a_member_cannot_become_the_menu_page() {
    let base = Url::parse("https://workshop.example/").unwrap();
    let parse = |url: &Url, html: &str| {
        Doc::from_html(url, html.as_bytes(), None, DocLimits::SITE_PAGE).unwrap()
    };
    let home = parse(
        &base,
        "<nav><ul><li><a href='/topics'>Topics</a><ul>
        <li><a href='/one'>One</a></li><li><a href='/two'>Two</a></li></ul></li></ul></nav>",
    );
    let landing = parse(&base.join("/one").unwrap(), "<p>First topic</p>");
    let key = zephium_map::PageKey::from_url(&base.join("/topics").unwrap()).unwrap();
    let inputs = zephium_map::site::SiteInputs {
        pages: [(key, &landing)].into(),
        ..Default::default()
    };
    let (place, _) = read_site_with_stats(&home, &inputs).unwrap();
    assert!(row(&place, "Topics").menu_only);
    assert_eq!(labels(row(&place, "Topics")), ["One", "Two"]);
}

#[test]
fn footer_lending_respects_both_header_membership_and_standalone_destinations() {
    for (members, expected) in [
        ("", vec!["Studio", "Labs"]),
        (
            "<a href='/studio'>Our studio</a><a href='/events'>Events</a>",
            vec!["Our studio", "Events"],
        ),
    ] {
        let (p, stats) = read(&format!("<header><nav><div><button>Product</button>{members}</div>
          <a href='/pricing'>Pricing</a><a href='/enterprise'>Enterprise</a></nav></header>
          <footer><div><h3>Product</h3><a href='/studio'>Studio</a><a href='/labs'>Labs</a>
          <a href='/de/pricing'>Plans</a><a href='/enterprise'>For business</a>
          <a href='/rules'>Privacy and terms</a><a href='/restrictions'>Prohibited and restricted businesses</a>
          </div></footer>"));
        assert_eq!(labels(row(&p, "Product")), expected);
        for direct in ["Enterprise", "Pricing"] {
            let target = row(&p, direct).card_page();
            assert!(p
                .sections
                .iter()
                .flat_map(|e| &e.children)
                .all(|c| c.card_page() != target));
        }
        assert_eq!(stats.missing_header_links, 0);
    }
}

#[test]
fn repeated_mobile_control_cannot_lend_footer_items_to_a_populated_desktop_menu() {
    let (p, _) = read(
        "<nav><ul><li><a href='/tools'>Tools</a><ul><li><a href='/a'>First</a></li>
        <li><a href='/b'>Second</a></li></ul></li></ul><div><button>Tools</button></div></nav>
        <footer><div><h3>Tools</h3><a href='/c'>Third</a></div></footer>",
    );
    assert_eq!(labels(row(&p, "Tools")), ["First", "Second"]);
}

#[test]
fn localized_children_keep_the_observed_name_and_existing_identity_in_both_orders() {
    for (first, other) in [("/review", "/de/review"), ("/de/review", "/review")] {
        let nav = |extra: &str| {
            format!(
                "<nav><div><button>Tools</button><a href='{first}'>Review</a>
            <a href='/cli'>CLI</a>{extra}</div></nav>"
            )
        };
        let (baseline, _) = read(&nav(""));
        let aliases = format!("<a href='{other}'>Code Review</a>");
        let (p, _) = read(&format!(
            "{}<footer><div><h3>Tools</h3>{aliases}</div></footer>",
            nav(&aliases)
        ));
        let tools = row(&p, "Tools");
        assert_eq!(labels(tools), ["Review", "CLI"]);
        assert_eq!(tools.children[0].id, row(&baseline, "Tools").children[0].id);
        assert_eq!(tools.children[0].key.as_url().path(), first);
    }
}

#[test]
fn skip_links_never_create_or_own_a_row() {
    for following in ["", "<a href='/a'>First</a><a href='/b'>Second</a>"] {
        let (p, _) = read(&format!(
            "<header><div><a href='#main'>Skip to content</a>{following}</div></header>"
        ));
        assert!(p.sections.iter().all(|e| e.label != "Skip to content"));
        if following.is_empty() {
            assert!(p.sections.is_empty());
        } else {
            for name in ["First", "Second"] {
                assert!(row(&p, name).children.is_empty());
            }
        }
    }
}

#[test]
fn a_disclosed_header_link_reuses_a_previously_discovered_footer_identity() {
    for (header, footer) in [("/de/review", "/review"), ("/review", "/de/review")] {
        let column =
            format!("<footer><div><h3>Work</h3><a href='{footer}'>Code review</a></div></footer>");
        let (before, _) = read(&column);
        let id = before
            .sections
            .iter()
            .find(|e| e.label == "Code review")
            .unwrap()
            .id
            .clone();
        let (after, _) = read(&format!(
            "<nav><button aria-controls='panel'>Tools</button>
            <div id='panel' aria-hidden='true'><a href='{header}'>Review</a><a href='/cli'>CLI</a>
            <a href='/catalog'>See all tools</a></div></nav>{column}"
        ));
        let tools = row(&after, "Tools");
        assert_eq!(tools.key.as_url().path(), "/catalog");
        assert_eq!(labels(tools), ["Review", "CLI"]);
        assert_eq!(tools.children[0].id, id);
        assert_eq!(tools.children[0].key.as_url().path(), header);
    }
}

#[test]
fn separate_navigation_representations_agree_on_members_and_independent_links() {
    let (p, _) = read("<header><nav><ul><li><span>Explore</span><ul><li><a href='/a'>One</a></li>
        <li><a href='/b'>Two</a></li></ul></li><li><a href='/pricing'>Pricing</a></li></ul></nav>
        <nav><ul><li>Explore</li><li><a href='/de/a'>First</a></li><li><a href='/b'>Second</a></li></ul>
        <a href='/pricing'>Pricing</a></nav></header>");
    assert_eq!(labels(row(&p, "Explore")), ["One", "Two"]);
    assert!(row(&p, "Pricing").children.is_empty());
}

#[test]
fn language_controls_need_evidence_from_their_destinations() {
    let alternates = "<link rel='alternate' hreflang='de' href='/eins'><link rel='alternate' hreflang='fr' href='/deux'>";
    for (head, links, picker) in [
        ("", "<a hreflang='de' href='/eins'>First</a><a hreflang='fr' href='/deux'>Second</a>", false),
        ("", "<a lang='de' href='/de'>First</a><a lang='fr' href='/fr'>Second</a>", true),
        (alternates, "<a href='/eins'>First</a><a href='/deux'>Second</a>", true),
        ("", "<a hreflang='pt-BR' href='/pt-BR/'>First</a><a hreflang='fr' href='/fr/'>Second</a>", true),
        ("", "<a lang='zh-Hant-TW' href='/zh-Hant-TW/'>First</a><a lang='es-419' href='/es-419/'>Second</a>", true),
        ("", "<a href='https://de.workshop.example/'>First</a><a href='https://fr.workshop.example/'>Second</a>", true),
        ("", "<a lang='en' href='/docs/rust'>Rust</a><a lang='en' href='/docs/go'>Go</a>", false),
    ] {
        let (p, _) = read(&format!("<head>{head}</head><header><nav><div><button>Languages</button>{links}</div>
             <a href='/about'>About</a></nav></header>"));
        if picker {
            assert_eq!(p.sections.len(), 1);
            assert!(row(&p, "About").children.is_empty());
        } else {
            assert_eq!(labels(row(&p, "Languages")).len(), 2);
        }
    }
}

#[test]
fn a_standalone_page_and_a_differently_named_control_keep_separate_rows() {
    let (p, _) = read("<nav><a href='/library'>Library</a><ul><li><a href='/library'>Resources</a>
        <ul><li><a href='/news'>News</a></li><li><a href='/talks'>Talks</a></li></ul></li></ul></nav>");
    assert!(!row(&p, "Library").menu_only);
    let resources = row(&p, "Resources");
    assert!(resources.menu_only);
    assert_eq!(labels(resources), ["News", "Talks"]);
}

#[test]
fn a_home_link_and_peer_utility_buttons_do_not_own_neighbouring_destinations() {
    let (p, _) = read(
        "<header><div><a href='/'>Workshop Home</a><div>
        <a href='/pricing'>Pricing</a></div></div>
        <div><button>Languages and currency</button><a href='/help'>Help</a>
        <button>Choose a profile</button><a href='/events'>Events</a></div></header>",
    );
    assert!(p.sections.iter().all(|e| !e.menu_only));
    for label in ["Pricing", "Help", "Events"] {
        assert!(row(&p, label).children.is_empty());
    }
}

#[test]
fn oversized_link_language_cannot_hide_an_ordinary_menu() {
    let too_long = "a".repeat(zephium_map::doc::MAX_LANGUAGE_BYTES + 1);
    let (p, _) = read(&format!(
        "<nav><div><button>Topics</button><a lang='{too_long}' href='/a'>One</a>
        <a lang='fr' href='/b'>Two</a></div></nav>"
    ));
    assert_eq!(labels(row(&p, "Topics")), ["One", "Two"]);
}

#[test]
fn site_names_use_bounded_metadata_then_the_registrable_brand_then_the_title() {
    let og = "<meta property='og:site_name' content='OG name'>";
    let application = "<meta name='application-name' content='App name'>";
    let website =
        r#"<script type='application/ld+json'>{"@type":"WebSite","name":"Site name"}</script>"#;
    let organization =
        r#"<script type='application/ld+json'>{"@type":"Organization","name":"Org name"}</script>"#;
    let title = "<title>Build something new | Work Shop</title>";
    let inputs = [og, application, website, organization, title];
    for (i, expected) in ["OG name", "App name", "Site name", "Org name", "Work Shop"]
        .iter()
        .enumerate()
    {
        assert_eq!(
            read(&inputs[i..].iter().rev().copied().collect::<String>())
                .0
                .title,
            *expected
        );
    }
    assert_eq!(
        read("<title>Build something new | Catalog</title>").0.title,
        "Build something new | Catalog"
    );
    assert_eq!(read("").0.title, "workshop.example");
    for separator in [" • ", " \\ "] {
        assert_eq!(
            read(&format!(
                "<title>Welcome to our site{separator}Workshop</title>"
            ))
            .0
            .title,
            "Workshop"
        );
    }
    for (n, accepted) in [
        (MAX_SITE_NAME_BYTES, true),
        (MAX_SITE_NAME_BYTES + 1, false),
    ] {
        let value = "a".repeat(n);
        let (p, _) = read(&format!(
            "<meta property='og:site_name' content='{value}'>{application}"
        ));
        assert_eq!(p.title, if accepted { &value } else { "App name" });
    }
}
