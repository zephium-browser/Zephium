use proptest::prelude::*;
use url::Url;
use zephium_map::{
    site::{read_site, MAX_ACTIONS, MAX_PRIMARY_LABEL_CHARS},
    Doc, DocLimits, Place, Tier,
};

fn card(markup: &str) -> Place {
    let url = Url::parse("https://workshop.example/").unwrap();
    let doc = Doc::from_html(&url, markup.as_bytes(), None, DocLimits::SITE_PAGE).unwrap();
    read_site(&doc, &Default::default()).unwrap()
}

#[test]
fn the_action_tray_observes_markup_destination_and_section_evidence() {
    for count in [MAX_ACTIONS, MAX_ACTIONS + 1] {
        let links: String = (0..count)
            .map(|i| format!("<a download href='/asset/{i}'>Abrir {i}</a>"))
            .collect();
        let p = card(&format!(
            "<nav><a href='/download'>Download</a>{links}</nav>"
        ));
        let actions: Vec<_> = p
            .sections
            .iter()
            .filter(|e| e.tier == Tier::Action)
            .collect();
        assert_eq!(actions.len(), MAX_ACTIONS);
        assert_eq!(
            actions.last().unwrap().label,
            format!("Abrir {}", MAX_ACTIONS - 1)
        );
        assert_eq!(p.sections[0].label, "Download");
        assert_eq!(p.sections[0].tier, Tier::Primary);
        assert_eq!(p.notes.truncated, count > MAX_ACTIONS);
    }
    let p=card("<nav><a href='/auth/login'>Continuer</a><a href='/guides/get-started'>Get started</a><a href='/contact'>Contact</a></nav>");
    assert_eq!(
        p.sections.iter().map(|e| e.tier).collect::<Vec<_>>(),
        [Tier::Primary, Tier::Primary, Tier::Action]
    );
}

#[test]
fn names_come_from_link_text_or_an_observed_path() {
    let p=card("<nav><a href='/a'><p>Hilfe</p><p>Information für alle, die hier eine Antwort suchen.</p></a><a href='/research-notes'>Read all the stories that our researchers have published this year</a><a href='/12345'>Read everything that the team has shared with the community</a></nav>");
    assert_eq!(
        p.sections
            .iter()
            .filter(|e| e.tier == Tier::Primary)
            .map(|e| e.label.as_str())
            .collect::<Vec<_>>(),
        ["Hilfe", "research notes"]
    );
    assert_eq!(p.sections.last().unwrap().tier, Tier::Secondary);
    for n in [MAX_PRIMARY_LABEL_CHARS, MAX_PRIMARY_LABEL_CHARS + 1] {
        let label = "界".repeat(n);
        let p = card(&format!("<nav><a href='/12345'>{label}</a></nav>"));
        assert_eq!(
            p.sections[0].tier == Tier::Primary,
            n == MAX_PRIMARY_LABEL_CHARS
        );
    }
}

#[test]
fn home_and_unnamed_menu_owners_yield_their_named_destinations() {
    let p=card("<nav><ul><li><a href='/'>Accueil</a><ul><li><a href='/agenda'>Agenda</a></li></ul></li><li><a href='/extras'>More</a><ul><li><a href='/archives'>Archives</a></li></ul></li><li><a href='/home'>Home</a></li></ul></nav>");
    assert_eq!(
        p.sections
            .iter()
            .map(|e| e.label.as_str())
            .collect::<Vec<_>>(),
        ["Agenda", "Archives", "Home"]
    );
}

#[test]
fn destination_label_and_localized_duplicates_keep_the_first_representative() {
    let base = card("<nav><a href='/de/preise'>Preise</a></nav>");
    let merged=card("<nav><a href='/de/preise'>Preise</a><a href='/de/preise/'>Tarif</a><a href='/preise'>Kosten</a><a href='/business'>Preise</a></nav>");
    assert_eq!(merged.sections.len(), 1);
    assert_eq!(merged.sections[0].id, base.sections[0].id);
    assert_eq!(
        merged.sections[0].children[0].key.as_url().path(),
        "/business"
    );
}

#[test]
fn sibling_identity_uses_display_host_and_path() {
    let p=card("<nav><ul><li><a href='/learn'>Learn</a><ul><li><a href='https://alpha.workshop.example/?mode=1'>Manual</a></li><li><a href='https://alpha.workshop.example/?mode=2'>Manual</a></li><li><a href='https://beta.workshop.example/'>Manual</a></li></ul></li></ul></nav>");
    let children = &p.sections[0].children;
    assert_eq!(children.len(), 2);
    assert_eq!(
        children[0].key.as_url().domain(),
        Some("alpha.workshop.example")
    );
    assert_eq!(
        children[1].key.as_url().domain(),
        Some("beta.workshop.example")
    );
}

proptest! {
    #[test]
    fn surviving_ids_ignore_label_noise_and_action_insertion(slugs in proptest::collection::btree_set("[a-z]{4,10}",1..10)) {
        let mut plain=String::from("<nav>");
        let mut noisy=String::from("<nav><a href='/'>Home</a><a href='/login'>Enter</a>");
        for slug in slugs {
            plain.push_str(&format!("<a href='/pages/{slug}'>{slug}</a>"));
            noisy.push_str(&format!("<a href='/pages/{slug}'><span>{slug}</span><span>A longer explanation</span></a><a href='/de/pages/{slug}'>{slug}</a>"));
        }
        let a=card(&(plain+"</nav>"));
        let b=card(&(noisy+"</nav>"));
        let ids=|p:Place|p.sections.into_iter().filter(|e|e.tier==Tier::Primary).map(|e|(e.label,e.id)).collect::<Vec<_>>();
        prop_assert_eq!(ids(a),ids(b));
    }
}

