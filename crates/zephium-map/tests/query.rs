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
        ("list", &["cheap shoes near me"]),
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
                QueryKind::List => "list",
                QueryKind::Instant => "instant",
                QueryKind::Place { .. } if read.confidence() == Confidence::Low => "uncertain",
                QueryKind::Place { .. } => "place",
                QueryKind::Passage { kind: Question } => "question",
                QueryKind::Passage { kind: Pasted } => "pasted",
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
        let QueryKind::Place {
            entity, section, ..
        } = q.kind()
        else {
            panic!()
        };
        assert_eq!(entity, &["github", "actions"]);
        assert_eq!(*section, Some(Section { role }));
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

#[test]
fn instant_listing_fresh_and_entity_boundaries() {
    let groups: &[(&str, &[&str])] = &[
        (
            "instant",
            &[
                "san francisco time",
                "time in cape town",
                "weather london",
                "weather in athens",
                "tokyo weather",
                "utc time",
                "100 usd to eur",
                "42 eur in pln",
                "5 miles in km",
                "1 kg to lb",
                "15% of 240",
                "5% of 12",
                "12 * 7",
                "(2+3)*4",
                "translate thank you to japanese",
                "translate hello to french",
                "define ephemeral",
                "define stack frame",
                "timer 10 minutes",
                "30 seconds timer",
            ],
        ),
        (
            "list",
            &[
                "best laptop for programming",
                "best noise cancelling headphones",
                "noise cancelling headphones",
                "pizza near me",
                "coffee near me",
                "tech news today",
                "flights to tokyo",
                "cheap flights to london",
                "hotels in lisbon",
                "easy dinner recipes",
                "tomato soup recipes",
                "analytics icon",
                "calendar icon",
                "remote rust jobs",
                "part time design jobs",
                "recipes for dinner",
            ],
        ),
        (
            "fresh",
            &[
                "new openai model",
                "latest claude model",
                "chrome latest version",
                "svelte 5 whats new",
                "safari release notes",
                "webkit news",
                "tauri announcements",
                "raycast latest",
                "firefox releases",
                "stripe announcement",
            ],
        ),
        (
            "place",
            &[
                "zoom download",
                "github status",
                "weather.com",
                "weather underground",
                "time magazine",
                "best buy",
                "timer app docs",
                "google translate",
                "google flights",
                "new york times",
                "steve jobs",
                "hilton hotels",
                "spotify wrapped",
                "weatherkit api",
                "linear integrations",
            ],
        ),
    ];
    for (group, cases) in groups {
        for &query in *cases {
            let q = read_query(query).unwrap();
            let actual = match q.kind() {
                QueryKind::Place { fresh: true, .. } => "fresh",
                QueryKind::Place { .. } => "place",
                QueryKind::List => "list",
                QueryKind::Instant => "instant",
                _ => "other",
            };
            assert_eq!(actual, *group, "{query}");
        }
    }
}

