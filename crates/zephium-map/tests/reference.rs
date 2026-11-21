use url::Url;
use zephium_core::search::SearchEngine;
use zephium_map::{
    doc::{Doc, DocLimits, Role},
    query::read_query,
    results::{read_results, FirstLayer, ResultsRead, Wall},
};

fn read(engine: SearchEngine, page: &str, html: &str) -> ResultsRead {
    read_with_stats(engine, page, html).0
}
fn read_with_stats(
    engine: SearchEngine,
    page: &str,
    html: &str,
) -> (ResultsRead, zephium_map::results::ResultStats) {
    let url = Url::parse(page).unwrap();
    let doc = Doc::from_html(&url, html.as_bytes(), None, DocLimits::default()).unwrap();
    let mut stats = zephium_map::results::ResultStats::default();
    let read = zephium_map::results::read_results_with_stats(
        engine,
        url.host_str(),
        &read_query("rust async runtime").unwrap(),
        &doc,
        &mut stats,
    );
    if let ResultsRead::FirstLayer { layer } = &read {
        for (i, hit) in layer.organic.iter().enumerate() {
            assert_eq!(hit.rank, i as u32 + 1);
            assert!(hit.snippet.as_ref().is_none_or(|s| !s.is_empty()));
        }
    }
    (read, stats)
}

fn layer(read: &ResultsRead) -> &FirstLayer {
    match read {
        ResultsRead::FirstLayer { layer } => layer,
        ResultsRead::Wall { wall: w } => panic!("unexpected wall {w:?}"),
    }
}
fn hits(read: &ResultsRead) -> Vec<(&str, &str, Option<&str>)> {
    layer(read)
        .organic
        .iter()
        .map(|h| (h.title.as_str(), h.url.as_str(), h.snippet.as_deref()))
        .collect()
}

const TOKIO: &str = "Tokio is an event-driven, non-blocking I/O platform for writing asynchronous applications with Rust.";
const BOOK: &str =
    "This book aims to be a comprehensive, up-to-date guide to Rust's async ecosystem.";
const ASYNC_STD: &str =
    "Async version of the Rust standard library, with an interface you already know.";

#[test]
fn google_results_skip_its_ads_and_own_links_and_unwrap_its_redirect() {
    let (read, stats) = read_with_stats(
        SearchEngine::Google,
        "https://www.google.com/search?q=rust+async+runtime",
        include_str!(
            "../../zephium-agentic/src/provider_transport/decision/fixtures/search/google.html"
        ),
    );
    let found = hits(&read);
    let urls: Vec<&str> = found.iter().map(|(_, url, _)| *url).collect();
    // The ad, the repeat with a fragment, the ftp link, related searches
    // and the footer are gone; /url?q= leads to its page.
    assert_eq!(
        urls,
        [
            "https://tokio.rs/",
            "https://rust-lang.github.io/async-book/",
            "https://async.rs/",
        ]
    );
    assert_eq!(found[0].0, "Tokio - An asynchronous Rust runtime");
    // Google wraps this snippet in a span the page script reads no prose
    // from; one in a div's own text is kept.
    assert_eq!(found[0].2, None);
    assert_eq!(
        found[1].2,
        Some("This book aims to be a comprehensive, up-to-date guide to Rust's async ecosystem, suitable for beginners and old hands alike.")
    );
    assert_eq!(
        found[2],
        ("async-std", "https://async.rs/", Some(ASYNC_STD))
    );
    assert_eq!(layer(&read).dropped.paid, 1);
    assert_eq!((stats.own, stats.repeat), (1, 1));
}

#[test]
fn duckduckgo_results_drop_its_ad_click_and_follow_uddg() {
    let read = read(
        SearchEngine::DuckDuckGo,
        "https://duckduckgo.com/?q=rust+async+runtime",
        include_str!(
            "../../zephium-agentic/src/provider_transport/decision/fixtures/search/duckduckgo.html"
        ),
    );
    assert_eq!(
        hits(&read),
        [
            (
                "Tokio - An asynchronous Rust runtime",
                "https://tokio.rs/",
                Some(TOKIO)
            ),
            (
                "Asynchronous Programming in Rust",
                "https://rust-lang.github.io/async-book/",
                Some(BOOK)
            ),
            ("async-std", "https://async.rs/", Some(ASYNC_STD)),
        ]
    );
}

