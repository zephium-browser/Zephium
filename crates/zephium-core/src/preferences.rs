//! Validated UI preferences shared by desktop admission and the application actor.
pub const KEYS: &[&str] = &[
    "appearance",
    "search.engine",
    "search.custom-url",
    "search.suggestions",
    "sidebar.mode",
    "sidebar.hidden",
    "ui.accent",
    "ui.material",
    "ui.reduce-motion",
    "ui.newtab-greeting",
    "ui.newtab-name",
    "ui.newtab-clock",
    "ui.newtab-clock-format",
    "ui.newtab-tasks",
    "ui.tab-layout",
    "onboarding",
    "ai.enabled",
    "work.enabled",
    "work.screenshots",
    "ui.language",
    "ui.density",
    "ui.text-size",
    "ui.contrast",
    "search.history",
    "tabs.new-position",
    "tabs.after-close",
    "tabs.switch-to-open",
    "tabs.automatic-pip",
    "tabs.startup",
    "tabs.startup-pages",
    "tabs.home",
    "network.proxy",
    "network.proxy-address",
    "time.track",
    "time.retention",
    "focus.minutes",
    "focus.breaks",
    "focus.goal",
    "focus.blocked",
    "performance.sleep",
    "performance.after",
    "performance.memory",
    "performance.exceptions",
    "updates.auto-check",
    "notice.seen-version",
    "notice.security-dismissed",
    "ui.theme",
    "ui.theme-custom",
    "ui.window-tint",
    "ui.font",
    "ui.font-size",
    "ui.code-font",
    "tabs.archive",
    "tabs.external-position",
    "tabs.close-button",
    "ui.theme-contrast",
    "notes.enabled",
    "tasks.enabled",
    "time.enabled",
    "work.favourite-models",
    "zoom.default",
];

/// Pages "Open specific pages" may hold.
pub const MAX_STARTUP_PAGES: usize = 10;

/// The addresses of a stored startup list, as web pages and without
/// repeats. Text that is not an address is dropped, so a hand-edited store
/// can only shorten the list.
pub fn startup_pages(value: &str) -> Vec<String> {
    let mut pages = Vec::new();
    for line in value.lines() {
        let Some(page) = crate::navigation::web_address(line) else {
            continue;
        };
        let page = page.to_string();
        if !pages.contains(&page) {
            pages.push(page);
        }
    }
    pages.truncate(MAX_STARTUP_PAGES);
    pages
}

/// The home page a stored value names, if it is a web address.
pub fn home_page(value: &str) -> Option<String> {
    crate::navigation::web_address(value).map(|page| page.to_string())
}

/// Interface languages a person may choose, by BCP 47 tag. One without a
/// translation yet shows English; a translation lands by adding its locale to
/// `project.inlang`. Mirrors `INTERFACE_LANGUAGES` in the frame.
pub const LANGUAGES: &[&str] = &[
    "system", "en", "ar", "bg", "bn", "ca", "cs", "da", "de", "el", "es", "es-419", "et", "fa",
    "fi", "fil", "fr", "he", "hi", "hr", "hu", "id", "it", "ja", "ko", "lt", "lv", "ms", "nb",
    "nl", "pl", "pt-BR", "pt-PT", "ro", "ru", "sk", "sl", "sr", "sv", "sw", "ta", "te", "th", "tr",
    "uk", "vi", "zh-CN", "zh-TW",
];

/// Monospaced faces for code, by id. The system's own on each platform, or
/// one the person has installed; the frame offers only those it can find.
pub const CODE_FONTS: &[&str] = &[
    "system",
    "menlo",
    "jetbrains-mono",
    "fira-code",
    "ibm-plex-mono",
    "cascadia-code",
    "source-code-pro",
    "consolas",
];

