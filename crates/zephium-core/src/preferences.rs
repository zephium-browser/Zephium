ca_alt", "mica_alt "] {
            assert!(!value_allowed("ui.material", value));
        }
        assert!(!value_allowed("ui.material", "sky"));
        assert!(value_allowed("ui.newtab-clock-format", "24h"));
        assert!(value_allowed("ui.newtab-tasks", "false"));
        assert!(value_allowed("ui.language", "en"));
        assert!(value_allowed("tabs.after-close", "recent"));
        assert!(value_allowed("tabs.startup", "new-tab"));
        assert!(value_allowed("tabs.startup", "pages"));
        assert!(!value_allowed("tabs.startup", "pagez"));
        assert!(value_allowed("ui.text-size", "large"));
        assert!(value_allowed("ui.tab-layout", "horizontal"));
        assert!(value_allowed("onboarding", "done"));
        for key in ["ai.enabled", "work.enabled", "work.screenshots"] {
            assert!(KEYS.contains(&key));
            assert!(value_allowed(key, "true"));
            assert!(value_allowed(key, "false"));
            for value in ["", "0", "1", "TRUE", " false", "false "] {
                assert!(!value_allowed(key, value));
            }
        }
        for (key, value) in [
            ("ui.custom-css", "body{}"),
            ("ui.accent", "url(evil)"),
            ("ui.newtab-style", "clock"),
            ("ui.newtab-clock-format", "12-hour"),
            ("ui.newtab-greeting", "yes"),
            ("ui.newtab-logo", "true"),
            ("appearance", "sepia"),
            ("ui.language", "klingon"),
            ("ui.density", "tight"),
            ("tabs.new-position", "start"),
            ("tabs.switch-to-open", "maybe"),
            ("ui.tab-layout", "diagonal"),
            ("onboarding", "skipped"),
        ] {
            assert!(!value_allowed(key, value));
        }
    }

    #[test]
    fn favourite_models_are_one_bounded_id_per_line() {
        let key = "work.favourite-models";
        assert!(KEYS.contains(&key));
        for value in ["", "anthropic/claude-opus-5-5", "a/b\nc/d:e.f"] {
            assert!(value_allowed(key, value), "{value:?}");
        }
        let many = (0..65).map(|n| format!("m/{n}")).collect::<Vec<_>>();
        for value in [
            "a\n",
            "a\n\nb",
            "a b",
            "a\na",
            "é",
            &"x".repeat(161),
            &many.join("\n"),
        ] {
            assert!(!value_allowed(key, value), "{value:?}");
        }
        assert!(value_allowed(key, &many[..64].join("\n")));
    }

    #[test]
    fn customization_preferences_are_closed_and_bounded() {
        for key in [
            "ui.theme",
            "ui.theme-custom",
            "ui.window-tint",
            "ui.font",
            "ui.font-size",
            "ui.code-font",
            "ui.theme-contrast",
        ] {
            assert!(KEYS.contains(&key));
        }
        assert!(value_allowed("ui.theme", "nord"));
        assert!(value_allowed("ui.theme", "custom"));
        assert!(!value_allowed("ui.theme", "monokai"));
        assert!(value_allowed("ui.theme-custom", ""));
        assert!(value_allowed(
            "ui.theme-custom",
            r##"{"light":{"bg":"#ffffff","fg":"#000000","accent":"#3366ff"},"dark":{"bg":"#000000","fg":"#ffffff","accent":"#88aaff"}}"##
        ));
        assert!(!value_allowed("ui.theme-custom", "{}"));
        assert!(value_allowed("ui.theme-contrast", "72"));
        assert!(!value_allowed("ui.theme-contrast", "101"));
        for tint in ["0", "35", "100"] {
            assert!(value_allowed("ui.window-tint", tint));
        }
        for tint in ["", "-1", "101", "050", "5.5", " 5"] {
            assert!(!value_allowed("ui.window-tint", tint), "{tint:?}");
        }
        assert!(value_allowed("ui.font", "rounded"));
        assert!(!value_allowed("ui.font", "comic-sans"));
        assert!(value_allowed("ui.font-size", "15"));
        assert!(!value_allowed("ui.font-size", "16"));
        assert!(value_allowed("ui.code-font", "jetbrains-mono"));
        assert!(!value_allowed("ui.code-font", "\"Fira Code\", serif"));
    }

    #[test]
    fn tab_options_are_closed_choices() {
        for key in [
            "tabs.archive",
            "tabs.external-position",
            "tabs.close-button",
        ] {
            assert!(KEYS.contains(&key));
        }
        for value in ["never", "12h", "1d", "7d"] {
            assert!(value_allowed("tabs.archive", value));
        }
        for value in ["", "0", "30d", "12 hours", "Never"] {
            assert!(!value_allowed("tabs.archive", value), "{value:?}");
        }
        for value in ["new-tab", "end", "after-current", "mini"] {
            assert!(value_allowed("tabs.external-position", value));
        }
        assert!(!value_allowed("tabs.external-position", "window"));
        assert!(value_allowed("tabs.close-button", "always"));
        assert!(!value_allowed("tabs.close-button", "never"));
    }

    fn stored(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |key: &str| {
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| (*v).to_owned())
        }
    }

    fn carried(pairs: &'static [(&'static str, &'static str)]) -> Vec<(&'static str, String)> {
        let writes = migrations(stored(pairs));
        for (key, value) in &writes {
            assert!(
                KEYS.contains(key) && value_allowed(key, value),
                "{key}={value}"
            );
        }
        writes
    }

    #[test]
    fn saved_shortcuts_migrate_without_retired_entries_once() {
        let mut entries = crate::search::ShortcutRegistry::default()
            .entries()
            .to_vec();
        entries[0].enabled = false;
        let expected = serde_json::to_string(&entries).unwrap();
        let mut retired = entries[0].clone();
        retired.id = "npm".into();
        entries.push(retired);
        let raw = serde_json::to_string(&entries).unwrap();
        let writes = migrations(|key| (key == "search.shortcuts").then(|| raw.clone()));
        assert_eq!(writes, vec![("search.shortcuts", expected.clone())]);
        for value in [expected.as_str(), "", "not json", "null"] {
            assert!(migrations(|key| (key == "search.shortcuts").then(|| value.into())).is_empty());
        }
    }

    #[test]
    fn near_limit_shortcut_migration_remains_readable() {
        use crate::search::{ShortcutRegistry, MAX_SHORTCUT_SETTINGS_BYTES};
        let mut entries: Vec<_> = (0..8)
            .map(|index| {
                serde_json::json!({
                    "id": format!("custom:near-limit-{index}"),
                    "name": "Near limit",
                    "keywords": [format!("near{index}")],
                    "search_url": format!("https://example.com/{}?q={{query}}", "s".repeat(2018)),
                    "home_url": format!("https://example.com/{}", "h".repeat(2028)),
                    "domain": "example.com",
                    "enabled": false
                })
            })
            .collect();
        let excess =
            serde_json::to_string(&entries).unwrap().len() - (MAX_SHORTCUT_SETTINGS_BYTES - 1);
        let home = entries.last_mut().unwrap()["home_url"]
            .as_str()
            .unwrap()
            .to_owned();
        entries.last_mut().unwrap()["home_url"] = home[..home.len() - excess].into();
        let raw = serde_json::to_string(&entries).unwrap();
        assert_eq!(raw.len(), MAX_SHORTCUT_SETTINGS_BYTES - 1);
        let registry = ShortcutRegistry::from_setting(&raw).unwrap();
        let writes = migrations(|key| (key == "search.shortcuts").then(|| raw.clone()));
        let saved = writes.first().map_or(raw.as_str(), |(_, value)| value);
        assert!(saved.len() <= MAX_SHORTCUT_SETTINGS_BYTES);
        assert_eq!(
            ShortcutRegistry::from_setting(saved).unwrap().entries(),
            registry.entries()
        );
    }

    #[test]
    fn the_retired_text_size_carries_into_the_interface_size_once() {
        let w = |pairs: &[(&'static str, &str)]| -> Vec<(&'static str, String)> {
            pairs.iter().map(|(k, v)| (*k, (*v).to_owned())).collect()
        };
        assert_eq!(
            carried(&[("ui.text-size", "large")]),
            w(&[("ui.font-size", "15"), ("ui.text-size", "default")])
        );
        assert_eq!(
            carried(&[("ui.text-size", "large"), ("ui.font-size", "13")]),
            w(&[("ui.text-size", "default")])
        );
        assert_eq!(
            carried(&[("ui.text-size", "small")]),
            w(&[("ui.text-size", "default")])
        );
        assert!(carried(&[("ui.text-size", "default")]).is_empty());
        assert!(carried(&[]).is_empty());
    }

    #[test]
    fn a_retired_quick_accent_becomes_the_theme_accent_once() {
        let writes = carried(&[("ui.accent", "sky")]);
        assert_eq!(writes[0], ("ui.theme", "custom".to_owned()));
        let theme = crate::theme::parse_custom(&writes[1].1).expect("a valid custom theme");
        assert_eq!(theme.light.accent, [0x3b, 0x82, 0xe6]);
        assert_eq!(theme.dark.accent, [0x7f, 0xb3, 0xf5]);
        assert_eq!(theme.from.as_deref(), Some("zephium"));
        assert_eq!(writes[2], ("ui.accent", "graphite".to_owned()));

        let writes = carried(&[
            ("ui.accent", "rose"),
            ("ui.theme", "custom"),
            (
                "ui.theme-custom",
                r##"{"light":{"bg":"#ffffff","fg":"#000000","accent":"#111111"},"dark":{"bg":"#000000","fg":"#ffffff","accent":"#222222"}}"##,
            ),
        ]);
        let theme = crate::theme::parse_custom(&writes[1].1).unwrap();
        assert_eq!(theme.light.bg, [0xff, 0xff, 0xff]);
        assert_eq!(theme.dark.accent, [0xee, 0x8f, 0x95]);

        assert!(carried(&[("ui.accent", "graphite")]).is_empty());
        assert!(carried(&[("ui.accent", "graphite"), ("ui.theme", "nord")]).is_empty());
    }

    #[test]
    fn every_feature_switch_defaults_on_and_stands_alone() {
        for feature in ["ai", "work", "notes", "tasks", "time"] {
            let key: &'static str = Box::leak(format!("{feature}.enabled").into_boxed_str());
            assert!(KEYS.contains(&key) && value_allowed(key, "false"));
            assert!(feature_enabled(stored(&[]), feature));
            let off: &'static [(&'static str, &'static str)] =
                Box::leak(Box::new([(key, "false")]));
            assert!(!feature_enabled(stored(off), feature));
            for other in ["ai", "work", "notes", "tasks", "time"]
                .iter()
                .filter(|o| **o != feature)
            {
                assert!(
                    feature_enabled(stored(off), other),
                    "{feature} off turned {other} off"
                );
            }
        }
        assert!(carried(&[("work.enabled", "false"), ("ai.enabled", "false")]).is_empty());
    }

    #[test]
    fn startup_pages_are_a_short_list_of_distinct_web_addresses() {
        assert!(value_allowed("tabs.startup-pages", ""));
        assert!(value_allowed(
            "tabs.startup-pages",
            "example.com\nhttps://docs.example.org/a?b=1"
        ));
        let full: Vec<String> = (0..MAX_STARTUP_PAGES)
            .map(|n| format!("https://example.com/{n}"))
            .collect();
        assert!(value_allowed("tabs.startup-pages", &full.join("\n")));
        let over = [full.as_slice(), &["https://example.com/x".to_owned()]].concat();
        assert!(!value_allowed("tabs.startup-pages", &over.join("\n")));
        for refused in [
            "two words",
            "javascript:alert(1)",
            "file:///etc/passwd",
            "/Users/me/page.html",
            "https://user:pass@example.com",
            "https://example.com\n\nhttps://example.org",
            "https://example.com\n",
            "example.com\nhttps://example.com/",
            "about:blank",
        ] {
            assert!(!value_allowed("tabs.startup-pages", refused), "{refused:?}");
        }
    }

    #[test]
    fn a_stored_startup_list_yields_only_pages_it_can_open() {
        assert_eq!(
            startup_pages("example.com\nnot a page\nhttps://example.com/\nfile:///x"),
            ["https://example.com/"]
        );
        let many = (0..30)
            .map(|n| format!("https://example.com/{n}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(startup_pages(&many).len(), MAX_STARTUP_PAGES);
        assert!(startup_pages("").is_empty());
    }

    #[test]
    fn the_home_page_is_empty_or_a_web_address() {
        assert!(value_allowed("tabs.home", ""));
        assert!(value_allowed("tabs.home", "example.com"));
        assert!(value_allowed("tabs.home", "https://example.com/start"));
        for refused in [
            "two words",
            "javascript:alert(1)",
            "file:///etc/hosts",
            "about:blank",
        ] {
            assert!(!value_allowed("tabs.home", refused), "{refused:?}");
        }
        assert_eq!(
            home_page("example.com").as_deref(),
            Some("https://example.com/")
        );
        assert_eq!(home_page(""), None);
    }

    #[test]
    fn new_tab_destination_is_empty_or_a_web_address_without_search_fallback() {
        for value in [
            "",
            "example.com",
            "https://example.com/start?q=1",
            "http://localhost:8080/",
        ] {
            assert!(value_allowed("tabs.new-page", value), "{value:?}");
        }
        for value in [
            "two words",
            "javascript:alert(1)",
            "file:///etc/hosts",
            "/Users/me/home.html",
            "C:\\dev\\home.html",
            "about:blank",
            "https://user:pass@example.com",
            "https://tauri.localhost/",
        ] {
            assert!(!value_allowed("tabs.new-page", value), "{value:?}");
        }
        assert!(!value_allowed(
            "tabs.new-page",
            &format!("https://example.com/{}", "a".repeat(2048))
        ));
    }

    #[test]
    fn the_proxy_is_the_systems_or_one_validated_address() {
        for mode in ["system", "http", "socks5"] {
            assert!(value_allowed("network.proxy", mode));
        }
        for mode in ["", "manual", "SOCKS5", "https"] {
            assert!(!value_allowed("network.proxy", mode));
        }
        assert!(value_allowed("network.proxy-address", ""));
        assert!(value_allowed(
            "network.proxy-address",
            "Proxy.Example.com:3128"
        ));
        assert!(value_allowed("network.proxy-address", "[::1]:1080"));
        for refused in [
            "proxy.example.com",
            "http://proxy.example.com:80",
            "user@proxy.example.com:80",
            "proxy.example.com:80 --disable-web-security",
        ] {
            assert!(
                !value_allowed("network.proxy-address", refused),
                "{refused:?}"
            );
        }
    }
}
