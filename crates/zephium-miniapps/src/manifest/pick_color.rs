/// Admits the colour forms a manifest is allowed to carry. Anything else is
/// dropped rather than passed through to a CSS sink.
pub fn pick_color(raw: Option<&str>) -> Option<String> {
    let value = raw?.trim().to_ascii_lowercase();
    let hex = value.strip_prefix('#')?;
    if !matches!(hex.len(), 3 | 4 | 6 | 8) || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    Some(format!("#{hex}"))
}
