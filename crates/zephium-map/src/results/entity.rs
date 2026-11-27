use super::*;
use crate::query::QueryKind;
use destination::on;

// The site must be named explicitly before an intermediary can represent an entity.
const INTERMEDIARIES: &[(&str, &[&str])] = &[
    ("wikipedia.org", &["wikipedia"]),
    ("wikimedia.org", &["wikimedia"]),
    ("britannica.com", &["britannica"]),
    ("github.com", &["github"]),
    ("github.io", &["github"]),
    ("gitlab.com", &["gitlab"]),
    ("bitbucket.org", &["bitbucket"]),
    ("sourceforge.net", &["sourceforge"]),
    ("codeberg.org", &["codeberg"]),
    ("facebook.com", &["facebook"]),
    ("instagram.com", &["instagram"]),
    ("threads.net", &["threads"]),
    ("linkedin.com", &["linkedin"]),
    ("reddit.com", &["reddit"]),
    ("mastodon.social", &["mastodon"]),
    ("mastodon.online", &["mastodon"]),
    ("fosstodon.org", &["fosstodon"]),
    ("hachyderm.io", &["hachyderm"]),
    ("infosec.exchange", &["infosec"]),
    ("vimeo.com", &["vimeo"]),
    ("news.google.com", &["googlenews"]),
    ("news.yahoo.com", &["yahoonews"]),
    ("pinterest.com", &["pinterest"]),
    ("tiktok.com", &["tiktok"]),
    ("amazon.com", &["amazon"]),
    ("amazon.co.uk", &["amazon"]),
    ("ebay.com", &["ebay"]),
    ("etsy.com", &["etsy"]),
    ("allegro.pl", &["allegro"]),
    ("walmart.com", &["walmart"]),
    ("quora.com", &["quora"]),
    ("stackoverflow.com", &["stackoverflow"]),
    ("stackexchange.com", &["stackexchange"]),
    ("trustpilot.com", &["trustpilot"]),
    ("g2.com", &["g2"]),
    ("capterra.com", &["capterra"]),
    ("yelp.com", &["yelp"]),
    ("tripadvisor.com", &["tripadvisor"]),
    ("nytimes.com", &["nytimes"]),
    ("theguardian.com", &["theguardian"]),
    ("bbc.com", &["bbc"]),
    ("bbc.co.uk", &["bbc"]),
    ("cnn.com", &["cnn"]),
    ("reuters.com", &["reuters"]),
    ("techcrunch.com", &["techcrunch"]),
    ("theverge.com", &["theverge"]),
    ("arstechnica.com", &["arstechnica"]),
    ("forbes.com", &["forbes"]),
    ("bloomberg.com", &["bloomberg"]),
    ("twitter.com", &["twitter", "x"]),
    ("x.com", &["twitter", "x"]),
    ("youtube.com", &["youtube"]),
    ("youtu.be", &["youtube"]),
    ("discord.gg", &["discord"]),
    ("discord.com", &["discord"]),
    ("bsky.app", &["bluesky"]),
    ("apps.apple.com", &["apple", "appstore"]),
    ("play.google.com", &["googleplay"]),
    ("apps.microsoft.com", &["microsoft"]),
    ("store.steampowered.com", &["steam"]),
    ("news.ycombinator.com", &["hackernews"]),
];

struct Entity {
    names: Vec<String>,
}
impl Entity {
    fn new(query: &QueryRead) -> Option<Self> {
        let QueryKind::Place { entity, .. } = query.kind() else {
            return None;
        };
        let first = entity.first()?;
        let mut names = vec![first.to_owned(), entity.join(""), entity.join("-")];
        if let Some(domain) = entity.first().and_then(|word| psl::domain_str(word)) {
            if let Some((label, _)) = domain.split_once('.') {
                names.push(label.to_owned());
            }
        }
        names.retain(|s| !s.is_empty());
        Some(Self { names })
    }

    fn matches(&self, text: &str) -> bool {
        self.names.iter().any(|n| n.eq_ignore_ascii_case(text))
    }

