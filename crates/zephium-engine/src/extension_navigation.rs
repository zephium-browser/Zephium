//! Extension document frame admission, independent of native default dialogs.
pub(crate) fn allows_frame(extension: &str, target: &str) -> bool {
    if target == "about:blank" {
        return true;
    }
    url::Url::parse(target).is_ok_and(|url| {
        url.scheme() == "chrome-extension"
            && url.host_str() == Some(extension)
            && url.username().is_empty()
            && url.password().is_none()
            && url.port().is_none()
    })
}

/// Every unresolved construction debt keeps the navigation guard alive.
/// Overflow is an untracked native ownership obligation and retains forever.
pub(crate) fn retain_failed_construction<D, O: 'static>(
    debts: impl IntoIterator<Item = D>,
    owner: std::rc::Rc<O>,
    overflowed: bool,
    mut retain: impl FnMut(D, std::rc::Rc<O>),
) {
    for debt in debts {
        retain(debt, owner.clone());
    }
    if overflowed {
        std::mem::forget(owner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const OWN: &str = "abcdefghijklmnopabcdefghijklmnop";

    #[test]
    fn unresolved_construction_debts_keep_native_navigation_guards_until_proven_closed() {
        use std::{cell::Cell, rc::Rc};
        struct Guard(Rc<Cell<usize>>);
        impl Drop for Guard {
            fn drop(&mut self) {
                self.0.set(self.0.get() + 1);
            }
        }
        let drops = Rc::new(Cell::new(0));
        let mut pending = Vec::new();
        retain_failed_construction(
            [1, 2],
            Rc::new(Guard(drops.clone())),
            false,
            |debt, owner| pending.push((debt, owner)),
        );
        assert_eq!(drops.get(), 0);
        pending.pop();
        assert_eq!(
            drops.get(),
            0,
            "one unresolved controller still owns the handlers"
        );
        pending.clear();
        assert_eq!(drops.get(), 1);
        retain_failed_construction(
            Vec::<u8>::new(),
            Rc::new(Guard(drops.clone())),
            true,
            |_, _| panic!("no tracked debt"),
        );
        assert_eq!(
            drops.get(),
            1,
            "untracked overflow cannot revoke deny handlers"
        );
    }

    #[test]
    fn extension_frames_keep_own_resources_without_delegating_native_schemes() {
        assert!(allows_frame(OWN, "about:blank"));
        assert!(allows_frame(
            OWN,
            &format!("chrome-extension://{OWN}/frame.html")
        ));
        for target in [
            "search-ms:query=private",
            "file:///private",
            "javascript:alert(1)",
            "https://example.test/",
            "chrome-extension://other/frame.html",
            "chrome-extension://user@abcdefghijklmnopabcdefghijklmnop/frame",
            "not a URL",
        ] {
            assert!(!allows_frame(OWN, target), "{target}");
        }
    }
}
