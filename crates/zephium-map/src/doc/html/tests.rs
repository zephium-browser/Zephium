use super::*;

#[test]
fn parser_and_charset_boundary() {
    let padded = format!(
        "{:width$}",
        "utf-8",
        width = decode::MAX_CHARSET_LABEL_BYTES
    );
    assert!(decode::charset(padded.as_bytes()).is_some());
    assert!(decode::charset((padded + " ").as_bytes()).is_none());
}

fn document(bytes: &[u8], limits: DocLimits) -> Doc {
    Doc::from_html(
        &Url::parse("https://example.com/").unwrap(),
        bytes,
        None,
        limits,
    )
    .unwrap()
}

fn large_page() -> String {
    const ELEMENTS: usize = 60_000;
    const BYTES: usize = 3 * 1024 * 1024;
    const LEVELS: usize = 40;
    const CARDS: usize = 1000;
    const TAIL: &str = "</script><h2>Final heading</h2><a href='/last'>Last link</a><p>Final passage</p></body></html>";
    let mut html = String::with_capacity(BYTES);
    html.push_str("<html><head><script>if (a < b) go()</script></head><body>");
    for _ in 0..LEVELS {
        html.push_str("<div>")
    }
    for _ in 0..CARDS {
        html.push_str("<section><h3>Result</h3><a href='/card'>Read</a><p>Summary</p></section>")
    }
    for _ in 0..ELEMENTS - (3 + 2 + 3 + LEVELS + CARDS * 4) {
        html.push_str("<span></span>")
    }
    for _ in 0..LEVELS {
        html.push_str("</div>")
    }
    html.push_str("<script type='application/json'>{\"data\":\"");
    let room = BYTES - html.len() - 2 - TAIL.len();
    const PART: &str = "abcdefghijklmno<";
    for _ in 0..room / PART.len() {
        html.push_str(PART)
    }
    html.push_str(&PART[..room % PART.len()]);
    html.push_str("\"}");
    html.push_str(TAIL);
    assert_eq!(html.len(), BYTES);
    html
}

#[test]
fn reads_large_page_through_the_last_passage() {
    let html = large_page();
    let arena = decode::parse(html.as_bytes(), None, DocLimits::default());
    assert_eq!(arena.element_count(), 60_000);
    let doc = document(html.as_bytes(), DocLimits::SITE_PAGE);
    assert!(!doc.truncated());
    assert!(doc
        .nodes()
        .iter()
        .any(|n| n.role == Role::Heading { level: Some(2) } && n.text == "Final heading"));
    assert!(doc.nodes().iter().any(|n| n.role == Role::Link
        && n.text == "Last link"
        && n.destination.as_ref().is_some_and(|u| u.path() == "/last")));
    assert!(doc
        .nodes()
        .iter()
        .any(|n| n.role == Role::Paragraph && n.text == "Final passage"));
}

#[test]
#[ignore = "release measurement"]
fn large_page_median() {
    let html = large_page();
    let mut timings = Vec::new();
    for _ in 0..20 {
        let start = std::time::Instant::now();
        let doc = std::hint::black_box(document(
            std::hint::black_box(html.as_bytes()),
            DocLimits::default(),
        ));
        timings.push(start.elapsed());
        assert!(!doc.truncated());
    }
    timings.sort_unstable();
    println!(
        "20 parses, median {:.3} ms",
        (timings[9] + timings[10]).as_secs_f64() * 500.0
    );
}

