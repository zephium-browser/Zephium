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

    fn apply_mini_presentation(
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

struct Mini {
    shell: Shell,
    engine: Arc<FakeEngine>,
    chrome: Arc<WindowChrome>,
    screen: Screen,
    log: Log,
    store: Arc<FakeStore>,
}

fn mini(hosts: bool) -> Mini {
    let store = Arc::new(FakeStore::default());
    let engine = Arc::new(FakeEngine::default());
    engine
        .hosts_mini_windows
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
        value: "mini".into(),
    });
    Mini {
        shell,
        engine,
        chrome,
        screen,
        log,
        store,
    }
}

impl Mini {
    fn opened(&self) -> Vec<zephium_ipc::MiniOpenedView> {
        self.log
            .lock()
            .unwrap()
            .iter()
            .filter_map(|projection| match projection {
                Projection::MiniOpened(opened) => Some(*opened),
                _ => None,
            })
            .collect()
    }

    fn closed(&self) -> Vec<zephium_ipc::MiniClosedView> {
        self.log
            .lock()
            .unwrap()
            .iter()
            .filter_map(|projection| match projection {
                Projection::MiniClosed(closed) => Some(*closed),
                _ => None,
            })
            .collect()
    }

    fn last_view(&self, window: WindowId) -> Option<zephium_ipc::MiniView> {
        self.log
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find_map(|projection| match projection {
                Projection::Mini(view) if view.window == window => Some(view.clone()),
                _ => None,
            })
    }

    fn page(&self, window: WindowId) -> ItemId {
        let view = self.last_view(window).expect("a mini window projection");
        ItemId::parse(&view.tab.expect("a page").id).unwrap()
    }

