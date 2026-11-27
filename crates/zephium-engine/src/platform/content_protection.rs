//! Whether one view's native filters hold what its content policy asks for.
//! A policy can be installed and still not filter a view: WebView2 changes a
//! view's request filters on every pause and resume, and those calls can fail.
//! The view's protection then shows as degraded until a later change or a
//! retry on its next navigation puts the filters back.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

/// What changed for the view, as the shell is told it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ProtectionReport {
    /// The view is resumed and some or all of its filters are missing.
    pub(crate) degraded: bool,
    /// Filters a retired cohort could not remove; they stay on the view until
    /// a retry takes them back.
    pub(crate) stranded_filters: usize,
}

#[derive(Clone, Default)]
pub(crate) struct ViewProtection(Rc<Inner>);

type OnChange = Rc<dyn Fn(ProtectionReport)>;

#[derive(Default)]
struct Inner {
    report: Cell<ProtectionReport>,
    on_change: RefCell<Option<OnChange>>,
    stranded: RefCell<Vec<usize>>,
    retry: RefCell<Option<Weak<dyn Fn()>>>,
}

impl ViewProtection {
    pub(crate) fn on_change(&self, report: impl Fn(ProtectionReport) + 'static) {
        self.0.on_change.replace(Some(Rc::new(report)));
    }

    /// Whether the live cohort's filters leave the resumed view unprotected.
    pub(crate) fn settle(&self, degraded: bool) {
        self.publish(ProtectionReport {
            degraded,
            ..self.0.report.get()
        });
    }

    /// A filter a retired cohort could not remove from the view.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub(crate) fn strand(&self, filter: usize) {
        let count = {
            let mut stranded = self.0.stranded.borrow_mut();
            stranded.push(filter);
            stranded.len()
        };
        self.publish(ProtectionReport {
            stranded_filters: count,
            ..self.0.report.get()
        });
    }

    /// The stranded filters, for a retry to remove; it strands again any it
    /// cannot.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub(crate) fn take_stranded(&self) -> Vec<usize> {
        let stranded = std::mem::take(&mut *self.0.stranded.borrow_mut());
        if !stranded.is_empty() {
            self.publish(ProtectionReport {
                stranded_filters: 0,
                ..self.0.report.get()
            });
        }
        stranded
    }

    /// What a retry runs: the live cohort's, held only while it lives.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub(crate) fn set_retry(&self, retry: Weak<dyn Fn()>) {
        self.0.retry.replace(Some(retry));
    }

    /// Puts a degraded or stranded view's filters right, on its next
    /// main-frame commit.
    pub(crate) fn retry(&self) {
        let report = self.0.report.get();
        if !report.degraded && report.stranded_filters == 0 {
            return;
        }
        let retry = self.0.retry.borrow().as_ref().and_then(Weak::upgrade);
        if let Some(retry) = retry {
            retry();
        }
    }

    fn publish(&self, report: ProtectionReport) {
        if self.0.report.replace(report) == report {
            return;
        }
        // Cloned out, so a report that re-enters this view never meets a held
        // borrow.
        let on_change = self.0.on_change.borrow().clone();
        if let Some(on_change) = on_change {
            on_change(report);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recorded(protection: &ViewProtection) -> Rc<RefCell<Vec<ProtectionReport>>> {
        let reports = Rc::new(RefCell::new(Vec::new()));
        let sink = reports.clone();
        protection.on_change(move |report| sink.borrow_mut().push(report));
        reports
    }

    #[test]
    fn only_changes_are_reported() {
        let protection = ViewProtection::default();
        let reports = recorded(&protection);
        protection.settle(false);
        protection.settle(true);
        protection.settle(true);
        protection.settle(false);
        let degraded: Vec<bool> = reports.borrow().iter().map(|r| r.degraded).collect();
        assert_eq!(degraded, [true, false]);
    }

    #[test]
    fn stranded_filters_are_reported_until_a_retry_takes_them() {
        let protection = ViewProtection::default();
        let reports = recorded(&protection);
        protection.strand(3);
        protection.strand(5);
        assert_eq!(reports.borrow().last().unwrap().stranded_filters, 2);
        assert_eq!(protection.take_stranded(), [3, 5]);
        assert_eq!(reports.borrow().last().unwrap().stranded_filters, 0);
        assert!(protection.take_stranded().is_empty());
    }

    #[test]
    fn a_retry_runs_only_for_a_view_that_needs_one_and_a_live_cohort() {
        let protection = ViewProtection::default();
        let runs = Rc::new(Cell::new(0));
        let counter = runs.clone();
        let retry: Rc<dyn Fn()> = Rc::new(move || counter.set(counter.get() + 1));
        protection.set_retry(Rc::downgrade(&retry));
        protection.retry();
        assert_eq!(runs.get(), 0, "a whole view needs no retry");
        protection.settle(true);
        protection.retry();
        assert_eq!(runs.get(), 1);
        drop(retry);
        protection.retry();
        assert_eq!(runs.get(), 1, "a retired cohort's retry is gone");
    }
}
