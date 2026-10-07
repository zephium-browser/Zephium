//! A renderer restart must settle before another can begin, and must never
//! become an endless crash/reload loop over a long-running browser session.

#[derive(Default)]
pub(crate) struct Recovery {
    attempts: u8,
    pending: Option<u8>,
    terminal: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Action {
    Reload(u8),
    Ignore,
    Stop,
}

impl Recovery {
    #[cfg(all(test, target_os = "windows"))]
    pub(crate) fn settled_attempts(&self) -> u8 {
        if self.pending.is_none() && !self.terminal {
            self.attempts
        } else {
            0
        }
    }

    pub(crate) fn crashed(&mut self) -> Action {
        if self.terminal || self.pending.is_some() {
            return Action::Ignore;
        }
        if self.attempts == 2 {
            self.terminal = true;
            return Action::Stop;
        }
        self.attempts += 1;
        self.pending = Some(self.attempts);
        Action::Reload(self.attempts)
    }

    pub(crate) fn ready(&mut self) -> bool {
        !self.terminal && self.pending.take().is_some()
    }

    pub(crate) fn timed_out(&mut self, attempt: u8) -> bool {
        if self.pending != Some(attempt) {
            return false;
        }
        self.stop()
    }

    pub(crate) fn stop(&mut self) -> bool {
        self.pending = None;
        !std::mem::replace(&mut self.terminal, true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_events_do_not_overlap_reloads_or_reset_the_lifetime_budget() {
        let mut recovery = Recovery::default();
        assert!(!recovery.ready());
        assert_eq!(recovery.crashed(), Action::Reload(1));
        assert_eq!(recovery.crashed(), Action::Ignore);
        assert!(recovery.ready());
        assert_eq!(recovery.crashed(), Action::Reload(2));
        assert!(recovery.ready());
        assert_eq!(recovery.crashed(), Action::Stop);
        assert_eq!(recovery.crashed(), Action::Ignore);
        assert!(!recovery.ready());
    }

    #[test]
    fn an_old_deadline_cannot_stop_a_recovered_or_newer_renderer() {
        let mut recovery = Recovery::default();
        assert_eq!(recovery.crashed(), Action::Reload(1));
        assert!(recovery.ready());
        assert!(!recovery.timed_out(1));
        assert_eq!(recovery.crashed(), Action::Reload(2));
        assert!(!recovery.timed_out(1));
        assert!(recovery.timed_out(2));
        assert!(!recovery.timed_out(2));
        assert!(!recovery.ready());
        assert_eq!(recovery.crashed(), Action::Ignore);
    }

    #[test]
    fn browser_exit_stops_even_an_in_flight_recovery_once() {
        let mut recovery = Recovery::default();
        assert_eq!(recovery.crashed(), Action::Reload(1));
        assert!(recovery.stop());
        assert!(!recovery.stop());
        assert!(!recovery.ready());
        assert!(!recovery.timed_out(1));
    }
}
