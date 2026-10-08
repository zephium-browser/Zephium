use super::*;

type HostLog = Arc<Mutex<Vec<bool>>>;

fn ready(fills_window: bool) -> (Shell, Arc<FakeEngine>, Screen, HostLog, ItemId) {
    let engine = Arc::new(FakeEngine::default());
    engine
        .fills_window_for_fullscreen
        .store(fills_window, std::sync::atomic::Ordering::Release);
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
    let host: HostLog = Arc::new(Mutex::new(Vec::new()));
    let (sink, host_sink) = (screen.clone(), host.clone());
    let mut shell = Shell::new_with_failure(
        engine.clone(),
        Arc::new(FakeStore::default()),
        Arc::new(ImmediateAllowAllCompiler),
        Box::new(|_| {}),
        Arc::new(FakeChrome),
        Box::new(move |projection| {
            if let Projection::HostFullscreen(active) = projection {
                host_sink.lock().unwrap().push(active);
            }
            apply_projection(&mut sink.lock().unwrap(), projection);
        }),
    );
    shell.handle(Command::SetWindowSize(Size::new(1200.0, 800.0)));
    shell.handle(Command::Bootstrap);
    shell.handle(Command::SetWindowFocused(true));
    let page = active_id(&screen);
    navigate_and_commit(&mut shell, page, "https://video.example/watch");
    (shell, engine, screen, host, page)
}

fn fullscreen(shell: &mut Shell, id: ItemId, active: bool) {
    shell.handle(Command::Engine(EngineEvent::FullscreenChanged {
        id,
        active,
    }));
}

fn exits(engine: &FakeEngine) -> Vec<ItemId> {
    engine.fullscreen_exits.lock().unwrap().clone()
}

fn last_region(engine: &FakeEngine) -> Option<Rect> {
    engine
        .layout_regions
        .lock()
        .unwrap()
        .last()
        .copied()
        .flatten()
}

fn open_second(shell: &mut Shell, screen: &Screen) -> ItemId {
    shell.handle(Command::Open);
    let second = active_id(screen);
    navigate_and_commit(shell, second, "https://other.example");
    second
}

#[test]
fn a_window_filling_page_takes_the_whole_window_and_gives_it_back() {
    let (mut shell, engine, screen, host, page) = ready(true);
    let windowed = last_region(&engine).expect("a laid out page");

    fullscreen(&mut shell, page, true);
    assert_eq!(*host.lock().unwrap(), [true]);
    assert_eq!(engine.last_layout(), [page.to_string()]);
    assert_eq!(
        last_region(&engine),
        Some(Rect::new(0.0, 0.0, 1200.0, 800.0))
    );
    // The window grows to the screen; the page follows it.
    shell.handle(Command::SetWindowSize(Size::new(1920.0, 1080.0)));
    assert_eq!(
        last_region(&engine),
        Some(Rect::new(0.0, 0.0, 1920.0, 1080.0))
    );
    assert_eq!(*host.lock().unwrap(), [true]);

    fullscreen(&mut shell, page, false);
    assert_eq!(*host.lock().unwrap(), [true, false]);
    shell.handle(Command::SetWindowSize(Size::new(1200.0, 800.0)));
    assert_eq!(last_region(&engine), Some(windowed));
    assert!(exits(&engine).is_empty());
    let _ = screen;
}

#[test]
fn an_own_window_engine_keeps_the_browser_layout() {
    let (mut shell, engine, _screen, host, page) = ready(false);
    let windowed = last_region(&engine);
    fullscreen(&mut shell, page, true);
    assert!(host.lock().unwrap().is_empty());
    assert_eq!(last_region(&engine), windowed);
    assert_eq!(engine.last_layout(), [page.to_string()]);
    assert!(shell.content_fullscreen().is_some());
}

#[test]
fn switching_tabs_takes_fullscreen_away_first() {
    for fills in [false, true] {
        let (mut shell, engine, screen, host, page) = ready(fills);
        let second = open_second(&mut shell, &screen);
        shell.handle(Command::Activate(page));
        fullscreen(&mut shell, page, true);

        shell.handle(Command::Activate(second));
        assert_eq!(exits(&engine), [page]);
        assert_eq!(engine.last_layout(), [second.to_string()]);
        assert!(shell.content_fullscreen().is_none());
        if fills {
            assert_eq!(*host.lock().unwrap(), [true, false]);
        }
        // The page's own exit arriving later changes nothing.
        fullscreen(&mut shell, page, false);
        assert_eq!(engine.last_layout(), [second.to_string()]);
        assert_eq!(exits(&engine), [page]);
    }
}

