use super::*;

pub(super) fn install_profile(shell: &mut Shell, profile: ProfileId, spaces: &[SpaceId]) {
    assert!(shell.profiles.insert(Profile {
        id: profile,
        name: "Extensions".into(),
        kind: ProfileKind::Default,
    }));
    assert!(shell.initialize_new_blocker_profile(profile));
    for &space in spaces {
        assert!(shell.spaces.insert(Space {
            id: space,
            profile,
            name: format!("Space {space}"),
            look: Default::default(),
        }));
    }
}

pub(super) fn activate_profile(shell: &mut Shell, profile: ProfileId) {
    let mut profiles = zephium_core::extensions::ExtensionActiveProfiles::EMPTY;
    assert!(profiles.try_insert(profile));
    assert!(shell
        .extension_browser_surfaces
        .replace_active_profiles(profiles));
}

#[test]
fn inert_shell_never_publishes_an_extension_browser_surface() {
    let (mut shell, engine, _) = setup();

    let settlement = shell.sync_extension_browser_surfaces();

    assert_eq!(settlement.native, NativeWork::default());
    assert!(engine.extension_browser_surfaces().is_empty());
}

#[test]
fn active_profile_publishes_all_logical_tabs_before_native_creation() {
    let (mut shell, engine, _) = setup();
    let profile = ProfileId::from(1);
    let first_space = SpaceId::from(10);
    let second_space = SpaceId::from(11);
    install_profile(&mut shell, profile, &[first_space, second_space]);
    engine.calls.lock().unwrap().clear();
    shell.windows.create(
        WindowKind::Main,
        profile,
        first_space,
        Size::new(1200.0, 800.0),
    );
    activate_profile(&mut shell, profile);

    let favorite = ItemId::from(1);
    let active = ItemId::from(2);
    let other_space = ItemId::from(3);
    assert!(shell
        .items
        .insert_tab(favorite, Placement::Favorites { space: first_space }));
    assert!(shell.items.insert_tab(
        active,
        Placement::Space {
            space: first_space,
            section: SpaceSection::Today,
        },
    ));
    assert!(shell.items.insert_tab(
        other_space,
        Placement::Space {
            space: second_space,
            section: SpaceSection::Pinned,
        },
    ));
    assert_eq!(shell.focus_tab(active), Vec::new());
    let effects = shell.items.navigate(active, "https://example.invalid/");

    let native = shell.apply(effects);

    assert!(native.scheduled);
    let calls = engine.calls();
    let surface_call = calls
        .iter()
        .position(|call| call.starts_with("extension-surface "))
        .unwrap();
    let create_call = calls
        .iter()
        .position(|call| call.starts_with(&format!("create {active} ")))
        .unwrap();
    assert!(surface_call < create_call, "native calls were {calls:?}");
    let surfaces = engine.extension_browser_surfaces();
    assert_eq!(surfaces.len(), 1);
    let first = &surfaces[0];
    assert_eq!(
        first.generation(),
        ExtensionBrowserSurfaceGeneration::INITIAL
    );
    assert_eq!(first.focused(), Some(1));
    let tabs = first.windows()[0].tabs();
    assert_eq!(
        tabs.iter().map(|tab| tab.id()).collect::<Vec<_>>(),
        vec![favorite, active, other_space]
    );
    assert!(!tabs[0].resident());
    assert!(tabs[1].resident());
    assert!(!tabs[2].resident());
    assert_eq!(tabs[0].title(), "New Tab");
    assert_eq!(tabs[1].title(), "New Tab");
    assert_eq!(tabs[2].title(), "New Tab");
    assert_eq!(tabs[0].url(), None);
    assert_eq!(tabs[1].url(), None);
    assert_eq!(tabs[2].url(), None);
    assert!(!tabs[0].loading());
    assert!(!tabs[1].loading());
    assert!(!tabs[2].loading());
    assert!(tabs[0].pinned());
    assert!(!tabs[1].pinned());
    assert!(tabs[2].pinned());

    let unchanged = shell.apply(Vec::new());
    assert_eq!(unchanged, NativeWork::default());
    assert_eq!(engine.extension_browser_surfaces().len(), 1);

    assert!(shell.items.mark_view_discarded(active));
    let discarded = shell.apply(Vec::new());
    assert!(discarded.scheduled);
    let surfaces = engine.extension_browser_surfaces();
    assert_eq!(surfaces.len(), 2);
    assert_eq!(
        surfaces[1].generation(),
        ExtensionBrowserSurfaceGeneration::new(2).unwrap()
    );
    assert!(surfaces[1].windows()[0].tabs()[1].discarded());
}

