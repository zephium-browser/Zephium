use std::{
    net::IpAddr,
    sync::{atomic::Ordering, Arc},
    time::{Duration, Instant},
};

use super::server::{close_delimited, ok, redirect, response, Reply, Server, Step};
use crate::decode::tests::{brotli, gzip};
use crate::fixture::Fixture;
use crate::route::Route;
use crate::{
    Expect, PublicReader, ReadRefusal, ReadRequest, ReaderLimits, SizeLimit, StatusClass,
    StatusRefusal, TimeoutPhase, MAX_REDIRECTS, USER_AGENT,
};

fn limits() -> ReaderLimits {
    ReaderLimits {
        connect_timeout: Duration::from_secs(3),
        first_byte_timeout: Duration::from_secs(3),
        max_duration: Duration::from_secs(8),
        ..ReaderLimits::default()
    }
}

fn reader_with(limits: ReaderLimits) -> (PublicReader, Arc<Fixture>) {
    let fixture = Arc::new(Fixture::default());
    (PublicReader::fixture(limits, fixture.clone()), fixture)
}

fn reader() -> (PublicReader, Arc<Fixture>) {
    reader_with(limits())
}

fn request(url: url::Url, expect: Expect) -> ReadRequest {
    ReadRequest {
        url,
        expect,
        max_bytes: 1 << 20,
        deadline: Instant::now() + Duration::from_secs(8),
    }
}

fn html(url: url::Url) -> ReadRequest {
    request(url, Expect::Html)
}

fn parse(url: &str) -> url::Url {
    url::Url::parse(url).unwrap()
}

const PAGE: &[u8] =
    b"<!doctype html><html><head><title>Docs</title></head><body>pricing</body></html>";

#[tokio::test]
async fn a_page_is_read_with_its_shape_and_timings() {
    let server = Server::start(|_| {
        Reply::Keep(response(
            "200 OK",
            &[("Content-Type", "text/html; charset=ISO-8859-1")],
            PAGE,
        ))
    });
    let (reader, _) = reader();
    let body = reader.read(html(server.url("/docs#part"))).await.unwrap();
    assert_eq!(body.bytes, PAGE);
    assert_eq!(body.final_url, server.url("/docs"));
    assert_eq!(body.hops, 0);
    assert_eq!(body.status, StatusClass::Ok);
    assert_eq!(body.media_type.as_deref(), Some("text/html"));
    assert_eq!(body.charset.as_deref(), Some("iso-8859-1"));
    assert!(!body.truncated);
    assert!(body.wire_bytes > PAGE.len() as u64);
    assert!(body.timings.connect.is_some());
    assert!(body.timings.first_byte <= body.timings.total);
    assert_eq!(server.seen()[0].path(), "/docs");
    let counters = reader.counters();
    assert_eq!(
        (counters.reads, counters.completed, counters.connections),
        (1, 1, 1)
    );
    assert_eq!(counters.decoded_bytes, PAGE.len() as u64);
    assert_eq!(counters.wire_bytes, body.wire_bytes);
}

