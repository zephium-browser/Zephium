use url::Url;
use zephium_map::{passage::*, *};
fn doc(path: &str, text: &str) -> Doc {
    Doc::from_html(
        &Url::parse(&format!("https://workshop.example/{path}")).unwrap(),
        text.as_bytes(),
        None,
        DocLimits::ARTICLE,
    )
    .unwrap()
}
#[test]
fn question_and_error_corpus() {
    let doc = doc("manual", include_str!("fixtures/answers.html"));
    let cases = [
        (
            "how do i reset the timer",
            "Press reset to clear the saved timer.",
        ),
        (
            "what does reset clear?",
            "Press reset to clear the saved timer.",
        ),
        (
            "what is the saved timer?",
            "Press reset to clear the saved timer.",
        ),
        (
            "how to rename a branch",
            "Rename a branch with the move command.",
        ),
        (
            "what is the move command?",
            "Rename a branch with the move command.",
        ),
        (
            "how do shipping labels expire",
            "Shipping labels expire after ten days.",
        ),
        (
            "what are shipping labels?",
            "Shipping labels expire after ten days.",
        ),
        (
            "what is offline recovery?",
            "Recovery requires the backup token.",
        ),
        (
            "what is the backup token?",
            "Recovery requires the backup token.",
        ),
        (
            "how do printed receipts work?",
            "Printed receipts use archival paper.",
        ),
        (
            "what is archival paper?",
            "Printed receipts use archival paper.",
        ),
        ("what are parcel rates?", "Parcel rates\tSeven credits"),
        (
            "error[E0106]: missing lifetime specifier",
            "fn name(s: &str) -> &str { s }",
        ),
        (
            "error[E0106]: missing lifetime specifier /src/file.rs:23",
            "fn name(s: &str) -> &str { s }",
        ),
        (
            "SyntaxError: unexpected token",
            "Remove the trailing comma.",
        ),
        (
            "TypeError: missing field 0x7fff 123",
            "value.get(\"field\")",
        ),
        ("error[E0999]: missing lifetime specifier", ""),
        ("TypeError: impossible coercion", ""),
        ("what is cobalt sapphire?", ""),
        ("what is the magnetic polar inclination?", ""),
        ("how do i do it", ""),
        ("'absent block' in /tmp/secret", ""),
    ];
    for (q, expected) in cases {
        let p = read_passages(&read_query(q).unwrap(), &[&doc]);
        assert_eq!(p.first().map_or("", |p| p.text.as_str()), expected, "{q}");
        if let Some(p) = p.first() {
            assert_eq!(p.source_title, "Workshop manual");
            assert_eq!(p.source.as_url(), doc.page());
            assert!(p.locator.element.is_some() && p.locator.quote.is_some());
            if q.contains("E0106") {
                assert!(p.code);
                assert_eq!(p.language.as_deref(), Some("rust"));
            }
        }
    }
}
#[test]
fn distinct_sources_keep_result_order_and_refuse_unfinished_blocks() {
    let q = read_query("what is a timer?").unwrap();
    let prose = format!(
        "<main><p>Timer settings persist. {}</p></main>",
        "x".repeat(MAX_PROSE_CHARS)
    );
    let a = doc("a", &prose);
    let b = doc("b", &prose);
    let c = doc("c", &prose);
    let d = doc("d", &prose);
    let passages = read_passages(&q, &[&a, &a, &b, &c, &d]);
    assert_eq!(passages.len(), MAX_PASSAGES);
    assert_eq!(
        passages
            .iter()
            .map(|p| p.source.as_url().path())
            .collect::<Vec<_>>(),
        ["/a", "/b", "/c"]
    );
    assert!(passages
        .iter()
        .all(|p| p.text == "Timer settings persist." && p.locator.quote.is_some()));
    let no = doc(
        "e",
        &format!("<main><p>Timer {}</p></main>", "x".repeat(MAX_PROSE_CHARS)),
    );
    assert!(read_passages(&q, &[&no]).is_empty());
    let code = doc(
        "e",
        &format!("<pre>timer.reset()\n{}</pre>", "x".repeat(MAX_CODE_BYTES)),
    );
    assert_eq!(read_passages(&q, &[&code])[0].text, "timer.reset()");
    let no = doc(
        "e",
        &format!("<pre>timer.reset() {}</pre>", "x".repeat(MAX_CODE_BYTES)),
    );
    assert!(read_passages(&q, &[&no]).is_empty());
}
#[test]
fn main_article_density_and_sponsored_marks() {
    let d=doc("a","<section><div><p>Timer fraud.</p></div></section><div><h2>Timer manual</h2><p>Press the large reset button on the timer.</p><div itemtype='https://schema.org/SponsoredContent'><p>Timer timer timer ad.</p></div></div>");
    let p = read_passages(&read_query("how to reset timer").unwrap(), &[&d]);
    assert_eq!(p[0].text, "Press the large reset button on the timer.");
}

