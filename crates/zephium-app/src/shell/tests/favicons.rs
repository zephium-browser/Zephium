use super::*;

use crate::shell::favicons::DEFERRED_PAGE_CHECK;

#[test]
fn favicon_pipeline_accepts_only_fixed_renderer_rasters_for_current_origin() {
    let engine = Arc::new(FakeEngine::default());
    let store = Arc::new(FakeStore::default());
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
    let sink = screen.clone();
    let mut shell = Shell::new(
        engine.clone(),
        store.clone(),
        Arc::new(FakeChrome),
        Box::new(move |p| apply_projection(&mut sink.lock().unwrap(), p)),
    );
    shell.handle(Command::SetWindowSize(Size::new(1200.0, 800.0)));
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    shell.handle(Command::Navigate {
        id,
        input: "example.com".into(),
    });

    shell.handle(Command::Engine(EngineEvent::UrlChanged {
        id,
        url: "https://example.com/".into(),
    }));
    present_committed(&mut shell, id, "https://example.com/");
    assert!(engine
        .calls()
        .iter()
        .any(|call| call == &format!("discover {id}")));

    shell.handle(Command::Engine(EngineEvent::FaviconPixels {
        id,
        page_url: "https://attacker.example/".into(),
        rgba: vec![1; zephium_core::icon::RGBA32_BYTES],
    }));
    assert!(store.icons.lock().unwrap().is_empty());

    shell.handle(Command::Engine(EngineEvent::FaviconPixels {
        id,
        page_url: "https://example.com/".into(),
        rgba: vec![1; zephium_core::icon::RGBA32_BYTES - 1],
    }));
    assert!(store.icons.lock().unwrap().is_empty());

    let rgba = vec![7; zephium_core::icon::RGBA32_BYTES];
    shell.handle(Command::Engine(EngineEvent::FaviconPixels {
        id,
        page_url: "https://example.com/".into(),
        rgba: rgba.clone(),
    }));
    assert_eq!(
        store.icons.lock().unwrap().as_slice(),
        &[(String::from("https://example.com"), rgba)]
    );
    let tab = last(&screen).tabs.into_iter().next().unwrap();
    assert_eq!(
        tab.icon.as_ref().map(|icon| icon.origin.as_str()),
        Some("https://example.com")
    );
    let profile = shell.windows.focused().unwrap().profile;
    assert!(shell
        .icon_ref_for_url(
            zephium_ipc::IconSurface::Chrome,
            profile,
            "https://example.com"
        )
        .is_some());
}

#[test]
fn stale_favicon_store_reply_cannot_cross_a_navigation_generation() {
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    shell.store_reads = Some(StoreReadQueue::new());
    let id = active_id(&screen);
    let profile = shell.windows.focused().unwrap().profile;

    shell.handle(Command::Engine(EngineEvent::UrlChanged {
        id,
        url: "https://first.example/".into(),
    }));
    let first_generation = shell.favicons.store_reads.get(&id).unwrap().generation;
    shell.handle(Command::Engine(EngineEvent::UrlChanged {
        id,
        url: "https://second.example/".into(),
    }));
    let second_generation = shell.favicons.store_reads.get(&id).unwrap().generation;
    let rgba = vec![91; zephium_core::icon::RGBA32_BYTES];

    shell.handle(Command::StoreRead(StoreReadResult::Favicon {
        generation: first_generation,
        id,
        profile,
        origin: "https://first.example".into(),
        icon: crate::store_reads::StoredIcon {
            rgba: Some(rgba.clone()),
            stale: false,
        },
        page: None,
    }));
    assert!(shell
        .icon_ref_for_url(
            zephium_ipc::IconSurface::Chrome,
            profile,
            "https://first.example/"
        )
        .is_none());
    assert_eq!(
        shell
            .favicons
            .store_reads
            .get(&id)
            .map(|pending| pending.generation),
        Some(second_generation)
    );

    shell.handle(Command::StoreRead(StoreReadResult::Favicon {
        generation: second_generation,
        id,
        profile,
        origin: "https://second.example".into(),
        icon: crate::store_reads::StoredIcon {
            rgba: Some(rgba),
            stale: false,
        },
        page: Some(Box::new(crate::store_reads::PageFavicon {
            key: "https://second.example/".into(),
            icon: crate::store_reads::StoredIcon {
                rgba: None,
                stale: true,
            },
        })),
    }));
    assert!(shell
        .icon_ref_for_url(
            zephium_ipc::IconSurface::Chrome,
            profile,
            "https://second.example/"
        )
        .is_some());
}

#[test]
fn lost_favicon_store_completion_falls_back_instead_of_sticking_pending() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    shell.store_reads = Some(StoreReadQueue::new());
    shell.self_queue = Some(CommandQueue::new());
    let id = active_id(&screen);
    shell.handle(Command::Engine(EngineEvent::UrlChanged {
        id,
        url: "https://fallback.example/".into(),
    }));
    assert!(shell.favicons.store_reads.contains_key(&id));
    assert!(engine
        .calls()
        .iter()
        .all(|call| call != &format!("discover {id}")));

    shell.handle(Command::FaviconPoll { id, attempt: 0 });
    assert!(!shell.favicons.store_reads.contains_key(&id));
    assert!(engine
        .calls()
        .iter()
        .any(|call| call == &format!("discover {id}")));
}

#[test]
fn bootstrap_hydrates_restored_tab_icons_without_native_callbacks() {
    let store = Arc::new(FakeStore::default());
    let (mut first, _engine, screen) = setup_with(store.clone());
    first.handle(Command::Bootstrap);
    let first_id = active_id(&screen);
    navigate_and_commit(&mut first, first_id, "restored-one.example");
    first.handle(Command::Open);
    let second_id = active_id(&screen);
    navigate_and_commit(&mut first, second_id, "restored-two.example");

    store.icons.lock().unwrap().extend([
        (
            "https://restored-one.example".to_owned(),
            vec![31; zephium_core::icon::RGBA32_BYTES],
        ),
        (
            "https://restored-two.example".to_owned(),
            vec![47; zephium_core::icon::RGBA32_BYTES],
        ),
    ]);
    drop(first);

    let (mut restored, engine, restored_screen) = setup_with(store);
    restored.handle(Command::Bootstrap);

    let state = last(&restored_screen);
    assert_eq!(state.tabs.len(), 3);
    for id in [first_id, second_id] {
        assert!(state
            .tabs
            .iter()
            .find(|tab| tab.id == id.to_string())
            .unwrap()
            .icon
            .is_some());
    }
    assert_eq!(
        restored.items.tab(active_id(&restored_screen)).unwrap().url,
        None
    );
    assert!(engine
        .calls()
        .iter()
        .all(|call| !call.starts_with("discover ")));
}

#[test]
fn positive_cache_eviction_allows_later_rehydration() {
    let (mut shell, _engine, _screen) = setup();
    let profile = ProfileId::from(91_000);
    let rgba = vec![83; zephium_core::icon::RGBA32_BYTES];
    let first = (profile, "https://icon-0.example".to_owned());

    for index in 0..=ICON_CACHE_CAPACITY {
        let key = (profile, format!("https://icon-{index}.example"));
        assert!(shell.cache_icon(key.clone(), &rgba));
        shell.favicons.icons_checked.insert(key);
    }

    assert!(!shell.favicons.icon_values.contains_key(&first));
    assert!(
        !shell.favicons.icons_checked.contains(&first),
        "an evicted positive must not become a permanent negative cache entry"
    );
}

#[test]
fn private_favicon_is_visible_but_never_written_to_persistent_storage() {
    let engine = Arc::new(FakeEngine::default());
    let store = Arc::new(FakeStore::default());
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
    let sink = screen.clone();
    let mut shell = Shell::new(
        engine,
        store.clone(),
        Arc::new(FakeChrome),
        Box::new(move |projection| {
            apply_projection(&mut sink.lock().unwrap(), projection);
        }),
    );
    shell.handle(Command::SetWindowSize(Size::new(1200.0, 800.0)));
    shell.handle(Command::Bootstrap);

    let profile = ProfileId::from(9001);
    let space = SpaceId::from(9002);
    assert!(shell.profiles.insert(Profile {
        id: profile,
        name: "Private".into(),
        kind: ProfileKind::Incognito,
    }));
    assert!(shell.initialize_new_blocker_profile(profile));
    assert!(shell.spaces.insert(Space {
        id: space,
        profile,
        name: "Private".into(),
        look: Default::default(),
    }));
    shell
        .windows
        .create(WindowKind::Main, profile, space, Size::new(1200.0, 800.0));
    shell.handle(Command::Open);
    let id = active_id(&screen);
    shell.handle(Command::Navigate {
        id,
        input: "private.example".into(),
    });
    shell.handle(Command::Engine(EngineEvent::UrlChanged {
        id,
        url: "https://private.example/".into(),
    }));
    present_committed(&mut shell, id, "https://private.example/");
    shell.handle(Command::Engine(EngineEvent::FaviconPixels {
        id,
        page_url: "https://private.example/".into(),
        rgba: vec![9; zephium_core::icon::RGBA32_BYTES],
    }));

    assert!(store.icons.lock().unwrap().is_empty());
    assert!(last(&screen)
        .tabs
        .iter()
        .find(|tab| tab.id == id.to_string())
        .is_some_and(|tab| tab.icon.is_some()));
}

