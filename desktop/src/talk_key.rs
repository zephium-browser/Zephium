use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TalkSignal {
    Down,
    Up,
    Cancel,
    Confirm,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Claimed {
    Escape = 1,
    Enter = 2,
}

pub(crate) struct TalkKey {
    down: AtomicBool,
    claims: AtomicU8,
    sink: OnceLock<Box<dyn Fn(TalkSignal) + Send + Sync>>,
}

impl TalkKey {
    pub(crate) fn new() -> Self {
        Self {
            down: AtomicBool::new(false),
            claims: AtomicU8::new(0),
            sink: OnceLock::new(),
        }
    }

    pub(crate) fn attach(&self, sink: impl Fn(TalkSignal) + Send + Sync + 'static) {
        let _ = self.sink.set(Box::new(sink));
    }

    pub(crate) fn claim(&self, escape: bool, enter: bool) {
        self.claims.store(
            (u8::from(escape) * Claimed::Escape as u8) | (u8::from(enter) * Claimed::Enter as u8),
            Ordering::Release,
        );
    }

    pub(crate) fn press(&self) {
        if !self.down.swap(true, Ordering::AcqRel) {
            self.send(TalkSignal::Down);
        }
    }

    pub(crate) fn release(&self) {
        if self.down.swap(false, Ordering::AcqRel) {
            self.send(TalkSignal::Up);
        }
    }

    pub(crate) fn claimed(&self, key: Claimed) -> bool {
        self.claims.load(Ordering::Acquire) & key as u8 != 0
    }

    pub(crate) fn take(&self, key: Claimed) {
        if self.claimed(key) {
            self.send(match key {
                Claimed::Escape => TalkSignal::Cancel,
                Claimed::Enter => TalkSignal::Confirm,
            });
        }
    }

    fn send(&self, signal: TalkSignal) {
        if let Some(sink) = self.sink.get() {
            sink(signal);
        }
    }
}

#[cfg(any(target_os = "macos", test))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Modifiers {
    pub(crate) meta: bool,
    pub(crate) ctrl: bool,
    pub(crate) alt: bool,
    pub(crate) shift: bool,
    pub(crate) other: bool,
}

#[cfg(any(target_os = "macos", test))]
impl Modifiers {
    fn contains(self, required: Self) -> bool {
        (!required.meta || self.meta)
            && (!required.ctrl || self.ctrl)
            && (!required.alt || self.alt)
            && (!required.shift || self.shift)
    }
}

#[cfg(any(target_os = "macos", test))]
#[derive(Clone, Copy, Debug)]
pub(crate) enum KeyEvent {
    Down,
    Up,
    FlagsChanged,
}

#[cfg(any(target_os = "macos", test))]
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct KeyAction {
    pub(crate) consume: bool,
    pub(crate) signal: Option<TalkSignal>,
}

#[cfg(any(target_os = "macos", test))]
#[derive(Default)]
pub(crate) struct Capture {
    held: Option<(u16, Modifiers)>,
    down: bool,
}

#[cfg(any(target_os = "macos", test))]
impl Capture {
    pub(crate) fn event(
        &mut self,
        kind: KeyEvent,
        key_code: u16,
        modifiers: Modifiers,
        repeat: bool,
        talk: bool,
        claimed: Option<Claimed>,
    ) -> KeyAction {
        match kind {
            KeyEvent::Up if self.held.is_some_and(|(code, _)| code == key_code) => {
                let mut action = self.resign();
                action.consume = true;
                action
            }
            KeyEvent::FlagsChanged
                if self.down
                    && self
                        .held
                        .is_some_and(|(_, required)| !modifiers.contains(required)) =>
            {
                // Keep swallowing the physical key until its key-up.
                self.down = false;
                KeyAction {
                    consume: false,
                    signal: Some(TalkSignal::Up),
                }
            }
            KeyEvent::Down => {
                let mut action = KeyAction::default();
                if self.held.is_some_and(|(code, _)| code == key_code) {
                    action.consume = true;
                } else if let Some(key) = claimed.filter(|_| modifiers == Modifiers::default()) {
                    action.consume = true;
                    if !repeat {
                        action.signal = Some(match key {
                            Claimed::Escape => TalkSignal::Cancel,
                            Claimed::Enter => TalkSignal::Confirm,
                        });
                    }
                } else if talk {
                    action.consume = true;
                    if !repeat && self.held.is_none() {
                        self.held = Some((key_code, modifiers));
                        self.down = true;
                        action.signal = Some(TalkSignal::Down);
                    }
                }
                action
            }
            _ => KeyAction::default(),
        }
    }

