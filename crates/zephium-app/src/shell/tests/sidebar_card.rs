use super::*;
use std::sync::atomic::Ordering;

fn raise_hidden_card(shell: &mut Shell) {
    shell.handle(Command::SetSidebarHidden(true));
    shell.handle(Command::PeekSidebar {
        peek: true,
        slide: true,
    });
    assert!(shell.windows.focused().unwrap().sidebar_peek);
}

#[test]
fn live_hidden_card_resize_uses_the_admitted_width_and_preserves_the_motion_target() {
    let (mut shell, engine, _) = setup();
    shell.handle(Command::Bootstrap);
    raise_hidden_card(&mut shell);
    let window = shell.windows.focused().unwrap().id;
    engine.chrome_raises.lock().unwrap().clear();

    for width in [340.0, 180.0, 460.0] {
        shell.handle(Command::SetSidebarWidth(width, false));
        let call = *engine.chrome_raises.lock().unwrap().last().unwrap();
        assert_eq!(
            call,
            ChromeRaise {
                window,
                raised: true,
                card: Rect::new(8.0, 8.0, width + 16.0, 784.0),
                slide: true,
                dispatch: NativeDispatch::Scheduled,
            }
        );
        assert_eq!(
            shell.windows.focused().unwrap().metrics.sidebar_width,
            width
        );
    }
    shell.handle(Command::SetWindowSize(Size::new(800.0, 600.0)));
    assert_eq!(
        engine.chrome_raises.lock().unwrap().last().unwrap().card,
        Rect::new(8.0, 8.0, 476.0, 584.0)
    );
    assert_eq!(engine.chrome_raises.lock().unwrap().len(), 4);
}

#[test]
fn lowering_pinning_reload_and_window_hide_retire_card_resize_replay() {
    for command in [
        Command::PeekSidebar {
            peek: false,
            slide: true,
        },
        Command::SetSidebarHidden(false),
        Command::Bootstrap,
        Command::SetWindowVisible(false),
    ] {
        let (mut shell, engine, _) = setup();
        shell.handle(Command::Bootstrap);
        raise_hidden_card(&mut shell);
        shell.handle(command);
        assert!(!shell.windows.focused().unwrap().sidebar_peek);
        assert!(!engine.chrome_raises.lock().unwrap().last().unwrap().raised);
        shell.handle(Command::SetWindowVisible(true));
        engine.chrome_raises.lock().unwrap().clear();
        shell.handle(Command::SetSidebarWidth(320.0, false));
        assert!(engine.chrome_raises.lock().unwrap().is_empty());
    }
}

#[test]
fn invalid_peek_cannot_arm_resize_replay() {
    let (mut shell, engine, _) = setup();
    shell.handle(Command::PeekSidebar {
        peek: true,
        slide: true,
    });
    assert!(engine.chrome_raises.lock().unwrap().is_empty());
    shell.handle(Command::Bootstrap);
    shell.handle(Command::PeekSidebar {
        peek: true,
        slide: true,
    });
    assert!(!shell.windows.focused().unwrap().sidebar_peek);
    assert!(engine.chrome_raises.lock().unwrap().is_empty());

    raise_hidden_card(&mut shell);
    // An invalid request cannot retain an earlier card target.
    shell.windows.focused_mut().unwrap().metrics.sidebar_hidden = false;
    shell.handle(Command::PeekSidebar {
        peek: true,
        slide: true,
    });
    assert!(!shell.windows.focused().unwrap().sidebar_peek);
    assert!(!engine.chrome_raises.lock().unwrap().last().unwrap().raised);

    shell.handle(Command::SetWindowVisible(false));
    shell.handle(Command::SetSidebarHidden(true));
    engine.chrome_raises.lock().unwrap().clear();
    shell.handle(Command::PeekSidebar {
        peek: true,
        slide: true,
    });
    assert!(!shell.windows.focused().unwrap().sidebar_peek);
    shell.handle(Command::SetWindowVisible(true));
    shell.handle(Command::SetSidebarWidth(320.0, false));
    assert!(engine.chrome_raises.lock().unwrap().is_empty());
}

