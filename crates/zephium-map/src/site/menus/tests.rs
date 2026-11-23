use super::*;

#[test]
fn structural_quality_compares_the_emitted_rows_to_observations() {
    let url = Url::parse("https://workshop.example/").unwrap();
    let doc = Doc::from_html(
        &url,
        b"<nav><a href='/a'>One</a><a href='/b'>Two</a></nav>",
        None,
        crate::DocLimits::SITE_PAGE,
    )
    .unwrap();
    let mut place = read_site(&doc, &Default::default()).unwrap();
    let child = place.sections.pop().unwrap();
    let evidence = Evidence {
        direct: [(child.id.clone(), child.card_route())].into(),
        pages: [("topics".into(), None)].into(),
        languages: ["editions".into()].into(),
        editions: Default::default(),
    };
    place.sections[0].label = "Topics".into();
    place.sections[0].children.push(child);
    let mut stats = PlaceStats::default();
    evidence.measure(&place, &mut stats);
    assert_eq!(stats.missing_header_links, 1);
    assert_eq!(stats.invalid_group_pages, 1);
    place.sections[0].label = "Editions".into();
    let mut stats = PlaceStats::default();
    evidence.measure(&place, &mut stats);
    assert_eq!(stats.language_groups, 1);
}
