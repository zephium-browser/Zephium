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
    let quote = Locator {
        quote: Some(TextQuote {
            prefix: None,
            suffix: None,
            start: LocatorText::new("a,b&-").unwrap(),
            end: Some(LocatorText::new("é #").unwrap()),
        }),
        ..Default::default()
    };
    assert_eq!(
        quote.fragment().as_deref(),
        Some("#:~:text=a%2Cb%26%2D,%C3%A9%20%23")
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

#[test]
fn stored_identity_is_independent_of_tracking_and_suffix_tables() {
    let old = "https://example.com/doc?srsltid=old";
    let stored: PageKey = serde_json::from_value(old.into()).unwrap();
    assert_eq!(stored.as_str(), old);
    assert_ne!(key(old).as_str(), old);
    assert_eq!(stored.page_match(), key(old).page_match());
    let suffix: SiteKey = serde_json::from_str("\"co.uk\"").unwrap();
    assert_eq!(suffix.as_str(), "co.uk");
    assert!(SiteKey::from_url(&Url::parse("https://co.uk/").unwrap()).is_none());
    for invalid in [
        "https://EXAMPLE.com/",
        "https://bücher.example/",
        "https://u@example.com/",
        "file:///local",
    ] {
        assert!(serde_json::from_value::<PageKey>(invalid.into()).is_err());
    }
}

proptest! {
    #[test]
    fn match_is_idempotent_and_preserves_meaning(path in "[a-z]{1,32}",value in "[0-9]{1,12}",route in "[a-z]{1,12}"){
        let canonical=key(&format!("https://example.com/{path}~?a=%2F&b={value}#/{route}"));
        let alias=key(&format!("http://www.example.com/{path}%7e/?b={value}&srsltid=1&a=%2f#/{route}"));
        let matched=canonical.page_match();
        prop_assert_eq!(&matched,&alias.page_match());
        prop_assert_eq!(&matched,&key(&format!("https://{}",matched.as_str())).page_match());
        for different in [format!("https://example.com/{path}x?a=%2F&b={value}#/{route}"),
                format!("https://example.com/{path}~?a=/&b={value}#/{route}"),
                format!("https://example.com/{path}~?a=%2F&b={value}#!/other")]{
            prop_assert_ne!(&matched,&key(&different).page_match());
        }
    }
}
#[test]
fn place_keys_scope_hosts_and_tenants() {
    let place =
        |s: &str| PlaceKey::from_url(&Url::parse(&format!("https://{s}")).unwrap()).unwrap();
    for (seed, tenant) in [
        ("github.com/a", Some("/a")),
        ("github.com/a/b/tree/main", Some("/a/b")),
        ("gitlab.com/team/project/blob/main", Some("/team")),
        ("medium.com/@writer/blog", Some("/@writer")),
        ("youtube.com/@channel/videos", Some("/@channel")),
        ("sites.google.com/view/work/pages", Some("/view/work")),
        ("x.com/name/status/1", Some("/name")),
        ("reddit.com/r/rust/comments", Some("/r/rust")),
        ("sites.google.com/view/", None),
        ("medium.com/story", None),
    ] {
        assert_eq!(place(seed).tenant.as_deref(), tenant);
    }
    assert_eq!(place("www.acme.com/deep"), place("acme.com/"));
    assert_ne!(place("docs.acme.com/"), place("acme.com/"));
}

#[test]
fn page_match_preserves_relative_order_of_repeated_parameters() {
    let original = key("https://example.com/?sort=name&count=10&sort=date").page_match();
    assert_eq!(
        original,
        key("https://example.com/?count=10&sort=name&sort=date").page_match()
    );
    assert_ne!(
        original,
        key("https://example.com/?sort=date&count=10&sort=name").page_match()
    );
}
