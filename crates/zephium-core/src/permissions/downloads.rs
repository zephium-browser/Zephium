//! Whether a site may save files to this computer, as Safari asks: the first
//! download from a site waits for the person, and the answer is kept per
//! profile and site. A download nobody clicked is confirmed even on a site
//! that is allowed.

use super::PageOrigin;

/// Sites one profile keeps a download decision for.
pub const MAX_DOWNLOAD_SITES: usize = 512;

/// How long a download waits for the person before it is cancelled.
pub const DOWNLOAD_PERMISSION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5 * 60);

/// What one site may do with downloads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DownloadGate {
    /// Saved without asking.
    Open,
    /// The person is asked first.
    Ask,
    /// Refused without asking.
    Refuse,
}

/// The gate for one download. `remembered` is the site's saved decision,
/// true for Allow and false for Block. `clicked` is false only when the
/// engine reports that nobody clicked.
pub const fn download_gate(remembered: Option<bool>, clicked: bool) -> DownloadGate {
    match (remembered, clicked) {
        (Some(false), _) => DownloadGate::Refuse,
        (Some(true), true) => DownloadGate::Open,
        _ => DownloadGate::Ask,
    }
}

/// A download the person is asked about, under the address field of the tab
/// it came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DownloadPermissionPrompt {
    /// Per-tab, never zero.
    pub request: u32,
    /// The site the tab shows; the decision is kept for it.
    pub site: PageOrigin,
    /// The suggested file name, already made safe.
    pub filename: String,
    /// The site is already allowed; only this download is asked about.
    pub allowed: bool,
    /// The engine reports that nobody clicked anything that downloads.
    pub unclicked: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_blocked_site_never_asks_and_an_allowed_one_asks_only_without_a_click() {
        assert_eq!(download_gate(Some(false), true), DownloadGate::Refuse);
        assert_eq!(download_gate(Some(false), false), DownloadGate::Refuse);
        assert_eq!(download_gate(Some(true), true), DownloadGate::Open);
        assert_eq!(download_gate(Some(true), false), DownloadGate::Ask);
        assert_eq!(download_gate(None, true), DownloadGate::Ask);
        assert_eq!(download_gate(None, false), DownloadGate::Ask);
    }
}
