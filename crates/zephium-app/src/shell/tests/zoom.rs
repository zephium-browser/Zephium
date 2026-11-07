use super::*;

use zephium_core::zoom::SavedLevel;
use zephium_ipc::{HistoryCall, HistoryRange, ZoomCall, ZoomError, ZoomLevelView, ZoomResponse};

fn requests_for(engine: &FakeEngine, id: ItemId) -> Vec<f64> {
    engine
        .zoom_requests
        .lock()
        .unwrap()
        .iter()
        .filter(|(item, ..)| *item == id)
        .map(|(_, scale, _)| *scale)
        .collect()
}

fn settle_last(shell: &mut Shell, engine: &FakeEngine, id: ItemId) {
    let (item, scale, request) = engine.last_zoom_request();
    assert_eq!(item, id);
    shell.handle(Command::Engine(EngineEvent::ZoomSettled {
        id,
        request,
        applied_scale: scale,
        succeeded: true,
    }));
}

/// Zooms the active tab in once and lets the engine apply it.
fn zoom_in_and_settle(shell: &mut Shell, engine: &FakeEngine, id: ItemId) {
    shell.handle(Command::Run("zoom.in".into()));
    settle_last(shell, engine, id);
}

fn saved(store: &FakeStore, profile: ProfileId) -> Vec<(String, u16)> {
    store
        .zoom_saved
        .lock()
        .unwrap()
        .get(&profile)
        .map(|levels| {
            levels
                .iter()
                .map(|level| (level.site.clone(), level.percent))
                .collect()
        })
        .unwrap_or_default()
}

fn open_and_browse(shell: &mut Shell, screen: &Screen, input: &str) -> ItemId {
    shell.handle(Command::Open);
    let id = active_id(screen);
    navigate_and_commit(shell, id, input);
    id
}

#[test]
fn zoom_stays_out_of_authoritative_state_until_exact_native_settlement() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, engine, screen) = setup_with(store.clone());
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    navigate_and_commit(&mut shell, id, "zoom-state.example");
    assert_eq!(shell.items.tab(id).unwrap().zoom, 1.0);
    assert_eq!(persisted_zoom(&store, id), 1.0);

    shell.handle(Command::Run("zoom.in".into()));
    let (_, first_scale, first) = engine.last_zoom_request();
    assert_eq!(first_scale, 1.1);
    assert_eq!(shell.items.tab(id).unwrap().zoom, 1.0);
    assert_eq!(persisted_zoom(&store, id), 1.0);

    // Rapid input is based on the pending desired value without making
    // that value part of a full-session snapshot.
    shell.handle(Command::Run("zoom.in".into()));
    let (_, second_scale, second) = engine.last_zoom_request();
    assert_eq!(second_scale, 1.25);
    assert_ne!(first, second);
    assert_eq!(shell.items.tab(id).unwrap().zoom, 1.0);

    shell.handle(Command::Engine(EngineEvent::ZoomSettled {
        id,
        request: first,
        applied_scale: 1.1,
        succeeded: true,
    }));
    assert_eq!(shell.items.tab(id).unwrap().zoom, 1.0);
    assert_eq!(persisted_zoom(&store, id), 1.0);
    let profile = shell.profile_of_item(id).unwrap();
    assert!(saved(&store, profile).is_empty());

    shell.handle(Command::Engine(EngineEvent::ZoomSettled {
        id,
        request: second,
        applied_scale: 1.25,
        succeeded: true,
    }));
    assert_eq!(shell.items.tab(id).unwrap().zoom, 1.25);
    assert_eq!(persisted_zoom(&store, id), 1.25);
    assert_eq!(
        saved(&store, profile),
        [("zoom-state.example".to_owned(), 125)]
    );
    assert!(!shell.zoom.pending.contains_key(&id));
}

