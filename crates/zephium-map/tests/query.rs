use zephium_map::query::*;

#[test]
fn query_corpus() {
    use PassageKind::*;
    let cases = [
        (
            "place",
            &[
                "stripe",
                "tauri",
                "mozilla",
                "github",
                "postgres",
                "OpenAI",
                "node.js",
                "c++",
                "c#",
                "stripe pricing",
                "github actions docs",
                "tauri changelog",
                "redis documentation",
                "blender download",
                "npm install",
                "docker login",
                "atlassian sign in",
                "stripe api",
                "rust reference",
                "vercel status",
                "slack support",
                "figma help",
                "linear blog",
                "github releases",
                "rust release notes",
                "mozilla careers",
                "signal about",
                "Zoom downloads",
            ][..],
        ),
        (
            "uncertain",
            &[
                "how",
                "vs",
                "or stripe",
                "stripe versus",
                "me or you",
                "the best library",
                "something to do",
                "cheap shoes near me",
                "amazon and ebay",
                "find a way to learn rust",
                "an or the",
                "ich möchte hilfe",
                "日本語",
                "github vs gitlab vs forgejo",
                "stripe or what",
                "what if it rains",
                "pricing for students",
                "Jane Doe",
                "blue green",
            ],
        ),
        (
            "question",
            &[
                "how to build rust",
                "how do i sign in",
                "how does git work",
                "how can i pay",
                "what is stripe",
                "what are slots",
                "what does mmap mean",
                "why is this slow",
                "why does rust borrow",
                "where is paris",
                "when is rustconf",
                "who is ada",
                "can i install it",
                "Rust?",
                "vercel vs netlify?",
                "is this supported?",
            ],
        ),
        (
            "pasted",
            &[
                "error[E0382]: borrow of moved value",
                "E0382",
                "E0123: moved",
                "TypeError: cannot read property",
                "ReferenceError: missing",
                "SyntaxError: unexpected token",
                "NullPointerException: value",
                "Traceback (most recent call last):",
                "thread 'main' panicked at src/main.rs:2",
                "/usr/bin/env",
                "./scripts/build",
                "../target/debug",
                "C:\\Users\\example\\main.rs",
                "src/lib.rs:24:8",
                "quoted \"invalid value\"",
                "'missing field' in [entry:1]",
                "expected `name` in data",
                "“invalid argument”",
                "error\n    at run (app.js:2:4)",
                "[parser::read(data[slot]): unexpected(',') at (42:1)]",
            ],
        ),
        (
            "compare",
            &[
                "vercel vs netlify pricing",
                "stripe versus paypal",
                "react or svelte",
                "rust vs. go",
                "GitHub VS GitLab",
                "apple music vs spotify",
                "docker versus podman docs",
                "postgres vs mysql release notes",
                "C++ or Rust",
                "figma versus sketch",
                "linear or jira",
                "typescript vs javascript",
                "AWS vs Azure",
                "Tauri or Electron",
                "Signal versus WhatsApp",
            ],
        ),
    ];
    assert!(cases.iter().map(|(_, v)| v.len()).sum::<usize>() >= 80);
    for (class, queries) in cases {
        for &input in queries {
            let read = read_query(input).unwrap();
            let actual = match read.kind() {
                QueryKind::Place { .. } if read.confidence() == Confidence::Low => "uncertain",
                QueryKind::Place { .. } => "place",
                QueryKind::Passage(Question) => "question",
                QueryKind::Passage(Pasted) => "pasted",
                QueryKind::Compare { .. } => "compare",
            };
            assert_eq!(actual, class, "{input}");
        }
    }
}

#[test]
fn carries_normalized_terms_and_sections() {
    let read = read_query("  Vercel   VS Netlify  pricing ").unwrap();
    assert_eq!(read.terms(), ["vercel", "vs", "netlify", "pricing"]);
    let QueryKind::Compare {
        left,
        right,
        shared,
    } = read.kind()
    else {
        panic!()
    };
    assert_eq!(left, &["vercel"]);
    assert_eq!(right, &["netlify"]);
    assert_eq!(shared.unwrap().role, SectionRole::Pricing);
    for &(word, role) in SECTION_WORDS {
        let q = read_query(&format!("GitHub Actions {word}")).unwrap();
        let QueryKind::Place { entity, section } = q.kind() else {
            panic!()
        };
        assert_eq!(entity, &["github", "actions"]);
        assert_eq!(*section, Some(Section { word, role }));
    }
}
#[test]
fn query_budgets() {
    assert!(read_query(&"x".repeat(MAX_QUERY_BYTES)).is_ok());
    assert!(matches!(
        read_query(&"x".repeat(MAX_QUERY_BYTES + 1)),
        Err(QueryError::Bytes)
    ));
    assert!(read_query(&"x ".repeat(MAX_QUERY_TERMS)).is_ok());
    assert!(matches!(
        read_query(&"x ".repeat(MAX_QUERY_TERMS + 1)),
        Err(QueryError::Terms)
    ));
    assert!(matches!(read_query(" \t\n"), Err(QueryError::Empty)));
}
