use super::*;

impl Downloads {
    pub(in crate::host) fn is_retired(&self, profile: ProfileId) -> bool {
        self.drained_profiles.borrow().contains(&profile)
    }

    pub(in crate::host) fn quiesce(
        self: &Rc<Self>,
        profile: Option<ProfileId>,
        done: Box<dyn FnOnce(bool) + Send>,
    ) {
        if let Some(profile) = profile {
            self.retired.borrow_mut().insert(profile);
        } else {
            self.stopping.set(true);
        }
        self.waiters.borrow_mut().push((profile, done));
        let ids: Vec<_> = self
            .active
            .borrow()
            .iter()
            .filter(|(_, transfer)| {
                profile.is_none_or(|profile| transfer.partition.profile() == profile)
            })
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            self.cancel(id, None);
        }
        let tokens: Vec<_> = self
            .calls
            .borrow()
            .iter()
            .filter(|(_, call)| profile.is_none_or(|profile| call.partition.profile() == profile))
            .map(|(token, _)| *token)
            .collect();
        for token in tokens {
            let request = self.calls.borrow_mut().remove(&token);
            let panel = self.directory_panels.borrow_mut().remove(&token);
            if let Some((panel, _lease)) = panel {
                platform::cancel_directory(&panel);
            }
            drop(request);
        }
        self.recent
            .borrow_mut()
            .retain(|(owner, _)| profile.is_some_and(|profile| owner.profile() != profile));
        self.preferences
            .borrow_mut()
            .retain(|owner, _| profile.is_some_and(|profile| *owner != profile));
        self.forgotten
            .borrow_mut()
            .retain(|(owner, _)| profile.is_some_and(|profile| *owner != profile));

        if let Some(profile) = profile {
            self.begin_retirement_recovery(profile);
        } else {
            self.begin_shutdown_recovery();
        }
        self.ensure_timer();
        #[cfg(target_os = "linux")]
        {
            // An idle manager has no deadline or worker reply to wake settlement.
            let weak = Rc::downgrade(self);
            gtk::glib::idle_add_local_once(move || {
                if let Some(manager) = weak.upgrade() {
                    manager.tick();
                }
            });
        }
    }
}