#[test]
fn element_capacity_stops_feeding_before_the_tail() {
    let tail = "<h2>after capacity</h2>";
    let mut html = "<br>".repeat((MAX_INPUT_BYTES - tail.len()) / 4);
    html.extend(std::iter::repeat_n(
        ' ',
        MAX_INPUT_BYTES - html.len() - tail.len(),
    ));
    html.push_str(tail);
    let limits = DocLimits::default();
    let arena = decode::parse(html.as_bytes(), None, limits);
    assert_eq!(arena.element_count(), MAX_ARENA_ELEMENTS);
    assert!(arena.stopped);
    let doc = document(html.as_bytes(), limits);
    assert!(doc.body_truncated());
    assert!(doc
        .nodes()
        .iter()
        .all(|n| !n.text.contains("after capacity")));
    assert!(matches!(
        crate::read_results(
            zephium_core::search::SearchEngine::Bing,
            None,
            &crate::read_query("example").unwrap(),
            &doc
        ),
        crate::ResultsRead::Wall {
            wall: crate::Wall::Unreadable
        }
    ));
    let mut html = "<div>".repeat(MAX_INPUT_BYTES / 5);
    html.extend(std::iter::repeat_n(' ', MAX_INPUT_BYTES - html.len()));
    let doc = document(html.as_bytes(), limits);
    assert!(doc.body_truncated());
    assert!(doc.nodes().len() <= MAX_NODES);
}

#[test]
fn depth_lifts_nodes_and_continues_into_later_siblings() {
    let limits = DocLimits {
        arena_depth: 8,
        depth: 4,
        ..Default::default()
    };
    let html = format!(
        "{}<h2>deep</h2>{}<p>shallow</p>",
        "<div>".repeat(40),
        "</div>".repeat(40)
    );
    let doc = document(html.as_bytes(), limits);
    assert!(doc.body_truncated());
    let heading = doc
        .nodes()
        .iter()
        .find(|n| n.role == Role::Heading { level: Some(2) } && n.text == "deep")
        .unwrap();
    let ancestors =
        std::iter::successors(heading.parent_index(), |&i| doc.nodes()[i].parent_index()).count();
    assert_eq!(ancestors, 3);
    assert!(doc
        .nodes()
        .iter()
        .any(|n| n.role == Role::Paragraph && n.text == "shallow" && n.parent_index() == Some(0)));
}

#[test]
fn declared_charsets_across_source_chunks() {
    for (label, first, second, expected) in [
        ("windows-1251", 0xcf, 0xf0, "Пр"),
        ("shift_jis", 0x93, 0xfa, "日"),
    ] {
        let mut bytes = format!("<meta charset='{label}'><!--").into_bytes();
        bytes.resize(decode::CHUNK_BYTES - b"--><p>".len() - 1, b' ');
        bytes.extend(b"--><p>");
        assert_eq!(bytes.len() + 1, decode::CHUNK_BYTES);
        bytes.extend([first, second]);
        assert!(document(&bytes, Default::default())
            .nodes()
            .iter()
            .any(|n| n.text == expected));
    }
}

#[test]
fn browser_repairs_paragraphs_lists_links_and_fostered_content() {
    let html = b"<main><p>one<p>two<table><p>fostered<tr><td>cell</table><ul><li>first<li><a href='/a'>A<a href='/b'>B</ul></main>";
    let doc = document(html, Default::default());
    assert!(!doc.truncated());
    let at = |role, text| {
        doc.nodes()
            .iter()
            .position(|n| n.role == role && n.text == text)
            .unwrap()
    };
    let one = at(Role::Paragraph, "one");
    let two = at(Role::Paragraph, "two");
    let fostered = at(Role::Paragraph, "fostered");
    let table = at(Role::Table, "");
    assert!(one < two && two < fostered && fostered < table);
    assert_eq!(
        doc.nodes()[one].parent_index(),
        doc.nodes()[two].parent_index()
    );
    assert_eq!(
        doc.nodes()[fostered].parent_index(),
        doc.nodes()[table].parent_index()
    );
    let cell = at(Role::Cell, "cell");
    assert_eq!(
        doc.nodes()[doc.nodes()[cell].parent_index().unwrap()].role,
        Role::Row
    );
    let first = at(Role::ListItem, "first");
    let second = at(Role::ListItem, "A B");
    assert_eq!(
        doc.nodes()[first].parent_index(),
        doc.nodes()[second].parent_index()
    );
    assert_eq!(
        doc.nodes()[at(Role::Link, "A")].parent_index(),
        Some(second)
    );
    assert_eq!(
        doc.nodes()[at(Role::Link, "B")].parent_index(),
        Some(second)
    );
}

