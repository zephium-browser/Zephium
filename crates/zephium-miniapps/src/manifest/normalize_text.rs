/// Collapses whitespace and rejects control characters while keeping the
/// visible text. The result is bounded in characters, never bytes, so a
/// multi-byte name cannot be sliced mid-codepoint.
pub fn normalize_text(raw: &str, max_chars: usize) -> Option<String> {
    let mut output = String::with_capacity(raw.len().min(256));
    for word in raw.split_whitespace() {
        if word.chars().any(char::is_control) {
            return None;
        }
        if !output.is_empty() {
            output.push(' ');
        }
        output.push_str(word);
    }
    let text: String = output.chars().take(max_chars).collect();
    let text = text.trim().to_owned();
    (!text.is_empty()).then_some(text)
}
