use std::collections::BTreeSet;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use url::{Host, Url};

use super::store::{digest, read_bytes};
use super::{BenchError, Result, MAX_URL_BYTES};

pub(super) const MAX_CORPUS_FILES: usize = 32;
pub(super) const MAX_QUERIES: usize = 1024;
const MAX_CORPUS_BYTES: usize = 1024 * 1024;
const MAX_TEXT_BYTES: usize = 4096;
const MAX_LABEL_BYTES: usize = 80;
const MAX_VALUES: usize = 32;
const MAX_PAGES: usize = 5;
const MAX_PAGE_URL_BYTES: usize = zephium_map::key::MAX_URL_BYTES;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Kind {
    Place,
    Passage,
    Compare,
    Instant,
    List,
}

impl From<&zephium_map::QueryKind> for Kind {
    fn from(kind: &zephium_map::QueryKind) -> Self {
        use zephium_map::QueryKind;
        match kind {
            QueryKind::Instant => Self::Instant,
            QueryKind::List => Self::List,
            QueryKind::Place { .. } => Self::Place,
            QueryKind::Compare { .. } => Self::Compare,
            QueryKind::Passage { .. } => Self::Passage,
        }
    }
}

impl Kind {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Place => "place",
            Self::Passage => "passage",
            Self::Compare => "compare",
            Self::Instant => "instant",
            Self::List => "list",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Group {
    Discussion,
    Video,
    Social,
    Images,
    Featured,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CorpusFile {
    query: Vec<RawQuery>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawQuery {
    id: String,
    text: String,
    kind: Kind,
    site: Option<String>,
    #[serde(default)]
    sites: Vec<String>,
    #[serde(default)]
    want: Vec<String>,
    answer: Option<String>,
    #[serde(default)]
    from: Vec<String>,
    pages: Option<Vec<String>>,
    #[serde(default)]
    group: BTreeSet<Group>,
    slice: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct QueryLabel {
    pub(super) id: String,
    pub(super) kind: Kind,
    pub(super) slice: Option<String>,
}

pub(super) struct CorpusQuery {
    pub(super) label: QueryLabel,
    pub(super) text: String,
    pub(super) site: Option<String>,
    pub(super) sites: Vec<String>,
    pub(super) want: Vec<Destination>,
    pub(super) answer: Option<String>,
    pub(super) from: Vec<String>,
    pub(super) pages: Vec<Url>,
    pub(super) group: BTreeSet<Group>,
    pub(super) index: usize,
}

pub(super) struct Destination {
    host: String,
    path: String,
    fragment: Option<String>,
}

impl Destination {
    fn parse(value: &str) -> Result<Self> {
        if value.len() > MAX_URL_BYTES {
            return Err(BenchError::new("corpus_destination_limit"));
        }
        let (name, _) = value.split_once('/').unwrap_or((value, ""));
        let host = host(name.trim_end_matches('.')).ok_or(BenchError::new("invalid_want"))?;
        let url =
            Url::parse(&format!("https://{value}")).map_err(|_| BenchError::new("invalid_want"))?;
        if url.query().is_some() || value.chars().any(|c| c.is_control() || c == '\\') {
            return Err(BenchError::new("invalid_want"));
        }
        Ok(Self {
            host,
            path: url.path().to_owned(),
            fragment: url.fragment().map(str::to_owned),
        })
    }

    pub(super) fn matched_locale(&self, url: &Url) -> Option<bool> {
        let host = self.host.trim_end_matches('.');
        let actual = url.host_str().unwrap_or_default().trim_end_matches('.');
        let expected = host.strip_prefix("www.").unwrap_or(host);
        let actual = actual.strip_prefix("www.").unwrap_or(actual);
        if actual != expected
            || self
                .fragment
                .as_deref()
                .is_some_and(|f| url.fragment() != Some(f))
        {
            return None;
        }
        let matches = |path: &str| {
            path.strip_prefix(&self.path).is_some_and(|tail| {
                tail.is_empty() || self.path.ends_with('/') || tail.starts_with('/')
            })
        };
        if matches(url.path()) {
            return Some(false);
        }
        let path = url.path().strip_prefix('/')?;
        let (segment, _) = path.split_once('/')?;
        let (lang, region) = segment
            .split_once('-')
            .map_or((segment, None), |(l, r)| (l, Some(r)));
        (lang.len() == 2
            && lang.bytes().all(|b| b.is_ascii_alphabetic())
            && region.is_none_or(|r| {
                (2..=4).contains(&r.len()) && r.bytes().all(|b| b.is_ascii_alphanumeric())
            })
            && matches(&path[segment.len()..]))
        .then_some(true)
    }
}

fn host(value: &str) -> Option<String> {
    if value.is_empty()
        || value.len() > MAX_URL_BYTES
        || value
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || "/\\?#@:%".contains(c))
    {
        return None;
    }
    let Host::Domain(domain) = Host::parse(value).ok()? else {
        return None;
    };
    let domain = domain.trim_end_matches('.');
    let valid_label = |label: &str| {
        !label.starts_with('-')
            && !label.ends_with('-')
            && !label.is_empty()
            && label
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-')
    };
    (psl::domain_str(domain).is_some() && domain.split('.').all(valid_label))
        .then(|| domain.to_owned())
}

fn label(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_LABEL_BYTES
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"-_".contains(&c))
}

pub(super) fn valid_id(value: &str) -> bool {
    label(value)
        && !matches!(value, "con" | "prn" | "aux" | "nul")
        && !["com", "lpt"].iter().any(|prefix| {
            value.strip_prefix(prefix).is_some_and(|suffix| {
                suffix.len() == 1 && suffix.bytes().all(|c| c.is_ascii_digit())
            })
        })
}

impl QueryLabel {
    pub(super) fn valid(&self) -> bool {
        valid_id(&self.id) && self.slice.as_deref().is_none_or(label)
    }
}

impl TryFrom<RawQuery> for CorpusQuery {
    type Error = BenchError;

    fn try_from(raw: RawQuery) -> Result<Self> {
        let label = QueryLabel {
            id: raw.id,
            kind: raw.kind,
            slice: raw.slice,
        };
        if !label.valid() {
            return Err(BenchError::new("invalid_query_label"));
        }
        let bad_text = |text: &str| text.trim().is_empty() || text.len() > MAX_TEXT_BYTES;
        if raw.answer.as_deref().is_some_and(bad_text) {
            return Err(BenchError::new("invalid_query_text_or_answer"));
        }
        if [raw.sites.len(), raw.want.len(), raw.from.len()]
            .into_iter()
            .any(|len| len > MAX_VALUES)
        {
            return Err(BenchError::new("corpus_list_limit"));
        }
        if raw.pages.is_some() && raw.kind != Kind::Passage {
            return Err(BenchError::new("pages_require_passage"));
        }
        let pages = raw.pages.unwrap_or_default();
        if pages.len() > MAX_PAGES {
            return Err(BenchError::new("corpus_pages_limit"));
        }
        let pages = pages
            .iter()
            .map(|page| {
                if page.len() > MAX_PAGE_URL_BYTES {
                    return Err(BenchError::new("corpus_page_url_limit"));
                }
                Url::parse(page)
                    .ok()
                    .filter(|url| {
                        page.trim() == page
                            && page
                                .get(.."https://".len())
                                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("https://"))
                            && !page.chars().any(|c| c.is_control() || c == '\\')
                            && url.as_str().len() <= MAX_PAGE_URL_BYTES
                            && zephium_public_fetch::guard::public_target(url)
                            && url.domain().and_then(host).is_some()
                    })
                    .ok_or(BenchError::new("invalid_corpus_page"))
            })
            .collect::<Result<_>>()?;
        let site = |value: &str| host(value).ok_or(BenchError::new("invalid_site"));
        let primary = raw.site.as_deref().map(site).transpose()?;
        let sites = raw
            .sites
            .iter()
            .map(|value| site(value))
            .collect::<Result<Vec<_>>>()?;
        match raw.kind {
            Kind::Place if primary.is_none() => return Err(BenchError::new("place_without_site")),
            Kind::Compare if sites.len() != 2 || sites[0] == sites[1] => {
                return Err(BenchError::new("compare_requires_two_distinct_sites"));
            }
            Kind::Passage if raw.answer.is_none() => {
                return Err(BenchError::new("passage_without_answer"));
            }
            _ => {}
        }
        let from = raw
            .from
            .iter()
            .map(|value| {
                host(value)
                    .filter(|domain| psl::domain_str(domain) == Some(domain))
                    .ok_or(BenchError::new("invalid_from_domain"))
            })
            .collect::<Result<_>>()?;
        let query = Self {
            label,
            text: raw.text,
            site: primary,
            sites,
            want: raw
                .want
                .iter()
                .map(|want| Destination::parse(want))
                .collect::<Result<_>>()?,
            answer: raw.answer,
            from,
            pages,
            group: raw.group,
            index: 0,
        };
        if bad_text(&query.text) {
            return Err(BenchError::new("invalid_query_text_or_answer"));
        }
        Ok(query)
    }
}

pub(super) struct Corpus {
    pub(super) queries: Vec<CorpusQuery>,
    pub(super) hashes: Vec<String>,
}

impl Corpus {
    pub(super) fn load(paths: &[PathBuf]) -> Result<Self> {
        if paths.is_empty() || paths.len() > MAX_CORPUS_FILES {
            return Err(BenchError::new("corpus_file_count"));
        }
        let mut corpus = Self {
            queries: Vec::new(),
            hashes: Vec::new(),
        };
        let mut ids = BTreeSet::new();
        for (index, path) in paths.iter().enumerate() {
            let source = read_bytes(path, MAX_CORPUS_BYTES).map_err(|error| error.at(index + 1))?;
            let text = std::str::from_utf8(&source)
                .map_err(|_| BenchError::new("corpus_encoding").at(index + 1))?;
            let mut queries = parse(text).map_err(|error| error.at(index + 1))?;
            if corpus.queries.len() + queries.len() > MAX_QUERIES {
                return Err(BenchError::new("corpus_query_limit"));
            }
            for (index, query) in queries.iter_mut().enumerate() {
                if !ids.insert(query.label.id.clone()) {
                    return Err(BenchError::new("duplicate_query_id").for_query(&query.label.id));
                }
                query.index = corpus.queries.len() + index;
            }
            corpus.queries.extend(queries);
            corpus.hashes.push(digest(&source));
        }
        Ok(corpus)
    }
}

fn parse(text: &str) -> Result<Vec<CorpusQuery>> {
    let file: CorpusFile = toml::from_str(text).map_err(|error| {
        let mut message = BenchError::new("invalid_corpus_toml");
        if let Some(span) = error.span() {
            let line = text.as_bytes()[..span.start.min(text.len())]
                .iter()
                .filter(|&&c| c == b'\n')
                .count();
            message = message.line(line + 1);
        }
        message
    })?;
    if file.query.is_empty() || file.query.len() > MAX_QUERIES {
        return Err(BenchError::new("corpus_query_count"));
    }
    file.query
        .into_iter()
        .enumerate()
        .map(|(index, query)| CorpusQuery::try_from(query).map_err(|error| error.entry(index + 1)))
        .collect()
}

#[cfg(test)]
pub(super) fn fixture(text: &str) -> CorpusQuery {
    parse(text).expect("test corpus").remove(0)
}

#[cfg(test)]
pub(super) fn load_external_fixture(paths: &[PathBuf]) -> Result<Option<Corpus>> {
    let mut present = false;
    for path in paths {
        if super::store::exists(path)? {
            present = true;
        } else {
            for parent in path.ancestors().skip(1) {
                if super::store::directory(parent, false)? {
                    break;
                }
            }
        }
    }
    if present {
        Corpus::load(paths).map(Some)
    } else {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLACE: &str =
        "[[query]]\nid='test'\ntext='private search'\nkind='place'\nsite='example.com'\n";

    #[test]
    fn external_fixture_only_skips_missing_paths() {
        let temp = tempfile::tempdir().unwrap();
        let missing = temp.path().join("missing.toml");
        assert!(load_external_fixture(std::slice::from_ref(&missing))
            .unwrap()
            .is_none());

        let file = temp.path().join("file.toml");
        std::fs::write(&file, PLACE).unwrap();
        assert!(load_external_fixture(std::slice::from_ref(&file))
            .unwrap()
            .is_some());
        assert!(load_external_fixture(&[file.clone(), missing.clone()]).is_err());
        assert!(load_external_fixture(&[missing, file.join("corpus.toml")]).is_err());
    }

    #[test]
    fn actual_corpora_when_available() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../zephium-audit/impl/search-map");
        let paths = [root.join("corpus-v0.toml"), root.join("corpus-v1.toml")];
        if let Some(corpus) = load_external_fixture(&paths).expect("both real corpora") {
            assert_eq!(corpus.hashes.len(), 2);
            for kind in [
                Kind::Place,
                Kind::Compare,
                Kind::Passage,
                Kind::Instant,
                Kind::List,
            ] {
                assert!(corpus.queries.iter().any(|query| query.label.kind == kind));
            }
        }
    }

    #[test]
    fn rejects_malformed_and_out_of_bounds_corpora_without_echoing_input() {
        let cases = [
            ("unknown", format!("{PLACE}private_field='private payload'")),
            (
                "unknown kind",
                PLACE.replace("kind='place'", "kind='private payload'"),
            ),
            ("missing site", PLACE.replace("site=", "#site=")),
            ("missing answer", PLACE.replace("'place'", "'passage'")),
            (
                "missing compare sites",
                PLACE.replace("'place'", "'compare'"),
            ),
            (
                "too many compare sites",
                format!(
                    "{}sites=['a.com','b.com','c.com']",
                    PLACE.replace("'place'", "'compare'")
                ),
            ),
            (
                "duplicate compare sites",
                format!(
                    "{}sites=['a.com','A.COM']",
                    PLACE.replace("'place'", "'compare'")
                ),
            ),
            (
                "bad site",
                PLACE.replace("example.com", "https://example.com"),
            ),
            ("bad want", format!("{PLACE}want=['https://example.com']")),
            ("bad from", format!("{PLACE}from=['docs.example.com']")),
            ("bad group", format!("{PLACE}group=['private payload']")),
            ("bad slice", format!("{PLACE}slice='private payload'")),
            ("unsafe id", PLACE.replace("'test'", "'../private payload'")),
            ("reserved id", PLACE.replace("'test'", "'lpt1'")),
            ("empty text", PLACE.replace("private search", "")),
            (
                "text limit",
                PLACE.replace("private search", &"x".repeat(MAX_TEXT_BYTES + 1)),
            ),
            (
                "label limit",
                PLACE.replace("'test'", &format!("'{}'", "x".repeat(MAX_LABEL_BYTES + 1))),
            ),
            (
                "list limit",
                format!("{PLACE}want=[{}]", "'example.com',".repeat(MAX_VALUES + 1)),
            ),
            (
                "want size limit",
                format!("{PLACE}want=['example.com/{}']", "x".repeat(MAX_URL_BYTES)),
            ),
            ("no queries", "query=[]".into()),
            ("query count", PLACE.repeat(MAX_QUERIES + 1)),
        ];
        for (scenario, text) in cases {
            let error = parse(&text)
                .err()
                .unwrap_or_else(|| panic!("accepted {scenario}"));
            assert!(!error.to_string().contains("private"), "{scenario}");
        }
    }

    #[test]
    fn pages_are_passage_only_and_public_https_within_both_limits() {
        let passage = PLACE.replace("'place'", "'passage'") + "answer='example'\n";
        let page = "'https://example.com/docs',";
        let with = |pages: &str| format!("{passage}pages=[{pages}]");
        assert_eq!(
            parse(&with(&page.repeat(MAX_PAGES))).unwrap()[0]
                .pages
                .len(),
            MAX_PAGES
        );
        assert_eq!(
            parse(&with(&page.repeat(MAX_PAGES + 1)))
                .err()
                .unwrap()
                .class,
            "corpus_pages_limit"
        );
        assert_eq!(
            parse(&format!("{PLACE}pages=[]")).err().unwrap().class,
            "pages_require_passage"
        );
        for bad in [
            "http://example.com/",
            "https://localhost/",
            "https://127.0.0.1/",
            "https://example.com:8443/",
            "https://name@example.com/",
            "https://example.invalid/",
            "https://intranet.local/",
            "https:example.com/",
            " https://example.com/",
        ] {
            assert_eq!(
                parse(&with(&format!("'{bad}'"))).err().unwrap().class,
                "invalid_corpus_page"
            );
        }
        let prefix = "https://example.com/";
        let at_limit = format!(
            "'{prefix}{}'",
            "x".repeat(MAX_PAGE_URL_BYTES - prefix.len())
        );
        assert!(parse(&with(&at_limit)).is_ok());
        let over_limit = at_limit.replace("/x", "/xx");
        assert_eq!(
            parse(&with(&over_limit)).err().unwrap().class,
            "corpus_page_url_limit"
        );
    }

    #[test]
    fn combined_queries_and_files_are_validated() {
        let directory = tempfile::tempdir().unwrap();
        let one = directory.path().join("one.toml");
        std::fs::write(&one, PLACE).unwrap();
        let duplicate = Corpus::load(&[one.clone(), one.clone()]).err().unwrap();
        assert_eq!(duplicate.class, "duplicate_query_id");
        assert!(Corpus::load(&vec![one.clone(); MAX_CORPUS_FILES + 1]).is_err());
        let two = directory.path().join("two.toml");
        std::fs::write(&two, PLACE.repeat(MAX_QUERIES)).unwrap();
        assert_eq!(
            Corpus::load(&[one, two.clone()]).err().unwrap().class,
            "corpus_query_limit"
        );
        std::fs::write(&two, vec![0; MAX_CORPUS_BYTES + 1]).unwrap();
        assert!(Corpus::load(&[two]).is_err());
    }
}

#[cfg(test)]
#[test]
fn a_partially_installed_external_corpus_is_an_error() {
    let root = tempfile::tempdir().unwrap();
    let paths = [root.path().join("v0.toml"), root.path().join("v1.toml")];
    assert!(load_external_fixture(&paths).unwrap().is_none());
    std::fs::write(
        &paths[0],
        "[[query]]\nid='v0'\ntext='Acme'\nkind='place'\nsite='example.com'\n",
    )
    .unwrap();
    assert!(load_external_fixture(&paths).is_err());
}