#[test]
fn newest_zoom_failure_reports_the_cumulative_native_scale_after_coalescing() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, engine, screen) = setup_with(store.clone());
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    navigate_and_commit(&mut shell, id, "zoom-coalesce.example");

    shell.handle(Command::Run("zoom.in".into()));
    let (_, _, first) = engine.last_zoom_request();
    shell.handle(Command::Run("zoom.in".into()));
    let (_, _, second) = engine.last_zoom_request();
    assert_ne!(first, second);

    // The first native call succeeded, but its event was coalesced before
    // the actor observed it. The latest failure still carries 1.1 as the
    // last actually applied native scale, so both model and disk converge.
    shell.handle(Command::Engine(EngineEvent::ZoomSettled {
        id,
        request: second,
        applied_scale: 1.1,
        succeeded: false,
    }));
    assert_eq!(shell.items.tab(id).unwrap().zoom, 1.1);
    assert_eq!(persisted_zoom(&store, id), 1.1);
    assert!(!shell.zoom.pending.contains_key(&id));
    // A request that was not applied is not what the site remembers, and is
    // not asked for again.
    let profile = shell.profile_of_item(id).unwrap();
    assert!(saved(&store, profile).is_empty());
    assert_eq!(requests_for(&engine, id).len(), 2);
}

#[test]
fn exact_malformed_zoom_settlement_retires_only_its_pending_obligation() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, engine, screen) = setup_with(store.clone());
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    navigate_and_commit(&mut shell, id, "zoom-malformed.example");

    shell.handle(Command::Run("zoom.in".into()));
    let (_, _, request) = engine.last_zoom_request();
    assert!(shell.zoom.pending.contains_key(&id));

    shell.handle(Command::Engine(EngineEvent::ZoomSettled {
        id,
        request,
        applied_scale: f64::NAN,
        succeeded: true,
    }));

    assert!(!shell.zoom.pending.contains_key(&id));
    assert_eq!(shell.items.tab(id).unwrap().zoom, 1.0);
    assert_eq!(persisted_zoom(&store, id), 1.0);
}

#[test]
fn recreated_view_starts_at_its_sites_zoom_and_a_failed_request_rolls_back() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, engine, screen) = setup_with(store.clone());
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    navigate_and_commit(&mut shell, id, "zoom-restore.example");
    zoom_in_and_settle(&mut shell, &engine, id);
    zoom_in_and_settle(&mut shell, &engine, id);
    assert_eq!(shell.items.tab(id).unwrap().zoom, 1.25);

    // A view comes back at 1.0 and is brought to what its site remembers,
    // whatever scale its tab last had.
    shell.items.view_creation_failed(id);
    let effects = shell.items.ensure_view(id);
    shell.apply(effects);
    assert_eq!(shell.items.tab(id).unwrap().zoom, 1.0);
    let (_, requested, request) = engine.last_zoom_request();
    assert_eq!(requested, 1.25);
    shell.handle(Command::Engine(EngineEvent::ZoomSettled {
        id,
        request,
        applied_scale: 1.0,
        succeeded: false,
    }));
    assert_eq!(shell.items.tab(id).unwrap().zoom, 1.0);
    assert_eq!(persisted_zoom(&store, id), 1.0);
    // The site still remembers what the person chose.
    let profile = shell.profile_of_item(id).unwrap();
    assert_eq!(
        saved(&store, profile),
        [("zoom-restore.example".to_owned(), 125)]
    );

    shell.handle(Command::Run("zoom.in".into()));
    assert!(shell.zoom.pending.contains_key(&id));
    shell.handle(Command::Engine(EngineEvent::ViewCreationFailed { id }));
    assert!(!shell.zoom.pending.contains_key(&id));
}

