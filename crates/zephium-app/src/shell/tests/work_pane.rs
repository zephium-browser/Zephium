use super::*;
use crate::api::WorkPaneTarget;
use zephium_ipc::WorkPaneLayout;

type LayoutSink = Arc<Mutex<Option<LayoutState>>>;

fn setup_with_layout_sink() -> (Shell, Arc<FakeEngine>, Screen, LayoutSink) {
    let engine = Arc::new(FakeEngine::default());
    let layouts: LayoutSink = Arc::new(Mutex::new(None));
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
    let (sink, layout_sink) = (screen.clone(), layouts.clone());
    let mut shell = Shell::new(
        engine.clone(),
        Arc::new(FakeStore::default()),
        Arc::new(FakeChrome),
        Box::new(move |p| match p {
            Projection::Layout(l) => *layout_sink.lock().unwrap() = Some(l),
            p => apply_projection(&mut sink.lock().unwrap(), p),
        }),
    );
    shell.handle(Command::SetWindowSize(Size::new(1200.0, 800.0)));
    (shell, engine, screen, layouts)
}

fn pane(layouts: &LayoutSink) -> Option<WorkPaneLayout> {
    layouts
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|l| l.work_pane.clone())
}

fn enter_work(shell: &mut Shell) {
    shell.handle(Command::ShowBrowserPage(Some(crate::BrowserPage::Work)));
    assert_eq!(shell.active_browser_page(), Some(crate::BrowserPage::Work));
}

fn rect() -> Rect {
    Rect::new(300.0, 120.0, 720.0, 560.0)
}

#[test]
fn a_space_tab_floats_in_the_pane_above_full_window_work_chrome() {
    let (mut shell, engine, screen, layouts) = setup_with_layout_sink();
    shell.handle(Command::Bootstrap);
    let first = active_id(&screen);
    navigate_and_commit(&mut shell, first, "example.com");
    enter_work(&mut shell);
    assert_eq!(engine.last_layout(), Vec::<String>::new());
    assert_eq!(pane(&layouts), None);

    let shown = shell.operation_work_pane_show(WorkPaneTarget::Tab(first), rect());
    assert_eq!(shown.outcome, OperationOutcome::Deferred);
    assert_eq!(engine.last_layout(), vec![first.to_string()]);
    let layout = pane(&layouts).expect("pane projected");
    assert_eq!(layout.tab, first.to_string());
    assert!(layout.presented);
    assert_eq!(
        (layout.x, layout.y, layout.width, layout.height),
        (300.0, 120.0, 720.0, 560.0)
    );
    assert_eq!(shell.windows.focused().unwrap().active, Some(first));
    assert!(shell.discard_protected_leaves().contains(&first));

    // Geometry follows the chrome's measured hole, clamped into the window.
    let generation = layout.generation;
    shell.handle(Command::WorkPaneSetRect {
        rect: Rect::new(900.0, 700.0, 100.0, 100.0),
        generation,
    });
    let moved = pane(&layouts).unwrap();
    assert_eq!(
        (moved.x, moved.y, moved.width, moved.height),
        (720.0, 480.0, 480.0, 320.0)
    );
    shell.handle(Command::WorkPaneSetRect {
        rect: rect(),
        generation: generation.wrapping_add(7),
    });
    assert_eq!(
        pane(&layouts).unwrap().x,
        720.0,
        "stale generation is ignored"
    );

    let hidden = shell.operation_work_pane_hide();
    assert_eq!(hidden.outcome, OperationOutcome::Deferred);
    assert_eq!(engine.last_layout(), Vec::<String>::new());
    assert_eq!(pane(&layouts), None);
    assert!(
        shell.items.tab(first).is_some_and(TabState::has_view),
        "hiding keeps the page"
    );
    assert_eq!(
        shell.operation_work_pane_hide().outcome,
        OperationOutcome::NoOp
    );
    let again = shell.operation_work_pane_show(WorkPaneTarget::Tab(first), rect());
    assert_eq!(again.outcome, OperationOutcome::Deferred);
    assert!(pane(&layouts).unwrap().generation > generation);
}