#[test]
fn a_background_page_cannot_take_fullscreen() {
    let (mut shell, engine, screen, host, page) = ready(true);
    let second = open_second(&mut shell, &screen);
    fullscreen(&mut shell, page, true);
    assert_eq!(exits(&engine), [page]);
    assert!(host.lock().unwrap().is_empty());
    assert_eq!(engine.last_layout(), [second.to_string()]);
    assert!(shell.content_fullscreen().is_none());
}

#[test]
fn closing_the_fullscreen_tab_restores_the_window_without_an_exit() {
    let (mut shell, engine, screen, host, page) = ready(true);
    let second = open_second(&mut shell, &screen);
    shell.handle(Command::Activate(page));
    fullscreen(&mut shell, page, true);

    shell.handle(Command::Close(page));
    assert_eq!(*host.lock().unwrap(), [true, false]);
    assert!(exits(&engine).is_empty());
    assert_eq!(engine.last_layout(), [second.to_string()]);
}

#[test]
fn browser_surfaces_and_a_hidden_window_take_fullscreen_away() {
    let (mut shell, engine, _screen, host, page) = ready(true);
    fullscreen(&mut shell, page, true);
    shell.handle(Command::SetWindowVisible(false));
    assert_eq!(exits(&engine), [page]);
    assert_eq!(*host.lock().unwrap(), [true, false]);
    shell.handle(Command::SetWindowVisible(true));
    assert_eq!(engine.last_layout(), [page.to_string()]);
    assert_ne!(
        last_region(&engine),
        Some(Rect::new(0.0, 0.0, 1200.0, 800.0))
    );

    fullscreen(&mut shell, page, true);
    let _ = shell.handle_operation(Command::ShowBrowserPage(Some(crate::BrowserPage::Settings)));
    assert_eq!(exits(&engine), [page, page]);
    assert_eq!(*host.lock().unwrap(), [true, false, true, false]);
}

#[test]
fn a_permission_prompt_takes_fullscreen_away() {
    use zephium_core::permissions::{
        PageOrigin, PagePermissionRequest, PagePermissionRequestId, PagePermissionRequestKind,
    };
    let (mut shell, engine, _screen, host, page) = ready(true);
    shell.page_permissions.remember_enabled = false;
    let profile = shell.windows.focused().unwrap().profile;
    fullscreen(&mut shell, page, true);
    shell.handle(Command::Engine(EngineEvent::PermissionRequested {
        id: page,
        profile,
        request: PagePermissionRequest {
            id: PagePermissionRequestId::new(7).unwrap(),
            origin: PageOrigin::parse_exact("https://video.example").unwrap(),
            kind: PagePermissionRequestKind::CameraAndMicrophone,
        },
    }));
    assert!(shell.page_permissions.is_visible());
    assert_eq!(exits(&engine), [page]);
    assert_eq!(*host.lock().unwrap(), [true, false]);
}

#[test]
fn a_split_stands_aside_for_the_fullscreen_page_and_returns_after() {
    let (mut shell, engine, screen, _host, page) = ready(true);
    let second = open_second(&mut shell, &screen);
    shell.handle(Command::SplitWith {
        other: page,
        axis: Axis::Row,
    });
    assert_eq!(engine.last_layout().len(), 2);

    // Either pane on screen may go fullscreen, not only the active one.
    fullscreen(&mut shell, page, true);
    assert_eq!(engine.last_layout(), [page.to_string()]);
    assert!(shell.windows.focused().unwrap().splits.is_some());

    fullscreen(&mut shell, page, false);
    let panes = engine.last_layout();
    assert_eq!(panes.len(), 2);
    assert!(panes.contains(&page.to_string()) && panes.contains(&second.to_string()));
    assert!(exits(&engine).is_empty());
}

#[test]
fn a_crashed_fullscreen_page_hands_the_window_back() {
    let (mut shell, _engine, _screen, host, page) = ready(true);
    fullscreen(&mut shell, page, true);
    shell.handle(Command::Engine(EngineEvent::Crashed { id: page }));
    assert!(shell.content_fullscreen().is_none());
    assert_eq!(*host.lock().unwrap(), [true, false]);
}

#[test]
fn a_second_page_replaces_the_first() {
    let (mut shell, engine, screen, _host, page) = ready(false);
    let second = open_second(&mut shell, &screen);
    shell.handle(Command::SplitWith {
        other: page,
        axis: Axis::Row,
    });
    fullscreen(&mut shell, page, true);
    fullscreen(&mut shell, second, true);
    assert_eq!(exits(&engine), [page]);
    assert_eq!(shell.content_fullscreen(), Some(second));
    // The first page's exit is not the second one's.
    fullscreen(&mut shell, page, false);
    assert_eq!(shell.content_fullscreen(), Some(second));
}