#[test]
fn rejected_surface_admission_is_retried_once_without_idle_rebuilds() {
    let (mut shell, engine, _) = setup();
    let profile = ProfileId::from(51_000);
    let space = SpaceId::from(51_001);
    install_profile(&mut shell, profile, &[space]);
    shell
        .windows
        .create(WindowKind::Main, profile, space, Size::new(1200.0, 800.0));
    activate_profile(&mut shell, profile);
    assert!(shell.items.insert_tab(
        ItemId::from(51_002),
        Placement::Space {
            space,
            section: SpaceSection::Today,
        },
    ));
    engine
        .reject_native_dispatch
        .store(true, std::sync::atomic::Ordering::Release);

    let rejected = shell.sync_extension_browser_surfaces();

    assert!(rejected.native.rejected);
    assert!(engine.extension_browser_surfaces().is_empty());

    engine
        .reject_native_dispatch
        .store(false, std::sync::atomic::Ordering::Release);
    let retried = shell.retry_extension_browser_surfaces();
    assert!(retried.native.scheduled);
    assert_eq!(engine.extension_browser_surfaces().len(), 1);

    let idle = shell.retry_extension_browser_surfaces();
    assert_eq!(idle.native, NativeWork::default());
    assert_eq!(engine.extension_browser_surfaces().len(), 1);
}

#[test]
fn final_runtime_retirement_publishes_an_empty_surface_and_releases_shell_state() {
    let (mut shell, engine, _) = setup();
    let profile = ProfileId::from(52_000);
    let space = SpaceId::from(52_001);
    install_profile(&mut shell, profile, &[space]);
    shell
        .windows
        .create(WindowKind::Main, profile, space, Size::new(1200.0, 800.0));
    activate_profile(&mut shell, profile);
    assert!(shell.items.insert_tab(
        ItemId::from(52_002),
        Placement::Space {
            space,
            section: SpaceSection::Today,
        },
    ));
    assert!(shell.sync_extension_browser_surfaces().native.scheduled);

    assert!(shell
        .extension_browser_surfaces
        .replace_active_profiles(zephium_core::extensions::ExtensionActiveProfiles::EMPTY));
    let retired = shell.sync_extension_browser_surfaces();

    assert!(retired.native.scheduled);
    assert!(!shell.extension_browser_surfaces.is_active(profile));
    let retired_surface = shell
        .extension_browser_surfaces
        .published_surface(profile)
        .expect("the empty generation remains as the monotonic reactivation floor");
    assert_eq!(
        retired_surface.generation(),
        ExtensionBrowserSurfaceGeneration::new(2).unwrap()
    );
    assert!(retired_surface.windows().is_empty());
    let surfaces = engine.extension_browser_surfaces();
    assert_eq!(surfaces.len(), 2);
    assert_eq!(
        surfaces[1].generation(),
        ExtensionBrowserSurfaceGeneration::new(2).unwrap()
    );
    assert!(surfaces[1].windows().is_empty());
}