#[test]
fn a_url_target_opens_an_unfocused_space_tab_in_the_pane() {
    let (mut shell, engine, screen, layouts) = setup_with_layout_sink();
    shell.handle(Command::Bootstrap);
    let first = active_id(&screen);
    navigate_and_commit(&mut shell, first, "example.com");
    enter_work(&mut shell);

    let shown = shell.operation_work_pane_show(
        WorkPaneTarget::Url("https://docs.example/page".into()),
        rect(),
    );
    assert_eq!(shown.outcome, OperationOutcome::Deferred);
    let layout = pane(&layouts).expect("pane projected");
    let opened = ItemId::parse(&layout.tab).unwrap();
    assert_ne!(opened, first);
    assert_eq!(engine.last_layout(), vec![opened.to_string()]);
    assert_eq!(shell.windows.focused().unwrap().active, Some(first));
    assert!(last(&screen).tabs.iter().any(|tab| tab.id == layout.tab));
    assert!(engine.calls().iter().any(|call| call
        == &format!("create {opened} https://docs.example/page")
        || call.starts_with(&format!("create {opened} "))));

    // Pane navigation stays inside Work and targets the pane's exact tab.
    let navigated = shell.operation_navigate(opened, "github.com".into());
    assert_eq!(navigated.outcome, OperationOutcome::Deferred);
    assert_eq!(shell.active_browser_page(), Some(crate::BrowserPage::Work));
    assert!(shell.browser_after_return.is_none());

    assert_eq!(
        shell
            .operation_work_pane_show(WorkPaneTarget::Url("javascript:alert(1)".into()), rect())
            .outcome,
        OperationOutcome::Rejected
    );
}

#[test]
fn hiding_the_pane_closes_a_page_it_opened_but_keeps_a_space_tab() {
    let (mut shell, _engine, screen, layouts) = setup_with_layout_sink();
    shell.handle(Command::Bootstrap);
    let first = active_id(&screen);
    navigate_and_commit(&mut shell, first, "example.com");
    enter_work(&mut shell);

    shell.operation_work_pane_show(
        WorkPaneTarget::Url("https://docs.example/page".into()),
        rect(),
    );
    let opened = pane(&layouts).expect("pane projected").tab;
    assert!(last(&screen).tabs.iter().any(|tab| tab.id == opened));
    shell.operation_work_pane_hide();
    assert!(shell.work_pane.is_none());
    assert!(!last(&screen).tabs.iter().any(|tab| tab.id == opened));

    shell.operation_work_pane_show(
        WorkPaneTarget::Url("https://docs.example/other".into()),
        rect(),
    );
    let opened = pane(&layouts).expect("pane projected").tab;
    shell.handle(Command::ShowBrowserPage(None));
    assert!(shell.work_pane.is_none());
    assert!(!last(&screen).tabs.iter().any(|tab| tab.id == opened));

    enter_work(&mut shell);
    shell.operation_work_pane_show(WorkPaneTarget::Tab(first), rect());
    shell.operation_work_pane_hide();
    assert!(shell.work_pane.is_none());
    assert!(last(&screen)
        .tabs
        .iter()
        .any(|tab| tab.id == first.to_string()));
}

#[test]
fn the_pane_is_cleared_by_return_page_switch_and_tab_close() {
    let (mut shell, engine, screen, layouts) = setup_with_layout_sink();
    shell.handle(Command::Bootstrap);
    let first = active_id(&screen);
    navigate_and_commit(&mut shell, first, "example.com");
    shell.handle(Command::Open);
    let second = active_id(&screen);
    navigate_and_commit(&mut shell, second, "github.com");
    enter_work(&mut shell);

    assert_eq!(
        shell
            .operation_work_pane_show(WorkPaneTarget::Tab(first), rect())
            .outcome,
        OperationOutcome::Deferred
    );
    // Open in Browse: exact return activates the pane's tab.
    assert_eq!(
        shell.operation_activate(first).outcome,
        OperationOutcome::Deferred
    );
    assert_eq!(shell.active_browser_page(), None);
    assert_eq!(pane(&layouts), None);
    assert_eq!(shell.windows.focused().unwrap().active, Some(first));
    assert_eq!(engine.last_layout(), vec![first.to_string()]);

    enter_work(&mut shell);
    shell.operation_work_pane_show(WorkPaneTarget::Tab(second), rect());
    assert_eq!(engine.last_layout(), vec![second.to_string()]);
    shell.handle(Command::ShowBrowserPage(Some(crate::BrowserPage::Settings)));
    assert_eq!(pane(&layouts), None);
    assert_eq!(engine.last_layout(), Vec::<String>::new());
    assert!(shell.work_pane.is_none());

    enter_work(&mut shell);
    shell.operation_work_pane_show(WorkPaneTarget::Tab(second), rect());
    assert_eq!(
        shell.operation_close(second).outcome,
        OperationOutcome::Deferred
    );
    assert!(shell.work_pane.is_none());
    assert_eq!(pane(&layouts), None);
    assert_eq!(shell.active_browser_page(), Some(crate::BrowserPage::Work));

    assert_eq!(
        shell
            .operation_work_pane_show(WorkPaneTarget::Tab(second), rect())
            .outcome,
        OperationOutcome::Rejected
    );
    shell.handle(Command::ShowBrowserPage(None));
    assert_eq!(
        shell
            .operation_work_pane_show(WorkPaneTarget::Tab(first), rect())
            .outcome,
        OperationOutcome::Rejected,
        "the pane exists only inside Work"
    );
}

