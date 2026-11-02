use super::*;
use zephium_core::item::{BrowserOwnedTab, TabContent};

fn extracted(id: ItemId, html: &str) -> Command {
    Command::Engine(EngineEvent::HtmlExtracted {
        id,
        html: html.into(),
        truncated: false,
    })
}

fn source_of(shell: &mut Shell, id: ItemId) -> Option<Arc<crate::PageSource>> {
    let (send, receive) = std::sync::mpsc::channel();
    shell.handle(Command::PageSource {
        id,
        done: crate::SourceCompletion::new(move |document| {
            let _ = send.send(document);
        }),
    });
    receive.recv().unwrap()
}

fn extractions(engine: &FakeEngine) -> usize {
    engine
        .calls()
        .iter()
        .filter(|call| call.starts_with("extract "))
        .count()
}

#[test]
fn the_source_opens_in_a_new_tab_only_once_the_engine_has_read_the_page() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let page = active_id(&screen);
    navigate_and_commit(&mut shell, page, "docs.example/guide?x=1#top");

    let asked = shell.handle_operation(Command::Run("page.viewSource".into()));
    assert_eq!(asked.outcome, OperationOutcome::Deferred);
    assert_eq!(extractions(&engine), 1);
    assert_eq!(active_id(&screen), page, "nothing opens until the answer");

    shell.handle(extracted(page, "<html><body>\n<p>hi</p></body></html>"));

    let tab = active_id(&screen);
    assert_ne!(tab, page);
    let state = shell.items.tab(tab).unwrap();
    assert_eq!(
        state.content,
        TabContent::BrowserOwned(BrowserOwnedTab::Source)
    );
    assert!(state.url.is_none());
    assert_eq!(state.title, "view-source:docs.example/guide");
    assert!(
        shell.items.tab(page).is_some(),
        "the page stays where it was"
    );
    assert_eq!(
        shell.active_browser_page(),
        Some(crate::BrowserPage::Source)
    );

    let document = source_of(&mut shell, tab).expect("the tab's document");
    assert_eq!(document.text, "<html><body>\n<p>hi</p></body></html>");
    assert!(document.url.starts_with("https://docs.example/guide"));
    assert!(!document.truncated);
    // The page's own tab holds no document, and an unknown tab holds none.
    assert!(source_of(&mut shell, page).is_none());
    assert!(source_of(&mut shell, ItemId::generate()).is_none());
}

#[test]
fn an_answer_nobody_asked_for_opens_nothing_and_each_answer_opens_one_tab() {
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let page = active_id(&screen);
    navigate_and_commit(&mut shell, page, "docs.example");

    shell.handle(extracted(page, "<html></html>"));
    assert_eq!(active_id(&screen), page, "never asked");

    let _ = shell.handle_operation(Command::Run("page.viewSource".into()));
    shell.handle(extracted(ItemId::generate(), "<html>other</html>"));
    assert_eq!(active_id(&screen), page, "another tab's answer is stale");

    shell.handle(extracted(page, "<html>one</html>"));
    let first = active_id(&screen);
    assert_ne!(first, page);
    shell.handle(extracted(page, "<html>again</html>"));
    assert_eq!(
        active_id(&screen),
        first,
        "an answer settles its request once"
    );
}

#[test]
fn only_a_loaded_page_in_front_can_show_its_source() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let blank = active_id(&screen);
    let refused = shell.handle_operation(Command::Run("page.viewSource".into()));
    assert_eq!(refused.outcome, OperationOutcome::NoOp);

    navigate_and_commit(&mut shell, blank, "docs.example");
    shell.handle_operation(Command::ShowBrowserPage(Some(crate::BrowserPage::Settings)));
    let covered = shell.handle_operation(Command::Run("page.viewSource".into()));
    assert_eq!(covered.outcome, OperationOutcome::NoOp);
    assert_eq!(extractions(&engine), 0);
}

#[test]
fn a_source_tab_cannot_be_asked_to_show_itself_or_be_opened_by_name() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let page = active_id(&screen);
    navigate_and_commit(&mut shell, page, "docs.example");
    let _ = shell.handle_operation(Command::Run("page.viewSource".into()));
    shell.handle(extracted(page, "<html></html>"));
    let tab = active_id(&screen);

    let again = shell.handle_operation(Command::Run("page.viewSource".into()));
    assert_eq!(again.outcome, OperationOutcome::NoOp);
    assert_eq!(extractions(&engine), 1);
    let named = shell.handle_operation(Command::ShowBrowserPage(Some(crate::BrowserPage::Source)));
    assert_eq!(named.outcome, OperationOutcome::Rejected);
    assert_eq!(active_id(&screen), tab);
}

