use super::*;

#[test]
fn application_selection_is_closed_and_sessions_do_not_alias_product() {
    for identifier in [
        "app.zephium",
        "app.zephium.dev",
        "app.zephium.review-qa",
        "app.zephium.webext-qa",
        "app.zephium.files-integration-qa",
        "app.zephium.performance",
        "app.zephium.protection-qa",
        "app.zephium.work-integration",
        "app.zephium.work-navigation-probe",
        "app.zephium.work-rendering-probe",
    ] {
        assert!(WindowsWorkStorage::for_application(identifier, None).is_ok());
        assert_eq!(
            WindowsWorkStorage::for_application(identifier, Some("fixture-1")).is_ok(),
            identifier == "app.zephium.webext-qa"
        );
    }
    assert!(WindowsWorkStorage::for_application("app.zephium.other", None).is_err());
    assert!(
        WindowsWorkStorage::for_application("app.zephium.webext-qa", Some("../product")).is_err()
    );
}

#[test]
fn installation_is_lazy_and_legacy_purge_has_no_protected_side_effect() {
    let ordinary = tempfile::tempdir().unwrap();
    let selector = WindowsWorkStorage::for_application("app.zephium", None).unwrap();
    let hub = Hub::open_with_windows_work_storage(ordinary.path().into(), selector).unwrap();
    assert!(hub.windows_work_database.is_none());
    assert!(hub.work.is_none());
    let mut legacy = Hub::in_memory().unwrap();
    assert!(legacy.purge_windows_work_artifacts(1_u128.into()).is_ok());
}

#[cfg(all(debug_assertions, feature = "windows-namespace-validation"))]
fn fixture() -> (tempfile::TempDir, tempfile::TempDir, Hub) {
    let profile = tempfile::tempdir().unwrap();
    let ordinary = tempfile::tempdir().unwrap();
    let selector = WindowsWorkStorage::for_validation_fixture(
        profile.path(),
        "app.zephium.work-integration",
        None,
    )
    .unwrap();
    let hub = Hub::open_with_windows_work_storage(ordinary.path().into(), selector).unwrap();
    (profile, ordinary, hub)
}

#[test]
#[cfg(all(debug_assertions, feature = "windows-namespace-validation"))]
fn protected_schema_refuses_injected_objects_before_writable_reopen() {
    let (_profile, _ordinary, mut hub) = fixture();
    let selector = hub.windows_work_storage.clone().unwrap();
    let database = hub.windows_work_database().unwrap();
    database
        .connection
        .execute_batch("CREATE TABLE unexpected(value TEXT)")
        .unwrap();
    let path = database.anchor.database_path();
    drop(hub);
    let before = std::fs::read(&path).unwrap();
    assert!(WindowsWorkDatabase::open(&selector).is_err());
    assert_eq!(std::fs::read(path).unwrap(), before);
    let mut reopened =
        Hub::open_with_windows_work_storage(_ordinary.path().into(), selector).unwrap();
    assert!(reopened
        .purge_windows_work_artifacts(1_u128.into())
        .is_err());
}