#[test]
fn a_site_zoom_reaches_its_open_tabs_and_new_ones_but_no_other_site() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, engine, screen) = setup_with(store.clone());
    shell.handle(Command::Bootstrap);
    let first = active_id(&screen);
    navigate_and_commit(&mut shell, first, "github.com/a");
    let sibling = open_and_browse(&mut shell, &screen, "gist.github.com");
    let other = open_and_browse(&mut shell, &screen, "example.org");
    shell.handle(Command::Activate(first));
    let before = engine.zoom_requests.lock().unwrap().len();

    zoom_in_and_settle(&mut shell, &engine, first);

    // The sibling on the same registrable domain is asked for the new level,
    // the other site is left alone.
    assert_eq!(requests_for(&engine, sibling), [1.1]);
    assert!(requests_for(&engine, other).is_empty());
    assert_eq!(engine.zoom_requests.lock().unwrap().len(), before + 2);
    settle_last(&mut shell, &engine, sibling);
    assert_eq!(shell.items.tab(sibling).unwrap().zoom, 1.1);
    assert_eq!(shell.items.tab(other).unwrap().zoom, 1.0);
    let profile = shell.profile_of_item(first).unwrap();
    assert_eq!(saved(&store, profile), [("github.com".to_owned(), 110)]);

    // A new tab, and a navigation within the site, arrive at it.
    let opened = open_and_browse(&mut shell, &screen, "github.com/b");
    assert_eq!(requests_for(&engine, opened), [1.1]);
    let count = engine.zoom_requests.lock().unwrap().len();
    commit_url(&mut shell, opened, "https://github.com/c");
    assert_eq!(engine.zoom_requests.lock().unwrap().len(), count);
}

#[test]
fn moving_to_a_site_without_a_level_returns_to_the_default() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, engine, screen) = setup_with(store.clone());
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    navigate_and_commit(&mut shell, id, "github.com");
    zoom_in_and_settle(&mut shell, &engine, id);
    assert_eq!(shell.items.tab(id).unwrap().zoom, 1.1);

    commit_url(&mut shell, id, "https://example.org/");
    let (_, requested, _) = engine.last_zoom_request();
    assert_eq!(requested, 1.0);
    settle_last(&mut shell, &engine, id);
    assert_eq!(shell.items.tab(id).unwrap().zoom, 1.0);

    // Back on the first site, its level returns without a keystroke.
    commit_url(&mut shell, id, "https://github.com/x");
    let (_, requested, _) = engine.last_zoom_request();
    assert_eq!(requested, 1.1);
}

#[test]
fn a_site_without_a_level_follows_the_default() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    navigate_and_commit(&mut shell, id, "default.example");
    zoom_in_and_settle(&mut shell, &engine, id);
    let other = open_and_browse(&mut shell, &screen, "elsewhere.example");

    shell.zoom.default_percent = 125;
    shell.reconcile_all_zoom();

    // Only the site with no level of its own changes.
    assert_eq!(requests_for(&engine, other), [1.25]);
    assert_eq!(requests_for(&engine, id), [1.1]);

    // Reset goes to the default and the site forgets its level.
    shell.handle(Command::Activate(id));
    shell.handle(Command::Run("zoom.reset".into()));
    let (_, requested, _) = engine.last_zoom_request();
    assert_eq!(requested, 1.25);
    settle_last(&mut shell, &engine, id);
    let profile = shell.profile_of_item(id).unwrap();
    assert!(shell.zoom.saved[&profile].is_empty());
}

#[test]
fn zooming_back_to_the_default_forgets_the_level() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, engine, screen) = setup_with(store.clone());
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    navigate_and_commit(&mut shell, id, "github.com");
    let profile = shell.profile_of_item(id).unwrap();
    zoom_in_and_settle(&mut shell, &engine, id);
    assert_eq!(saved(&store, profile), [("github.com".to_owned(), 110)]);

    shell.handle(Command::Run("zoom.out".into()));
    settle_last(&mut shell, &engine, id);
    assert!(saved(&store, profile).is_empty());
}