#[test]
fn source_tabs_are_not_saved_with_the_session() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, _engine, screen) = setup_with(store.clone());
    shell.handle(Command::Bootstrap);
    let page = active_id(&screen);
    navigate_and_commit(&mut shell, page, "docs.example");
    let _ = shell.handle_operation(Command::Run("page.viewSource".into()));
    shell.handle(extracted(page, "<html></html>"));
    let tab = active_id(&screen);

    shell.persist();

    let saved = store.saved.lock().unwrap().clone().expect("a session save");
    assert!(saved.items.iter().all(|item| item.id != tab));
    assert!(saved.items.iter().any(|item| item.id == page));
}

#[test]
fn old_documents_are_let_go_so_memory_stays_bounded() {
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let page = active_id(&screen);
    navigate_and_commit(&mut shell, page, "docs.example");
    let mut tabs = Vec::new();
    for at in 0..30 {
        shell.handle_operation(Command::Activate(page));
        let _ = shell.handle_operation(Command::Run("page.viewSource".into()));
        shell.handle(extracted(page, &format!("<html>{at}</html>")));
        tabs.push(active_id(&screen));
    }
    assert!(source_of(&mut shell, tabs[0]).is_none());
    assert_eq!(
        source_of(&mut shell, *tabs.last().unwrap()).unwrap().text,
        "<html>29</html>"
    );
}

#[test]
fn save_page_goes_to_the_engine_only_for_a_loaded_page_in_front() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let page = active_id(&screen);
    let saves = |engine: &FakeEngine| {
        engine
            .calls()
            .iter()
            .filter(|call| call.starts_with("save "))
            .count()
    };
    // A blank tab has nothing to save.
    let blank = shell.handle_operation(Command::Run("page.save".into()));
    assert_eq!(blank.outcome, OperationOutcome::NoOp);

    navigate_and_commit(&mut shell, page, "docs.example");
    let saved = shell.handle_operation(Command::Run("page.save".into()));
    assert_eq!(saved.outcome, OperationOutcome::Deferred);
    assert_eq!(saves(&engine), 1);

    // Nor does a browser page, or a source tab, hold a page to save.
    shell.handle_operation(Command::ShowBrowserPage(Some(crate::BrowserPage::Settings)));
    let covered = shell.handle_operation(Command::Run("page.save".into()));
    assert_eq!(covered.outcome, OperationOutcome::NoOp);
    shell.handle_operation(Command::ShowBrowserPage(None));
    let _ = shell.handle_operation(Command::Run("page.viewSource".into()));
    shell.handle(extracted(page, "<html></html>"));
    let source_tab = shell.handle_operation(Command::Run("page.save".into()));
    assert_eq!(source_tab.outcome, OperationOutcome::NoOp);
    assert_eq!(saves(&engine), 1);
}

#[test]
fn autofill_password_asks_the_engine_only_for_the_loaded_page_in_front() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let page = active_id(&screen);
    let asked = |engine: &FakeEngine| -> Vec<String> {
        engine
            .calls()
            .into_iter()
            .filter(|call| call.starts_with("autofill "))
            .collect()
    };
    let run =
        |shell: &mut Shell| shell.handle_operation(Command::Run("page.autofillPassword".into()));

    assert_eq!(run(&mut shell).outcome, OperationOutcome::NoOp);
    navigate_and_commit(&mut shell, page, "login.example");
    assert_eq!(run(&mut shell).outcome, OperationOutcome::Deferred);
    assert_eq!(asked(&engine), [format!("autofill {page}")]);

    // Chrome's own pages are not web pages, and Work is never offered it.
    for covering in [crate::BrowserPage::Settings, crate::BrowserPage::Work] {
        shell.handle_operation(Command::ShowBrowserPage(Some(covering)));
        assert_eq!(run(&mut shell).outcome, OperationOutcome::NoOp);
    }
    shell.handle_operation(Command::ShowBrowserPage(None));
    assert_eq!(asked(&engine).len(), 1);

    // A private window's page gets the system's passwords like any other.
    shell.handle(Command::Run("window.newPrivate".into()));
    let private = active_id(&screen);
    assert!(shell.incognito_profile(shell.windows.focused().unwrap().profile));
    navigate_and_commit(&mut shell, private, "login.example");
    assert_eq!(run(&mut shell).outcome, OperationOutcome::Deferred);
    assert_eq!(asked(&engine).last(), Some(&format!("autofill {private}")));
}