#[test]
fn a_favourite_tab_restores_its_icon_like_any_other() {
    let store = Arc::new(FakeStore::default());
    let (mut first, _engine, screen) = setup_with(store.clone());
    first.handle(Command::Bootstrap);
    let id = active_id(&screen);
    navigate_and_commit(&mut first, id, "kept.example");
    first.handle(Command::SetTabEssential {
        id,
        essential: true,
        before: None,
    });
    store.icons.lock().unwrap().push((
        "https://kept.example".to_owned(),
        vec![19; zephium_core::icon::RGBA32_BYTES],
    ));
    drop(first);

    let (mut restored, _engine, restored_screen) = setup_with(store);
    restored.handle(Command::Bootstrap);

    let state = last(&restored_screen);
    let kept = state
        .tabs
        .iter()
        .find(|tab| tab.url.as_deref() == Some("https://kept.example/"))
        .expect("the favourite survives the restart");
    assert_eq!(
        kept.icon.as_ref().map(|icon| icon.origin.as_str()),
        Some("https://kept.example")
    );
}

#[test]
fn a_stored_icon_is_drawn_whatever_its_age_while_a_newer_one_is_fetched() {
    let store = Arc::new(FakeStore::default());
    store.icons.lock().unwrap().push((
        "https://ancient.example".to_owned(),
        vec![23; zephium_core::icon::RGBA32_BYTES],
    ));
    store
        .icon_ages
        .lock()
        .unwrap()
        .insert("https://ancient.example".to_owned(), 400 * 24 * 3600);

    let (mut shell, engine, screen) = setup_with(store);
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    navigate_and_commit(&mut shell, id, "ancient.example");

    let profile = shell.windows.focused().unwrap().profile;
    assert!(shell
        .icon_ref_for_url(
            zephium_ipc::IconSurface::Chrome,
            profile,
            "https://ancient.example/"
        )
        .is_some());
    assert!(engine
        .calls()
        .iter()
        .any(|call| call == &format!("discover {id}")));
}

#[test]
fn identical_pixels_are_sent_once_and_a_reattached_surface_gets_them_again() {
    let (mut shell, _engine, screen, icons) = setup_with_icon_log(Arc::new(FakeStore::default()));
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    navigate_and_commit(&mut shell, id, "once.example");
    let rgba = vec![55; zephium_core::icon::RGBA32_BYTES];

    shell.handle(Command::Engine(EngineEvent::FaviconPixels {
        id,
        page_url: "https://once.example/".into(),
        rgba: rgba.clone(),
    }));
    let delivered: usize = icons.lock().unwrap().iter().map(|v| v.entries.len()).sum();
    assert_eq!(delivered, 1);

    // The renderer rediscovering the same pixels must not resend them.
    shell.handle(Command::Engine(EngineEvent::FaviconPixels {
        id,
        page_url: "https://once.example/".into(),
        rgba,
    }));
    let delivered: usize = icons.lock().unwrap().iter().map(|v| v.entries.len()).sum();
    assert_eq!(delivered, 1);

    // A chrome reload leaves the webview with an empty raster cache.
    shell.handle(Command::Bootstrap);
    let entries: Vec<_> = icons
        .lock()
        .unwrap()
        .iter()
        .flat_map(|view| view.entries.iter().map(|entry| entry.origin.clone()))
        .collect();
    assert_eq!(entries, ["https://once.example", "https://once.example"]);
    assert!(icons
        .lock()
        .unwrap()
        .iter()
        .all(|view| view.surface == zephium_ipc::IconSurface::Chrome));
}

fn delivered_origins(icons: &IconLog) -> Vec<String> {
    icons
        .lock()
        .unwrap()
        .iter()
        .filter(|view| view.surface == zephium_ipc::IconSurface::Chrome)
        .flat_map(|view| view.entries.iter().map(|entry| entry.origin.clone()))
        .collect()
}

#[test]
fn returning_from_a_private_profile_resends_unchanged_favicons() {
    let (mut shell, _engine, screen, icons) = setup_with_icon_log(Arc::new(FakeStore::default()));
    shell.handle(Command::Bootstrap);
    let regular = active_id(&screen);
    navigate_and_commit(&mut shell, regular, "kept.example");
    shell.handle(Command::Engine(EngineEvent::FaviconPixels {
        id: regular,
        page_url: "https://kept.example/".into(),
        rgba: vec![5; zephium_core::icon::RGBA32_BYTES],
    }));
    shell.handle(Command::Run("window.newPrivate".into()));
    let private = active_id(&screen);
    let private_profile = shell.windows.focused().unwrap().profile;
    assert!(icons
        .lock()
        .unwrap()
        .iter()
        .any(|view| { view.profile_id == private_profile.to_string() && view.entries.is_empty() }));
    shell.handle(Command::Close(private));
    assert_eq!(
        delivered_origins(&icons),
        ["https://kept.example", "https://kept.example"]
    );
}

#[test]
fn a_background_profiles_favicon_does_not_replace_foreground_rasters() {
    let (mut shell, _engine, _screen, icons) = setup_with_icon_log(Arc::new(FakeStore::default()));
    shell.handle(Command::Bootstrap);
    let regular_profile = shell.windows.focused().unwrap().profile;
    shell.handle(Command::Run("window.newPrivate".into()));
    icons.lock().unwrap().clear();
    shell.handle(Command::Engine(EngineEvent::WorkPageFavicon {
        profile: regular_profile,
        page_url: "https://background.example/".into(),
        rgba: vec![6; zephium_core::icon::RGBA32_BYTES],
    }));
    assert!(shell
        .favicons
        .icon_values
        .contains_key(&(regular_profile, "https://background.example".into())));
    assert!(icons.lock().unwrap().is_empty());
}

#[test]
fn work_page_read_feeds_the_favicon_cache_by_origin() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, _engine, _screen, icons) = setup_with_icon_log(store.clone());
    shell.handle(Command::Bootstrap);
    let profile = shell.windows.focused().unwrap().profile;

    shell.handle(Command::Engine(EngineEvent::WorkPageFavicon {
        profile,
        page_url: "https://docs.example/guide".into(),
        rgba: vec![3; zephium_core::icon::RGBA32_BYTES - 1],
    }));
    shell.handle(Command::Engine(EngineEvent::WorkPageFavicon {
        profile: ProfileId::from(0xdead_u128),
        page_url: "https://other.example/".into(),
        rgba: vec![3; zephium_core::icon::RGBA32_BYTES],
    }));
    assert!(shell.favicons.icon_values.is_empty());

    let rgba = vec![3; zephium_core::icon::RGBA32_BYTES];
    shell.handle(Command::Engine(EngineEvent::WorkPageFavicon {
        profile,
        page_url: "https://docs.example/guide".into(),
        rgba: rgba.clone(),
    }));
    assert!(shell
        .favicons
        .icon_values
        .contains_key(&(profile, "https://docs.example".to_owned())));
    assert_eq!(
        store.icons.lock().unwrap().as_slice(),
        &[(String::from("https://docs.example"), rgba)]
    );
    assert_eq!(delivered_origins(&icons), ["https://docs.example"]);
}

type ProbeLog = Arc<Mutex<Vec<String>>>;

/// What the prober was asked for pages: the ticket and its source.
type PageLog = Arc<Mutex<Vec<(u64, crate::FaviconPageSource)>>>;

fn attach_probers(shell: &mut Shell) -> (ProbeLog, PageLog) {
    let origins: ProbeLog = Arc::new(Mutex::new(Vec::new()));
    let pages: PageLog = Arc::new(Mutex::new(Vec::new()));
    let (origin_sink, page_sink) = (origins.clone(), pages.clone());
    shell.handle(Command::AttachFaviconProber(
        crate::FaviconProberAttachment(Arc::new(move |request| match request {
            crate::FaviconProbe::Origin { origin, .. } => origin_sink.lock().unwrap().push(origin),
            crate::FaviconProbe::Page { ticket, source, .. } => {
                page_sink.lock().unwrap().push((ticket, source))
            }
        })),
    ));
    (origins, pages)
}

fn attach_prober(shell: &mut Shell) -> ProbeLog {
    attach_probers(shell).0
}

