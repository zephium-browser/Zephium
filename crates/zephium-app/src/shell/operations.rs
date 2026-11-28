

    pub(super) fn operation_set_app_setting(
        &mut self,
        key: String,
        value: String,
    ) -> OperationDisposition {
        if !zephium_core::preferences::value_allowed(&key, &value) {
            return operation_result(OperationOutcome::Rejected, OperationReason::InvalidInput);
        }
        #[cfg(feature = "work-planning")]
        if key == "work.screenshots" {
            let Some(change) = crate::work_screenshots::begin_preference(value == "true") else {
                return operation_result(
                    OperationOutcome::Rejected,
                    OperationReason::StoreConflict,
                );
            };
            return self.operation_set_work_screenshots(change);
        }
        let value = if key == "search.shortcuts" && !value.is_empty() {
            let Some(filtered) = zephium_core::search::ShortcutRegistry::from_setting(&value)
                .and_then(|registry| registry.to_setting())
            else {
                return operation_result(OperationOutcome::Rejected, OperationReason::InvalidInput);
            };
            filtered
        } else {
            value
        };
        if !self.store.set_app_setting(key.clone(), value.clone()) {
            return operation_result(
                OperationOutcome::Rejected,
                OperationReason::StoreAdmissionRejected,
            );
        }
        if key == "search.shortcuts" {
            self.search.shortcuts =
                zephium_core::search::ShortcutRegistry::from_setting(&value).unwrap_or_default();
            self.search.pending = None;
            self.search.context = None;
            self.search.results.clear();
        }
        if key == "search.custom-url" {
            self.search.custom_url = value.clone();
        }
        if key == "search.engine" {
            self.search.engine =
                zephium_core::search::SearchEngine::from_id(&value).unwrap_or_default();
        }
        if key == "search.history" {
            self.search.include_history = value == "true";
        }
        self.tab_preferences.apply(&key, &value);
        if self.proxy.apply(&key, &value) {
            let server = self.proxy.server();
            let enabled = server.is_some();
            self.engine.set_proxy(server);
            self.proxy.report();
            if enabled {
                self.stop_network_favicon_probes();
            }
        }
        if key == "tabs.automatic-pip" && value == "false" {
            self.engine.cancel_automatic_pip();
        }
        self.apply_time_setting(&key, &value);
        self.apply_performance_setting(&key, &value);
        self.apply_zoom_setting(&key, &value);
        // This projection is downstream of truthful store-queue admission.
        // The desktop composition root applies native theme state from this
        // signal, never optimistically from the IPC request itself.
        let command = if key == "appearance" {
            format!("theme.{value}")
        } else {
            format!("preference.{key}={value}")
        };
        (self.emit)(Projection::UiCommand(command));
        if value == "false"
            && self.browser_page.is_some_and(|(_, page)| {
                page.feature()
                    .is_some_and(|feature| key.strip_suffix(".enabled") == Some(feature))
            })
        {
            let _ = self.operation_show_browser_page(None);
        }
        operation_result(
            OperationOutcome::Deferred,
            OperationReason::StoreWorkPending,
        )
    }

    #[cfg(feature = "work-planning")]
    pub(super) fn operation_set_work_screenshots(
        &mut self,
        change: crate::work_screenshots::PreferenceChange,
    ) -> OperationDisposition {
        let confirmed = change.persist(self.store.as_ref());
        (self.emit)(Projection::UiCommand(format!(
            "preference.work.screenshots={}",
            change.effective()
        )));
        if confirmed {
            operation_result(OperationOutcome::Applied, OperationReason::MutationApplied)
        } else {
            operation_result(
                OperationOutcome::Rejected,
                OperationReason::StoreOutcomeUnknown,
            )
        }
    }

    pub(super) fn operation_run_command(&mut self, id: &str) -> OperationDisposition {
        // A shortcut means the page in front. Work's pane tab inside Work; a
        // window page such as Settings or History stands over the active tab,
        // so it has no tab to act on; otherwise the active tab, including a
        // browser-owned one like Extensions.
        let window_page = self.windows.focused().and_then(|window| {
            self.browser_page
                .filter(|(owner, _)| *owner == window.id)
                .map(|(_, page)| page)
        });
        let in_work = window_page == Some(crate::BrowserPage::Work);
        let active = match window_page {
            Some(crate::BrowserPage::Work) => self.work_pane_tab(),
            Some(_) => None,
            None => self.windows.focused().and_then(|window| window.active),
        };
        match id {
            "tab.new" => self.operation_open(),
            "window.newPrivate" => self.operation_enter_private(),
            // The desktop opens the launcher for a mini window instead; this
            // runs only while the private window is in front, whose pages
            // never leave it, so the quick page is a private tab.
            "window.newMini" => self.operation_open(),
            // Keys a mini window answers; the desktop hands them to the one
            // in front, and with none in front they do nothing.
            "mini.openInMain" | "mini.keepOnTop" => {
                operation_result(OperationOutcome::NoOp, OperationReason::StateUnchanged)
            }
            "window.closePrivate" => self.operation_close_private(),
            "window.togglePrivate" => self.operation_toggle_private(),
            "window.showRegular" => self.operation_show_regular(),
            "window.showPrivate" => self.operation_show_private(),
            "tab.close" if in_work => self.operation_work_pane_hide(),
            "tab.close" if window_page.is_some() => self.operation_show_browser_page(None),
            "tab.close" => active.map_or_else(
                || operation_result(OperationOutcome::NoOp, OperationReason::NoFocusedWindow),
                |id| self.operation_close(id),
            ),
            "work.pane.close" => self.operation_work_pane_hide(),
            "work.pane.openInBrowse" => active.filter(|_| in_work).map_or_else(
                || operation_result(OperationOutcome::NoOp, OperationReason::StateUnchanged),
                |id| self.operation_activate(id),
            ),
            "tab.reopen" => self.operation_reopen_closed_tab(None),
            _ if id.starts_with("tab.reopen=") => {
                match zephium_core::ids::ClosedSessionId::parse(&id["tab.reopen=".len()..]) {
                    Some(named) => self.operation_reopen_closed_tab(Some(named)),
                    None => {
                        operation_result(OperationOutcome::Rejected, OperationReason::InvalidInput)
                    }
                }
            }
            "tab.recent.next" => self.operation_recent_tab(false),
            "tab.recent.previous" => self.operation_recent_tab(true),
            "tab.next" | "tab.below" => self.operation_cycle_tab(1),
            "tab.previous" | "tab.above" => self.operation_cycle_tab(-1),
            "nav.back" => active.map_or_else(
                || operation_result(OperationOutcome::NoOp, OperationReason::NoFocusedWindow),
                |id| self.operation_history(id, false),
            ),
            "nav.forward" => active.map_or_else(
                || operation_result(OperationOutcome::NoOp, OperationReason::NoFocusedWindow),
                |id| self.operation_history(id, true),
            ),
            "nav.reload" => active.map_or_else(
                || operation_result(OperationOutcome::NoOp, OperationReason::NoFocusedWindow),
                |id| self.operation_reload(id),
            ),
            "nav.hardReload" => active.map_or_else(
                || operation_result(OperationOutcome::NoOp, OperationReason::NoFocusedWindow),
                |id| self.operation_reload_with(id, true),
            ),
            "nav.home" => match (active, self.tab_preferences.home.clone()) {
                (Some(id), Some(home)) => self.operation_navigate(id, home),
                (None, Some(home)) => {
                    let position = self.tab_preferences.new_position;
                    self.open_url_in_new_tab_at(home, position)
                }
                (_, None) => self.operation_open(),
            },
            "nav.stop" => active.map_or_else(
                || operation_result(OperationOutcome::NoOp, OperationReason::NoFocusedWindow),
                |id| {
                    let mut native = NativeWork::default();
                    native.record(self.engine.stop(id));
                    mutation_result(native)
                },
            ),
            "space.next" => self.operation_space(crate::SpaceCommand::Step(1)),
            "space.previous" => self.operation_space(crate::SpaceCommand::Step(-1)),
            "profile.next" => self.operation_step_profile(true),
            "profile.previous" => self.operation_step_profile(false),
            id if id.starts_with("space.select.") => id["space.select.".len()..]
                .parse::<usize>()
                .ok()
                .filter(|position| (1..=9).contains(position))
                .map_or_else(
                    || {
                        operation_result(
                            OperationOutcome::Rejected,
                            OperationReason::UnsupportedCommand,
                        )
                    },
                    |position| self.operation_select_space(position),
                ),
            "tab.select.last" => self.operation_select_tab(None),
            id if id.starts_with("tab.select.") => id["tab.select.".len()..]
                .parse::<usize>()
                .ok()
                .filter(|position| (1..=8).contains(position))
                .map_or_else(
                    || {
                        operation_result(
                            OperationOutcome::Rejected,
                            OperationReason::UnsupportedCommand,
                        )
                    },
                    |position| self.operation_select_tab(Some(position)),
                ),
            "bookmark.add" => self.operation_bookmark_page(None),
            "page.print" => active.map_or_else(
                || operation_result(OperationOutcome::NoOp, OperationReason::NoFocusedWindow),
                |id| {
                    let mut native = NativeWork::default();
                    native.record(self.engine.print(id));
                    mutation_result(native)
                },
            ),
            "page.viewSource" => self.operation_view_source(),
            "page.save" => self.operation_save_page(),
            "page.autofillPassword" => self.operation_autofill_password(),
            "page.devtools" => active.map_or_else(
                || operation_result(OperationOutcome::NoOp, OperationReason::NoFocusedWindow),
                |id| {
                    let mut native = NativeWork::default();
                    native.record(self.engine.open_devtools(id));
                    mutation_result(native)
                },
            ),
            "zoom.in" => self.operation_adjust_zoom(ZoomStep::In),
            "zoom.out" => self.operation_adjust_zoom(ZoomStep::Out),
            "zoom.reset" => self.operation_adjust_zoom(ZoomStep::Reset),
            "url.focus" => {
                (self.emit)(Projection::UiCommand("url.focus".into()));
                operation_result(OperationOutcome::Applied, OperationReason::MutationApplied)
            }
            _ => operation_result(
                OperationOutcome::Rejected,
                OperationReason::UnsupportedCommand,
            ),
        }
    }

    /// Reopens the newest closed tab of the focused profile, or the one named,
    /// in the space it came from. Nothing to restore is a no-op, not a
    /// failure.
    pub(super) fn operation_reopen_closed_tab(
        &mut self,
        named: Option<zephium_core::ids::ClosedSessionId>,
    ) -> OperationDisposition {
        if self.active_browser_page().is_some() {
            let command =
                named.map_or_else(|| "tab.reopen".into(), |id| format!("tab.reopen={id}"));
            self.browser_after_return = Some(Box::new(Command::Run(command)));
            return self.operation_show_browser_page(None);
        }
        let Some(profile) = self.windows.focused().map(|window| window.profile) else {
            return operation_result(OperationOutcome::Rejected, OperationReason::NoFocusedWindow);
        };
        match self.restore_recently_closed_tab(profile, named) {
            Some((_, native)) => mutation_result(native),
            None => operation_result(OperationOutcome::NoOp, OperationReason::MutationApplied),
        }
    }

    fn operation_cycle_tab(&mut self, step: isize) -> OperationDisposition {
        let Some(win) = self.windows.focused() else {
            return operation_result(OperationOutcome::NoOp, OperationReason::NoFocusedWindow);
        };
        let Some(active) = win.active else {
            return operation_result(OperationOutcome::NoOp, OperationReason::NoFocusedWindow);
        };
        let tabs = self.keyboard_tabs(win.space);
        let Some(position) = tabs.iter().position(|id| *id == active) else {
            return operation_result(OperationOutcome::Rejected, OperationReason::InvalidScope);
        };
        if tabs.len() < 2 {
            return operation_result(OperationOutcome::NoOp, OperationReason::StateUnchanged);
        }
        let next = (position as isize + step).rem_euclid(tabs.len() as isize) as usize;
        self.operation_activate(tabs[next])
    }

    /// Selects by sidebar position, counting from one. `None` is the last tab,
    /// however many there are.
    fn operation_select_tab(&mut self, position: Option<usize>) -> OperationDisposition {
        let Some(win) = self.windows.focused() else {
            return operation_result(OperationOutcome::NoOp, OperationReason::NoFocusedWindow);
        };
        let tabs = self.keyboard_tabs(win.space);
        let target = match position {
            Some(position) => position.checked_sub(1).and_then(|index| tabs.get(index)),
            None => tabs.last(),
        };
        match target {
            Some(id) if win.active != Some(*id) => self.operation_activate(*id),
            _ => operation_result(OperationOutcome::NoOp, OperationReason::StateUnchanged),
        }
    }
}
