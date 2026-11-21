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
        .any(|n| n.role == Role::Heading(Some(2)) && n.text == "Final heading"));
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
        crate::ResultsRead::Wall(crate::Wall::Unreadable)
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
        .find(|n| n.role == Role::Heading(Some(2)) && n.text == "deep")
        .unwrap();
    let ancestors = std::iter::successors(heading.parent, |&i| doc.nodes()[i].parent).count();
    assert_eq!(ancestors, 3);
    assert!(doc
        .nodes()
        .iter()
        .any(|n| n.role == Role::Paragraph && n.text == "shallow" && n.parent == Some(0)));
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
    assert_eq!(doc.nodes()[one].parent, doc.nodes()[two].parent);
    assert_eq!(doc.nodes()[fostered].parent, doc.nodes()[table].parent);
    let cell = at(Role::Cell, "cell");
    assert_eq!(
        doc.nodes()[doc.nodes()[cell].parent.unwrap()].role,
        Role::Row
    );
    let first = at(Role::ListItem, "first");
    let second = at(Role::ListItem, "A B");
    assert_eq!(doc.nodes()[first].parent, doc.nodes()[second].parent);
    assert_eq!(doc.nodes()[at(Role::Link, "A")].parent, Some(second));
    assert_eq!(doc.nodes()[at(Role::Link, "B")].parent, Some(second));
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
        .any(|n| n.role == Role::Paragraph && n.text == "one two three four"));
}

#[test]
fn arena_budgets_and_presets() {
    for limits in [
        DocLimits {
            arena_elements: MAX_ARENA_ELEMENTS + 1,
            ..Default::default()
        },
        DocLimits {
            arena_text_bytes: MAX_ARENA_TEXT_BYTES + 1,
            ..Default::default()
        },
        DocLimits {
            arena_depth: MAX_ARENA_DEPTH + 1,
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
    assert!(doc.nodes().iter().all(|n| n.role != Role::Heading(Some(1))));
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
        arena_depth in 1usize..15, depth in 1usize..8,
    ) {
        let tokens = ["<div>", "</div>", "<template>", "</template>", "<table>", "<tr>", "<td>", "</table>",
            "<b>", "</b>", "<a href=x>", "</a>", "<p>", "<i>", "</i>", "<span>", "word", " ",
            "<select>", "<option>", "</select>", "<svg>", "</svg>", "<math>", "</math>",
            "<main>", "<section>", "<em>", "</em>", "<body>", "<button>"];
        let html = parts.into_iter().map(|i| tokens[i]).collect::<String>();
        let limits = DocLimits {arena_depth, depth, ..Default::default()};
        let doc = document(html.as_bytes(),limits);
        for (at, n) in doc.nodes().iter().enumerate() {
            proptest::prop_assert!(n.parent.is_none_or(|p| p < at));
            let ancestors = std::iter::successors(n.parent, |&i| doc.nodes()[i].parent).count();
            proptest::prop_assert!(ancestors < depth);
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
        .any(|n| n.role == Role::Heading(Some(2)) && n.text == "two"));
    assert!(doc.nodes().iter().all(|n| !n.text.contains("private")));
}
