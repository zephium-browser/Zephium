use super::*;
use crate::home::{
    HomeCommand, HomeMaps, HomeMapsAttachment, MapKind, MapRefusal, MapStart, Reply,
    ResultsDocument, ResultsRefusal,
};
use zephium_core::home::{HomeMode, MapId, Origin, SearchId, SearchKind, MAX_SEARCHES_PER_PROFILE};
use zephium_core::ports::engine::{DocumentReadId, PageHtml};
use zephium_core::search::SearchEngine;

#[derive(Default)]
struct FakeMaps {
    answer: Mutex<Option<Result<MapKind, MapRefusal>>>,
    started: Mutex<Vec<MapStart>>,
    forgotten: Mutex<Vec<MapId>>,
    profiles: Mutex<Vec<ProfileId>>,
    /// Maps whose driver the host has given up.
    dropped: Mutex<Vec<MapId>>,
}

impl HomeMaps for FakeMaps {
    fn start(&self, start: MapStart) -> Result<MapKind, MapRefusal> {
        self.started.lock().unwrap().push(start);
        self.answer
            .lock()
            .unwrap()
            .unwrap_or(Err(MapRefusal::NotAMap))
    }
    fn holds(&self, map: MapId) -> bool {
        self.started.lock().unwrap().iter().any(|s| s.map == map)
            && !self.dropped.lock().unwrap().contains(&map)
            && !self.forgotten.lock().unwrap().contains(&map)
    }
    fn forget(&self, map: MapId) {
        self.forgotten.lock().unwrap().push(map);
    }
    fn forget_profile(&self, profile: ProfileId) {
        self.profiles.lock().unwrap().push(profile);
    }
}

type Seen = Arc<Mutex<Vec<Projection>>>;

fn home_shell(
    answer: Result<MapKind, MapRefusal>,
) -> (Shell, Arc<FakeEngine>, Arc<FakeMaps>, Seen) {
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    let engine = Arc::new(FakeEngine::default());
    let mut shell = Shell::new(
        engine.clone(),
        Arc::new(FakeStore::default()),
        Arc::new(FakeChrome),
        Box::new(move |projection| sink.lock().unwrap().push(projection)),
    );
    shell.handle(Command::SetWindowSize(Size::new(1200.0, 800.0)));
    shell.handle(Command::Bootstrap);
    shell.tab_preferences.home_mode = HomeMode::Home;
    let maps = Arc::new(FakeMaps::default());
    *maps.answer.lock().unwrap() = Some(answer);
    shell.handle(Command::Home(Box::new(HomeCommand::Attach(
        HomeMapsAttachment(maps.clone()),
    ))));
    (shell, engine, maps, seen)
}

fn front(shell: &Shell) -> ItemId {
    shell.windows.focused().unwrap().active.unwrap()
}

fn context(shell: &Shell, tab: ItemId) -> zephium_ipc::SearchContext {
    let window = shell.windows.focused().unwrap();
    zephium_ipc::SearchContext {
        window_id: window.id.to_string(),
        profile_id: window.profile.to_string(),
        space_id: window.space.to_string(),
        session_id: format!("newtab:{tab}:1"),
        request_id: "request-1".into(),
        plain: None,
    }
}

/// Types `query` into Home's field and runs the row of `kind`.
fn run_from_home(shell: &mut Shell, query: &str, kind: &str, plain: bool) -> OperationDisposition {
    let tab = front(shell);
    let mut context = context(shell, tab);
    shell.handle(Command::SearchScoped {
        mini: false,
        query: query.into(),
        context: Box::new(context.clone()),
    });
    let action = shell
        .search
        .results
        .iter()
        .find(|result| result.kind == kind)
        .map(|result| result.action.clone())
        .expect("the row is offered");
    context.plain = plain.then_some(true);
    shell.handle_operation(Command::RunSearchAction {
        context: Box::new(context),
        action,
        background: false,
        replace: None,
        new_mini: false,
    })
}

fn only_search(shell: &Shell) -> zephium_core::home::Search {
    let records = shell.home_records().unwrap();
    assert_eq!(records.searches.len(), 1);
    records.searches[0].clone()
}

fn shown(seen: &Seen) -> Vec<String> {
    seen.lock()
        .unwrap()
        .iter()
        .filter_map(|projection| match projection {
            Projection::HomeShow { search, .. } => Some(search.clone()),
            _ => None,
        })
        .collect()
}

fn read(
    shell: &mut Shell,
    command: impl FnOnce(Reply<Result<ResultsDocument, ResultsRefusal>>) -> HomeCommand,
) -> std::sync::mpsc::Receiver<Result<ResultsDocument, ResultsRefusal>> {
    let (send, receive) = std::sync::mpsc::channel();
    shell.handle(Command::Home(Box::new(command(Reply::new(
        move |answer| {
            let _ = send.send(answer);
        },
    )))));
    receive
}

fn reads(engine: &FakeEngine) -> Vec<String> {
    engine
        .calls()
        .into_iter()
        .filter(|call| call.starts_with("read "))
        .collect()
}

fn engine_url(text: &str) -> url::Url {
    SearchEngine::default().search(text).unwrap()
}

fn read_id(engine: &FakeEngine, at: usize) -> DocumentReadId {
    DocumentReadId(
        reads(engine)[at]
            .rsplit(' ')
            .next()
            .unwrap()
            .parse()
            .unwrap(),
    )
}

fn loaded(shell: &mut Shell, tab: ItemId, url: &str) {
    commit_url(shell, tab, url);
    shell.handle(Command::Engine(EngineEvent::LoadingChanged {
        id: tab,
        loading: false,
    }));
}

fn answered(shell: &mut Shell, tab: ItemId, read: DocumentReadId, url: &str, html: &str) {
    shell.handle(Command::Engine(EngineEvent::DocumentRead {
        id: tab,
        read,
        url: url.into(),
        html: PageHtml(html.into()),
        truncated: false,
    }));
}

fn open_page(shell: &mut Shell, search: SearchId, entry: &str) -> ItemId {
    let opened = shell.handle_operation(Command::Home(Box::new(HomeCommand::OpenPage {
        search,
        slot: 0,
        entry: entry.into(),
        url: Some(url::Url::parse(&format!("https://{entry}.example/")).unwrap()),
    })));
    assert_eq!(opened.outcome, OperationOutcome::Applied);
    front(shell)
}

fn today(shell: &Shell) -> Vec<ItemId> {
    let space = shell.windows.focused().unwrap().space;
    shell
        .items
        .roots(Placement::Space {
            space,
            section: SpaceSection::Today,
        })
        .to_vec()
}

fn homes(shell: &Shell) -> Vec<ItemId> {
    let space = shell.windows.focused().unwrap().space;
    shell
        .regular_tabs(space)
        .into_iter()
        .filter(|id| shell.is_home_tab(*id))
        .collect()
}

fn navigations(engine: &FakeEngine, tab: ItemId) -> usize {
    let prefix = format!("navigate {tab} ");
    engine
        .calls()
        .iter()
        .filter(|call| call.starts_with(&prefix))
        .count()
}

fn search_of(shell: &Shell, text: &str) -> zephium_core::home::Search {
    shell
        .home_records()
        .unwrap()
        .searches
        .into_iter()
        .find(|search| search.text == text)
        .expect("the search is kept")
}