    pub(crate) fn resign(&mut self) -> KeyAction {
        self.held = None;
        KeyAction {
            consume: false,
            signal: std::mem::take(&mut self.down).then_some(TalkSignal::Up),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn attached() -> (TalkKey, Arc<Mutex<Vec<TalkSignal>>>) {
        let key = TalkKey::new();
        let signals = Arc::new(Mutex::new(Vec::new()));
        let received = signals.clone();
        key.attach(move |signal| received.lock().unwrap().push(signal));
        (key, signals)
    }

    #[test]
    fn only_one_release_follows_a_press() {
        let (key, signals) = attached();
        key.release();
        assert!(signals.lock().unwrap().is_empty());
        key.press();
        key.press();
        key.release();
        key.release();
        assert_eq!(*signals.lock().unwrap(), [TalkSignal::Down, TalkSignal::Up]);
    }

    #[test]
    fn claims_are_independent_and_can_be_revoked() {
        let (key, signals) = attached();
        key.take(Claimed::Escape);
        key.take(Claimed::Enter);
        assert!(signals.lock().unwrap().is_empty());
        key.claim(true, false);
        assert!(key.claimed(Claimed::Escape));
        assert!(!key.claimed(Claimed::Enter));
        key.take(Claimed::Escape);
        key.take(Claimed::Enter);
        key.claim(false, true);
        key.take(Claimed::Escape);
        key.take(Claimed::Enter);
        key.claim(false, false);
        key.take(Claimed::Enter);
        assert_eq!(
            *signals.lock().unwrap(),
            [TalkSignal::Cancel, TalkSignal::Confirm]
        );
    }

    #[test]
    fn no_receiver_is_safe_and_only_the_first_receiver_is_installed() {
        let key = TalkKey::new();
        key.release();
        key.press();
        key.release();
        key.claim(true, true);
        key.take(Claimed::Escape);
        key.take(Claimed::Enter);
        let (key, signals) = attached();
        key.attach(|_| panic!("second receiver must not be installed"));
        key.press();
        assert_eq!(*signals.lock().unwrap(), [TalkSignal::Down]);
    }

    const CHORD: Modifiers = Modifiers {
        meta: true,
        shift: true,
        ctrl: false,
        alt: false,
        other: false,
    };

    fn press(capture: &mut Capture, code: u16, modifiers: Modifiers) {
        assert_eq!(
            capture.event(KeyEvent::Down, code, modifiers, false, true, None),
            KeyAction {
                consume: true,
                signal: Some(TalkSignal::Down),
            }
        );
    }

    #[test]
    fn physical_key_up_ends_the_hold_regardless_of_current_modifiers() {
        let mut capture = Capture::default();
        press(&mut capture, 0, CHORD);
        assert_eq!(
            capture.event(KeyEvent::Up, 1, CHORD, false, false, None),
            KeyAction::default()
        );
        assert_eq!(
            capture.event(KeyEvent::Up, 0, Modifiers::default(), false, false, None),
            KeyAction {
                consume: true,
                signal: Some(TalkSignal::Up),
            }
        );
        assert_eq!(capture.resign(), KeyAction::default());
    }

    #[test]
    fn releasing_any_chord_modifier_ends_once_and_keeps_repeats_from_the_page() {
        for modifiers in [
            Modifiers {
                meta: false,
                ..CHORD
            },
            Modifiers {
                shift: false,
                ..CHORD
            },
        ] {
            let mut capture = Capture::default();
            press(&mut capture, 0, CHORD);
            assert_eq!(
                capture.event(KeyEvent::FlagsChanged, 56, modifiers, false, false, None),
                KeyAction {
                    consume: false,
                    signal: Some(TalkSignal::Up),
                }
            );
            assert_eq!(
                capture.event(KeyEvent::Down, 0, modifiers, true, false, None),
                KeyAction {
                    consume: true,
                    signal: None
                }
            );
            assert_eq!(
                capture.event(KeyEvent::Up, 0, modifiers, false, false, None),
                KeyAction {
                    consume: true,
                    signal: None
                }
            );
            assert_eq!(capture.resign(), KeyAction::default());
        }
    }

    #[test]
    fn repeats_and_duplicate_downs_never_start_another_hold() {
        let mut capture = Capture::default();
        for repeat in [false, true] {
            if !repeat {
                press(&mut capture, 0, CHORD);
            }
            assert_eq!(
                capture.event(KeyEvent::Down, 0, CHORD, repeat, true, None),
                KeyAction {
                    consume: true,
                    signal: None
                }
            );
        }
        assert_eq!(capture.resign().signal, Some(TalkSignal::Up));
        assert_eq!(capture.resign(), KeyAction::default());
        assert_eq!(
            capture.event(KeyEvent::Down, 0, CHORD, true, true, None),
            KeyAction {
                consume: true,
                signal: None
            }
        );
        press(&mut capture, 0, CHORD);
    }

    #[test]
    fn window_or_app_resign_releases_once_and_allows_a_fresh_press() {
        let mut capture = Capture::default();
        press(&mut capture, 0, CHORD);
        assert_eq!(capture.resign().signal, Some(TalkSignal::Up));
        assert_eq!(capture.resign(), KeyAction::default());
        assert_eq!(
            capture.event(KeyEvent::Up, 0, CHORD, false, false, None),
            KeyAction::default()
        );
        press(&mut capture, 0, CHORD);
    }

    #[test]
    fn rebound_chords_track_only_their_own_modifiers() {
        let modifiers = Modifiers {
            ctrl: true,
            alt: true,
            ..Modifiers::default()
        };
        let mut capture = Capture::default();
        press(&mut capture, 9, modifiers);
        assert_eq!(
            capture.event(KeyEvent::FlagsChanged, 56, modifiers, false, false, None),
            KeyAction::default()
        );
        assert_eq!(
            capture
                .event(
                    KeyEvent::FlagsChanged,
                    59,
                    Modifiers {
                        ctrl: false,
                        ..modifiers
                    },
                    false,
                    false,
                    None
                )
                .signal,
            Some(TalkSignal::Up)
        );
        capture.resign();
        press(&mut capture, 9, modifiers);
        assert_eq!(
            capture
                .event(
                    KeyEvent::FlagsChanged,
                    58,
                    Modifiers {
                        alt: false,
                        ..modifiers
                    },
                    false,
                    false,
                    None
                )
                .signal,
            Some(TalkSignal::Up)
        );
    }

    #[test]
    fn only_plain_claimed_key_downs_are_taken() {
        let mut capture = Capture::default();
        for (code, key, signal) in [
            (53, Claimed::Escape, TalkSignal::Cancel),
            (36, Claimed::Enter, TalkSignal::Confirm),
            (76, Claimed::Enter, TalkSignal::Confirm),
        ] {
            assert_eq!(
                capture.event(
                    KeyEvent::Down,
                    code,
                    Modifiers::default(),
                    false,
                    false,
                    None
                ),
                KeyAction::default()
            );
            assert_eq!(
                capture.event(
                    KeyEvent::Down,
                    code,
                    Modifiers::default(),
                    false,
                    false,
                    Some(key)
                ),
                KeyAction {
                    consume: true,
                    signal: Some(signal)
                }
            );
            assert_eq!(
                capture.event(
                    KeyEvent::Down,
                    code,
                    Modifiers::default(),
                    true,
                    false,
                    Some(key)
                ),
                KeyAction {
                    consume: true,
                    signal: None
                }
            );
            assert_eq!(
                capture.event(
                    KeyEvent::Up,
                    code,
                    Modifiers::default(),
                    false,
                    false,
                    Some(key)
                ),
                KeyAction::default()
            );
            for modifiers in [
                CHORD,
                Modifiers {
                    ctrl: true,
                    ..Modifiers::default()
                },
                Modifiers {
                    alt: true,
                    ..Modifiers::default()
                },
                Modifiers {
                    other: true,
                    ..Modifiers::default()
                },
            ] {
                assert_eq!(
                    capture.event(KeyEvent::Down, code, modifiers, false, false, Some(key)),
                    KeyAction::default()
                );
            }
        }
    }
}
