//! Host-pull fallback for extension-owned views whose web process has no
//! usable module. It follows the engine's approved pull pattern: the host
//! authors every evaluated body, results are primitive strings whose length
//! the body checks before returning, and the host checks it again.
//!
//! Contract with the page glue, in the view's main frame and page world:
//! `__zephiumPullOutbox()` returns (or resolves to) the next envelope as
//! standard base64, or `''`; `__zephiumPullInbox(base64)` receives one.

use std::time::Duration;

use base64::Engine as _;
use zephium_webext_runtime::limits::MAX_MESSAGE_BYTES_RUNTIME_OWNED;

/// First and longest poll interval while nothing is queued.
pub const PULL_MIN_INTERVAL: Duration = Duration::from_millis(16);
pub const PULL_MAX_INTERVAL: Duration = Duration::from_millis(500);

/// Longest base64 text of one runtime-owned envelope.
pub(crate) const MAX_PULL_UNITS: usize = (MAX_MESSAGE_BYTES_RUNTIME_OWNED as usize).div_ceil(3) * 4;

pub(crate) fn pull_body() -> String {
    format!(
        r#"const take = globalThis.__zephiumPullOutbox;
if (typeof take !== 'function') return '';
const value = await take();
if (typeof value !== 'string' || value.length > {MAX_PULL_UNITS}) return '';
return value;"#
    )
}

pub(crate) const DELIVER_BODY: &str = r#"const give = globalThis.__zephiumPullInbox;
if (typeof give !== 'function') return false;
give(bytes);
return true;"#;

/// Decodes a pulled result. `None` for anything but bounded strict base64.
pub(crate) fn decode(text: &[u8]) -> Option<Vec<u8>> {
    if text.len() > MAX_PULL_UNITS {
        return None;
    }
    base64::engine::general_purpose::STANDARD.decode(text).ok()
}

pub(crate) fn encode(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

pub(crate) fn next_interval(current: Duration) -> Duration {
    (current * 2).clamp(PULL_MIN_INTERVAL, PULL_MAX_INTERVAL)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pulled_text_is_bounded_strict_base64() {
        assert_eq!(decode(b"AAEC"), Some(vec![0, 1, 2]));
        assert_eq!(decode(b""), Some(Vec::new()));
        for bad in [&b"AAE"[..], b"AA=C", b"A\0EC", b"AAEC\n", b"****"] {
            assert_eq!(decode(bad), None, "{bad:?}");
        }
        assert!(decode(&vec![b'A'; MAX_PULL_UNITS + 4]).is_none());
        let largest = vec![7u8; MAX_MESSAGE_BYTES_RUNTIME_OWNED as usize];
        let text = encode(&largest);
        assert!(text.len() <= MAX_PULL_UNITS);
        assert_eq!(
            decode(text.as_bytes()).map(|bytes| bytes.len()),
            Some(largest.len())
        );
        assert!(pull_body().contains(&format!("value.length > {MAX_PULL_UNITS}")));
    }

    #[test]
    fn polling_backs_off_within_bounds() {
        let mut interval = PULL_MIN_INTERVAL;
        for _ in 0..10 {
            interval = next_interval(interval);
            assert!(interval <= PULL_MAX_INTERVAL);
        }
        assert_eq!(interval, PULL_MAX_INTERVAL);
        assert_eq!(next_interval(Duration::ZERO), PULL_MIN_INTERVAL);
    }
}