#[test]
fn a_place_stays_on_home_as_a_map() {
    let (mut shell, engine, maps, seen) = home_shell(Ok(MapKind::Place));
    let home = front(&shell);
    assert!(shell.is_home_tab(home));
    let ran = run_from_home(&mut shell, "coffee near shibuya", "search", false);
    assert_eq!(ran.reason, OperationReason::HomeMapStarted);
    assert_eq!(front(&shell), home);
    assert!(shell.is_home_tab(home), "Home did not navigate");
    assert!(!engine
        .calls()
        .iter()
        .any(|call| call.starts_with("navigate")));
    let search = only_search(&shell);
    assert_eq!(search.kind, SearchKind::Place);
    assert_eq!(search.text, "coffee near shibuya");
    let started = maps.started.lock().unwrap().clone();
    assert_eq!(started.len(), 1);
    assert_eq!(Some(started[0].map), search.map);
    assert!(!started[0].private);
    assert_eq!(shown(&seen), vec![search.id.to_string()]);

    // The same words again show the same map, without a second.
    run_from_home(&mut shell, "Coffee  near Shibuya", "search", false);
    assert_eq!(maps.started.lock().unwrap().len(), 1);
    assert_eq!(only_search(&shell).id, search.id);
}

#[test]
fn a_search_that_is_no_map_loads_its_results_as_before_and_is_recorded() {
    let (mut shell, _engine, _maps, _seen) = home_shell(Err(MapRefusal::NotAMap));
    let home = front(&shell);
    let ran = run_from_home(&mut shell, "rust lifetimes", "search", false);
    assert_ne!(ran.outcome, OperationOutcome::Rejected);
    assert_ne!(ran.reason, OperationReason::HomeMapStarted);
    assert!(
        !shell.is_home_tab(home),
        "the tab left Home for its results"
    );
    let search = only_search(&shell);
    assert_eq!(search.kind, SearchKind::Other);
    assert_eq!(search.results, Some(home));
    let view = shell.home_tab_search(home).unwrap();
    assert_eq!(view.id, search.id.to_string());
    assert!(view.results);
}

#[test]
fn a_plain_run_and_classic_mode_never_start_a_map() {
    let (mut shell, _engine, maps, _seen) = home_shell(Ok(MapKind::Place));
    run_from_home(&mut shell, "coffee near shibuya", "search", true);
    assert!(maps.started.lock().unwrap().is_empty());
    assert_eq!(only_search(&shell).kind, SearchKind::Other);

    let (mut shell, _engine, maps, _seen) = home_shell(Ok(MapKind::Place));
    shell.tab_preferences.home_mode = HomeMode::Classic;
    let home = front(&shell);
    assert!(!shell.is_home_tab(home));
    run_from_home(&mut shell, "coffee near shibuya", "search", false);
    assert!(maps.started.lock().unwrap().is_empty());
    assert!(shell.home_records().is_none());
    assert!(shell.home_tab_search(home).is_none());
}

#[test]
fn an_address_from_home_is_kept_without_a_map() {
    let (mut shell, _engine, maps, _seen) = home_shell(Ok(MapKind::Place));
    let home = front(&shell);
    run_from_home(&mut shell, "docs.rs", "url", false);
    assert!(maps.started.lock().unwrap().is_empty());
    let search = only_search(&shell);
    assert_eq!(search.kind, SearchKind::Address);
    assert_eq!(search.text, "docs.rs");
    assert_eq!(search.map, None);
    assert_eq!(search.pages.len(), 1);
    assert_eq!(search.pages[0].tab, home);
    assert_eq!(search.pages[0].host, "docs.rs");
}

#[test]
fn results_are_read_from_a_background_tab_once_it_has_loaded() {
    let (mut shell, engine, _maps, _seen) = home_shell(Ok(MapKind::Place));
    let home = front(&shell);
    run_from_home(&mut shell, "coffee near shibuya", "search", false);
    let search = only_search(&shell);
    let map = search.map.unwrap();
    let url = engine_url("coffee near shibuya");

    let other = read(&mut shell, |done| HomeCommand::ReadResults {
        map,
        url: engine_url("tea near shibuya"),
        done,
    });
    assert_eq!(other.try_recv().unwrap(), Err(ResultsRefusal::Unknown));
    let unknown = read(&mut shell, |done| HomeCommand::ReadResults {
        map: MapId(map.0 + 99),
        url: url.clone(),
        done,
    });
    assert_eq!(unknown.try_recv().unwrap(), Err(ResultsRefusal::Unknown));

    let answer = read(&mut shell, |done| HomeCommand::ReadResults {
        map,
        url: url.clone(),
        done,
    });
    assert_eq!(front(&shell), home, "the results load behind Home");
    let tab = only_search(&shell).results.unwrap();
    assert_ne!(tab, home);
    assert!(shell.discard_protected_leaves().contains(&tab));
    assert!(reads(&engine).is_empty(), "nothing is read while it loads");
    commit_url(&mut shell, tab, url.as_str());
    shell.handle(Command::Engine(EngineEvent::LoadingChanged {
        id: tab,
        loading: false,
    }));
    let asked = reads(&engine);
    assert_eq!(asked.len(), 1);
    let id: u64 = asked[0].rsplit(' ').next().unwrap().parse().unwrap();

    // A read answered for another request is not this one's.
    shell.handle(Command::Engine(EngineEvent::DocumentRead {
        id: tab,
        read: DocumentReadId(id + 1),
        url: url.to_string(),
        html: PageHtml("<main>late</main>".into()),
        truncated: false,
    }));
    assert!(answer.try_recv().is_err());
    shell.handle(Command::Engine(EngineEvent::DocumentRead {
        id: tab,
        read: DocumentReadId(id),
        url: url.to_string(),
        html: PageHtml("<main>results</main>".into()),
        truncated: false,
    }));
    let document = answer.try_recv().unwrap().unwrap();
    assert_eq!(document.url, url);
    assert_eq!(document.html.0, "<main>results</main>");
    assert_ne!(front(&shell), tab);
    assert!(
        !shell.discard_protected_leaves().contains(&tab),
        "protected only while a read waits"
    );

    // Read again as the page is now, in the same tab.
    let again = read(&mut shell, |done| HomeCommand::ReadAgain { map, done });
    assert_eq!(reads(&engine).len(), 2);
    let id: u64 = reads(&engine)[1]
        .rsplit(' ')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    shell.handle(Command::Engine(EngineEvent::DocumentRead {
        id: tab,
        read: DocumentReadId(id),
        url: "https://consent.google.com/ml?continue=x".into(),
        html: PageHtml("<form></form>".into()),
        truncated: false,
    }));
    assert_eq!(again.try_recv().unwrap(), Err(ResultsRefusal::Redirected));
}

#[test]
fn a_results_tab_closed_or_failed_before_it_reads_answers_unavailable() {
    let (mut shell, _engine, _maps, _seen) = home_shell(Ok(MapKind::Place));
    run_from_home(&mut shell, "coffee near shibuya", "search", false);
    let map = only_search(&shell).map.unwrap();
    let url = engine_url("coffee near shibuya");
    let answer = read(&mut shell, |done| HomeCommand::ReadResults {
        map,
        url: url.clone(),
        done,
    });
    let tab = only_search(&shell).results.unwrap();
    let _ = shell.handle_operation(Command::Close(tab));
    assert_eq!(answer.try_recv().unwrap(), Err(ResultsRefusal::Unavailable));
    assert_eq!(only_search(&shell).results, None);

    let abandoned = read(&mut shell, |done| HomeCommand::ReadResults {
        map,
        url,
        done,
    });
    shell.handle(Command::Home(Box::new(HomeCommand::ReadAbandoned { map })));
    assert!(shell.home.reads.is_empty());
    assert!(abandoned.try_recv().is_err());
}