#[tokio::test]
async fn every_request_carries_only_the_fixed_product_identity() {
    let server = Server::start(|_| ok("text/plain", b"User-agent: *\n"));
    let (reader, _) = reader();
    for expect in [Expect::Html, Expect::Xml, Expect::Text] {
        let _ = reader
            .read(request(server.url("/robots.txt"), expect))
            .await;
    }
    for seen in server.seen() {
        let mut names: Vec<&str> = seen.headers.iter().map(|(name, _)| name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(
            names,
            ["accept", "accept-encoding", "host", "user-agent"],
            "{seen:?}"
        );
        assert_eq!(seen.header("user-agent"), Some(USER_AGENT));
        assert_eq!(seen.header("accept-encoding"), Some("gzip, br"));
        assert_eq!(seen.method, "GET");
    }
}

#[tokio::test]
async fn no_cookie_or_referer_is_ever_sent_within_a_session_or_across_redirects() {
    let server = Server::start(|seen| match seen.path() {
        "/set" => Reply::Keep(response(
            "200 OK",
            &[
                ("Content-Type", "text/html"),
                ("Set-Cookie", "session=secret; Path=/"),
                ("Set-Cookie", "other=1; Domain=127.0.0.1"),
            ],
            PAGE,
        )),
        "/hop" => Reply::Keep(response(
            "302 Found",
            &[("Location", "/landing"), ("Set-Cookie", "hop=1")],
            b"",
        )),
        _ => ok("text/html", PAGE),
    });
    let (reader, _) = reader();
    let session = reader.session();
    session.read(html(server.url("/set"))).await.unwrap();
    session.read(html(server.url("/next"))).await.unwrap();
    let hopped = session.read(html(server.url("/hop"))).await.unwrap();
    assert_eq!(hopped.hops, 1);
    reader.read(html(server.url("/fresh"))).await.unwrap();
    let seen = server.seen();
    assert_eq!(seen.len(), 5);
    for request in seen {
        for header in [
            "cookie",
            "referer",
            "authorization",
            "origin",
            "proxy-authorization",
        ] {
            assert!(request.header(header).is_none(), "{header} in {request:?}");
        }
    }
}

#[tokio::test]
async fn a_url_with_credentials_is_refused_before_any_connection() {
    let server = Server::start(|_| ok("text/html", PAGE));
    let (reader, _) = reader();
    for url in [
        format!("http://user:secret@127.0.0.1:{}/", server.port),
        format!("http://user@127.0.0.1:{}/", server.port),
        "https://user:secret@docs.example.com/".to_owned(),
    ] {
        assert_eq!(
            reader.read(html(parse(&url))).await,
            Err(ReadRefusal::NotPublicHttps)
        );
    }
    assert_eq!(server.connections.load(Ordering::Acquire), 0);
    assert_eq!(reader.counters().refused(ReadRefusal::NotPublicHttps), 3);
}

#[tokio::test]
async fn the_product_policy_refuses_what_the_fixture_alone_admits() {
    let product = PublicReader::new(
        limits(),
        crate::RouteSource::new(&tokio::runtime::Handle::current()),
    );
    let server = Server::start(|_| ok("text/html", PAGE));
    for url in [
        server.url("/").to_string(),
        "https://127.0.0.1/".into(),
        "https://localhost/".into(),
        "https://93.184.216.34/".into(),
        "https://docs.example.com:8443/".into(),
        "http://docs.example.com/".into(),
        "https://intranet/".into(),
        "ftp://docs.example.com/".into(),
    ] {
        assert_eq!(
            product.read(html(parse(&url))).await,
            Err(ReadRefusal::NotPublicHttps),
            "{url}"
        );
    }
    assert_eq!(server.connections.load(Ordering::Acquire), 0);
}

#[tokio::test]
async fn a_redirect_that_leaves_public_https_is_refused_not_followed() {
    let long = format!("/{}", "a".repeat(crate::MAX_LOCATION_BYTES));
    let server = Server::start(move |seen| match seen.path() {
        "/to-http" => redirect("http://docs.example.com/"),
        "/to-private" => redirect("https://10.0.0.1/"),
        "/to-literal" => redirect("https://93.184.216.34/"),
        "/to-localhost" => redirect("https://localhost/"),
        "/to-local" => redirect("https://printer.local/"),
        "/to-port" => redirect("https://docs.example.com:8443/"),
        "/to-credentials" => redirect("https://user:pw@docs.example.com/"),
        "/to-file" => redirect("file:///etc/hosts"),
        "/no-location" => Reply::Keep(response("302 Found", &[], b"")),
        "/two-locations" => Reply::Keep(response(
            "302 Found",
            &[("Location", "/a"), ("Location", "/b")],
            b"",
        )),
        "/long-location" => redirect(&long),
        _ => ok("text/html", PAGE),
    });
    let (reader, fixture) = reader();
    for path in [
        "/to-http",
        "/to-private",
        "/to-literal",
        "/to-localhost",
        "/to-local",
        "/to-port",
        "/to-credentials",
        "/to-file",
        "/no-location",
        "/two-locations",
        "/long-location",
    ] {
        assert_eq!(
            reader.read(html(server.url(path))).await,
            Err(ReadRefusal::RedirectRefused),
            "{path}"
        );
    }
    assert_eq!(server.seen().len(), 11);
    assert!(fixture.lookups.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_name_that_resolves_inward_or_mixed_is_refused_before_connecting() {
    let server = Server::start(|seen| match seen.path() {
        "/to-rebind" => redirect("https://rebind.example.com/"),
        _ => ok("text/html", PAGE),
    });
    let (reader, fixture) = reader();
    {
        let mut names = fixture.names.lock().unwrap();
        names.insert(
            "rebind.example.com".into(),
            vec!["127.0.0.1".parse().unwrap()],
        );
        let mixed: Vec<IpAddr> = vec![
            "93.184.216.34".parse().unwrap(),
            "10.0.0.1".parse().unwrap(),
        ];
        names.insert("mixed.example.com".into(), mixed);
        names.insert(
            "metadata.example.com".into(),
            vec!["169.254.169.254".parse().unwrap()],
        );
    }
    assert_eq!(
        reader.read(html(server.url("/to-rebind"))).await,
        Err(ReadRefusal::DnsRefused)
    );
    for url in [
        "https://mixed.example.com/",
        "https://metadata.example.com/latest",
    ] {
        assert_eq!(
            reader.read(html(parse(url))).await,
            Err(ReadRefusal::DnsRefused),
            "{url}"
        );
    }
    assert_eq!(
        reader
            .read(html(parse("https://unknown.example.com/")))
            .await,
        Err(ReadRefusal::Transport)
    );
    assert_eq!(server.connections.load(Ordering::Acquire), 1);
    assert_eq!(reader.counters().refused(ReadRefusal::DnsRefused), 3);
}

#[tokio::test]
async fn redirects_stop_at_the_hop_limit_and_on_a_loop() {
    let server = Server::start(|seen| {
        let path = seen.path().to_owned();
        if let Some(rest) = path.strip_prefix("/chain/") {
            let left: u8 = rest.parse().unwrap();
            if left == 0 {
                return ok("text/html", PAGE);
            }
            return redirect(&format!("/chain/{}", left - 1));
        }
        match path.as_str() {
            "/loop-a" => redirect("/loop-b"),
            "/loop-b" => redirect("/loop-a"),
            _ => ok("text/html", PAGE),
        }
    });
    let (reader, _) = reader();
    let body = reader
        .read(html(server.url(&format!("/chain/{MAX_REDIRECTS}"))))
        .await
        .unwrap();
    assert_eq!(body.hops, MAX_REDIRECTS);
    assert_eq!(body.final_url, server.url("/chain/0"));
    let before = server.seen().len();
    assert_eq!(
        reader
            .read(html(server.url(&format!("/chain/{}", MAX_REDIRECTS + 1))))
            .await,
        Err(ReadRefusal::TooManyRedirects)
    );
    assert_eq!(server.seen().len() - before, usize::from(MAX_REDIRECTS) + 1);
    let before = server.seen().len();
    assert_eq!(
        reader.read(html(server.url("/loop-a"))).await,
        Err(ReadRefusal::TooManyRedirects)
    );
    assert_eq!(server.seen().len() - before, 2);
}

#[tokio::test]
async fn each_status_class_is_refused_by_class() {
    let server = Server::start(|seen| {
        let reply = |status: &str, headers: &[(&str, &str)]| {
            Reply::Keep(response(
                status,
                &[&[("Content-Type", "text/html")], headers].concat(),
                PAGE,
            ))
        };
        match seen.path() {
            "/401" => reply("401 Unauthorized", &[]),
            "/403" => reply("403 Forbidden", &[]),
            "/429" => reply("429 Too Many Requests", &[]),
            "/404" => reply("404 Not Found", &[]),
            "/410" => reply("410 Gone", &[]),
            "/500" => reply("500 Internal Server Error", &[]),
            "/503" => reply("503 Service Unavailable", &[]),
            "/204" => Reply::Keep(b"HTTP/1.1 204 No Content\r\n\r\n".to_vec()),
            "/300" => reply("300 Multiple Choices", &[]),
            "/cf" => reply("403 Forbidden", &[("cf-mitigated", "challenge")]),
            "/cf-ok" => reply("200 OK", &[("CF-Mitigated", "Challenge")]),
            "/waf" => reply("202 Accepted", &[("x-amzn-waf-action", "captcha")]),
            _ => reply("200 OK", &[]),
        }
    });
    let (reader, _) = reader();
    for (path, refusal) in [
        ("/401", StatusRefusal::Denied),
        ("/403", StatusRefusal::Denied),
        ("/429", StatusRefusal::RateLimited),
        ("/404", StatusRefusal::Client),
        ("/410", StatusRefusal::Client),
        ("/500", StatusRefusal::Server),
        ("/503", StatusRefusal::Server),
        ("/204", StatusRefusal::Unexpected),
        ("/300", StatusRefusal::Unexpected),
        ("/cf", StatusRefusal::Challenge),
        ("/cf-ok", StatusRefusal::Challenge),
        ("/waf", StatusRefusal::Challenge),
    ] {
        assert_eq!(
            reader.read(html(server.url(path))).await,
            Err(ReadRefusal::Status(refusal)),
            "{path}"
        );
    }
}

#[tokio::test]
async fn compressed_bodies_are_decoded_and_unsupported_codings_refused() {
    let big: Vec<u8> = PAGE.iter().copied().cycle().take(300_000).collect();
    let gzipped = gzip(&big);
    let brotlied = brotli(&big);
    let server = Server::start(move |seen| {
        let encoded = |coding: &str, body: &[u8]| {
            Reply::Keep(response(
                "200 OK",
                &[("Content-Type", "text/html"), ("Content-Encoding", coding)],
                body,
            ))
        };
        match seen.path() {
            "/gzip" => encoded("gzip", &gzipped),
            "/br" => encoded("br", &brotlied),
            "/identity" => encoded("identity", PAGE),
            "/deflate" => encoded("deflate", PAGE),
            "/zstd" => encoded("zstd", PAGE),
            "/stacked" => encoded("gzip, br", PAGE),
            "/mislabelled" => encoded("gzip", PAGE),
            "/two" => Reply::Keep(response(
                "200 OK",
                &[
                    ("Content-Type", "text/html"),
                    ("Content-Encoding", "gzip"),
                    ("Content-Encoding", "br"),
                ],
                PAGE,
            )),
            _ => ok("text/html", PAGE),
        }
    });
    let (reader, _) = reader();
    for path in ["/gzip", "/br"] {
        let body = reader.read(html(server.url(path))).await.unwrap();
        assert_eq!(body.bytes, big, "{path}");
        assert!(body.wire_bytes < big.len() as u64 / 4, "{path}");
    }
    assert_eq!(
        reader
            .read(html(server.url("/identity")))
            .await
            .unwrap()
            .bytes,
        PAGE
    );
    for path in ["/deflate", "/zstd", "/stacked", "/mislabelled", "/two"] {
        assert_eq!(
            reader.read(html(server.url(path))).await,
            Err(ReadRefusal::Encoding),
            "{path}"
        );
    }
    let counters = reader.counters();
    assert_eq!(
        counters.decoded_bytes,
        2 * big.len() as u64 + PAGE.len() as u64
    );
    assert!(counters.wire_bytes < counters.decoded_bytes);
}

#[tokio::test]
async fn a_compression_bomb_stops_at_the_cap() {
    let bomb = gzip(&vec![b'x'; 16 * 1024 * 1024]);
    let server = Server::start(move |_| {
        Reply::Keep(response(
            "200 OK",
            &[("Content-Type", "text/html"), ("Content-Encoding", "gzip")],
            &bomb,
        ))
    });
    let (reader, _) = reader();
    let cap = 64 * 1024;
    let body = reader
        .read(ReadRequest {
            max_bytes: cap,
            ..html(server.url("/"))
        })
        .await
        .unwrap();
    assert!(body.truncated);
    assert_eq!(body.bytes.len(), cap);
    assert_eq!(reader.counters().truncated, 1);
}

#[tokio::test]
async fn a_truncated_compressed_stream_is_refused() {
    let gzipped = gzip(&PAGE.repeat(100));
    let brotlied = brotli(&PAGE.repeat(100));
    let server = Server::start(move |seen| {
        let (coding, body) = if seen.path() == "/br" {
            ("br", &brotlied)
        } else {
            ("gzip", &gzipped)
        };
        close_delimited(
            &[("Content-Type", "text/html"), ("Content-Encoding", coding)],
            &body[..body.len() / 2],
        )
    });
    let (reader, _) = reader();
    for path in ["/gzip", "/br"] {
        assert_eq!(
            reader.read(html(server.url(path))).await,
            Err(ReadRefusal::Encoding),
            "{path}"
        );
    }
}

#[tokio::test]
async fn a_lying_content_length_is_never_trusted() {
    let gzipped = gzip(&PAGE.repeat(50));
    let server = Server::start(move |seen| {
        let mut bytes = match seen.path() {
            "/long" => b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 100000\r\nConnection: close\r\n\r\n".to_vec(),
            "/short" => b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 15\r\nConnection: close\r\n\r\n".to_vec(),
            _ => format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Encoding: gzip\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                gzipped.len() / 2
            )
            .into_bytes(),
        };
        if seen.path() == "/gzip" {
            bytes.extend_from_slice(&gzipped);
        } else {
            bytes.extend_from_slice(PAGE);
        }
        Reply::Close(bytes)
    });
    let (reader, _) = reader();
    assert_eq!(
        reader.read(html(server.url("/long"))).await,
        Err(ReadRefusal::Transport)
    );
    assert_eq!(
        reader.read(html(server.url("/short"))).await.unwrap().bytes,
        &PAGE[..15]
    );
    assert_eq!(
        reader.read(html(server.url("/gzip"))).await,
        Err(ReadRefusal::Encoding)
    );
}

#[tokio::test]
async fn slow_headers_and_a_slow_body_time_out_in_their_phase() {
    let server = Server::start(|seen| match seen.path() {
        "/slow-head" => Reply::Steps(vec![
            Step::Sleep(Duration::from_millis(1500)),
            Step::Write(response("200 OK", &[("Content-Type", "text/html")], PAGE)),
        ]),
        "/partial-head" => Reply::Steps(vec![
            Step::Write(b"HTTP/1.1 200 OK\r\nContent-Type: te".to_vec()),
            Step::Sleep(Duration::from_millis(1500)),
        ]),
        _ => {
            let mut steps = vec![Step::Write(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 1000\r\n\r\n<html>"
                    .to_vec(),
            )];
            for _ in 0..40 {
                steps.push(Step::Sleep(Duration::from_millis(100)));
                steps.push(Step::Write(b" ".to_vec()));
            }
            Reply::Steps(steps)
        }
    });
    let (reader, _) = reader_with(ReaderLimits {
        first_byte_timeout: Duration::from_millis(300),
        max_duration: Duration::from_secs(8),
        ..limits()
    });
    for path in ["/slow-head", "/partial-head"] {
        let started = Instant::now();
        assert_eq!(
            reader.read(html(server.url(path))).await,
            Err(ReadRefusal::Timeout(TimeoutPhase::FirstByte)),
            "{path}"
        );
        assert!(started.elapsed() < Duration::from_millis(1200));
    }
    let started = Instant::now();
    let deadline = Instant::now() + Duration::from_millis(700);
    assert_eq!(
        reader
            .read(ReadRequest {
                deadline,
                ..html(server.url("/drip"))
            })
            .await,
        Err(ReadRefusal::Timeout(TimeoutPhase::Body))
    );
    assert!(started.elapsed() < Duration::from_millis(1500));
}

#[tokio::test]
async fn an_endless_body_is_cut_at_the_cap_or_refused_at_the_wire_limit() {
    let server = Server::start(|_| Reply::Endless {
        head: b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nConnection: close\r\n\r\n<html>"
            .to_vec(),
        chunk: vec![b'x'; 8192],
    });
    let (reader, _) = reader();
    let body = reader
        .read(ReadRequest {
            max_bytes: 100_000,
            ..html(server.url("/"))
        })
        .await
        .unwrap();
    assert!(body.truncated);
    assert_eq!(body.bytes.len(), 100_000);
    let (narrow, _) = reader_with(ReaderLimits {
        max_wire_bytes: 64 * 1024,
        ..limits()
    });
    assert_eq!(
        narrow
            .read(ReadRequest {
                max_bytes: 1 << 20,
                ..html(server.url("/"))
            })
            .await,
        Err(ReadRefusal::TooLarge(SizeLimit::Wire))
    );
    let xml = Server::start(|_| {
        Reply::Endless {
        head: b"HTTP/1.1 200 OK\r\nContent-Type: application/xml\r\nConnection: close\r\n\r\n<?xml version=\"1.0\"?><urlset>".to_vec(),
        chunk: b"<url><loc>https://example.com/</loc></url>".to_vec(),
    }
    });
    let body = reader
        .read(ReadRequest {
            max_bytes: 10_000,
            ..request(xml.url("/sitemap.xml"), Expect::Xml)
        })
        .await
        .unwrap();
    assert!(body.truncated);
    assert_eq!(body.bytes.len(), 10_000);
}

#[tokio::test]
async fn headers_over_the_limit_are_refused() {
    let limit = 4096;
    let server = Server::start(|seen| {
        let size: usize = seen.path()[1..].parse().unwrap();
        let filler = "v".repeat(size);
        Reply::Keep(response(
            "200 OK",
            &[("Content-Type", "text/html"), ("X-Filler", &filler)],
            PAGE,
        ))
    });
    let (reader, _) = reader_with(ReaderLimits {
        max_header_bytes: limit,
        ..limits()
    });
    let fixed = "content-type".len()
        + "text/html".len()
        + 4
        + "content-length".len()
        + PAGE.len().to_string().len()
        + 4
        + "x-filler".len()
        + 4;
    let fits = limit - fixed;
    assert!(reader
        .read(html(server.url(&format!("/{fits}"))))
        .await
        .is_ok());
    assert_eq!(
        reader
            .read(html(server.url(&format!("/{}", fits + 1))))
            .await,
        Err(ReadRefusal::TooLarge(SizeLimit::Headers))
    );
    let (default, _) = reader_with(limits());
    assert_eq!(
        default
            .read(html(
                server.url(&format!("/{}", crate::MAX_HEADER_BYTES + 1))
            ))
            .await,
        Err(ReadRefusal::TooLarge(SizeLimit::Headers))
    );
}

#[tokio::test]
async fn content_types_are_gated_and_sniffed_per_family() {
    let sitemap =
        b"<?xml version=\"1.0\"?><urlset><url><loc>https://example.com/</loc></url></urlset>"
            .to_vec();
    let wrapped = gzip(&sitemap);
    let png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
    let served = sitemap.clone();
    let server = Server::start(move |seen| match seen.path() {
        "/html-as-text" => ok("text/plain", PAGE),
        "/binary-as-html" => ok("text/html", &png),
        "/untyped-html" => Reply::Keep(response("200 OK", &[], PAGE)),
        "/untyped-text" => Reply::Keep(response("200 OK", &[], b"# llms.txt\n> A site\n")),
        "/untyped-binary" => Reply::Keep(response("200 OK", &[], &png)),
        "/sitemap.xml" => ok("text/xml; charset=utf-8", &served),
        "/sitemap.xml.gz" => ok("application/x-gzip", &wrapped),
        "/sitemap-gzip.xml.gz" => ok("application/gzip", &wrapped),
        "/html-as-xml" => ok("application/xml", PAGE),
        "/llms.txt" => ok(
            "text/markdown",
            b"# Docs\n- [Pricing](https://example.com/pricing)\n",
        ),
        "/json" => ok("application/json", b"{}"),
        "/two-types" => Reply::Keep(response(
            "200 OK",
            &[
                ("Content-Type", "text/html"),
                ("Content-Type", "text/plain"),
            ],
            PAGE,
        )),
        _ => ok("text/html", PAGE),
    });
    let (reader, _) = reader();
    let read = |path: &str, expect| reader.read(request(server.url(path), expect));
    let wrong = Err(ReadRefusal::WrongContentType);
    assert_eq!(read("/html-as-text", Expect::Html).await, wrong);
    assert_eq!(read("/html-as-text", Expect::Text).await, wrong);
    assert_eq!(read("/binary-as-html", Expect::Html).await, wrong);
    assert_eq!(read("/untyped-binary", Expect::Html).await, wrong);
    assert_eq!(read("/untyped-binary", Expect::Text).await, wrong);
    assert_eq!(read("/html-as-xml", Expect::Xml).await, wrong);
    assert_eq!(read("/json", Expect::Text).await, wrong);
    assert_eq!(read("/two-types", Expect::Html).await, wrong);
    assert_eq!(read("/sitemap.xml.gz", Expect::Html).await, wrong);
    let sniffed = read("/untyped-html", Expect::Html).await.unwrap();
    assert_eq!((sniffed.media_type, sniffed.bytes.as_slice()), (None, PAGE));
    assert_eq!(read("/untyped-html", Expect::Text).await, wrong);
    assert!(read("/untyped-text", Expect::Text).await.is_ok());
    let xml = read("/sitemap.xml", Expect::Xml).await.unwrap();
    assert_eq!(xml.bytes, sitemap);
    assert_eq!(xml.charset.as_deref(), Some("utf-8"));
    for path in ["/sitemap.xml.gz", "/sitemap-gzip.xml.gz"] {
        assert_eq!(
            read(path, Expect::Xml).await.unwrap().bytes,
            sitemap,
            "{path}"
        );
    }
    assert!(read("/llms.txt", Expect::Text).await.is_ok());
    assert_eq!(reader.counters().refused(ReadRefusal::WrongContentType), 10);
}

#[tokio::test]
async fn a_deadline_already_past_or_spent_queueing_is_a_queue_timeout() {
    let hang = Server::start(|_| Reply::Hang);
    let (reader, _) = reader_with(ReaderLimits {
        max_concurrent: 1,
        ..limits()
    });
    let past = ReadRequest {
        deadline: Instant::now(),
        ..html(hang.url("/"))
    };
    assert_eq!(
        reader.read(past).await,
        Err(ReadRefusal::Timeout(TimeoutPhase::Queue))
    );
    let holder = {
        let reader = reader.clone();
        let url = hang.url("/hold");
        tokio::spawn(async move { reader.read(html(url)).await })
    };
    wait_for(|| !hang.seen().is_empty()).await;
    let other = Server::start(|_| ok("text/html", PAGE));
    let queued = ReadRequest {
        deadline: Instant::now() + Duration::from_millis(200),
        ..html(other.url("/"))
    };
    assert_eq!(
        reader.read(queued).await,
        Err(ReadRefusal::Timeout(TimeoutPhase::Queue))
    );
    holder.abort();
}

async fn wait_for(mut condition: impl FnMut() -> bool) {
    let started = Instant::now();
    while !condition() {
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "condition never held"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_global_limit_holds_under_a_burst() {
    let server = Server::start(|_| {
        Reply::Steps(vec![
            Step::Sleep(Duration::from_millis(150)),
            Step::Write(response("200 OK", &[("Content-Type", "text/html")], PAGE)),
        ])
    });
    let (reader, _) = reader_with(ReaderLimits {
        max_concurrent: 2,
        max_per_host: 4,
        ..limits()
    });
    let mut reads = tokio::task::JoinSet::new();
    for n in 0..6 {
        let reader = reader.clone();
        let url = server.url(&format!("/{n}"));
        reads.spawn(async move { reader.read(html(url)).await });
    }
    while let Some(read) = reads.join_next().await {
        assert!(read.unwrap().is_ok());
    }
    assert_eq!(server.in_flight.max.load(Ordering::Acquire), 2);
    assert_eq!(server.seen().len(), 6);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_per_host_limit_holds_while_other_hosts_proceed() {
    let shared = Arc::new(super::server::Gauge::default());
    let slow = || {
        |_: &super::server::Seen| {
            Reply::Steps(vec![
                Step::Sleep(Duration::from_millis(150)),
                Step::Write(response("200 OK", &[("Content-Type", "text/html")], PAGE)),
            ])
        }
    };
    let first = Server::start_with(shared.clone(), slow());
    let second = Server::start_with(shared.clone(), slow());
    let (reader, _) = reader_with(ReaderLimits {
        max_concurrent: 4,
        max_per_host: 1,
        ..limits()
    });
    let mut reads = tokio::task::JoinSet::new();
    for n in 0..3 {
        for server in [&first, &second] {
            let reader = reader.clone();
            let url = server.url(&format!("/{n}"));
            reads.spawn(async move { reader.read(html(url)).await });
        }
    }
    while let Some(read) = reads.join_next().await {
        assert!(read.unwrap().is_ok());
    }
    assert_eq!(first.in_flight.max.load(Ordering::Acquire), 1);
    assert_eq!(second.in_flight.max.load(Ordering::Acquire), 1);
    assert_eq!(shared.max.load(Ordering::Acquire), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancelled_read_frees_its_permits_and_stops_reading() {
    let hang = Server::start(|_| Reply::Hang);
    let drip = Server::start(|_| {
        let mut steps = vec![Step::Write(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nConnection: close\r\n\r\n<html>"
                .to_vec(),
        )];
        for _ in 0..200 {
            steps.push(Step::Sleep(Duration::from_millis(25)));
            steps.push(Step::Write(vec![b'x'; 1024]));
        }
        Reply::Steps(steps)
    });
    let (reader, _) = reader_with(ReaderLimits {
        max_concurrent: 1,
        max_per_host: 1,
        ..limits()
    });
    for server in [&hang, &drip] {
        let held = {
            let reader = reader.clone();
            let url = server.url("/");
            tokio::spawn(async move {
                reader
                    .read(ReadRequest {
                        max_bytes: crate::MAX_DECODED_BYTES,
                        ..html(url)
                    })
                    .await
            })
        };
        wait_for(|| !server.seen().is_empty()).await;
        held.abort();
        assert!(held.await.unwrap_err().is_cancelled());
        wait_for(|| server.closed.load(Ordering::Acquire) == 1).await;
    }
    let other = Server::start(|_| ok("text/html", PAGE));
    let started = Instant::now();
    assert!(reader.read(html(other.url("/"))).await.is_ok());
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[tokio::test]
async fn connections_are_reused_within_a_session_and_never_across_sessions() {
    let server = Server::start(|_| ok("text/html", PAGE));
    let (reader, _) = reader();
    let session = reader.session();
    let first = session.read(html(server.url("/a"))).await.unwrap();
    let second = session.read(html(server.url("/b"))).await.unwrap();
    assert!(first.timings.connect.is_some());
    assert_eq!(second.timings.connect, None);
    assert_eq!(server.connections.load(Ordering::Acquire), 1);
    let other = reader.session();
    let third = other.read(html(server.url("/c"))).await.unwrap();
    assert!(third.timings.connect.is_some());
    assert_eq!(server.connections.load(Ordering::Acquire), 2);
    reader.read(html(server.url("/d"))).await.unwrap();
    assert_eq!(server.connections.load(Ordering::Acquire), 3);
    assert_eq!(reader.counters().connections, 3);
}

#[tokio::test]
async fn through_the_persons_proxy_the_destination_is_never_resolved_or_reached_directly() {
    let proxy = Server::start(|_| {
        Reply::Close(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n".to_vec())
    });
    let (reader, fixture) = reader();
    fixture.names.lock().unwrap().insert(
        "docs.example.com".into(),
        vec!["127.0.0.1".parse().unwrap()],
    );
    reader
        .routes()
        .set_app_proxy(zephium_core::proxy::from_settings(
            "http",
            &format!("127.0.0.1:{}", proxy.port),
        ));
    assert_eq!(
        reader
            .read(html(parse("https://docs.example.com/pricing")))
            .await,
        Err(ReadRefusal::Transport)
    );
    let seen = proxy.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].method, "CONNECT");
    assert_eq!(seen[0].target, "docs.example.com:443");
    assert!(seen[0].header("proxy-authorization").is_none());
    assert!(fixture.lookups.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_proxy_that_never_answers_is_a_connect_timeout() {
    let proxy = Server::start(|_| Reply::Hang);
    let (reader, _) = reader_with(ReaderLimits {
        connect_timeout: Duration::from_millis(300),
        ..limits()
    });
    reader
        .routes()
        .set_app_proxy(zephium_core::proxy::from_settings(
            "http",
            &format!("127.0.0.1:{}", proxy.port),
        ));
    let started = Instant::now();
    assert_eq!(
        reader.read(html(parse("https://docs.example.com/"))).await,
        Err(ReadRefusal::Timeout(TimeoutPhase::Connect))
    );
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[tokio::test]
async fn a_proxy_setting_that_cannot_be_reproduced_sends_nothing() {
    let server = Server::start(|_| ok("text/html", PAGE));
    let (reader, fixture) = reader();
    reader
        .routes()
        .set_app_proxy(zephium_core::proxy::from_settings(
            "socks5",
            "127.0.0.1:9050",
        ));
    assert_eq!(
        reader.read(html(server.url("/"))).await,
        Err(ReadRefusal::ProxyUnsupported)
    );
    reader.routes().set_app_proxy(None);
    *fixture.system.lock().unwrap() = Some(Route::Unsupported);
    reader.routes().reread().await;
    assert_eq!(
        reader.read(html(server.url("/"))).await,
        Err(ReadRefusal::ProxyUnsupported)
    );
    assert_eq!(server.connections.load(Ordering::Acquire), 0);
    *fixture.system.lock().unwrap() = Some(Route::Direct);
    reader.routes().reread().await;
    assert!(reader.read(html(server.url("/"))).await.is_ok());
    assert_eq!(reader.counters().refused(ReadRefusal::ProxyUnsupported), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn changing_the_proxy_refuses_reads_in_flight_and_reroutes_the_session() {
    let hang = Server::start(|_| Reply::Hang);
    let proxy = Server::start(|_| {
        Reply::Close(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n".to_vec())
    });
    let (reader, _) = reader();
    let session = Arc::new(reader.session());
    let in_flight = {
        let session = session.clone();
        let url = hang.url("/");
        tokio::spawn(async move { session.read(html(url)).await })
    };
    wait_for(|| !hang.seen().is_empty()).await;
    reader
        .routes()
        .set_app_proxy(zephium_core::proxy::from_settings(
            "http",
            &format!("127.0.0.1:{}", proxy.port),
        ));
    assert_eq!(in_flight.await.unwrap(), Err(ReadRefusal::RouteChanged));
    wait_for(|| hang.closed.load(Ordering::Acquire) == 1).await;
    let _ = session.read(html(parse("https://docs.example.com/"))).await;
    assert_eq!(proxy.seen().len(), 1);
    assert_eq!(reader.counters().refused(ReadRefusal::RouteChanged), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_system_route_change_at_a_refresh_refuses_reads_in_flight() {
    let hang = Server::start(|_| Reply::Hang);
    let server = Server::start(|_| ok("text/html", PAGE));
    let (reader, fixture) = reader();
    let session = Arc::new(reader.session());
    let in_flight = {
        let session = session.clone();
        let url = hang.url("/");
        tokio::spawn(async move { session.read(html(url)).await })
    };
    wait_for(|| !hang.seen().is_empty()).await;
    *fixture.system.lock().unwrap() = Some(Route::Unsupported);
    reader.routes().reread().await;
    assert_eq!(in_flight.await.unwrap(), Err(ReadRefusal::RouteChanged));
    assert_eq!(
        session.read(html(server.url("/"))).await,
        Err(ReadRefusal::ProxyUnsupported)
    );
    assert_eq!(server.connections.load(Ordering::Acquire), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_read_before_the_route_is_known_is_refused_without_connecting() {
    let server = Server::start(|_| ok("text/html", PAGE));
    let (answer, gate) = std::sync::mpsc::channel::<()>();
    let gate = std::sync::Mutex::new(gate);
    let routes = crate::RouteSource::unread(
        Arc::new(move || {
            let _ = gate.lock().unwrap().recv();
            Route::Direct
        }),
        crate::RouteSource::FIXTURE,
    );
    let reader = PublicReader::fixture_with(limits(), Arc::new(Fixture::default()), routes.clone());
    routes.set_app_proxy(None);
    assert_eq!(
        reader.read(html(server.url("/"))).await,
        Err(ReadRefusal::ProxyUnsupported)
    );
    assert_eq!(server.connections.load(Ordering::Acquire), 0);
    answer.send(()).unwrap();
    routes.settled().await;
    assert!(reader.read(html(server.url("/"))).await.is_ok());
}

#[tokio::test]
async fn a_connection_that_fails_asks_for_a_system_read() {
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = closed.local_addr().unwrap().port();
    drop(closed);
    let (reader, _) = reader();
    assert_eq!(reader.routes().reads(), 0);
    assert_eq!(
        reader
            .read(html(parse(&format!("http://127.0.0.1:{port}/"))))
            .await,
        Err(ReadRefusal::Transport)
    );
    wait_for(|| reader.routes().reads() == 1).await;
}

#[tokio::test]
async fn binary_is_refused_once_the_sniff_window_fills_not_at_the_end() {
    let server = Server::start(|seen| {
        let head: &[u8] = if seen.path() == "/untyped" {
            b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n"
        } else {
            b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nConnection: close\r\n\r\n"
        };
        Reply::Endless {
            head: [head, b"<html>"].concat(),
            chunk: vec![0; crate::SNIFF_BYTES],
        }
    });
    let (reader, _) = reader();
    for path in ["/untyped", "/declared"] {
        assert_eq!(
            reader
                .read(ReadRequest {
                    max_bytes: crate::MAX_DECODED_BYTES,
                    ..html(server.url(path))
                })
                .await,
            Err(ReadRefusal::WrongContentType),
            "{path}"
        );
    }
    assert!(reader.counters().wire_bytes < 1 << 20);
}

#[tokio::test]
async fn zero_body_cap_never_returns_payload_bytes() {
    let server = Server::start(|_| ok("text/html", PAGE));
    let (reader, _) = reader();
    let body = reader
        .read(ReadRequest {
            max_bytes: 0,
            ..html(server.url("/"))
        })
        .await
        .unwrap();
    assert!(body.truncated && body.bytes.is_empty());
}

#[tokio::test]
async fn cross_host_redirect_releases_global_capacity_before_waiting() {
    let target = Server::start(|_| ok("text/html", PAGE));
    let landing = target.url("/landing");
    let (release, gated) = std::sync::mpsc::channel();
    let gated = std::sync::Mutex::new(gated);
    let start = Server::start(move |_| {
        gated
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(3))
            .unwrap();
        redirect(landing.as_str())
    });
    let (reader, _) = reader_with(ReaderLimits {
        max_concurrent: 1,
        max_per_host: 1,
        ..limits()
    });
    let first = tokio::spawn({
        let reader = reader.clone();
        let url = start.url("/");
        async move { reader.read(html(url)).await }
    });
    wait_for(|| !start.seen().is_empty()).await;
    let second = reader.read(html(target.url("/independent")));
    tokio::pin!(second);
    std::future::poll_fn(|cx| {
        use std::future::Future;
        assert!(second.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        assert!(second.await.is_ok());
        assert_eq!(first.await.unwrap().unwrap().hops, 1);
    })
    .await
    .unwrap();
}
