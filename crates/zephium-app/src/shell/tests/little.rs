use super::*;

/// Records which chrome verified each presentation.
#[derive(Default)]
struct WindowChrome {
    verified: Mutex<Vec<(Option<WindowId>, ItemId)>>,
}

impl GeometryChrome for WindowChrome {
    fn position(&self, _frame: ChromeFrame) -> bool {
        true
    }
}

impl PresentationChrome for WindowChrome {
    fn apply_tab_for_presentation(
        &self,
        presentation: ChromePresentation,
        _done: ChromePresentationCallback,
    ) -> ChromePresentationDispatch {
        self.verified.lock().unwrap().push((None, presentation.id));
        ChromePresentationDispatch::Applied
    }

    fn apply_little_presentation(
        &self,
        window: WindowId,
        presentation: ChromePresentation,
        _done: ChromePresentationCallback,
    ) -> ChromePresentationDispatch {
        assert_eq!(presentation.active, Some(presentation.id));
        self.verified
            .lock()
            .unwrap()
            .push((Some(window), presentation.id));
        ChromePresentationDispatch::Applied
    }
}

type Log = Arc<Mutex<Vec<Projection>>>;

struct Little {
    shell: Shell,
    engine: Arc<FakeEngine>,
    chrome: Arc<WindowChrome>,
    screen: Screen,
    log: Log,
    store: Arc<FakeStore>,
}

fn little(hosts: bool) -> Little {
    let store = Arc::new(FakeStore::default());
    let engine = Arc::new(FakeEngine::default());
    engine
        .hosts_little_windows
        .store(hosts, std::sync::atomic::Ordering::Release);
    let chrome = Arc::new(WindowChrome::default());
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
    let log: Log = Arc::new(Mutex::new(Vec::new()));
    let (sink, record) = (screen.clone(), log.clone());
    let mut shell = Shell::new_with_failure(
        engine.clone(),
        store.clone(),
        Arc::new(ImmediateAllowAllCompiler),
        Box::new(|_| {}),
        chrome.clone(),
        Box::new(move |projection| {
            record.lock().unwrap().push(projection.clone());
            apply_projection(&mut sink.lock().unwrap(), projection);
        }),
    );
    shell.handle(Command::SetWindowSize(Size::new(1200.0, 800.0)));
    shell.handle(Command::Bootstrap);
    shell.handle(Command::SetAppSetting {
        key: "tabs.external-position".into(),
        value: "little".into(),
    });
    Little {
        shell,
        engine,
        chrome,
        screen,
        log,
        store,
    }
}

impl Little {
    fn opened(&self) -> Vec<zephium_ipc::LittleOpenedView> {
        self.log
            .lock()
            .unwrap()
            .iter()
            .filter_map(|projection| match projection {
                Projection::LittleOpened(opened) => Some(*opened),
                _ => None,
            })
            .collect()
    }

    fn closed(&self) -> Vec<zephium_ipc::LittleClosedView> {
        self.log
            .lock()
            .unwrap()
            .iter()
            .filter_map(|projection| match projection {
                Projection::LittleClosed(closed) => Some(*closed),
                _ => None,
            })
            .collect()
    }

    fn last_view(&self, window: WindowId) -> Option<zephium_ipc::LittleView> {
        self.log
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find_map(|projection| match projection {
                Projection::Little(view) if view.window == window => Some(view.clone()),
                _ => None,
            })
    }

    fn page(&self, window: WindowId) -> ItemId {
        let view = self.last_view(window).expect("a Little window projection");
        ItemId::parse(&view.tab.expect("a page").id).unwrap()
    }

    /// Runs what the launcher offers for `query`, as Enter does.
    fn launcher(
        &mut self,
        query: &str,
        replace: Option<String>,
        new_little: bool,
    ) -> OperationDisposition {
        let window = self.shell.windows.focused().unwrap();
        let context = zephium_ipc::SearchContext {
            window_id: window.id.to_string(),
            session_id: "0000000000000001".into(),
            request_id: "one".into(),
            profile_id: window.profile.to_string(),
            space_id: window.space.to_string(),
        };
        self.shell.handle(Command::SearchScoped {
            little: new_little,
            query: query.into(),
            context: Box::new(context.clone()),
        });
        let action = self
            .shell
            .search
            .results
            .iter()
            .find(|result| matches!(result.action, SearchAction::OpenUrl { .. }))
            .map(|result| result.action.clone())
            .expect("the typed address is offered");
        self.shell.handle_operation(Command::RunSearchAction {
            context: Box::new(context),
            action,
            background: false,
            replace,
            new_little,
        })
    }