    fn result(
        &self,
        hit: &Organic,
        position: usize,
        panel: &[SiteKey],
    ) -> Option<(usize, Official)> {
        let host = hit.url.host_str()?;
        let site = SiteKey::from_url(&hit.url)?;
        let (label, _) = site.as_str().split_once('.')?;
        let intermediary = INTERMEDIARIES.iter().find(|(domain, _)| on(host, domain));
        let named_intermediary = intermediary.is_some_and(|(_, aliases)| {
            aliases
                .iter()
                .any(|alias| self.names.iter().any(|n| n == alias))
        });
        if intermediary.is_some() && !named_intermediary {
            return None;
        }
        let domain_match = named_intermediary
            || self
                .names
                .iter()
                .any(|n| n == label || label.strip_suffix("-lang") == Some(n.as_str()));
        let panel_website = panel.contains(&site);
        if !domain_match && !panel_website {
            return None;
        }
        let sitelinks = !hit.sitelinks.is_empty();
        let score = usize::from(domain_match) * 4
            + usize::from(panel_website) * 4
            + usize::from(sitelinks) * 2
            + usize::from(position == 0);
        Some((
            score,
            Official {
                site,
                confidence: if domain_match && (position == 0 || panel_website || sitelinks) {
                    Confidence::High
                } else {
                    Confidence::Low
                },
                evidence: Evidence {
                    domain_match,
                    sitelinks,
                    panel_website,
                },
            },
        ))
    }
}

const PROFILE_HOSTS: &[(Network, &[&str])] = &[
    (Network::Twitter, &["x.com", "twitter.com"]),
    (Network::GitHub, &["github.com"]),
    (Network::YouTube, &["youtube.com"]),
    (Network::LinkedIn, &["linkedin.com"]),
    (Network::Discord, &["discord.gg", "discord.com"]),
    (Network::Reddit, &["reddit.com"]),
    (
        Network::Mastodon,
        &[
            "mastodon.social",
            "mastodon.online",
            "fosstodon.org",
            "hachyderm.io",
            "infosec.exchange",
        ],
    ),
    (Network::Bluesky, &["bsky.app"]),
    (Network::Instagram, &["instagram.com"]),
    (Network::Facebook, &["facebook.com"]),
];

fn profile(url: &Url, title: &str, entity: Option<&Entity>) -> Option<Network> {
    let host = url.host_str()?;
    let (net, _) = PROFILE_HOSTS
        .iter()
        .find(|(_, hosts)| hosts.iter().any(|s| on(host, s)))?;
    let path = url.path().trim_matches('/');
    let (section, last) = path.split_once('/').unwrap_or(("", path));
    if last.is_empty() || last.contains('/') {
        return None;
    }
    let handle = match (net, section) {
        (Network::YouTube | Network::Mastodon, "") => last.strip_prefix('@')?,
        (Network::GitHub, "")
            if ["pricing", "login"]
                .iter()
                .any(|p| last.eq_ignore_ascii_case(p)) =>
        {
            return None
        }
        (Network::Twitter, "") if last.eq_ignore_ascii_case("search") => return None,
        (Network::Twitter | Network::GitHub | Network::Instagram | Network::Facebook, "") => last,
        (Network::LinkedIn, "company" | "in" | "school")
        | (Network::Reddit, "r")
        | (Network::YouTube, "user" | "c") => last,
        (Network::Bluesky, "profile") => last.split('.').next()?,
        (Network::Discord, "invite") | (Network::Discord, "")
            if section == "invite" || on(host, "discord.gg") =>
        {
            last
        }
        (Network::YouTube, "channel") => last,
        _ => return None,
    };
    let opaque = matches!(
        (net, section),
        (Network::YouTube, "channel") | (Network::Discord, _)
    );
    entity
        .is_none_or(|entity| {
            entity.matches(handle)
                || opaque && title.split_whitespace().any(|word| entity.matches(word))
        })
        .then_some(*net)
}

fn video(url: &Url) -> bool {
    let host = url.host_str().unwrap_or_default();
    let path = url.path().trim_matches('/');
    let (kind, id) = path.split_once('/').unwrap_or(("", path));
    let single = !id.is_empty() && !id.contains('/');
    if on(host, "youtube.com") || on(host, "youtube-nocookie.com") {
        path == "watch" && url.query_pairs().any(|(k, v)| k == "v" && !v.is_empty())
            || single && matches!(kind, "shorts" | "embed" | "live")
    } else if on(host, "youtu.be") {
        single && kind.is_empty()
    } else if on(host, "vimeo.com") {
        single && matches!(kind, "" | "video") && id.bytes().all(|b| b.is_ascii_digit())
    } else {
        on(host, "dailymotion.com") && kind == "video" && single
    }
}