#[test]
fn a_page_opened_from_a_map_returns_to_home_when_closed() {
    let (mut shell, _engine, _maps, seen) = home_shell(Ok(MapKind::Place));
    let home = front(&shell);
    run_from_home(&mut shell, "coffee near shibuya", "search", false);
    let search = only_search(&shell).id;
    let opened = shell.handle_operation(Command::Home(Box::new(HomeCommand::OpenPage {
        search,
        slot: 0,
        entry: "a1".into(),
        url: Some(url::Url::parse("https://cafe.example/").unwrap()),
    })));
    assert_eq!(opened.outcome, OperationOutcome::Applied);
    let page = front(&shell);
    assert_ne!(page, home);
    assert_eq!(shell.home_origin(page).unwrap().search, search);
    let record = only_search(&shell);
    assert_eq!(record.pages.len(), 1);
    assert_eq!(record.pages[0].row.as_ref().unwrap().entry, "a1");

    seen.lock().unwrap().clear();
    let _ = shell.handle_operation(Command::Close(page));
    assert_eq!(front(&shell), home);
    assert_eq!(shown(&seen), vec![search.to_string()]);
    let record = only_search(&shell);
    assert!(!record.pages[0].open);
    assert_eq!(record.pages[0].host, "cafe.example");

    // Only an https page is opened, for a search of this space.
    for (search, url) in [
        (search, "http://cafe.example/"),
        (SearchId(search.0 + 7), "https://cafe.example/"),
    ] {
        let refused = shell.handle_operation(Command::Home(Box::new(HomeCommand::OpenPage {
            search,
            slot: 0,
            entry: "a1".into(),
            url: Some(url::Url::parse(url).unwrap()),
        })));
        assert_eq!(refused.outcome, OperationOutcome::Rejected);
    }
}

#[test]
fn home_stays_open_and_a_new_tab_shows_it() {
    let (mut shell, _engine, _maps, _seen) = home_shell(Err(MapRefusal::NotAMap));
    let home = front(&shell);
    let shown = shell.handle_operation(Command::Run("tab.new".into()));
    assert_ne!(shown.outcome, OperationOutcome::Rejected);
    assert_eq!(front(&shell), home, "one Home per space");
    assert_eq!(
        shell
            .regular_tabs(shell.windows.focused().unwrap().space)
            .len(),
        1
    );

    navigate_and_commit(&mut shell, home, "example.com");
    let _ = shell.handle_operation(Command::Run("tab.new".into()));
    let fresh = front(&shell);
    assert_ne!(fresh, home);
    assert!(shell.is_home_tab(fresh));
    let _ = shell.handle_operation(Command::Close(fresh));
    assert!(
        shell.items.tab(fresh).is_some(),
        "Home is not closed beside other tabs"
    );
}

#[test]
fn forgetting_a_search_ends_its_map_and_leaves_its_tabs_open() {
    let (mut shell, _engine, maps, _seen) = home_shell(Ok(MapKind::Place));
    run_from_home(&mut shell, "coffee near shibuya", "search", false);
    let search = only_search(&shell);
    let _ = shell.handle_operation(Command::Home(Box::new(HomeCommand::OpenPage {
        search: search.id,
        slot: 0,
        entry: "a1".into(),
        url: Some(url::Url::parse("https://cafe.example/").unwrap()),
    })));
    let page = front(&shell);
    let forgot = shell.handle_operation(Command::Home(Box::new(HomeCommand::ForgetSearch {
        search: search.id,
    })));
    assert_eq!(forgot.outcome, OperationOutcome::Applied);
    assert!(shell.items.tab(page).is_some());
    assert!(shell.home_origin(page).is_none());
    assert_eq!(*maps.forgotten.lock().unwrap(), vec![search.map.unwrap()]);
    assert!(shell.home_records().unwrap().searches.is_empty());
    let again = shell.handle_operation(Command::Home(Box::new(HomeCommand::ForgetSearch {
        search: search.id,
    })));
    assert_eq!(again.reason, OperationReason::HomeSearchUnknown);
}

#[test]
fn a_profile_gone_takes_its_searches_and_memo_with_it() {
    let (mut shell, _engine, maps, _seen) = home_shell(Ok(MapKind::Place));
    run_from_home(&mut shell, "coffee near shibuya", "search", false);
    let search = only_search(&shell);
    shell.home_forget_profile(search.profile);
    assert!(shell.home_records().unwrap().searches.is_empty());
    assert_eq!(*maps.profiles.lock().unwrap(), vec![search.profile]);
    assert_eq!(*maps.forgotten.lock().unwrap(), vec![search.map.unwrap()]);
}

#[test]
fn a_row_dragged_away_from_its_search_no_longer_names_it() {
    let (mut shell, _engine, _maps, _seen) = home_shell(Ok(MapKind::Place));
    let home = front(&shell);
    run_from_home(&mut shell, "coffee near shibuya", "search", false);
    let search = only_search(&shell).id;
    let mut pages = Vec::new();
    for entry in ["a1", "a2"] {
        let _ = shell.handle_operation(Command::Home(Box::new(HomeCommand::OpenPage {
            search,
            slot: 0,
            entry: entry.into(),
            url: Some(url::Url::parse(&format!("https://{entry}.example/")).unwrap()),
        })));
        pages.push(front(&shell));
    }
    let space = shell.windows.focused().unwrap().space;
    let today = Placement::Space {
        space,
        section: SpaceSection::Today,
    };
    let roots = shell.items.roots(today).to_vec();
    let at = roots.iter().position(|id| *id == pages[0]).unwrap();
    assert_eq!(
        roots.get(at + 1),
        Some(&pages[1]),
        "a search's rows stay together"
    );

    // A tab dropped between them moves past them.
    let _ = shell.handle_operation(Command::SetTabEssential {
        id: home,
        essential: false,
        before: Some(pages[1]),
    });
    let roots = shell.items.roots(today).to_vec();
    let position = |id: ItemId| roots.iter().position(|root| *root == id).unwrap();
    assert_eq!(position(pages[1]), position(pages[0]) + 1);
    assert_eq!(position(home), position(pages[1]) + 1);
    assert!(shell.home_origin(home).is_none());

    // One dragged away from them no longer names the search.
    let _ = shell.handle_operation(Command::SetTabEssential {
        id: pages[1],
        essential: false,
        before: None,
    });
    let roots = shell.items.roots(today).to_vec();
    assert_eq!(roots.last(), Some(&pages[1]));
    assert_ne!(roots[roots.len() - 2], pages[0]);
    assert!(shell.home_origin(pages[1]).is_none());
    assert!(shell.home_origin(pages[0]).is_some());
}

#[test]
fn a_page_the_map_offers_no_address_for_opens_nothing() {
    let (mut shell, _engine, _maps, _seen) = home_shell(Ok(MapKind::Place));
    let home = front(&shell);
    run_from_home(&mut shell, "coffee near shibuya", "search", false);
    let search = only_search(&shell).id;
    let refused = shell.handle_operation(Command::Home(Box::new(HomeCommand::OpenPage {
        search,
        slot: 0,
        entry: "a1".into(),
        url: None,
    })));
    assert_eq!(refused.reason, OperationReason::HomeTargetUnavailable);
    assert_eq!(front(&shell), home);
    shell.tab_preferences.home_mode = HomeMode::Classic;
    let off = shell.handle_operation(Command::Home(Box::new(HomeCommand::FocusSearch { search })));
    assert_eq!(off.reason, OperationReason::HomeUnavailable);
}

