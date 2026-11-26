use super::{MAX_CHILDREN, MAX_SECTIONS};
use crate::{key::MAX_URL_BYTES, robots::MAX_SITEMAPS, PageKey, SiteKey};
use quick_xml::{events::Event, Reader};
use std::collections::BTreeMap;
use url::Url;

pub const MAX_XML_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_SITEMAP_URLS: usize = 50_000;
pub const MAX_XML_DEPTH: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum XmlError {
    Limit,
    Malformed,
}
#[derive(Default)]
pub struct Sitemap {
    pub index: Vec<Url>,
    groups: BTreeMap<crate::PageMatch, Cluster>,
    order: Vec<crate::PageMatch>,
    pub truncated: bool,
    pub count: u32,
}
#[derive(Default)]
pub(super) struct Cluster {
    pub pages: Vec<PageKey>,
    pub count: u32,
}

impl Sitemap {
    pub fn parse(bytes: &[u8], source: &Url) -> Result<Self, XmlError> {
        Self::parse_sample(bytes, source, false, source)
    }
    pub(crate) fn parse_sample(
        bytes: &[u8],
        source: &Url,
        truncated: bool,
        entry: &Url,
    ) -> Result<Self, XmlError> {
        if bytes.len() > MAX_XML_BYTES {
            return Err(XmlError::Limit);
        }
        let text = match std::str::from_utf8(bytes) {
            Ok(s) => s,
            Err(e) if truncated && e.error_len().is_none() => {
                std::str::from_utf8(&bytes[..e.valid_up_to()]).map_err(|_| XmlError::Malformed)?
            }
            Err(_) => return Err(XmlError::Malformed),
        };
        let text = if truncated {
            text.rsplit_once('>')
                .map_or("", |(before, _)| &text[..before.len() + 1])
        } else {
            text
        };
        let mut xml = Reader::from_str(text);
        let mut depth = 0usize;
        let mut index = false;
        let mut root = false;
        let mut record = false;
        let mut loc = false;
        let mut value = String::new();
        let mut out = Self::default();
        let site = SiteKey::from_url(source).ok_or(XmlError::Malformed)?;
        loop {
            let event = match xml.read_event() {
                Ok(event) => event,
                Err(quick_xml::Error::Syntax(_)) if truncated => break,
                Err(_) => return Err(XmlError::Malformed),
            };
            match event {
                Event::Start(tag) => {
                    depth += 1;
                    if depth > MAX_XML_DEPTH {
                        return Err(XmlError::Limit);
                    }
                    let name = tag.local_name();
                    match depth {
                        1 => {
                            if root || !matches!(name.as_ref(), "sitemapindex" | "urlset") {
                                return Err(XmlError::Malformed);
                            }
                            root = true;
                            index = name.as_ref() == "sitemapindex";
                        }
                        2 => record = name.as_ref() == if index { "sitemap" } else { "url" },
                        3 => {
                            loc = record && name.as_ref() == "loc";
                            value.clear();
                        }
                        _ if loc => return Err(XmlError::Malformed),
                        _ => {}
                    }
                }
                Event::End(_) => {
                    if depth == 3 && loc {
                        out.count += 1;
                        if out.count as usize > MAX_SITEMAP_URLS {
                            return Err(XmlError::Limit);
                        }
                        if let Ok(url) = Url::parse(value.trim()) {
                            if SiteKey::from_url(&url).as_ref() == Some(&site) {
                                out.add(url, source, index, entry);
                            }
                        }
                        loc = false;
                    }
                    depth = depth.checked_sub(1).ok_or(XmlError::Malformed)?;
                }
                Event::Text(t) if loc => append_loc(&mut value, &t.into_inner())?,
                Event::CData(t) if loc => append_loc(&mut value, &t.into_inner())?,
                Event::GeneralRef(r) if loc => {
                    let c = match r.as_ref() {
                        "amp" => '&',
                        "lt" => '<',
                        "gt" => '>',
                        "apos" => '\'',
                        "quot" => '"',
                        _ => r
                            .resolve_char_ref()
                            .map_err(|_| XmlError::Malformed)?
                            .ok_or(XmlError::Malformed)?,
                    };
                    value.push(c);
                }
                Event::CData(_) | Event::GeneralRef(_) if depth == 0 => {
                    return Err(XmlError::Malformed);
                }
                Event::DocType(_) => return Err(XmlError::Malformed),
                Event::Empty(tag) if depth == 0 => {
                    if root || !matches!(tag.local_name().as_ref(), "urlset" | "sitemapindex") {
                        return Err(XmlError::Malformed);
                    }
                    root = true;
                }
                Event::Empty(_) if depth >= MAX_XML_DEPTH => return Err(XmlError::Limit),
                Event::Empty(_) if loc => return Err(XmlError::Malformed),
                Event::Text(t) if depth == 0 && !t.trim().is_empty() => {
                    return Err(XmlError::Malformed)
                }
                Event::Eof => break,
                _ => {}
            }
            if value.len() > MAX_URL_BYTES {
                return Err(XmlError::Limit);
            }
        }
        out.truncated |= truncated;
        if !truncated && (!root || depth != 0) {
            return Err(XmlError::Malformed);
        }
        Ok(out)
    }
    fn add(&mut self, url: Url, source: &Url, index: bool, entry: &Url) {
        let Ok(key) = PageKey::from_url(&url) else {
            return;
        };
        if index {
            if crate::key::same_page(source, key.as_url())
                || self
                    .index
                    .iter()
                    .any(|u| crate::key::same_page(u, key.as_url()))
            {
                return;
            }
            if self.index.len() >= MAX_SITEMAPS {
                let rank = |u: &Url| (super::sitemap_priority(u, entry), u.as_str().to_owned());
                if let Some(worst) = self.index.iter_mut().max_by_key(|u| rank(u)) {
                    if rank(key.as_url()) < rank(worst) {
                        *worst = key.as_url().clone();
                    }
                }
                self.truncated = true;
                return;
            }
            self.index.push(key.as_url().clone());
            return;
        }
        let Some(prefix) = url
            .path_segments()
            .and_then(|mut s| s.find(|s| !s.is_empty()))
        else {
            return;
        };
        let Ok(base) = url.join(&format!("/{prefix}")) else {
            return;
        };
        let Ok(base) = PageKey::from_url(&base).map(|k| k.page_match()) else {
            return;
        };
        if self.groups.len() >= MAX_SECTIONS && !self.groups.contains_key(&base) {
            self.truncated = true;
            return;
        }
        if !self.groups.contains_key(&base) {
            self.order.push(base.clone());
        }
        let group = self.groups.entry(base).or_default();
        group.add(key);
    }
    pub(super) fn groups(&self) -> impl Iterator<Item = (&crate::PageMatch, &Cluster)> {
        self.order
            .iter()
            .filter_map(|p| Some((p, self.groups.get(p)?)))
    }
    pub(super) fn merge(&mut self, other: &Self) {
        self.count = self.count.saturating_add(other.count);
        self.truncated |= other.truncated;
        for (prefix, cluster) in other.groups() {
            if self.groups.len() >= MAX_SECTIONS && !self.groups.contains_key(prefix) {
                self.truncated = true;
                continue;
            }
            if !self.groups.contains_key(prefix) {
                self.order.push(prefix.clone());
            }
            let into = self.groups.entry(prefix.clone()).or_default();
            into.count += cluster.count - cluster.pages.len() as u32;
            for key in &cluster.pages {
                into.add(key.clone());
            }
        }
    }
}
fn append_loc(value: &mut String, chunk: &str) -> Result<(), XmlError> {
    if chunk.len() > MAX_URL_BYTES - value.len() {
        return Err(XmlError::Limit);
    }
    value.push_str(chunk);
    Ok(())
}
impl Cluster {
    fn add(&mut self, key: PageKey) {
        // Keep the shallowest landing page and a deterministic sample at fixed cost.
        let rank = |p: &PageKey| (p.as_url().path().matches('/').count(), p.as_str().len());
        if self
            .pages
            .iter()
            .any(|p| p.page_match() == key.page_match())
        {
            return;
        }
        if let Err(pos) = self
            .pages
            .binary_search_by(|p| rank(p).cmp(&rank(&key)).then_with(|| p.cmp(&key)))
        {
            self.count += 1;
            if pos <= MAX_CHILDREN {
                self.pages.insert(pos, key);
                self.pages.truncate(MAX_CHILDREN + 1);
            }
        }
    }
}
