use proptest::prelude::*;
use url::Url;
use zephium_map::key::*;

fn key(s: &str) -> PageKey {
    PageKey::from_url(&Url::parse(s).unwrap()).unwrap()
}
#[test]
fn page_identities() {
    for (left, right) in [
        ("HTTPS://EXAMPLE.COM.:443#foo", "https://example.com/"),
        (
            "http://ExAmPlE.com:80/?a=%2f&uTm_campaign=x&b=&a=2",
            "http://example.com/?a=%2f&b=&a=2",
        ),
        (
            "https://example.com/?gclid=1&fbclid=2&msclkid=3&ref_src=a&ref=z&%75tm_x=4",
            "https://example.com/?ref=z",
        ),
        (
            "https://bücher.example/a",
            "https://xn--bcher-kva.example/a",
        ),
        (
            "https://example.com/p?flag&mc_cid=abc&keep=%20",
            "https://example.com/p?flag&keep=%20",
        ),
    ] {
        assert!(key(left) == key(right));
    }
    for (left, right) in [
        ("https://a.com/?ref=x", "https://a.com/?ref=y"),
        ("http://a.com", "https://a.com"),
        ("https://a.com:81", "https://a.com"),
        ("https://a.com/x", "https://a.com/x/"),
        ("https://www.a.com", "https://a.com"),
        ("https://a.com/?a=1&b=2", "https://a.com/?b=2&a=1"),
        ("https://a.com/?id=1", "https://a.com/?id=2"),
    ] {
        assert!(key(left) != key(right));
    }
    for raw in [
        "ftp://host.com",
        "data:text/plain,x",
        "javascript:alert(1)",
        "https://:password@x.com",
        "https://user@x.com",
    ] {
        assert!(PageKey::from_url(&Url::parse(raw).unwrap()).is_err());
    }
}
#[test]
fn site_and_locator() {
    for (url, site) in [
        ("https://docs.acme.co.uk", Some("acme.co.uk")),
        ("https://u.github.io", Some("u.github.io")),
        ("https://127.0.0.1", None),
        ("https://[::1]", None),
        ("https://localhost", None),
        ("https://co.uk", None),
    ] {
        let key = SiteKey::from_url(&Url::parse(url).unwrap());
        assert_eq!(key.as_ref().map(SiteKey::as_str), site);
    }
    let quote = Locator::TextQuote {
        start: LocatorText::new("a,b&-").unwrap(),
        end: Some(LocatorText::new("é #").unwrap()),
    };
    assert_eq!(
        quote.fragment().as_deref(),
        Some(":~:text=a%2Cb%26%2D,%C3%A9%20%23")
    );
    assert!(LocatorText::new(&"a".repeat(MAX_LOCATOR_BYTES)).is_ok());
    for raw in [" ".into(), "a\0b".into(), "a".repeat(MAX_LOCATOR_BYTES + 1)] {
        assert!(LocatorText::new(&raw).is_err());
    }
    let prefix = "https://a.com/";
    let good = format!("{prefix}{}", "a".repeat(MAX_URL_BYTES - prefix.len()));
    assert!(PageKey::from_url(&Url::parse(&good).unwrap()).is_ok());
    assert!(matches!(
        PageKey::from_url(&Url::parse(&(good + "x")).unwrap()),
        Err(KeyError::UrlLength)
    ));
}
proptest! {
    #[test] fn canonicalization_is_idempotent(path in "[A-Za-z0-9%._/-]{0,80}", query in ".{0,80}") {
        let mut u=Url::parse("https://Example.com.:443").unwrap();
        u.set_path(&path);u.set_query(Some(&query));u.set_fragment(Some("section"));
        let first=PageKey::from_url(&u).unwrap();
        prop_assert_eq!(first.as_str(),key(first.as_str()).as_str().to_owned());
    }
    #[test] fn arbitrary_inputs_are_total(input in ".{0,512}") {
        if let Ok(url)=Url::parse(&input){let _=PageKey::from_url(&url);let _=SiteKey::from_url(&url);}
    }
}
