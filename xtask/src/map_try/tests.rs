use super::*;
use zephium_map::{read_results, Doc, DocLimits, ResultsRead};

fn args(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

#[test]
fn accepts_one_or_two_public_sites_in_order_and_passes_private_through() {
    let arguments = args(&["Stripe Pricing", "--site", "Stripe.COM.", "--private"]);
    let (request, _) = Options::parse(&arguments).unwrap().prepare().unwrap();
    assert_eq!(request.query, "Stripe Pricing");
    assert!(request.private);
    let arguments = args(&[
        "Figma vs Sketch",
        "--site",
        "figma.com",
        "--site",
        "sketch.com",
    ]);
    let options = Options::parse(&arguments).unwrap();
    assert_eq!(options.sites[0].as_str(), "https://figma.com/");
    assert_eq!(options.sites[1].as_str(), "https://sketch.com/");
    assert!(!options.prepare().unwrap().0.private);
    assert_eq!(
        site_url("bücher.de").unwrap().host_str(),
        Some("xn--bcher-kva.de")
    );
    assert_eq!(site_url("stripe.com.").unwrap().path(), "/");
}

#[test]
fn accepts_repeated_open_labels_and_enforces_their_bounds() {
    let mut arguments = args(&[
        "Stripe",
        "--site",
        "stripe.com",
        "--open",
        "Docs",
        "--open",
        "Pricing",
    ]);
    assert_eq!(
        Options::parse(&arguments).unwrap().opens,
        ["Docs", "Pricing"]
    );
    arguments.truncate(3);
    for _ in 0..MAX_OPEN_LABELS {
        arguments.extend(args(&["--open", "Docs"]));
    }
    assert!(Options::parse(&arguments).is_ok());
    arguments.extend(args(&["--open", "Docs"]));
    assert_eq!(Options::parse(&arguments).err(), Some(Error::BadOpenLabel));
    arguments.truncate(4);
    for label in [
        "".to_owned(),
        "Docs\nPricing".to_owned(),
        "x".repeat(MAX_TEXT_BYTES + 1),
    ] {
        arguments.push(label);
        assert_eq!(Options::parse(&arguments).err(), Some(Error::BadOpenLabel));
        arguments.pop();
    }
    assert_eq!(Options::parse(&arguments).err(), Some(Error::BadOpenLabel));
}

#[test]
fn refuses_malformed_options_and_arity() {
    for (raw, error) in [
        (&[][..], Error::MissingQuery),
        (&["--site", "stripe.com"], Error::MissingQuery),
        (&["Stripe"], Error::MissingSite),
        (&["Stripe", "--site"], Error::MissingSite),
        (&["Stripe", "--site", "--private"], Error::MissingSite),
        (
            &["Stripe", "--site", "stripe.com", "--help"],
            Error::UnexpectedOption,
        ),
        (
            &["Stripe", "--site", "stripe.com", "--private", "--private"],
            Error::UnexpectedOption,
        ),
        (
            &["Figma", "vs", "Sketch", "--site", "figma.com"],
            Error::UnexpectedOption,
        ),
        (
            &["Stripe", "--site", "stripe.com", "--site", "figma.com"],
            Error::PlaceArity,
        ),
        (
            &["Figma vs Sketch", "--site", "figma.com"],
            Error::CompareArity,
        ),
        (
            &[
                "A vs B",
                "--site",
                "stripe.com",
                "--site",
                "figma.com",
                "--site",
                "sketch.com",
            ],
            Error::TooManySites,
        ),
        (
            &["2 + 2", "--site", "stripe.com"],
            Error::Refused(StartRefusal::NotAMap),
        ),
        (
            &["", "--site", "stripe.com"],
            Error::Refused(StartRefusal::Invalid),
        ),
    ] {
        let arguments = args(raw);
        assert_eq!(
            Options::parse(&arguments).and_then(Options::prepare).err(),
            Some(error),
            "{raw:?}"
        );
    }
}

#[test]
fn rejects_each_non_public_or_non_bare_host_shape() {
    for host in [
        "",
        "https://stripe.com",
        "stripe.com/",
        "stripe.com/pricing",
        "stripe.com:443",
        "stripe.com:8443",
        "user@stripe.com",
        "user:password@stripe.com",
        "stripe.com?x",
        "stripe.com#here",
        "stripe.com\\",
        " stripe.com",
        "stripe.com\n",
        "stri\tpe.com",
        "127.0.0.1",
        "8.8.8.8",
        "0x08080808",
        "[2001:4860:4860::8888]",
        "localhost",
        "test.localhost",
        "test.local",
        "test.internal",
        "stripe",
        "stripe.invalid",
        "test.onion",
        "test.arpa",
        "test.alt",
        "com",
        "co.uk",
        "stripe%2ecom",
        "stripe..com",
        "stripe.com..",
        "-stripe.com",
        "stripe-.com",
        "stripe_.com",
    ] {
        let arguments = args(&["Stripe", "--site", host]);
        assert_eq!(
            Options::parse(&arguments).err(),
            Some(Error::BadHost),
            "{host:?}"
        );
    }

    let max_label = "a".repeat(MAX_DNS_LABEL_BYTES);
    assert!(site_url(&format!("{max_label}.com")).is_some());
    assert!(site_url(&format!("{max_label}a.com")).is_none());
    let max_host = format!("{max_label}.{max_label}.{max_label}.{}.com", "a".repeat(57));
    assert_eq!(max_host.len(), MAX_HOST_BYTES);
    assert!(site_url(&max_host).is_some());
    assert!(site_url(&format!("a{max_host}")).is_none());
}

#[test]
fn query_refusal_uses_the_engines_byte_and_term_limits() {
    use zephium_map::query::{MAX_QUERY_BYTES, MAX_QUERY_TERMS};
    for query in [
        "a".repeat(MAX_QUERY_BYTES + 1),
        "a ".repeat(MAX_QUERY_TERMS + 1),
    ] {
        let arguments = args(&[&query, "--site", "stripe.com"]);
        assert_eq!(
            Options::parse(&arguments).unwrap().prepare().err(),
            Some(Error::Refused(StartRefusal::Invalid))
        );
    }
}

#[tokio::test]
async fn the_engine_reads_one_and_two_stand_in_results_in_order() {
    for (values, expected) in [
        (
            &["Stripe Pricing", "--site", "stripe.com"][..],
            &[("Stripe", "https://stripe.com/")][..],
        ),
        (
            &[
                "Figma vs Sketch",
                "--site",
                "figma.com",
                "--site",
                "sketch.com",
            ][..],
            &[
                ("Figma", "https://figma.com/"),
                ("Sketch", "https://sketch.com/"),
            ][..],
        ),
    ] {
        let arguments = args(values);
        let (request, stand_in) = Options::parse(&arguments).unwrap().prepare().unwrap();
        let url = request
            .engine
            .configured_search(&request.query, "")
            .unwrap();
        let page = stand_in.load(url).await.unwrap();
        let doc =
            Doc::from_html(&page.url, page.html.as_bytes(), None, DocLimits::RESULTS).unwrap();
        let query = read_query(&request.query).unwrap();
        let ResultsRead::FirstLayer { layer } = read_results(request.engine, None, &query, &doc)
        else {
            panic!("the stand-in must parse as results");
        };
        assert!(layer.social.is_empty());
        let actual = layer
            .organic
            .iter()
            .map(|hit| {
                assert!(hit.snippet.is_none());
                assert!(hit.sitelinks.is_empty());
                (hit.title.as_str(), hit.url.as_str())
            })
            .collect::<Vec<_>>();
        assert_eq!(actual, expected);
    }
}

#[test]
fn injected_non_direct_route_exits_before_creating_any_io() {
    for route in [PublicRoute::Proxied, PublicRoute::Unsupported] {
        assert_eq!(
            on_direct_route::<()>(route, || panic!("I/O started")),
            Err(Error::Proxy)
        );
    }
    assert_eq!(on_direct_route(PublicRoute::Direct, || Ok(7)), Ok(7));
    assert_eq!(
        Error::Proxy.to_string(),
        "The reader refuses to go around a proxy. A direct route is required."
    );
}

#[tokio::test]
async fn an_interrupt_cancels_the_driver_and_prints_its_end() {
    struct NoFetch;
    impl Fetcher for NoFetch {
        fn fetch(&self, _: FetchRequest) -> Reading<Result<FetchResult, Failure>> {
            panic!("the cancelled driver must not start a fetch");
        }
    }

    for buffered in [false, true] {
        let arguments = args(&["Stripe Pricing", "--site", "stripe.com"]);
        let (request, results) = Options::parse(&arguments).unwrap().prepare().unwrap();
        let io = Io {
            results: Arc::new(results),
            fetch: Arc::new(NoFetch),
            memo: Arc::new(MemoryMemo::new()),
            clock: Arc::new(SystemClock::new()),
        };
        let (driver, mut changes) = Driver::start(request, io, &Handle::current()).unwrap();
        if buffered {
            let (tx, rx) = tokio::sync::mpsc::channel(1);
            tx.try_send(Change::End {
                seq: 0,
                outcome: Outcome::Done,
                elapsed_ms: 0,
            })
            .unwrap();
            changes = rx;
        }
        let (_route, direct) = watch::channel(PublicRoute::Direct);
        let mut output = Vec::new();
        let outcome = stream(
            &driver,
            changes,
            direct,
            std::future::ready(Ok(())),
            &mut output,
            vec!["Pricing"],
        )
        .await;
        assert_eq!(outcome, Ok(Outcome::Cancelled));
        assert_eq!(driver.counts(), Counts::default());
        let printed = std::str::from_utf8(&output).unwrap();
        assert_eq!(printed.lines().count(), 1);
        assert!(printed.starts_with("Cancelled  "));
        assert!(printed.ends_with("ms  reads 0  data 0.0 MB  renders wanted 0\n"));
    }
}

#[tokio::test]
async fn a_closed_route_watch_is_not_a_proxy() {
    struct Refuse;
    impl Fetcher for Refuse {
        fn fetch(&self, _: FetchRequest) -> Reading<Result<FetchResult, Failure>> {
            Box::pin(async { Err(Failure::Unavailable) })
        }
    }

    let arguments = args(&["Stripe Pricing", "--site", "stripe.com"]);
    let (request, results) = Options::parse(&arguments).unwrap().prepare().unwrap();
    let io = Io {
        results: Arc::new(results),
        fetch: Arc::new(Refuse),
        memo: Arc::new(MemoryMemo::new()),
        clock: Arc::new(SystemClock::new()),
    };
    let (driver, changes) = Driver::start(request, io, &Handle::current()).unwrap();
    let (route, direct) = watch::channel(PublicRoute::Direct);
    drop(route);
    let mut output = Vec::new();
    let outcome = stream(
        &driver,
        changes,
        direct,
        std::future::pending(),
        &mut output,
        Vec::new(),
    )
    .await;
    assert!(outcome.is_ok(), "{outcome:?}");
    let printed = std::str::from_utf8(&output).unwrap();
    assert!(printed.lines().count() > 1, "{printed}");
}

#[tokio::test]
async fn labelled_opens_after_end_reprint_the_card_and_each_read_cost() {
    struct Fixture;
    impl Fetcher for Fixture {
        fn fetch(&self, request: FetchRequest) -> Reading<Result<FetchResult, Failure>> {
            Box::pin(async move {
                let html = match request.url.path() {
                    "/" => "<nav><a href='/docs'>Docs</a><a href='/pricing'>Pricing</a></nav>",
                    "/docs" => "<h2 id='first'>First guide</h2>",
                    "/pricing" => "<h2 id='teams'>Team plans</h2>",
                    _ => return Err(Failure::Missing),
                };
                Ok(FetchResult {
                    final_url: request.url,
                    content_type: "text/html".into(),
                    charset: None,
                    bytes: html.as_bytes().to_vec(),
                    truncated: false,
                })
            })
        }
    }
    let arguments = args(&[
        "Stripe",
        "--site",
        "stripe.com",
        "--open",
        "Docs",
        "--open",
        "Pricing",
    ]);
    let options = Options::parse(&arguments).unwrap();
    let opens = options.opens.clone();
    let (request, results) = options.prepare().unwrap();
    let io = Io {
        results: Arc::new(results),
        fetch: Arc::new(Fixture),
        memo: Arc::new(MemoryMemo::new()),
        clock: Arc::new(SystemClock::new()),
    };
    let (driver, changes) = Driver::start(request, io, &Handle::current()).unwrap();
    let (_route, direct) = watch::channel(PublicRoute::Direct);
    let mut output = Vec::new();
    assert_eq!(
        stream(
            &driver,
            changes,
            direct,
            std::future::pending(),
            &mut output,
            opens
        )
        .await,
        Ok(Outcome::Done)
    );
    let printed = std::str::from_utf8(&output).unwrap();
    let end = printed.find("Done").unwrap();
    let docs = printed.find("First guide").unwrap();
    let plans = printed.find("Team plans").unwrap();
    assert!(end < docs && docs < plans);
    let opens: Vec<_> = printed
        .lines()
        .filter(|line| line.starts_with("Opened read"))
        .collect();
    assert_eq!(opens.len(), 2);
    assert!(opens.iter().all(|line| line.contains("reads 1  bytes ")));
}

#[tokio::test]
async fn failed_opens_affect_status_and_remaining_opens_still_run() {
    struct Fixture;
    impl Fetcher for Fixture {
        fn fetch(&self, request: FetchRequest) -> Reading<Result<FetchResult, Failure>> {
            Box::pin(async move {
                let body = match request.url.path() {
                    "/" => "<nav><a href='/docs'>Docs</a><a href='/pricing'>Pricing</a></nav>",
                    "/pricing" => "<h2 id='annual'>Annual</h2>",
                    "/docs" => return Err(Failure::Timeout),
                    _ => return Err(Failure::Missing),
                };
                Ok(FetchResult {
                    final_url: request.url,
                    bytes: body.as_bytes().into(),
                    content_type: "text/html".into(),
                    charset: None,
                    truncated: false,
                })
            })
        }
    }
    for labels in [
        vec!["Unknown", "Pricing"],
        vec!["Docs", "Pricing"],
        vec!["Pricing", "Pricing"],
    ] {
        let args = args(&["Stripe", "--site", "stripe.com"]);
        let (request, results) = Options::parse(&args).unwrap().prepare().unwrap();
        let io = Io {
            results: Arc::new(results),
            fetch: Arc::new(Fixture),
            memo: Arc::new(MemoryMemo::new()),
            clock: Arc::new(SystemClock::new()),
        };
        let (driver, changes) = Driver::start(request, io, &Handle::current()).unwrap();
        let (_route, direct) = watch::channel(PublicRoute::Direct);
        let mut output = Vec::new();
        let outcome = stream(
            &driver,
            changes,
            direct,
            std::future::pending(),
            &mut output,
            labels,
        )
        .await;
        assert!(matches!(outcome, Err(_) | Ok(Outcome::Failed { .. })));
        let printed = String::from_utf8(output).unwrap();
        assert!(printed.contains("Opened read"));
        assert!(printed.contains("refused"));
    }
}
