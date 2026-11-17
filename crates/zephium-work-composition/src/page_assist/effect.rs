//! Whether a step took, judged from the page read after it against the read
//! before it. Dispatch only says the page was handed the step; the effect is
//! what the person would see changed: the value typed, the box ticked, the
//! option chosen, or for a press, the page changing at the control. A press
//! never takes credit for a place the page was seen changing on its own.
use std::collections::{HashMap, HashSet};

use zephium_page_assist::actions::Verb;
use zephium_page_assist::text;

use super::port::{Passage, Readout, Role};

/// Passages either side of a control within which a change counts as its own.
const NEAR: usize = 12;

/// What a read after a step shows.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Observed {
    /// The step's expected effect is on the page.
    Took,
    /// Nothing on the page changed.
    Nothing,
    /// Something changed, but not provably what the step names.
    Unclear,
}

/// A control as it stays recognisable across reads of one document: its
/// kind and name, and which of the controls so named it is.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Control {
    role: Role,
    text: String,
    nth: usize,
}

fn kind(role: Role) -> Role {
    match role {
        Role::Checkbox { .. } => Role::Checkbox { checked: false },
        role => role,
    }
}

impl Control {
    /// Passage `id` of `read`.
    pub(crate) fn of(read: &Readout, id: u16) -> Option<Self> {
        let at = read.passages.iter().position(|passage| passage.id == id)?;
        let passage = &read.passages[at];
        let nth = read.passages[..at]
            .iter()
            .filter(|other| kind(other.role) == kind(passage.role) && other.text == passage.text)
            .count();
        Some(Self {
            role: kind(passage.role),
            text: passage.text.clone(),
            nth,
        })
    }

    /// Where it is in `read`, if it is still there under the same name.
    pub(crate) fn find(&self, read: &Readout) -> Option<usize> {
        read.passages
            .iter()
            .enumerate()
            .filter(|(_, passage)| kind(passage.role) == self.role && passage.text == self.text)
            .nth(self.nth)
            .map(|(at, _)| at)
    }
}

type Signature<'a> = (Role, &'a str, Option<&'a str>);

fn signature(passage: &Passage) -> Signature<'_> {
    (
        passage.role,
        passage.text.as_str(),
        passage.value.as_deref(),
    )
}

/// Positions in `from` of passages `to` does not hold as often.
fn unmatched(from: &Readout, to: &Readout) -> Vec<usize> {
    let mut left: HashMap<Signature<'_>, usize> = HashMap::new();
    for passage in &to.passages {
        *left.entry(signature(passage)).or_default() += 1;
    }
    from.passages
        .iter()
        .enumerate()
        .filter_map(|(at, passage)| match left.get_mut(&signature(passage)) {
            Some(count) if *count > 0 => {
                *count -= 1;
                None
            }
            _ => Some(at),
        })
        .collect()
}

/// Whether two reads of one document differ anywhere: a sign the page is
/// still working, never a verdict on a step.
pub(crate) fn changed(before: &Readout, after: &Readout) -> bool {
    !unmatched(before, after).is_empty() || !unmatched(after, before).is_empty()
}

type Owned = (Role, String, Option<String>);

/// Where a changed passage sits, recognisable while its own text changes:
/// its kind and region, and the nearest unchanged passage either side.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct Slot {
    role: Role,
    region: u8,
    before: Option<Owned>,
    after: Option<Owned>,
}

fn slot(read: &Readout, changed: &HashSet<usize>, at: usize) -> Slot {
    let owned = |(_, passage): (usize, &Passage)| {
        (passage.role, passage.text.clone(), passage.value.clone())
    };
    let passages = &read.passages;
    Slot {
        role: kind(passages[at].role),
        region: passages[at].region,
        before: passages[..at]
            .iter()
            .enumerate()
            .rev()
            .find(|(i, _)| !changed.contains(i))
            .map(owned),
        after: passages
            .iter()
            .enumerate()
            .skip(at + 1)
            .find(|(i, _)| !changed.contains(i))
            .map(owned),
    }
}

/// Places on a document seen changing between two reads with no step
/// between them: a clock, a countdown, a rotating banner. A change there is
/// the page's own, never a step's effect.
#[derive(Clone, Debug, Default)]
pub(crate) struct Volatile(HashSet<Slot>);

impl Volatile {
    /// Learns from two reads of one document that no step came between.
    pub(crate) fn learn(&mut self, earlier: &Readout, later: &Readout) {
        if earlier.epoch != later.epoch {
            return;
        }
        for (read, other) in [(earlier, later), (later, earlier)] {
            let changed: HashSet<usize> = unmatched(read, other).into_iter().collect();
            for &at in &changed {
                self.0.insert(slot(read, &changed, at));
            }
        }
    }

    fn holds(&self, read: &Readout, changed: &HashSet<usize>, at: usize) -> bool {
        !self.0.is_empty() && self.0.contains(&slot(read, changed, at))
    }
}

