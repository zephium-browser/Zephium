use super::*;
use serde_json::json;

const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const ALIAS: &str = "cccccccccccccccccccccccccccccccc";

struct NoHost;
impl RequestHost for NoHost {
    fn requester(&self, _: &webkit2gtk::WebView) -> Option<Requester> {
        None
    }
}

fn fixture(war: Value) -> (tempfile::TempDir, Scheme) {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("public.txt"), "package").unwrap();
    let scheme = Scheme {
        host: Rc::new(NoHost),
        favicon: None,
        installs: RefCell::new(BTreeMap::new()),
        active: Arc::new(AtomicBool::new(true)),
        streams: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    };
    let manifest =
        Manifest::from_value(json!({"manifest_version":3,"web_accessible_resources":war}));
    scheme
        .add_install(1.into(), A, directory.path(), &manifest)
        .unwrap();
    scheme
        .add_install(
            2.into(),
            B,
            directory.path(),
            &Manifest::from_value(json!({"manifest_version":3})),
        )
        .unwrap();
    scheme
        .installs
        .borrow_mut()
        .get_mut(&1.into())
        .unwrap()
        .alias = ALIAS.into();
    (directory, scheme)
}

fn tab(origin: &str) -> Requester {
    Requester::Tab {
        committed_top_level: Url::parse(origin).unwrap(),
        extension_install: None,
    }
}
fn uri(host: &str, path: &str) -> String {
    format!("chrome-extension://{host}/{path}")
}

#[test]
fn admission_uses_host_origin_and_live_install_generation() {
    let (_directory, scheme) =
        fixture(json!([{ "resources":["public*"], "matches":["https://*.example.test/*"] }]));
    assert!(scheme.allows(&tab("https://sub.example.test/deep"), &uri(A, "public.txt")));
    for requester in [
        tab("http://example.test/"),
        tab("https://example.test.evil/"),
        tab("data:text/html,x"),
        tab(&uri(A, "page.html")),
        Requester::ExtensionOwned {
            install: 999.into(),
        },
    ] {
        assert!(!scheme.allows(&requester, &uri(A, "public.txt")));
    }
    let own = Requester::ExtensionOwned { install: 1.into() };
    assert!(scheme.allows(&own, &uri(A, "private.txt")));
    assert!(scheme.allows(&own, &uri(A, "_zephium/background.html")));
    assert!(!scheme.allows(&own, &uri(B, "private.txt")));
    let extension_tab = Requester::Tab {
        committed_top_level: Url::parse(&uri(A, "page.html")).unwrap(),
        extension_install: Some(1.into()),
    };
    assert!(scheme.allows(&extension_tab, &uri(A, "private.txt")));
    assert!(!scheme.allows(&extension_tab, &uri(A, "_zephium/background.html")));
    let stale_tab = Requester::Tab {
        committed_top_level: Url::parse("https://example.test/").unwrap(),
        extension_install: Some(1.into()),
    };
    assert!(!scheme.allows(&stale_tab, &uri(A, "public.txt")));
    scheme.remove_install(1.into());
    assert!(!scheme.allows(&own, &uri(A, "private.txt")));
    assert!(!scheme.allows(&extension_tab, &uri(A, "public.txt")));
}

#[test]
fn dynamic_aliases_and_extension_ids_are_independent_grants() {
    let (_directory, scheme) = fixture(json!([
        { "resources":["public*"], "matches":["https://example.test/*"], "use_dynamic_url":true },
        { "resources":["shared/*"], "extension_ids":[B] }
    ]));
    let web = tab("https://example.test/");
    assert!(!scheme.allows(&web, &uri(A, "public.txt")));
    assert!(scheme.allows(&web, &uri(ALIAS, "public.txt")));
    assert!(!scheme.allows(&tab("https://other.test/"), &uri(ALIAS, "public.txt")));
    assert!(!scheme.allows(&web, &uri(ALIAS, "private.txt")));
    assert!(!scheme.allows(&web, &uri(A, "shared/code.js")));
    let other = Requester::ExtensionOwned { install: 2.into() };
    assert!(scheme.allows(&other, &uri(A, "shared/code.js")));
    assert!(!scheme.allows(&other, &uri(A, "public.txt")));
    scheme.remove_install(2.into());
    assert!(!scheme.allows(&other, &uri(A, "shared/code.js")));
    scheme.retire();
    assert!(!scheme.allows(&web, &uri(ALIAS, "public.txt")));
}

