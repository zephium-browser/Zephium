),
        }));
        assert_eq!(items.items.len(), crate::session::MAX_ITEM_TREE_DEPTH + 1);
    }

    #[test]
    fn a_restored_page_that_fails_to_load_keeps_its_address_and_reason() {
        use crate::ports::engine::NavigationFailureReason as Reason;
        let mut items = Items::default();
        let url = Url::parse("http://localhost:5173/app").unwrap();
        assert!(items.insert_unloaded_tab(id(1), today(), url.clone(), "Dev app"));
        assert_eq!(items.ensure_view(id(1)).len(), 1);

        assert!(items.record_navigation_failure(id(1), Reason::Unreachable, None, false));
        items.view_creation_failed(id(1));

        let tab = items.tab(id(1)).unwrap();
        assert!(!tab.has_view());
        assert_eq!(tab.title, "Dev app");
        assert_eq!(tab.url.as_ref(), Some(&url));
        let failure = tab.failure.as_deref().unwrap();
        assert_eq!((&failure.url, failure.reason), (&url, Reason::Unreachable));
    }

    #[test]
    fn a_page_navigation_after_commit_is_not_a_tab_failure() {
        use crate::ports::engine::NavigationFailureReason as Reason;
        let mut items = Items::default();
        assert!(items.insert_tab(id(1), today()));
        items.navigate(id(1), "example.com");
        assert!(items.set_committed_url_str(id(1), "https://example.com/"));

        assert!(!items.record_navigation_failure(id(1), Reason::Offline, None, false));
        assert!(items.tab(id(1)).unwrap().failure.is_none());
    }

    #[test]
    fn a_replaced_page_is_covered_by_its_failure_until_a_document_commits() {
        use crate::ports::engine::NavigationFailureReason as Reason;
        let mut items = Items::default();
        assert!(items.insert_tab(id(1), today()));
        items.navigate(id(1), "example.com");
        assert!(items.set_committed_url_str(id(1), "https://example.com/"));
        let failed = Url::parse("https://example.com/down").unwrap();

        // A link the page followed: no address was asked for, yet the page
        // it left is gone.
        assert!(items.record_navigation_failure(
            id(1),
            Reason::Unreachable,
            Some(failed.clone()),
            true
        ));
        let tab = items.tab(id(1)).unwrap();
        let failure = tab.failure.as_deref().unwrap();
        assert_eq!((&failure.url, failure.covers_page), (&failed, true));
        assert!(tab.has_view() && !tab.shows_page());
        assert_eq!(tab.url.as_ref().unwrap().as_str(), "https://example.com/");

        // Retrying keeps the error document covered while it loads.
        assert_eq!(items.navigate(id(1), failed.as_str()).len(), 1);
        assert!(!items.tab(id(1)).unwrap().shows_page());

        assert!(items.set_committed_url(id(1), failed.clone()));
        let tab = items.tab(id(1)).unwrap();
        assert!(tab.failure.is_none() && tab.shows_page());
    }

    #[test]
    fn a_covering_failure_needs_a_view_and_goes_with_it() {
        use crate::ports::engine::NavigationFailureReason as Reason;
        let failed = Url::parse("https://example.com/down").unwrap();
        let mut items = Items::default();
        assert!(items.insert_tab(id(1), today()));
        assert!(!items.record_navigation_failure(
            id(1),
            Reason::Offline,
            Some(failed.clone()),
            true
        ));

        items.navigate(id(1), "example.com");
        assert!(items.set_committed_url_str(id(1), "https://example.com/"));
        assert!(items.record_navigation_failure(id(1), Reason::Offline, Some(failed), true));
        assert!(items.mark_view_discarded(id(1)));
        let tab = items.tab(id(1)).unwrap();
        assert!(
            tab.failure.is_none(),
            "the error document went with the view"
        );
        assert!(!tab.shows_page());
    }

    #[test]
    fn navigate_creates_view_then_navigates() {
        let mut items = Items::default();
        assert!(items.insert_tab(id(1), today()));
        assert!(!items.tab(id(1)).unwrap().has_view());

        let fx = items.navigate(id(1), "example.com");
        assert_eq!(
            fx,
            vec![Effect::CreateView {
                id: id(1),
                url: "https://example.com/".into()
            }]
        );
        assert!(items.tab(id(1)).unwrap().has_view());
        assert!(items.tab(id(1)).unwrap().url.is_none());
        assert!(!items.tab(id(1)).unwrap().loading);
        assert_eq!(
            items.pending_navigation(id(1)),
            Some(NavigationRequestId(1))
        );

        items.set_committed_url_str(id(1), "https://example.com/");
        assert_eq!(
            items.tab(id(1)).unwrap().url.as_ref().map(Url::as_str),
            Some("https://example.com/")
        );
        assert_eq!(items.pending_navigation(id(1)), None);

        let fx = items.navigate(id(1), "github.com");
        assert_eq!(
            fx,
            vec![Effect::Navigate {
                id: id(1),
                url: "https://github.com/".into(),
                request: NavigationRequestId(2),
            }]
        );
        // An intent is never exposed as the page currently displayed.
        assert_eq!(
            items.tab(id(1)).unwrap().url.as_ref().map(Url::as_str),
            Some("https://example.com/")
        );
        assert_eq!(
            items.pending_navigation(id(1)),
            Some(NavigationRequestId(2))
        );
    }

    #[test]
    fn an_unloaded_tab_knows_its_page_without_a_view() {
        let mut items = Items::default();
        let kept = Placement::Favorites {
            space: SpaceId::from(1),
        };
        let url = Url::parse("https://app.slack.com/client").unwrap();
        assert!(items.insert_unloaded_tab(id(1), kept, url, "Slack\u{202e}"));
        let tab = items.tab(id(1)).unwrap();
        assert!(!tab.has_view());
        assert_eq!(tab.title, "Slack");
        assert_eq!(
            tab.url.as_ref().map(Url::as_str),
            Some("https://app.slack.com/client")
        );
        assert_eq!(items.roots(kept), &[id(1)]);
        assert!(matches!(
            items.ensure_view(id(1)).as_slice(),
            [Effect::CreateView { url, .. }] if url == "https://app.slack.com/client"
        ));

        let script = Url::parse("javascript:alert(1)").unwrap();
        assert!(!items.insert_unloaded_tab(id(2), kept, script, "x"));
        assert!(items.tab(id(2)).is_none());
    }

    #[test]
    fn acknowledged_native_discard_preserves_metadata_without_a_second_close() {
        let mut items = Items::default();
        assert!(items.insert_tab(id(1), today()));
        assert!(matches!(
            items.navigate(id(1), "https://example.com/path").as_slice(),
            [Effect::CreateView { .. }]
        ));
        assert!(items.set_committed_url_str(id(1), "https://example.com/path"));
        items.set_title(id(1), "Kept title".into());

        assert!(items.mark_view_discarded(id(1)));
        let tab = items.tab(id(1)).unwrap();
        assert!(!tab.has_view());
        assert_eq!(tab.lifecycle, Lifecycle::Hibernated);
        assert_eq!(tab.title, "Kept title");
        assert_eq!(
            tab.url.as_ref().map(Url::as_str),
            Some("https://example.com/path")
        );
        assert!(!items.mark_view_discarded(id(1)));
        assert!(matches!(
            items.ensure_view(id(1)).as_slice(),
            [Effect::CreateView { url, .. }] if url == "https://example.com/path"
        ));
    }

    #[test]
    fn only_current_navigation_failure_clears_pending_intent() {
        let mut items = Items::default();
        assert!(items.insert_tab(id(1), today()));
        items.navigate(id(1), "first.example");
        let fx = items.navigate(id(1), "second.example");
        let Effect::Navigate { request, .. } = fx[0] else {
            panic!("existing view navigation expected");
        };

        assert!(!items.navigation_failed(id(1), NavigationRequestId(1)));
        assert_eq!(items.pending_navigation(id(1)), Some(request));
        assert!(items.navigation_failed(id(1), request));
        assert_eq!(items.pending_navigation(id(1)), None);
        assert!(items.tab(id(1)).unwrap().url.is_none());
    }

    #[test]
    fn a_refused_first_page_s_blank_stand_in_is_expected_and_never_commits() {
        let mut items = Items::default();
        assert!(items.insert_tab(id(1), today()));
        let blank = Url::parse("about:blank").unwrap();
        // Before any refusal a blank page is an ordinary commit.
        assert!(!items.is_refused_stand_in(id(1), &blank));
        assert!(items.record_navigation_failure(
            id(1),
            crate::ports::engine::NavigationFailureReason::Blocked(
                crate::ports::engine::NavigationBlockReason::Badware,
            ),
            Some(Url::parse("https://scam.example/").unwrap()),
            true,
        ));
        assert!(items.is_refused_stand_in(id(1), &blank));
        assert!(!items.set_committed_url(id(1), blank.clone()));
        assert!(items.tab(id(1)).unwrap().failure.is_some());
        // A blank page the person asks for themselves commits over it.
        items.navigate(id(1), "about:blank");
        assert!(!items.is_refused_stand_in(id(1), &blank));
        assert!(items.set_committed_url(id(1), blank));
        assert!(items.record_navigation_failure(
            id(1),
            crate::ports::engine::NavigationFailureReason::Blocked(
                crate::ports::engine::NavigationBlockReason::Badware,
            ),
            Some(Url::parse("https://scam.example/").unwrap()),
            true,
        ));
        // A real page the person then opens commits as usual.
        let page = Url::parse("https://news.example/").unwrap();
        assert!(!items.is_refused_stand_in(id(1), &page));
        assert!(items.set_committed_url(id(1), page));
    }

    #[test]
    fn insert_refuses_duplicate_and_bad_parent() {
        let mut items = Items::default();
        assert!(items.insert_tab(id(1), today()));
        assert!(!items.insert_tab(id(1), today()));

        // parent must exist and be a folder
        assert!(!items.insert(Item {
            id: id(2),
            parent: Some(id(1)),
            placement: today(),
            kind: ItemKind::Tab(TabState::new()),
        }));
        assert!(!items.insert(Item {
            id: id(3),
            parent: Some(id(99)),
            placement: today(),
            kind: ItemKind::Tab(TabState::new()),
        }));
    }

    #[test]
    fn remove_folder_removes_subtree_and_closes_views() {
        let mut items = Items::default();
        let f = folder(&mut items, 10);
        assert!(items.insert(Item {
            id: id(11),
            parent: Some(f),
            placement: today(),
            kind: ItemKind::Tab(TabState::new()),
        }));
        items.navigate(id(11), "example.com");
        assert!(items.insert_tab(id(12), today()));

        let fx = items.remove(f);
        assert_eq!(fx, vec![Effect::Close { id: id(11) }]);
        assert!(items.get(f).is_none());
        assert!(items.get(id(11)).is_none());
        assert_eq!(items.roots(today()), &[id(12)]);
    }

    fn check_invariants(items: &Items) {
        let mut seen = std::collections::HashSet::new();
        for (placement, list) in &items.roots {
            for rid in list {
                assert!(seen.insert(*rid), "id listed twice");
                let item = items.get(*rid).expect("root exists");
                assert_eq!(item.parent, None);
                assert_eq!(item.placement, *placement);
            }
        }
        for (parent, list) in &items.children {
            let p = items.get(*parent).expect("parent exists");
            assert!(matches!(p.kind, ItemKind::Folder { .. }));
            for cid in list {
                assert!(seen.insert(*cid), "id listed twice");
                let c = items.get(*cid).expect("child exists");
                assert_eq!(c.parent, Some(*parent));
                assert_eq!(c.placement, p.placement);
            }
        }
        assert_eq!(
            seen.len(),
            items.items.len(),
            "every item is listed exactly once"
        );
    }

    #[derive(Clone, Debug)]
    enum Op {
        InsertTab(u128),
        InsertFolder(u128),
        InsertChild(u128, usize),
        Remove(usize),
        Navigate(usize, String),
    }

    fn ops() -> impl Strategy<Value = Op> {
        prop_oneof![
            (1u128..200).prop_map(Op::InsertTab),
            (1u128..200).prop_map(Op::InsertFolder),
            ((1u128..200), any::<usize>()).prop_map(|(n, i)| Op::InsertChild(n, i)),
            any::<usize>().prop_map(Op::Remove),
            (any::<usize>(), "\\PC*").prop_map(|(i, s)| Op::Navigate(i, s)),
        ]
    }

    proptest! {
        #[test]
        fn invariants_hold_under_random_ops(seq in proptest::collection::vec(ops(), 0..60)) {
            let mut items = Items::default();
            let mut ids: Vec<ItemId> = Vec::new();
            let pick = |ids: &Vec<ItemId>, i: usize| ids.get(i % ids.len().max(1)).copied();
            for op in seq {
                match op {
                    Op::InsertTab(n) => {
                        if items.insert_tab(id(n), today()) {
                            ids.push(id(n));
                        }
                    }
                    Op::InsertFolder(n) => {
                        let item = Item {
                            id: id(n),
                            parent: None,
                            placement: today(),
                            kind: ItemKind::Folder { name: "F".into() },
                        };
                        if items.insert(item) {
                            ids.push(id(n));
                        }
                    }
                    Op::InsertChild(n, i) => {
                        if let Some(parent) = pick(&ids, i) {
                            let item = Item {
                                id: id(n),
                                parent: Some(parent),
                                placement: today(),
                                kind: ItemKind::Tab(TabState::new()),
                            };
                            if items.insert(item) {
                                ids.push(id(n));
                            }
                        }
                    }
                    Op::Remove(i) => {
                        if let Some(target) = pick(&ids, i) {
                            items.remove(target);
                            ids.retain(|x| items.get(*x).is_some());
                        }
                    }
                    Op::Navigate(i, s) => {
                        if let Some(target) = pick(&ids, i) {
                            items.navigate(target, &s);
                        }
                    }
                }
                check_invariants(&items);
            }
        }
    }
}
