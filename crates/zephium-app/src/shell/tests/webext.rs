use super::*;
use zephium_core::ids::ExtensionInstallId;
use zephium_core::ports::engine::WebExtensionLoad;

fn load() -> WebExtensionLoad {
    WebExtensionLoad {
        install: ExtensionInstallId::from(1),
        extension_id: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
        root: "/fixture".into(),
        external_native_messaging: false,
        native_messaging_grantable: false,
        permissions: vec![],
        match_patterns: vec![],
        start_background: false,
    }
}

#[test]
fn package_preparation_failure_is_profile_scoped_and_survives_bootstrap() {
    let (mut shell, _, screen) = setup();
    shell.handle(Command::Bootstrap);
    let profile = shell.profile_of_item(active_id(&screen)).unwrap();
    let other = add_inactive_named_profile(&mut shell, 900);
    let failure = (
        load().install,
        "The extension package could not be prepared.".into(),
    );
    shell.bootstrapped = false;
    shell.handle(Command::SetWebExtensions {
        profile,
        extensions: vec![],
        failures: vec![failure.clone()],
        retry: None,
    });
    shell.bootstrapped = true;
    shell.apply_deferred_web_extensions();
    assert_eq!(
        shell.web_extension_status(profile),
        vec![(
            failure.0,
            super::super::webext::WebExtensionStatus::Failed(failure.1)
        )]
    );
    assert!(shell.web_extension_status(other).is_empty());
}

#[test]
fn late_preparation_never_loads_missing_private_or_deleting_profiles() {
    for state in ["missing", "private", "deleting", "deleted"] {
        let (mut shell, engine, screen) = setup();
        shell.handle(Command::Bootstrap);
        let active = shell.profile_of_item(active_id(&screen)).unwrap();
        let profile = ProfileId::from(900);
        if state != "missing" {
            shell.profiles.insert(Profile {
                id: profile,
                name: "Other".into(),
                kind: if state == "private" {
                    ProfileKind::Incognito
                } else {
                    ProfileKind::Named
                },
            });
        }
        if state == "deleting" {
            shell.profile_deletion.states.insert(
                profile,
                ProfileDeletionState::new(ProfileDeletionPhase::AwaitingAuthorization, None, 0),
            );
        }
        if state == "deleted" {
            shell.apply_profile_tombstone(profile);
        }
        let calls = engine.calls().len();
        shell.bootstrapped = false;
        shell.handle(Command::SetWebExtensions {
            profile,
            extensions: vec![load()],
            failures: vec![(load().install, "failed".into())],
            retry: None,
        });
        shell.bootstrapped = true;
        shell.apply_deferred_web_extensions();
        assert!(
            !engine.calls()[calls..]
                .iter()
                .any(|call| call.starts_with("extension-load")),
            "{state}"
        );
        assert!(shell.web_extension_status(profile).is_empty(), "{state}");
        assert!(shell.web_extension_status(active).is_empty());
    }
}

#[test]
fn retry_restarts_only_the_requested_installation() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let profile = shell.profile_of_item(active_id(&screen)).unwrap();
    let first = load();
    let mut second = first.clone();
    second.install = ExtensionInstallId::from(2);
    shell.set_web_extensions(profile, vec![first.clone(), second.clone()]);
    shell.on_web_extension_settled(profile, first.install, Err("failed first".into()));
    shell.on_web_extension_settled(profile, second.install, Err("failed second".into()));
    let before = engine
        .calls()
        .iter()
        .filter(|call| call.starts_with("extension-load"))
        .count();
    shell.handle(Command::SetWebExtensions {
        profile,
        extensions: vec![first.clone(), second.clone()],
        failures: vec![],
        retry: Some(first.install),
    });
    let after: Vec<_> = engine
        .calls()
        .into_iter()
        .filter(|call| call.starts_with("extension-load"))
        .collect();
    assert_eq!(after.len(), before + 1);
    assert!(after.last().unwrap().ends_with(&first.install.to_string()));
    assert!(shell.web_extension_status(profile).iter().any(|(id, status)| *id == second.install && matches!(status, super::super::webext::WebExtensionStatus::Failed(error) if error == "failed second")));
}

#[test]
fn final_install_target_validation_rejects_a_deleted_profile() {
    let (mut shell, _, screen) = setup();
    shell.handle(Command::Bootstrap);
    let profile = shell.profile_of_item(active_id(&screen)).unwrap();
    assert_eq!(shell.web_extension_target(None).unwrap().profile, profile);
    shell.apply_profile_tombstone(profile);
    assert!(shell.web_extension_target(None).is_none());
}

#[test]
fn the_store_button_answers_only_for_the_listing_in_front() {
    use zephium_core::ports::engine::{StoreButton, StoreButtonState};
    const LISTED: &str = "dbepggeogbaibhgnhhndojpepiihcmeb";
    let engine = Arc::new(FakeEngine::default());
    let clicks: Arc<Mutex<Vec<zephium_ipc::WebExtensionStoreClickView>>> = Arc::default();
    let screen: Screen = Arc::new(Mutex::new(ItemsState {
        projection_revision: String::new(),
        profile: None,
        spaces: Vec::new(),
        active_space_id: None,
        nodes: Vec::new(),
        tabs: Vec::new(),
        active: None,
        split_group: None,
    }));
    let (sink, view) = (clicks.clone(), screen.clone());
    let mut shell = Shell::new_with_failure(
        engine.clone(),
        Arc::new(FakeStore::default()),
        Arc::new(ImmediateAllowAllCompiler),
        Box::new(|_| {}),
        Arc::new(FakeChrome),
        Box::new(move |projection| match projection {
            Projection::WebExtensionStoreClick(click) => sink.lock().unwrap().push(click),
            other => apply_projection(&mut view.lock().unwrap(), other),
        }),
    );
    shell.handle(Command::SetWindowSize(Size::new(1200.0, 800.0)));
    shell.handle(Command::Bootstrap);
    let listing = active_id(&screen);
    commit_url(
        &mut shell,
        listing,
        &format!("https://chromewebstore.google.com/detail/vimium/{LISTED}"),
    );
    let show = |tab, extension: &str| Command::ShowStoreButton {
        tab,
        button: StoreButton {
            extension: extension.into(),
            state: StoreButtonState::Add,
            label: "Add to Zephium".into(),
        },
    };
    let click = |id, extension: &str| {
        Command::Engine(EngineEvent::StoreButtonClicked {
            id,
            extension: extension.into(),
        })
    };
    let other = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    shell.handle(show(listing, LISTED));
    shell.handle(show(listing, other));
    shell.handle(click(listing, other));
    shell.handle(click(listing, LISTED));
    shell.handle(Command::Run("tab.new".into()));
    shell.handle(show(listing, LISTED));
    shell.handle(click(listing, LISTED));

    let shown: Vec<String> = engine
        .calls()
        .into_iter()
        .filter(|call| call.starts_with("store-button"))
        .collect();
    assert_eq!(shown, [format!("store-button {listing} {LISTED} Add")]);
    assert_eq!(
        *clicks.lock().unwrap(),
        [zephium_ipc::WebExtensionStoreClickView {
            tab_id: listing.to_string(),
            extension_id: LISTED.into(),
        }]
    );
}