#[test]
fn a_space_left_with_home_in_front_shows_home_on_return() {
    let (mut shell, _engine, _maps, _seen) = home_shell(Err(MapRefusal::NotAMap));
    let first = shell.windows.focused().unwrap().space;
    let page = front(&shell);
    navigate_and_commit(&mut shell, page, "example.com");
    let _ = shell.handle_operation(Command::Run("tab.new".into()));
    assert!(shell.is_home_tab(front(&shell)));

    let _ = shell.handle_operation(Command::Space(Box::new(crate::api::SpaceCommand::Create {
        name: "Second".into(),
        look: zephium_core::spaces::SpaceLook::default(),
    })));
    let second = shell.windows.focused().unwrap().space;
    assert_ne!(second, first);
    let switch = |shell: &mut Shell, space| {
        shell.handle_operation(Command::Space(Box::new(crate::api::SpaceCommand::Switch(
            space,
        ))))
    };
    let _ = switch(&mut shell, first);
    assert!(shell.is_home_tab(front(&shell)));
    let _ = switch(&mut shell, second);
    let _ = switch(&mut shell, first);
    assert!(shell.is_home_tab(front(&shell)), "Home is back in front");

    shell.handle(Command::Engine(EngineEvent::TitleChanged {
        id: page,
        title: "Example".into(),
    }));
    let _ = shell.handle_operation(Command::Activate(page));
    let _ = switch(&mut shell, second);
    let _ = switch(&mut shell, first);
    assert_eq!(front(&shell), page, "the page left in front comes back");
}

#[test]
fn home_commands_and_documents_log_no_address() {
    let url = engine_url("coffee near shibuya");
    let command = HomeCommand::ReadResults {
        map: MapId(1),
        url: url.clone(),
        done: Reply::new(|_| {}),
    };
    let document = ResultsDocument {
        url,
        html: PageHtml("<a href='https://cafe.example/'>Cafe</a>".into()),
        truncated: false,
    };
    for logged in [
        format!("{:?}", Command::Home(Box::new(command))),
        format!("{document:?}"),
    ] {
        assert!(
            !logged.contains("http") && !logged.contains("coffee"),
            "{logged}"
        );
    }
}

#[test]
fn every_way_to_a_new_tab_shows_the_one_home_of_its_space() {
    let (mut shell, _engine, _maps, _seen) = home_shell(Err(MapRefusal::NotAMap));
    let first_space = shell.windows.focused().unwrap().space;
    let home = front(&shell);
    for _ in 0..2 {
        let _ = shell.handle_operation(Command::Run("tab.new".into()));
        assert_eq!(front(&shell), home);
        let _ = shell.handle_operation(Command::Open);
        assert_eq!(front(&shell), home);
    }
    assert_eq!(shell.regular_tabs(first_space), vec![home]);

    let _ = shell.handle_operation(Command::Space(Box::new(crate::api::SpaceCommand::Create {
        name: "Second".into(),
        look: zephium_core::spaces::SpaceLook::default(),
    })));
    let second = front(&shell);
    assert!(shell.is_home_tab(second));
    let _ = shell.handle_operation(Command::Run("tab.new".into()));
    assert_eq!(front(&shell), second);
    assert_eq!(homes(&shell), vec![second]);
    let _ = shell.handle_operation(Command::Space(Box::new(crate::api::SpaceCommand::Switch(
        first_space,
    ))));
    // A blank tab left behind is not kept; the space gets one Home again.
    assert_eq!(shell.regular_tabs(first_space), vec![front(&shell)]);
    assert_eq!(homes(&shell), vec![front(&shell)]);

    let before: Vec<_> = shell.profiles.iter().map(|profile| profile.id).collect();
    let created = shell.handle_operation(Command::CreateProfile {
        name: "Work".into(),
        avatar: zephium_core::profiles::ProfileAvatar {
            style: zephium_core::profiles::ProfileAvatarStyle::Symbol,
            color: 2,
            symbol: 5,
            revision: 0,
        },
        photo: None,
        switch: false,
    });
    assert_eq!(created.outcome, OperationOutcome::Applied);
    let work = shell
        .profiles
        .iter()
        .map(|profile| profile.id)
        .find(|id| !before.contains(id))
        .unwrap();
    let personal = shell.windows.focused().unwrap().profile;
    let _ = shell.handle_operation(Command::SwitchProfile(work));
    let work_home = front(&shell);
    assert!(shell.is_home_tab(work_home));
    let _ = shell.handle_operation(Command::Run("tab.new".into()));
    assert_eq!(front(&shell), work_home);
    assert_eq!(homes(&shell), vec![work_home]);
    let _ = shell.handle_operation(Command::SwitchProfile(personal));
    assert_eq!(homes(&shell), vec![front(&shell)]);

    shell.handle(Command::Run("window.newPrivate".into()));
    let private = front(&shell);
    assert!(shell.is_home_tab(private));
    let _ = shell.handle_operation(Command::Run("tab.new".into()));
    assert_eq!(front(&shell), private);
    assert_eq!(homes(&shell), vec![private]);
}

#[test]
fn a_new_tab_page_setting_opens_that_page_and_leaves_home_to_private() {
    let (mut shell, engine, _maps, _seen) = home_shell(Err(MapRefusal::NotAMap));
    let space = shell.windows.focused().unwrap().space;
    let first = front(&shell);
    let destination = "https://start.example/";
    shell.handle(Command::SetAppSetting {
        key: "tabs.new-page".into(),
        value: destination.into(),
    });
    assert!(!shell.is_home_tab(first));
    let mut opened = Vec::new();
    for _ in 0..2 {
        let _ = shell.handle_operation(Command::Run("tab.new".into()));
        let tab = front(&shell);
        assert!(!opened.contains(&tab) && tab != first);
        assert!(engine
            .calls()
            .iter()
            .any(|call| call.contains(&tab.to_string()) && call.contains(destination)));
        opened.push(tab);
    }
    assert_eq!(shell.regular_tabs(space).len(), 3);
    assert!(homes(&shell).is_empty());

    shell.handle(Command::Run("window.newPrivate".into()));
    let private = front(&shell);
    assert!(shell.is_home_tab(private), "a private new tab is Home");
    let _ = shell.handle_operation(Command::Run("tab.new".into()));
    assert_eq!(front(&shell), private);
}

#[test]
fn a_private_search_keeps_its_map_private_and_ends_with_the_session() {
    let (mut shell, _engine, maps, _seen) = home_shell(Ok(MapKind::Place));
    let regular = shell.windows.focused().unwrap().profile;
    shell.handle(Command::Run("window.newPrivate".into()));
    let private = shell.windows.focused().unwrap().profile;
    assert_ne!(private, regular);
    assert!(shell.is_home_tab(front(&shell)));
    run_from_home(&mut shell, "coffee near shibuya", "search", false);
    let search = only_search(&shell);
    assert_eq!(search.profile, private);
    let started = maps.started.lock().unwrap().clone();
    assert_eq!(started.len(), 1);
    assert!(started[0].private, "the map reads and writes no memo");
    assert_eq!(started[0].profile, private);

    // ⌘W on the lone private Home ends the session, and its searches go.
    let _ = shell.handle_operation(Command::Run("tab.close".into()));
    assert_eq!(shell.windows.focused().unwrap().profile, regular);
    assert_eq!(*maps.profiles.lock().unwrap(), vec![private]);
    assert_eq!(*maps.forgotten.lock().unwrap(), vec![search.map.unwrap()]);
    assert!(shell.home_records().unwrap().searches.is_empty());
}

#[test]
fn home_alone_in_a_regular_space_stays_on_close() {
    let (mut shell, _engine, _maps, _seen) = home_shell(Err(MapRefusal::NotAMap));
    let home = front(&shell);
    let space = shell.windows.focused().unwrap().space;
    for _ in 0..2 {
        let _ = shell.handle_operation(Command::Run("tab.close".into()));
        assert_eq!(front(&shell), home);
        assert_eq!(shell.regular_tabs(space), vec![home]);
    }

    // Beside another tab, ⌘W on Home does nothing either.
    navigate_and_commit(&mut shell, home, "example.com");
    let _ = shell.handle_operation(Command::Run("tab.new".into()));
    let fresh = front(&shell);
    assert!(shell.is_home_tab(fresh));
    let _ = shell.handle_operation(Command::Run("tab.close".into()));
    assert_eq!(front(&shell), fresh);
    assert_eq!(shell.regular_tabs(space).len(), 2);
}