#[test]
fn work_shortcuts_address_the_pane_tab_instead_of_the_hidden_browse_tab() {
    let (mut shell, engine, screen, layouts) = setup_with_layout_sink();
    shell.handle(Command::Bootstrap);
    let first = active_id(&screen);
    navigate_and_commit(&mut shell, first, "example.com");
    shell.handle(Command::Open);
    let second = active_id(&screen);
    navigate_and_commit(&mut shell, second, "github.com");
    enter_work(&mut shell);

    assert_eq!(
        shell.operation_run_command("tab.close").outcome,
        OperationOutcome::NoOp,
        "no pane: nothing visible to close"
    );
    assert!(shell.items.tab(second).is_some());

    shell.operation_work_pane_show(WorkPaneTarget::Tab(first), rect());
    assert_eq!(
        shell.operation_run_command("nav.reload").outcome,
        OperationOutcome::Deferred
    );
    assert!(engine
        .calls()
        .iter()
        .any(|c| c == &format!("reload {first}")));
    assert_eq!(
        shell.operation_run_command("nav.hardReload").outcome,
        OperationOutcome::Deferred
    );
    assert!(engine
        .calls()
        .iter()
        .any(|c| c == &format!("hard-reload {first}")));
    assert_eq!(
        shell.operation_run_command("tab.close").outcome,
        OperationOutcome::Deferred
    );
    assert!(
        shell.items.tab(first).is_some(),
        "tab.close hides the pane only"
    );
    assert_eq!(pane(&layouts), None);

    shell.operation_work_pane_show(WorkPaneTarget::Tab(first), rect());
    assert_eq!(
        shell.operation_run_command("work.pane.close").outcome,
        OperationOutcome::Deferred
    );
    assert_eq!(pane(&layouts), None);
    assert_eq!(
        shell
            .operation_run_command("work.pane.openInBrowse")
            .outcome,
        OperationOutcome::NoOp
    );
    shell.operation_work_pane_show(WorkPaneTarget::Tab(first), rect());
    assert_eq!(
        shell
            .operation_run_command("work.pane.openInBrowse")
            .outcome,
        OperationOutcome::Deferred
    );
    assert_eq!(shell.active_browser_page(), None);
    assert_eq!(shell.windows.focused().unwrap().active, Some(first));
}

#[test]
fn a_modal_prompt_removes_the_pane_page_from_the_stage_but_keeps_the_pane() {
    let (mut shell, engine, screen, layouts) = setup_with_layout_sink();
    shell.handle(Command::Bootstrap);
    let first = active_id(&screen);
    navigate_and_commit(&mut shell, first, "example.com");
    enter_work(&mut shell);
    shell.operation_work_pane_show(WorkPaneTarget::Tab(first), rect());
    assert!(pane(&layouts).unwrap().presented);

    shell.handle(Command::SetWindowVisible(false));
    assert_eq!(engine.last_layout(), Vec::<String>::new());
    let hidden = pane(&layouts).unwrap();
    assert!(!hidden.presented);
    assert_eq!(hidden.tab, first.to_string());

    shell.handle(Command::SetWindowVisible(true));
    assert!(pane(&layouts).unwrap().presented);
    assert_eq!(engine.last_layout(), vec![first.to_string()]);

    shell.handle(Command::Engine(EngineEvent::Crashed { id: first }));
    assert!(
        shell.items.tab(first).is_some_and(TabState::has_view),
        "the visible pane leaf is recreated after a crash"
    );
    assert_eq!(engine.last_layout(), vec![first.to_string()]);
}

#[test]
fn a_new_tab_asked_for_in_work_waits_for_chrome_and_then_fills_the_window() {
    let (mut shell, engine, chrome, screen) = setup_with_async_chrome();
    shell.handle(Command::Bootstrap);
    let page = active_id(&screen);
    shell.handle(Command::Navigate {
        id: page,
        input: "example.com".into(),
    });
    shell.handle(Command::Engine(EngineEvent::UrlChanged {
        id: page,
        url: "https://example.com/".into(),
    }));
    present_committed(&mut shell, page, "https://example.com/");
    enter_work(&mut shell);

    assert_eq!(
        shell.handle_operation(Command::Open).outcome,
        OperationOutcome::Deferred
    );
    // Nothing is opened until chrome has taken Browse back.
    assert_eq!(active_id(&screen), page);
    let (revision, _) = chrome.complete_browser_return(true);
    shell.browser_chrome_restored(revision, true);

    let opened = active_id(&screen);
    assert_ne!(opened, page);
    assert_eq!(shell.active_browser_page(), None);
    assert!(shell.browser_after_return.is_none());
    let frame = chrome.last_frame().expect("chrome was placed");
    assert!(frame.fill_width, "{frame:?}");
    assert!(engine
        .calls()
        .iter()
        .rev()
        .find(|call| call.starts_with("layout@"))
        .is_some_and(|call| !call.contains(&page.to_string())));
}