#[test]
fn a_persons_zoom_in_flight_is_not_fought_by_its_site() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let first = active_id(&screen);
    navigate_and_commit(&mut shell, first, "github.com/a");
    let second = open_and_browse(&mut shell, &screen, "github.com/b");

    zoom_in_and_settle(&mut shell, &engine, second);
    // The first tab was told to follow; before that settles, the person zooms it.
    shell.handle(Command::Activate(first));
    shell.handle(Command::Run("zoom.in".into()));
    let (_, wanted, person) = engine.last_zoom_request();
    assert_eq!(wanted, 1.25);

    // The site changing underneath does not replace it.
    let count = engine.zoom_requests.lock().unwrap().len();
    shell.reconcile_zoom(first);
    assert_eq!(engine.zoom_requests.lock().unwrap().len(), count);

    shell.handle(Command::Engine(EngineEvent::ZoomSettled {
        id: first,
        request: person,
        applied_scale: 1.25,
        succeeded: true,
    }));
    // Once it settles the site follows it, and the other tab follows the site.
    assert_eq!(requests_for(&engine, second).last(), Some(&1.25));
}

#[test]
fn saved_levels_load_once_per_profile_and_apply_to_restored_tabs() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, engine, screen) = setup_with(store.clone());
    shell.handle(Command::Bootstrap);
    let first = active_id(&screen);
    let profile = shell.profile_of_item(first).unwrap();
    store.zoom_saved.lock().unwrap().insert(
        profile,
        vec![SavedLevel {
            site: "github.com".into(),
            percent: 80,
            set_at: 5,
        }],
    );

    navigate_and_commit(&mut shell, first, "github.com");
    let second = open_and_browse(&mut shell, &screen, "gist.github.com");

    assert_eq!(
        requests_for(&engine, first).first().copied(),
        Some(0.8),
        "the first tab opens at the saved level"
    );
    assert_eq!(requests_for(&engine, second), [0.8]);
    assert_eq!(
        store.zoom_loads.load(std::sync::atomic::Ordering::Relaxed),
        1
    );
}

#[test]
fn unusable_saved_levels_are_ignored() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, engine, screen) = setup_with(store.clone());
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    let profile = shell.profile_of_item(id).unwrap();
    store.zoom_saved.lock().unwrap().insert(
        profile,
        vec![SavedLevel {
            site: "github.com".into(),
            percent: 9_000,
            set_at: 5,
        }],
    );
    navigate_and_commit(&mut shell, id, "github.com");
    assert!(requests_for(&engine, id).is_empty());
}

#[test]
fn private_levels_stay_in_memory_and_go_with_the_session() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, engine, screen) = setup_with(store.clone());
    shell.handle(Command::Bootstrap);
    let regular = active_id(&screen);
    navigate_and_commit(&mut shell, regular, "github.com");
    let regular_profile = shell.profile_of_item(regular).unwrap();
    let loads = || store.zoom_loads.load(std::sync::atomic::Ordering::Relaxed);
    assert_eq!(loads(), 1);

    shell.handle(Command::Run("window.newPrivate".into()));
    let private = active_id(&screen);
    let private_profile = shell.profile_of_item(private).unwrap();
    assert_ne!(private_profile, regular_profile);
    navigate_and_commit(&mut shell, private, "github.com");
    zoom_in_and_settle(&mut shell, &engine, private);
    assert_eq!(shell.items.tab(private).unwrap().zoom, 1.1);

    // Nothing is written or read for the private profile, and the regular
    // profile's site is untouched.
    assert!(saved(&store, private_profile).is_empty());
    assert!(saved(&store, regular_profile).is_empty());
    assert_eq!(loads(), 1);
    assert!(requests_for(&engine, regular).is_empty());
    let other_private = open_and_browse(&mut shell, &screen, "gist.github.com");
    assert_eq!(requests_for(&engine, other_private), [1.1]);

    // The next private session starts from nothing.
    shell.handle(Command::Run("window.closePrivate".into()));
    assert!(shell.private.is_none());
    assert!(!shell.zoom.saved.contains_key(&private_profile));
    shell.handle(Command::Run("window.newPrivate".into()));
    let fresh = active_id(&screen);
    navigate_and_commit(&mut shell, fresh, "github.com");
    assert!(requests_for(&engine, fresh).is_empty());
}