#[test]
fn a_link_from_a_page_of_a_search_joins_its_run_and_closing_returns_to_its_opener() {
    let (mut shell, _engine, _maps, seen) = home_shell(Ok(MapKind::Place));
    let home = front(&shell);
    run_from_home(&mut shell, "coffee near shibuya", "search", false);
    let search = only_search(&shell).id;
    let first = open_page(&mut shell, search, "a1");
    commit_url(&mut shell, first, "https://a1.example/");
    let second = open_page(&mut shell, search, "a2");
    let _ = shell.handle_operation(Command::Activate(first));
    let before = today(&shell);
    shell.handle(Command::Engine(EngineEvent::NewWindowRequested {
        id: first,
        url: "https://menu.example/".into(),
    }));
    let link = today(&shell)
        .into_iter()
        .find(|id| !before.contains(id))
        .expect("the link opened a tab");
    assert_eq!(
        shell.home_origin(link),
        Some(Origin {
            search,
            opener: Some(first),
        })
    );
    assert_eq!(shell.home_tab_search(link).unwrap().id, search.to_string());
    let roots = today(&shell);
    let at = roots.iter().position(|id| *id == first).unwrap();
    assert_eq!(roots[at..at + 3], [first, second, link], "after its run");

    let _ = shell.handle_operation(Command::Activate(link));
    seen.lock().unwrap().clear();
    let _ = shell.handle_operation(Command::Close(link));
    assert_eq!(front(&shell), first, "the opener takes over");
    assert!(shown(&seen).is_empty());

    // Opened from Home itself: Home takes over, at the search.
    let _ = shell.handle_operation(Command::Close(first));
    assert_eq!(front(&shell), home);
    assert_eq!(shown(&seen), vec![search.to_string()]);

    // An opener closed first is no longer one.
    let _ = shell.handle_operation(Command::Activate(second));
    shell.handle(Command::Engine(EngineEvent::NewWindowRequested {
        id: second,
        url: "https://menu.example/".into(),
    }));
    let orphan = *today(&shell).last().unwrap();
    assert_eq!(shell.home_origin(orphan).unwrap().opener, Some(second));
    let _ = shell.handle_operation(Command::Close(second));
    assert_eq!(shell.home_origin(orphan).unwrap().opener, None);
    let _ = shell.handle_operation(Command::Activate(orphan));
    seen.lock().unwrap().clear();
    let _ = shell.handle_operation(Command::Close(orphan));
    assert_eq!(front(&shell), home);
    assert_eq!(shown(&seen), vec![search.to_string()]);
}

#[test]
fn a_tab_not_opened_from_home_follows_tabs_after_close() {
    for preference in ["next", "previous"] {
        let (mut shell, _engine, _maps, seen) = home_shell(Err(MapRefusal::NotAMap));
        shell.operation_set_app_setting("tabs.after-close".into(), preference.into());
        let mut pages = Vec::new();
        for host in ["one.example", "two.example", "three.example"] {
            let _ = shell.handle_operation(Command::Run("tab.new".into()));
            let tab = front(&shell);
            navigate_and_commit(&mut shell, tab, host);
            pages.push(tab);
        }
        let window = shell.windows.focused().unwrap();
        let pool = shell.successor_pool(window.profile, window.space);
        assert_eq!(pool.len(), 3, "{preference}");
        let middle = pool[1];
        let expected = match preference {
            "next" => pool[2],
            _ => pool[0],
        };
        let _ = shell.handle_operation(Command::Activate(middle));
        seen.lock().unwrap().clear();
        let _ = shell.handle_operation(Command::Close(middle));
        assert_eq!(front(&shell), expected, "{preference}");
        assert!(shown(&seen).is_empty(), "{preference}");
        assert!(homes(&shell).is_empty(), "{preference}");
    }
}

/// Runs `text` from Home as a map and opens one page from it, so the next
/// search does not let it go, then brings Home back to the front.
fn kept_search(shell: &mut Shell, home: ItemId, text: &str) -> zephium_core::home::Search {
    let _ = shell.handle_operation(Command::Activate(home));
    run_from_home(shell, text, "search", false);
    let search = search_of(shell, text);
    open_page(shell, search.id, "a1");
    let _ = shell.handle_operation(Command::Activate(home));
    search_of(shell, text)
}

fn results_tab(shell: &mut Shell, map: MapId, text: &str) -> ItemId {
    let _answer = read(shell, |done| HomeCommand::ReadResults {
        map,
        url: engine_url(text),
        done,
    });
    shell
        .home
        .searches
        .by_map(map)
        .and_then(|search| search.results)
        .expect("a results tab opened")
}

#[test]
fn the_thirty_third_search_lets_go_of_the_oldest_and_its_tabs() {
    let (mut shell, _engine, maps, _seen) = home_shell(Ok(MapKind::Place));
    let home = front(&shell);
    let oldest = kept_search(&mut shell, home, "query 0");
    let page = oldest.pages[0].tab;
    assert!(shell.home_tab_search(page).is_some());
    let unseen = results_tab(&mut shell, oldest.map.unwrap(), "query 0");
    for n in 1..=MAX_SEARCHES_PER_PROFILE {
        kept_search(&mut shell, home, &format!("query {n}"));
        assert_eq!(front(&shell), home);
    }
    let kept = shell.home_records().unwrap().searches;
    assert_eq!(kept.len(), MAX_SEARCHES_PER_PROFILE);
    assert!(kept.iter().all(|search| search.id != oldest.id));
    assert!(kept.iter().any(|search| search.text == "query 1"));
    assert!(shell.items.tab(page).is_some(), "its pages stay open");
    assert!(shell.home_origin(page).is_none());
    assert!(shell.home_tab_search(page).is_none());
    assert!(
        shell.items.tab(unseen).is_none(),
        "a results tab never in front closes with it"
    );
    assert_eq!(*maps.forgotten.lock().unwrap(), vec![oldest.map.unwrap()]);
}

#[test]
fn at_most_eight_results_reads_wait_at_once() {
    use super::super::home::MAX_PENDING_READS;
    let (mut shell, _engine, _maps, _seen) = home_shell(Ok(MapKind::Place));
    let home = front(&shell);
    let mut waiting = Vec::new();
    for n in 0..MAX_PENDING_READS {
        let text = format!("query {n}");
        let map = kept_search(&mut shell, home, &text).map.unwrap();
        waiting.push(read(&mut shell, |done| HomeCommand::ReadResults {
            map,
            url: engine_url(&text),
            done,
        }));
    }
    let tabs = today(&shell).len();
    let text = format!("query {MAX_PENDING_READS}");
    let map = kept_search(&mut shell, home, &text).map.unwrap();
    let tabs = tabs + 1;
    let refused = read(&mut shell, |done| HomeCommand::ReadResults {
        map,
        url: engine_url(&text),
        done,
    });
    assert_eq!(
        refused.try_recv().unwrap(),
        Err(ResultsRefusal::Unavailable)
    );
    assert_eq!(today(&shell).len(), tabs, "no tab opens for it");
    assert_eq!(search_of(&shell, &text).results, None);
    assert_eq!(shell.home.reads.len(), MAX_PENDING_READS);
    assert!(waiting.iter().all(|answer| answer.try_recv().is_err()));
}

