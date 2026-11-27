use url::Url;
use zephium_miniapps::{
    app_id, is_in_scope, parse_manifest, DisplayMode, MiniAppError, MAX_MANIFEST_BYTES,
};

const MANIFEST: &str = r##"{
  "name": "  Zephium   Mail ",
  "start_url": "/inbox/?source=install#top",
  "scope": "/",
  "display": "standalone",
  "theme_color": "#1A2B3C",
  "background_color": "#fff",
  "icons": [
    {"src": "/small.png", "sizes": "64x64", "type": "image/png"},
    {"src": "/large.png", "sizes": "512x512", "type": "image/png"},
    {"src": "data:image/png;base64,AAAA", "sizes": "1024x1024"}
  ]
}"##;

fn base() -> Url {
    Url::parse("https://mail.example.com/manifest.webmanifest").unwrap()
}

#[test]
fn parses_a_complete_manifest_and_canonicalises_it() {
    let app = parse_manifest(MANIFEST.as_bytes(), &base(), 1_700_000_000).unwrap();
    assert_eq!(app.name, "Zephium Mail");
    assert_eq!(
        app.start_url,
        "https://mail.example.com/inbox/?source=install"
    );
    assert_eq!(app.scope, "https://mail.example.com/");
    assert_eq!(app.display, DisplayMode::Standalone);
    assert_eq!(app.theme_color.as_deref(), Some("#1a2b3c"));
    assert_eq!(app.background_color.as_deref(), Some("#fff"));
    assert_eq!(
        app.icon.as_deref(),
        Some("https://mail.example.com/large.png")
    );
    assert_eq!(app.installed_at, 1_700_000_000);
    assert_eq!(app.id, app_id(&app.start_url));
}

#[test]
fn defaults_an_absent_scope_to_the_start_directory() {
    let manifest = r#"{"name":"Docs","start_url":"https://docs.example.com/guide/intro"}"#;
    let app = parse_manifest(manifest.as_bytes(), &base(), 0).unwrap();
    assert_eq!(app.scope, "https://docs.example.com/guide/");
    assert!(is_in_scope(
        &Url::parse("https://docs.example.com/guide/next").unwrap(),
        &Url::parse(&app.scope).unwrap()
    ));
    assert!(!is_in_scope(
        &Url::parse("https://docs.example.com/guidelines").unwrap(),
        &Url::parse(&app.scope).unwrap()
    ));
}

#[test]
fn refuses_insecure_or_credentialed_start_addresses() {
    for start in ["http://example.com/", "https://user:pass@example.com/"] {
        let manifest = format!(r#"{{"name":"X","start_url":"{start}"}}"#);
        let error = parse_manifest(manifest.as_bytes(), &base(), 0).unwrap_err();
        assert!(matches!(
            error,
            MiniAppError::InsecureUrl | MiniAppError::CredentialsInUrl
        ));
    }
}

#[test]
fn refuses_a_cross_origin_or_escaping_scope() {
    let cross =
        r#"{"name":"X","start_url":"https://example.com/app/","scope":"https://evil.example/"}"#;
    assert_eq!(
        parse_manifest(cross.as_bytes(), &base(), 0).unwrap_err(),
        MiniAppError::ScopeOutsideOrigin
    );
    let outside = r#"{"name":"X","start_url":"https://example.com/app/","scope":"/other/"}"#;
    let example = Url::parse("https://example.com/manifest.json").unwrap();
    assert_eq!(
        parse_manifest(outside.as_bytes(), &example, 0).unwrap_err(),
        MiniAppError::StartOutsideScope
    );
}

#[test]
fn refuses_missing_names_and_missing_start_addresses() {
    let missing_start = r#"{"name":"X"}"#;
    assert_eq!(
        parse_manifest(missing_start.as_bytes(), &base(), 0).unwrap_err(),
        MiniAppError::MissingStartUrl
    );
    let missing_name = r#"{"start_url":"https://example.com/"}"#;
    let app = parse_manifest(missing_name.as_bytes(), &base(), 0).unwrap();
    assert_eq!(app.name, "example.com");
    let control_name = "{\"name\":\"bad\\u0000name\",\"start_url\":\"https://example.com/\"}";
    let app = parse_manifest(control_name.as_bytes(), &base(), 0).unwrap();
    assert_eq!(app.name, "example.com");
}

#[test]
fn rejects_malformed_and_oversized_manifests() {
    assert_eq!(
        parse_manifest(b"{", &base(), 0).unwrap_err(),
        MiniAppError::InvalidJson
    );
    let oversized = vec![b' '; MAX_MANIFEST_BYTES + 1];
    assert_eq!(
        parse_manifest(&oversized, &base(), 0).unwrap_err(),
        MiniAppError::ManifestTooLarge
    );
}

#[test]
fn drops_unknown_display_modes_and_invalid_colours() {
    let manifest = r##"{
        "name":"X",
        "start_url":"https://example.com/",
        "display":"window-controls-overlay",
        "theme_color":"red",
        "background_color":"#12345"
    }"##;
    let app = parse_manifest(manifest.as_bytes(), &base(), 0).unwrap();
    assert_eq!(app.display, DisplayMode::Browser);
    assert!(app.theme_color.is_none());
    assert!(app.background_color.is_none());
}

#[test]
fn identities_are_stable_and_distinct() {
    let a = app_id("https://example.com/app");
    let b = app_id("https://example.com/app");
    let c = app_id("https://example.com/other");
    assert_eq!(a, b);
    assert_ne!(a, c);
    assert_eq!(a.len(), 32);
    assert!(a.bytes().all(|byte| byte.is_ascii_hexdigit()));
}