    fn search_little(&mut self, query: &str, little: bool) {
        let window = self.shell.windows.focused().unwrap();
        let context = zephium_ipc::SearchContext {
            window_id: window.id.to_string(),
            session_id: "0000000000000001".into(),
            request_id: "one".into(),
            profile_id: window.profile.to_string(),
            space_id: window.space.to_string(),
        };
        self.shell.handle(Command::SearchScoped {
            little,
            query: query.into(),
            context: Box::new(context),
        });
    }

    fn external_tabs_opened(&self) -> bool {
        self.log
            .lock()
            .unwrap()
            .iter()
            .any(|projection| matches!(projection, Projection::ExternalTabsOpened))
    }

    /// Opens a link handed over by another application, then builds its
    /// window and shows its first document.
    fn open_shown(&mut self, url: &str) -> (WindowId, ItemId) {
        self.shell.handle(Command::OpenExternal(vec![url.into()]));
        let window = self.opened().last().expect("a Little window").window;
        self.shell.handle(Command::LittleAttached {
            window,
            size: Size::new(720.0, 640.0),
        });
        let page = self.page(window);
        commit_url(&mut self.shell, page, url);
        present_committed(&mut self.shell, page, url);
        (window, page)
    }
}

#[test]
fn a_link_from_another_app_opens_in_a_little_window_outside_the_sidebar() {
    let mut little = little(true);
    let tabs_before = last(&little.screen).tabs.len();
    let active_before = last(&little.screen).active.clone();

    little
        .shell
        .handle(Command::OpenExternal(vec!["https://example.com/".into()]));

    let opened = little.opened();
    assert_eq!(opened.len(), 1);
    let page = little.page(opened[0].window);
    assert!(little
        .engine
        .calls()
        .iter()
        .any(|call| call.starts_with(&format!("create {page} https://example.com/ "))));
    assert_eq!(last(&little.screen).tabs.len(), tabs_before);
    assert_eq!(last(&little.screen).active, active_before);
    assert!(!little.external_tabs_opened());
}

#[test]
fn the_little_toolbar_verifies_the_page_before_it_appears_in_its_window() {
    let mut little = little(true);
    let (window, page) = little.open_shown("https://example.com/");

    assert_eq!(
        little.chrome.verified.lock().unwrap().as_slice(),
        &[(Some(window), page)]
    );
    let calls = little.engine.calls();
    assert!(calls
        .iter()
        .any(|call| call.starts_with(&format!("present {page} "))));
    assert!(calls.contains(&format!("layout@{window} {page}")));
    let shown = little.last_view(window).unwrap().tab.unwrap();
    assert_eq!(shown.url.as_deref(), Some("https://example.com/"));
}

#[test]
fn a_little_window_lays_out_only_once_its_native_window_exists() {
    let mut little = little(true);
    little
        .shell
        .handle(Command::OpenExternal(vec!["https://example.com/".into()]));
    let window = little.opened()[0].window;
    assert!(!little
        .engine
        .calls()
        .iter()
        .any(|call| call.starts_with(&format!("layout@{window} "))));

    little.shell.handle(Command::LittleAttached {
        window,
        size: Size::new(720.0, 640.0),
    });

    assert!(little
        .engine
        .layout_regions
        .lock()
        .unwrap()
        .contains(&Some(Rect::new(
            0.0,
            crate::LITTLE_TOOLBAR_HEIGHT,
            720.0,
            640.0 - crate::LITTLE_TOOLBAR_HEIGHT,
        ))));
}

#[test]
fn opening_in_the_main_window_keeps_the_live_page_and_puts_it_in_front() {
    let mut little = little(true);
    let (window, page) = little.open_shown("https://example.com/");

    little.shell.handle(Command::Little {
        window,
        action: crate::LittleAction::OpenInMain,
    });

    let calls = little.engine.calls();
    assert!(calls.contains(&format!("release@{window}")));
    assert!(!calls.contains(&format!("close {page}")));
    assert_eq!(
        calls
            .iter()
            .filter(|call| call.starts_with(&format!("create {page} ")))
            .count(),
        1
    );
    assert_eq!(active_id(&little.screen), page);
    assert!(last(&little.screen)
        .tabs
        .iter()
        .any(|tab| tab.id == page.to_string()));
    assert_eq!(
        little.closed(),
        vec![zephium_ipc::LittleClosedView {
            window,
            reveal_main: true,
        }]
    );
    let release = calls
        .iter()
        .rposition(|call| *call == format!("release@{window}"))
        .unwrap();
    let main_layout = calls
        .iter()
        .rposition(|call| call.starts_with("layout@1 ") && call.contains(&page.to_string()))
        .unwrap();
    assert!(release < main_layout);
}

