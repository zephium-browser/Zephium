use super::*;
use crate::doc::Head;

pub const MAX_SITE_NAME_BYTES: usize = 128;

pub fn title_segments(title: &str) -> impl Iterator<Item = &str> {
    title
        .split(['|', '–', '—', '·', '•', ':', '\\'])
        .flat_map(|s| s.split(" - "))
        .map(str::trim)
}

pub(super) fn site_title(head: &Head, site: &SiteKey) -> String {
    fn name(value: &str) -> Option<String> {
        let value = crate::flatten(&crate::clean_text(value));
        (!value.is_empty() && value.len() <= MAX_SITE_NAME_BYTES).then_some(value)
    }
    if let Some(value) = name(&head.site_name).or_else(|| name(&head.application_name)) {
        return value;
    }
    let mut website = None;
    let mut organization = None;
    linked::objects(head, |obj| {
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
    });
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