#[test]
fn asking_the_same_search_again_reads_its_results_tab_and_opens_no_other() {
    let (mut shell, engine, _maps, _seen) = home_shell(Ok(MapKind::Place));
    run_from_home(&mut shell, "coffee near shibuya", "search", false);
    let map = only_search(&shell).map.unwrap();
    let url = engine_url("coffee near shibuya");
    let rewritten = format!("{url}&ia=web");
    let first = read(&mut shell, |done| HomeCommand::ReadResults {
        map,
        url: url.clone(),
        done,
    });
    let tab = only_search(&shell).results.unwrap();
    loaded(&mut shell, tab, &rewritten);
    answered(
        &mut shell,
        tab,
        read_id(&engine, 0),
        &rewritten,
        "<main>one</main>",
    );
    assert_eq!(
        first.try_recv().unwrap().unwrap().html.0,
        "<main>one</main>"
    );
    let tabs = today(&shell).len();
    let navigated = navigations(&engine, tab);

    // The engine rewrote the address: the same tab is read as it is.
    let again = read(&mut shell, |done| HomeCommand::ReadResults {
        map,
        url: url.clone(),
        done,
    });
    assert_eq!(only_search(&shell).results, Some(tab));
    assert_eq!(today(&shell).len(), tabs);
    assert_eq!(navigations(&engine, tab), navigated);
    assert_eq!(reads(&engine).len(), 2, "read at once");
    answered(
        &mut shell,
        tab,
        read_id(&engine, 1),
        &rewritten,
        "<main>two</main>",
    );
    assert_eq!(
        again.try_recv().unwrap().unwrap().html.0,
        "<main>two</main>"
    );

    // The person took the tab elsewhere: that tab is sent back.
    navigate_and_commit(&mut shell, tab, "example.com");
    let navigated = navigations(&engine, tab);
    let back = read(&mut shell, |done| HomeCommand::ReadResults {
        map,
        url: url.clone(),
        done,
    });
    assert_eq!(only_search(&shell).results, Some(tab));
    assert_eq!(today(&shell).len(), tabs);
    assert_eq!(navigations(&engine, tab), navigated + 1);
    assert_eq!(reads(&engine).len(), 2, "nothing is read while it loads");
    loaded(&mut shell, tab, url.as_str());
    answered(
        &mut shell,
        tab,
        read_id(&engine, 2),
        url.as_str(),
        "<main>three</main>",
    );
    assert_eq!(
        back.try_recv().unwrap().unwrap().html.0,
        "<main>three</main>"
    );

    // Only once it is closed does another tab open.
    let _ = shell.handle_operation(Command::Close(tab));
    let _fresh = read(&mut shell, |done| HomeCommand::ReadResults {
        map,
        url: url.clone(),
        done,
    });
    let fresh = only_search(&shell).results.unwrap();
    assert_ne!(fresh, tab);
    assert_eq!(today(&shell).len(), tabs);
}

#[test]
fn a_results_tab_showing_another_page_is_never_read() {
    let (mut shell, engine, _maps, _seen) = home_shell(Ok(MapKind::Place));
    let home = front(&shell);
    run_from_home(&mut shell, "coffee near shibuya", "search", false);
    let map = only_search(&shell).map.unwrap();
    let url = engine_url("coffee near shibuya");
    let elsewhere = [
        engine_url("tea near shibuya").to_string(),
        SearchEngine::Bing
            .search("coffee near shibuya")
            .unwrap()
            .to_string(),
        "https://consent.google.com/ml?continue=x".to_owned(),
    ];
    let mut tab = None;
    for (at, shown) in elsewhere.iter().enumerate() {
        let answer = read(&mut shell, |done| HomeCommand::ReadResults {
            map,
            url: url.clone(),
            done,
        });
        let results = only_search(&shell).results.unwrap();
        assert_eq!(*tab.get_or_insert(results), results, "one tab, case {at}");
        assert!(shell.discard_protected_leaves().contains(&results));
        loaded(&mut shell, results, shown);
        assert_eq!(
            answer.try_recv().unwrap(),
            Err(ResultsRefusal::Redirected),
            "case {at}"
        );
        assert!(reads(&engine).is_empty(), "never asked, case {at}");
        assert_ne!(front(&shell), results);
        assert!(!shell.discard_protected_leaves().contains(&results));
    }
    let tab = tab.unwrap();
    assert_eq!(front(&shell), home);

    // Read again on a tab that no longer shows the results: refused unread.
    let again = read(&mut shell, |done| HomeCommand::ReadAgain { map, done });
    assert_eq!(again.try_recv().unwrap(), Err(ResultsRefusal::Redirected));
    assert!(reads(&engine).is_empty());

    // A document that comes back as another search's is not this one's.
    let answer = read(&mut shell, |done| HomeCommand::ReadResults {
        map,
        url: url.clone(),
        done,
    });
    loaded(&mut shell, tab, url.as_str());
    assert_eq!(reads(&engine).len(), 1);
    answered(
        &mut shell,
        tab,
        read_id(&engine, 0),
        engine_url("tea near shibuya").as_str(),
        "<main>tea</main>",
    );
    assert_eq!(answer.try_recv().unwrap(), Err(ResultsRefusal::Redirected));
    assert!(shell.home.reads.is_empty());
}

#[test]
fn view_source_still_works_while_home_reads_a_results_page() {
    let (mut shell, engine, _maps, _seen) = home_shell(Ok(MapKind::Place));
    let home = front(&shell);
    run_from_home(&mut shell, "coffee near shibuya", "search", false);
    let map = only_search(&shell).map.unwrap();
    let url = engine_url("coffee near shibuya");
    let answer = read(&mut shell, |done| HomeCommand::ReadResults {
        map,
        url: url.clone(),
        done,
    });
    let tab = only_search(&shell).results.unwrap();
    loaded(&mut shell, tab, url.as_str());
    present_committed(&mut shell, tab, url.as_str());
    assert_eq!(reads(&engine).len(), 1);
    let extract = |tab: ItemId| {
        Command::Engine(EngineEvent::HtmlExtracted {
            id: tab,
            html: "<html>source</html>".into(),
            truncated: false,
        })
    };

    // Another tab's source, while the read waits.
    navigate_and_commit(&mut shell, home, "docs.example");
    let asked = shell.handle_operation(Command::Run("page.viewSource".into()));
    assert_eq!(asked.outcome, OperationOutcome::Deferred);
    shell.handle(extract(home));
    let source = front(&shell);
    assert_ne!(source, home);
    assert!(shell.items.tab(source).unwrap().url.is_none());
    assert!(answer.try_recv().is_err(), "the read still waits");

    // The results tab's own source, while the read waits on it.
    let _ = shell.handle_operation(Command::Activate(tab));
    let asked = shell.handle_operation(Command::Run("page.viewSource".into()));
    assert_eq!(asked.outcome, OperationOutcome::Deferred);
    shell.handle(extract(tab));
    let own = front(&shell);
    assert!(own != tab && own != source);
    assert!(
        answer.try_recv().is_err(),
        "the source answer is not the read"
    );

    answered(
        &mut shell,
        tab,
        read_id(&engine, 0),
        url.as_str(),
        "<main>results</main>",
    );
    assert_eq!(
        answer.try_recv().unwrap().unwrap().html.0,
        "<main>results</main>"
    );
    assert_eq!(front(&shell), own, "the read opens nothing");
}

fn shown_rows(seen: &Seen) -> Vec<(String, Option<u8>, Option<String>)> {
    seen.lock()
        .unwrap()
        .iter()
        .filter_map(|projection| match projection {
            Projection::HomeShow {
                search,
                slot,
                entry,
            } => Some((search.clone(), *slot, entry.clone())),
            _ => None,
        })
        .collect()
}