#[test]
fn language_editions_need_a_matching_language_declaration() {
    let p=card("<nav><div lang='ja'><a href='https://ja.workshop.example/'>日本語</a></div><a hreflang='de' href='https://de.workshop.example/'>Deutsch</a><a href='https://ai.workshop.example/'>Studio</a><a href='/go/'>Go</a></nav>");
    assert_eq!(
        p.sections
            .iter()
            .filter(|e| e.tier == Tier::Primary)
            .map(|e| e.label.as_str())
            .collect::<Vec<_>>(),
        ["Studio", "Go"]
    );
    let edition = p.sections.last().unwrap();
    assert_eq!(edition.label, "日本語");
    assert_eq!(edition.children[0].label, "Deutsch");
}

#[test]
fn sparse_navigation_uses_a_named_footer_section_without_promoting_ancillary_links() {
    for (nav, footer_tier) in [
        ("", Tier::Primary),
        ("<a href='/c'>Third</a>", Tier::Secondary),
    ] {
        let p=card(&format!("<nav><a href='/a'>First</a><a href='/b'>Second</a>{nav}</nav><footer><a href='/pricing'>Pricing</a><a href='/privacy'>Privacy</a></footer>"));
        assert_eq!(
            p.sections
                .iter()
                .find(|e| e.label == "Pricing")
                .unwrap()
                .tier,
            footer_tier
        );
        assert_eq!(
            p.sections
                .iter()
                .find(|e| e.label == "Privacy")
                .unwrap()
                .tier,
            Tier::Secondary
        );
    }
}

#[test]
fn repeated_locale_roots_are_not_sections_and_query_specific_routes_remain_distinct() {
    let p=card("<nav><a href='/fr/'>Français</a><a href='/de'>Deutsch</a><a href='/products?t=one'>First</a><a href='/products?t=two'>Second</a></nav>");
    assert_eq!(
        p.sections
            .iter()
            .map(|e| e.label.as_str())
            .collect::<Vec<_>>(),
        ["First", "Second"]
    );
}

#[test]
fn the_word_boundary_refuses_a_teaser_and_keeps_a_short_title() {
    let p = card("<nav><a href='/one'>A B C D E F</a><a href='/two'>A B C D E F G</a></nav>");
    assert_eq!(p.sections[0].label, "A B C D E F");
    assert_eq!(p.sections[1].label, "two");
}

#[test]
fn an_observed_directory_represents_three_or_more_deep_sibling_articles() {
    for (count, tier) in [(2, Tier::Secondary), (3, Tier::Primary)] {
        let links: String = (0..count)
            .map(|i| format!("<a href='/docs/manuals/tools/{i}'>Tool {i}</a>"))
            .collect();
        let p = card(&format!(
            "<nav>{links}</nav><footer><a href='/docs/manuals/tools'>42 tools</a></footer>"
        ));
        let directory = p
            .sections
            .iter()
            .find(|e| e.key.as_url().path().ends_with("tools"))
            .unwrap();
        assert_eq!(directory.tier, tier);
        if count == 3 {
            assert_eq!(directory.label, "tools");
        }
    }
}

#[test]
fn a_shared_clipped_path_does_not_create_indistinguishable_children() {
    let common = "x".repeat(zephium_map::site::MAX_PATH_CHARS);
    let p=card(&format!("<nav><ul><li><a href='/guide'>Guide</a><ul><li><a href='/{common}1'>Index</a></li><li><a href='/{common}2'>Index</a></li><li><a href='/{common}3'>Index C#</a></li><li><a href='/{common}4'>Index C++</a></li></ul></li></ul></nav>"));
    assert_eq!(
        p.sections[0]
            .children
            .iter()
            .map(|e| e.label.as_str())
            .collect::<Vec<_>>(),
        ["Index", "Index C#", "Index C++"]
    );
}

#[test]
fn version_badges_release_items_and_breadcrumbs_do_not_displace_sections() {
    let p = card("<nav><a href='/versions'>v3.4</a><a href='/changelog'>Changelog</a><a href='/changelog/cedar'>Cedar</a><a href='/changelog/feeds'>Feeds</a><a href='/topics#breadcrumbs'>Trail</a><a href='/topics#formats'>Formats</a></nav><footer><a href='/changelog/feeds'>Feeds</a></footer>");
    let primary: Vec<_> = p
        .sections
        .iter()
        .filter(|e| e.tier == Tier::Primary)
        .map(|e| e.label.as_str())
        .collect();
    assert_eq!(primary, ["versions", "Changelog", "Feeds", "Formats"]);
    assert!(p
        .sections
        .iter()
        .any(|e| e.label == "Cedar" && e.tier == Tier::Secondary));
}

#[test]
fn platform_download_calls_and_markdown_sales_contacts_are_actions() {
    let p = card("<nav><a href='/downloads/solaris'>Download for Solaris</a><a href='/downloads/bsd'>BSD</a><a href='/contact/sales.md'>Talk to us</a></nav>");
    let actions: Vec<_> = p
        .sections
        .iter()
        .filter(|e| e.tier == Tier::Action)
        .map(|e| e.label.as_str())
        .collect();
    assert_eq!(actions, ["Download for Solaris", "Talk to us"]);
    assert_eq!(p.sections[0].label, "BSD");
}
