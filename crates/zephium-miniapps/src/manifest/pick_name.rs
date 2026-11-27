use crate::manifest::normalize_text::normalize_text;

/// Prefers the full name, then the short name, then a readable host. A site
/// without any name still installs under a name its owner would recognise.
pub fn pick_name(name: Option<&str>, short_name: Option<&str>, host: Option<&str>) -> String {
    for candidate in [name, short_name] {
        if let Some(value) = candidate.and_then(|value| normalize_text(value, 80)) {
            return value;
        }
    }
    host.map(|host| host.trim_start_matches("www.").to_owned())
        .filter(|host| !host.is_empty())
        .unwrap_or_else(|| "Web app".to_owned())
}