#[test]
fn rejected_and_unsupported_native_raise_do_not_arm_resize_replay() {
    for refusal in [NativeDispatch::Rejected, NativeDispatch::Unsupported] {
        let (mut shell, engine, _) = setup();
        shell.handle(Command::Bootstrap);
        shell.handle(Command::SetSidebarHidden(true));
        engine
            .reject_native_dispatch
            .store(refusal == NativeDispatch::Rejected, Ordering::Release);
        engine
            .unsupported_chrome_raise
            .store(refusal == NativeDispatch::Unsupported, Ordering::Release);
        shell.handle(Command::PeekSidebar {
            peek: true,
            slide: true,
        });
        assert_eq!(
            engine
                .chrome_raises
                .lock()
                .unwrap()
                .last()
                .unwrap()
                .dispatch,
            refusal
        );
        assert!(!shell.windows.focused().unwrap().sidebar_peek);
        engine
            .reject_native_dispatch
            .store(false, Ordering::Release);
        engine
            .unsupported_chrome_raise
            .store(false, Ordering::Release);
        engine.chrome_raises.lock().unwrap().clear();
        shell.handle(Command::SetSidebarWidth(320.0, false));
        assert!(engine.chrome_raises.lock().unwrap().is_empty());
    }
}

#[test]
fn refused_refresh_keeps_prior_intent_but_refused_lower_retires_it() {
    let (mut shell, engine, _) = setup();
    shell.handle(Command::Bootstrap);
    raise_hidden_card(&mut shell);
    engine.reject_native_dispatch.store(true, Ordering::Release);
    shell.handle(Command::SetSidebarWidth(320.0, false));
    assert_eq!(
        engine
            .chrome_raises
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .dispatch,
        NativeDispatch::Rejected
    );
    assert!(shell.windows.focused().unwrap().sidebar_peek);
    engine
        .reject_native_dispatch
        .store(false, Ordering::Release);
    shell.handle(Command::SetSidebarWidth(360.0, false));
    assert_eq!(
        engine
            .chrome_raises
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .dispatch,
        NativeDispatch::Scheduled
    );

    engine.reject_native_dispatch.store(true, Ordering::Release);
    shell.handle(Command::PeekSidebar {
        peek: false,
        slide: false,
    });
    assert_eq!(
        engine
            .chrome_raises
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .dispatch,
        NativeDispatch::Rejected
    );
    assert!(!shell.windows.focused().unwrap().sidebar_peek);
    engine
        .reject_native_dispatch
        .store(false, Ordering::Release);
    engine.chrome_raises.lock().unwrap().clear();
    shell.handle(Command::SetSidebarWidth(400.0, false));
    assert!(engine.chrome_raises.lock().unwrap().is_empty());
}

#[test]
fn peek_intent_belongs_to_its_window_and_shutdown_retires_every_window() {
    let (mut shell, engine, _) = setup();
    shell.handle(Command::Bootstrap);
    raise_hidden_card(&mut shell);
    let first = shell.windows.focused().unwrap();
    let (first_id, profile, space) = (first.id, first.profile, first.space);
    let second_id = shell
        .windows
        .create(WindowKind::Main, profile, space, Size::new(900.0, 700.0));
    assert!(!shell.windows.focused().unwrap().sidebar_peek);
    engine.chrome_raises.lock().unwrap().clear();
    shell.handle(Command::SetSidebarWidth(320.0, false));
    assert!(engine.chrome_raises.lock().unwrap().is_empty());
    assert!(shell.windows.get(first_id).unwrap().sidebar_peek);
    raise_hidden_card(&mut shell);
    shell.handle(Command::SetSidebarWidth(360.0, false));
    assert_eq!(
        engine.chrome_raises.lock().unwrap().last().unwrap().window,
        second_id
    );
    assert_eq!(
        shell.windows.get(first_id).unwrap().metrics.sidebar_width,
        240.0
    );

    let (ack, done) = sync_channel(1);
    shell.handle(Command::Shutdown {
        deadline: test_shutdown_deadline(),
        ack,
    });
    assert_eq!(done.recv().unwrap(), ShutdownOutcome::Clean);
    assert!(shell.windows.iter().all(|window| !window.sidebar_peek));
    engine.chrome_raises.lock().unwrap().clear();
    shell.handle(Command::SetSidebarWidth(400.0, false));
    shell.handle(Command::PeekSidebar {
        peek: true,
        slide: true,
    });
    assert!(engine.chrome_raises.lock().unwrap().is_empty());
}