#[test]
fn bing_results_decode_ck_redirects_and_skip_labelled_ads_and_repeats() {
    let read = read(
        SearchEngine::Bing,
        "https://www.bing.com/search?q=rust+async+runtime",
        include_str!(
            "../../zephium-agentic/src/provider_transport/decision/fixtures/search/bing.html"
        ),
    );
    // The repeat carries a campaign parameter and still names the same page.
    assert_eq!(
        hits(&read),
        [
            (
                "Tokio - An asynchronous Rust runtime",
                "https://tokio.rs/",
                Some(TOKIO)
            ),
            (
                "Asynchronous Programming in Rust",
                "https://rust-lang.github.io/async-book/",
                Some(BOOK)
            ),
            ("async-std", "https://async.rs/", Some(ASYNC_STD)),
        ]
    );
}

#[test]
fn brave_results_read_titles_held_as_text_and_skip_sponsored() {
    let read = read(
        SearchEngine::Brave,
        "https://search.brave.com/search?q=rust+async+runtime",
        include_str!(
            "../../zephium-agentic/src/provider_transport/decision/fixtures/search/brave.html"
        ),
    );
    assert_eq!(
        hits(&read),
        [
            (
                "Tokio - An asynchronous Rust runtime",
                "https://tokio.rs/",
                Some(TOKIO)
            ),
            (
                "Asynchronous Programming in Rust",
                "https://rust-lang.github.io/async-book/",
                Some(BOOK)
            ),
            ("async-std", "https://async.rs/", Some(ASYNC_STD)),
        ]
    );
}

#[test]
fn a_custom_engine_reads_heading_links_and_drops_its_own() {
    let read = read(
        SearchEngine::Custom,
        "https://search.example.net/search?q=rust+async+runtime",
        include_str!(
            "../../zephium-agentic/src/provider_transport/decision/fixtures/search/custom.html"
        ),
    );
    assert_eq!(
        hits(&read),
        [
            (
                "Tokio - An asynchronous Rust runtime",
                "https://tokio.rs/",
                Some(TOKIO)
            ),
            ("async-std", "https://async.rs/", Some(ASYNC_STD)),
        ]
    );
}

#[test]
fn walls_say_what_stood_in_the_way() {
    for (engine, page, html, wall) in [
        (
            SearchEngine::Google,
            "https://www.google.com/sorry/index?continue=https://www.google.com/search",
            include_str!("../../zephium-agentic/src/provider_transport/decision/fixtures/search/google-sorry.html"),
            Wall::Challenge,
        ),
        (
            SearchEngine::DuckDuckGo,
            "https://duckduckgo.com/?q=rust+async+runtime",
            include_str!("../../zephium-agentic/src/provider_transport/decision/fixtures/search/duckduckgo-anomaly.html"),
            Wall::Challenge,
        ),
        (
            SearchEngine::Google,
            "https://consent.google.com/ml?continue=https://www.google.com/search",
            include_str!("../../zephium-agentic/src/provider_transport/decision/fixtures/search/google-consent.html"),
            Wall::Consent,
        ),
        (
            SearchEngine::Bing,
            "https://www.bing.com/search?q=qzxv+wkrrt+plorb",
            include_str!("../../zephium-agentic/src/provider_transport/decision/fixtures/search/bing-none.html"),
            Wall::NoResults,
        ),
        (
            SearchEngine::Brave,
            "https://search.brave.com/search?q=rust",
            "<html><head><title>Brave Search</title></head><body><main><p>Loading</p></main></body></html>",
            Wall::Unreadable,
        ),
    ] {
        let read = read(engine, page, html);
        assert!(matches!(read,ResultsRead::Wall{wall:w} if w==wall), "{page}");
    }
}