#[test]
fn favicon_probe_delivers_held_icons_and_asks_four_origins_at_a_time() {
    let store = Arc::new(FakeStore::default());
    let held = vec![5; zephium_core::icon::RGBA32_BYTES];
    store.icons.lock().unwrap().extend([
        ("https://held.example".to_owned(), held.clone()),
        ("https://old.example".to_owned(), held.clone()),
    ]);
    store.icon_ages.lock().unwrap().extend([
        ("https://held.example".to_owned(), 60),
        (
            "https://old.example".to_owned(),
            FAVICON_CACHE_MAX_AGE_SECONDS + 1,
        ),
    ]);
    let (mut shell, _engine, _screen, icons) = setup_with_icon_log(store.clone());
    shell.handle(Command::Bootstrap);
    let profile = shell.windows.focused().unwrap().profile;
    let probed = attach_prober(&mut shell);

    let origins = [
        "https://held.example/a",
        "https://old.example/",
        "https://a.example/x",
        "https://b.example/",
        "https://c.example/",
        "https://d.example/",
        "https://a.example/y",
        "http://plain.example/",
    ];
    shell.handle(Command::ProbeFavicons {
        profile,
        origins: origins.iter().map(|origin| origin.to_string()).collect(),
    });
    let delivered = delivered_origins(&icons);
    assert!(delivered.contains(&"https://held.example".to_owned()));
    assert!(delivered.contains(&"https://old.example".to_owned()));
    assert_eq!(
        probed.lock().unwrap().as_slice(),
        [
            "https://old.example",
            "https://a.example",
            "https://b.example",
            "https://c.example"
        ]
    );

    let rgba = vec![9; zephium_core::icon::RGBA32_BYTES];
    shell.handle(Command::FaviconProbed {
        profile,
        origin: "https://a.example".into(),
        rgba: Some(rgba.clone()),
    });
    assert!(shell
        .favicons
        .icon_values
        .contains_key(&(profile, "https://a.example".to_owned())));
    assert!(store
        .icons
        .lock()
        .unwrap()
        .contains(&("https://a.example".to_owned(), rgba)));
    assert!(delivered_origins(&icons).contains(&"https://a.example".to_owned()));
    assert_eq!(probed.lock().unwrap().last().unwrap(), "https://d.example");

    // An answer nobody asked for is ignored.
    shell.handle(Command::FaviconProbed {
        profile,
        origin: "https://unasked.example".into(),
        rgba: Some(vec![1; zephium_core::icon::RGBA32_BYTES]),
    });
    assert!(!shell
        .favicons
        .icon_values
        .contains_key(&(profile, "https://unasked.example".to_owned())));
}

#[test]
fn a_failed_origin_is_not_probed_again_within_the_hour() {
    let (mut shell, _engine, _screen, _icons) = setup_with_icon_log(Arc::new(FakeStore::default()));
    shell.handle(Command::Bootstrap);
    let profile = shell.windows.focused().unwrap().profile;
    let probed = attach_prober(&mut shell);
    let ask = |shell: &mut Shell| {
        shell.handle(Command::ProbeFavicons {
            profile,
            origins: vec!["https://quiet.example/".into()],
        })
    };

    ask(&mut shell);
    // Still in flight: a second request does not start another probe.
    ask(&mut shell);
    assert_eq!(probed.lock().unwrap().len(), 1);
    shell.handle(Command::FaviconProbed {
        profile,
        origin: "https://quiet.example".into(),
        rgba: None,
    });
    ask(&mut shell);
    assert_eq!(probed.lock().unwrap().len(), 1);

    // A raster that is not the fixed shape counts as a failure too.
    shell.handle(Command::ProbeFavicons {
        profile,
        origins: vec!["https://odd.example/".into()],
    });
    shell.handle(Command::FaviconProbed {
        profile,
        origin: "https://odd.example".into(),
        rgba: Some(vec![1; 16]),
    });
    shell.handle(Command::ProbeFavicons {
        profile,
        origins: vec!["https://odd.example/".into()],
    });
    assert_eq!(probed.lock().unwrap().len(), 2);
    assert!(shell.favicons.icon_values.is_empty());
}

#[test]
fn a_listed_address_without_an_icon_is_fetched_once_and_sent_to_the_surface_that_asked() {
    let store = Arc::new(FakeStore::default());
    let stored = vec![4; zephium_core::icon::RGBA32_BYTES];
    store
        .icons
        .lock()
        .unwrap()
        .push(("https://stored.example".to_owned(), stored));
    store
        .icon_ages
        .lock()
        .unwrap()
        .insert("https://stored.example".to_owned(), 60);
    let (mut shell, _engine, _screen, icons) = setup_with_icon_log(store);
    shell.handle(Command::Bootstrap);
    let profile = shell.windows.focused().unwrap().profile;
    let probed = attach_prober(&mut shell);
    let panel_origins = |icons: &IconLog| -> Vec<String> {
        icons
            .lock()
            .unwrap()
            .iter()
            .filter(|view| view.surface == zephium_ipc::IconSurface::Panel)
            .flat_map(|view| view.entries.iter().map(|entry| entry.origin.clone()))
            .collect()
    };

    shell.want_icons(
        zephium_ipc::IconSurface::Panel,
        profile,
        [
            "https://stored.example/page",
            "https://music.example/album/1",
            "https://music.example/album/2",
            "http://plain.example/",
        ],
    );
    shell.publish_icons();
    // The store answers first; only what it lacks goes to the network, once.
    assert_eq!(panel_origins(&icons), ["https://stored.example"]);
    assert_eq!(probed.lock().unwrap().as_slice(), ["https://music.example"]);

    shell.handle(Command::FaviconProbed {
        profile,
        origin: "https://music.example".into(),
        rgba: Some(vec![7; zephium_core::icon::RGBA32_BYTES]),
    });
    assert!(panel_origins(&icons).contains(&"https://music.example".to_owned()));

    // Now held: listing it again asks nothing.
    shell.want_icons(
        zephium_ipc::IconSurface::Panel,
        profile,
        ["https://music.example/album/3"],
    );
    assert_eq!(probed.lock().unwrap().len(), 1);
}

#[test]
fn the_launcher_is_sent_its_icons_again_only_after_its_page_loads_afresh() {
    let store = Arc::new(FakeStore::default());
    store.icons.lock().unwrap().push((
        "https://stored.example".to_owned(),
        vec![4; zephium_core::icon::RGBA32_BYTES],
    ));
    store
        .icon_ages
        .lock()
        .unwrap()
        .insert("https://stored.example".to_owned(), 60);
    let (mut shell, _engine, _screen, icons) = setup_with_icon_log(store);
    shell.handle(Command::Bootstrap);
    let window = shell.windows.focused().unwrap();
    let (profile, window_id, space) = (window.profile, window.id, window.space);
    let sent = |icons: &IconLog| {
        icons
            .lock()
            .unwrap()
            .iter()
            .filter(|view| view.surface == zephium_ipc::IconSurface::Panel)
            .map(|view| view.entries.len())
            .sum::<usize>()
    };
    let present = |shell: &mut Shell, session: &str| {
        shell.handle(Command::SearchScoped {
            query: String::new(),
            context: Box::new(zephium_ipc::SearchContext {
                window_id: window_id.to_string(),
                profile_id: profile.to_string(),
                space_id: space.to_string(),
                session_id: session.into(),
                request_id: session.into(),
            }),
        });
        shell.want_icons(
            zephium_ipc::IconSurface::Panel,
            profile,
            ["https://stored.example/page"],
        );
        // Once held, a result names the icon it carries and the surface is
        // sent whatever it lacks.
        let _ = shell.icon_ref_for(
            zephium_ipc::IconSurface::Panel,
            profile,
            "https://stored.example",
        );
        shell.publish_icons();
    };

    present(&mut shell, "first");
    assert_eq!(sent(&icons), 1);
    // The page stays mounted between presentations and still holds it.
    present(&mut shell, "second");
    assert_eq!(sent(&icons), 1);
    // A page loaded afresh holds nothing.
    shell.handle(Command::PanelLoaded);
    present(&mut shell, "third");
    assert_eq!(sent(&icons), 2);
}

#[test]
fn an_essential_never_opened_is_given_its_icon_from_the_store_or_the_network() {
    let store = Arc::new(FakeStore::default());
    store.icons.lock().unwrap().push((
        "https://kept.example".to_owned(),
        vec![6; zephium_core::icon::RGBA32_BYTES],
    ));
    let (mut shell, _engine, _screen, icons) = setup_with_icon_log(store);
    shell.handle(Command::Bootstrap);
    let window = shell.windows.focused().unwrap();
    let (profile, space) = (window.profile, window.space);
    let probed = attach_prober(&mut shell);
    let site = |url: &str| crate::ImportedSite {
        url: url.into(),
        title: String::new(),
    };
    let destination = shell.import_destination().unwrap();
    shell.handle(Command::Import {
        destination,
        work: Box::new(crate::ImportWork::Essentials(vec![
            site("https://kept.example/inbox"),
            site("https://bare.example/"),
        ])),
        done: crate::ImportCompletion::new(|_| {}),
    });

    shell.hydrate_favicon_cache(profile, space);
    assert!(delivered_origins(&icons).contains(&"https://kept.example".to_owned()));
    assert_eq!(probed.lock().unwrap().as_slice(), ["https://bare.example"]);
}

#[derive(Default)]
struct Surface {
    owner: Option<String>,
    held: std::collections::HashSet<(String, String)>,
}

impl Surface {
    fn replay(icons: &IconLog) -> Self {
        let mut surface = Self::default();
        for view in icons
            .lock()
            .unwrap()
            .iter()
            .filter(|view| view.surface == zephium_ipc::IconSurface::Chrome)
        {
            surface.owner = Some(view.profile_id.clone());
            for entry in &view.entries {
                surface
                    .held
                    .insert((entry.origin.clone(), entry.revision.clone()));
            }
        }
        surface
    }

    fn draws(&self, state: &ItemsState) -> bool {
        let icons: Vec<_> = state
            .tabs
            .iter()
            .filter_map(|tab| tab.icon.as_ref())
            .collect();
        !icons.is_empty()
            && icons.iter().all(|icon| {
                self.held
                    .contains(&(icon.origin.clone(), icon.revision.clone()))
            })
    }
}

fn paint(shell: &mut Shell, id: ItemId, host: &str, fill: u8) {
    navigate_and_commit(shell, id, host);
    shell.handle(Command::Engine(EngineEvent::FaviconPixels {
        id,
        page_url: format!("https://{host}/"),
        rgba: vec![fill; zephium_core::icon::RGBA32_BYTES],
    }));
}