/// Settings from an earlier release carried into their replacements, as the
/// writes to make. Run once at startup: each retired value is reset as it is
/// carried, so a second run finds nothing to do.
pub fn migrations(stored: impl Fn(&str) -> Option<String>) -> Vec<(&'static str, String)> {
    let mut writes = Vec::new();
    match stored("ui.text-size").as_deref() {
        Some("large") => {
            if stored("ui.font-size").is_none() {
                writes.push(("ui.font-size", "15".to_owned()));
            }
            writes.push(("ui.text-size", "default".to_owned()));
        }
        Some("small") => writes.push(("ui.text-size", "default".to_owned())),
        _ => {}
    }
    // The quick accents are gone; a chosen one becomes the theme's accent.
    if let Some(accent) = stored("ui.accent").as_deref().and_then(retired_accent) {
        let theme = stored("ui.theme").unwrap_or_else(|| crate::theme::DEFAULT.to_owned());
        let custom = stored("ui.theme-custom");
        if let Some(carried) = crate::theme::with_accent(&theme, custom.as_deref(), accent) {
            writes.push(("ui.theme", crate::theme::CUSTOM.to_owned()));
            writes.push(("ui.theme-custom", carried));
        }
        writes.push(("ui.accent", "graphite".to_owned()));
    }
    writes
}

/// A retired quick accent as (light, dark) colours.
fn retired_accent(id: &str) -> Option<(crate::theme::Rgb, crate::theme::Rgb)> {
    Some(match id {
        "sky" => ([0x3b, 0x82, 0xe6], [0x7f, 0xb3, 0xf5]),
        "sage" => ([0x2f, 0x8a, 0x5e], [0x86, 0xc9, 0xa4]),
        "rose" => ([0xd2, 0x54, 0x5e], [0xee, 0x8f, 0x95]),
        "amber" => ([0xa5, 0x68, 0x0f], [0xe9, 0xb8, 0x72]),
        "teal" => ([0x18, 0x8a, 0x84], [0x6f, 0xc6, 0xc1]),
        "lavender" => ([0x6e, 0x5b, 0xd6], [0xb3, 0xa4, 0xf2]),
        "orchid" => ([0xb1, 0x4f, 0xa9], [0xdc, 0x9a, 0xd8]),
        _ => return None,
    })
}

/// AI: models, agents and dictation. Off, nothing model-backed runs and
/// no AI surface is shown; keys and data stay.
pub fn ai_enabled(stored: impl Fn(&str) -> Option<String>) -> bool {
    stored("ai.enabled").as_deref() != Some("false")
}

/// Whether a feature switch (`work`, `notes`, `tasks`, `time`) is on.
pub fn feature_enabled(stored: impl Fn(&str) -> Option<String>, feature: &str) -> bool {
    match feature {
        "ai" => ai_enabled(stored),
        "work" => stored("work.enabled").as_deref() != Some("false"),
        "notes" => stored("notes.enabled").as_deref() != Some("false"),
        "tasks" => stored("tasks.enabled").as_deref() != Some("false"),
        "time" => stored("time.enabled").as_deref() != Some("false"),
        _ => true,
    }
}