#[test]
fn answer_marks_language_and_term_boundaries() {
    use zephium_map::doc::{Marks, Role, MAX_LANGUAGE_BYTES};
    let length = |n| {
        format!(
            "<pre><code class='language-{}'>  reset timer()\n</code></pre>",
            "x".repeat(n)
        )
    };
    for n in [MAX_LANGUAGE_BYTES, MAX_LANGUAGE_BYTES + 1] {
        let d = doc("a", &length(n));
        let p = read_passages(&read_query("how to reset timer").unwrap(), &[&d]);
        assert_eq!(p[0].language.is_some(), n == MAX_LANGUAGE_BYTES);
        assert_eq!(p[0].text, "  reset timer()\n");
    }
    for count in [MAX_CONTENT_TERMS, MAX_CONTENT_TERMS + 1] {
        let words = (0..count)
            .map(|i| format!("term{i}"))
            .collect::<Vec<_>>()
            .join("-");
        let d = doc("a", &format!("<p>{words}</p>"));
        assert_eq!(
            read_passages(&read_query(&format!("how to {words}")).unwrap(), &[&d]).is_empty(),
            count > MAX_CONTENT_TERMS
        );
    }
    let d=doc("a","<main><h2>Timer manual</h2><div itemprop='suggestedAnswer'><p>Manual reset operation.</p></div><p itemprop='acceptedAnswer'>Timer reset operation.</p></main>");
    assert!(d.nodes().iter().any(|n| n.marks.has(Marks::SUGGESTED)));
    assert!(d
        .nodes()
        .iter()
        .any(|n| n.role == Role::Paragraph && n.marks.has(Marks::ACCEPTED)));
    let none = doc("none", "<p>nothing relevant.</p>");
    let answer = doc("a", "<p>Timer reset operation.</p>");
    let mut pages = vec![&none; MAX_RESULT_PAGES];
    pages.push(&answer);
    assert!(read_passages(&read_query("how to reset timer").unwrap(), &pages).is_empty());
}

#[test]
fn an_adjacent_solution_heading_can_scope_the_fix() {
    let d=doc("a","<main><h2>Problem</h2><pre>TypeError: invalid bucket</pre><h2>Solution</h2><pre>bucket.reset()</pre></main>");
    let q = read_query(
        "Traceback (most recent call last):\n /tmp/project.py:14\nTypeError: invalid bucket",
    )
    .unwrap();
    assert_eq!(read_passages(&q, &[&d])[0].text, "bucket.reset()");
    let other = doc(
        "b",
        "<main><p>TypeError: invalid bucket</p><h2>Other issue</h2><pre>unrelated()</pre></main>",
    );
    assert_eq!(
        read_passages(&q, &[&other])[0].text,
        "TypeError: invalid bucket"
    );
}

#[test]
fn selectors_reanchor_with_context_and_element_fallback() {
    use zephium_map::key::{LocatorText, TextPosition, TextQuote};
    let read = |markup: &str| {
        Doc::from_html(
            &Url::parse("https://example.com/").unwrap(),
            markup.as_bytes(),
            None,
            DocLimits::ARTICLE,
        )
        .unwrap()
    };
    let markup="<main><h2 id='a'>First</h2><p>Before one</p><p>Shared timer reset.</p><h2 id='b'>Second</h2><p>Before two</p><p>Shared timer reset.</p><p>After two</p></main>";
    let original = read(markup);
    let q = read_query("how to reset timer").unwrap();
    let passage = passage::read_passages(&q, &[&original]).pop().unwrap();
    let generated = passage.locator;
    assert!(generated.element.is_some() && generated.position.is_some());
    let quote = generated.quote.as_ref().unwrap();
    assert!(quote.prefix.is_some() && quote.suffix.is_some());
    let renamed = read(&markup.replace("id='a'", "id='changed'"));
    let range = generated.resolve(&renamed).unwrap();
    assert_eq!(
        renamed.nodes()[range.start as usize].text,
        "Shared timer reset."
    );
    let reworded = read("<main><h2 id='a'>First</h2><p>New wording.</p></main>");
    let found = generated.resolve(&reworded).unwrap();
    assert_eq!(
        reworded.nodes()[found.start as usize].element_id.as_deref(),
        Some("a")
    );
    let repeated = Locator {
        quote: Some(TextQuote {
            prefix: Some(LocatorText::new("Before two").unwrap()),
            start: LocatorText::new("Shared timer reset.").unwrap(),
            end: None,
            suffix: None,
        }),
        ..Default::default()
    };
    let selected = repeated.resolve(&original).unwrap().start as usize;
    assert_eq!(original.nodes()[selected - 1].text, "Before two");
    assert!(repeated
        .resolve(&read(
            "<main><p>Before one</p><p>Shared timer reset.</p></main>"
        ))
        .is_none());
    assert!(generated
        .resolve(&read("<main><h2>Archive</h2><p>Another topic.</p></main>"))
        .is_none());
    assert!(Locator {
        position: Some(TextPosition { start: 0, end: 1 }),
        ..Default::default()
    }
    .resolve(&original)
    .is_none());
}

#[test]
fn fragment_encoding_and_unknown_selector_fields() {
    use zephium_map::key::LocatorText;
    let locator: Locator =
        serde_json::from_str(r#"{"element":"standard-pricing.v2_é","future":{"enabled":true}}"#)
            .unwrap();
    assert_eq!(
        locator.fragment().as_deref(),
        Some("#standard-pricing.v2_%C3%A9")
    );
    for route in ["/inbox", "!/x"] {
        let loc = Locator {
            element: Some(LocatorText::new(route).unwrap()),
            ..Default::default()
        };
        assert_eq!(loc.fragment(), Some(format!("#{route}")));
    }
    let quote: Locator = serde_json::from_str(
        r#"{"quote":{"prefix":"pre-","start":"a,b","end":"x y","suffix":"é"}}"#,
    )
    .unwrap();
    assert_eq!(
        quote.fragment().as_deref(),
        Some("#:~:text=pre%2D-,a%2Cb,x%20y,-%C3%A9")
    );
}
