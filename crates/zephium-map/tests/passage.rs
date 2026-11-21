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
    let d=doc("a","<main><h2>Problem</h2><pre>TypeError: invalid bucket</pre><h3>Solution</h3><pre>bucket.reset()</pre></main>");
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

#[test]
fn article_lead_excludes_editorial_furniture_and_citations() {
    let page = doc("a", "<title>Sound waves - Reference</title><main><h1>Sound waves</h1><div class='hatnote'>Main article: Sound waves.</div><figure><figcaption>Sound waves in air.</figcaption></figure><p>Sound waves propagate through matter.<sup class='reference'>[1]</sup></p><h2>See also</h2><ul><li>Sound waves and music.</li></ul></main>");
    for q in ["what is sound waves", "sound waves meaning"] {
        let answers = read_passages(&read_query(q).unwrap(), &[&page]);
        assert_eq!(answers[0].text, "Sound waves propagate through matter.");
    }
}

#[test]
fn canonical_error_pairs_the_lead_with_the_first_example() {
    for title in [
        "<title>E2071 - Compiler guide</title><main>",
        "<main><h1>Error code E2071</h1>",
    ] {
        let page = doc("errors", &format!("{title}<p>The destination has already been moved.</p><p>Example:</p><pre><code class='language-rust'>take(x);\nuse_it(x);</code></pre><pre>later()</pre></main>"));
        let answers = read_passages(
            &read_query("error[E2071]: cannot use moved data").unwrap(),
            &[&page],
        );
        assert_eq!(
            answers[0].text,
            "The destination has already been moved.\ntake(x);\nuse_it(x);"
        );
        assert!(answers[0].code && answers[0].locator.resolve(&page).is_some());
    }
    let page = doc(
        "name",
        "<title>BucketError</title><main><p>The bucket is empty.</p><pre>fill(bucket)</pre></main>",
    );
    assert_eq!(
        read_passages(&read_query("BucketError: missing item").unwrap(), &[&page])[0].text,
        "The bucket is empty.\nfill(bucket)"
    );
}

#[test]
fn command_stays_with_its_introduction_and_heading() {
    let page = doc("guide","<main><h2>Rename a branch</h2><p>To rename a branch, run:</p><pre>tool move old new</pre><h2>Delete</h2><pre>tool delete old</pre></main>");
    assert_eq!(
        read_passages(&read_query("how to rename a branch").unwrap(), &[&page])[0].text,
        "To rename a branch, run:\ntool move old new"
    );
}

#[test]
fn numbered_subject_is_required_in_the_opening_or_heading() {
    let q = read_query("what is http status 472").unwrap();
    for (pad, expected) in [
        (SUBJECT_START_WORDS - 1, true),
        (SUBJECT_START_WORDS, false),
    ] {
        let page = doc(
            "codes",
            &format!(
                "<main><h1>HTTP status</h1><p>{}472 is a test status.</p></main>",
                "word ".repeat(pad)
            ),
        );
        assert_eq!(!read_passages(&q, &[&page]).is_empty(), expected);
    }
    let page = doc("other","<main><h2>HTTP status 473</h2><p>HTTP status 473 refers to an unrelated condition.</p></main>");
    assert!(read_passages(&q, &[&page]).is_empty());
}

#[test]
fn numbered_reference_prefers_lead_to_specification_and_incidental_mentions() {
    let page = doc("ref","<title>472 Reserved - Protocol</title><main><h1>472 Reserved</h1><p>The protocol <code>472</code> status indicates a reserved value.</p><h2>Specifications</h2><p>Protocol status #472</p></main>");
    let other = doc("history","<main><h1>History of networking</h1><p>In a recent update protocol releases included 472 as a new status.</p></main>");
    let answers = read_passages(
        &read_query("what is protocol status 472").unwrap(),
        &[&page, &other],
    );
    assert_eq!(answers.len(), 1);
    assert_eq!(
        answers[0].text,
        "The protocol 472 status indicates a reserved value."
    );
}

#[test]
fn an_unmarked_contents_list_cannot_answer_a_how_to() {
    let page = doc("manual", "<main><h1>How to use a timer</h1><ul><li><p><a href='#use'>Use a timer</a></p></li></ul><h2 id='use'>Use a timer</h2><p>Use the dial on the timer to select a duration.</p></main>");
    assert_eq!(
        read_passages(&read_query("how to use a timer").unwrap(), &[&page])[0].text,
        "Use the dial on the timer to select a duration."
    );
}