#[test]
fn icons_survive_switching_profiles_and_coming_back() {
    let (mut shell, _engine, screen, icons) = setup_with_icon_log(Arc::new(FakeStore::default()));
    shell.handle(Command::Bootstrap);
    let personal = shell.windows.focused().unwrap().profile;
    paint(&mut shell, active_id(&screen), "shared.example", 21);

    shell.handle_operation(Command::CreateProfile {
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
    let work = shell
        .profiles
        .iter()
        .map(|profile| profile.id)
        .find(|id| *id != personal)
        .unwrap();
    shell.handle_operation(Command::SwitchProfile(work));
    paint(&mut shell, active_id(&screen), "shared.example", 99);
    let surface = Surface::replay(&icons);
    assert_eq!(surface.owner.as_deref(), Some(&*work.to_string()));
    assert!(surface.draws(&last(&screen)));

    shell.handle_operation(Command::SwitchProfile(personal));
    let surface = Surface::replay(&icons);
    assert_eq!(surface.owner.as_deref(), Some(&*personal.to_string()));
    assert!(surface.draws(&last(&screen)));

    shell.handle_operation(Command::SwitchProfile(work));
    shell.handle_operation(Command::SwitchProfile(personal));
    assert!(Surface::replay(&icons).draws(&last(&screen)));
}

#[test]
fn a_returning_profile_is_announced_even_when_every_icon_is_already_held() {
    let (mut shell, _engine, screen, icons) = setup_with_icon_log(Arc::new(FakeStore::default()));
    shell.handle(Command::Bootstrap);
    let personal = shell.windows.focused().unwrap().profile;
    paint(&mut shell, active_id(&screen), "home.example", 5);
    let work = {
        shell.handle_operation(Command::CreateProfile {
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
        shell
            .profiles
            .iter()
            .map(|profile| profile.id)
            .find(|id| *id != personal)
            .unwrap()
    };
    shell.handle_operation(Command::SwitchProfile(work));
    let before = icons.lock().unwrap().len();
    shell.handle_operation(Command::SwitchProfile(personal));
    let views = icons.lock().unwrap();
    let announced = views[before..]
        .iter()
        .any(|view| view.profile_id == personal.to_string() && view.entries.is_empty());
    assert!(announced);
}

fn raster(fill: u8) -> Vec<u8> {
    vec![fill; zephium_core::icon::RGBA32_BYTES]
}

fn tab_icon(screen: &Screen, id: ItemId) -> Option<zephium_ipc::IconRef> {
    last(screen)
        .tabs
        .into_iter()
        .find(|tab| tab.id == id.to_string())
        .and_then(|tab| tab.icon)
}

/// Opens `url` in the tab and gives it the icon the page declares.
fn open_page(shell: &mut Shell, id: ItemId, url: &str, icon: &[u8]) {
    // The interval that spaces a tab's page lookups is not what these test.
    shell.favicons.page_checks.clear();
    navigate_and_commit(shell, id, url);
    shell.handle(Command::Engine(EngineEvent::FaviconPixels {
        id,
        page_url: url.into(),
        rgba: icon.to_vec(),
    }));
}

#[test]
fn pages_of_one_origin_each_wear_the_icon_they_declare() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, _engine, screen) = setup_with(store.clone());
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    let (docs, sheets) = (raster(10), raster(20));
    let revision = |rgba: &[u8]| zephium_core::icon::revision(rgba).unwrap();

    open_page(&mut shell, id, "https://example.com/docs/", &docs);
    assert_eq!(
        store.icons.lock().unwrap().as_slice(),
        &[("https://example.com".to_owned(), docs.clone())],
        "the first page's icon is the origin's"
    );
    let icon = tab_icon(&screen, id).unwrap();
    assert_eq!(
        (icon.origin.as_str(), icon.revision),
        ("https://example.com", revision(&docs))
    );

    open_page(&mut shell, id, "https://example.com/sheets/", &sheets);
    assert_eq!(
        store.icons.lock().unwrap().last(),
        Some(&("https://example.com/sheets/".to_owned(), sheets.clone()))
    );
    let icon = tab_icon(&screen, id).unwrap();
    assert_eq!(
        (icon.origin.as_str(), icon.revision),
        ("https://example.com/sheets/", revision(&sheets))
    );

    let saved = store.icons.lock().unwrap().len();
    open_page(&mut shell, id, "https://example.com/docs/", &docs);
    assert_eq!(
        store.icons.lock().unwrap().len(),
        saved,
        "nothing new to keep"
    );
    let icon = tab_icon(&screen, id).unwrap();
    assert_eq!(
        (icon.origin.as_str(), icon.revision),
        ("https://example.com", revision(&docs))
    );
}

#[test]
fn a_page_declaring_its_origins_icon_costs_no_row_and_no_cache_entry() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, _engine, screen) = setup_with(store.clone());
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    let site = raster(10);

    open_page(&mut shell, id, "https://example.com/a", &site);
    for page in ["b", "c?q=1", "d#top", "e"] {
        open_page(
            &mut shell,
            id,
            &format!("https://example.com/{page}"),
            &site,
        );
    }

    assert_eq!(store.icons.lock().unwrap().len(), 1);
    assert_eq!(shell.favicons.icon_values.len(), 1);
    assert_eq!(tab_icon(&screen, id).unwrap().origin, "https://example.com");
}

#[test]
fn an_icon_is_looked_up_by_page_then_origin_then_nothing_of_another_page() {
    let (mut shell, _engine, _screen) = setup();
    shell.handle(Command::Bootstrap);
    let profile = shell.windows.focused().unwrap().profile;
    let key = |value: &str| (profile, value.to_owned());
    assert!(shell.cache_icon(key("https://example.com"), &raster(1)));
    assert!(shell.cache_icon(key("https://example.com/sheets/"), &raster(2)));
    let origin_of = |shell: &Shell, url: &str| {
        shell
            .icon_ref_for_url(zephium_ipc::IconSurface::Chrome, profile, url)
            .map(|icon| icon.origin)
    };

    assert_eq!(
        origin_of(&shell, "https://example.com/sheets/?tab=2#cell"),
        Some("https://example.com/sheets/".into())
    );
    assert_eq!(
        origin_of(&shell, "https://example.com/docs/"),
        Some("https://example.com".into())
    );
    assert_eq!(
        origin_of(&shell, "https://example.com/sheets"),
        Some("https://example.com".into()),
        "another page, not the sheets one"
    );
    let long = format!("https://example.com/sheets/{}", "x".repeat(2100));
    assert_eq!(origin_of(&shell, &long), Some("https://example.com".into()));
    assert_eq!(origin_of(&shell, "https://other.example/sheets/"), None);
}

#[test]
fn a_page_never_borrows_another_pages_icon_as_its_sites() {
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    let profile = shell.windows.focused().unwrap().profile;
    assert!(shell.cache_icon((profile, "https://example.com/sheets/".into()), &raster(2)));

    navigate_and_commit(&mut shell, id, "https://app.example.com/inbox");

    assert!(tab_icon(&screen, id).is_none());
}

#[test]
fn a_private_pages_own_icon_is_drawn_but_never_written() {
    let engine = Arc::new(FakeEngine::default());
    let store = Arc::new(FakeStore::default());
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
    let sink = screen.clone();
    let mut shell = Shell::new(
        engine,
        store.clone(),
        Arc::new(FakeChrome),
        Box::new(move |projection| {
            apply_projection(&mut sink.lock().unwrap(), projection);
        }),
    );
    shell.handle(Command::SetWindowSize(Size::new(1200.0, 800.0)));
    shell.handle(Command::Bootstrap);
    let profile = ProfileId::from(9001);
    let space = SpaceId::from(9002);
    assert!(shell.profiles.insert(Profile {
        id: profile,
        name: "Private".into(),
        kind: ProfileKind::Incognito,
    }));
    assert!(shell.initialize_new_blocker_profile(profile));
    assert!(shell.spaces.insert(Space {
        id: space,
        profile,
        name: "Private".into(),
        look: Default::default(),
    }));
    shell
        .windows
        .create(WindowKind::Main, profile, space, Size::new(1200.0, 800.0));
    shell.handle(Command::Open);
    let id = active_id(&screen);

    open_page(&mut shell, id, "https://private.example/docs/", &raster(10));
    open_page(
        &mut shell,
        id,
        "https://private.example/sheets/",
        &raster(20),
    );

    assert!(store.icons.lock().unwrap().is_empty());
    assert_eq!(
        tab_icon(&screen, id).map(|icon| icon.origin),
        Some("https://private.example/sheets/".to_owned())
    );
    assert!(shell
        .favicons
        .icon_values
        .contains_key(&(profile, "https://private.example/sheets/".to_owned())));
}

#[test]
fn restored_tabs_wear_their_stored_page_icon_before_any_page_loads() {
    let store = Arc::new(FakeStore::default());
    let (mut first, _engine, screen) = setup_with(store.clone());
    first.handle(Command::Bootstrap);
    let id = active_id(&screen);
    navigate_and_commit(&mut first, id, "https://example.com/sheets/");
    store.icons.lock().unwrap().extend([
        ("https://example.com".to_owned(), raster(1)),
        ("https://example.com/sheets/".to_owned(), raster(2)),
    ]);
    drop(first);

    let (mut restored, engine, restored_screen) = setup_with(store);
    restored.handle(Command::Bootstrap);

    let tab = last(&restored_screen).tabs.into_iter().next().unwrap();
    assert_eq!(
        tab.icon.map(|icon| icon.origin),
        Some("https://example.com/sheets/".to_owned())
    );
    assert!(engine
        .calls()
        .iter()
        .all(|call| !call.starts_with("discover ")));
}

