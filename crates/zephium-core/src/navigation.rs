use url::Url;

const SEARCH_BASE: &str = "https://duckduckgo.com/";
const MAX_URL_BYTES: usize = 8 * 1024;
const RESERVED_HOSTS: &[&str] = &["asset.localhost", "ipc.localhost", "tauri.localhost"];

/// Turn raw omnibox text into an allowed URL. Explicit but forbidden or
/// malformed URL-like input is rejected instead of being sent to the search
/// provider: a local file path or password-bearing URL must not become a
/// network query merely because native navigation policy blocks it.
pub fn classify(input: &str) -> Option<Url> {
    let s = input.trim();
    if s.is_empty() {
        return Url::parse("about:blank").ok();
    }
    match classify_input(s) {
        InputKind::Direct(url) | InputKind::Search(url) => Some(url),
        InputKind::Rejected => None,
    }
}

/// Whether the omnibox input will be treated as a web search rather than a URL.
pub fn is_query(input: &str) -> bool {
    let s = input.trim();
    !s.is_empty() && matches!(classify_input(s), InputKind::Search(_))
}

/// Converts an explicit browser-search request into the configured provider
/// URL without reinterpreting host-shaped text as navigation. Dangerous
/// absolute schemes, credentials, and local paths retain the omnibox's
/// exfiltration guard even when supplied by an extension search API.
pub fn search_query(input: &str) -> Option<Url> {
    let value = input.trim();
    if value.is_empty() || looks_like_local_path(value) {
        return None;
    }
    if value.contains("://") || Url::parse(value).is_ok() {
        let parsed = Url::parse(value).ok()?;
        if !is_allowed(&parsed) {
            return None;
        }
    }
    search_url(value)
}

enum InputKind {
    Direct(Url),
    Search(Url),
    Rejected,
}

fn classify_input(s: &str) -> InputKind {
    // Path-shaped input is commonly pasted into an omnibox by mistake. Never
    // disclose it to a remote search provider. Explicit file: URLs are caught
    // by the absolute-URL branch below; this handles native paths that the URL
    // parser intentionally does not recognize as URLs.
    if looks_like_local_path(s) {
        return InputKind::Rejected;
    }
    if looks_like_host(s) {
        return Url::parse(&format!("https://{s}"))
            .ok()
            .filter(is_allowed)
            .map_or(InputKind::Rejected, InputKind::Direct);
    }

    // A syntactically absolute input expresses an intent to navigate, not to
    // search. This includes schemes without `//` such as file:, data: and
    // javascript:. Reject every disallowed absolute URL without exfiltrating
    // its original text to SEARCH_BASE. A malformed `scheme://` is treated the
    // same way because it may still contain credentials or a local path.
    if s.contains("://") || Url::parse(s).is_ok() {
        return Url::parse(s)
            .ok()
            .filter(is_browser_target)
            .map_or(InputKind::Rejected, InputKind::Direct);
    }

    search_url(s).map_or(InputKind::Rejected, InputKind::Search)
}

fn search_url(value: &str) -> Option<Url> {
    let mut url = Url::parse(SEARCH_BASE).ok()?;
    url.query_pairs_mut().append_pair("q", value);
    // Percent-encoding can expand an otherwise bounded input. Apply the
    // native URL ceiling to the final request.
    is_allowed(&url).then_some(url)
}

fn looks_like_local_path(s: &str) -> bool {
    s.starts_with('/')
        || s.starts_with("~/")
        || s.starts_with("./")
        || s.starts_with("../")
        || s.starts_with('\\')
        || matches!(
            s.as_bytes(),
            [drive, b':', b'\\' | b'/', ..] if drive.is_ascii_alphabetic()
        )
}

/// Whether a URL may commit. Blocks file, javascript, internal and external app
/// schemes; only http(s) and exact `about:blank` pass. Internal pseudo-hosts
/// and credentials are rejected even if a platform happens to expose them as
/// http(s).
pub fn is_allowed(url: &Url) -> bool {
    if url.as_str() == "about:blank" {
        return true;
    }
    matches!(url.scheme(), "http" | "https")
        && url.as_str().len() <= MAX_URL_BYTES
        && url.username().is_empty()
        && url.password().is_none()
        && url
            .host_str()
            .is_some_and(|host| !RESERVED_HOSTS.contains(&host))
}

