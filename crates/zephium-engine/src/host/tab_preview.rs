use super::*;

impl EngineHost {
    pub(crate) fn capture_tab_preview(
        &self,
        id: ItemId,
        url: String,
        done: Box<dyn FnOnce(Option<Vec<u8>>) + Send>,
    ) {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        if self.dormant.contains(&id) {
            done(None);
            return;
        }
        let Some(view) = self.views.get(&id) else {
            done(None);
            return;
        };
        #[cfg(target_os = "linux")]
        if !view.presentable || !view.presentation_permit.load(Ordering::Acquire) {
            done(None);
            return;
        }
        let navigation = view.navigation.clone();
        let Some((epoch, committed)) = navigation
            .committed_snapshot()
            .filter(|(_, committed)| *committed == url)
        else {
            done(None);
            return;
        };
        crate::platform::imp::tab_preview::capture(
            &view.view,
            Box::new(move |bytes| {
                done(bytes.filter(|_| navigation.matches_committed_snapshot(epoch, &committed)));
            }),
        );
    }
}