#[test]
fn a_stored_page_icon_that_now_matches_its_origin_is_dropped() {
    let store = Arc::new(FakeStore::default());
    store.icons.lock().unwrap().extend([
        ("https://example.com".to_owned(), raster(1)),
        ("https://example.com/sheets/".to_owned(), raster(2)),
    ]);
    {
        let mut ages = store.icon_ages.lock().unwrap();
        ages.insert("https://example.com".to_owned(), 60);
        ages.insert("https://example.com/sheets/".to_owned(), 400 * 24 * 3600);
    }
    let (mut shell, engine, screen) = setup_with(store.clone());
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    navigate_and_commit(&mut shell, id, "https://example.com/sheets/");

    // The old page icon is drawn while a newer one is fetched.
    assert_eq!(
        tab_icon(&screen, id).map(|icon| icon.origin),
        Some("https://example.com/sheets/".to_owned())
    );
    assert!(engine
        .calls()
        .iter()
        .any(|call| call == &format!("discover {id}")));

    shell.handle(Command::Engine(EngineEvent::FaviconPixels {
        id,
        page_url: "https://example.com/sheets/".into(),
        rgba: raster(1),
    }));

    assert_eq!(
        tab_icon(&screen, id).map(|icon| icon.origin),
        Some("https://example.com".to_owned())
    );
    assert!(store
        .icons
        .lock()
        .unwrap()
        .iter()
        .all(|(key, _)| key != "https://example.com/sheets/"));
}

#[test]
fn a_fresh_stored_page_icon_needs_no_renderer_pass() {
    let store = Arc::new(FakeStore::default());
    store.icons.lock().unwrap().extend([
        ("https://example.com".to_owned(), raster(1)),
        ("https://example.com/sheets/".to_owned(), raster(2)),
    ]);
    {
        let mut ages = store.icon_ages.lock().unwrap();
        ages.insert("https://example.com".to_owned(), 60);
        ages.insert("https://example.com/sheets/".to_owned(), 60);
    }
    let (mut shell, engine, screen) = setup_with(store);
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    navigate_and_commit(&mut shell, id, "https://example.com/sheets/");

    assert!(engine
        .calls()
        .iter()
        .all(|call| call != &format!("discover {id}")));
    for _ in 0..3 {
        shell.handle(Command::Engine(EngineEvent::LoadingChanged {
            id,
            loading: false,
        }));
    }
    assert!(engine
        .calls()
        .iter()
        .all(|call| call != &format!("discover {id}")));
}

#[test]
fn a_page_looked_up_once_is_not_looked_up_again_for_its_other_addresses() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    open_page(&mut shell, id, "https://example.com/docs/", &raster(10));
    open_page(&mut shell, id, "https://example.com/sheets/", &raster(20));
    let discovered = |engine: &FakeEngine| {
        engine
            .calls()
            .iter()
            .filter(|call| call.as_str() == format!("discover {id}"))
            .count()
    };
    let before = discovered(&engine);

    for url in [
        "https://example.com/sheets/?tab=2",
        "https://example.com/sheets/#cell",
    ] {
        commit_url(&mut shell, id, url);
    }
    shell.handle(Command::Engine(EngineEvent::LoadingChanged {
        id,
        loading: false,
    }));

    assert_eq!(discovered(&engine), before);
}

#[test]
fn page_lookups_within_an_origin_are_spaced_and_the_last_address_wins() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    shell.self_queue = Some(CommandQueue::new());
    let id = active_id(&screen);
    open_page(&mut shell, id, "https://example.com/a", &raster(10));
    let discovered = |engine: &FakeEngine| {
        engine
            .calls()
            .iter()
            .filter(|call| call.as_str() == format!("discover {id}"))
            .count()
    };
    let before = discovered(&engine);

    for page in ["b", "c", "d"] {
        commit_url(&mut shell, id, &format!("https://example.com/{page}"));
    }
    assert_eq!(discovered(&engine), before, "held back inside the interval");
    assert!(shell.favicons.icon_attempts.is_empty());

    let (_, at) = shell.favicons.page_checks.get_mut(&id).unwrap();
    *at = std::time::Instant::now()
        .checked_sub(std::time::Duration::from_secs(5))
        .unwrap();
    shell.handle(Command::FaviconPoll {
        id,
        attempt: DEFERRED_PAGE_CHECK,
    });

    assert_eq!(discovered(&engine), before + 1);
    let attempt = shell.favicons.icon_attempts.get(&id).unwrap();
    assert_eq!(
        attempt.target.page.as_deref(),
        Some("https://example.com/d")
    );
}

#[test]
fn a_different_origin_is_never_held_back() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    shell.self_queue = Some(CommandQueue::new());
    let id = active_id(&screen);
    open_page(&mut shell, id, "https://example.com/a", &raster(10));
    commit_url(&mut shell, id, "https://other.example/");

    assert!(engine
        .calls()
        .iter()
        .any(|call| call == &format!("discover {id}")));
    assert!(shell.favicons.icon_attempts.contains_key(&id));
}

#[test]
fn replacing_an_origins_icon_keeps_it_for_the_open_pages_that_wore_it() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, _engine, screen) = setup_with(store.clone());
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    let profile = shell.windows.focused().unwrap().profile;
    let (docs, sheets) = (raster(10), raster(20));
    open_page(&mut shell, id, "https://example.com/docs/", &docs);

    assert!(shell.admit_origin_icon(profile, "https://example.com", &sheets));

    let icon = tab_icon(&screen, id).unwrap();
    assert_eq!(icon.origin, "https://example.com/docs/");
    assert_eq!(icon.revision, zephium_core::icon::revision(&docs).unwrap());
    let saved = store.icons.lock().unwrap().clone();
    assert!(saved
        .iter()
        .any(|(key, bytes)| key == "https://example.com/docs/" && *bytes == docs));
    assert!(saved
        .iter()
        .any(|(key, bytes)| key == "https://example.com" && *bytes == sheets));
    assert_eq!(
        shell
            .icon_ref_for_url(
                zephium_ipc::IconSurface::Chrome,
                profile,
                "https://example.com/other"
            )
            .map(|icon| icon.revision),
        zephium_core::icon::revision(&sheets)
    );
}

#[test]
fn a_page_icon_that_leaves_the_cache_is_looked_up_again() {
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    let profile = shell.windows.focused().unwrap().profile;
    open_page(&mut shell, id, "https://example.com/docs/", &raster(10));
    open_page(&mut shell, id, "https://example.com/sheets/", &raster(20));
    assert!(shell.favicons.settled.contains_key(&id));

    for index in 0..ICON_CACHE_CAPACITY {
        assert!(shell.cache_icon(
            (profile, format!("https://icon-{index}.example")),
            &raster(3)
        ));
    }

    assert!(!shell.favicons.settled.contains_key(&id));
    assert!(!shell
        .favicons
        .icon_values
        .contains_key(&(profile, "https://example.com/sheets/".to_owned())));
}

#[test]
fn deleting_a_profile_forgets_its_page_icons_and_settled_tabs() {
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    let profile = shell.windows.focused().unwrap().profile;
    open_page(&mut shell, id, "https://example.com/docs/", &raster(10));
    open_page(&mut shell, id, "https://example.com/sheets/", &raster(20));
    assert!(shell.favicons.settled.contains_key(&id));

    shell.apply_profile_tombstone(profile);

    assert!(shell.favicons.settled.is_empty());
    assert!(shell
        .favicons
        .icon_values
        .keys()
        .all(|(owner, _)| *owner != profile));
}

#[test]
fn a_path_too_long_to_key_falls_back_to_its_origin_alone() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, _engine, screen) = setup_with(store.clone());
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    let url = format!("https://example.com/{}", "a".repeat(2100));

    open_page(&mut shell, id, &url, &raster(10));

    assert_eq!(
        store.icons.lock().unwrap().as_slice(),
        &[("https://example.com".to_owned(), raster(10))]
    );
    assert!(
        shell
            .favicons
            .settled
            .values()
            .all(|target| target.page.is_none()),
        "a page too long to key settles as its origin alone"
    );
}

use zephium_core::icon::{IconCandidate, IconKind};

fn link(url: &str, kind: IconKind, side: Option<u32>) -> IconCandidate {
    let svg = url.ends_with(".svg");
    IconCandidate {
        url: url.into(),
        kind,
        side,
        scalable: svg,
        svg,
    }
}

fn png(url: &str, side: u32) -> IconCandidate {
    link(url, IconKind::Icon, Some(side))
}

fn report(
    shell: &mut Shell,
    id: ItemId,
    page_url: &str,
    complete: bool,
    links: Vec<IconCandidate>,
) {
    shell.handle(Command::Engine(EngineEvent::FaviconCandidates {
        id,
        page_url: page_url.into(),
        complete,
        candidates: links,
    }));
}