#[test]
fn clearing_history_forgets_the_levels_it_covers_and_returns_tabs_to_the_default() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, engine, screen) = setup_with(store.clone());
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    navigate_and_commit(&mut shell, id, "github.com");
    let profile = shell.profile_of_item(id).unwrap();
    zoom_in_and_settle(&mut shell, &engine, id);
    assert_eq!(shell.zoom.saved[&profile].len(), 1);

    // A range that ended before the level was set leaves it.
    shell.clear_zoom_levels(profile, Some(i64::MAX));
    assert_eq!(shell.zoom.saved[&profile].len(), 1);

    shell.handle(Command::HistoryCall {
        expected_profile: profile,
        call: Box::new(HistoryCall::Clear {
            range: HistoryRange::Everything,
        }),
        done: crate::api::HistoryCompletion::new(|_| {}),
    });
    assert!(shell.zoom.saved[&profile].is_empty());
    let (_, requested, _) = engine.last_zoom_request();
    assert_eq!(requested, 1.0);
}

#[test]
fn forgetting_a_level_returns_its_tabs_to_the_default() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, engine, screen) = setup_with(store.clone());
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    navigate_and_commit(&mut shell, id, "github.com");
    let profile = shell.profile_of_item(id).unwrap();
    zoom_in_and_settle(&mut shell, &engine, id);

    assert!(shell.forget_zoom_level(profile, "github.com"));
    assert!(!shell.forget_zoom_level(profile, "github.com"));
    assert!(saved(&store, profile).is_empty());
    let (_, requested, _) = engine.last_zoom_request();
    assert_eq!(requested, 1.0);
}

fn zoom_call(shell: &mut Shell, profile: ProfileId, call: ZoomCall) -> ZoomResponse {
    let slot = Arc::new(Mutex::new(None));
    let sink = slot.clone();
    shell.handle(Command::ZoomCall {
        expected_profile: profile,
        call: Box::new(call),
        done: crate::api::ZoomCompletion::new(move |response| {
            *sink.lock().unwrap() = Some(response);
        }),
    });
    let answered = slot.lock().unwrap().take();
    answered.expect("every zoom call is answered")
}

fn listed(response: ZoomResponse) -> Vec<(String, u16)> {
    let ZoomResponse::Levels { levels } = response else {
        panic!("the call lists the levels after it")
    };
    levels
        .into_iter()
        .map(|ZoomLevelView { site, percent }| (site, percent))
        .collect()
}

#[test]
fn the_default_zoom_is_read_at_startup_and_changed_by_the_setting() {
    let store = Arc::new(FakeStore::default());
    store
        .settings
        .lock()
        .unwrap()
        .insert("zoom.default".into(), "90".into());
    let (mut shell, engine, screen) = setup_with(store.clone());
    shell.handle(Command::Bootstrap);
    assert_eq!(shell.zoom.default_percent, 90);
    let id = active_id(&screen);
    navigate_and_commit(&mut shell, id, "github.com");
    assert_eq!(requests_for(&engine, id), [0.9]);
    settle_last(&mut shell, &engine, id);

    shell.operation_set_app_setting("zoom.default".into(), "150".into());
    assert_eq!(shell.zoom.default_percent, 150);
    assert_eq!(requests_for(&engine, id), [0.9, 1.5]);

    // A value the setting does not allow changes nothing.
    let rejected = shell.operation_set_app_setting("zoom.default".into(), "133".into());
    assert_eq!(rejected.outcome, OperationOutcome::Rejected);
    assert_eq!(shell.zoom.default_percent, 150);
}

#[test]
fn a_stored_default_that_is_not_offered_is_ignored() {
    let store = Arc::new(FakeStore::default());
    store
        .settings
        .lock()
        .unwrap()
        .insert("zoom.default".into(), "250".into());
    let (shell, _engine, _screen) = setup_with(store);
    assert_eq!(shell.zoom.default_percent, 100);
}