#[test]
fn rejected_final_runtime_surface_retirement_is_bounded_and_retryable() {
    let (mut shell, engine, _) = setup();
    let profile = ProfileId::from(53_000);
    let space = SpaceId::from(53_001);
    install_profile(&mut shell, profile, &[space]);
    shell
        .windows
        .create(WindowKind::Main, profile, space, Size::new(1200.0, 800.0));
    activate_profile(&mut shell, profile);
    assert!(shell.items.insert_tab(
        ItemId::from(53_002),
        Placement::Space {
            space,
            section: SpaceSection::Today,
        },
    ));
    assert!(shell.sync_extension_browser_surfaces().native.scheduled);
    assert!(shell
        .extension_browser_surfaces
        .replace_active_profiles(zephium_core::extensions::ExtensionActiveProfiles::EMPTY));
    engine
        .reject_native_dispatch
        .store(true, std::sync::atomic::Ordering::Release);

    let refused = shell.sync_extension_browser_surfaces();
    assert!(refused.native.rejected);
    assert!(shell
        .extension_browser_surfaces
        .published_surface(profile)
        .is_some());

    engine
        .reject_native_dispatch
        .store(false, std::sync::atomic::Ordering::Release);
    let retried = shell.retry_extension_browser_surfaces();
    assert!(retried.native.scheduled);
    assert!(shell
        .extension_browser_surfaces
        .published_surface(profile)
        .is_some_and(|surface| surface.windows().is_empty()));
    assert!(engine
        .extension_browser_surfaces()
        .last()
        .unwrap()
        .windows()
        .is_empty());
    assert_eq!(
        shell.retry_extension_browser_surfaces().native,
        NativeWork::default()
    );
}

#[test]
fn reactivation_advances_past_the_empty_retirement_generation() {
    let (mut shell, engine, _) = setup();
    let profile = ProfileId::from(54_000);
    let space = SpaceId::from(54_001);
    install_profile(&mut shell, profile, &[space]);
    shell
        .windows
        .create(WindowKind::Main, profile, space, Size::new(1200.0, 800.0));
    activate_profile(&mut shell, profile);
    assert!(shell.items.insert_tab(
        ItemId::from(54_002),
        Placement::Space {
            space,
            section: SpaceSection::Today,
        },
    ));
    assert!(shell.sync_extension_browser_surfaces().native.scheduled);

    assert!(shell
        .extension_browser_surfaces
        .replace_active_profiles(zephium_core::extensions::ExtensionActiveProfiles::EMPTY));
    assert!(shell.sync_extension_browser_surfaces().native.scheduled);

    let mut active = zephium_core::extensions::ExtensionActiveProfiles::EMPTY;
    assert!(active.try_insert(profile));
    assert!(shell
        .extension_browser_surfaces
        .replace_active_profiles(active));
    assert!(shell.sync_extension_browser_surfaces().native.scheduled);

    let surfaces = engine.extension_browser_surfaces();
    assert_eq!(surfaces.len(), 3);
    assert_eq!(
        surfaces[2].generation(),
        ExtensionBrowserSurfaceGeneration::new(3).unwrap()
    );
    assert_eq!(surfaces[2].windows().len(), 1);
}

#[test]
fn ambiguous_multiwindow_ownership_still_opens_the_page() {
    let (mut shell, engine, _) = setup();
    let profile = ProfileId::from(1);
    let space = SpaceId::from(10);
    install_profile(&mut shell, profile, &[space]);
    engine.calls.lock().unwrap().clear();
    shell
        .windows
        .create(WindowKind::Main, profile, space, Size::new(1200.0, 800.0));
    shell
        .windows
        .create(WindowKind::Main, profile, space, Size::new(1200.0, 800.0));
    activate_profile(&mut shell, profile);
    let item = ItemId::from(1);
    assert!(shell.items.insert_tab(
        item,
        Placement::Space {
            space,
            section: SpaceSection::Today,
        },
    ));
    let effects = shell.items.navigate(item, "https://example.invalid/");

    shell.apply(effects);

    // Extensions cannot see an ambiguous window, but browsing never waits on them.
    assert!(shell.items.tab(item).unwrap().has_view());
    assert!(engine.extension_browser_surfaces().is_empty());
    assert!(engine
        .calls()
        .iter()
        .any(|call| call.starts_with("create ")));
}