fn answer(shell: &mut Shell, ticket: u64, rgba: Option<Vec<u8>>, source: &str, failed: &[&str]) {
    shell.handle(Command::FaviconFetched {
        ticket,
        rgba,
        source: Some(source.into()),
        failed: failed.iter().map(|address| address.to_string()).collect(),
    });
}

fn addresses(pages: &PageLog, nth: usize) -> Vec<String> {
    match &pages.lock().unwrap()[nth].1 {
        crate::FaviconPageSource::Addresses(addresses) => addresses.clone(),
        other => panic!("not an address request: {other:?}"),
    }
}

fn calls_matching(engine: &FakeEngine, prefix: &str) -> usize {
    engine
        .calls()
        .iter()
        .filter(|call| call.starts_with(prefix))
        .count()
}

/// A private window's first tab.
fn private_tab(shell: &mut Shell, screen: &Screen) -> (ProfileId, ItemId) {
    let profile = ProfileId::from(9001);
    let space = SpaceId::from(9002);
    assert!(shell.profiles.insert(Profile {
        id: profile,
        name: "Private".into(),
        kind: ProfileKind::Incognito,
    }));
    assert!(shell.initialize_new_blocker_profile(profile));
    assert!(shell.spaces.insert(Space {
        id: space,
        profile,
        name: "Private".into(),
        look: Default::default(),
    }));
    shell
        .windows
        .create(WindowKind::Main, profile, space, Size::new(1200.0, 800.0));
    shell.handle(Command::Open);
    (profile, active_id(screen))
}

#[test]
fn a_page_names_its_icons_and_the_prober_is_given_the_best_ones() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, engine, screen) = setup_with(store.clone());
    shell.handle(Command::Bootstrap);
    let (_, pages) = attach_probers(&mut shell);
    let id = active_id(&screen);
    let page = "https://docs.example/document/d/1";
    navigate_and_commit(&mut shell, id, page);
    assert_eq!(calls_matching(&engine, &format!("discover {id}")), 1);
    // The same page reported again while it is being looked up starts nothing.
    shell.handle(Command::Engine(EngineEvent::UrlChanged {
        id,
        url: format!("{page}?tab=1#section"),
    }));
    shell.handle(Command::Engine(EngineEvent::LoadingChanged {
        id,
        loading: false,
    }));
    assert_eq!(calls_matching(&engine, &format!("discover {id}")), 1);

    report(
        &mut shell,
        id,
        page,
        false,
        vec![
            png("https://docs.example/16.png", 16),
            png("https://ssl.gstatic.com/docs/doc_32.png", 32),
            link("https://docs.example/touch.png", IconKind::Touch, Some(180)),
            link("https://docs.example/pin.svg", IconKind::Mask, None),
        ],
    );
    // Cross-origin icons are fetched like any other; the mask is not asked for.
    assert_eq!(pages.lock().unwrap().len(), 1);
    assert_eq!(
        addresses(&pages, 0),
        [
            "https://ssl.gstatic.com/docs/doc_32.png",
            "https://docs.example/16.png",
            "https://docs.example/touch.png"
        ]
    );
    assert!(tab_icon(&screen, id).is_none());

    let ticket = pages.lock().unwrap()[0].0;
    answer(
        &mut shell,
        ticket,
        Some(raster(7)),
        "https://ssl.gstatic.com/docs/doc_32.png",
        &[],
    );
    assert_eq!(
        tab_icon(&screen, id).map(|icon| icon.origin),
        Some("https://docs.example".to_owned())
    );
    assert_eq!(
        store.icons.lock().unwrap().as_slice(),
        &[("https://docs.example".to_owned(), raster(7))]
    );
    // Nothing else is asked of the page or the network.
    report(&mut shell, id, page, true, Vec::new());
    assert_eq!(pages.lock().unwrap().len(), 1);
    assert_eq!(calls_matching(&engine, "rasterize"), 0);
    assert!(!shell.favicons.icon_attempts.contains_key(&id));
    assert_eq!(calls_matching(&engine, &format!("discover {id}")), 1);
}

#[test]
fn a_report_for_a_page_the_tab_has_left_is_ignored() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let (_, pages) = attach_probers(&mut shell);
    let id = active_id(&screen);
    navigate_and_commit(&mut shell, id, "https://first.example/a");
    navigate_and_commit(&mut shell, id, "https://second.example/b");

    report(
        &mut shell,
        id,
        "https://first.example/a",
        true,
        vec![png("https://first.example/i.png", 32)],
    );
    report(
        &mut shell,
        id,
        "https://second.example/other",
        true,
        vec![png("https://second.example/i.png", 32)],
    );
    assert!(pages.lock().unwrap().is_empty());
    assert_eq!(calls_matching(&engine, "rasterize"), 0);
}

#[test]
fn a_page_naming_the_address_its_origin_wears_costs_no_fetch_and_no_row() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, _engine, screen) = setup_with(store.clone());
    shell.handle(Command::Bootstrap);
    let (_, pages) = attach_probers(&mut shell);
    let id = active_id(&screen);
    let shared = "https://app.example/icon-32.png";

    navigate_and_commit(&mut shell, id, "https://app.example/inbox");
    report(
        &mut shell,
        id,
        "https://app.example/inbox",
        true,
        vec![png(shared, 32)],
    );
    let ticket = pages.lock().unwrap()[0].0;
    answer(&mut shell, ticket, Some(raster(5)), shared, &[]);

    shell.favicons.page_checks.clear();
    navigate_and_commit(&mut shell, id, "https://app.example/settings");
    report(
        &mut shell,
        id,
        "https://app.example/settings",
        true,
        vec![png(shared, 32)],
    );

    assert_eq!(
        pages.lock().unwrap().len(),
        1,
        "the same address is not fetched twice"
    );
    assert_eq!(
        tab_icon(&screen, id).map(|icon| icon.origin),
        Some("https://app.example".to_owned())
    );
    assert_eq!(
        store.icons.lock().unwrap().len(),
        1,
        "and costs no row of its own"
    );
    let profile = shell.profile_of_item(id).unwrap();
    assert!(!shell
        .favicons
        .icon_values
        .contains_key(&(profile, "https://app.example/settings".to_owned())));
}

#[test]
fn a_page_naming_another_address_wears_what_it_gives() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, _engine, screen) = setup_with(store.clone());
    shell.handle(Command::Bootstrap);
    let (_, pages) = attach_probers(&mut shell);
    let id = active_id(&screen);

    navigate_and_commit(&mut shell, id, "https://suite.example/docs/");
    report(
        &mut shell,
        id,
        "https://suite.example/docs/",
        true,
        vec![png("https://suite.example/docs.png", 32)],
    );
    let ticket = pages.lock().unwrap()[0].0;
    answer(
        &mut shell,
        ticket,
        Some(raster(1)),
        "https://suite.example/docs.png",
        &[],
    );

    shell.favicons.page_checks.clear();
    navigate_and_commit(&mut shell, id, "https://suite.example/sheets/");
    report(
        &mut shell,
        id,
        "https://suite.example/sheets/",
        true,
        vec![png("https://suite.example/sheets.png", 32)],
    );
    assert_eq!(pages.lock().unwrap().len(), 2);
    let ticket = pages.lock().unwrap()[1].0;
    answer(
        &mut shell,
        ticket,
        Some(raster(2)),
        "https://suite.example/sheets.png",
        &[],
    );

    assert_eq!(
        tab_icon(&screen, id).map(|icon| icon.origin),
        Some("https://suite.example/sheets/".to_owned())
    );
    assert!(store
        .icons
        .lock()
        .unwrap()
        .contains(&("https://suite.example/sheets/".to_owned(), raster(2))));
}

#[test]
fn a_later_report_ranking_another_address_first_replaces_the_icon() {
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let (_, pages) = attach_probers(&mut shell);
    let id = active_id(&screen);
    let page = "https://chat.example/room";

    navigate_and_commit(&mut shell, id, page);
    report(
        &mut shell,
        id,
        page,
        false,
        vec![png("https://chat.example/idle.png", 32)],
    );
    let ticket = pages.lock().unwrap()[0].0;
    answer(
        &mut shell,
        ticket,
        Some(raster(1)),
        "https://chat.example/idle.png",
        &[],
    );
    let idle = tab_icon(&screen, id).unwrap().revision;

    // The page swaps its icon for an unread badge while it is still settling.
    report(
        &mut shell,
        id,
        page,
        false,
        vec![png("https://chat.example/unread.png", 32)],
    );
    assert_eq!(pages.lock().unwrap().len(), 2);
    let ticket = pages.lock().unwrap()[1].0;
    answer(
        &mut shell,
        ticket,
        Some(raster(2)),
        "https://chat.example/unread.png",
        &[],
    );
    assert_ne!(tab_icon(&screen, id).unwrap().revision, idle);

    // A report that only adds lower-ranked addresses asks for nothing.
    report(
        &mut shell,
        id,
        page,
        true,
        vec![
            png("https://chat.example/unread.png", 32),
            png("https://chat.example/other.png", 48),
        ],
    );
    assert_eq!(pages.lock().unwrap().len(), 2);
}