pub fn value_allowed(key: &str, value: &str) -> bool {
    match key {
        "search.custom-url" => value.is_empty() || crate::search::valid_template(value),
        "search.engine" => crate::search::SearchEngine::from_id(value).is_some(),
        "appearance" => matches!(value, "system" | "light" | "dark"),
        "sidebar.mode" => matches!(value, "default" | "compact"),
        "ui.accent" => matches!(
            value,
            "graphite" | "sky" | "sage" | "rose" | "amber" | "teal" | "lavender" | "orchid"
        ),
        // What the window is made of. `auto` takes the best the system offers;
        // a material the system cannot install falls back on its own.
        "ui.material" => matches!(
            value,
            "auto" | "liquid_glass" | "vibrancy" | "acrylic" | "none"
        ),
        "ui.newtab-clock-format" => matches!(value, "system" | "12h" | "24h"),
        "ui.tab-layout" => matches!(value, "vertical" | "horizontal"),
        "ui.language" => LANGUAGES.contains(&value),
        "ui.density" => matches!(value, "comfortable" | "compact"),
        // Retired for `ui.font-size`; still read once, by `migrations`.
        "ui.text-size" => matches!(value, "small" | "default" | "large"),
        "ui.theme" => crate::theme::theme_allowed(value),
        "ui.theme-custom" => value.is_empty() || crate::theme::parse_custom(value).is_some(),
        "ui.window-tint" => value
            .parse::<u8>()
            .is_ok_and(|tint| tint <= 100 && value == tint.to_string()),
        "ui.font" => matches!(value, "system" | "rounded" | "serif" | "mono"),
        "ui.font-size" => matches!(value, "13" | "14" | "15"),
        "ui.code-font" => CODE_FONTS.contains(&value),
        "ui.theme-contrast" => crate::theme::contrast_allowed(value),
        "tabs.new-position" => matches!(value, "end" | "after-current"),
        // Where a link handed over by another application opens.
        "tabs.external-position" => matches!(value, "new-tab" | "end" | "after-current"),
        // How long a Today tab may sit unused before it closes into History.
        "tabs.archive" => matches!(value, "never" | "12h" | "1d" | "7d"),
        "tabs.close-button" => matches!(value, "hover" | "always"),
        "tabs.after-close" => matches!(value, "next" | "previous" | "recent"),
        // A new tab in front at launch; everything restored stays in the sidebar.
        "tabs.startup" => matches!(value, "continue" | "new-tab" | "pages"),
        // One web address per line, at most ten, none repeated.
        "tabs.startup-pages" => {
            let lines: Vec<&str> = value.lines().collect();
            lines.len() <= MAX_STARTUP_PAGES
                && !value.ends_with('\n')
                && lines.iter().enumerate().all(|(index, line)| {
                    crate::navigation::web_address(line).is_some_and(|page| {
                        !lines[..index].iter().any(|earlier| {
                            crate::navigation::web_address(earlier).as_ref() == Some(&page)
                        })
                    })
                })
        }
        // The system's proxy, or one proxy of a kind at one address.
        "network.proxy" => matches!(value, "system" | "http" | "socks5"),
        "network.proxy-address" => value.is_empty() || crate::proxy::parse_address(value).is_some(),
        "tabs.home" => value.is_empty() || home_page(value).is_some(),
        // Native writes `pending` on a fresh install; chrome only ever
        // finishes it, or asks for it again to replay.
        "onboarding" => matches!(value, "pending" | "done"),
        "time.retention" => matches!(value, "30" | "90" | "365"),
        "focus.minutes" => value.parse::<u16>().is_ok_and(|minutes| {
            value == minutes.to_string()
                && (crate::time::MIN_FOCUS_MINUTES..=crate::time::MAX_FOCUS_MINUTES)
                    .contains(&minutes)
        }),
        "focus.goal" => matches!(value, "30" | "60" | "120" | "180" | "240" | "360"),
        "performance.after" => matches!(value, "5" | "15" | "30" | "60"),
        "performance.memory" => matches!(value, "balanced" | "save-memory" | "keep-ready"),
        // One site per line, each already in the form the shell matches on.
        "focus.blocked" | "performance.exceptions" => {
            let sites: Vec<&str> = value.lines().collect();
            sites.len() <= crate::time::MAX_BLOCKED_SITES
                && !value.ends_with('\n')
                && sites.iter().enumerate().all(|(index, site)| {
                    crate::time::normalize_site(site).as_deref() == Some(*site)
                        && !sites[..index].contains(site)
                })
        }
        // The models the person starred in Work's picker, one id per line.
        "work.favourite-models" => {
            let ids: Vec<&str> = value.lines().collect();
            ids.len() <= 64
                && !value.ends_with('\n')
                && ids.iter().enumerate().all(|(index, id)| {
                    !id.is_empty()
                        && id.len() <= 160
                        && id.bytes().all(|byte| byte.is_ascii_graphic())
                        && !ids[..index].contains(id)
                })
        }
        // The page zoom a site shows at until it has a level of its own.
        "zoom.default" => value.parse::<u16>().is_ok_and(|percent| {
            value == percent.to_string() && crate::zoom::DEFAULTS.contains(&percent)
        }),
        // The Zephium version a notice was last acknowledged at.
        "notice.seen-version" | "notice.security-dismissed" => {
            !value.is_empty()
                && value.len() <= 64
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'+' | b'-'))
        }
        "search.suggestions"
        | "ui.reduce-motion"
        | "ui.newtab-greeting"
        | "ui.newtab-name"
        | "ui.newtab-clock"
        | "ui.newtab-tasks"
        | "ai.enabled"
        | "work.enabled"
        | "notes.enabled"
        | "tasks.enabled"
        | "time.enabled"
        | "work.screenshots"
        | "ui.contrast"
        | "search.history"
        | "tabs.switch-to-open"
        | "tabs.automatic-pip"
        | "time.track"
        | "performance.sleep"
        | "updates.auto-check"
        | "focus.breaks"
        | "sidebar.hidden" => {
            matches!(value, "true" | "false")
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn performance_preferences_validate_modes_timing_and_normalized_exceptions() {
        for minutes in ["5", "15", "30", "60"] {
            assert!(value_allowed("performance.after", minutes));
        }
        for value in ["0", "1", "05", "15 minutes", "120"] {
            assert!(!value_allowed("performance.after", value));
        }
        for mode in ["balanced", "save-memory", "keep-ready"] {
            assert!(value_allowed("performance.memory", mode));
        }
        assert!(!value_allowed("performance.memory", "unlimited"));
        assert!(value_allowed("performance.sleep", "false"));
        assert!(value_allowed(
            "performance.exceptions",
            "example.com\nmail.example.org"
        ));
        assert!(value_allowed("performance.exceptions", ""));
        for value in [
            "https://example.com",
            "Example.com",
            "example.com\nexample.com",
            "example.com\n",
        ] {
            assert!(!value_allowed("performance.exceptions", value));
        }
        let oversized = (0..=crate::time::MAX_BLOCKED_SITES)
            .map(|n| format!("site{n}.example"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!value_allowed("performance.exceptions", &oversized));
    }
    #[test]
    fn focus_settings_hold_normalized_sites_and_bounded_numbers() {
        assert!(value_allowed("focus.blocked", ""));
        assert!(value_allowed("focus.blocked", "x.com\nyoutube.com"));
        for value in [
            "x.com\n",
            "x.com\nx.com",
            "https://x.com",
            "www.x.com",
            "X.com",
            "x",
        ] {
            assert!(!value_allowed("focus.blocked", value), "{value:?}");
        }
        assert!(value_allowed("focus.minutes", "25"));
        assert!(value_allowed("focus.minutes", "180"));
        for value in ["4", "181", "025", "25.0", ""] {
            assert!(!value_allowed("focus.minutes", value), "{value:?}");
        }
        assert!(value_allowed("time.retention", "365"));
        assert!(!value_allowed("time.retention", "7"));
        assert!(value_allowed("time.track", "false"));
    }

    #[test]
    fn the_default_zoom_is_one_of_the_offered_levels() {
        assert!(KEYS.contains(&"zoom.default"));
        for percent in crate::zoom::DEFAULTS {
            assert!(value_allowed("zoom.default", &percent.to_string()));
        }
        for value in [
            "", "0", "30", "100%", "1.25", "0100", "125 ", "250", "300", "49",
        ] {
            assert!(!value_allowed("zoom.default", value), "{value:?}");
        }
    }

    #[test]
    fn update_preferences_hold_a_switch_and_bounded_versions() {
        assert!(value_allowed("updates.auto-check", "false"));
        assert!(!value_allowed("updates.auto-check", "off"));
        for key in ["notice.seen-version", "notice.security-dismissed"] {
            assert!(KEYS.contains(&key));
            assert!(value_allowed(key, "1.0.0"));
            assert!(value_allowed(key, "1.0.0-beta.1+build.7"));
            assert!(value_allowed(key, &"9".repeat(64)));
            for value in ["", " 1.0.0", "1.0.0\n", "v1/0", "1.0.0_rc", "é"] {
                assert!(!value_allowed(key, value), "{value:?}");
            }
            assert!(!value_allowed(key, &"9".repeat(65)));
        }
    }

    #[test]
    fn settings_are_closed_and_values_are_bounded() {
        assert!(value_allowed("sidebar.mode", "compact"));
        assert!(value_allowed("ui.accent", "sage"));
        assert!(value_allowed("ui.accent", "orchid"));
        assert!(!value_allowed("ui.accent", "purple"));
        assert!(value_allowed("ui.material", "vibrancy"));
        assert!(!value_allowed("ui.material", "mica"));
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
        for value in ["new-tab", "end", "after-current"] {
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
