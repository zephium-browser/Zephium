use url::Url;
use zephium_core::search::SearchEngine;
use zephium_map::{
    doc::{Doc, DocLimits},
    query::read_query,
    results::*,
};

fn read(query: &str, html: &str) -> ResultsRead {
    let url = Url::parse("https://search.example/results").unwrap();
    let doc = Doc::from_html(&url, html.as_bytes(), None, DocLimits::default()).unwrap();
    read_results(
        SearchEngine::Custom,
        None,
        &read_query(query).unwrap(),
        &doc,
    )
}
fn layer(query: &str, html: &str) -> FirstLayer {
    match read(query, html) {
        ResultsRead::FirstLayer { layer: l } => l,
        ResultsRead::Wall { wall: w } => panic!("wall {w:?}"),
    }
}
#[test]
fn extracts_first_layer() {
    let l = layer("acme pricing", include_str!("fixtures/first-layer.html"));
    assert_eq!(l.organic.len(), 4);
    let official = l.organic[1].official.as_ref().unwrap();
    assert!(l.organic[0].official.is_none());
    assert_eq!(l.organic[1].rank, 2);
    assert!(
        official.evidence.domain_match
            && official.evidence.panel_website
            && official.evidence.sitelinks
    );
    let hit = &l.organic[1];
    assert_eq!(
        hit.snippet.as_deref(),
        Some("Visit Acme for its products, plans and documentation.")
    );
    assert_eq!(
        hit.sitelinks
            .iter()
            .map(|l| l.title.as_str())
            .collect::<Vec<_>>(),
        ["Pricing", "Documentation"]
    );
    assert_eq!(l.video[0].title, "Acme training");
    assert_eq!(
        l.social.iter().map(|p| p.network).collect::<Vec<_>>(),
        [Network::GitHub, Network::Twitter]
    );
    assert_eq!(l.images.len(), 1);
    assert_eq!(l.images[0].destination.as_str(), "https://acme.com/gallery");
    assert_eq!(
        l.images[0].source.as_str(),
        "https://cdn.acme.com/product.jpg"
    );
    assert_eq!(l.images[0].title, "Acme product");
}
#[test]
fn nested_heading_sitelinks_do_not_consume_later_cards() {
    let l = layer("acme", include_str!("fixtures/heading-sitelinks.html"));
    assert_eq!(l.organic.len(), 2);
    assert_eq!(l.organic[0].sitelinks.len(), 2);
    assert_eq!(
        l.organic[0].snippet.as_deref(),
        Some("Find specifications and support for Acme tools here.")
    );
    assert_eq!(l.organic[1].title, "Acme news");
}
fn card(target: &str, title: &str) -> String {
    format!("<article><h2><a href='https://{target}'>{title}</a></h2></article>")
}
#[test]
fn official_site_corpus() {
    let cases = [
        (
            "stripe pricing",
            "en.wikipedia.org stripe.com",
            "stripe.com",
        ),
        (
            "github actions docs",
            "docs.github.com microsoft.com",
            "github.com",
        ),
        (
            "rust programming language",
            "en.wikipedia.org github.com/rust-lang/rust rust-lang.org",
            "rust-lang.org",
        ),
        (
            "wikipedia rust language",
            "rust-lang.org en.wikipedia.org",
            "wikipedia.org",
        ),
        (
            "tauri changelog",
            "github.com/tauri-apps/tauri tauri.app",
            "tauri.app",
        ),
        (
            "svelte docs",
            "svelte-support.com docs.svelte.dev",
            "svelte.dev",
        ),
        (
            "stripe",
            "stripe.com.example.net s-tripe.com stripes.com",
            "",
        ),
        (
            "postgres",
            "github.com/postgres stackoverflow.com docs.postgres.com",
            "postgres.com",
        ),
        ("mozilla", "mozilla-news.com mozilla.org", "mozilla.org"),
        ("linear", "nytimes.com linear.app", "linear.app"),
        (
            "vercel status",
            "status.vercel.com vercel.com",
            "vercel.com",
        ),
        ("netlify", "github.io reddit.com/r/netlify", ""),
        ("docker", "docker.io/docs docker.com", "docker.io"),
        ("rust", "en.wikipedia.org github.com", ""),
        ("acme", "store.steampowered.com news.ycombinator.com", ""),
        ("acme", "apps.apple.com play.google.com amazon.com", ""),
        ("acme", "quora.com g2.com trustpilot.com", ""),
        ("acme", "techcrunch.com reuters.com bbc.co.uk", ""),
        ("acme", "acmeevil.com fake-acme.com acme-login.com", ""),
        ("figma", "help.figma.com", "figma.com"),
        ("gitlab releases", "about.gitlab.com", "gitlab.com"),
        ("github", "gitlab.com github.com", "github.com"),
        ("reddit community", "old.reddit.com", "reddit.com"),
        ("amazon", "amazon.co.uk", "amazon.co.uk"),
        ("bbc", "bbc.co.uk", "bbc.co.uk"),
        ("youtube", "youtube.com", "youtube.com"),
        ("bluesky", "bsky.app", "bsky.app"),
        ("twitter", "x.com", "x.com"),
        ("bitbucket", "bitbucket.org", "bitbucket.org"),
        ("signal download", "support.signal.org", "signal.org"),
        ("acme tools", "acmetools.co.uk", "acmetools.co.uk"),
        ("acme", "acme.github.io", ""),
        ("acme", "docs.acme.co.uk", "acme.co.uk"),
        ("acme", "x.com/acme acme.app", "acme.app"),
        ("acme vs stripe", "acme.app stripe.com", ""),
        ("what is acme", "acme.app", ""),
    ];
    for (query, sites, expected) in cases {
        let html = sites
            .split_whitespace()
            .map(|s| card(s, "Result page"))
            .collect::<String>();
        let official = layer(query, &html)
            .organic
            .into_iter()
            .find_map(|r| r.official);
        assert_eq!(
            official.as_ref().map(|v| v.site.as_str()).unwrap_or(""),
            expected,
            "{query} {sites}"
        );
    }
}
#[test]
fn panel_evidence_requires_the_website_link_and_respects_intermediaries() {
    for (host, eligible) in [
        ("different-brand.com", true),
        ("github.com", false),
        ("en.wikipedia.org", false),
        ("apps.apple.com", false),
        ("quora.com", false),
        ("nytimes.com", false),
        ("ebay.com", false),
        ("linkedin.com", false),
    ] {
        let entry = card(host, "Acme");
        for (markup, match_expected) in [("<aside>", eligible), ("<div>", false)] {
            let l = layer(
                "acme",
                &format!("<main>{entry}{markup}<a href='https://{host}'>Website</a>"),
            );
            assert_eq!(
                l.organic.iter().any(|r| r.official.is_some()),
                match_expected,
                "{host}"
            );
        }
    }
}
#[test]
fn profiles_require_entity_and_path_and_are_deduplicated() {
    let mut html = card("acme.com", "Acme home");
    for path in [
        "github.com/acme/repo",
        "twitter.com/acme/status/2",
        "youtube.com/watch?v=acme",
        "instagram.com/wrong",
        "linkedin.com/posts/acme",
        "reddit.com/user/acme",
        "example.com/@acme",
        "facebook.com/acme/posts/1",
        "github.com/acme-tools",
    ] {
        html += &format!("<a href='https://{path}'>Acme</a>");
    }
    assert!(layer("acme", &html).social.is_empty());
    for path in [
        "github.com/acme",
        "x.com/acme",
        "twitter.com/acme",
        "youtube.com/@acme",
        "linkedin.com/company/acme",
        "discord.gg/abc",
        "reddit.com/r/acme",
        "fosstodon.org/@acme",
        "bsky.app/profile/acme.bsky.social",
        "instagram.com/acme",
        "facebook.com/acme",
    ] {
        html += &format!("<a href='https://{path}'>Acme</a>");
    }
    let l = layer("acme", &html);
    assert_eq!(l.social.len(), MAX_SOCIAL);
    assert_eq!(
        l.social
            .iter()
            .filter(|p| p.network == Network::Twitter)
            .count(),
        1
    );
}
#[test]
fn ad_labels_are_scoped_in_each_language_and_secondary_lists() {
    for label in [
        "Ad",
        "Gesponsert",
        "Anzeige",
        "Annonce",
        "Publicité",
        "Anuncio",
        "Patrocinado",
        "Sponsored",
    ] {
        let html=format!("<main><article><h2><a href='https://acme.com/'>Acme home</a></h2></article><article><span>{label}</span><h2><a href='https://youtube.com/watch?v=1'>Promotion</a></h2><a href='https://x.com/acme'>Follow</a><img src='https://acme.com/ad.png'></article></main>");
        let l = layer("acme", &html);
        assert_eq!(l.organic.len(), 1, "{label}");
        assert_eq!(l.dropped.paid, 1);
        assert!(l.images.is_empty() && l.social.is_empty() && l.video.is_empty());
    }
}
#[test]
fn bounded_collections_and_multibyte_text() {
    let title = "é".repeat(TITLE_BYTES);
    let snippet = "ç".repeat(SNIPPET_BYTES);
    let mut html =
        format!("<main><article><h2><a href='https://acme.com/'>{title}</a></h2><p>{snippet}</p>");
    for i in 0..=MAX_SITELINKS {
        html += &format!("<a href='https://acme.com/section/{i}'>Section {i}</a>");
    }
    html += "</article>";
    for i in 0..=MAX_IMAGES {
        html += &format!("<img alt='image {i}' src='https://cdn.acme.com/{i}.png'>");
    }
    for i in 0..=MAX_RESULTS {
        html += &card(&format!("youtube.com/watch?v={i}"), "A video");
    }
    let l = layer("acme", &html);
    assert_eq!(l.organic.len(), MAX_RESULTS);
    assert_eq!(l.video.len(), MAX_VIDEO);
    assert_eq!(l.images.len(), MAX_IMAGES);
    let first = &l.organic[0];
    assert_eq!(first.sitelinks.len(), MAX_SITELINKS);
    assert_eq!(first.title.len(), TITLE_BYTES);
    assert_eq!(first.snippet.as_ref().unwrap().len(), SNIPPET_BYTES);
}

