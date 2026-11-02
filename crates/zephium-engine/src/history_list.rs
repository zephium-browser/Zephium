//! A view's back/forward history as the DevTools protocol reports it. WebView2
//! has no list API of its own, and `Page.getNavigationHistory` is the
//! browser's own record: it follows every way a page moves through history,
//! which events observed from outside cannot tell apart.

use zephium_core::ports::engine::MAX_HISTORY_SIDE;

const MAX_RESPONSE_BYTES: usize = 1 << 20;
const MAX_ENTRIES: usize = 1024;
const MAX_ENTRY_BYTES: usize = 8 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct NavigationEntry {
    pub id: i64,
    pub title: String,
    pub url: String,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct NavigationHistory {
    entries: Vec<NavigationEntry>,
    current: usize,
}

/// Reads the answer to `Page.getNavigationHistory`; None unless it is well
/// formed and bounded.
pub(crate) fn parse(response: &str) -> Option<NavigationHistory> {
    if response.len() > MAX_RESPONSE_BYTES {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(response).ok()?;
    let current = usize::try_from(value.get("currentIndex")?.as_u64()?).ok()?;
    let raw = value.get("entries")?.as_array()?;
    if raw.len() > MAX_ENTRIES || current >= raw.len() {
        return None;
    }
    let text = |entry: &serde_json::Value, key: &str| {
        entry
            .get(key)
            .and_then(serde_json::Value::as_str)
            .filter(|text| text.len() <= MAX_ENTRY_BYTES)
            .unwrap_or_default()
            .to_owned()
    };
    let entries = raw
        .iter()
        .map(|entry| {
            Some(NavigationEntry {
                id: entry.get("id")?.as_i64()?,
                title: text(entry, "title"),
                url: text(entry, "url"),
            })
        })
        .collect::<Option<Vec<_>>>()?;
    Some(NavigationHistory { entries, current })
}

impl NavigationHistory {
    /// The entries within reach of the menu, as (title, url) with the index of
    /// the current one among them.
    pub(crate) fn window(&self) -> (Vec<(String, String)>, usize) {
        let start = self.current.saturating_sub(MAX_HISTORY_SIDE);
        let end = (self.current + MAX_HISTORY_SIDE + 1).min(self.entries.len());
        (
            self.entries[start..end]
                .iter()
                .map(|entry| (entry.title.clone(), entry.url.clone()))
                .collect(),
            self.current - start,
        )
    }

    /// The id of the entry `offset` steps from the current one, but only if it
    /// is still at `url`: the page may have moved since the list was shown.
    pub(crate) fn entry_at(&self, offset: i32, url: &str) -> Option<i64> {
        if offset == 0 {
            return None;
        }
        let at = i64::try_from(self.current)
            .ok()?
            .checked_add(i64::from(offset))?;
        let entry = self.entries.get(usize::try_from(at).ok()?)?;
        (entry.url == url).then_some(entry.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(current: usize, urls: &[&str]) -> String {
        let entries: Vec<_> = urls
            .iter()
            .enumerate()
            .map(|(at, url)| {
                serde_json::json!({
                    "id": 100 + at,
                    "url": url,
                    "userTypedURL": url,
                    "title": format!("Page {at}"),
                    "transitionType": "link",
                })
            })
            .collect();
        serde_json::json!({ "currentIndex": current, "entries": entries }).to_string()
    }

    #[test]
    fn the_browsers_own_list_is_read_with_its_entry_ids() {
        let history = parse(&response(
            1,
            &["about:blank", "https://a.example/", "https://b.example/"],
        ))
        .unwrap();
        let (entries, current) = history.window();
        assert_eq!(current, 1);
        assert_eq!(
            entries[2],
            ("Page 2".to_owned(), "https://b.example/".to_owned())
        );
        assert_eq!(history.entry_at(1, "https://b.example/"), Some(102));
        assert_eq!(history.entry_at(-1, "about:blank"), Some(100));
    }

    #[test]
    fn an_entry_is_only_found_where_the_list_shown_had_it() {
        let history = parse(&response(
            1,
            &[
                "https://a.example/",
                "https://b.example/",
                "https://c.example/",
            ],
        ))
        .unwrap();
        // The page moved on to somewhere else at that step.
        assert_eq!(history.entry_at(1, "https://elsewhere.example/"), None);
        assert_eq!(history.entry_at(0, "https://b.example/"), None);
        assert_eq!(history.entry_at(-2, "https://a.example/"), None);
        assert_eq!(history.entry_at(2, "https://c.example/"), None);
        assert_eq!(history.entry_at(i32::MIN, "https://a.example/"), None);
        assert_eq!(history.entry_at(i32::MAX, "https://a.example/"), None);
    }

    #[test]
    fn only_the_entries_near_the_current_one_reach_the_menu() {
        let urls: Vec<String> = (0..400)
            .map(|at| format!("https://e{at}.example/"))
            .collect();
        let urls: Vec<&str> = urls.iter().map(String::as_str).collect();
        let history = parse(&response(200, &urls)).unwrap();
        let (entries, current) = history.window();
        assert_eq!(entries.len(), 2 * MAX_HISTORY_SIDE + 1);
        assert_eq!(current, MAX_HISTORY_SIDE);
        assert_eq!(entries[current].1, "https://e200.example/");
        let near = parse(&response(0, &["https://a.example/", "https://b.example/"])).unwrap();
        assert_eq!(near.window().1, 0);
    }

    #[test]
    fn a_malformed_or_oversized_answer_is_no_list() {
        assert!(parse("").is_none());
        assert!(parse("{}").is_none());
        assert!(parse(&response(5, &["https://a.example/"])).is_none());
        assert!(parse(r#"{"currentIndex":0,"entries":[{"url":"https://a.example/"}]}"#).is_none());
        assert!(parse(&format!(
            r#"{{"currentIndex":0,"entries":[],"pad":"{}"}}"#,
            "x".repeat(MAX_RESPONSE_BYTES)
        ))
        .is_none());
        let long = "https://a.example/".to_owned() + &"x".repeat(MAX_ENTRY_BYTES);
        let history = parse(&response(0, &[long.as_str()])).unwrap();
        assert_eq!(history.window().0[0].1, "");
    }
}
