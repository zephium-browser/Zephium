use url::Url;
use zephium_miniapps::{
    app_id, parse_manifest, MiniAppError, Registry, MAX_INSTALLED_APPS, MAX_REGISTRY_BYTES,
};

fn app(start: &str) -> zephium_miniapps::MiniApp {
    let manifest = format!(r#"{{"name":"{start}","start_url":"{start}"}}"#);
    let base = Url::parse("https://example.com/manifest.json").unwrap();
    let start_url = Url::parse(start).unwrap();
    let manifest_url = if start_url.host_str() == base.host_str() {
        base
    } else {
        start_url.clone()
    };
    parse_manifest(manifest.as_bytes(), &manifest_url, 7).unwrap()
}

#[test]
fn registry_round_trips_and_replaces_by_identity() {
    let first = app("https://one.example/");
    let second = app("https://two.example/");
    let mut registry = Registry::new();
    registry.add(first.clone()).unwrap();
    registry.add(second.clone()).unwrap();
    let json = registry.to_json().unwrap();
    let restored = Registry::from_json(&json).unwrap();
    assert_eq!(restored.apps(), &[first.clone(), second.clone()]);

    let mut replacement = first.clone();
    replacement.name = "Renamed".into();
    registry.add(replacement).unwrap();
    assert_eq!(registry.apps().len(), 2);
    assert_eq!(registry.apps()[0].name, "Renamed");
}

#[test]
fn removal_is_explicit_about_a_missing_app() {
    let mut registry = Registry::new();
    assert_eq!(registry.remove("nope").unwrap_err(), MiniAppError::NotFound);
    let installed = app("https://one.example/");
    registry.add(installed.clone()).unwrap();
    assert_eq!(registry.remove(&installed.id).unwrap(), installed);
    assert!(registry.apps().is_empty());
}

#[test]
fn the_installation_set_is_bounded() {
    let mut registry = Registry::new();
    for index in 0..MAX_INSTALLED_APPS {
        registry
            .add(app(&format!("https://site-{index}.example/")))
            .unwrap();
    }
    assert_eq!(
        registry
            .add(app("https://one-too-many.example/"))
            .unwrap_err(),
        MiniAppError::RegistryFull
    );
}

#[test]
fn corrupt_foreign_or_duplicated_documents_are_refused() {
    assert_eq!(
        Registry::from_json("[not an object]").unwrap_err(),
        MiniAppError::InvalidRegistry
    );
    let duplicate = app("https://one.example/");
    let mut registry = Registry::new();
    registry.add(duplicate.clone()).unwrap();
    let mut value: serde_json::Value = serde_json::from_str(&registry.to_json().unwrap()).unwrap();
    let apps = value["apps"].as_array_mut().unwrap();
    apps.push(serde_json::to_value(&duplicate).unwrap());
    assert_eq!(
        Registry::from_json(&value.to_string()).unwrap_err(),
        MiniAppError::InvalidRegistry
    );
    value["version"] = serde_json::json!(99);
    assert_eq!(
        Registry::from_json(&value.to_string()).unwrap_err(),
        MiniAppError::InvalidRegistry
    );
    assert_eq!(
        Registry::from_json(&"x".repeat(MAX_REGISTRY_BYTES + 1)).unwrap_err(),
        MiniAppError::RegistryTooLarge
    );
    assert_eq!(Registry::from_json("   ").unwrap().apps().len(), 0);
}

#[test]
fn restoration_rejects_credentialed_start_urls_even_with_a_matching_id() {
    let mut installed = app("https://example.com/");
    installed.start_url = "https://user:secret@example.com/".into();
    installed.id = app_id(&installed.start_url);
    let mut registry = Registry::new();
    registry.add(installed).unwrap();
    assert_eq!(
        Registry::from_json(&registry.to_json().unwrap()).unwrap_err(),
        MiniAppError::InvalidRegistry
    );
}
