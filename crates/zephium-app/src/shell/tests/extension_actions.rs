use super::extension_browser_surface::{activate_profile, install_profile};
use super::*;
use crate::shell::extension_actions::ExtensionActionInvocationObservation;
use zephium_core::extensions::{
    ExtensionActionIcon, ExtensionActionRevision, ExtensionActionScope, ExtensionActionSnapshot,
    ExtensionActionSnapshotSettlement, ExtensionActionState, ExtensionBrowserSurface,
    ExtensionBrowserTab, ExtensionBrowserWindow, ExtensionPopupAnchor, ExtensionRuntimeGeneration,
    ExtensionRuntimeInstance, EXTENSION_ACTION_ICON_RGBA_BYTES,
};
use zephium_core::ids::ExtensionInstallId;

#[test]
fn newest_exact_action_snapshot_replaces_and_failures_retain() {
    let (mut shell, engine, _) = setup();
    shell.bootstrapped = true;
    let profile = ProfileId::from(71_000);
    let space = SpaceId::from(71_001);
    let tab = ItemId::from(71_002);
    install_profile(&mut shell, profile, &[space]);
    shell
        .windows
        .create(WindowKind::Main, profile, space, Size::new(1200.0, 800.0));
    activate_profile(&mut shell, profile);
    assert!(shell.items.insert_tab(
        tab,
        Placement::Space {
            space,
            section: SpaceSection::Today,
        },
    ));
    assert_eq!(shell.focus_tab(tab), Vec::new());
    assert!(shell.sync_extension_browser_surfaces().native.scheduled);
    let generation = shell
        .extension_browser_surfaces
        .published_surface(profile)
        .unwrap()
        .generation();
    let runtime = ExtensionRuntimeInstance::new(
        profile,
        ExtensionInstallId::from(1),
        ExtensionRuntimeGeneration::INITIAL,
    );
    let action = ExtensionActionState::new(
        runtime,
        ExtensionActionScope::Tab(tab),
        ExtensionActionRevision::INITIAL,
        "Bitwarden",
        "",
        Some(ExtensionActionIcon::from_rgba(vec![7; EXTENSION_ACTION_ICON_RGBA_BYTES]).unwrap()),
        true,
        true,
        false,
    )
    .unwrap();
    let snapshot = ExtensionActionSnapshot::new(profile, tab, generation, vec![action]).unwrap();

    shell.handle(Command::Engine(
        EngineEvent::ExtensionActionsSnapshotSettled {
            profile,
            tab,
            surface_generation: generation,
            settlement: ExtensionActionSnapshotSettlement::Applied(snapshot.clone()),
        },
    ));
    assert_eq!(shell.extension_actions.snapshot(profile), Some(&snapshot));
    let resident_surface = ExtensionBrowserSurface::new(
        profile,
        generation,
        Some(1),
        vec![ExtensionBrowserWindow::new(
            1,
            false,
            Some(tab),
            vec![ExtensionBrowserTab::from_snapshot(
                None,
                tab,
                true,
                "Bitwarden test",
                Some(&url::Url::parse("https://example.test/").unwrap()),
                false,
                false,
            )
            .unwrap()],
        )
        .unwrap()],
    )
    .unwrap();
    assert_eq!(
        shell.extension_actions.shortcut_action_revision(
            Some(&resident_surface),
            runtime,
            tab,
            generation,
        ),
        Ok(ExtensionActionRevision::INITIAL)
    );
    assert_eq!(
        shell.extension_actions.shortcut_action_revision(
            shell.extension_browser_surfaces.published_surface(profile),
            runtime,
            tab,
            generation,
        ),
        Ok(ExtensionActionRevision::INITIAL)
    );
    assert_eq!(
        shell.extension_actions.shortcut_action_revision(
            shell.extension_browser_surfaces.published_surface(profile),
            runtime,
            tab,
            generation.next().unwrap(),
        ),
        Err(zephium_core::extensions::ExtensionActionRejection::TabUnavailable)
    );
    let projected = shell
        .extension_actions
        .projected_actions(profile, tab, generation);
    assert_eq!(projected.len(), 1);
    assert_eq!(
        projected[0].runtime.install_id,
        runtime.install_id().to_string()
    );
    assert_eq!(projected[0].runtime.generation, "0000000000000001");
    assert_eq!(projected[0].revision, "0000000000000001");
    assert_eq!(projected[0].label, "Bitwarden");
    assert_eq!(
        projected[0].icon_rgba_base64.as_deref().map(str::len),
        Some(5_464)
    );
    // A newer generation for the same tab keeps the latest known actions.
    assert_eq!(
        shell
            .extension_actions
            .projected_actions(profile, tab, generation.next().unwrap())
            .len(),
        1
    );
    assert_eq!(shell.retry_extension_actions(), NativeWork::default());
    assert_eq!(engine.extension_action_requests().len(), 1);
    let anchor = ExtensionPopupAnchor::new(Rect::new(1100.0, 8.0, 28.0, 28.0)).unwrap();
    assert_eq!(
        shell.invoke_extension_action(
            runtime,
            ExtensionActionRevision::INITIAL.next().unwrap(),
            anchor,
        ),
        Err(zephium_core::extensions::ExtensionActionRejection::RuntimeSuperseded)
    );
    let invocation = shell
        .invoke_extension_action(runtime, ExtensionActionRevision::INITIAL, anchor)
        .unwrap();
    let native = engine.extension_action_invocations();
    assert_eq!(native.len(), 1);
    assert_eq!(native[0].id(), invocation);
    assert_eq!(native[0].runtime(), runtime);
    assert_eq!(native[0].tab(), tab);
    assert_eq!(native[0].surface_generation(), generation);
    assert_eq!(
        native[0].action_revision(),
        ExtensionActionRevision::INITIAL
    );
    assert_eq!(native[0].anchor(), anchor);
    shell.handle(Command::Engine(EngineEvent::ExtensionActionSettled {
        profile,
        request: invocation,
        settlement: zephium_core::extensions::ExtensionActionSettlement::Dispatched,
    }));
    let dismissed_invocation = shell
        .invoke_extension_action(runtime, ExtensionActionRevision::INITIAL, anchor)
        .unwrap();
    assert_eq!(
        shell.extension_actions.settle_invocation(
            profile,
            dismissed_invocation,
            zephium_core::extensions::ExtensionActionSettlement::PopupDismissed,
        ),
        ExtensionActionInvocationObservation::PopupDismissed
    );
    let rejected_invocation = shell
        .invoke_extension_action(runtime, ExtensionActionRevision::INITIAL, anchor)
        .unwrap();
    assert_eq!(
        shell.extension_actions.settle_invocation(
            profile,
            rejected_invocation,
            zephium_core::extensions::ExtensionActionSettlement::Rejected(
                zephium_core::extensions::ExtensionActionRejection::TabDiscarded,
            ),
        ),
        ExtensionActionInvocationObservation::Rejected {
            tab,
            reason: zephium_core::extensions::ExtensionActionRejection::TabDiscarded,
        }
    );
    shell.handle(Command::Engine(EngineEvent::ExtensionActionsInvalidated {
        profile,
    }));
    assert_eq!(engine.extension_action_requests().len(), 2);

    shell.handle(Command::Engine(
        EngineEvent::ExtensionActionsSnapshotSettled {
            profile,
            tab,
            surface_generation: generation,
            settlement: ExtensionActionSnapshotSettlement::Rejected(
                zephium_core::extensions::ExtensionActionRejection::RuntimeUnavailable,
            ),
        },
    ));
    assert_eq!(shell.extension_actions.snapshot(profile), Some(&snapshot));
    assert!(shell.retry_extension_actions().scheduled);
    assert_eq!(engine.extension_action_requests().len(), 3);

    shell.handle(Command::Engine(
        EngineEvent::ExtensionActionsSnapshotSettled {
            profile,
            tab,
            surface_generation: generation,
            settlement: ExtensionActionSnapshotSettlement::Applied(snapshot.clone()),
        },
    ));
    let stale_generation = generation.next().unwrap();
    shell.handle(Command::Engine(
        EngineEvent::ExtensionActionsSnapshotSettled {
            profile,
            tab,
            surface_generation: stale_generation,
            settlement: ExtensionActionSnapshotSettlement::Rejected(
                zephium_core::extensions::ExtensionActionRejection::RuntimeUnavailable,
            ),
        },
    ));
    assert_eq!(shell.retry_extension_actions(), NativeWork::default());
    assert_eq!(engine.extension_action_requests().len(), 3);
    // Settled actions change only through native invalidations; the
    // maintenance heartbeat reads nothing.
    shell.handle(Command::Tick);
    assert_eq!(engine.extension_action_requests().len(), 3);

    let click_only = ExtensionActionState::new(
        runtime,
        ExtensionActionScope::Tab(tab),
        ExtensionActionRevision::INITIAL,
        "Click only",
        "",
        None,
        true,
        false,
        false,
    )
    .unwrap();
    shell.handle(Command::Engine(
        EngineEvent::ExtensionActionsSnapshotSettled {
            profile,
            tab,
            surface_generation: generation,
            settlement: ExtensionActionSnapshotSettlement::Applied(
                ExtensionActionSnapshot::new(profile, tab, generation, vec![click_only]).unwrap(),
            ),
        },
    ));
    assert_eq!(
        shell.extension_actions.shortcut_action_revision(
            shell.extension_browser_surfaces.published_surface(profile),
            runtime,
            tab,
            generation,
        ),
        Err(zephium_core::extensions::ExtensionActionRejection::TabDiscarded)
    );

    // A physical view can bind after the default action was projected. The
    // native binding invalidates that projection; a fresh revision must make
    // both the first click and a later reopen use the current target.
    shell.handle(Command::Engine(EngineEvent::ExtensionActionsInvalidated {
        profile,
    }));
    let rebound_revision = ExtensionActionRevision::INITIAL.next().unwrap();
    let rebound_action = ExtensionActionState::new(
        runtime,
        ExtensionActionScope::Tab(tab),
        rebound_revision,
        "Bitwarden",
        "",
        None,
        true,
        true,
        false,
    )
    .unwrap();
    shell.handle(Command::Engine(
        EngineEvent::ExtensionActionsSnapshotSettled {
            profile,
            tab,
            surface_generation: generation,
            settlement: ExtensionActionSnapshotSettlement::Applied(
                ExtensionActionSnapshot::new(profile, tab, generation, vec![rebound_action])
                    .unwrap(),
            ),
        },
    ));
    assert_eq!(
        shell.invoke_extension_action(runtime, ExtensionActionRevision::INITIAL, anchor),
        Err(zephium_core::extensions::ExtensionActionRejection::RuntimeSuperseded)
    );
    let opened = shell
        .invoke_extension_action(runtime, rebound_revision, anchor)
        .unwrap();
    shell.handle(Command::Engine(EngineEvent::ExtensionActionSettled {
        profile,
        request: opened,
        settlement: zephium_core::extensions::ExtensionActionSettlement::PopupPresented(Size::new(
            320.0, 400.0,
        )),
    }));
    let reopened = shell
        .invoke_extension_action(runtime, rebound_revision, anchor)
        .unwrap();
    assert_eq!(
        shell.extension_actions.settle_invocation(
            profile,
            reopened,
            zephium_core::extensions::ExtensionActionSettlement::PopupDismissed,
        ),
        ExtensionActionInvocationObservation::PopupDismissed
    );

    // A disabled action still opens its menu, and a menu that cannot open
    // is not reported as a failed click.
    let disabled = ExtensionActionState::new(
        runtime,
        ExtensionActionScope::Tab(tab),
        rebound_revision,
        "Bitwarden",
        "",
        None,
        false,
        true,
        false,
    )
    .unwrap();
    shell.handle(Command::Engine(
        EngineEvent::ExtensionActionsSnapshotSettled {
            profile,
            tab,
            surface_generation: generation,
            settlement: ExtensionActionSnapshotSettlement::Applied(
                ExtensionActionSnapshot::new(profile, tab, generation, vec![disabled]).unwrap(),
            ),
        },
    ));
    assert_eq!(
        shell.invoke_extension_action(runtime, rebound_revision, anchor),
        Err(zephium_core::extensions::ExtensionActionRejection::ActionDisabled)
    );
    let menu = shell
        .dispatch_extension_action(runtime, rebound_revision, anchor, true)
        .unwrap();
    let native = engine.extension_action_invocations();
    let request = native.last().unwrap();
    assert_eq!(request.id(), menu);
    assert!(request.is_menu());
    assert_eq!(
        shell.extension_actions.settle_invocation(
            profile,
            menu,
            zephium_core::extensions::ExtensionActionSettlement::Rejected(
                zephium_core::extensions::ExtensionActionRejection::UnsupportedPlatform,
            ),
        ),
        ExtensionActionInvocationObservation::Dispatched
    );
}