/// Gate for page-initiated navigations (the engine hands us a resolved URL).
pub fn is_allowed_str(url: &str) -> bool {
    Url::parse(url).map(|u| is_allowed(&u)).unwrap_or(false)
}

/// Typed text read only as a web address, as when bookmarking one by hand:
/// `example.com/docs` is the page, while words that would be searched for in
/// the address field are not an address at all.
pub fn web_address(input: &str) -> Option<Url> {
    let input = input.trim();
    if input.is_empty() || input.len() > MAX_URL_BYTES {
        return None;
    }
    match classify_input(input) {
        InputKind::Direct(url) if matches!(url.scheme(), "http" | "https") && is_allowed(&url) => {
            Some(url)
        }
        _ => None,
    }
}

/// Addresses one hand-off from another application may carry.
pub const MAX_EXTERNAL_TARGETS: usize = 16;

/// An address handed to Zephium by another application: a clicked link, or
/// the command line of a launch. Only ordinary web pages are admitted, so a
/// hand-off can never open an internal page, run script, or read a file.
pub fn external_target(argument: &str) -> Option<Url> {
    if argument.len() > MAX_URL_BYTES {
        return None;
    }
    Url::parse(argument.trim())
        .ok()
        .filter(|url| matches!(url.scheme(), "http" | "https") && is_allowed(url))
}

/// A document a page builds for one of its own frames: an `about:srcdoc` or
/// `about:blank` frame, or one from `data:` or `blob:`. The engine keeps
/// these in the page's own origin rules; they are never a top-level page.
pub fn is_subframe_document(target: &str) -> bool {
    if target.len() > MAX_URL_BYTES {
        return false;
    }
    Url::parse(target).is_ok_and(|url| match url.scheme() {
        "about" => matches!(url.path(), "blank" | "srcdoc"),
        "data" | "blob" => true,
        _ => false,
    })
}

/// Schemes a page never hands to another application: they load browser
/// content, run script, reach files or shares, or reach Windows handlers
/// that have been used to run code from a link.
const NOT_FOR_APPS: &[&str] = &[
    "about",
    "afp",
    "asset",
    "blob",
    "chrome",
    "chrome-extension",
    "data",
    "disk",
    "disks",
    "edge",
    "file",
    "filesystem",
    "ftp",
    "hcp",
    "http",
    "https",
    "ie.http",
    "intent",
    "ipc",
    "javascript",
    "mk",
    "ms-appinstaller",
    "ms-cxh",
    "ms-cxh-full",
    "ms-help",
    "ms-its",
    "ms-msdt",
    "ms-officecmd",
    "ms-search",
    "ms-settings",
    "nfs",
    "nntp",
    "res",
    "safari-web-extension",
    "search",
    "search-ms",
    "shell",
    "smb",
    "tauri",
    "vbscript",
    "view-source",
    "vnd.ms.radio",
    "webkit-extension",
    "ws",
    "wss",
    "x-apple-systempreferences",
    "zephium",
];

/// A link meant for an application on this computer, such as `zoommtg:`,
/// `mailto:` or `slack:`. It never loads in a tab; the person decides
/// whether the application it names may open it.
pub fn external_app_link(target: &str) -> Option<Url> {
    if target.len() > MAX_URL_BYTES {
        return None;
    }
    let url = Url::parse(target).ok()?;
    let scheme = url.scheme();
    (scheme.len() <= 64
        && !NOT_FOR_APPS.contains(&scheme)
        && !scheme.starts_with("zephium")
        && url.username().is_empty()
        && url.password().is_none())
    .then_some(url)
}

/// Syntactic browser-tab target. Windows extension documents additionally
/// require the engine's live, profile-specific installation grant. This is not
/// permission to load an extension or to expose its resources to web pages.
pub fn is_browser_target(url: &Url) -> bool {
    is_allowed(url) || (cfg!(target_os = "windows") && extension_document_id(url).is_some())
}

pub fn is_browser_target_str(target: &str) -> bool {
    Url::parse(target).is_ok_and(|url| is_browser_target(&url))
}