    /// Runs what the launcher offers for `query`, as Enter does.
    fn launcher(
        &mut self,
        query: &str,
        replace: Option<String>,
        new_mini: bool,
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
            mini: new_mini,
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
            new_mini,
        })
    }

    fn search_mini(&mut self, query: &str, mini: bool) {
        let window = self.shell.windows.focused().unwrap();
        let context = zephium_ipc::SearchContext {
            window_id: window.id.to_string(),
            session_id: "0000000000000001".into(),
            request_id: "one".into(),
            profile_id: window.profile.to_string(),
            space_id: window.space.to_string(),
        };
        self.shell.handle(Command::SearchScoped {
            mini,
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
        let window = self.opened().last().expect("a mini window").window;
        self.shell.handle(Command::MiniAttached {
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
fn a_link_from_another_app_opens_in_a_mini_window_outside_the_sidebar() {
    let mut mini = mini(true);
    let tabs_before = last(&mini.screen).tabs.len();
    let active_before = last(&mini.screen).active.clone();

    mini.shell
        .handle(Command::OpenExternal(vec!["https://example.com/".into()]));

    let opened = mini.opened();
    assert_eq!(opened.len(), 1);
    let page = mini.page(opened[0].window);
    assert!(mini
        .engine
        .calls()
        .iter()
        .any(|call| call.starts_with(&format!("create {page} https://example.com/ "))));
    assert_eq!(last(&mini.screen).tabs.len(), tabs_before);
    assert_eq!(last(&mini.screen).active, active_before);
    assert!(!mini.external_tabs_opened());
}

#[test]
fn the_mini_toolbar_verifies_the_page_before_it_appears_in_its_window() {
    let mut mini = mini(true);
    let (window, page) = mini.open_shown("https://example.com/");

    assert_eq!(
        mini.chrome.verified.lock().unwrap().as_slice(),
        &[(Some(window), page)]
    );
    let calls = mini.engine.calls();
    assert!(calls
        .iter()
        .any(|call| call.starts_with(&format!("present {page} "))));
    assert!(calls.contains(&format!("layout@{window} {page}")));
    let shown = mini.last_view(window).unwrap().tab.unwrap();
    assert_eq!(shown.url.as_deref(), Some("https://example.com/"));
}

#[test]
fn a_mini_window_lays_out_only_once_its_native_window_exists() {
    let mut mini = mini(true);
    mini.shell
        .handle(Command::OpenExternal(vec!["https://example.com/".into()]));
    let window = mini.opened()[0].window;
    assert!(!mini
        .engine
        .calls()
        .iter()
        .any(|call| call.starts_with(&format!("layout@{window} "))));

    mini.shell.handle(Command::MiniAttached {
        window,
        size: Size::new(720.0, 640.0),
    });

    assert!(mini
        .engine
        .layout_regions
        .lock()
        .unwrap()
        .contains(&Some(Rect::new(
            crate::MINI_PAGE_INSET,
            crate::MINI_TOOLBAR_HEIGHT,
            720.0 - 2.0 * crate::MINI_PAGE_INSET,
            640.0 - crate::MINI_TOOLBAR_HEIGHT - crate::MINI_PAGE_INSET,
        ))));
}

#[test]
fn opening_in_the_main_window_keeps_the_live_page_and_puts_it_in_front() {
    let mut mini = mini(true);
    let (window, page) = mini.open_shown("https://example.com/");

    mini.shell.handle(Command::Mini {
        window,
        action: crate::MiniAction::OpenInMain,
    });

    let calls = mini.engine.calls();
    assert!(calls.contains(&format!("release@{window}")));
    assert!(!calls.contains(&format!("close {page}")));
    assert_eq!(
        calls
            .iter()
            .filter(|call| call.starts_with(&format!("create {page} ")))
            .count(),
        1
    );
    assert_eq!(active_id(&mini.screen), page);
    assert!(last(&mini.screen)
        .tabs
        .iter()
        .any(|tab| tab.id == page.to_string()));
    assert_eq!(
        mini.closed(),
        vec![zephium_ipc::MiniClosedView {
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
fn closing_a_mini_window_ends_its_page() {
    let mut mini = mini(true);
    let (window, page) = mini.open_shown("https://example.com/");

    mini.shell.handle(Command::Mini {
        window,
        action: crate::MiniAction::Close,
    });

    let calls = mini.engine.calls();
    assert!(calls.contains(&format!("close {page}")));
    assert!(calls.contains(&format!("release@{window}")));
    assert!(mini.shell.items.tab(page).is_none());
    assert_eq!(
        mini.closed(),
        vec![zephium_ipc::MiniClosedView {
            window,
            reveal_main: false,
        }]
    );
}

#[test]
fn a_mini_page_is_out_of_the_main_windows_reach_and_never_saved() {
    let mut mini = mini(true);
    let (_window, page) = mini.open_shown("https://example.com/");

    mini.shell.handle(Command::Close(page));
    mini.shell.handle(Command::Activate(page));
    mini.shell.persist();

    assert!(mini.shell.items.tab(page).is_some());
    assert_ne!(last(&mini.screen).active, Some(page.to_string()));
    let saved = mini.store.saved.lock().unwrap().clone().expect("a save");
    assert!(saved.items.iter().all(|item| item.id != page));
}

#[test]
fn a_mini_page_is_kept_awake_while_its_window_is_open() {
    let mut mini = mini(true);
    let (_window, page) = mini.open_shown("https://example.com/");

    assert!(mini.shell.discard_protected_leaves().contains(&page));
}

#[test]
fn the_toolbar_goes_back_and_reloads_only_its_own_page() {
    let mut mini = mini(true);
    let (window, page) = mini.open_shown("https://example.com/");
    mini.shell.handle(Command::Engine(EngineEvent::NavState {
        id: page,
        can_go_back: true,
        can_go_forward: false,
    }));

    mini.shell.handle(Command::Mini {
        window,
        action: crate::MiniAction::Back,
    });
    mini.shell.handle(Command::Mini {
        window,
        action: crate::MiniAction::Reload,
    });
    mini.shell.handle(Command::Mini {
        window: window + 100,
        action: crate::MiniAction::Reload,
    });

    let calls = mini.engine.calls();
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
    let mut mini = mini(false);
    let tabs_before = last(&mini.screen).tabs.len();

    mini.shell
        .handle(Command::OpenExternal(vec!["https://example.com/".into()]));

    assert!(mini.opened().is_empty());
    assert_eq!(last(&mini.screen).tabs.len(), tabs_before + 1);
    assert!(mini.external_tabs_opened());
}

#[test]
fn links_past_the_window_limit_open_as_tabs() {
    let mut mini = mini(true);
    let urls: Vec<String> = (0..=super::super::mini::MAX_MINI_WINDOWS)
        .map(|index| format!("https://site{index}.example/"))
        .collect();
    let tabs_before = last(&mini.screen).tabs.len();

    mini.shell.handle(Command::OpenExternal(urls));

    assert_eq!(mini.opened().len(), super::super::mini::MAX_MINI_WINDOWS);
    assert_eq!(last(&mini.screen).tabs.len(), tabs_before + 1);
    assert!(mini.external_tabs_opened());
}

#[test]
fn open_in_zephium_puts_the_page_in_the_space_the_person_picks() {
    let mut mini = mini(true);
    let home = mini.shell.windows.focused().unwrap().space;
    let created =
        mini.shell
            .handle_operation(Command::Space(Box::new(crate::SpaceCommand::Create {
                name: "Work".into(),
                look: zephium_core::spaces::SpaceLook::default(),
            })));
    assert_ne!(created.outcome, OperationOutcome::Rejected);
    let work = mini.shell.windows.focused().unwrap().space;
    mini.shell
        .handle_operation(Command::Space(Box::new(crate::SpaceCommand::Switch(home))));
    let (window, page) = mini.open_shown("https://example.com/");

    let view = mini.last_view(window).unwrap();
    assert_eq!(
        view.spaces
            .iter()
            .map(|space| space.id.clone())
            .collect::<Vec<_>>(),
        vec![home.to_string(), work.to_string()]
    );
    assert_eq!(view.space, Some(home.to_string()));

    mini.shell.handle(Command::Mini {
        window,
        action: crate::MiniAction::OpenInSpace(work),
    });

    assert_eq!(mini.shell.windows.focused().unwrap().space, work);
    assert!(mini
        .shell
        .items
        .roots(Placement::Space {
            space: work,
            section: SpaceSection::Today,
        })
        .contains(&page));
    assert_eq!(active_id(&mini.screen), page);
}

#[test]
fn the_mini_launcher_offers_an_open_site_as_a_page_for_a_mini_window() {
    let mut mini = mini(true);
    let site = active_id(&mini.screen);
    commit_url(&mut mini.shell, site, "https://example.com/");
    present_committed(&mut mini.shell, site, "https://example.com/");

    mini.search_mini("", true);
    let row = mini
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

    mini.search_mini("", false);
    assert!(mini
        .shell
        .search
        .results
        .iter()
        .any(|result| matches!(result.action, SearchAction::ActivateTab { .. })));
}

#[test]
fn the_mini_launcher_chooses_an_open_site_without_moving_its_tab() {
    let mut mini = mini(true);
    let site = active_id(&mini.screen);
    commit_url(&mut mini.shell, site, "https://example.com/");
    present_committed(&mut mini.shell, site, "https://example.com/");
    let tabs_before = last(&mini.screen).tabs.len();

    let opened = mini.launcher("example.com", None, true);

    assert_eq!(opened.reason, zephium_ipc::OperationReason::OpenedInMini);
    assert_eq!(mini.opened().len(), 1);
    assert_eq!(last(&mini.screen).tabs.len(), tabs_before);
    assert_eq!(active_id(&mini.screen), site);
}

#[test]
fn the_mini_launcher_offers_neither_a_blank_tab_nor_a_command() {
    let mut mini = mini(true);
    assert!(mini
        .shell
        .items
        .tab(active_id(&mini.screen))
        .unwrap()
        .url
        .is_none());

    mini.search_mini("", true);
    assert!(mini.shell.search.results.is_empty());

    mini.search_mini("new", true);
    assert!(mini
        .shell
        .search
        .results
        .iter()
        .all(|result| matches!(result.action, SearchAction::OpenUrl { .. })));

    mini.search_mini("new", false);
    assert!(mini
        .shell
        .search
        .results
        .iter()
        .any(|result| result.kind == "command"));
}

#[test]
fn an_address_chosen_in_the_mini_launcher_opens_a_mini_window_not_a_tab() {
    let mut mini = mini(true);
    let tabs_before = last(&mini.screen).tabs.len();

    let opened = mini.launcher("example.org", None, true);

    assert_eq!(opened.reason, OperationReason::OpenedInMini);
    let window = mini.opened().last().expect("a mini window").window;
    let page = mini.page(window);
    assert!(mini
        .engine
        .calls()
        .iter()
        .any(|call| call.starts_with(&format!("create {page} https://example.org"))));
    assert_eq!(last(&mini.screen).tabs.len(), tabs_before);
}

#[test]
fn the_launcher_replaces_the_page_of_the_mini_window_it_edits() {
    let mut mini = mini(true);
    let (window, page) = mini.open_shown("https://example.com/");
    let tabs_before = last(&mini.screen).tabs.len();

    let opened = mini.launcher("iana.org", Some(page.to_string()), false);

    assert_eq!(opened.reason, OperationReason::OpenedInMini);
    assert!(mini.closed().is_empty());
    assert_eq!(mini.opened().len(), 1);
    assert_eq!(last(&mini.screen).tabs.len(), tabs_before);
    assert!(mini.shell.items.pending_navigation_request(page).is_some());
    commit_url(&mut mini.shell, page, "https://www.iana.org/");
    present_committed(&mut mini.shell, page, "https://www.iana.org/");
    assert_eq!(mini.page(window), page);
    assert!(!mini.engine.calls().contains(&format!("close {page}")));
}

#[test]
fn a_private_session_keeps_what_it_searches_out_of_a_mini_window() {
    let mut mini = mini(true);
    mini.shell.handle(Command::Run("window.newPrivate".into()));
    let tabs_before = last(&mini.screen).tabs.len();

    let opened = mini.launcher("example.org", None, true);

    assert_ne!(opened.reason, OperationReason::OpenedInMini);
    assert!(mini.opened().is_empty());
    assert_eq!(last(&mini.screen).tabs.len(), tabs_before + 1);
}

#[test]
fn past_the_window_limit_the_launcher_opens_a_tab_and_the_main_window_comes_forward() {
    let mut mini = mini(true);
    for index in 0..super::super::mini::MAX_MINI_WINDOWS {
        mini.launcher(&format!("site{index}.example"), None, true);
    }
    let tabs_before = last(&mini.screen).tabs.len();

    let opened = mini.launcher("one-more.example", None, true);

    assert_ne!(opened.reason, OperationReason::OpenedInMini);
    assert_eq!(last(&mini.screen).tabs.len(), tabs_before + 1);
}

#[test]
fn without_mini_windows_the_command_opens_a_new_tab() {
    let mut mini = mini(false);
    let tabs_before = last(&mini.screen).tabs.len();

    mini.shell.handle(Command::Run("window.newMini".into()));

    assert!(mini.opened().is_empty());
    assert_eq!(last(&mini.screen).tabs.len(), tabs_before + 1);
}

#[test]
fn keep_on_top_is_the_shells_to_toggle_and_project() {
    let mut mini = mini(true);
    let (window, _) = mini.open_shown("https://example.com/");
    assert!(!mini.last_view(window).unwrap().pinned);

    for (action, pinned) in [
        (crate::MiniAction::KeepOnTop(None), true),
        (crate::MiniAction::KeepOnTop(Some(true)), true),
        (crate::MiniAction::KeepOnTop(None), false),
    ] {
        mini.shell.handle(Command::Mini { window, action });
        assert_eq!(mini.last_view(window).unwrap().pinned, pinned);
    }
}

#[test]
fn mini_actions_settle_through_the_operation_ledger() {
    let mut mini = mini(true);
    let (window, _) = mini.open_shown("https://example.com/");
    let mut act = |action| {
        mini.shell
            .handle_operation(Command::Mini { window, action })
    };

    let pinned = act(crate::MiniAction::KeepOnTop(Some(true)));
    assert_eq!(pinned.outcome, OperationOutcome::Applied);
    let again = act(crate::MiniAction::KeepOnTop(Some(true)));
    assert_eq!(again.outcome, OperationOutcome::NoOp);
    let back = act(crate::MiniAction::Back);
    assert_eq!(back.outcome, OperationOutcome::Rejected);
    assert_eq!(back.reason, OperationReason::HistoryUnavailable);
    assert_ne!(
        act(crate::MiniAction::Reload).outcome,
        OperationOutcome::Rejected
    );
    assert_ne!(
        act(crate::MiniAction::Close).outcome,
        OperationOutcome::Rejected
    );
    assert_eq!(
        act(crate::MiniAction::Close).outcome,
        OperationOutcome::Rejected
    );
}

#[test]
fn the_page_takes_focus_once_its_window_is_built() {
    let mut mini = mini(true);
    let (window, page) = mini.open_shown("https://example.com/");
    mini.shell.handle(Command::MiniResized {
        window,
        size: Size::new(700.0, 500.0),
    });

    let focus = format!("focus {page}");
    let calls = mini.engine.calls();
    assert_eq!(calls.iter().filter(|call| **call == focus).count(), 1);
}

#[test]
fn a_new_mini_window_opens_at_the_size_the_last_one_was_given() {
    let mut mini = mini(true);
    let (window, _) = mini.open_shown("https://one.example/");
    assert_eq!(
        (mini.opened()[0].width, mini.opened()[0].height),
        (MINI_WINDOW_SIZE.width, MINI_WINDOW_SIZE.height)
    );
    mini.shell.handle(Command::MiniResized {
        window,
        size: Size::new(500.0, 900.0),
    });
    // A size no window can have changes nothing.
    for size in [
        Size::new(f64::NAN, 900.0),
        Size::new(500.0, -4.0),
        Size::new(0.0, 0.0),
        Size::new(1.0e9, 900.0),
    ] {
        mini.shell.handle(Command::MiniResized { window, size });
    }

    mini.shell
        .handle(Command::OpenExternal(vec!["https://two.example/".into()]));

    let second = *mini.opened().last().unwrap();
    assert_ne!(second.window, window);
    assert_eq!((second.width, second.height), (500.0, 900.0));
}

#[test]
fn a_toolbar_reporting_a_size_no_window_has_does_not_attach() {
    let mut mini = mini(true);
    mini.shell
        .handle(Command::OpenExternal(vec!["https://example.com/".into()]));
    let window = mini.opened()[0].window;

    mini.shell.handle(Command::MiniAttached {
        window,
        size: Size::new(f64::INFINITY, 400.0),
    });

    assert!(!mini.shell.mini.windows[&window].attached);
    assert!(!mini
        .engine
        .calls()
        .iter()
        .any(|call| call.starts_with(&format!("layout@{window} "))));
}

#[test]
fn a_window_that_could_not_be_built_hands_its_page_to_the_main_window() {
    let mut mini = mini(true);
    mini.shell
        .handle(Command::OpenExternal(vec!["https://example.com/".into()]));
    let window = mini.opened()[0].window;
    let page = mini.page(window);

    mini.shell.handle(Command::Mini {
        window,
        action: crate::MiniAction::Unbuilt,
    });

    assert!(mini.shell.mini.windows.is_empty());
    assert!(mini.shell.items.tab(page).is_some());
    assert!(last(&mini.screen)
        .tabs
        .iter()
        .any(|tab| tab.id == page.to_string()));
    assert_eq!(
        mini.closed(),
        vec![zephium_ipc::MiniClosedView {
            window,
            reveal_main: true,
        }]
    );
}

#[test]
fn a_built_window_ignores_a_late_report_that_it_could_not_be_built() {
    let mut mini = mini(true);
    let (window, page) = mini.open_shown("https://example.com/");

    mini.shell.handle(Command::Mini {
        window,
        action: crate::MiniAction::Unbuilt,
    });

    assert_eq!(mini.shell.mini.windows[&window].page, page);
    assert!(mini.closed().is_empty());
}

#[test]
fn actions_for_a_window_that_is_not_a_mini_window_do_nothing() {
    let mut mini = mini(true);
    let (window, page) = mini.open_shown("https://example.com/");
    let main = mini.shell.windows.focused().unwrap().id;
    let calls = mini.engine.calls().len();

    for target in [main, window + 40] {
        for action in [
            crate::MiniAction::Close,
            crate::MiniAction::OpenInMain,
            crate::MiniAction::Reload,
            crate::MiniAction::KeepOnTop(None),
            crate::MiniAction::Unbuilt,
        ] {
            let result = mini.shell.handle_operation(Command::Mini {
                window: target,
                action,
            });
            assert_eq!(result.outcome, OperationOutcome::Rejected);
            assert_eq!(result.reason, OperationReason::InvalidScope);
        }
    }

    assert_eq!(mini.engine.calls().len(), calls);
    assert!(mini.closed().is_empty());
    assert_eq!(mini.shell.mini.windows[&window].page, page);
}

#[test]
fn a_link_a_mini_page_opens_gets_its_own_mini_window() {
    let mut mini = mini(true);
    let (window, page) = mini.open_shown("https://example.com/");
    let tabs_before = last(&mini.screen).tabs.len();

    mini.shell
        .handle(Command::Engine(EngineEvent::NewWindowRequested {
            id: page,
            url: "https://popup.example/".into(),
        }));

    let opened = mini.opened();
    assert_eq!(opened.len(), 2);
    let popup = opened[1].window;
    assert_ne!(popup, window);
    assert_eq!(last(&mini.screen).tabs.len(), tabs_before);
    let child = mini.page(popup);
    assert_eq!(
        mini.shell.items.get(child).map(|item| item.placement),
        mini.shell.items.get(page).map(|item| item.placement)
    );
}

#[test]
fn past_the_window_limit_a_mini_pages_link_opens_as_a_background_tab() {
    let mut mini = mini(true);
    let urls: Vec<String> = (0..super::super::mini::MAX_MINI_WINDOWS)
        .map(|index| format!("https://site{index}.example/"))
        .collect();
    mini.shell.handle(Command::OpenExternal(urls));
    let window = mini.opened()[0].window;
    mini.shell.handle(Command::MiniAttached {
        window,
        size: Size::new(720.0, 640.0),
    });
    let page = mini.page(window);
    commit_url(&mut mini.shell, page, "https://site0.example/");
    let active = mini.shell.windows.focused().unwrap().active;
    let tabs_before = last(&mini.screen).tabs.len();

    mini.shell
        .handle(Command::Engine(EngineEvent::NewWindowRequested {
            id: page,
            url: "https://popup.example/".into(),
        }));

    assert_eq!(mini.opened().len(), super::super::mini::MAX_MINI_WINDOWS);
    assert_eq!(last(&mini.screen).tabs.len(), tabs_before + 1);
    assert_eq!(mini.shell.windows.focused().unwrap().active, active);
}

#[test]
fn a_mini_page_that_crashes_gets_a_new_view_while_another_tab_is_in_front() {
    let mut mini = mini(true);
    let (_, page) = mini.open_shown("https://example.com/");
    assert_ne!(active_id(&mini.screen), page);

    mini.shell
        .handle(Command::Engine(EngineEvent::Crashed { id: page }));

    let create = format!("create {page} ");
    assert_eq!(
        mini.engine
            .calls()
            .iter()
            .filter(|call| call.starts_with(&create))
            .count(),
        2
    );
}

#[test]
fn past_the_window_limit_a_popup_a_mini_page_opened_is_kept_as_a_background_tab() {
    let mut mini = mini(true);
    let urls: Vec<String> = (0..super::super::mini::MAX_MINI_WINDOWS)
        .map(|index| format!("https://site{index}.example/"))
        .collect();
    mini.shell.handle(Command::OpenExternal(urls));
    let window = mini.opened()[0].window;
    mini.shell.handle(Command::MiniAttached {
        window,
        size: Size::new(720.0, 640.0),
    });
    let page = mini.page(window);
    commit_url(&mut mini.shell, page, "https://site0.example/");
    let active = mini.shell.windows.focused().unwrap().active;
    let child = ItemId::generate();
    let (send, receive) = std::sync::mpsc::channel();

    mini.shell
        .handle(Command::Engine(EngineEvent::NativeTabOpened {
            id: page,
            child,
            foreground: true,
            adoption: zephium_core::ports::engine::NativeTabAdoption::new(move |ok| {
                send.send(ok).unwrap()
            }),
        }));

    assert!(receive.recv().unwrap());
    assert_eq!(mini.opened().len(), super::super::mini::MAX_MINI_WINDOWS);
    assert!(mini.shell.items.tab(child).is_some_and(TabState::has_view));
    assert!(matches!(
        mini.shell.items.get(child).map(|item| item.placement),
        Some(Placement::Space { .. })
    ));
    assert_eq!(mini.shell.windows.focused().unwrap().active, active);
}

#[test]
fn open_in_zephium_waits_for_a_browser_page_to_return_first() {
    let (mut shell, engine, chrome, screen) = setup_with_async_chrome();
    engine
        .hosts_mini_windows
        .store(true, std::sync::atomic::Ordering::Release);
    shell.handle(Command::Bootstrap);
    shell.handle(Command::SetAppSetting {
        key: "tabs.external-position".into(),
        value: "mini".into(),
    });
    let main = active_id(&screen);
    shell.handle(Command::Navigate {
        id: main,
        input: "main.example".into(),
    });
    shell.handle(Command::Engine(EngineEvent::UrlChanged {
        id: main,
        url: "https://main.example/".into(),
    }));
    present_committed(&mut shell, main, "https://main.example/");
    shell.handle(Command::OpenExternal(vec!["https://example.com/".into()]));
    let (window, page) = shell
        .mini
        .windows
        .iter()
        .map(|(window, mini)| (*window, mini.page))
        .next()
        .expect("a mini window");
    shell.handle(Command::MiniAttached {
        window,
        size: Size::new(720.0, 640.0),
    });
    shell.handle(Command::ShowBrowserPage(Some(crate::BrowserPage::Settings)));
    let release = format!("release@{window}");

    shell.handle(Command::Mini {
        window,
        action: crate::MiniAction::OpenInMain,
    });

    // The window and its page stay until the browser page has gone.
    assert_eq!(shell.mini.windows[&window].page, page);
    assert!(!engine.calls().contains(&release));

    let (revision, _) = chrome.complete_browser_return(true);
    shell.browser_chrome_restored(revision, true);

    assert_eq!(shell.active_browser_page(), None);
    assert!(!shell.mini.windows.contains_key(&window));
    assert!(engine.calls().contains(&release));
    assert_eq!(active_id(&screen), page);
}