#[test]
fn headings_in_inline_tables_belong_to_the_primary_result() {
    let html="<main><div><a href='https://acme.com'><h3>Acme home</h3></a><table><tr><td><a href='https://acme.com/plans'><h3>Plans</h3></a></td></tr></table></div><div><a href='https://acme.com/news'><h3>News</h3></a></div></main>";
    let l = layer("acme", html);
    assert_eq!(l.organic.len(), 2);
    assert_eq!(l.organic[0].sitelinks[0].title, "Plans");
}
#[test]
fn adjacent_wrappers_preserve_snippets_and_paid_labels() {
    let html="<main><div>Sponsored</div><div><a href='https://promoted.example'><h2>Unwanted result</h2></a></div>
    <p>Paid words supplied by the search engine.</p><div><a href='https://acme.com'><h2>Acme</h2></a></div>
    <p>Documentation for Acme contains the latest release notes.</p></main>";
    let l = layer("acme", html);
    assert_eq!(l.organic.len(), 1);
    assert_eq!(l.dropped.paid, 1);
    assert_eq!(
        l.organic[0].snippet.as_deref(),
        Some("Documentation for Acme contains the latest release notes.")
    );
}
#[test]
fn media_only_pages_and_engine_walls() {
    let html="<main><a href='https://acme.com'><img src='/thumb' alt='Example'></a><a href='https://youtu.be/abc'>Training</a><a href='https://youtube.com/watch'>Browse</a><a href='https://example.com/watch?v=1'>Browse</a></main>";
    let l = layer("acme", html);
    assert_eq!(l.images.len(), 1);
    assert_eq!(l.video.len(), 1);
    assert!(l.organic.is_empty());
    assert!(matches!(
        read(
            "acme",
            &format!("<title>Verify you are human</title>{html}")
        ),
        ResultsRead::Wall {
            wall: Wall::Challenge
        }
    ));
}
#[test]
fn the_reference_reader_deduplicates_www_slashes_and_campaigns() {
    let html = card("www.a.example/x/?utm_source=s#top", "First")
        + &card("a.example/x", "Duplicate")
        + &card("a.example/x?id=2", "Different");
    let l = layer("acme", &html);
    assert_eq!(l.organic.len(), 2);
    assert_eq!(repeat_count(&html), 1);
}
#[test]
fn an_oversized_custom_host_is_unreadable() {
    use zephium_map::key::MAX_URL_BYTES;
    let doc = Doc::from_html(
        &Url::parse("https://search.example/").unwrap(),
        card("acme.com", "Acme").as_bytes(),
        None,
        DocLimits::default(),
    )
    .unwrap();
    let query = read_query("acme").unwrap();
    for (len, valid) in [(MAX_URL_BYTES, true), (MAX_URL_BYTES + 1, false)] {
        let read = read_results(SearchEngine::Custom, Some(&"x".repeat(len)), &query, &doc);
        assert_eq!(matches!(read, ResultsRead::FirstLayer { layer: _ }), valid);
    }
}