impl Page<'_> {
    pub(super) fn sitelinks(
        &self,
        title: &Title,
        scope: Option<usize>,
        rules: &Rules,
    ) -> Vec<Link> {
        let mut links = Vec::new();
        let Ok(url) = &title.destination else {
            return links;
        };
        let site = SiteKey::from_url(url);
        let Some(site) = site else { return links };
        let end = scope.map_or(self.context[title.at].end, |p| self.context[p].end);
        for at in self.context[title.at].end..end {
            let n = &self.nodes[at];
            if !self.eligible(at) || self.context[at].paid || n.role != Role::Link {
                continue;
            }
            let Some(target) = n
                .destination
                .as_ref()
                .and_then(|u| rules.destination(u).ok())
            else {
                continue;
            };
            let Some(name) = past_site(&flatten(words(n)), &target).filter(|s| !s.is_empty())
            else {
                continue;
            };
            if crate::key::same_page(&target, url)
                || SiteKey::from_url(&target).as_ref() != Some(&site)
                || links
                    .iter()
                    .any(|l: &Link| crate::key::same_page(&l.url, &target))
            {
                continue;
            }
            links.push(Link {
                title: clip(&name, TITLE_BYTES),
                url: target,
            });
            if links.len() >= MAX_SITELINKS {
                break;
            }
        }
        links
    }

    pub(super) fn enrich(
        &self,
        layer: &mut FirstLayer,
        rules: &Rules,
        query: &QueryRead,
        excluded: &[bool],
    ) {
        let entity = Entity::new(query);
        let mut panel = Vec::new();
        for (at, n) in self.nodes.iter().enumerate() {
            let ctx = self.context[at];
            if ctx.hidden || ctx.paid || ctx.widget || excluded[at] {
                continue;
            }
            if n.role == Role::Link {
                let Some(url) = n
                    .destination
                    .as_ref()
                    .and_then(|u| rules.destination(u).ok())
                else {
                    continue;
                };
                if ctx.panel
                    && [words(n), n.accessible_name()].iter().any(|s| {
                        ["website", "official website"]
                            .iter()
                            .any(|w| s.trim().eq_ignore_ascii_case(w))
                    })
                {
                    if let Some(site) = SiteKey::from_url(&url) {
                        if layer
                            .organic
                            .iter()
                            .any(|r| SiteKey::from_url(&r.url).as_ref() == Some(&site))
                            && !panel.contains(&site)
                        {
                            panel.push(site);
                        }
                    }
                }
                if !self.eligible(at) && !ctx.panel {
                    continue;
                }
                if self.eligible(at)
                    && layer.video.len() < MAX_VIDEO
                    && video(&url)
                    && !layer
                        .video
                        .iter()
                        .any(|v| crate::key::same_page(&v.url, &url))
                {
                    let title = layer
                        .organic
                        .iter()
                        .find(|hit| crate::key::same_page(&hit.url, &url))
                        .map_or_else(|| clip(words(n), TITLE_BYTES), |hit| hit.title.clone());
                    if !title.is_empty() {
                        layer.video.push(Link {
                            title,
                            url: url.clone(),
                        });
                    }
                }
                if layer.social.len() < MAX_SOCIAL {
                    if let Some(network) = entity
                        .as_ref()
                        .and_then(|e| profile(&url, words(n), Some(e)))
                    {
                        if !layer.social.iter().any(|p| p.network == network) {
                            layer.social.push(Profile {
                                network,
                                url,
                                basis: ProfileBasis::Results,
                            });
                        }
                    }
                }
            } else if n.role == Role::Image && self.eligible(at) && layer.images.len() < MAX_IMAGES
            {
                let Some(source) = &n.image_source else {
                    continue;
                };
                let anchor = self
                    .ancestors(at)
                    .find(|&p| self.nodes[p].role == Role::Link);
                let dest = match anchor {
                    Some(p) => self.nodes[p]
                        .destination
                        .as_ref()
                        .and_then(|u| rules.destination(u).ok()),
                    None => rules.destination(source).ok(),
                };
                if let Some(destination) = dest {
                    if !layer.images.iter().any(|i| {
                        i.source == *source && crate::key::same_page(&i.destination, &destination)
                    }) {
                        layer.images.push(Image {
                            title: clip(words(n), TITLE_BYTES),
                            source: source.clone(),
                            destination,
                        });
                    }
                }
            }
        }
        let mut best = None;
        for (position, hit) in layer.organic.iter().enumerate() {
            if let Some((score, official)) = entity
                .as_ref()
                .and_then(|e| e.result(hit, position, &panel))
            {
                if best.as_ref().is_none_or(|(prior, _, _)| score > *prior) {
                    best = Some((score, position, official));
                }
            }
        }
        if let Some((_, index, official)) = best {
            layer.organic[index].official = Some(official);
        }
    }
}

pub(crate) fn site_profile(url: &Url) -> Option<Network> {
    profile(url, "", None)
}
