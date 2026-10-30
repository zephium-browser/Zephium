//! Tabs each extension opened with `windows.create`. Extension windows open
//! as tabs of the focused window, so removing one closes those tabs and never
//! the window or the user's own tabs in it.

use std::cell::RefCell;
use std::collections::HashMap;

#[derive(Default)]
pub(crate) struct Opened(RefCell<HashMap<String, Vec<u64>>>);

impl Opened {
    pub(crate) fn record(&self, extension: &str, tab: u64) {
        let mut opened = self.0.borrow_mut();
        let tabs = opened.entry(extension.to_owned()).or_default();
        if !tabs.contains(&tab) {
            tabs.push(tab);
        }
    }

    /// Removes and returns the extension's tabs now in `window`. `window_of`
    /// gives a tab's current window, or `None` once it has closed, which also
    /// forgets it.
    pub(crate) fn take_in(
        &self,
        extension: &str,
        window: u64,
        window_of: impl Fn(u64) -> Option<u64>,
    ) -> Vec<u64> {
        let mut opened = self.0.borrow_mut();
        let Some(tabs) = opened.get_mut(extension) else {
            return Vec::new();
        };
        let mut taken = Vec::new();
        tabs.retain(|&tab| match window_of(tab) {
            Some(current) if current == window => {
                taken.push(tab);
                false
            }
            Some(_) => true,
            None => false,
        });
        if tabs.is_empty() {
            opened.remove(extension);
        }
        taken
    }
}

#[cfg(test)]
mod tests {
    use super::Opened;

    #[test]
    fn removing_a_window_takes_only_the_extensions_tabs_in_it() {
        let opened = Opened::default();
        opened.record("a", 10);
        opened.record("a", 11);
        opened.record("a", 10);
        opened.record("b", 12);
        // 10 is in window 1, 11 moved to window 2, 12 is another extension's.
        let window_of = |tab| match tab {
            10 | 12 => Some(1),
            11 => Some(2),
            _ => None,
        };
        assert_eq!(opened.take_in("a", 1, window_of), [10]);
        assert_eq!(opened.take_in("a", 1, window_of), Vec::<u64>::new());
        assert_eq!(opened.take_in("a", 2, window_of), [11]);
        assert_eq!(opened.take_in("b", 1, window_of), [12]);
    }

    #[test]
    fn closed_tabs_are_forgotten_and_never_returned() {
        let opened = Opened::default();
        opened.record("a", 10);
        assert!(opened.take_in("a", 1, |_| None).is_empty());
        assert!(opened.0.borrow().is_empty());
    }

    #[test]
    fn a_tab_whose_close_failed_is_taken_again_next_time() {
        let opened = Opened::default();
        opened.record("a", 10);
        assert_eq!(opened.take_in("a", 1, |_| Some(1)), [10]);
        // The browser refused the close; the window's close records it again.
        opened.record("a", 10);
        assert_eq!(opened.take_in("a", 1, |_| Some(1)), [10]);
    }

    #[test]
    fn an_extension_that_opened_nothing_closes_nothing() {
        let opened = Opened::default();
        opened.record("a", 10);
        assert!(opened.take_in("b", 1, |_| Some(1)).is_empty());
        assert_eq!(opened.take_in("a", 1, |_| Some(1)), [10]);
    }
}