pub fn extension_document_id(url: &Url) -> Option<&str> {
    let id = url.host_str()?;
    (url.scheme() == "chrome-extension"
        && id.len() == 32
        && id.bytes().all(|byte| (b'a'..=b'p').contains(&byte))
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none()
        && url.as_str().len() <= MAX_URL_BYTES)
        .then_some(id)
}

fn looks_like_host(s: &str) -> bool {
    if s.contains(char::is_whitespace) {
        return false;
    }
    if s == "localhost" || s.starts_with("localhost:") || s.starts_with("localhost/") {
        return true;
    }
    let authority = s.split(['/', '?', '#']).next().unwrap_or(s);
    // `user@host` in bare input is an email or a `trusted.com@evil.com`
    // spoof; both belong in search, never in the address.
    if authority.contains('@') {
        return false;
    }
    let host = authority.split(':').next().unwrap_or(authority);
    if host.parse::<std::net::Ipv4Addr>().is_ok() {
        return true;
    }
    matches!(host.rsplit_once('.'), Some((label, tld)) if !label.is_empty() && tld.len() >= 2)
}

#[cfg(test)]
mod tests {
    #[test]
    fn frames_may_hold_documents_their_page_builds() {
        for frame in [
            "about:srcdoc",
            "about:blank#top",
            "data:text/html,<p>hi</p>",
            "blob:https://challenges.example/0f1e",
        ] {
            assert!(super::is_subframe_document(frame), "{frame}");
        }
        for frame in [
            "about:settings",
            "javascript:alert(1)",
            "file:///etc/passwd",
            "zoommtg://x",
        ] {
            assert!(!super::is_subframe_document(frame), "{frame}");
        }
    }

    #[test]
    fn only_application_links_are_handed_to_applications() {
        for link in [
            "zoommtg://zoom.us/join?confno=123",
            "mailto:someone@example.com",
            "msteams:/l/meetup-join/1",
            "slack://open",
            "tel:+15551234",
        ] {
            assert!(super::external_app_link(link).is_some(), "{link}");
        }
        for link in [
            "https://zoom.us/j/1",
            "http://example.com/",
            "about:blank",
            "javascript:alert(1)",
            "data:text/html,hi",
            "file:///etc/passwd",
            "blob:https://example.com/1",
            "ms-msdt:/id PCWDiagnostic",
            "search-ms:query=x",
            "smb://host/share",
            "zephium://settings",
            "chrome-extension://abc/page.html",
            "mailto://user:secret@example.com",
            "not a url",
        ] {
            assert!(super::external_app_link(link).is_none(), "{link}");
        }
    }

    use super::*;
    use proptest::prelude::*;

    #[test]
    fn bare_host_gets_https() {
        assert_eq!(
            classify("example.com").unwrap().as_str(),
            "https://example.com/"
        );
        assert_eq!(
            classify("  github.com  ").unwrap().as_str(),
            "https://github.com/"
        );
        assert_eq!(
            classify("sub.example.com:8080/x").unwrap().scheme(),
            "https"
        );
    }

    #[test]
    fn explicit_url_passes_through() {
        assert_eq!(
            classify("https://x.com/a").unwrap().as_str(),
            "https://x.com/a"
        );
    }

    #[test]
    fn is_query_splits_urls_from_text() {
        assert!(is_query("hello world"));
        assert!(!is_query("example.com"));
        assert!(!is_query("https://x.com/a"));
        assert!(!is_query("file:///etc/passwd"));
        assert!(!is_query("tauri.localhost/index.html"));
        assert!(!is_query(""));
    }

    #[test]
    fn text_becomes_search() {
        let u = classify("hello world").unwrap();
        assert_eq!(u.host_str(), Some("duckduckgo.com"));
        assert_eq!(u.query(), Some("q=hello+world"));
    }

    #[test]
    fn explicit_search_never_turns_host_shaped_text_into_navigation() {
        assert_eq!(
            search_query("example.com").unwrap().as_str(),
            "https://duckduckgo.com/?q=example.com"
        );
        assert!(search_query("https://user:secret@example.com/").is_none());
        assert!(search_query("file:///private.txt").is_none());
        assert!(search_query("/Users/alice/private.txt").is_none());
    }

