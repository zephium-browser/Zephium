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