#[test]
fn adoption_agency_reparents_children_and_merges_root_attributes() {
    let doc = document(
        b"<body id=first><body id=second aria-label=merged><b><p>one<i>two</b>three</i>four",
        Default::default(),
    );
    assert_eq!(doc.nodes()[0].element_id.as_deref(), Some("first"));
    assert_eq!(doc.nodes()[0].name, "merged");
    assert!(doc
        .nodes()
        .iter()
        .any(|n| n.role == Role::Paragraph && n.text == "onetwothreefour"));
}

#[test]
fn only_formatting_runs_without_source_whitespace_join() {
    let doc = document(
        b"<main><p>micro<b>scope</b> of H<sub>2</sub>O and decline<code>_code</code></p>\
<p><span>Git</span><span>Hub</span></p><p><kbd>Ctrl</kbd><kbd>K</kbd></p>\
<p>plain <em>emphasis</em>, and<em> again</em>x<i>y</i><a href='/w'>link</a>z</p>\
<p><strong>Ada Lovelace</strong>Analytical Engine</p>\
<p>line<br>break</p><ul><li><p>one</p>two</li></ul><div>block<div>inner</div>after</div>\
<table><tr><td>cell<td>next</tr></table><pre>keep  <b>as</b>\nis</pre></main>",
        Default::default(),
    );
    let texts: Vec<_> = doc.nodes().iter().map(|n| n.text.as_str()).collect();
    for expected in [
        "microscope of H2O and decline_code",
        "Git Hub",
        "Ctrl K",
        "plain emphasis, and againxy link z",
        "Ada Lovelace Analytical Engine",
        "line break",
        "one two",
        "block inner after",
        "keep  as\nis",
    ] {
        assert!(texts.contains(&expected), "{expected:?} in {texts:?}");
    }
    let cells: Vec<_> = doc
        .nodes()
        .iter()
        .filter(|n| n.role == Role::Cell)
        .map(|n| n.text.as_str())
        .collect();
    assert_eq!(cells, ["cell", "next"]);
}

#[test]
fn arena_budgets_and_presets() {
    for limits in [
        DocLimits {
            arena_elements: MAX_ARENA_ELEMENTS as u32 + 1,
            ..Default::default()
        },
        DocLimits {
            arena_text_bytes: MAX_ARENA_TEXT_BYTES as u32 + 1,
            ..Default::default()
        },
        DocLimits {
            arena_depth: MAX_ARENA_DEPTH as u32 + 1,
            ..Default::default()
        },
    ] {
        assert!(matches!(limits.validate(), Err(DocError::Limits)))
    }
    for limits in [DocLimits::RESULTS, DocLimits::SITE_PAGE, DocLimits::ARTICLE] {
        assert!(!document(b"<p>readable", limits).truncated());
    }
    let l = DocLimits {
        arena_text_bytes: 4,
        ..Default::default()
    };
    assert!(!document(b"<p>1234", l).truncated());
    let doc = document(b"<p>12345<h1>tail", l);
    assert!(doc.body_truncated());
    assert!(doc.nodes().iter().any(|n| n.text == "1234"));
    assert!(doc
        .nodes()
        .iter()
        .all(|n| n.role != Role::Heading { level: Some(1) }));
    for l in [
        DocLimits {
            arena_elements: 0,
            ..Default::default()
        },
        DocLimits {
            arena_depth: 0,
            ..Default::default()
        },
        DocLimits {
            depth: 0,
            ..Default::default()
        },
        DocLimits {
            nodes: 0,
            ..Default::default()
        },
    ] {
        assert!(document(b"<p>x", l).body_truncated());
    }
}

#[test]
fn opaque_subtrees_do_not_spend_the_text_budget() {
    for tag in ["script", "style", "template", "svg"] {
        let html = format!("<{tag}>{}</{tag}><p>end", "invisible".repeat(2000));
        let l = DocLimits {
            arena_text_bytes: 3,
            ..Default::default()
        };
        let doc = document(html.as_bytes(), l);
        assert!(!doc.truncated(), "{tag}");
        assert!(doc.nodes().iter().any(|n| n.text == "end"));
    }
}

