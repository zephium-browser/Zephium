use url::Url;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlanceSource {
    Essential,
    Pinned,
    NewTab,
    Ordinary,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GlanceTrigger {
    #[default]
    Shift,
    Ctrl,
    Always,
    Off,
}

impl GlanceTrigger {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "shift" => Some(Self::Shift),
            "ctrl" => Some(Self::Ctrl),
            "always" => Some(Self::Always),
            "off" => Some(Self::Off),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LinkModifiers {
    pub shift: bool,
    pub ctrl: bool,
}

struct SiteRule {
    host: &'static str,
    paths: &'static [&'static str],
}

const SITES: &[SiteRule] = &[
    SiteRule {
        host: "reddit.com",
        paths: &["*/comments/*"],
    },
    SiteRule {
        host: "x.com",
        paths: &["*/status/*"],
    },
    SiteRule {
        host: "facebook.com",
        paths: &["*/posts/*", "/permalink.php*", "/photo.php*"],
    },
    SiteRule {
        host: "youtube.com",
        paths: &["/watch*", "/shorts/*"],
    },
    SiteRule {
        host: "instagram.com",
        paths: &["/p/*", "/reel/*"],
    },
    SiteRule {
        host: "github.com",
        paths: &["*/issues/*", "*/pull/*", "*/discussions/*"],
    },
    SiteRule {
        host: "feedly.com",
        paths: &["/i/entry/*"],
    },
];

pub fn opens_in_glance(
    source: GlanceSource,
    target: &Url,
    trigger: GlanceTrigger,
    modifiers: LinkModifiers,
) -> bool {
    if trigger == GlanceTrigger::Off || !matches!(target.scheme(), "http" | "https") {
        return false;
    }
    let triggered = match trigger {
        GlanceTrigger::Shift => modifiers.shift,
        GlanceTrigger::Ctrl => modifiers.ctrl,
        GlanceTrigger::Always => true,
        GlanceTrigger::Off => false,
    };
    if triggered || source != GlanceSource::Ordinary {
        return true;
    }
    let Some(host) = target.host_str() else {
        return false;
    };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let path = match target.query() {
        Some(query) => format!("{}?{query}", target.path()),
        None => target.path().to_owned(),
    };
    SITES.iter().any(|site| {
        (host == site.host || host.ends_with(&format!(".{}", site.host)))
            && site
                .paths
                .iter()
                .any(|pattern| matches_glob(pattern, &path))
    })
}

fn matches_glob(pattern: &str, text: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    let (first, rest) = parts.split_first().expect("split yields at least one part");
    let Some(mut remaining) = text.strip_prefix(first) else {
        return false;
    };
    let Some((last, middle)) = rest.split_last() else {
        return remaining.is_empty();
    };
    for part in middle {
        match remaining.find(part) {
            Some(at) => remaining = &remaining[at + part.len()..],
            None => return false,
        }
    }
    remaining.ends_with(last)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(text: &str) -> Url {
        Url::parse(text).unwrap()
    }

    const NONE: LinkModifiers = LinkModifiers {
        shift: false,
        ctrl: false,
    };
    const SHIFT: LinkModifiers = LinkModifiers {
        shift: true,
        ctrl: false,
    };
    const CTRL: LinkModifiers = LinkModifiers {
        shift: false,
        ctrl: true,
    };

    #[test]
    fn the_chosen_click_and_kept_tabs_glance() {
        let page = url("https://example.com/article");
        let shift = GlanceTrigger::Shift;
        let ctrl = GlanceTrigger::Ctrl;
        let ordinary = GlanceSource::Ordinary;
        assert!(opens_in_glance(ordinary, &page, shift, SHIFT));
        assert!(!opens_in_glance(ordinary, &page, shift, CTRL));
        assert!(opens_in_glance(ordinary, &page, ctrl, CTRL));
        assert!(!opens_in_glance(ordinary, &page, ctrl, SHIFT));
        assert!(opens_in_glance(
            ordinary,
            &page,
            GlanceTrigger::Always,
            NONE
        ));
        assert!(opens_in_glance(GlanceSource::Essential, &page, shift, NONE));
        assert!(opens_in_glance(GlanceSource::Pinned, &page, shift, NONE));
        assert!(opens_in_glance(GlanceSource::NewTab, &page, shift, NONE));
        assert!(!opens_in_glance(ordinary, &page, shift, NONE));
        assert!(!opens_in_glance(
            GlanceSource::Essential,
            &page,
            GlanceTrigger::Off,
            SHIFT
        ));
        let file = url("file:///c:/x");
        assert!(!opens_in_glance(
            GlanceSource::Essential,
            &file,
            shift,
            SHIFT
        ));
        assert_eq!(GlanceTrigger::parse("ctrl"), Some(GlanceTrigger::Ctrl));
        assert_eq!(GlanceTrigger::parse("alt"), None);
    }

    #[test]
    fn sites_glance_only_their_content_pages() {
        let glances = |text: &str| {
            opens_in_glance(
                GlanceSource::Ordinary,
                &url(text),
                GlanceTrigger::Shift,
                NONE,
            )
        };
        assert!(glances("https://www.reddit.com/r/rust/comments/abc/title/"));
        assert!(glances("https://old.reddit.com/r/rust/comments/abc/"));
        assert!(!glances("https://www.reddit.com/r/rust/"));
        assert!(glances("https://www.youtube.com/watch?v=dQw4w9WgXcQ"));
        assert!(glances("https://youtube.com/shorts/xyz"));
        assert!(!glances("https://www.youtube.com/@channel"));
        assert!(glances("https://github.com/owner/repo/pull/12"));
        assert!(!glances("https://github.com/owner/repo"));
        assert!(glances("https://x.com/someone/status/1"));
        assert!(glances(
            "https://www.facebook.com/permalink.php?story_fbid=1"
        ));
        assert!(!glances("https://notreddit.com/r/a/comments/b/"));
        assert!(!glances("https://reddit.com.evil.test/r/a/comments/b/"));
    }

    #[test]
    fn globs_anchor_at_both_ends() {
        assert!(matches_glob("/watch*", "/watch?v=1"));
        assert!(!matches_glob("/watch*", "/x/watch"));
        assert!(matches_glob("*/comments/*", "/r/a/comments/b"));
        assert!(!matches_glob("/p/*", "/p"));
        assert!(matches_glob("/exact", "/exact"));
        assert!(!matches_glob("/exact", "/exact/more"));
    }
}
