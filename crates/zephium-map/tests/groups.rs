use url::Url;
use zephium_core::search::SearchEngine;
use zephium_map::*;
fn read(html: &str) -> FirstLayer {
    let doc = Doc::from_html(
        &Url::parse("https://www.bing.com/search").unwrap(),
        html.as_bytes(),
        None,
        DocLimits::RESULTS,
    )
    .unwrap();
    let ResultsRead::FirstLayer(layer) = read_results(
        SearchEngine::Bing,
        None,
        &read_query("timer").unwrap(),
        &doc,
    ) else {
        panic!("expected first layer")
    };
    layer
}
#[test]
fn cited_answers_discussions_and_widget() {
    let html = include_str!("fixtures/engine-groups.html");
    let layer = read(html);
    assert!(layer.widget);
    assert_eq!(layer.dropped.generated, 1);
    assert_eq!(layer.organic.len(), 3);
    assert_eq!(layer.discussion, [1, 2]);
    let f = layer.featured.unwrap();
    assert_eq!(f.organic_index, 0);
    assert_eq!(f.text, "The reset switch clears the saved timer.");
    assert!(layer.organic.iter().all(|r| !r
        .snippet
        .as_deref()
        .unwrap_or_default()
        .contains("Generated")));
    assert!(read(&html.replace("guide\">Source", "uncited\">Source"))
        .featured
        .is_none());
}

#[test]
fn semantic_overview_heading_suppresses_its_card() {
    let layer=read("<main><section><h2>AI Overview</h2><p>Generated content.</p><h3><a href='https://unrelated.example/'>Invented answer</a></h3></section><article><h2><a href='https://workshop.example/'>Workshop</a></h2></article></main>");
    assert_eq!(layer.organic.len(), 1);
    assert_eq!(layer.dropped.generated, 1);
    assert!(layer.featured.is_none());
}

#[test]
fn widget_contents_do_not_supply_organic_evidence() {
    let layer=read("<main><section aria-label='Weather'><h2><a href='https://unrelated.example/'>Forecast provider</a></h2></section><article><h2><a href='https://workshop.example/'>Workshop</a></h2></article></main>");
    assert!(layer.widget);
    assert_eq!(layer.organic.len(), 1);
    assert_eq!(layer.organic[0].url.host_str(), Some("workshop.example"));
}