#[test]
fn no_dom_query_runtime_dependency() {
    let mut production = false;
    for line in include_str!("../../../Cargo.toml").lines().map(str::trim) {
        if line.starts_with('[') {
            production = line.contains("dependencies")
                && !line.contains("dev-dependencies")
                && !line.contains("build-dependencies")
        }
        if production {
            assert!(!line.contains("dom_query") && !line.contains("dom-query"))
        }
    }
}

proptest::proptest! {
    #[test]
    fn bounded_projection_of_malformed_markup(
        parts in proptest::collection::vec(0usize..31, 0..300),
        arena_depth in 1u32..15, depth in 1u32..8,
    ) {
        let tokens = ["<div>", "</div>", "<template>", "</template>", "<table>", "<tr>", "<td>", "</table>",
            "<b>", "</b>", "<a href=x>", "</a>", "<p>", "<i>", "</i>", "<span>", "word", " ",
            "<select>", "<option>", "</select>", "<svg>", "</svg>", "<math>", "</math>",
            "<main>", "<section>", "<em>", "</em>", "<body>", "<button>"];
        let html = parts.into_iter().map(|i| tokens[i]).collect::<String>();
        let limits = DocLimits {arena_depth, depth, ..Default::default()};
        let doc = document(html.as_bytes(),limits);
        for (at, n) in doc.nodes().iter().enumerate() {
            proptest::prop_assert!(n.parent_index().is_none_or(|p| p < at));
            let ancestors = std::iter::successors(n.parent_index(), |&i| doc.nodes()[i].parent_index()).count();
            proptest::prop_assert!(ancestors < depth as usize);
        }
    }
}

#[test]
fn one_level_arena_keeps_visible_roles() {
    let l = DocLimits {
        arena_depth: 1,
        ..Default::default()
    };
    let doc = document(b"<p>one</p><div hidden><p>private</p></div><h2>two", l);
    assert!(doc.body_truncated());
    assert!(doc
        .nodes()
        .iter()
        .any(|n| n.role == Role::Paragraph && n.text == "one"));
    assert!(doc
        .nodes()
        .iter()
        .any(|n| n.role == Role::Heading { level: Some(2) } && n.text == "two"));
    assert!(doc.nodes().iter().all(|n| !n.text.contains("private")));
}

#[test]
fn large_inline_attributes_allow_later_content() {
    let payload = "Z".repeat(300 * 1024);
    let html = format!("<p>before</p><img src='data:image/png;base64,{payload}'><div data-state='{payload}'></div><p>after</p>");
    let doc = document(html.as_bytes(), DocLimits::RESULTS);
    assert!(!doc.truncated());
    assert!(doc.nodes().iter().any(|n| n.text == "after"));
    assert!(doc
        .nodes()
        .iter()
        .all(|n| n.image_source.is_none() && !n.text.contains('Z')));
}

#[test]
fn code_keeps_html_and_literal_line_breaks() {
    let doc = document(
        b"<pre><code># note<br>echo ready\nnext</code></pre>",
        DocLimits::default(),
    );
    assert!(doc
        .nodes()
        .iter()
        .any(|n| n.role == Role::CodeBlock && n.text == "# note\necho ready\nnext"));
}

#[test]
fn bom_takes_precedence_over_transport_and_meta() {
    let page = Url::parse("https://example.com/").unwrap();
    let bytes = "\u{feff}<meta charset=windows-1252><p>café</p>".as_bytes();
    for label in [None, Some("windows-1252")] {
        let doc = Doc::from_html(&page, bytes, label, DocLimits::default()).unwrap();
        assert!(doc.nodes().iter().any(|n| n.text == "café"));
    }
}