#[test]
fn a_results_address_link_never_stands_as_its_title() {
    // The address link sits in the result's heading too, before the title.
    let html = r#"<html><body><main><ol>
      <li><article>
        <h3><a href="https://allegro.pl/listing?string=karta+3090"><span>allegro.pl</span></a></h3>
        <h2><a href="https://allegro.pl/listing?string=karta+3090"><span>Karta 3090 24 GB - Allegro</span></a></h2>
      </article></li>
      <li><article>
        <h2><a href="https://allegro.pl/listing?string=rtx+3090"><span>allegro.pl</span></a>
            <a href="https://allegro.pl/listing?string=rtx+3090"><span>Karta graficzna RTX 3090 - Niska cena na Allegro</span></a></h2>
        <div><span>Karty graficzne RTX 3090 24 GB w niskich cenach.</span></div>
      </article></li>
      <li><article>
        <h2><a href="https://www.ycombinator.com/blog/demo-day"><span>ycombinator.com › blog</span></a>
            <a href="https://www.ycombinator.com/blog/demo-day"><span>2026 Demo Day Dates | Y Combinator</span></a></h2>
      </article></li>
    </ol></main></body></html>"#;
    let read = read(
        SearchEngine::DuckDuckGo,
        "https://duckduckgo.com/?q=rtx+3090",
        html,
    );
    let titles: Vec<&str> = hits(&read).iter().map(|(title, ..)| *title).collect();
    assert_eq!(
        titles,
        [
            "Karta 3090 24 GB - Allegro",
            "Karta graficzna RTX 3090 - Niska cena na Allegro",
            "2026 Demo Day Dates | Y Combinator"
        ]
    );
}

#[test]
fn redirect_destinations_through_html() {
    use SearchEngine::*;
    for (engine, host, target, expected) in [
        (
            Google,
            "www.google.com",
            "https://www.google.com/url?q=https://a.example/x&sa=U",
            Some("https://a.example/x"),
        ),
        (
            Google,
            "www.google.co.uk",
            "/url?q=javascript:alert(1)",
            None,
        ),
        (
            Google,
            "www.google.com",
            "https://www.googleadservices.com/pagead/aclk?adurl=https://a.example/",
            None,
        ),
        (
            Google,
            "www.google.com",
            "https://googleadservices.com/url?q=https://a.example/",
            None,
        ),
        (
            Google,
            "www.google.com",
            "https://support.google.com/websearch",
            Some("https://support.google.com/websearch"),
        ),
        (
            DuckDuckGo,
            "duckduckgo.com",
            "/y.js?ad_domain=a.example&u3=https://a.example/",
            None,
        ),
        (
            Bing,
            "bing.com",
            "/ck/a?u=a1aHR0cDovL2EuZXhhbXBsZS8&ntb=1",
            Some("http://a.example/"),
        ),
        (Bing, "bing.com", "/ck/a?u=a1ZmlsZTovLy9ldGMvcGFzc3dk", None),
        (
            Brave,
            "search.brave.com",
            "/a/redirect?url=https://a.example/",
            Some("https://a.example/"),
        ),
        (
            Custom,
            "search.example",
            "/go?target=https://:pass@a.example/",
            None,
        ),
        (
            Custom,
            "search.example",
            "/go?url=https://doubleclick.net/",
            None,
        ),
    ] {
        let html = format!("<h2><a href='{target}'>A result</a></h2>");
        let read = read(engine, &format!("https://{host}"), &html);
        let actual = match &read {
            ResultsRead::FirstLayer { layer: l } => Some(l.organic[0].url.as_str()),
            _ => None,
        };
        assert_eq!(actual, expected);
    }
}

#[test]
fn live_nodes_and_html_return_the_same_three_cited_rows() {
    use zephium_map::doc::{DocBuilder, Node};
    let url = Url::parse("https://duckduckgo.com/").unwrap();
    let html = include_str!(
        "../../zephium-agentic/src/provider_transport/decision/fixtures/search/duckduckgo.html"
    );
    let doc = Doc::from_html(&url, html.as_bytes(), None, DocLimits::default()).unwrap();
    let mut builder = DocBuilder::new(&url, DocLimits::default()).unwrap();
    for n in doc.nodes() {
        let mut observed = Node::new(n.role, n.parent);
        observed.name = n.accessible_name().to_owned();
        if !matches!(
            n.role,
            Role::Heading { level: _ } | Role::Link | Role::Button
        ) {
            observed.text = n.text.clone();
        }
        observed.destination = n.destination.clone();
        observed.image_source = n.image_source.clone();
        builder.push(observed).unwrap();
    }
    let live = builder.finish();
    let query = read_query("rust async runtime").unwrap();
    let a = read_results(SearchEngine::DuckDuckGo, None, &query, &doc);
    let b = read_results(SearchEngine::DuckDuckGo, None, &query, &live);
    assert_eq!(hits(&a), hits(&b));
    assert_eq!(layer(&b).organic.len(), 3);
    for (html, observed) in layer(&a).organic.iter().zip(&layer(&b).organic) {
        assert_eq!(html.rank, observed.rank);
        assert!(html.key == observed.key);
    }
}