#[test]
fn wildcard_war_cannot_expose_runtime_or_favicon() {
    let (_directory, scheme) =
        fixture(json!([{ "resources":["*"], "matches":["<all_urls>"], "extension_ids":["*"] }]));
    for requester in [
        tab("https://example.test/"),
        Requester::ExtensionOwned { install: 2.into() },
    ] {
        for path in [
            "_zephium/background.html",
            "%5fzephium/secret",
            "_favicon/icon.png",
        ] {
            assert!(!scheme.allows(&requester, &uri(A, path)));
        }
    }
    assert!(scheme
        .set_runtime_resource(
            1.into(),
            "/_zephium/background.html",
            MemoryResource::new(vec![1], "text/html").unwrap()
        )
        .is_ok());
    assert!(scheme
        .set_runtime_resource(
            1.into(),
            "/disk.html",
            MemoryResource::new(vec![1], "text/html").unwrap()
        )
        .is_err());
    assert!(MemoryResource::new(vec![0; MAX_MEMORY_BYTES + 1], "text/plain").is_err());
    assert!(MemoryResource::new(vec![], "text/plain\r\nInjected: true").is_err());
}

#[test]
fn malformed_war_is_refused_instead_of_broadened() {
    for value in [
        json!(["*"]),
        json!([{"resources":["*"],"matches":["https://example.test/private/*"]}]),
        json!([{"resources":["*"],"matches":["*" ]}]),
        json!([{"resources":["*"],"extension_ids":["invalid"]}]),
        json!([{"resources":["*"],"extension_ids":["*", B]}]),
        json!([{"resources":["../secret"],"matches":["<all_urls>"]}]),
        json!([{"resources":["*"],"matches":["<all_urls>"],"use_dynamic_url":"true"}]),
        json!([{"resources":["*"],"matches":[]}]),
    ] {
        assert!(WarEntry::parse(&Manifest::from_value(
            json!({"web_accessible_resources":value})
        ))
        .is_err());
    }
}

#[test]
fn removal_and_retirement_cancel_existing_stream_reads() {
    for profile in [false, true] {
        let active = Arc::new(AtomicBool::new(true));
        let install = Arc::new(AtomicBool::new(true));
        let streams = Arc::new(std::sync::atomic::AtomicUsize::new(1));
        let mut reader = RevocableReader {
            body: Box::new(io::Cursor::new(vec![7; 100])),
            profile_active: active.clone(),
            install_active: install.clone(),
            streams: streams.clone(),
            head: false,
        };
        let mut buffer = [0; 10];
        assert_eq!(reader.read(&mut buffer).unwrap(), 10);
        if profile {
            active.store(false, Ordering::Release);
        } else {
            install.store(false, Ordering::Release);
        }
        assert_eq!(
            reader.read(&mut buffer).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        drop(reader);
        assert_eq!(streams.load(Ordering::Acquire), 0);
    }
}

#[test]
fn new_admission_rotates_alias_and_rejects_duplicate_identity() {
    let (directory, scheme) = fixture(json!([]));
    let manifest = Manifest::from_value(json!({"manifest_version":3}));
    assert!(scheme
        .add_install(3.into(), A, directory.path(), &manifest)
        .is_err());
    assert!(scheme
        .add_install(
            1.into(),
            "dddddddddddddddddddddddddddddddd",
            directory.path(),
            &manifest
        )
        .is_err());
    scheme.remove_install(1.into());
    let alias = scheme
        .add_install(3.into(), A, directory.path(), &manifest)
        .unwrap();
    assert!(valid_id(&alias));
    assert_ne!(alias, ALIAS);
    assert!(!scheme.allows(
        &Requester::ExtensionOwned { install: 1.into() },
        &uri(A, "public.txt")
    ));
    scheme.retire();
    assert!(matches!(
        scheme.add_install(4.into(), A, directory.path(), &manifest),
        Err(SchemeError::Retired)
    ));
}