#[test]
fn settings_list_forget_and_reset_the_focused_profiles_levels() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, engine, screen) = setup_with(store.clone());
    shell.handle(Command::Bootstrap);
    let first = active_id(&screen);
    let profile = shell.profile_of_item(first).unwrap();
    navigate_and_commit(&mut shell, first, "nytimes.com");
    zoom_in_and_settle(&mut shell, &engine, first);
    let second = open_and_browse(&mut shell, &screen, "github.com");
    zoom_in_and_settle(&mut shell, &engine, second);
    zoom_in_and_settle(&mut shell, &engine, second);

    // By site name, whatever the order they were set in.
    assert_eq!(
        listed(zoom_call(&mut shell, profile, ZoomCall::Levels)),
        [
            ("github.com".to_owned(), 125),
            ("nytimes.com".to_owned(), 110)
        ]
    );

    let after = zoom_call(
        &mut shell,
        profile,
        ZoomCall::Forget {
            site: "github.com".into(),
        },
    );
    assert_eq!(listed(after), [("nytimes.com".to_owned(), 110)]);
    assert_eq!(saved(&store, profile), [("nytimes.com".to_owned(), 110)]);
    // Its open tab shows the default again.
    let (item, requested, _) = engine.last_zoom_request();
    assert_eq!((item, requested), (second, 1.0));

    assert!(listed(zoom_call(&mut shell, profile, ZoomCall::ForgetAll)).is_empty());
    assert!(saved(&store, profile).is_empty());
    let (item, requested, _) = engine.last_zoom_request();
    assert_eq!((item, requested), (first, 1.0));
}

#[test]
fn a_zoom_call_for_another_profile_or_an_unusable_site_is_refused() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, engine, screen) = setup_with(store.clone());
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    let profile = shell.profile_of_item(id).unwrap();
    navigate_and_commit(&mut shell, id, "github.com");
    zoom_in_and_settle(&mut shell, &engine, id);

    for call in [
        ZoomCall::Levels,
        ZoomCall::ForgetAll,
        ZoomCall::Forget {
            site: "github.com".into(),
        },
    ] {
        assert_eq!(
            zoom_call(&mut shell, ProfileId::from(9_001), call),
            ZoomResponse::Error {
                error: ZoomError::Invalid
            }
        );
    }
    assert_eq!(
        zoom_call(&mut shell, profile, ZoomCall::Forget { site: "".into() }),
        ZoomResponse::Error {
            error: ZoomError::Invalid
        }
    );
    assert_eq!(saved(&store, profile), [("github.com".to_owned(), 110)]);
}

fn shown_zoom(screen: &Screen, id: ItemId) -> Option<u16> {
    last(screen)
        .tabs
        .iter()
        .find(|tab| tab.id == id.to_string())
        .and_then(|tab| tab.zoom)
}

#[test]
fn chrome_is_told_a_pages_zoom_when_it_is_asked_for_and_when_it_settles() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    // A New Tab has no page to zoom.
    assert_eq!(shown_zoom(&screen, id), None);
    navigate_and_commit(&mut shell, id, "github.com");
    assert_eq!(shown_zoom(&screen, id), Some(100));

    // The level asked for shows at once, so the badge answers the key.
    shell.handle(Command::Run("zoom.in".into()));
    assert_eq!(shown_zoom(&screen, id), Some(110));
    let (_, _, request) = engine.last_zoom_request();

    // A view that could not apply it shows what it is at.
    shell.handle(Command::Engine(EngineEvent::ZoomSettled {
        id,
        request,
        applied_scale: 1.0,
        succeeded: false,
    }));
    assert_eq!(shown_zoom(&screen, id), Some(100));

    zoom_in_and_settle(&mut shell, &engine, id);
    assert_eq!(shown_zoom(&screen, id), Some(110));
}

