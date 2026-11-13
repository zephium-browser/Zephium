use super::*;
use zephium_core::glance::{opens_in_glance, GlanceSource};
use zephium_core::windows::{Glance, WindowKind};

const PAGE_SHARE: f64 = 0.8;

pub(super) struct GlanceBackdrop {
    pub(super) window: WindowId,
    pub(super) parent: ItemId,
    pub(super) request: u64,
    pub(super) bytes: Option<Box<[u8]>>,
}

pub(super) fn glance_page(region: Rect) -> Rect {
    let width = (region.width * PAGE_SHARE).round();
    Rect::new(
        region.x + ((region.width - width) / 2.0).round(),
        region.y,
        width,
        region.height,
    )
}

fn backdrop_url(window: WindowId, parent: ItemId, request: u64) -> String {
    let prefix = if cfg!(target_os = "windows") {
        "http://zephium-preview.localhost"
    } else {
        "zephium-preview://localhost"
    };
    format!("{prefix}/{window}/{parent}?v={request}")
}

impl Shell {
    pub(super) fn wants_glance(
        &self,
        source: ItemId,
        url: &str,
        modifiers: zephium_core::glance::LinkModifiers,
    ) -> bool {
        if !cfg!(target_os = "windows") {
            return false;
        }
        let Some(win) = self.windows.focused() else {
            return false;
        };
        if win.kind != WindowKind::Main
            || win.active != Some(source)
            || win.splits.is_some()
            || self.incognito_profile(win.profile)
        {
            return false;
        }
        let Ok(target) = url::Url::parse(url) else {
            return false;
        };
        let Some(item) = self.items.get(source) else {
            return false;
        };
        let kind = match item.placement {
            Placement::Favorites { .. } => GlanceSource::Essential,
            Placement::Space {
                section: SpaceSection::Pinned,
                ..
            } => GlanceSource::Pinned,
            Placement::Space { .. } => GlanceSource::Ordinary,
        };
        opens_in_glance(kind, &target, self.tab_preferences.glance, modifiers)
    }

    pub(super) fn show_glance(&mut self, parent: ItemId, tab: ItemId) {
        let Some(window) = self.windows.focused().map(|win| win.id) else {
            return;
        };
        self.glance_serial = self.glance_serial.saturating_add(1);
        let request = self.glance_serial;
        self.glance_backdrop = Some(GlanceBackdrop {
            window,
            parent,
            request,
            bytes: None,
        });
        let url = self
            .items
            .tab(parent)
            .filter(|state| state.has_view())
            .and_then(|state| state.url.as_ref())
            .map(ToString::to_string);
        if let (Some(url), Some(queue)) = (url, self.self_queue.clone()) {
            self.engine.capture_backdrop(
                parent,
                url,
                Box::new(move |bytes| {
                    let _ = queue.try_push(Command::GlanceBackdropReady { request, bytes });
                }),
            );
        }
        if let Some(win) = self.windows.focused_mut() {
            win.glance = Some(Glance {
                tab,
                parent,
                backdrop: None,
            });
        }
        let effects = self.focus_tab(tab);
        self.commit(effects);
    }

    pub(super) fn glance_backdrop_ready(&mut self, request: u64, bytes: Option<Vec<u8>>) {
        let Some(pending) = self
            .glance_backdrop
            .as_mut()
            .filter(|pending| pending.request == request)
        else {
            return;
        };
        let Some(bytes) = bytes.filter(|bytes| !bytes.is_empty()) else {
            return;
        };
        pending.bytes = Some(bytes.into_boxed_slice());
        let (window, parent) = (pending.window, pending.parent);
        let Some(win) = self.windows.get_mut(window) else {
            return;
        };
        let Some(glance) = win.glance.as_mut().filter(|glance| glance.parent == parent) else {
            return;
        };
        glance.backdrop = Some(request);
        let _ = self.relayout();
    }

    pub(super) fn read_glance_backdrop(
        &self,
        window: WindowId,
        tab: ItemId,
        revision: u64,
    ) -> Option<Vec<u8>> {
        let pending = self.glance_backdrop.as_ref()?;
        let shown = self.windows.get(window)?.glance.as_ref()?;
        (pending.window == window
            && pending.parent == tab
            && pending.request == revision
            && shown.parent == tab
            && shown.backdrop == Some(revision))
        .then(|| pending.bytes.as_deref().map(<[u8]>::to_vec))
        .flatten()
    }

    pub(super) fn glance_layout(&self, region: Rect) -> Option<(Rect, zephium_ipc::GlanceLayout)> {
        let win = self.windows.focused()?;
        let glance = win.glance.as_ref()?;
        if win.active != Some(glance.tab) || self.items.tab(glance.parent).is_none() {
            return None;
        }
        let page = glance_page(region);
        let rect = |r: Rect| zephium_ipc::GlanceRect {
            x: r.x,
            y: r.y,
            width: r.width,
            height: r.height,
        };
        Some((
            page,
            zephium_ipc::GlanceLayout {
                tab: glance.tab.to_string(),
                parent: glance.parent.to_string(),
                region: rect(region),
                page: rect(page),
                backdrop: glance
                    .backdrop
                    .map(|request| backdrop_url(win.id, glance.parent, request)),
            },
        ))
    }

    pub(super) fn end_glance_unless(&mut self, tab: ItemId) {
        let Some(win) = self.windows.focused_mut() else {
            return;
        };
        if win.glance.as_ref().is_some_and(|glance| glance.tab != tab) {
            win.glance = None;
            self.glance_backdrop = None;
        }
    }

    pub(super) fn glance_action(&mut self, action: zephium_ipc::GlanceAction) {
        let Some(glance) = self
            .windows
            .focused()
            .and_then(|win| win.glance.clone().filter(|g| win.active == Some(g.tab)))
        else {
            return;
        };
        match action {
            zephium_ipc::GlanceAction::Close => {
                let _ = self.close(glance.tab);
            }
            zephium_ipc::GlanceAction::Expand => {
                if let Some(win) = self.windows.focused_mut() {
                    win.glance = None;
                }
                self.glance_backdrop = None;
                let _ = self.relayout();
            }
            zephium_ipc::GlanceAction::Split => {
                if let Some(win) = self.windows.focused_mut() {
                    win.glance = None;
                }
                self.glance_backdrop = None;
                self.split_glance(glance.parent, glance.tab);
            }
        }
    }

    fn split_glance(&mut self, parent: ItemId, tab: ItemId) {
        let effects = self.focus_tab(parent);
        self.commit(effects);
        let _ = self.operation_split(tab, Axis::Row);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_takes_the_middle_four_fifths_at_full_height() {
        let page = glance_page(Rect::new(300.0, 8.0, 1000.0, 700.0));
        assert_eq!(page, Rect::new(400.0, 8.0, 800.0, 700.0));
    }

    #[test]
    fn backdrop_addresses_name_the_window_page_and_revision() {
        let url = backdrop_url(3, ItemId::from(7), 12);
        assert!(url.ends_with(&format!("/3/{}?v=12", ItemId::from(7))));
    }
}
