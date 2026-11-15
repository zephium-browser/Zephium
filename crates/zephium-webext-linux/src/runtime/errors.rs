//! Bounded, rate-limited extension error records for the host.

use std::collections::VecDeque;

use zephium_core::ids::ExtensionInstallId;
use zephium_webext_runtime::context::ContextKind;

/// Records kept per install; older ones are dropped first.
pub const MAX_ERRORS_PER_INSTALL: usize = 64;
/// Longest message kept, in bytes.
pub const MAX_ERROR_MESSAGE_BYTES: usize = 1024;
/// Longest source URL kept, in bytes.
pub const MAX_ERROR_SOURCE_BYTES: usize = 2048;
const BURST: u64 = 16;
const PER_SECOND: u64 = 4;
const NANOS: u64 = 1_000_000_000;

/// Where an error came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ExtensionErrorKind {
    /// The background did not start: its script failed to load or evaluate,
    /// or its install handlers failed.
    Startup,
    /// An uncaught exception.
    Uncaught,
    /// A promise rejection nothing handled.
    UnhandledRejection,
    /// `console.error`, including the binding's own unchecked `lastError`
    /// and listener exception reports.
    Console,
    /// The background's web process died.
    Crash,
    /// The runtime disabled the background after repeated crashes.
    Disabled,
}

/// One error, for an "extension errors" view and for tests. All text is
/// extension-controlled: show it as text, never as markup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtensionError {
    /// The install.
    pub install: ExtensionInstallId,
    /// The extension id.
    pub extension_id: String,
    /// Where it came from.
    pub kind: ExtensionErrorKind,
    /// The context that reported it.
    pub context: ContextKind,
    /// The message, at most [`MAX_ERROR_MESSAGE_BYTES`].
    pub message: String,
    /// The script URL, when known.
    pub source: String,
    /// One-based line, or 0.
    pub line: u32,
    /// One-based column, or 0.
    pub column: u32,
}

pub(crate) fn truncate(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

/// One install's records and rate limit.
#[derive(Debug)]
pub(crate) struct ErrorLog {
    records: VecDeque<ExtensionError>,
    tokens: u64,
    refilled: u64,
    dropped: u64,
}

impl ErrorLog {
    pub(crate) fn new() -> Self {
        Self {
            records: VecDeque::new(),
            tokens: BURST,
            refilled: 0,
            dropped: 0,
        }
    }

    /// Keeps `error` unless the install is over its rate. Lifecycle records
    /// (startup, crash, disable) are never rate-limited.
    pub(crate) fn push(&mut self, error: ExtensionError, now_nanos: u64) -> bool {
        let elapsed = now_nanos.saturating_sub(self.refilled);
        let earned = elapsed.saturating_mul(PER_SECOND) / NANOS;
        if earned > 0 {
            self.tokens = (self.tokens + earned).min(BURST);
            self.refilled = now_nanos;
        }
        let lifecycle = matches!(
            error.kind,
            ExtensionErrorKind::Startup | ExtensionErrorKind::Crash | ExtensionErrorKind::Disabled
        );
        if !lifecycle {
            if self.tokens == 0 {
                self.dropped += 1;
                return false;
            }
            self.tokens -= 1;
        }
        if self.records.len() == MAX_ERRORS_PER_INSTALL {
            self.records.pop_front();
            self.dropped += 1;
        }
        self.records.push_back(error);
        true
    }

    pub(crate) fn records(&self) -> Vec<ExtensionError> {
        self.records.iter().cloned().collect()
    }

    #[cfg(test)]
    pub(crate) fn dropped(&self) -> u64 {
        self.dropped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn error(kind: ExtensionErrorKind) -> ExtensionError {
        ExtensionError {
            install: ExtensionInstallId::from(1),
            extension_id: "a".repeat(32),
            kind,
            context: ContextKind::Background,
            message: "boom".into(),
            source: String::new(),
            line: 0,
            column: 0,
        }
    }

    #[test]
    fn rate_limits_reports_but_never_lifecycle_records() {
        let mut log = ErrorLog::new();
        let kept = (0..100)
            .filter(|_| log.push(error(ExtensionErrorKind::Console), 0))
            .count();
        assert_eq!(kept, BURST as usize);
        assert!(log.push(error(ExtensionErrorKind::Crash), 0));
        assert!(log.push(error(ExtensionErrorKind::Console), NANOS));
        assert_eq!(log.dropped(), 100 - BURST);
    }

    #[test]
    fn keeps_a_bounded_window() {
        let mut log = ErrorLog::new();
        for index in 0..(MAX_ERRORS_PER_INSTALL as u64 * 2) {
            log.push(error(ExtensionErrorKind::Startup), index);
        }
        assert_eq!(log.records().len(), MAX_ERRORS_PER_INSTALL);
    }

    #[test]
    fn truncates_on_character_boundaries() {
        assert_eq!(truncate("aé", 2), "a");
        assert_eq!(truncate("abc", 8), "abc");
    }
}