#[test]
fn asking_again_restarts_a_map_whose_driver_is_gone_and_reuses_one_still_held() {
    let (mut shell, _engine, maps, seen) = home_shell(Ok(MapKind::Place));
    run_from_home(&mut shell, "coffee near shibuya", "search", false);
    let search = only_search(&shell);
    let first = search.map.unwrap();

    // Held, finished or not: shown again, nothing started.
    let again = run_from_home(&mut shell, "coffee near shibuya", "search", false);
    assert_eq!(again.reason, OperationReason::HomeMapStarted);
    assert_eq!(maps.started.lock().unwrap().len(), 1);
    assert_eq!(only_search(&shell).map, Some(first));

    // The host gave up its driver: a new map for the same search.
    let waiting = read(&mut shell, |done| HomeCommand::ReadResults {
        map: first,
        url: engine_url("coffee near shibuya"),
        done,
    });
    maps.dropped.lock().unwrap().push(first);
    seen.lock().unwrap().clear();
    let restarted = run_from_home(&mut shell, "coffee near shibuya", "search", false);
    assert_eq!(restarted.reason, OperationReason::HomeMapStarted);
    let started = maps.started.lock().unwrap().clone();
    assert_eq!(started.len(), 2);
    let second = started[1].map;
    assert_ne!(second, first);
    assert_eq!(started[1].search, search.id);
    let record = only_search(&shell);
    assert_eq!(record.id, search.id);
    assert_eq!(record.map, Some(second));
    assert_eq!(record.kind, SearchKind::Place);
    assert_eq!(*maps.forgotten.lock().unwrap(), vec![first]);
    assert_eq!(waiting.try_recv().unwrap(), Err(ResultsRefusal::Unknown));
    assert_eq!(shown(&seen), vec![search.id.to_string()]);

    // The new map is held: asked again, it is shown, not started.
    run_from_home(&mut shell, "coffee near shibuya", "search", false);
    assert_eq!(maps.started.lock().unwrap().len(), 2);
}

#[test]
fn home_is_shown_at_the_row_a_closed_page_was_opened_from() {
    let (mut shell, _engine, _maps, seen) = home_shell(Ok(MapKind::Place));
    let home = front(&shell);
    run_from_home(&mut shell, "coffee near shibuya", "search", false);
    let search = only_search(&shell).id;
    let opened = shell.handle_operation(Command::Home(Box::new(HomeCommand::OpenPage {
        search,
        slot: 1,
        entry: "0123456789abcdef".into(),
        url: Some(url::Url::parse("https://cafe.example/menu").unwrap()),
    })));
    assert_eq!(opened.outcome, OperationOutcome::Applied);
    let page = front(&shell);
    seen.lock().unwrap().clear();
    let _ = shell.handle_operation(Command::Close(page));
    assert_eq!(front(&shell), home);
    assert_eq!(
        shown_rows(&seen),
        vec![(
            search.to_string(),
            Some(1),
            Some("0123456789abcdef".to_owned())
        )]
    );

    // The results page has no row.
    let _ = shell.handle_operation(Command::Home(Box::new(HomeCommand::OpenResults { search })));
    let results = front(&shell);
    assert_ne!(results, home);
    seen.lock().unwrap().clear();
    let _ = shell.handle_operation(Command::Close(results));
    assert_eq!(shown_rows(&seen), vec![(search.to_string(), None, None)]);

    // Nor has a click on the group's title.
    seen.lock().unwrap().clear();
    let _ = shell.handle_operation(Command::Home(Box::new(HomeCommand::FocusSearch { search })));
    assert_eq!(shown_rows(&seen), vec![(search.to_string(), None, None)]);

    // Nor an address.
    run_from_home(&mut shell, "docs.rs", "url", false);
    let address = search_of(&shell, "docs.rs").id;
    let docs = front(&shell);
    let _ = shell.handle_operation(Command::Run("tab.new".into()));
    let _ = shell.handle_operation(Command::Activate(docs));
    seen.lock().unwrap().clear();
    let _ = shell.handle_operation(Command::Close(docs));
    assert_eq!(shown_rows(&seen), vec![(address.to_string(), None, None)]);
}

fn projected_home(seen: &Seen) -> bool {
    seen.lock()
        .unwrap()
        .iter()
        .rev()
        .find_map(|projection| match projection {
            Projection::Items(state) => Some(state.home),
            _ => None,
        })
        .expect("the items were projected")
}

#[test]
fn the_items_say_whether_a_new_tab_here_is_home() {
    let (mut shell, _engine, _maps, seen) = home_shell(Err(MapRefusal::NotAMap));
    shell.project_items();
    assert!(projected_home(&seen));
    let set = |shell: &mut Shell, key: &str, value: &str| {
        shell.handle(Command::SetAppSetting {
            key: key.into(),
            value: value.into(),
        });
    };

    set(&mut shell, "home.mode", "classic");
    assert!(!projected_home(&seen), "classic");
    set(&mut shell, "home.mode", "home");
    assert!(projected_home(&seen));

    set(&mut shell, "tabs.new-page", "https://start.example/");
    assert!(!projected_home(&seen), "a custom new-tab page");
    shell.handle(Command::Run("window.newPrivate".into()));
    assert!(projected_home(&seen), "private keeps Home");
    let _ = shell.handle_operation(Command::Run("tab.close".into()));
    assert!(!shell.incognito_profile(shell.windows.focused().unwrap().profile));
    assert!(!projected_home(&seen), "back to the custom page");
    set(&mut shell, "tabs.new-page", "");
    assert!(projected_home(&seen));
}

#[test]
fn showing_a_search_keeps_its_place_and_asking_it_again_moves_it() {
    let (mut shell, _engine, _maps, _seen) = home_shell(Ok(MapKind::Place));
    let home = front(&shell);
    let older = kept_search(&mut shell, home, "query a");
    kept_search(&mut shell, home, "query b");
    let order = |shell: &Shell| -> Vec<String> {
        shell
            .home_records()
            .unwrap()
            .searches
            .into_iter()
            .map(|search| search.text)
            .collect()
    };
    assert_eq!(order(&shell), ["query b", "query a"]);
    let focused = shell.handle_operation(Command::Home(Box::new(HomeCommand::FocusSearch {
        search: older.id,
    })));
    assert_ne!(focused.outcome, OperationOutcome::Rejected);
    assert_eq!(order(&shell), ["query b", "query a"]);
    assert_eq!(search_of(&shell, "query a").at_ms, older.at_ms);

    let _ = shell.handle_operation(Command::Activate(home));
    run_from_home(&mut shell, "query a", "search", false);
    assert_eq!(order(&shell), ["query a", "query b"]);
}