#[test]
fn closing_a_little_window_ends_its_page() {
    let mut little = little(true);
    let (window, page) = little.open_shown("https://example.com/");

    little.shell.handle(Command::Little {
        window,
        action: crate::LittleAction::Close,
    });

    let calls = little.engine.calls();
    assert!(calls.contains(&format!("close {page}")));
    assert!(calls.contains(&format!("release@{window}")));
    assert!(little.shell.items.tab(page).is_none());
    assert_eq!(
        little.closed(),
        vec![zephium_ipc::LittleClosedView {
            window,
            reveal_main: false,
        }]
    );
}

#[test]
fn a_little_page_is_out_of_the_main_windows_reach_and_never_saved() {
    let mut little = little(true);
    let (_window, page) = little.open_shown("https://example.com/");

    little.shell.handle(Command::Close(page));
    little.shell.handle(Command::Activate(page));
    little.shell.persist();

    assert!(little.shell.items.tab(page).is_some());
    assert_ne!(last(&little.screen).active, Some(page.to_string()));
    let saved = little.store.saved.lock().unwrap().clone().expect("a save");
    assert!(saved.items.iter().all(|item| item.id != page));
}

#[test]
fn a_little_page_is_kept_awake_while_its_window_is_open() {
    let mut little = little(true);
    let (_window, page) = little.open_shown("https://example.com/");

    assert!(little.shell.discard_protected_leaves().contains(&page));
}

#[test]
fn the_toolbar_goes_back_and_reloads_only_its_own_page() {
    let mut little = little(true);
    let (window, page) = little.open_shown("https://example.com/");
    little.shell.handle(Command::Engine(EngineEvent::NavState {
        id: page,
        can_go_back: true,
        can_go_forward: false,
    }));

    little.shell.handle(Command::Little {
        window,
        action: crate::LittleAction::Back,
    });
    little.shell.handle(Command::Little {
        window,
        action: crate::LittleAction::Reload,
    });
    little.shell.handle(Command::Little {
        window: window + 100,
        action: crate::LittleAction::Reload,
    });

    let calls = little.engine.calls();
    assert!(calls.contains(&format!("back {page}")));
    assert_eq!(
        calls
            .iter()
            .filter(|call| **call == format!("reload {page}"))
            .count(),
        1
    );
}

#[test]
fn without_engine_support_links_open_as_tabs_and_bring_the_window_forward() {
    let mut little = little(false);
    let tabs_before = last(&little.screen).tabs.len();

    little
        .shell
        .handle(Command::OpenExternal(vec!["https://example.com/".into()]));

    assert!(little.opened().is_empty());
    assert_eq!(last(&little.screen).tabs.len(), tabs_before + 1);
    assert!(little.external_tabs_opened());
}

#[test]
fn links_past_the_window_limit_open_as_tabs() {
    let mut little = little(true);
    let urls: Vec<String> = (0..=super::super::little::MAX_LITTLE_WINDOWS)
        .map(|index| format!("https://site{index}.example/"))
        .collect();
    let tabs_before = last(&little.screen).tabs.len();

    little.shell.handle(Command::OpenExternal(urls));

    assert_eq!(
        little.opened().len(),
        super::super::little::MAX_LITTLE_WINDOWS
    );
    assert_eq!(last(&little.screen).tabs.len(), tabs_before + 1);
    assert!(little.external_tabs_opened());
}

#[test]
fn open_in_zephium_puts_the_page_in_the_space_the_person_picks() {
    let mut little = little(true);
    let home = little.shell.windows.focused().unwrap().space;
    let created =
        little
            .shell
            .handle_operation(Command::Space(Box::new(crate::SpaceCommand::Create {
                name: "Work".into(),
                look: zephium_core::spaces::SpaceLook::default(),
            })));
    assert_ne!(created.outcome, OperationOutcome::Rejected);
    let work = little.shell.windows.focused().unwrap().space;
    little
        .shell
        .handle_operation(Command::Space(Box::new(crate::SpaceCommand::Switch(home))));
    let (window, page) = little.open_shown("https://example.com/");

    let view = little.last_view(window).unwrap();
    assert_eq!(
        view.spaces
            .iter()
            .map(|space| space.id.clone())
            .collect::<Vec<_>>(),
        vec![home.to_string(), work.to_string()]
    );
    assert_eq!(view.space, Some(home.to_string()));

    little.shell.handle(Command::Little {
        window,
        action: crate::LittleAction::OpenInSpace(work),
    });

    assert_eq!(little.shell.windows.focused().unwrap().space, work);
    assert!(little
        .shell
        .items
        .roots(Placement::Space {
            space: work,
            section: SpaceSection::Today,
        })
        .contains(&page));
    assert_eq!(active_id(&little.screen), page);
}