#[test]
fn what_the_prober_cannot_read_is_decoded_by_the_page() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let (_, pages) = attach_probers(&mut shell);
    let id = active_id(&screen);
    let page = "http://plain.example/";
    navigate_and_commit(&mut shell, id, page);

    // A plain-http page: the prober only takes public https, so the page's
    // renderer decodes its own address, with its cookies and its policy.
    report(
        &mut shell,
        id,
        page,
        false,
        vec![png("http://plain.example/i.png", 32)],
    );
    assert!(pages.lock().unwrap().is_empty());
    assert_eq!(
        engine
            .calls()
            .iter()
            .filter(|call| call.starts_with(&format!("rasterize {id} ")))
            .cloned()
            .collect::<Vec<_>>(),
        [format!("rasterize {id} http://plain.example/i.png")]
    );
    shell.handle(Command::Engine(EngineEvent::FaviconPixels {
        id,
        page_url: page.into(),
        rgba: raster(3),
    }));
    assert!(tab_icon(&screen, id).is_some());
}

#[test]
fn a_vector_icon_is_decoded_by_the_page_before_lower_ranked_bitmaps_are_tried() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let (_, pages) = attach_probers(&mut shell);
    let id = active_id(&screen);
    let page = "https://vector.example/";
    navigate_and_commit(&mut shell, id, page);

    report(
        &mut shell,
        id,
        page,
        true,
        vec![link(
            "https://vector.example/logo.svg",
            IconKind::Icon,
            None,
        )],
    );
    assert!(
        pages.lock().unwrap().is_empty(),
        "the prober never sees a vector"
    );
    assert_eq!(
        calls_matching(
            &engine,
            &format!("rasterize {id} https://vector.example/logo.svg")
        ),
        1
    );
    shell.handle(Command::Engine(EngineEvent::FaviconPixels {
        id,
        page_url: page.into(),
        rgba: raster(4),
    }));
    assert!(tab_icon(&screen, id).is_some());
    assert!(!shell.favicons.icon_attempts.contains_key(&id));
}

#[test]
fn an_address_nobody_can_read_is_remembered_and_the_page_tries_the_next() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let (_, pages) = attach_probers(&mut shell);
    let id = active_id(&screen);
    let page = "https://gated.example/a";
    navigate_and_commit(&mut shell, id, page);
    report(
        &mut shell,
        id,
        page,
        true,
        vec![png("https://gated.example/icon.png", 32)],
    );
    // The prober is given the declared icon and the site's default one.
    assert_eq!(
        addresses(&pages, 0),
        [
            "https://gated.example/icon.png",
            "https://gated.example/favicon.ico"
        ]
    );
    let ticket = pages.lock().unwrap()[0].0;
    answer(
        &mut shell,
        ticket,
        None,
        "",
        &[
            "https://gated.example/icon.png",
            "https://gated.example/favicon.ico",
        ],
    );

    // Anonymously unreadable is not unreadable: the page asks with its cookies.
    assert_eq!(
        calls_matching(
            &engine,
            &format!("rasterize {id} https://gated.example/icon.png")
        ),
        1
    );
    shell.handle(Command::Engine(EngineEvent::FaviconRasterFailed {
        id,
        page_url: page.into(),
        url: "https://gated.example/icon.png".into(),
    }));
    assert_eq!(
        calls_matching(
            &engine,
            &format!("rasterize {id} https://gated.example/favicon.ico")
        ),
        1
    );
    shell.handle(Command::Engine(EngineEvent::FaviconRasterFailed {
        id,
        page_url: page.into(),
        url: "https://gated.example/favicon.ico".into(),
    }));
    assert!(tab_icon(&screen, id).is_none());
    assert!(
        !shell.favicons.icon_attempts.contains_key(&id),
        "nothing is left to try"
    );

    // Another page of the site starts no network work for what already failed.
    shell.favicons.page_checks.clear();
    navigate_and_commit(&mut shell, id, "https://gated.example/b");
    report(
        &mut shell,
        id,
        "https://gated.example/b",
        true,
        vec![png("https://gated.example/icon.png", 32)],
    );
    assert_eq!(pages.lock().unwrap().len(), 1);
    assert_eq!(calls_matching(&engine, "rasterize"), 2);
}

#[test]
fn a_private_tab_never_asks_the_prober_and_never_stores() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, engine, screen) = setup_with(store.clone());
    shell.handle(Command::Bootstrap);
    let (_, pages) = attach_probers(&mut shell);
    let (profile, id) = private_tab(&mut shell, &screen);
    let page = "https://secret.example/a";
    navigate_and_commit(&mut shell, id, page);

    report(
        &mut shell,
        id,
        page,
        true,
        vec![
            png("https://cdn.secret.example/icon.png", 32),
            png("https://secret.example/own.png", 32),
        ],
    );
    assert!(pages.lock().unwrap().is_empty());
    // Same-origin addresses go first: a canvas can read only those.
    assert_eq!(
        calls_matching(
            &engine,
            &format!("rasterize {id} https://secret.example/own.png")
        ),
        1
    );
    shell.handle(Command::Engine(EngineEvent::FaviconPixels {
        id,
        page_url: page.into(),
        rgba: raster(8),
    }));
    assert!(tab_icon(&screen, id).is_some());
    assert!(store.icons.lock().unwrap().is_empty());
    assert!(shell
        .favicons
        .icon_values
        .contains_key(&(profile, "https://secret.example".to_owned())));
}

#[test]
fn a_page_naming_nothing_is_concluded_once_and_duplicate_loads_start_nothing() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let (_, pages) = attach_probers(&mut shell);
    let id = active_id(&screen);
    let page = "https://bare.example/";
    navigate_and_commit(&mut shell, id, page);
    shell.handle(Command::Engine(EngineEvent::LoadingChanged {
        id,
        loading: true,
    }));

    // A loaded document with no icon links: the site's default is asked for.
    report(&mut shell, id, page, false, Vec::new());
    assert_eq!(addresses(&pages, 0), ["https://bare.example/favicon.ico"]);
    let ticket = pages.lock().unwrap()[0].0;
    answer(
        &mut shell,
        ticket,
        None,
        "",
        &["https://bare.example/favicon.ico"],
    );
    assert_eq!(
        calls_matching(&engine, "rasterize"),
        1,
        "then the page tries the same address"
    );
    shell.handle(Command::Engine(EngineEvent::FaviconRasterFailed {
        id,
        page_url: page.into(),
        url: "https://bare.example/favicon.ico".into(),
    }));
    report(&mut shell, id, page, true, Vec::new());

    assert!(tab_icon(&screen, id).is_none());
    assert!(!shell.favicons.icon_attempts.contains_key(&id));
    let discovered = calls_matching(&engine, &format!("discover {id}"));
    for _ in 0..3 {
        shell.handle(Command::Engine(EngineEvent::LoadingChanged {
            id,
            loading: false,
        }));
        shell.handle(Command::Engine(EngineEvent::UrlChanged {
            id,
            url: page.into(),
        }));
    }
    assert_eq!(
        calls_matching(&engine, &format!("discover {id}")),
        discovered
    );
    assert_eq!(pages.lock().unwrap().len(), 1);
}

#[test]
fn the_engines_own_icon_comes_first_and_declared_addresses_wait_for_the_load() {
    let (mut shell, engine, screen) = setup();
    engine
        .native_favicons
        .store(true, std::sync::atomic::Ordering::Release);
    shell.handle(Command::Bootstrap);
    let (_, pages) = attach_probers(&mut shell);
    let id = active_id(&screen);
    let page = "https://win.example/x";
    navigate_and_commit(&mut shell, id, page);
    shell.handle(Command::Engine(EngineEvent::LoadingChanged {
        id,
        loading: true,
    }));

    report(
        &mut shell,
        id,
        page,
        false,
        vec![png("https://win.example/i.png", 32)],
    );
    assert!(
        pages.lock().unwrap().is_empty(),
        "the engine may still name its own"
    );

    shell.handle(Command::Engine(EngineEvent::FaviconImage {
        id,
        page_url: page.into(),
        png: vec![1, 2, 3],
    }));
    assert_eq!(pages.lock().unwrap().len(), 1);
    assert_eq!(
        pages.lock().unwrap()[0].1,
        crate::FaviconPageSource::Encoded(vec![1, 2, 3])
    );
    let ticket = pages.lock().unwrap()[0].0;
    answer(&mut shell, ticket, Some(raster(6)), "engine", &[]);
    assert!(tab_icon(&screen, id).is_some());

    shell.handle(Command::Engine(EngineEvent::LoadingChanged {
        id,
        loading: false,
    }));
    report(
        &mut shell,
        id,
        page,
        true,
        vec![png("https://win.example/i.png", 32)],
    );
    assert_eq!(
        pages.lock().unwrap().len(),
        1,
        "the declared address is only a fallback"
    );
    assert_eq!(calls_matching(&engine, "rasterize"), 0);
}

#[test]
fn declared_addresses_back_up_an_engine_that_named_no_icon() {
    let (mut shell, engine, screen) = setup();
    engine
        .native_favicons
        .store(true, std::sync::atomic::Ordering::Release);
    shell.handle(Command::Bootstrap);
    let (_, pages) = attach_probers(&mut shell);
    let id = active_id(&screen);
    let page = "https://win.example/y";
    navigate_and_commit(&mut shell, id, page);
    report(
        &mut shell,
        id,
        page,
        false,
        vec![png("https://win.example/i.png", 32)],
    );
    assert!(pages.lock().unwrap().is_empty());

    shell.handle(Command::Engine(EngineEvent::LoadingChanged {
        id,
        loading: false,
    }));
    assert_eq!(addresses(&pages, 0)[0], "https://win.example/i.png");
}