    #[test]
    fn percent_expanded_search_must_fit_the_native_url_ceiling() {
        assert!(classify(&"x".repeat(MAX_URL_BYTES)).is_none());
        assert!(classify(&"💣".repeat(MAX_URL_BYTES / 4)).is_none());

        let accepted = classify(&"x".repeat(1024)).expect("small search");
        assert!(accepted.as_str().len() <= MAX_URL_BYTES);
        assert!(is_allowed(&accepted));
    }

    #[test]
    fn scheme_gate_blocks_dangerous() {
        assert!(!is_allowed(&Url::parse("javascript:alert(1)").unwrap()));
        assert!(!is_allowed(&Url::parse("file:///etc/passwd").unwrap()));
        assert!(!is_allowed(&Url::parse("about:config").unwrap()));
        assert!(!is_allowed(&Url::parse("about:srcdoc").unwrap()));
        assert!(!is_allowed(
            &Url::parse("https://user:secret@example.com/").unwrap()
        ));
        assert!(!is_allowed(
            &Url::parse("http://tauri.localhost/index.html").unwrap()
        ));
        assert!(is_allowed(&Url::parse("https://example.com").unwrap()));
        assert!(is_allowed(&Url::parse("about:blank").unwrap()));
        assert!(is_allowed_str("https://x.com/"));
        assert!(!is_allowed_str("javascript:1"));
        assert!(!is_allowed_str("not a url"));
    }

    #[test]
    fn omnibox_rejects_explicit_forbidden_targets_without_searching() {
        for input in [
            "javascript:alert(1)",
            "file:///etc/passwd",
            "data:text/html,secret",
            "about:config",
            "tauri.localhost/index.html",
            "https://user:secret@example.com/",
            "https://",
            "/home/alice/private.txt",
            "~/private.txt",
            "./private.txt",
            "../private.txt",
            r"C:\Users\alice\private.txt",
            r"C:/Users/alice/private.txt",
            r"\\server\share\private.txt",
        ] {
            assert!(classify(input).is_none(), "accepted {input}");
            assert!(!is_query(input), "searched {input}");
        }
    }

    #[test]
    fn userinfo_input_searches_instead_of_spoofing() {
        assert_eq!(
            classify("paypal.com@evil.com").unwrap().host_str(),
            Some("duckduckgo.com")
        );
        assert_eq!(
            classify("someone@example.com").unwrap().host_str(),
            Some("duckduckgo.com")
        );
        assert!(is_query("paypal.com@evil.com"));
        assert!(classify("https://user:secret@example.com/").is_none());
    }

    #[test]
    fn ipv4_literals_navigate() {
        assert_eq!(
            classify("192.168.1.1").unwrap().as_str(),
            "https://192.168.1.1/"
        );
        assert_eq!(
            classify("192.168.1.1:8080/admin").unwrap().as_str(),
            "https://192.168.1.1:8080/admin"
        );
        // not a valid address: searched, not navigated
        assert!(is_query("999.1.1.1"));
    }

    proptest! {
        #[test]
        fn classify_never_panics(s in "\\PC*") {
            let _ = classify(&s);
        }
    }

    #[test]
    fn hand_offs_admit_only_ordinary_web_pages() {
        assert_eq!(
            external_target(" https://example.com/a?b=1 ").map(|url| url.to_string()),
            Some("https://example.com/a?b=1".into())
        );
        assert!(external_target("http://example.com").is_some());
        for refused in [
            "javascript:alert(1)",
            "data:text/html,hi",
            "file:///etc/passwd",
            "about:blank",
            "zephium://settings",
            "https://user:pass@example.com/",
            "/Applications/Zephium.app",
            "--flag",
            "",
        ] {
            assert_eq!(external_target(refused), None, "{refused}");
        }
        assert_eq!(
            external_target(&format!(
                "https://example.com/{}",
                "a".repeat(MAX_URL_BYTES)
            )),
            None
        );
    }

    #[test]
    fn a_typed_web_address_is_never_a_search() {
        assert_eq!(
            web_address(" example.com/docs ").map(|url| url.to_string()),
            Some("https://example.com/docs".into())
        );
        assert!(web_address("https://a.example/").is_some());
        for refused in [
            "",
            "what is rust",
            "about:blank",
            "file:///etc/hosts",
            "javascript:1",
        ] {
            assert!(web_address(refused).is_none(), "{refused}");
        }
    }
}