#[test]
fn task_fact_section_and_name_boundaries() {
    let questions = [
        "git rename branch",
        "gmail smtp port",
        "difference between 401 and 403",
        "python create virtual environment",
        "docker remove all images",
        "javascript deep clone object",
        "postgres list all tables",
        "rust convert string to integer",
        "generate ssh key ed25519",
        "install docker on ubuntu",
        "npm install package",
        "download rust source",
        "netflix login failed",
        "delete postgres database",
        "git undo commit",
        "git reset branch",
        "enable wireguard",
        "disable windows search",
        "change node version",
        "set python path",
        "get git revision",
        "find sqlite schema",
        "fix cargo lock",
        "update npm packages",
        "git merge branches",
        "git revert commit",
        "cargo command",
        "editor shortcut",
        "lua syntax",
        "sql join example",
        "clang optimization flag",
        "nginx default timeout",
        "redis memory limit",
        "lua version number",
        "grpc status code",
        "curl error code",
        "mutex meaning",
        "authentication vs authorization",
        "tcp vs udp",
        "elixir how to start",
        "401 vs 403",
        "404 versus 410",
        "how to change directory",
        "how do i install a font",
        "what is a socket",
        "remove docker images",
        "rust download sources",
        "python install pip",
        "login docker registry",
    ];
    for q in questions {
        assert!(
            matches!(
                read_query(q).unwrap().kind(),
                QueryKind::Passage {
                    kind: PassageKind::Question
                }
            ),
            "{q}"
        );
    }
    use SectionRole::*;
    for (q, role) in [
        ("stripe pricing", Pricing),
        ("github docs", Documentation),
        ("vscode download", Download),
        ("netflix login", Login),
        ("tauri changelog", Changelog),
        ("python download", Download),
        ("rust book", Documentation),
        ("docker hub", Product),
        ("openai api", Api),
        ("package install", Install),
        ("project sign in", Login),
        ("engine reference", Reference),
        ("editor support", Support),
        ("forge releases", Releases),
        ("forge latest", News),
        ("sdk examples", Examples),
        ("cargo blog", Blog),
        ("rust community", Community),
        ("sqlite documentation", Documentation),
        ("sql tutorials", Learn),
        ("cargo features", Features),
        ("editor downloads", Download),
        ("database status", Status),
        ("service careers", Careers),
    ] {
        assert!(
            matches!(read_query(q).unwrap().kind(), QueryKind::Place { section: Some(s), .. } if s.role == role),
            "{q}"
        );
    }
    for q in [
        "docker",
        "rust",
        "python",
        "javascript",
        "go",
        "clojure",
        "vscode",
        "ssh",
        "git",
        "gmail",
        "sqlite",
        "web kit",
        "install",
        "download",
        "login",
    ] {
        assert!(
            matches!(
                read_query(q).unwrap().kind(),
                QueryKind::Place { section: None, .. }
            ),
            "{q}"
        );
    }
    for q in [
        "git vs hg",
        "docker vs podman",
        "sqlite versus mysql",
        "signal or telegram",
    ] {
        assert!(
            matches!(read_query(q).unwrap().kind(), QueryKind::Compare { .. }),
            "{q}"
        );
    }
}

#[test]
fn action_words_need_a_following_term() {
    for (place, question) in [
        ("windows update", "update npm packages"),
        ("lego set", "set timezone"),
        ("forbes list", "list remote branches"),
        ("password reset", "git reset branch"),
        ("wireguard enable", "enable wireguard"),
        ("account disable", "disable notifications"),
        ("github clone", "clone github repository"),
        ("project rename", "rename workspace"),
        ("archive remove", "remove branch"),
        ("account delete", "delete account"),
        ("batch convert", "convert audio files"),
        ("key generate", "generate recovery codes"),
        ("editor undo", "undo text changes"),
        ("address change", "change billing address"),
        ("artifact get", "get package metadata"),
        ("finder find", "find tracked files"),
        ("quick fix", "fix permissions"),
        ("mail merge", "merge document templates"),
        ("commit revert", "revert last commit"),
        ("project create", "create release tag"),
        ("offline install", "install rust toolchain"),
        ("manual download", "download release archive"),
        ("member login", "login member portal"),
    ] {
        assert!(
            matches!(read_query(place).unwrap().kind(), QueryKind::Place { .. }),
            "{place}"
        );
        assert!(
            matches!(
                read_query(question).unwrap().kind(),
                QueryKind::Passage {
                    kind: PassageKind::Question
                }
            ),
            "{question}"
        );
    }
}

#[test]
fn arithmetic_division_and_file_paths_are_distinct() {
    for query in ["10 /2", "10 / 2", "10/2"] {
        assert!(matches!(
            read_query(query).unwrap().kind(),
            QueryKind::Instant
        ));
    }
    for query in ["/usr/bin", "/2"] {
        assert!(matches!(
            read_query(query).unwrap().kind(),
            QueryKind::Passage {
                kind: PassageKind::Pasted
            }
        ));
    }
}

#[test]
fn freshness_removes_qualifiers_without_removing_new_from_names() {
    assert!(matches!(
        read_query("new york transit news").unwrap().kind(),
        QueryKind::Place { entity, fresh: true, .. } if entity == &["new", "york", "transit"]
    ));
    assert!(matches!(
        read_query("new openai model").unwrap().kind(),
        QueryKind::Place { entity, fresh: true, .. } if entity == &["openai", "model"]
    ));
}
