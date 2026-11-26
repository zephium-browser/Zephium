use super::*;

fn admitted_browser_size(size: Size) -> Option<Size> {
    let edge = |value: f64| value.is_finite() && (1.0..=32_768.0).contains(&value);
    (edge(size.width) && edge(size.height)).then_some(size)
}

impl Shell {
    pub(super) fn open_browser_window(&mut self) -> zephium_ipc::OperationDisposition {
        let Some((profile, space, size, mode, metrics)) = self.windows.focused().map(|window| {
            (
                window.profile,
                window.space,
                window.size,
                window.mode,
                window.metrics,
            )
        }) else {
            return operation_result(
                zephium_ipc::OperationOutcome::Rejected,
                zephium_ipc::OperationReason::NoFocusedWindow,
            );
        };
        if !self.bootstrapped || self.profile_deletion_quarantines(profile) {
            return operation_result(
                zephium_ipc::OperationOutcome::Rejected,
                zephium_ipc::OperationReason::InvalidScope,
            );
        }
        let window = self.windows.create_unfocused(
            zephium_core::windows::WindowKind::Main,
            profile,
            space,
            size,
        );
        if let Some(native) = self.windows.get_mut(window) {
            native.mode = mode;
            native.metrics = metrics;
        }
        (self.emit)(Projection::BrowserWindowOpened(
            zephium_ipc::BrowserWindowOpenedView {
                window,
                width: size.width,
                height: size.height,
            },
        ));
        operation_result(
            zephium_ipc::OperationOutcome::Deferred,
            zephium_ipc::OperationReason::NativeWorkPending,
        )
    }

    pub(super) fn browser_window_attached(&mut self, window: WindowId, size: Size) {
        let Some(size) = admitted_browser_size(size) else {
            self.browser_window_closed(window);
            return;
        };
        let Some(native) = self.windows.get_mut(window) else {
            let _ = self.engine.release_window(window);
            return;
        };
        if native.kind != zephium_core::windows::WindowKind::Main {
            let _ = self.engine.release_window(window);
            return;
        }
        native.size = size;
        native.visible = true;
        self.windows.focus(window);
        self.window_visible = true;
        self.window_focused = true;
        let _ = self.operation_open();
        self.project_items();
        let _ = self.relayout();
    }

    pub(super) fn browser_window_resized(&mut self, window: WindowId, size: Size) {
        let Some(size) = admitted_browser_size(size) else {
            return;
        };
        let Some(native) = self.windows.get_mut(window) else {
            return;
        };
        if native.kind != zephium_core::windows::WindowKind::Main {
            return;
        }
        native.size = size;
        if self
            .windows
            .focused()
            .is_some_and(|focused| focused.id == window)
        {
            self.refresh_sidebar_peek();
            let _ = self.relayout();
        }
    }

    pub(super) fn browser_window_focused(&mut self, window: WindowId, focused: bool) {
        if focused {
            let Some(native) = self.windows.get_mut(window) else {
                return;
            };
            if native.kind != zephium_core::windows::WindowKind::Main {
                return;
            }
            native.visible = true;
            self.windows.focus(window);
            self.window_visible = true;
            self.window_focused = true;
            self.project_items();
            let _ = self.relayout();
        } else if self
            .windows
            .focused()
            .is_some_and(|native| native.id == window)
        {
            self.window_focused = false;
            self.cancel_page_permission_if_not_foreground();
        }
    }

    pub(super) fn browser_window_visible(&mut self, window: WindowId, visible: bool) {
        let Some(native) = self.windows.get_mut(window) else {
            return;
        };
        if native.kind != zephium_core::windows::WindowKind::Main {
            return;
        }
        native.visible = visible;
        self.window_visible = self.windows.iter().any(|candidate| {
            candidate.kind == zephium_core::windows::WindowKind::Main && candidate.visible
        });
        if !visible {
            if self
                .windows
                .focused()
                .is_some_and(|candidate| candidate.id == window)
            {
                if self
                    .windows
                    .get(window)
                    .is_some_and(|candidate| candidate.sidebar_peek)
                {
                    self.peek_sidebar(false, false);
                }
                self.drop_divider();
                if let Some(active) = self
                    .windows
                    .get(window)
                    .and_then(|candidate| candidate.active)
                {
                    self.touch(active);
                }
                self.cancel_page_permission_if_not_foreground();
            }
            let _ = self.engine.set_content(window, None, None);
        } else if self
            .windows
            .focused()
            .is_some_and(|candidate| candidate.id == window)
        {
            let _ = self.relayout();
        }
        self.maintain_views();
    }

    pub(super) fn browser_window_closed(&mut self, window: WindowId) {
        let Some(native) = self.windows.close(window) else {
            return;
        };
        if native.kind != zephium_core::windows::WindowKind::Main {
            return;
        }
        let _ = self.engine.release_window(window);
        self.window_visible = self.windows.iter().any(|candidate| {
            candidate.kind == zephium_core::windows::WindowKind::Main && candidate.visible
        });
        self.window_focused = self.windows.focused().is_some();
        (self.emit)(Projection::BrowserWindowClosed(window));
        if self.windows.focused().is_some() {
            self.project_items();
            let _ = self.relayout();
        }
    }
}
