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
        ["Hilfe", "Research notes"]
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
        for slug in &slugs {
            plain.push_str(&format!("<a href='/pages/{slug}'>{slug}</a>"));
            noisy.push_str(&format!("<a href='/pages/{slug}'><span>{slug}</span><span>A longer explanation</span></a><a href='/de/pages/{slug}'>{slug}</a>"));
        }
        let a=card(&(plain+"</nav>"));
        let b=card(&(noisy+"</nav>"));
        let ids=|p:Place|p.sections.into_iter().filter(|e|e.tier==Tier::Primary).map(|e|(e.label,e.id)).collect::<Vec<_>>();
        let original = ids(a);
        prop_assert_eq!(&original,&ids(b));
        let children = slugs.iter().map(|slug| format!("<a href='/pages/{slug}'>{slug}</a>")).collect::<String>();
        let grouped = card(&format!("<nav><div><button>Topics</button>{children}</div></nav>"));
        if original.len()>1 {
            prop_assert_eq!(grouped.sections.len(),1);
            let placed:Vec<_>=grouped.sections[0].children.iter().map(|e|(e.label.clone(),e.id.clone())).collect();
            prop_assert_eq!(original,placed);
        } else {prop_assert_eq!(original,ids(grouped));}
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
    assert_eq!(p.sections[1].label, "Two");
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
            assert_eq!(directory.label, "Tools");
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
    assert_eq!(primary, ["Versions", "Changelog", "Feeds", "Formats"]);
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

#[test]
fn a_group_uses_its_observed_index_and_flattens_nested_menus() {
    let p=card("<nav><div><button>Tools</button><a href='/catalog'>See all tools</a><a href='/one'>First</a><div><button>Editors</button><a href='/two'>Second</a><a href='/three'>Third</a></div></div></nav>");
    assert_eq!(p.sections.len(), 1);
    let tools = &p.sections[0];
    assert_eq!(tools.label, "Tools");
    assert_eq!(tools.key.as_url().path(), "/catalog");
    assert!(!tools.menu_only);
    assert!(p.entry_openable(&tools.id, &Default::default()));
    assert_eq!(
        tools
            .children
            .iter()
            .map(|c| c.label.as_str())
            .collect::<Vec<_>>(),
        ["First", "Second", "Third"]
    );
    assert!(tools.children.iter().all(|e| e.children.is_empty()));
}

#[test]
fn a_collapsed_panel_needs_its_controls_reference_and_visible_members() {
    for (id, expected) in [("panel", 1), ("other", 0)] {
        let p=card(&format!("<nav><button aria-controls='panel'>Tools</button><div aria-hidden='true' id='{id}'><a href='/x'>First</a><a href='/y'>Second</a><div hidden><a href='/hidden'>Hidden</a></div></div></nav>"));
        assert_eq!(p.sections.len(), expected);
        if let Some(group) = p.sections.first() {
            assert!(group.menu_only);
            assert_eq!(group.children.len(), 2);
        }
    }
}

#[test]
fn only_path_names_change_case_and_thin_counts_primary_rows() {
    for (extra, thin) in [("", true), ("<a href='/third'>tHIRD</a>", false)] {
        let p=card(&format!("<nav><a href='/ёж'>Here are the latest versions of this resource</a><a href='/foo'>lower case</a>{extra}<a href='/login'>Log in</a></nav>"));
        assert_eq!(p.notes.thin, thin);
        assert_eq!(
            (&*p.sections[0].label, p.sections[0].path_name),
            ("Ёж", true)
        );
        assert_eq!(
            (&*p.sections[1].label, p.sections[1].path_name),
            ("lower case", false)
        );
        if !thin {
            assert_eq!(p.sections[2].label, "tHIRD");
        }
    }
}

#[test]
fn supplemental_roles_keep_the_navigation_id_and_fill_sparse_cards_roles_first() {
    use zephium_map::site::SiteInputs;
    let url = Url::parse("https://workshop.example/").unwrap();
    for (extra, sparse) in [("", true), ("<a href='/three'>Three</a>", false)] {
        let html = format!("<nav><a href='/manual'>Docs</a><a href='/two'>Two</a>{extra}</nav>");
        let d = Doc::from_html(&url, html.as_bytes(), None, DocLimits::SITE_PAGE).unwrap();
        let base = read_site(&d, &Default::default()).unwrap();
        let p=read_site(&d,&SiteInputs {llms:Some("## Documentation\n- [One](/help/one)\n- [Two guides](/help/two)\n## Assorted\n- [Start](/pricing)\n## Careers\n- [Open roles](/work)"),..Default::default()}).unwrap();
        let docs = &p.sections[0];
        assert_eq!(docs.id, base.sections[0].id);
        assert_eq!(docs.label, "Docs");
        assert_eq!(docs.children.len(), 2);
        assert!(!p.sections.iter().any(|e| e.label == "Documentation"));
        let row = |name: &str| p.sections.iter().position(|e| e.label == name).unwrap();
        assert!(row("Careers") < row("Assorted"));
        assert_eq!(p.sections[row("Careers")].tier, Tier::Primary);
        assert_eq!(p.sections[row("Assorted")].tier == Tier::Primary, sparse);
    }
}

#[test]
fn an_index_heading_does_not_take_its_first_footer_links_role() {
    let url = Url::parse("https://workshop.example/").unwrap();
    let doc=Doc::from_html(&url,b"<nav><div><button>Resources</button><a href='/one'>One</a><a href='/two'>Two</a></div><a href='/three'>Three</a><a href='/four'>Four</a></nav><footer><a href='/help'>Support</a></footer>",None,DocLimits::SITE_PAGE).unwrap();
    let p = read_site(
        &doc,
        &zephium_map::site::SiteInputs {
            llms: Some("## Resources\n- [Help center](/help)\n- [Reports](/reports)"),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(p.sections.iter().all(|e| e.label != "Support"));
    let row = p.sections.iter().find(|e| e.label == "Resources").unwrap();
    assert!(row.menu_only);
    assert!(row
        .children
        .iter()
        .any(|e| e.key.as_url().path() == "/help"));
    assert!(row.children.iter().any(|e| e.label == "Reports"));
}

#[test]
fn a_collapsed_menu_obeys_the_documents_text_budget() {
    let url = Url::parse("https://workshop.example/").unwrap();
    let html="<nav><button aria-controls='menu'>Tools</button><div id='menu' aria-hidden='true'><a href='/first'>First</a><a href='/second'>Second</a></div></nav>";
    let room = ["/first", "/second"]
        .iter()
        .map(|path| url.join(path).unwrap().as_str().len())
        .sum::<usize>()
        + "ToolsFirstSecond".len();
    for (limit, truncated) in [(room, false), (room - 1, true)] {
        let d = Doc::from_html(
            &url,
            html.as_bytes(),
            None,
            DocLimits {
                text_bytes: limit as u32,
                ..DocLimits::SITE_PAGE
            },
        )
        .unwrap();
        assert_eq!(d.body_truncated(), truncated);
    }
}