#[test]
fn a_labelled_media_card_never_supplies_an_image_profile_or_video() {
    let html=card("acme.com","Acme home")+"<article><span>Anúncio</span><a href='https://youtu.be/sample'>Play</a><a href='https://x.com/acme'>Follow</a><img src='https://acme.com/ad'></article>";
    let l = layer("acme", &html);
    assert_eq!(l.organic.len(), 1);
    assert!(l.video.is_empty() && l.images.is_empty() && l.social.is_empty());
}

#[test]
fn drop_counts_include_repeats_beyond_the_display_limit() {
    let mut html = (0..=MAX_RESULTS)
        .map(|i| card(&format!("example.com/{i}"), "A result"))
        .collect::<String>();
    html += &card(&format!("example.com/{MAX_RESULTS}"), "Repeat");
    let l = layer("example", &html);
    assert_eq!(l.organic.len(), MAX_RESULTS);
    assert_eq!(repeat_count(&html), 1);
}

fn repeat_count(html: &str) -> u32 {
    let doc = Doc::from_html(
        &Url::parse("https://google.com/search").unwrap(),
        html.as_bytes(),
        None,
        DocLimits::RESULTS,
    )
    .unwrap();
    let mut stats = zephium_map::results::ResultStats::default();
    zephium_map::results::read_results_with_stats(
        SearchEngine::Google,
        None,
        &read_query("acme").unwrap(),
        &doc,
        &mut stats,
    );
    stats.repeat
}

