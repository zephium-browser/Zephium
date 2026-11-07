use super::*;
use zephium_core::ids::ExtensionInstallId;
use zephium_core::ports::engine::{WebExtensionLoad, WebExtensionLoaded};

fn load(seed: u128) -> WebExtensionLoad {
    WebExtensionLoad {
        install: ExtensionInstallId::from(seed),
        extension_id: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
        root: format!("/fixture/{seed}").into(),
        external_native_messaging: false,
        native_messaging_grantable: false,
        permissions: vec![],
        match_patterns: vec![],
        start_background: false,
    }
}

#[test]
fn capacity_admission_is_stable_and_retries_after_disable_or_remove() {
    for remove in [false, true] {
        let (mut shell, engine, screen) = setup();
        shell.handle(Command::Bootstrap);
        let profile = shell.profile_of_item(active_id(&screen)).unwrap();
        let loads: Vec<_> = (1..=9).rev().map(load).collect();
        shell.set_web_extensions(profile, loads.clone());
        let calls: Vec<_> = engine
            .calls()
            .into_iter()
            .filter(|c| c.starts_with("extension-load"))
            .collect();
        assert_eq!(calls.len(), 8);
        for (index, call) in calls.iter().enumerate() {
            assert!(call.ends_with(&ExtensionInstallId::from(index as u128 + 1).to_string()));
        }
        assert!(shell
            .web_extension_status(profile)
            .iter()
            .any(|(id, status)| *id == ExtensionInstallId::from(9)
                && matches!(status, super::super::webext::WebExtensionStatus::Failed(_))));
        if remove {
            shell.remove_web_extension(profile, load(1));
        } else {
            shell.set_web_extensions(
                profile,
                loads
                    .into_iter()
                    .filter(|l| l.install != ExtensionInstallId::from(1))
                    .collect(),
            );
        }
        let calls: Vec<_> = engine
            .calls()
            .into_iter()
            .filter(|c| c.starts_with("extension-load"))
            .collect();
        assert_eq!(calls.len(), 9);
        assert!(calls
            .last()
            .unwrap()
            .ends_with(&ExtensionInstallId::from(9).to_string()));
    }
}

#[test]
fn typed_extension_tab_close_requires_running_install_in_its_profile() {
    let (mut shell, _, screen) = setup();
    shell.handle(Command::Bootstrap);
    let tab = active_id(&screen);
    let profile = shell.profile_of_item(tab).unwrap();
    shell.set_web_extensions(profile, vec![load(1)]);
    // A failed or still-loading extension does not grant shell close authority.
    let target = "chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/options.html";
    shell
        .items
        .set_committed_url(tab, url::Url::parse(target).unwrap());
    shell.close_owned_native_tab(tab);
    assert!(shell.items.get(tab).is_some());
    shell.on_web_extension_settled(
        profile,
        ExtensionInstallId::from(1),
        Ok(WebExtensionLoaded {
            name: "fixture".into(),
            version: "1".into(),
        }),
    );
    shell.close_owned_native_tab(tab);
    assert!(shell.items.get(tab).is_none());
}

#[test]
fn replacing_at_capacity_keeps_its_slot_and_does_not_admit_the_waiter() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let profile = shell.profile_of_item(active_id(&screen)).unwrap();
    shell.set_web_extensions(profile, (1..=9).map(load).collect());
    let mut updated: Vec<_> = (1..=9).map(load).collect();
    updated[0].root = "/updated/1".into();
    shell.set_web_extensions(profile, updated);
    let calls: Vec<_> = engine
        .calls()
        .into_iter()
        .filter(|c| c.starts_with("extension-load"))
        .collect();
    assert_eq!(calls.len(), 9);
    assert!(calls
        .last()
        .unwrap()
        .ends_with(&ExtensionInstallId::from(1).to_string()));
    assert!(matches!(
        shell
            .web_extension_status(profile)
            .into_iter()
            .find(|(id, _)| *id == ExtensionInstallId::from(9))
            .map(|(_, status)| status),
        Some(super::super::webext::WebExtensionStatus::Failed(_))
    ));
    shell.on_web_extension_settled(
        profile,
        ExtensionInstallId::from(1),
        Err("Invalid package".into()),
    );
    let calls: Vec<_> = engine
        .calls()
        .into_iter()
        .filter(|c| c.starts_with("extension-load"))
        .collect();
    assert_eq!(calls.len(), 10);
    assert!(calls
        .last()
        .unwrap()
        .ends_with(&ExtensionInstallId::from(9).to_string()));
}