#[test]
fn action_backpressure_does_not_reject_a_published_browser_surface() {
    let (mut shell, engine, _) = setup();
    let profile = ProfileId::from(72_000);
    let space = SpaceId::from(72_001);
    let tab = ItemId::from(72_002);
    install_profile(&mut shell, profile, &[space]);
    shell
        .windows
        .create(WindowKind::Main, profile, space, Size::new(1200.0, 800.0));
    activate_profile(&mut shell, profile);
    assert!(shell.items.insert_tab(
        tab,
        Placement::Space {
            space,
            section: SpaceSection::Today,
        },
    ));
    assert_eq!(shell.focus_tab(tab), Vec::new());

    let surface = shell.sync_extension_browser_surfaces();
    assert!(surface.native.scheduled);
    assert!(!surface.native.rejected);
    assert_eq!(engine.extension_browser_surfaces().len(), 1);

    engine
        .reject_native_dispatch
        .store(true, std::sync::atomic::Ordering::Release);
    let rejected_action = shell.refresh_extension_actions(profile);
    assert!(rejected_action.rejected);
    assert_eq!(engine.extension_browser_surfaces().len(), 1);

    engine
        .reject_native_dispatch
        .store(false, std::sync::atomic::Ordering::Release);
    assert!(shell.retry_extension_actions().scheduled);
    assert_eq!(engine.extension_action_requests().len(), 2);
}
