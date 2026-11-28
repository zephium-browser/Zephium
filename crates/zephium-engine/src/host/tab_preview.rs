use super::*;

impl EngineHost {
    pub(crate) fn capture_tab_preview(
        &self,
        id: ItemId,
        url: String,
        full: bool,
        done: Box<dyn FnOnce(Option<Vec<u8>>) + Send>,
    ) {
        if self.dormant.contains(&id) {
            done(None);
            return;
        }
        let Some(view) = self.views.get(&id) else {
            done(None);
            return;
        };
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
            full,
            Box::new(move |bytes| {
                done(bytes.filter(|_| navigation.matches_committed_snapshot(epoch, &committed)));
            }),
        );
    }
}