#[test]
fn the_little_launcher_offers_an_open_site_as_a_page_for_a_little_window() {
    let mut little = little(true);
    let site = active_id(&little.screen);
    commit_url(&mut little.shell, site, "https://example.com/");
    present_committed(&mut little.shell, site, "https://example.com/");

    little.search_little("", true);
    let row = little
        .shell
        .search
        .results
        .iter()
        .find(|result| result.kind == "tab")
        .expect("the open site is offered");
    assert_eq!(
        row.action,
        SearchAction::OpenUrl {
            url: "https://example.com/".into()
        }
    );

    little.search_little("", false);
    assert!(little
        .shell
        .search
        .results
        .iter()
        .any(|result| matches!(result.action, SearchAction::ActivateTab { .. })));
}

#[test]
fn the_little_launcher_chooses_an_open_site_without_moving_its_tab() {
    let mut little = little(true);
    let site = active_id(&little.screen);
    commit_url(&mut little.shell, site, "https://example.com/");
    present_committed(&mut little.shell, site, "https://example.com/");
    let tabs_before = last(&little.screen).tabs.len();

    let opened = little.launcher("example.com", None, true);

    assert_eq!(opened.reason, zephium_ipc::OperationReason::OpenedInLittle);
    assert_eq!(little.opened().len(), 1);
    assert_eq!(last(&little.screen).tabs.len(), tabs_before);
    assert_eq!(active_id(&little.screen), site);
}

#[test]
fn the_little_launcher_offers_neither_a_blank_tab_nor_a_command() {
    let mut little = little(true);
    assert!(little
        .shell
        .items
        .tab(active_id(&little.screen))
        .unwrap()
        .url
        .is_none());

    little.search_little("", true);
    assert!(little.shell.search.results.is_empty());

    little.search_little("new", true);
    assert!(little
        .shell
        .search
        .results
        .iter()
        .all(|result| matches!(result.action, SearchAction::OpenUrl { .. })));

    little.search_little("new", false);
    assert!(little
        .shell
        .search
        .results
        .iter()
        .any(|result| result.kind == "command"));
}

#[test]
fn an_address_chosen_in_the_little_launcher_opens_a_little_window_not_a_tab() {
    let mut little = little(true);
    let tabs_before = last(&little.screen).tabs.len();

    let opened = little.launcher("example.org", None, true);

    assert_eq!(opened.reason, OperationReason::OpenedInLittle);
    let window = little.opened().last().expect("a Little window").window;
    let page = little.page(window);
    assert!(little
        .engine
        .calls()
        .iter()
        .any(|call| call.starts_with(&format!("create {page} https://example.org"))));
    assert_eq!(last(&little.screen).tabs.len(), tabs_before);
}

#[test]
fn the_launcher_replaces_the_page_of_the_little_window_it_edits() {
    let mut little = little(true);
    let (window, page) = little.open_shown("https://example.com/");
    let tabs_before = last(&little.screen).tabs.len();

    let opened = little.launcher("iana.org", Some(page.to_string()), false);

    assert_eq!(opened.reason, OperationReason::OpenedInLittle);
    assert!(little.closed().is_empty());
    assert_eq!(little.opened().len(), 1);
    assert_eq!(last(&little.screen).tabs.len(), tabs_before);
    assert!(little
        .shell
        .items
        .pending_navigation_request(page)
        .is_some());
    commit_url(&mut little.shell, page, "https://www.iana.org/");
    present_committed(&mut little.shell, page, "https://www.iana.org/");
    assert_eq!(little.page(window), page);
    assert!(!little.engine.calls().contains(&format!("close {page}")));
}

#[test]
fn a_private_session_keeps_what_it_searches_out_of_a_little_window() {
    let mut little = little(true);
    little
        .shell
        .handle(Command::Run("window.newPrivate".into()));
    let tabs_before = last(&little.screen).tabs.len();

    let opened = little.launcher("example.org", None, true);

    assert_ne!(opened.reason, OperationReason::OpenedInLittle);
    assert!(little.opened().is_empty());
    assert_eq!(last(&little.screen).tabs.len(), tabs_before + 1);
}

#[test]
fn past_the_window_limit_the_launcher_opens_a_tab_and_the_main_window_comes_forward() {
    let mut little = little(true);
    for index in 0..super::super::little::MAX_LITTLE_WINDOWS {
        little.launcher(&format!("site{index}.example"), None, true);
    }
    let tabs_before = last(&little.screen).tabs.len();

    let opened = little.launcher("one-more.example", None, true);

    assert_ne!(opened.reason, OperationReason::OpenedInLittle);
    assert_eq!(last(&little.screen).tabs.len(), tabs_before + 1);
}

#[test]
fn without_little_windows_the_command_opens_a_new_tab() {
    let mut little = little(false);
    let tabs_before = last(&little.screen).tabs.len();

    little.shell.handle(Command::Run("window.newLittle".into()));

    assert!(little.opened().is_empty());
    assert_eq!(last(&little.screen).tabs.len(), tabs_before + 1);
}
