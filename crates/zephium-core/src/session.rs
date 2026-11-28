t_active_ms,
                custom_icon: None,
            },
        };
        let state = SessionState {
            profiles: vec![PersistedProfile {
                id: profile,
                name: "P".into(),
                kind: ProfileKind::Default,
            }],
            spaces: vec![PersistedSpace {
                id: space,
                profile,
                name: "S".into(),
                look: Default::default(),
            }],
            items: vec![tab(10, Some(1_700_000_000_000)), tab(11, None)],
            ..SessionState::default()
        };
        let restored = restore(state.clone());
        assert_eq!(
            restored.items.tab(ItemId::from(10)).unwrap().last_active_ms,
            Some(1_700_000_000_000)
        );
        assert_eq!(canonicalize(state.clone()), state);
        let json = serde_json::to_string(&state.items[1]).unwrap();
        assert!(!json.contains("last_active_ms"));
        let legacy: PersistedItem = serde_json::from_str(&json).unwrap();
        assert_eq!(legacy, state.items[1]);

        let mut hostile = state;
        hostile.items = vec![tab(12, Some(MAX_EXACT_MS + 1)), tab(13, Some(0))];
        let restored = restore(hostile);
        for id in [12, 13] {
            assert_eq!(
                restored.items.tab(ItemId::from(id)).unwrap().last_active_ms,
                None
            );
        }
    }

    #[test]
    fn an_essential_keeps_its_valid_mark_and_a_tab_cannot_carry_one() {
        use crate::spaces::SpaceIcon;
        let profile = ProfileId::from(1);
        let space = SpaceId::from(2);
        let item = |id: u128, placement: Placement, icon: Option<SpaceIcon>| PersistedItem {
            id: ItemId::from(id),
            parent: None,
            placement,
            kind: PersistedKind::Tab {
                url: "https://example.com/".into(),
                title: "E".into(),
                zoom: 1.0,
                last_active_ms: None,
                custom_icon: icon,
            },
        };
        let state = SessionState {
            profiles: vec![PersistedProfile {
                id: profile,
                name: "P".into(),
                kind: ProfileKind::Default,
            }],
            spaces: vec![PersistedSpace {
                id: space,
                profile,
                name: "S".into(),
                look: Default::default(),
            }],
            items: vec![
                item(
                    10,
                    Placement::Favorites { space },
                    Some(SpaceIcon::Emoji("🚀".into())),
                ),
                item(11, today(space), Some(SpaceIcon::Symbol(2))),
                item(
                    12,
                    Placement::Favorites { space },
                    Some(SpaceIcon::Symbol(200)),
                ),
            ],
            ..SessionState::default()
        };
        let restored = restore(state);
        assert_eq!(
            restored.items.tab(ItemId::from(10)).unwrap().custom_icon,
            Some(SpaceIcon::Emoji("🚀".into()))
        );
        assert_eq!(
            restored.items.tab(ItemId::from(11)).unwrap().custom_icon,
            None
        );
        assert_eq!(
            restored.items.tab(ItemId::from(12)).unwrap().custom_icon,
            None
        );
    }

    #[test]
    fn restore_drops_dangling_references() {
        let state = SessionState {
            profiles: vec![],
            spaces: vec![PersistedSpace {
                id: SpaceId::from(2),
                profile: ProfileId::from(1),
                name: "Orphan".into(),
                look: Default::default(),
            }],
            items: vec![PersistedItem {
                id: ItemId::from(10),
                parent: None,
                placement: today(SpaceId::from(2)),
                kind: PersistedKind::Tab {
                    url: "https://example.com/".into(),
                    title: "E".into(),
                    zoom: 1.0,
                    last_active_ms: None,
                    custom_icon: None,
                },
            }],
            active_space: Some(SpaceId::from(2)),
            active_item: Some(ItemId::from(10)),
            splits: Some(Pane::leaf(ItemId::from(10))),
            recently_closed: Vec::new(),
        };
        let restored = restore(state);
        assert!(restored.spaces.is_empty());
        assert!(restored.items.get(ItemId::from(10)).is_none());
        assert_eq!(restored.active_item, None);
        assert_eq!(restored.splits, None);
    }

    #[test]
    fn canonicalize_filters_private_dangerous_and_hostile_display_data() {
        let profile = ProfileId::from(1);
        let private = ProfileId::from(2);
        let space = SpaceId::from(3);
        let private_space = SpaceId::from(4);
        let kept = ItemId::from(10);
        let dangerous = ItemId::from(11);
        let private_tab = ItemId::from(12);
        let state = SessionState {
            profiles: vec![
                PersistedProfile {
                    id: profile,
                    name: format!("\u{202e}Personal\0{}", "x".repeat(300)),
                    kind: ProfileKind::Default,
                },
                PersistedProfile {
                    id: private,
                    name: "Private".into(),
                    kind: ProfileKind::Incognito,
                },
            ],
            spaces: vec![
                PersistedSpace {
                    id: space,
                    profile,
                    name: "\u{2066}Work\n".into(),
                    look: Default::default(),
                },
                PersistedSpace {
                    id: private_space,
                    profile: private,
                    name: "Secret".into(),
                    look: Default::default(),
                },
            ],
            items: vec![
                PersistedItem {
                    id: kept,
                    parent: None,
                    placement: today(space),
                    kind: PersistedKind::Tab {
                        url: "https://example.com/".into(),
                        title: format!("\u{202e}Example\0{}", "y".repeat(600)),
                        zoom: f64::INFINITY,
                        last_active_ms: None,
                        custom_icon: None,
                    },
                },
                PersistedItem {
                    id: dangerous,
                    parent: None,
                    placement: today(space),
                    kind: PersistedKind::Tab {
                        url: "file:///etc/passwd".into(),
                        title: "Local file".into(),
                        zoom: 1.0,
                        last_active_ms: None,
                        custom_icon: None,
                    },
                },
                PersistedItem {
                    id: private_tab,
                    parent: None,
                    placement: today(private_space),
                    kind: PersistedKind::Tab {
                        url: "https://private.example/".into(),
                        title: "Private".into(),
                        zoom: 1.0,
                        last_active_ms: None,
                        custom_icon: None,
                    },
                },
            ],
            active_space: Some(private_space),
            active_item: Some(dangerous),
            splits: Some(Pane::Branch {
                axis: crate::split::Axis::Row,
                ratio: 0.5,
                a: Box::new(Pane::Leaf(kept)),
                b: Box::new(Pane::Leaf(kept)),
            }),
            recently_closed: Vec::new(),
        };

        let clean = canonicalize(state);
        assert_eq!(clean.profiles.len(), 1);
        assert_eq!(clean.spaces.len(), 1);
        assert_eq!(clean.items.len(), 1);
        assert_eq!(
            clean.profiles[0].name.chars().count(),
            MAX_SESSION_NAME_CHARS
        );
        assert!(clean.profiles[0]
            .name
            .chars()
            .all(|c| !c.is_control() && c != '\u{202e}'));
        assert_eq!(clean.spaces[0].name, "Work");
        let PersistedKind::Tab { title, zoom, .. } = &clean.items[0].kind else {
            panic!("kept item changed kind")
        };
        assert_eq!(title.chars().count(), crate::item::MAX_PAGE_TITLE_CHARS);
        assert!(title.chars().all(|c| !c.is_control() && c != '\u{202e}'));
        assert_eq!(*zoom, 1.0);
        assert_eq!(clean.active_space, None);
        assert_eq!(clean.active_item, None);
        assert_eq!(
            clean.splits, None,
            "duplicate split leaves must be rejected"
        );
    }

    #[test]
    fn focus_and_splits_cannot_cross_the_active_space_or_profile() {
        let (mut profiles, mut spaces, mut items, profile, active_space) = seed();
        let sibling_space = SpaceId::from(3);
        spaces.insert(Space {
            id: sibling_space,
            profile,
            name: "Sibling".into(),
            look: Default::default(),
        });
        let foreign_profile = ProfileId::from(4);
        let foreign_space = SpaceId::from(5);
        profiles.insert(Profile {
            id: foreign_profile,
            name: "Foreign".into(),
            kind: ProfileKind::Named,
        });
        spaces.insert(Space {
            id: foreign_space,
            profile: foreign_profile,
            name: "Foreign".into(),
            look: Default::default(),
        });

        let local = ItemId::from(10);
        let sibling = ItemId::from(11);
        let foreign = ItemId::from(12);
        let favorite = ItemId::from(13);
        let foreign_favorite = ItemId::from(14);
        let sibling_favorite = ItemId::from(15);
        for (id, placement, url) in [
            (local, today(active_space), "local.example"),
            (sibling, today(sibling_space), "sibling.example"),
            (foreign, today(foreign_space), "foreign.example"),
            (
                favorite,
                Placement::Favorites {
                    space: active_space,
                },
                "favorite.example",
            ),
            (
                sibling_favorite,
                Placement::Favorites {
                    space: sibling_space,
                },
                "sibling-favorite.example",
            ),
            (
                foreign_favorite,
                Placement::Favorites {
                    space: foreign_space,
                },
                "foreign-favorite.example",
            ),
        ] {
            assert!(items.insert_tab(id, placement));
            navigate_and_commit(&mut items, id, url);
        }

        // A space's own Essentials are in its scope, a sibling space's are not.
        let favorite_state = snapshot(
            &profiles,
            &spaces,
            &items,
            Some(active_space),
            Some(favorite),
            Some(&Pane::Leaf(favorite)),
        );
        assert_eq!(favorite_state.active_item, Some(favorite));
        assert_eq!(favorite_state.splits, Some(Pane::Leaf(favorite)));

        for attacker in [sibling, foreign, sibling_favorite, foreign_favorite] {
            let state = snapshot(
                &profiles,
                &spaces,
                &items,
                Some(active_space),
                Some(attacker),
                Some(&Pane::Branch {
                    axis: crate::split::Axis::Row,
                    ratio: 0.5,
                    a: Box::new(Pane::Leaf(local)),
                    b: Box::new(Pane::Leaf(attacker)),
                }),
            );
            assert_eq!(state.active_item, None);
            assert_eq!(state.splits, None);

            // Restore must independently reject equivalent forged store
            // references; it cannot rely on snapshots being the producer.
            let mut forged = snapshot(
                &profiles,
                &spaces,
                &items,
                Some(active_space),
                Some(local),
                None,
            );
            forged.active_item = Some(attacker);
            forged.splits = Some(Pane::Branch {
                axis: crate::split::Axis::Row,
                ratio: 0.5,
                a: Box::new(Pane::Leaf(local)),
                b: Box::new(Pane::Leaf(attacker)),
            });
            let restored = restore(forged);
            assert_eq!(restored.active_space, Some(active_space));
            assert_eq!(restored.active_item, None);
            assert_eq!(restored.splits, None);
        }
    }

    #[test]
    fn restore_rejects_out_of_range_split_geometry() {
        let (profiles, spaces, mut items, _profile, space) = seed();
        let (a, b) = (ItemId::from(10), ItemId::from(11));
        items.insert_tab(a, today(space));
        items.insert_tab(b, today(space));
        navigate_and_commit(&mut items, a, "example.com");
        navigate_and_commit(&mut items, b, "example.org");
        let mut state = snapshot(&profiles, &spaces, &items, Some(space), Some(a), None);
        state.splits = Some(Pane::Branch {
            axis: crate::split::Axis::Col,
            ratio: -10.0,
            a: Box::new(Pane::Leaf(a)),
            b: Box::new(Pane::Leaf(b)),
        });

        assert_eq!(restore(state).splits, None);
    }

    #[test]
    fn spaces_keep_order_and_look_and_a_damaged_look_is_sanitized() {
        use crate::spaces::{SpaceIcon, SpaceTheme};
        let (profiles, mut spaces, items, profile, space) = seed();
        let look = SpaceLook {
            icon: Some(SpaceIcon::Symbol(4)),
            color: 6,
            theme: Some(SpaceTheme::Preset("dracula".into())),
        };
        assert!(spaces.create(SpaceId::from(3), profile, "Night", look.clone()));
        assert!(spaces.reorder(SpaceId::from(3), Some(space)));
        let mut state = snapshot(
            &profiles,
            &spaces,
            &items,
            Some(SpaceId::from(3)),
            None,
            None,
        );
        assert_eq!(canonicalize(state.clone()), state);
        let restored = restore(state.clone());
        let order: Vec<_> = restored
            .spaces
            .iter()
            .map(|s| (s.id, s.look.clone()))
            .collect();
        assert_eq!(
            order,
            vec![(SpaceId::from(3), look), (space, SpaceLook::default())]
        );
        assert_eq!(restored.active_space, Some(SpaceId::from(3)));

        state.spaces[0].look.color = 200;
        state.spaces[0].look.theme = Some(SpaceTheme::Custom("{}".into()));
        let restored = restore(state);
        let look = &restored.spaces.get(SpaceId::from(3)).unwrap().look;
        assert_eq!((look.color, &look.theme), (0, &None));
        assert_eq!(look.icon, Some(SpaceIcon::Symbol(4)));
    }
}