#[test]
fn task_prefers_the_first_command_in_the_matching_heading_path() {
    let page = doc("guide", "<title>Loom handbook</title><main><h1>Workshop guide</h1>
      <p>Loom can create a spool of yarn.</p>
      <h2>Creating spools</h2><h3>From a sample</h3>
      <p><a class='reference internal' href='#spool'><em>Run</em></a> the <code>loom</code> command:</p>
      <pre>loom weave sample</pre><p>Inspect the result:</p><pre>loom inspect spool</pre>
      <h2>Cleaning</h2><pre>loom clean</pre></main>");
    let answers = read_passages(&read_query("loom create spool").unwrap(), &[&page]);
    assert_eq!(answers[0].text, "Run the loom command:\nloom weave sample");
    assert!(answers[0].locator.resolve(&page).is_some());
}

#[test]
fn a_command_and_its_introduction_can_supply_different_subject_terms() {
    let page = doc(
        "guide",
        "<main><h2>Utilities</h2><p>To generate a ceramic sample, run:</p>
        <pre>press --porcelain</pre><h2>Another task</h2><pre>press --earthenware</pre></main>",
    );
    let read = |q| read_passages(&read_query(q).unwrap(), &[&page]);
    assert_eq!(
        read("generate porcelain sample")[0].text,
        "To generate a ceramic sample, run:\npress --porcelain"
    );
    assert!(read("generate earthenware sample").is_empty());
}

#[test]
fn port_values_keep_their_heading_and_whole_row_or_item() {
    let q = read_query("loom relay port").unwrap();
    let page = doc(
        "guide",
        "<title>Loom</title><main><h2>Spool relay</h2><h3>Settings</h3>
        <ul><li>Port 9955 or 9956</li></ul></main>",
    );
    assert_eq!(
        read_passages(&q, &[&page])[0].text,
        "Spool relay\nPort 9955 or 9956"
    );
    let page = doc("guide", "<title>Loom</title><main><h2>Relay</h2>
        <table><tr><th>Setting</th><th>Value</th></tr><tr><td><p>Port</p></td><td><code>4455</code></td></tr></table></main>");
    assert_eq!(
        read_passages(&q, &[&page])[0].text,
        "Setting\tValue\nPort\t4455"
    );
}

#[test]
fn a_reference_returns_the_matching_option_and_its_own_description() {
    let q = read_query("loom rename spool").unwrap();
    let page = doc("reference", "<title>Loom reference</title><div><div><p>Dense unrelated prose describing a different subject.</p></div>
        <dl><dt>--drop</dt><dd>Remove a spool.</dd><dt>-r</dt><dt>--retitle</dt><dd><p>Renames a spool, keeping its color.</p></dd></dl></div>");
    assert_eq!(
        read_passages(&q, &[&page])[0].text,
        "-r --retitle\nRenames a spool, keeping its color."
    );
    let page = doc(
        "reference",
        "<title>Loom reference</title><main><h2>--retitle</h2><p>Renames a spool.</p></main>",
    );
    assert_eq!(
        read_passages(&q, &[&page])[0].text,
        "--retitle\nRenames a spool."
    );
}

#[test]
fn comparison_requires_evidence_for_both_sides() {
    let q = read_query("difference between 471 and 472").unwrap();
    let a = doc(
        "a",
        "<title>471 Empty</title><main><p>Code 471 means the box is empty.</p></main>",
    );
    let b = doc(
        "b",
        "<main><h1>472 Full</h1><p>Code 472 means the box is full.</p></main>",
    );
    assert!(read_passages(&q, &[&a, &a]).is_empty());
    let pair = read_passages(&q, &[&b, &a]);
    assert_eq!(
        pair.iter().map(|p| p.text.as_str()).collect::<Vec<_>>(),
        [
            "Code 471 means the box is empty.",
            "Code 472 means the box is full."
        ]
    );
    let both = doc(
        "c",
        "<main><p>Code 471 means empty, while 472 means full.</p></main>",
    );
    assert_eq!(
        read_passages(&q, &[&a, &both])[0].text,
        "Code 471 means empty, while 472 means full."
    );
}

#[test]
fn narrower_examples_do_not_answer_unqualified_tasks() {
    let page = doc("guide", "<title>Loom guide</title><main><h2>Examples</h2><p>Example output:</p>
        <pre>$ loom purge\nThis will remove all spools without thread.</pre>
        <dl><dt>Undo winding permanently</dt><dd>Undo the last three winding operations.</dd></dl></main>");
    for q in ["loom remove all spools", "how to undo last winding"] {
        assert!(read_passages(&read_query(q).unwrap(), &[&page]).is_empty());
    }
}

#[test]
fn structured_values_are_refused_whole_when_they_exceed_the_prose_limit() {
    let query = read_query("loom port").unwrap();
    for extra in [0, 1] {
        let value = format!(
            "Port 123. {}",
            "v".repeat(MAX_PROSE_CHARS - "Port 123. ".len() + extra)
        );
        let page = doc(
            "ports",
            &format!("<title>Loom</title><main><ul><li>{value}</li></ul></main>"),
        );
        assert_eq!(!read_passages(&query, &[&page]).is_empty(), extra == 0);
    }
}

#[test]
fn a_platform_in_the_title_supplies_context_but_cannot_supply_the_task() {
    let page = doc("screen", "<title>Print a stencil on Loom</title><main><h2>Using paper</h2><p>To print a stencil, press the green key.</p></main>");
    assert_eq!(
        read_passages(
            &read_query("how to print a stencil on loom").unwrap(),
            &[&page]
        )[0]
        .text,
        "To print a stencil, press the green key."
    );
    assert!(read_passages(
        &read_query("how to fold a stencil on loom").unwrap(),
        &[&page]
    )
    .is_empty());
}

#[test]
fn plural_query_matches_a_singular_subject_and_gerund_heading() {
    let page = doc("guide", "<title>Atlas</title><main><h2>Renaming a branch</h2><p>Run this command:</p><pre>atlas mv old new</pre></main>");
    assert_eq!(
        read_passages(&read_query("atlas rename branches").unwrap(), &[&page])[0].text,
        "Run this command:\natlas mv old new"
    );
}

#[test]
fn textual_list_values_keep_inline_content_and_the_subject_heading() {
    let page = doc("guide","<title>Loom settings</title><main><h2>Storage</h2><h3>Settings</h3>
        <ul><li><p><a class='reference external' href='/modes'><code>Default</code></a> mode</p><em>Read only</em></li></ul></main>");
    let answers = read_passages(&read_query("loom storage default").unwrap(), &[&page]);
    assert_eq!(answers[0].text, "Storage\nDefault mode Read only");
}

#[test]
fn accepted_answers_do_not_cross_unrelated_issue_headings() {
    let query = read_query("TypeError: invalid bucket").unwrap();
    for heading in ["Printer setup", "Printer solution"] {
        let page = doc("issues", &format!("<main><h2>Bucket failure</h2><pre>TypeError: invalid bucket</pre><h2>{heading}</h2><div itemprop='acceptedAnswer'><pre>printer.reset()</pre></div></main>"));
        assert_eq!(
            read_passages(&query, &[&page])[0].text,
            "TypeError: invalid bucket"
        );
    }
    let page = doc("issues", "<main><div><h2>Bucket failure</h2><pre>TypeError: invalid bucket</pre></div><div><h3>Printer solution</h3><pre>printer.reset()</pre></div></main>");
    assert_eq!(
        read_passages(&query, &[&page])[0].text,
        "TypeError: invalid bucket"
    );
}

#[test]
fn a_last_task_noun_may_appear_in_the_title() {
    let page = doc(
        "guide",
        "<title>Undo a commit in Git</title><p>To undo the last commit, use git revert.</p>",
    );
    let query = read_query("how to undo last commit in git").unwrap();
    assert_eq!(
        read_passages(&query, &[&page])
            .first()
            .map(|p| p.text.as_str()),
        Some("To undo the last commit, use git revert.")
    );
}

#[test]
fn timing_clause_can_follow_an_unrestricted_task_quantity() {
    let page = doc(
        "guide",
        "<title>Loom guide</title><p>Remove all spools before restarting.</p>",
    );
    for text in ["loom remove all spools", "how to remove all spools?"] {
        let q = read_query(text).unwrap();
        assert_eq!(
            read_passages(&q, &[&page]).first().map(|p| p.text.as_str()),
            Some("Remove all spools before restarting.")
        );
    }
}

#[test]
fn comparison_skips_an_alias_to_find_a_distinct_second_source() {
    let a = doc(
        "codes",
        "<h1>471 Empty</h1><p>Code 471 means the box is empty.</p>",
    );
    let alias = doc(
        "codes/",
        "<h1>472 Full</h1><p>Code 472 means the box is full.</p>",
    );
    let b = doc(
        "full",
        "<h1>472 Full</h1><p>Code 472 means the box is full.</p>",
    );
    let q = read_query("difference between 471 and 472").unwrap();
    let answer = read_passages(&q, &[&a, &alias, &b]);
    assert_eq!(answer.len(), 2);
    assert_eq!(answer[1].source.as_url().path(), "/full");
}

#[test]
fn invisible_heading_does_not_end_the_visible_anchor_scope() {
    use zephium_map::{
        doc::{Marks, Role},
        key::{LocatorText, TextQuote},
    };
    let url = Url::parse("https://example.com/").unwrap();
    let mut b = DocBuilder::new(&url, DocLimits::ARTICLE).unwrap();
    let main = b.push(Node::new(Role::Group, None)).unwrap();
    let mut heading = Node::new(Role::Heading { level: Some(2) }, Some(main));
    heading.element_id = Some("first".into());
    b.push(heading).unwrap();
    let mut hidden = Node::new(Role::Heading { level: Some(2) }, Some(main));
    hidden.marks.insert(Marks::GENERATED);
    b.push(hidden).unwrap();
    let mut p = Node::new(Role::Paragraph, Some(main));
    p.text = "Repeated phrase".into();
    let first = b.push(p).unwrap();
    b.push(Node::new(Role::Heading { level: Some(2) }, Some(main)))
        .unwrap();
    let mut p = Node::new(Role::Paragraph, Some(main));
    p.text = "Repeated phrase".into();
    b.push(p).unwrap();
    let locator = Locator {
        element: Some(LocatorText::new("first").unwrap()),
        quote: Some(TextQuote {
            start: LocatorText::new("Repeated phrase").unwrap(),
            prefix: None,
            suffix: None,
            end: None,
        }),
        position: None,
    };
    assert_eq!(locator.resolve(&b.finish()), Some(first..first + 1));
}
