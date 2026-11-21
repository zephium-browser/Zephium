use url::Url;
use zephium_core::search::SearchEngine;
use zephium_map::{
    doc::*,
    key::{KeyError, MAX_URL_BYTES},
    query::read_query,
    results::{read_results, ResultsRead, Wall},
};
fn page() -> Url {
    Url::parse("https://example.com/start/page").unwrap()
}
fn parse(raw: &[u8], limits: DocLimits) -> Result<Doc, DocError> {
    Doc::from_html(&page(), raw, None, limits)
}
fn defaults(raw: &str) -> Doc {
    parse(raw.as_bytes(), DocLimits::default()).unwrap()
}

#[test]
fn handles_declared_and_meta_charsets() {
    for meta in [
        "<META CHARSET=windows-1252>",
        "<meta http-equiv='Content-Type' content='text/html; charset=windows-1252'>",
    ] {
        let mut raw = meta.as_bytes().to_vec();
        raw.extend(b"<p>\x80</p>");
        let p = |doc: Doc| {
            doc.nodes()
                .iter()
                .find(|n| n.role == Role::Paragraph)
                .unwrap()
                .text
                .clone()
        };
        assert_eq!(p(parse(&raw, DocLimits::default()).unwrap()), "€");
        assert_eq!(
            p(Doc::from_html(&page(), &raw, Some("utf-8"), DocLimits::default()).unwrap()),
            "�"
        );
        assert_eq!(
            p(Doc::from_html(&page(), &raw, Some("not-a-charset"), DocLimits::default()).unwrap()),
            "€"
        );
    }
    let mut raw = b"<!-- <meta charset=windows-1252> --><p>".to_vec();
    raw.push(0x80);
    assert!(parse(&raw, DocLimits::default())
        .unwrap()
        .nodes()
        .iter()
        .any(|n| n.text == "�"));
    let meta = b"<meta charset=windows-1252>";
    for (extra, expected) in [(0, "€"), (1, "�")] {
        let mut raw = vec![b' '; 4096 - meta.len() + extra];
        raw.extend(meta);
        raw.extend(b"<p>\x80");
        assert!(parse(&raw, DocLimits::default())
            .unwrap()
            .nodes()
            .iter()
            .any(|n| n.text == expected));
    }
}
#[test]
fn head_metadata_and_safe_base() {
    let html = r#"<head><base href="/resources/"><title>  Some   title  </title><link rel=canonical href=page>
       <meta property="og:site_name" content="An Example"><link rel="alternate feed" type="application/rss+xml" href="feed" hreflang=en title="Recent items">
       <script type="application/ld+json"> { "name": "opaque" } </script></head>
       <main><a href="go">go</a><img alt="photo" src="pic.jpg"></main>"#;
    let doc = defaults(html);
    assert!(!doc.truncated());
    assert_eq!(doc.head().title, "Some title");
    assert_eq!(
        doc.head().canonical.as_ref().unwrap().as_str(),
        "https://example.com/resources/page"
    );
    assert_eq!(doc.head().site_name, "An Example");
    assert_eq!(doc.head().linked_data, [" { \"name\": \"opaque\" } "]);
    let alt = &doc.head().alternates[0];
    assert_eq!(alt.url.path(), "/resources/feed");
    assert_eq!(alt.media_type, "application/rss+xml");
    assert_eq!(alt.language, "en");
    assert_eq!(alt.title, "Recent items");
    assert!(doc.nodes().iter().any(|n| n
        .destination
        .as_ref()
        .is_some_and(|u| u.path() == "/resources/go")));
    assert!(doc.nodes().iter().any(|n| n
        .image_source
        .as_ref()
        .is_some_and(|u| u.path() == "/resources/pic.jpg")
        && n.name == "photo"));
    for base in [
        "https://other.example/",
        "http://example.com/",
        "https://example.com:444/",
        "https://name@example.com/",
    ] {
        let doc = defaults(&format!("<base href='{base}'><a href=relative>text</a>"));
        assert_eq!(
            doc.nodes()
                .iter()
                .find_map(|n| n.destination.as_ref())
                .unwrap()
                .as_str(),
            "https://example.com/start/relative"
        );
    }
}
#[test]
fn structure_visibility_and_raw_code() {
    let doc=defaults("<main><h3 id='intro'> first  <em>heading</em></h3><nav aria-label='navigation'><a href='/x' aria-label='accessible'>visible</a></nav>
        <pre><code>  fn x() {\n    x +  1\n} </code></pre><table><tr><th>Name</th><td>A</td></tr></table>
        <div role='unsupported heading' aria-level=4>Custom</div>
        <div hidden>forbidden</div><span aria-hidden='TRUE'>forbidden</span><script>forbidden</script><style>forbidden</style><template>forbidden</template><noscript>forbidden</noscript><svg><title>forbidden</title></svg>
        <img src='data:image/png;base64,AA'><a href='javascript:alert(1)'>js</a><a href='https://:pass@example.com/'>credentials</a><a href='data:text/plain,x'>data</a></main>");
    assert!(doc.nodes().iter().all(|n| !n.text.contains("forbidden")));
    let role = |role| doc.nodes().iter().find(|n| n.role == role).unwrap();
    assert_eq!(role(Role::Heading { level: Some(3) }).text, "first heading");
    assert_eq!(
        role(Role::Heading { level: Some(3) }).element_id.as_deref(),
        Some("intro")
    );
    assert_eq!(role(Role::CodeBlock).text, "  fn x() {\n    x +  1\n} ");
    assert_eq!(role(Role::Heading { level: Some(4) }).text, "Custom");
    assert_eq!(role(Role::CellHeader).text, "Name");
    assert_eq!(role(Role::Cell).text, "A");
    let link = role(Role::Link);
    assert_eq!(link.name, "accessible");
    assert_eq!(link.text, "visible");
    assert_eq!(
        doc.nodes()[link.parent.unwrap() as usize].role,
        Role::Landmark {
            landmark: Some(Landmark::Navigation)
        }
    );
    assert_eq!(
        doc.nodes()
            .iter()
            .filter(|n| n.destination.is_some())
            .count(),
        1
    );
    assert!(role(Role::Image).image_source.is_none());
}
#[test]
fn hostile_sizes_are_refused_before_parsing() {
    let nested = "<div>".repeat(10 * 1024 * 1024 / 5 + 1);
    let links = "<a href='/a'>x</a>".repeat(1_000_000);
    for text in [&nested, &links] {
        assert!(matches!(
            parse(text.as_bytes(), DocLimits::default()),
            Err(DocError::Input)
        ));
    }
    let doc = parse(b"<p>bad\xff\0 <em>unclosed", DocLimits::default()).unwrap();
    assert!(doc
        .nodes()
        .iter()
        .any(|n| n.text.contains("bad�") && n.text.contains("unclosed")));
    assert!(parse(&vec![b' '; MAX_INPUT_BYTES], DocLimits::default()).is_ok());
    assert!(matches!(
        parse(&vec![b' '; MAX_INPUT_BYTES + 1], DocLimits::default()),
        Err(DocError::Input)
    ));
}

#[test]
fn builder_validates_preorder_and_refusals_leave_it_usable() {
    let mut b = DocBuilder::new(
        &page(),
        DocLimits {
            nodes: 4,
            depth: 2,
            node_text_bytes: 4,
            text_bytes: 6,
            ..Default::default()
        },
    )
    .unwrap();
    let add = |text: &str, p| {
        let mut n = Node::new(Role::Paragraph, p);
        n.text = text.into();
        n
    };
    assert_eq!(b.push(add("", Some(0))), Err(DocError::Parent));
    assert_eq!(b.push(add("abcd", None)), Ok(0));
    assert_eq!(b.push(add("", Some(2))), Err(DocError::Parent));
    assert_eq!(b.push(add("abcde", Some(0))), Err(DocError::Text));
    assert_eq!(b.push(add("ab", Some(0))), Ok(1));
    assert_eq!(b.push(add("", Some(1))), Err(DocError::Depth));
    assert_eq!(b.push(add("x", Some(0))), Err(DocError::Text));
    assert_eq!(b.push(add("", None)), Ok(2));
    assert_eq!(b.push(add("", Some(0))), Err(DocError::Parent));
    assert_eq!(b.push(add("", Some(2))), Ok(3));
    assert_eq!(b.push(add("", None)), Err(DocError::Nodes));
    assert_eq!(b.finish().nodes().len(), 4);
}
#[test]
fn html_limits_report_truncation_and_preserve_atomic_fields() {
    for (html, limits) in [
        (
            "<p>0</p><p>1</p>",
            DocLimits {
                nodes: 2,
                ..Default::default()
            },
        ),
        (
            "<main><div><p>Hello</p></div></main>",
            DocLimits {
                depth: 2,
                ..Default::default()
            },
        ),
        (
            "<p>ééé",
            DocLimits {
                node_text_bytes: 5,
                ..Default::default()
            },
        ),
        (
            "<p>x</p><p>y</p>",
            DocLimits {
                text_bytes: 1,
                ..Default::default()
            },
        ),
        (
            "<p id='too-long'>s",
            DocLimits {
                node_text_bytes: 1,
                ..Default::default()
            },
        ),
        (
            "<head><script type='application/ld+json'>{}</script>",
            DocLimits {
                node_text_bytes: 1,
                ..Default::default()
            },
        ),
    ] {
        let doc = parse(html.as_bytes(), limits).unwrap();
        assert!(doc.truncated());
        assert!(doc.head().linked_data.is_empty());
        assert!(doc.nodes().iter().all(|n| n.text.len() + n.name.len()
            <= limits.node_text_bytes as usize
            && n.element_id.is_none()));
    }
    let head = "<link rel=alternate href='/feed'><script type='application/ld+json'>{}</script>";
    for (count, truncated) in [(MAX_HEAD_ITEMS, false), (MAX_HEAD_ITEMS + 1, true)] {
        let doc = defaults(&head.repeat(count));
        assert_eq!(doc.truncated(), truncated);
        assert_eq!(doc.head().alternates.len(), MAX_HEAD_ITEMS);
        assert_eq!(doc.head().linked_data.len(), MAX_HEAD_ITEMS);
    }
    let u = page();
    let l = DocLimits {
        url_bytes: u.as_str().len() as u32,
        ..Default::default()
    };
    let doc = parse(b"<a href='/more-than-the-original-url'>x", l).unwrap();
    assert!(doc.truncated());
    assert!(!doc.nodes().iter().any(|n| n.destination.is_some()));
    assert!(matches!(
        DocBuilder::new(
            &u,
            DocLimits {
                url_bytes: l.url_bytes - 1,
                ..l
            }
        ),
        Err(DocError::Url {
            error: KeyError::UrlLength
        })
    ));
}
#[test]
fn absolute_ceilings_apply_to_callers() {
    for limits in [
        DocLimits {
            input_bytes: MAX_INPUT_BYTES as u32 + 1,
            ..Default::default()
        },
        DocLimits {
            nodes: MAX_NODES as u32 + 1,
            ..Default::default()
        },
        DocLimits {
            depth: MAX_DEPTH as u32 + 1,
            ..Default::default()
        },
        DocLimits {
            text_bytes: MAX_TEXT_BYTES as u32 + 1,
            ..Default::default()
        },
        DocLimits {
            node_text_bytes: MAX_NODE_TEXT_BYTES as u32 + 1,
            ..Default::default()
        },
        DocLimits {
            url_bytes: MAX_URL_BYTES as u32 + 1,
            ..Default::default()
        },
    ] {
        assert!(matches!(
            DocBuilder::new(&page(), limits),
            Err(DocError::Limits)
        ));
    }
    let mut b = DocBuilder::new(&page(), DocLimits::default()).unwrap();
    for _ in 0..MAX_NODES {
        b.push(Node::new(Role::Group, None)).unwrap();
    }
    assert_eq!(b.push(Node::new(Role::Group, None)), Err(DocError::Nodes));
    let mut b = DocBuilder::new(&page(), DocLimits::default()).unwrap();
    for i in 0..MAX_DEPTH {
        b.push(Node::new(Role::Group, i.checked_sub(1).map(|p| p as u32)))
            .unwrap();
    }
    assert_eq!(
        b.push(Node::new(Role::Group, Some(MAX_DEPTH as u32 - 1))),
        Err(DocError::Depth)
    );
    let mut b = DocBuilder::new(&page(), DocLimits::default()).unwrap();
    let text = |size| {
        let mut n = Node::new(Role::Paragraph, None);
        n.text = "x".repeat(size);
        n
    };
    assert_eq!(b.push(text(MAX_NODE_TEXT_BYTES + 1)), Err(DocError::Text));
    for _ in 0..MAX_TEXT_BYTES / MAX_NODE_TEXT_BYTES {
        b.push(text(MAX_NODE_TEXT_BYTES)).unwrap();
    }
    assert_eq!(b.push(text(1)), Err(DocError::Text));
}
#[test]
fn live_document_filters_hidden_descendants_and_validates_role_urls() {
    let mut b = DocBuilder::new(&page(), Default::default()).unwrap();
    let mut root = Node::new(Role::Group, None);
    root.hidden = true;
    let p = b.push(root).unwrap();
    let mut h = Node::new(Role::Heading { level: None }, Some(p));
    h.name = "Heading".into();
    let h = b.push(h).unwrap();
    let mut n = Node::new(Role::Link, Some(h));
    n.destination = Some(page());
    n.name = "Result".into();
    b.push(n).unwrap();
    let mut bad = Node::new(Role::Paragraph, None);
    bad.destination = Some(page());
    assert_eq!(b.push(bad), Err(DocError::Role));
    assert_eq!(
        b.push(Node::new(Role::Heading { level: Some(8) }, None)),
        Err(DocError::Role)
    );
    let mut bad = Node::new(Role::Link, None);
    bad.destination = Some(Url::parse("https://user@example.net").unwrap());
    assert!(matches!(b.push(bad), Err(DocError::Url { error: _ })));
    let q = read_query("example").unwrap();
    assert!(matches!(
        read_results(SearchEngine::Bing, None, &q, &b.finish()),
        ResultsRead::Wall {
            wall: Wall::Unreadable
        }
    ));
    let mut b = DocBuilder::new(&page(), Default::default()).unwrap();
    b.mark_truncated();
    assert!(b.finish().truncated());
}

#[test]
fn dropped_head_metadata_keeps_complete_organic_results_readable() {
    let html=format!("<script type='application/ld+json'>{}</script><h2><a href='https://acme.net'>Acme</a></h2>","x".repeat(MAX_NODE_TEXT_BYTES+1));
    let doc = defaults(&html);
    assert!(doc.truncated() && doc.head().truncated);
    assert!(!doc.body_truncated());
    assert!(matches!(
        read_results(SearchEngine::Bing, None, &read_query("acme").unwrap(), &doc),
        ResultsRead::FirstLayer { layer: _ }
    ));
    let html = format!(
        "<base href='{}'><a href='next'>Next</a>",
        "x".repeat(MAX_URL_BYTES + 1)
    );
    assert!(matches!(
        parse(html.as_bytes(), DocLimits::default()),
        Err(DocError::Url {
            error: KeyError::UrlLength
        })
    ));
}

#[test]
fn page_strings_strip_directional_and_zero_width_controls() {
    let doc=defaults("<title>A\u{202e}cme</title><main><h2 id='a\u{200b}b'>A\u{2066}B</h2><h3 id='kept'>Kept</h3><a aria-label='X\u{feff}Y' href='/'>Link</a></main>");
    assert_eq!(doc.head().title, "Acme");
    let heading = doc.nodes().iter().find(|n| n.element_id.is_some()).unwrap();
    assert_eq!(heading.element_id.as_deref(), Some("ab"));
    assert!(heading.element_id_cleaned);
    assert_eq!(heading.text, "AB");
    let kept = doc.nodes().iter().find(|n| n.text == "Kept").unwrap();
    assert_eq!(kept.element_id.as_deref(), Some("kept"));
    assert!(!kept.element_id_cleaned);
    assert!(doc.nodes().iter().any(|n| n.name == "XY"));
    assert_eq!(
        zephium_map::key::LocatorText::new("a\u{200d}b")
            .unwrap()
            .as_str(),
        "ab"
    );
}