#[test]
fn a_tab_follows_its_site_in_what_chrome_is_told() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let first = active_id(&screen);
    navigate_and_commit(&mut shell, first, "github.com");
    let second = open_and_browse(&mut shell, &screen, "gist.github.com");
    shell.handle(Command::Activate(first));
    zoom_in_and_settle(&mut shell, &engine, first);
    assert_eq!(shown_zoom(&screen, second), Some(110));
    settle_last(&mut shell, &engine, second);
    assert_eq!(shown_zoom(&screen, second), Some(110));
}

#[test]
fn a_sleeping_tab_carries_no_zoom() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    navigate_and_commit(&mut shell, id, "github.com");
    zoom_in_and_settle(&mut shell, &engine, id);
    shell.items.view_creation_failed(id);
    shell.project_tab(id);
    assert_eq!(shown_zoom(&screen, id), None);
}

fn gesture(shell: &mut Shell, id: ItemId, scale: f64) {
    shell.handle(Command::Engine(EngineEvent::ZoomGesture { id, scale }));
}

#[test]
fn a_zoom_made_in_the_page_is_remembered_like_a_command() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, engine, screen) = setup_with(store.clone());
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    navigate_and_commit(&mut shell, id, "github.com");
    let sibling = open_and_browse(&mut shell, &screen, "gist.github.com");

    gesture(&mut shell, id, 0.6666666666666666);

    let profile = shell.profile_of_item(id).unwrap();
    assert_eq!(shell.items.tab(id).unwrap().zoom, 0.6666666666666666);
    assert_eq!(persisted_zoom(&store, id), 0.6666666666666666);
    assert_eq!(saved(&store, profile), [("github.com".to_owned(), 67)]);
    assert_eq!(shown_zoom(&screen, id), Some(67));
    assert_eq!(requests_for(&engine, sibling), [0.67]);
    // The tab that was zoomed already shows it; it is not asked again.
    assert!(requests_for(&engine, id).is_empty());
}

#[test]
fn a_zoom_made_in_the_page_replaces_a_request_in_flight() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    navigate_and_commit(&mut shell, id, "github.com");
    shell.handle(Command::Run("zoom.in".into()));
    let (_, _, request) = engine.last_zoom_request();

    gesture(&mut shell, id, 1.5);
    assert!(!shell.zoom.pending.contains_key(&id));

    // The settlement of what was asked before is stale and changes nothing.
    shell.handle(Command::Engine(EngineEvent::ZoomSettled {
        id,
        request,
        applied_scale: 1.1,
        succeeded: true,
    }));
    assert_eq!(shell.items.tab(id).unwrap().zoom, 1.5);
    let profile = shell.profile_of_item(id).unwrap();
    assert_eq!(shell.zoom.saved[&profile]["github.com"].percent, 150);
}

#[test]
fn a_zoom_made_in_the_page_back_to_the_default_forgets_the_level() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, engine, screen) = setup_with(store.clone());
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    navigate_and_commit(&mut shell, id, "github.com");
    let profile = shell.profile_of_item(id).unwrap();
    zoom_in_and_settle(&mut shell, &engine, id);
    assert_eq!(saved(&store, profile).len(), 1);

    gesture(&mut shell, id, 1.0);
    assert!(saved(&store, profile).is_empty());
}

#[test]
fn a_page_zoom_for_a_tab_with_no_view_or_a_malformed_scale_changes_nothing() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, _engine, screen) = setup_with(store.clone());
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    navigate_and_commit(&mut shell, id, "github.com");
    let profile = shell.profile_of_item(id).unwrap();

    for scale in [f64::NAN, f64::INFINITY, 0.0, 0.25, 4.0] {
        gesture(&mut shell, id, scale);
    }
    shell.items.view_creation_failed(id);
    gesture(&mut shell, id, 1.5);
    gesture(&mut shell, ItemId::from(99_999), 1.5);

    assert_eq!(shell.items.tab(id).unwrap().zoom, 1.0);
    assert!(saved(&store, profile).is_empty());
}