/// Whether a change within reach of the control at `at` is not one of the
/// page's own.
fn near(changed: &[usize], at: Option<usize>, read: &Readout, volatile: &Volatile) -> bool {
    let set: HashSet<usize> = changed.iter().copied().collect();
    at.is_some_and(|at| {
        changed
            .iter()
            .any(|&changed| changed.abs_diff(at) <= NEAR && !volatile.holds(read, &set, changed))
    })
}

fn same(value: &str, wanted: &str) -> bool {
    let value = text::clean(value);
    let wanted = text::clean(wanted);
    value.eq_ignore_ascii_case(&wanted)
        // A long value comes back as a bounded preview of its start.
        || (value.chars().count() >= 32 && wanted.starts_with(value.trim_end_matches('…')))
}

/// Whether the control now holds what the step asked for; none when the read
/// cannot show it.
fn holds(verb: Verb, value: Option<&str>, passage: &Passage) -> Option<bool> {
    match (verb, passage.role) {
        (Verb::Fill, _) => Some(same(passage.value.as_deref()?, value?)),
        (Verb::Check, Role::Checkbox { checked }) => Some(checked),
        (Verb::Uncheck, Role::Checkbox { checked }) => Some(!checked),
        (Verb::Select, _) => {
            let wanted = value?;
            let shown = passage.value.as_deref().or(Some(passage.text.as_str()))?;
            Some(same(shown, wanted))
        }
        _ => None,
    }
}

