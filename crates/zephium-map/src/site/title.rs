use super::*;
use crate::doc::Head;

pub const MAX_SITE_NAME_BYTES: usize = 128;

pub fn title_segments(title: &str) -> impl Iterator<Item = &str> {
    title
        .split(['|', '–', '—', '·', '•', ':', '\\'])
        .flat_map(|s| s.split(" - "))
        .map(str::trim)
}

pub(super) fn site_title(head: &Head, site: &SiteKey, page: &Url) -> String {
    fn name(value: &str) -> Option<String> {
        let value = crate::flatten(&crate::clean_text(value));
        (!value.is_empty() && value.len() <= MAX_SITE_NAME_BYTES).then_some(value)
    }
    if let Some(value) = name(&head.site_name).or_else(|| name(&head.application_name)) {
        return value;
    }
    let mut website = None;
    let mut organization = None;
    let mut visit = |obj: &serde_json::Value| {
        if obj.get("url").is_some_and(|v| {
            v.as_str()
                .and_then(|s| page.join(s).ok())
                .and_then(|u| SiteKey::from_url(&u))
                .as_ref()
                != Some(site)
        }) {
            return;
        }
        let slot = if linked::typed(obj, "WebSite") {
            &mut website
        } else if linked::typed(obj, "Organization") {
            &mut organization
        } else {
            return;
        };
        if slot.is_none() {
            *slot = obj["name"].as_str().and_then(name);
        }
    };
    for raw in &head.linked_data {
        let Ok(root) = serde_json::from_str::<serde_json::Value>(raw) else {
            continue;
        };
        for node in root
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or(std::slice::from_ref(&root))
        {
            visit(node);
            if let Some(graph) = node["@graph"].as_array() {
                graph.iter().for_each(&mut visit);
            }
        }
    }
    let label = site.as_str().split('.').next().unwrap_or_default();
    website
        .or(organization)
        .or_else(|| {
            let brand = normal(label).replace(' ', "");
            title_segments(&head.title)
                .find(|s| normal(s).replace(' ', "") == brand)
                .and_then(name)
        })
        .unwrap_or_else(|| {
            clip(
                if head.title.trim().is_empty() {
                    site.as_str()
                } else {
                    head.title.trim()
                },
                TITLE_BYTES,
            )
        })
}