#[test]
fn html_meta_utf16_labels_describe_utf8() {
    for meta in [
        "<meta charset=utf-16le>",
        "<meta http-equiv=Content-Type content='text/html; charset=utf-16be'>",
    ] {
        let html = format!("{meta}<p>café</p>");
        assert!(document(html.as_bytes(), DocLimits::default())
            .nodes()
            .iter()
            .any(|n| n.text == "café"));
    }
    let page = Url::parse("https://example.com/").unwrap();
    let bytes: Vec<_> = "<p>café</p>"
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    let doc = Doc::from_html(&page, &bytes, Some("utf-16le"), DocLimits::default()).unwrap();
    assert!(doc.nodes().iter().any(|n| n.text == "café"));
}

#[test]
fn nested_code_preserves_exclusions_without_changing_ordinary_code() {
    let doc = document(
        b"<main><pre><code aria-label='AI overview'>Generated answer</code><code rel='sponsored'>Paid answer</code><code>  run()\n</code></pre><a href='/docs'>Docs<code class='sponsored'>Paid label</code></a></main>",
        DocLimits::default(),
    );
    let visible: Vec<_> = doc
        .nodes()
        .iter()
        .enumerate()
        .filter(|(at, _)| crate::visible(doc.nodes(), *at))
        .map(|(_, node)| node)
        .collect();
    assert_eq!(
        visible
            .iter()
            .find(|node| node.role == Role::CodeBlock)
            .unwrap()
            .text,
        "  run()\n"
    );
    assert_eq!(
        visible
            .iter()
            .find(|node| node.role == Role::Link)
            .unwrap()
            .text,
        "Docs"
    );
    assert!(visible
        .iter()
        .all(|node| { !node.text.contains("Generated") && !node.text.contains("Paid") }));
}

#[test]
fn arena_and_document_lifts_preserve_generated_exclusion() {
    let html = b"<main><div aria-label='AI overview'><p>Generated answer</p></div><div><p>Ordinary answer</p></div></main>";
    for limits in [
        DocLimits {
            arena_depth: 4,
            ..DocLimits::default()
        },
        DocLimits {
            depth: 3,
            ..DocLimits::default()
        },
    ] {
        let doc = document(html, limits);
        for (text, visible) in [("Generated answer", false), ("Ordinary answer", true)] {
            let at = doc
                .nodes()
                .iter()
                .position(|node| node.text == text)
                .unwrap();
            assert_eq!(crate::visible(doc.nodes(), at), visible, "{text}");
        }
    }
}

#[test]
fn arena_and_document_lifts_preserve_sponsored_exclusion() {
    let html = b"<main><div class='sponsored'><p>Paid answer</p><a href='/paid'>Paid child</a></div><div><p>Ordinary answer</p></div></main>";
    for limits in [
        DocLimits {
            arena_depth: 4,
            ..DocLimits::default()
        },
        DocLimits {
            depth: 3,
            ..DocLimits::default()
        },
    ] {
        let doc = document(html, limits);
        for (text, visible) in [
            ("Paid answer", false),
            ("Paid child", false),
            ("Ordinary answer", true),
        ] {
            let at = doc
                .nodes()
                .iter()
                .position(|node| node.text == text)
                .unwrap();
            assert_eq!(crate::visible(doc.nodes(), at), visible, "{text}");
        }
    }
}

#[test]
fn linked_answers_match_clean_text_but_never_empty_structural_nodes() {
    let doc = document(
        "<script type='application/ld+json'>{\"@type\":\"QAPage\",\"mainEntity\":{\"acceptedAnswer\":[{\"text\":\"a\u{200b}b\"},{\"text\":\"   \"}]}}</script><main><p>a\u{200b}b</p><p>Unrelated</p></main>".as_bytes(),
        DocLimits::default(),
    );
    assert!(doc
        .nodes()
        .iter()
        .any(|n| n.text == "ab" && n.marks.has(Marks::ACCEPTED)));
    assert!(doc
        .nodes()
        .iter()
        .all(|n| n.text == "ab" || !n.marks.has(Marks::ACCEPTED)));
}