/// Judges `verb` on `control` from the reads before and after it. A press
/// takes credit only for a change that is not in a `volatile` place.
pub(crate) fn judge(
    verb: Verb,
    value: Option<&str>,
    control: &Control,
    before: &Readout,
    after: &Readout,
    volatile: &Volatile,
) -> Observed {
    let removed = unmatched(before, after);
    let added = unmatched(after, before);
    let unchanged = removed.is_empty() && added.is_empty();
    let found = control.find(after);
    match verb {
        Verb::Fill | Verb::Check | Verb::Uncheck | Verb::Select => {
            match found.and_then(|at| holds(verb, value, &after.passages[at])) {
                Some(true) => Observed::Took,
                Some(false) if unchanged => Observed::Nothing,
                _ => Observed::Unclear,
            }
        }
        Verb::Scroll => Observed::Took,
        Verb::Click | Verb::Expand | Verb::Open | Verb::Submit => {
            let was = control.find(before);
            let gone: HashSet<usize> = removed.iter().copied().collect();
            if unchanged {
                Observed::Nothing
            } else if was.is_some_and(|at| found.is_none() && !volatile.holds(before, &gone, at))
                || near(&removed, was, before, volatile)
                || near(&added, found, after, volatile)
            {
                Observed::Took
            } else {
                Observed::Unclear
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::page_assist::port::FieldKind;

    /// The rule with no place known to change on its own.
    fn judge(
        verb: Verb,
        value: Option<&str>,
        control: &Control,
        before: &Readout,
        after: &Readout,
    ) -> Observed {
        super::judge(verb, value, control, before, after, &Volatile::default())
    }

    fn passage(id: u16, role: Role, text: &str, value: Option<&str>) -> Passage {
        Passage {
            id,
            role,
            text: text.into(),
            verbatim: true,
            value: value.map(str::to_owned),
            region: 0,
            href: None,
        }
    }

    fn read(passages: Vec<Passage>) -> Readout {
        Readout {
            epoch: 1,
            read: 1,
            title: None,
            passages,
        }
    }

    fn filler(from: u16, count: u16) -> Vec<Passage> {
        (from..from + count)
            .map(|id| passage(id, Role::Text, &format!("Line {id}"), None))
            .collect()
    }

    fn page(extra: Vec<Passage>) -> Readout {
        let mut passages = vec![passage(1, Role::Text, "Clock 10:00", None)];
        passages.extend(filler(2, 30));
        passages.push(passage(40, Role::Button, "Add to wishlist", None));
        passages.push(passage(41, Role::Field(FieldKind::Text), "Name", Some("")));
        passages.push(passage(42, Role::Checkbox { checked: true }, "Email", None));
        passages.push(passage(43, Role::Select, "Topic", Some("General")));
        passages.extend(extra);
        read(passages)
    }

    fn with(read: &Readout, id: u16, change: impl FnOnce(&mut Passage)) -> Readout {
        let mut read = read.clone();
        change(read.passages.iter_mut().find(|p| p.id == id).unwrap());
        read
    }

    #[test]
    fn a_press_that_changes_nothing_had_no_effect() {
        let before = page(vec![]);
        let control = Control::of(&before, 40).unwrap();
        assert_eq!(
            judge(Verb::Click, None, &control, &before, &before.clone()),
            Observed::Nothing
        );
        let elsewhere = with(&before, 1, |p| p.text = "Clock 10:01".into());
        assert_eq!(
            judge(Verb::Click, None, &control, &before, &elsewhere),
            Observed::Unclear,
            "a change far from the control is not its effect"
        );
        let mut beside = before.clone();
        beside
            .passages
            .insert(33, passage(90, Role::Text, "Added to your wishlist", None));
        assert_eq!(
            judge(Verb::Click, None, &control, &before, &beside),
            Observed::Took
        );
        let renamed = with(&before, 40, |p| p.text = "On your wishlist".into());
        assert_eq!(
            judge(Verb::Click, None, &control, &before, &renamed),
            Observed::Took
        );
    }

    #[test]
    fn a_value_step_took_only_when_the_control_holds_it() {
        let before = page(vec![]);
        let name = Control::of(&before, 41).unwrap();
        let typed = with(&before, 41, |p| p.value = Some("Alex Moreno".into()));
        assert_eq!(
            judge(Verb::Fill, Some("Alex  Moreno"), &name, &before, &typed),
            Observed::Took
        );
        assert_eq!(
            judge(Verb::Fill, Some("Alex Moreno"), &name, &before, &before),
            Observed::Nothing
        );
        let other = with(&before, 41, |p| p.value = Some("(555) 123".into()));
        assert_eq!(
            judge(Verb::Fill, Some("555123"), &name, &before, &other),
            Observed::Unclear
        );
        let unread = with(&before, 41, |p| p.value = None);
        assert_eq!(
            judge(Verb::Fill, Some("x"), &name, &unread, &unread),
            Observed::Unclear,
            "a field whose value is never read cannot prove anything"
        );
        let long = "please resend my March invoice and the April one too";
        let preview = with(&before, 41, |p| p.value = Some(long[..40].to_owned() + "…"));
        assert_eq!(
            judge(Verb::Fill, Some(long), &name, &before, &preview),
            Observed::Took
        );

        let email = Control::of(&before, 42).unwrap();
        let off = with(&before, 42, |p| p.role = Role::Checkbox { checked: false });
        assert_eq!(
            judge(Verb::Uncheck, None, &email, &before, &off),
            Observed::Took
        );
        assert_eq!(
            judge(Verb::Uncheck, None, &email, &before, &before),
            Observed::Nothing
        );
        assert_eq!(
            judge(Verb::Check, None, &email, &before, &before),
            Observed::Took,
            "already as asked"
        );

        let topic = Control::of(&before, 43).unwrap();
        let billing = with(&before, 43, |p| p.value = Some("Billing".into()));
        assert_eq!(
            judge(Verb::Select, Some("billing"), &topic, &before, &billing),
            Observed::Took
        );
        assert_eq!(
            judge(Verb::Select, Some("Billing"), &topic, &before, &before),
            Observed::Nothing
        );
    }

    #[test]
    fn a_press_never_takes_credit_for_a_place_that_changes_on_its_own() {
        let before = page(vec![]);
        // A clock just above the button, inside its reach.
        let clocked = |time: &str| {
            let mut read = before.clone();
            read.passages.insert(
                31,
                passage(91, Role::Text, &format!("Ends in {time}"), None),
            );
            read
        };
        let (earlier, now) = (clocked("0:59"), clocked("0:58"));
        let mut volatile = Volatile::default();
        volatile.learn(&earlier, &now);
        let control = Control::of(&now, 40).unwrap();
        let ticked = clocked("0:57");
        assert_eq!(
            super::judge(
                Verb::Click,
                None,
                &control,
                &now,
                &ticked,
                &Volatile::default()
            ),
            Observed::Took,
            "without a step-free pair the rule is as before"
        );
        assert_eq!(
            super::judge(Verb::Click, None, &control, &now, &ticked, &volatile),
            Observed::Unclear,
            "the clock ticking is not the click's effect"
        );
        let relabeled = with(&ticked, 40, |p| p.text = "On your wishlist".into());
        assert_eq!(
            super::judge(Verb::Click, None, &control, &now, &relabeled, &volatile),
            Observed::Took,
            "the button's own label changed"
        );
        let mut noted = ticked.clone();
        noted
            .passages
            .insert(34, passage(92, Role::Text, "Saved: 1", None));
        assert_eq!(
            super::judge(Verb::Click, None, &control, &now, &noted, &volatile),
            Observed::Took,
            "a count appeared beside the button"
        );
        let name = Control::of(&now, 41).unwrap();
        let typed = with(&ticked, 41, |p| p.value = Some("Alex".into()));
        assert_eq!(
            super::judge(Verb::Fill, Some("Alex"), &name, &now, &typed, &volatile),
            Observed::Took,
            "a value step is judged on the control alone"
        );
    }

    #[test]
    fn controls_are_found_again_by_kind_name_and_order() {
        let before = read(vec![
            passage(1, Role::Button, "Add to cart", None),
            passage(2, Role::Button, "Add to cart", None),
        ]);
        let second = Control::of(&before, 2).unwrap();
        let mut after = read(vec![passage(7, Role::Text, "New banner", None)]);
        after
            .passages
            .extend(before.passages.iter().cloned().map(|mut p| {
                p.id += 10;
                p
            }));
        assert_eq!(second.find(&after), Some(2));
        assert_eq!(Control::of(&before, 9), None);
    }
}