#[test]
fn one_unscoped_paid_label_only_marks_the_following_card() {
    let l = layer("acme", "<main><h2><a href='https://acme.com/'>Acme</a></h2><p>Sponsored</p><h2><a href='https://promotion.example/'>Promotion</a></h2></main>");
    assert_eq!(l.dropped.paid, 1);
    assert_eq!(
        l.organic
            .iter()
            .map(|hit| hit.title.as_str())
            .collect::<Vec<_>>(),
        ["Acme"]
    );
}

#[test]
fn a_unique_trailing_paid_label_still_marks_its_card() {
    let l = layer("acme", "<main><h2><a href='https://promotion.example/'>Promotion</a></h2><p>Sponsored</p><p>One</p><p>Two</p><p>Three</p><p>Four</p><h2><a href='https://acme.com/'>Acme</a></h2></main>");
    assert_eq!(l.dropped.paid, 1);
    assert_eq!(l.organic[0].title, "Acme");
}

#[test]
fn both_direct_and_wrapped_result_destinations_keep_route_fragments() {
    let first = card("example.net/app#/docs", "Docs");
    let second = "<article><h2><a href='/url?q=https%3A%2F%2Fexample.net%2Fapp%23%2Fpricing'>Pricing</a></h2></article>";
    let ordinary = card("example.net/plain#heading", "Plain");
    let l = layer("acme", &format!("<main>{first}{second}{ordinary}</main>"));
    assert_eq!(
        l.organic
            .iter()
            .map(|hit| hit.url.fragment())
            .collect::<Vec<_>>(),
        [Some("/docs"), Some("/pricing"), None]
    );
}