#[test]
fn a_new_search_lets_go_of_the_ones_nothing_came_of_in_its_space() {
    let (mut shell, engine, maps, _seen) = home_shell(Ok(MapKind::Place));
    let first_space = shell.windows.focused().unwrap().space;
    let home = front(&shell);
    let opened = kept_search(&mut shell, home, "opened");

    // A results page loaded in place was in front.
    *maps.answer.lock().unwrap() = Some(Err(MapRefusal::NotAMap));
    run_from_home(&mut shell, "rust lifetimes", "search", false);
    let in_place = search_of(&shell, "rust lifetimes");
    assert_eq!(in_place.results, Some(home));
    assert!(in_place.results_seen);
    let _ = shell.handle_operation(Command::Run("tab.new".into()));
    let home = front(&shell);
    assert!(shell.is_home_tab(home));
    *maps.answer.lock().unwrap() = Some(Ok(MapKind::Place));

    // A background results tab the person looked at.
    run_from_home(&mut shell, "looked at", "search", false);
    let looked = search_of(&shell, "looked at");
    let looked_tab = results_tab(&mut shell, looked.map.unwrap(), "looked at");
    let _ = shell.handle_operation(Command::Activate(looked_tab));
    let _ = shell.handle_operation(Command::Activate(home));
    assert!(search_of(&shell, "looked at").results_seen);

    run_from_home(&mut shell, "docs.rs", "url", false);
    let _ = shell.handle_operation(Command::Run("tab.new".into()));

    // Another space keeps its own idle search.
    let _ = shell.handle_operation(Command::Space(Box::new(crate::api::SpaceCommand::Create {
        name: "Second".into(),
        look: zephium_core::spaces::SpaceLook::default(),
    })));
    run_from_home(&mut shell, "elsewhere", "search", false);
    let elsewhere = search_of(&shell, "elsewhere");
    let _ = shell.handle_operation(Command::Space(Box::new(crate::api::SpaceCommand::Switch(
        first_space,
    ))));
    let home = front(&shell);
    assert!(shell.is_home_tab(home));

    // A results tab behind Home that was never in front.
    run_from_home(&mut shell, "unseen", "search", false);
    let unseen = search_of(&shell, "unseen");
    let unseen_tab = results_tab(&mut shell, unseen.map.unwrap(), "unseen");
    assert_eq!(front(&shell), home);
    let forgotten = maps.forgotten.lock().unwrap().len();

    run_from_home(&mut shell, "next", "search", false);
    let texts: Vec<String> = shell
        .home_records()
        .unwrap()
        .searches
        .into_iter()
        .map(|search| search.text)
        .collect();
    assert_eq!(
        texts,
        ["next", "docs.rs", "looked at", "rust lifetimes", "opened"]
    );
    assert!(shell.items.tab(unseen_tab).is_none(), "its tab closed");
    assert!(shell.items.tab(looked_tab).is_some());
    assert!(shell.items.tab(opened.pages[0].tab).is_some());
    let gone = maps.forgotten.lock().unwrap()[forgotten..].to_vec();
    assert_eq!(gone, vec![unseen.map.unwrap()]);
    assert!(shell.home.searches.get(elsewhere.id).is_some());
    assert!(!engine.calls().iter().any(|call| call.starts_with("read ")));

    // A search with no results tab at all leaves too.
    run_from_home(&mut shell, "after", "search", false);
    assert!(shell
        .home
        .searches
        .get(search_of(&shell, "after").id)
        .is_some());
    assert!(shell
        .home_records()
        .unwrap()
        .searches
        .iter()
        .all(|search| search.text != "next"));
}

#[test]
fn forgetting_a_search_closes_its_results_tab_only_if_never_in_front() {
    let (mut shell, _engine, _maps, _seen) = home_shell(Ok(MapKind::Place));
    let home = front(&shell);
    let unseen = kept_search(&mut shell, home, "unseen");
    let map = unseen.map.unwrap();
    let answer = read(&mut shell, |done| HomeCommand::ReadResults {
        map,
        url: engine_url("unseen"),
        done,
    });
    let unseen_tab = search_of(&shell, "unseen").results.unwrap();
    let forgot = shell.handle_operation(Command::Home(Box::new(HomeCommand::ForgetSearch {
        search: unseen.id,
    })));
    assert_eq!(forgot.outcome, OperationOutcome::Applied);
    assert!(shell.items.tab(unseen_tab).is_none());
    assert_eq!(
        answer.try_recv().unwrap(),
        Err(ResultsRefusal::Unavailable),
        "settled as for a removed tab"
    );
    assert!(shell.home.reads.is_empty());
    assert!(shell.items.tab(unseen.pages[0].tab).is_some(), "pages stay");
    assert_eq!(front(&shell), home);

    let looked = kept_search(&mut shell, home, "looked");
    let looked_tab = results_tab(&mut shell, looked.map.unwrap(), "looked");
    let _ = shell.handle_operation(Command::Activate(looked_tab));
    let _ = shell.handle_operation(Command::Activate(home));
    let _ = shell.handle_operation(Command::Home(Box::new(HomeCommand::ForgetSearch {
        search: looked.id,
    })));
    assert!(shell.items.tab(looked_tab).is_some(), "a seen one stays");
    assert!(shell.home_origin(looked_tab).is_none());

    // One in front now stays too, whatever its flag says.
    let front_now = kept_search(&mut shell, home, "in front");
    let tab = results_tab(&mut shell, front_now.map.unwrap(), "in front");
    shell
        .home
        .searches
        .get_mut(front_now.id)
        .unwrap()
        .results_seen = false;
    let window = shell.windows.focused_mut().unwrap();
    window.active = Some(tab);
    let _ = shell.handle_operation(Command::Home(Box::new(HomeCommand::ForgetSearch {
        search: front_now.id,
    })));
    assert!(shell.items.tab(tab).is_some());
}

fn untouched(shell: &Shell) -> Vec<ItemId> {
    let space = shell.windows.focused().unwrap().space;
    shell
        .regular_tabs(space)
        .into_iter()
        .filter(|id| shell.is_untouched_new_tab(*id))
        .collect()
}

#[test]
fn one_home_is_left_per_space_when_home_turns_on_or_after_restore() {
    let store = Arc::new(FakeStore::default());
    store.settings.lock().unwrap().extend([
        ("tabs.startup".to_owned(), "continue".to_owned()),
        ("home.mode".to_owned(), "classic".to_owned()),
    ]);
    let attach = |shell: &mut Shell| {
        shell.handle(Command::Home(Box::new(HomeCommand::Attach(
            HomeMapsAttachment(Arc::new(FakeMaps::default())),
        ))));
    };
    let (mut classic, _engine, _screen) = setup_with(store.clone());
    classic.handle(Command::Bootstrap);
    attach(&mut classic);
    classic.handle(Command::Open);
    classic.handle(Command::Open);
    let tabs = untouched(&classic);
    assert_eq!(tabs.len(), 3, "classic opens a blank tab each time");
    let _ = classic.handle_operation(Command::Activate(tabs[1]));
    attach(&mut classic);
    assert_eq!(untouched(&classic), tabs, "classic keeps them all");
    assert!(tabs.iter().all(|id| !classic.is_home_tab(*id)));

    // Turning Home on keeps the one in front.
    classic.handle(Command::SetAppSetting {
        key: "home.mode".into(),
        value: "home".into(),
    });
    assert_eq!(untouched(&classic), vec![tabs[1]]);
    assert_eq!(homes(&classic), vec![tabs[1]]);
    assert_eq!(front(&classic), tabs[1]);
    drop(classic);

    // A blank tab is never saved, so a restore brings back one Home.
    let (mut restored, _engine, _screen) = setup_with(store.clone());
    restored.handle(Command::Bootstrap);
    attach(&mut restored);
    assert_eq!(homes(&restored).len(), 1);
    assert_eq!(untouched(&restored).len(), 1);
    drop(restored);

    // Home mode before anything hosts it: blank tabs gather until Home is
    // attached, then the first is kept when none of them is in front.
    *store.saved.lock().unwrap() = None;
    let (mut early, _engine, _screen) = setup_with(store);
    early.handle(Command::Bootstrap);
    early.handle(Command::Open);
    early.handle(Command::Open);
    let tabs = untouched(&early);
    assert_eq!(tabs.len(), 3);
    navigate_and_commit(&mut early, tabs[0], "example.com");
    let blank = untouched(&early);
    assert_eq!(blank, tabs[1..]);
    let _ = early.handle_operation(Command::Activate(tabs[0]));
    attach(&mut early);
    assert_eq!(homes(&early), vec![blank[0]]);
    assert_eq!(untouched(&early), vec![blank[0]]);
    assert_eq!(front(&early), tabs[0]);
}