#[test]
fn an_engine_icon_for_another_page_or_without_a_prober_is_dropped() {
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    let page = "https://win.example/z";
    navigate_and_commit(&mut shell, id, page);
    shell.handle(Command::Engine(EngineEvent::FaviconImage {
        id,
        page_url: page.into(),
        png: vec![1],
    }));
    let (_, pages) = attach_probers(&mut shell);
    shell.handle(Command::Engine(EngineEvent::FaviconImage {
        id,
        page_url: "https://other.example/".into(),
        png: vec![1],
    }));
    shell.handle(Command::Engine(EngineEvent::FaviconImage {
        id,
        page_url: page.into(),
        png: vec![0; 256 * 1024 + 1],
    }));
    shell.handle(Command::Engine(EngineEvent::FaviconImage {
        id,
        page_url: page.into(),
        png: Vec::new(),
    }));
    assert!(pages.lock().unwrap().is_empty());
}

#[test]
fn an_answer_for_a_page_the_tab_left_is_dropped_and_taken_once() {
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let (_, pages) = attach_probers(&mut shell);
    let id = active_id(&screen);
    navigate_and_commit(&mut shell, id, "https://old.example/a");
    report(
        &mut shell,
        id,
        "https://old.example/a",
        true,
        vec![png("https://old.example/i.png", 32)],
    );
    let ticket = pages.lock().unwrap()[0].0;
    navigate_and_commit(&mut shell, id, "https://new.example/b");

    answer(
        &mut shell,
        ticket,
        Some(raster(9)),
        "https://old.example/i.png",
        &[],
    );
    assert!(tab_icon(&screen, id).is_none());
    assert!(!shell
        .favicons
        .icon_values
        .keys()
        .any(|(_, key)| key.contains("old.example")));
    // An answer is taken once.
    answer(
        &mut shell,
        ticket,
        Some(raster(9)),
        "https://old.example/i.png",
        &[],
    );
    assert!(shell.favicons.icon_values.is_empty());
}

#[test]
fn a_tab_keeps_its_icon_while_another_page_of_the_same_site_loads() {
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    open_page(
        &mut shell,
        id,
        "https://mail.google.com/mail/u/0/",
        &raster(1),
    );
    let mail = tab_icon(&screen, id).unwrap();

    // docs.google.com has nothing yet: the tab keeps the mark it has rather
    // than flashing the globe, and switches when the new page's is known.
    navigate_and_commit(&mut shell, id, "https://docs.google.com/document/d/1");
    shell.handle(Command::Engine(EngineEvent::LoadingChanged {
        id,
        loading: true,
    }));
    assert_eq!(tab_icon(&screen, id), Some(mail.clone()));

    shell.handle(Command::Engine(EngineEvent::FaviconPixels {
        id,
        page_url: "https://docs.google.com/document/d/1".into(),
        rgba: raster(2),
    }));
    let docs = tab_icon(&screen, id).unwrap();
    assert_ne!(docs.revision, mail.revision);
    assert_eq!(docs.origin, "https://docs.google.com");
}

#[test]
fn a_tab_moving_to_another_site_shows_the_loading_state_not_the_old_mark() {
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    open_page(
        &mut shell,
        id,
        "https://mail.google.com/mail/u/0/",
        &raster(1),
    );
    assert!(tab_icon(&screen, id).is_some());

    navigate_and_commit(&mut shell, id, "https://example.org/");
    shell.handle(Command::Engine(EngineEvent::LoadingChanged {
        id,
        loading: true,
    }));
    assert!(tab_icon(&screen, id).is_none());
    let tab = last(&screen)
        .tabs
        .into_iter()
        .find(|tab| tab.id == id.to_string())
        .unwrap();
    assert!(tab.loading, "the row says it is loading");

    // And coming back to the first site does not resurrect it either.
    navigate_and_commit(&mut shell, id, "https://docs.google.com/");
    assert!(tab_icon(&screen, id).is_none());
}

#[test]
fn the_held_icon_is_let_go_when_the_wait_runs_out_or_the_page_settles_without_one() {
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let (_, pages) = attach_probers(&mut shell);
    let id = active_id(&screen);
    open_page(
        &mut shell,
        id,
        "https://mail.google.com/mail/u/0/",
        &raster(1),
    );
    let page = "https://docs.google.com/document/d/1";
    navigate_and_commit(&mut shell, id, page);
    assert!(tab_icon(&screen, id).is_some());

    shell.handle(Command::FaviconPoll {
        id,
        attempt: crate::shell::favicons::HOLD_EXPIRED,
    });
    assert!(
        tab_icon(&screen, id).is_none(),
        "a page that never delivers stops wearing the last one"
    );

    // The same when the page reports it has no icon at all.
    shell.favicons.page_checks.clear();
    open_page(
        &mut shell,
        id,
        "https://mail.google.com/mail/u/1/",
        &raster(1),
    );
    navigate_and_commit(&mut shell, id, page);
    assert!(tab_icon(&screen, id).is_some());
    report(&mut shell, id, page, true, Vec::new());
    let ticket = pages.lock().unwrap()[0].0;
    answer(
        &mut shell,
        ticket,
        None,
        "",
        &["https://docs.google.com/favicon.ico"],
    );
    shell.handle(Command::Engine(EngineEvent::FaviconRasterFailed {
        id,
        page_url: page.into(),
        url: "https://docs.google.com/favicon.ico".into(),
    }));
    assert!(tab_icon(&screen, id).is_none());
}

#[test]
fn only_the_apex_and_www_of_one_host_stand_in_for_each_other() {
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let id = active_id(&screen);
    open_page(&mut shell, id, "https://example.com/", &raster(3));
    let profile = shell.profile_of_item(id).unwrap();
    let wears = |shell: &Shell, url: &str| {
        shell
            .icon_ref_for_url(zephium_ipc::IconSurface::Chrome, profile, url)
            .is_some()
    };
    assert!(wears(&shell, "https://example.com/other"));
    assert!(!wears(&shell, "https://www.example.com/"));

    // The tab itself, and not a lookup by address, applies the pair.
    shell.favicons.page_checks.clear();
    navigate_and_commit(&mut shell, id, "https://www.example.com/");
    assert_eq!(
        tab_icon(&screen, id).map(|icon| icon.origin),
        Some("https://example.com".to_owned())
    );

    // Every other subdomain wears its own mark or none, never a sibling's.
    open_page(&mut shell, id, "https://mail.google.com/", &raster(4));
    shell.favicons.page_checks.clear();
    for host in ["gemini.google.com", "www.google.com", "google.com"] {
        navigate_and_commit(&mut shell, id, &format!("https://{host}/"));
        shell.handle(Command::FaviconPoll {
            id,
            attempt: crate::shell::favicons::HOLD_EXPIRED,
        });
        assert!(
            tab_icon(&screen, id).is_none(),
            "{host} borrowed a sibling's icon"
        );
        shell.favicons.page_checks.clear();
    }
}

#[test]
fn many_sites_that_gave_nothing_never_keep_a_new_one_from_being_tried() {
    let (mut shell, engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let profile = shell.windows.focused().unwrap().profile;
    let now = std::time::Instant::now();
    for index in 0..5000 {
        let address = format!("https://gone-{index}.example/favicon.ico");
        shell
            .favicons
            .failed
            .insert((profile, address.clone()), now);
        shell.favicons.unfetched.insert((profile, address), now);
        shell
            .favicons
            .icons_checked
            .insert((profile, format!("https://checked-{index}.example")));
    }
    let id = active_id(&screen);
    navigate_and_commit(&mut shell, id, "https://brand-new.example/");
    assert_eq!(calls_matching(&engine, &format!("discover {id}")), 1);
}

#[test]
fn failed_addresses_expire_and_the_oldest_give_way_first() {
    use crate::shell::favicons::FailedIcons;
    let profile = ProfileId::from(1);
    let other = ProfileId::from(2);
    let start = std::time::Instant::now();
    let after = |seconds: u64| start + std::time::Duration::from_secs(seconds);
    let key = |name: &str| (profile, name.to_owned());
    let mut failed = FailedIcons::with_limits(3, std::time::Duration::from_secs(100));

    failed.insert(key("a"), after(0));
    failed.insert(key("b"), after(1));
    failed.insert(key("c"), after(2));
    assert!(failed.contains(&key("a"), after(50)));
    assert!(
        !failed.contains(&key("a"), after(100)),
        "an entry lasts its ttl"
    );
    assert!(failed.contains(&key("b"), after(100)));

    // The oldest goes, never the newest.
    failed.insert(key("d"), after(3));
    assert_eq!(failed.len(), 3);
    assert!(!failed.contains(&key("a"), after(3)));
    assert!(failed.contains(&key("d"), after(3)));

    // Failing again moves an entry to the back and renews it.
    failed.insert(key("b"), after(4));
    failed.insert(key("e"), after(5));
    assert!(failed.contains(&key("b"), after(5)));
    assert!(!failed.contains(&key("c"), after(5)));

    // Expired entries make room without evicting live ones.
    failed.insert(key("f"), after(150));
    assert!(failed.contains(&key("f"), after(150)));
    assert_eq!(failed.len(), 1);

    failed.insert((other, "x".to_owned()), after(150));
    failed.forget_profile(other);
    assert!(!failed.contains(&(other, "x".to_owned()), after(150)));
    assert!(failed.contains(&key("f"), after(150)));
}
