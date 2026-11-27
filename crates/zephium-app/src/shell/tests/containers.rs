use super::*;
use crate::ContainerCommand;
use zephium_core::containers::SpaceAccess;
use zephium_core::spaces::SpaceLook;

#[test]
fn assigning_a_container_is_denied_when_its_space_scope_excludes_the_tab() {
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);

    let window = shell.windows.focused().unwrap();
    let profile = window.profile;
    let space = window.space;
    let tab = active_id(&screen);

    for (name, spaces) in [
        ("Available", SpaceAccess::Selected(vec![space])),
        ("Unavailable", SpaceAccess::Selected(Vec::new())),
    ] {
        let created =
            shell.handle_operation(Command::Container(Box::new(ContainerCommand::Create {
                name: name.into(),
                look: SpaceLook::default(),
                spaces,
            })));
        assert_eq!(created.outcome, OperationOutcome::Applied);
    }

    let (available, unavailable) = {
        let mut containers = shell.containers.for_profile(profile);
        (containers.next().unwrap().id, containers.next().unwrap().id)
    };
    let allowed =
        shell.handle_operation(Command::Container(Box::new(ContainerCommand::AssignTab {
            tab,
            container: Some(available),
        })));
    assert_ne!(allowed.outcome, OperationOutcome::Rejected);
    assert_eq!(shell.items.tab(tab).unwrap().container, Some(available));

    let denied =
        shell.handle_operation(Command::Container(Box::new(ContainerCommand::AssignTab {
            tab,
            container: Some(unavailable),
        })));
    assert_eq!(denied.outcome, OperationOutcome::Rejected);
    assert_eq!(denied.reason, OperationReason::InvalidScope);
    assert_eq!(shell.items.tab(tab).unwrap().container, Some(available));
}

#[test]
fn revoked_space_access_detaches_the_container_and_recreates_its_tabs() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let profile = shell.windows.focused().unwrap().profile;
    let space = shell.windows.focused().unwrap().space;
    let tab = active_id(&screen);
    navigate_and_commit(&mut shell, tab, "https://revoke.example/");
    let created = shell.handle_operation(Command::Container(Box::new(ContainerCommand::Create {
        name: "Temporary".into(),
        look: SpaceLook::default(),
        spaces: SpaceAccess::All,
    })));
    assert_eq!(created.outcome, OperationOutcome::Applied);
    let container = shell.containers.for_profile(profile).next().unwrap().id;
    let assigned =
        shell.handle_operation(Command::Container(Box::new(ContainerCommand::AssignTab {
            tab,
            container: Some(container),
        })));
    assert_ne!(assigned.outcome, OperationOutcome::Rejected);
    assert_eq!(
        shell.container_recreates.get(&tab).unwrap().0,
        Some(container)
    );

    let updated = shell.handle_operation(Command::Container(Box::new(ContainerCommand::Update {
        id: container,
        name: "Temporary".into(),
        look: SpaceLook::default(),
        spaces: SpaceAccess::Selected(Vec::new()),
    })));

    assert_ne!(updated.outcome, OperationOutcome::Rejected);
    assert!(!shell.containers.get(container).unwrap().available_in(space));
    assert_eq!(shell.items.tab(tab).unwrap().container, None);
    let (recreate_container, recreate_url) = shell.container_recreates.get(&tab).unwrap();
    assert_eq!(*recreate_container, None);
    assert_eq!(recreate_url.as_str(), "https://revoke.example/");

    shell.on_view_capacity_retry(tab);

    assert!(engine
        .calls()
        .iter()
        .any(|call| call.starts_with(&format!("create {tab} https://revoke.example/ [default]"))));
}

#[test]
fn container_recreation_uses_pending_navigation() {
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let profile = shell.windows.focused().unwrap().profile;
    let space = shell.windows.focused().unwrap().space;
    let tab = active_id(&screen);
    let created = shell.handle_operation(Command::Container(Box::new(ContainerCommand::Create {
        name: "Pending".into(),
        look: SpaceLook::default(),
        spaces: SpaceAccess::Selected(vec![space]),
    })));
    assert_eq!(created.outcome, OperationOutcome::Applied);
    let container = shell.containers.for_profile(profile).next().unwrap().id;
    let _ = shell.items.navigate(tab, "https://first-load.example/");
    let assigned =
        shell.handle_operation(Command::Container(Box::new(ContainerCommand::AssignTab {
            tab,
            container: Some(container),
        })));

    assert_ne!(assigned.outcome, OperationOutcome::Rejected);
    assert_eq!(
        shell.container_recreates.get(&tab).unwrap().1.as_str(),
        "https://first-load.example/"
    );
}

#[test]
fn a_new_navigation_cancels_the_pending_container_recreation() {
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let profile = shell.windows.focused().unwrap().profile;
    let space = shell.windows.focused().unwrap().space;
    let tab = active_id(&screen);
    let created = shell.handle_operation(Command::Container(Box::new(ContainerCommand::Create {
        name: "Pending".into(),
        look: SpaceLook::default(),
        spaces: SpaceAccess::Selected(vec![space]),
    })));
    assert_eq!(created.outcome, OperationOutcome::Applied);
    let container = shell.containers.for_profile(profile).next().unwrap().id;
    let _ = shell.items.navigate(tab, "https://first-load.example/");
    let assigned =
        shell.handle_operation(Command::Container(Box::new(ContainerCommand::AssignTab {
            tab,
            container: Some(container),
        })));
    assert_ne!(assigned.outcome, OperationOutcome::Rejected);
    assert!(shell.container_recreates.contains_key(&tab));

    let navigated = shell.handle_operation(Command::Navigate {
        id: tab,
        input: "https://new-target.example/".into(),
    });
    assert_ne!(navigated.outcome, OperationOutcome::Rejected);
    assert!(!shell.container_recreates.contains_key(&tab));
}
