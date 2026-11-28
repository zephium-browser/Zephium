           .map(|window| {
                    let point = self
                        .convertPoint_fromView(window.mouseLocationOutsideOfEventStream(), None);
                    self.hot_rect().is_some_and(|hot| NSPointInRect(point, hot))
                })
                .unwrap_or(false);
            self.ivars().reveal.pointer(inside);
            self.reveal_fullscreen();
        } else {
            self.ivars().reveal.set_fullscreen(false);
            // Leaving fullscreen, AppKit lays the titlebar out afresh at its
            // own position. The inset is applied as the content view draws,
            // which a window that comes back at the same size never asks for.
            self.reinset();
            self.setNeedsDisplay(true);
            self.resync(false);
        }
    }

    /// Leaving fullscreen, AppKit animates the titlebar back with the
    /// controls at its own position, a few points off the inset, and lays
    /// them out again until the animation ends. They are hidden for the
    /// length of it and come back, already in place, when it is done.
    fn leaving(&self) {
        self.tell_chrome(false);
        for button in self.buttons() {
            fade(&button, 0.0, 0.0);
        }
    }

    /// Puts the controls back at their inset now. The inset is applied as the
    /// content view draws; asking for that draw at once, rather than on the
    /// next pass, keeps AppKit's own position from showing for a frame.
    fn reinset(&self) {
        if let Some(content) = self.window().and_then(|window| window.contentView()) {
            content.setNeedsDisplay(true);
            content.displayIfNeeded();
        }
    }

    /// Tells the chrome whether the controls are gone for fullscreen. Sent
    /// as the transition begins either way, so the chrome moves with AppKit
    /// rather than after it; repeating the same news is harmless.
    fn tell_chrome(&self, fullscreen: bool) {
        if let Some(chrome) = self.ivars().chrome.borrow().as_ref() {
            let _ = chrome.eval(format!(
                "window.dispatchEvent(new CustomEvent('zephium:fullscreen',{{detail:{fullscreen}}}))"
            ));
        }
    }

    fn observe(&self, window: &NSWindow) {
        let center = NSNotificationCenter::defaultCenter();
        let weak = Weak::from_retained(&self.retain());
        let add = |name, action: fn(&WindowControls)| {
            let weak = weak.clone();
            let block = RcBlock::new(move |_: NonNull<NSNotification>| {
                if let Some(view) = weak.load() {
                    action(&view);
                }
            });
            // SAFETY: window notifications are posted on the main thread, the
            // block holds the view weakly, and the token is removed when the
            // view is deallocated.
            let token = unsafe {
                center.addObserverForName_object_queue_usingBlock(
                    Some(name),
                    Some(AsRef::<AnyObject>::as_ref(window)),
                    None,
                    &block,
                )
            };
            self.ivars().observers.borrow_mut().push(token);
        };
        // SAFETY: immutable framework constants.
        unsafe {
            add(NSWindowWillEnterFullScreenNotification, |view| {
                view.suspend(true)
            });
            add(NSWindowWillExitFullScreenNotification, |view| {
                view.leaving()
            });
            add(NSWindowDidExitFullScreenNotification, |view| {
                view.suspend(false)
            });
            add(NSWindowDidDeminiaturizeNotification, |view| {
                view.resync(false)
            });
            add(NSWindowDidBecomeKeyNotification, |view| view.resync(true));
            add(NSWindowDidResignKeyNotification, |view| view.resync(true));
        }
    }
}

/// Whether this is macOS 26 or later, whose titlebar lays the controls out
/// differently from the releases before it.
fn redesigned_titlebar() -> bool {
    NSProcessInfo::processInfo()
        .operatingSystemVersion()
        .majorVersion
        >= 26
}

/// How much lower than the configured position the controls are put. The
/// configured one is measured on the releases before macOS 26; from 26 the
/// same inset lays them out 3pt higher in the titlebar, above the head they
/// line up with.
pub fn inset_lift() -> f64 {
    if redesigned_titlebar() {
        3.0
    } else {
        0.0
    }
}

/// The disc a control draws within its frame.
fn disc(frame: NSRect) -> NSRect {
    if redesigned_titlebar() {
        return frame;
    }
    NSInsetRect(
        frame,
        (frame.size.width - LEGACY_DISC) / 2.0,
        (frame.size.height - LEGACY_DISC) / 2.0,
    )
}

/// Moves a view's layer to `opacity`, continuing from wherever an unfinished
/// fade has got to so a pointer crossing back mid-fade never jumps.
fn fade(view: &NSView, opacity: f32, duration: f64) {
    let Some(layer) = view.layer() else {
        return;
    };
    // SAFETY: a main-thread read of the layer's in-flight rendering state.
    let from =
        unsafe { layer.presentationLayer() }.map_or(layer.opacity(), |shown| shown.opacity());
    CATransaction::begin();
    CATransaction::setDisableActions(true);
    layer.setOpacity(opacity);
    layer.removeAnimationForKey(ns_string!("opacity"));
    if duration > 0.0 && (from - opacity).abs() > f32::EPSILON {
        let animation = CABasicAnimation::animationWithKeyPath(Some(ns_string!("opacity")));
        // SAFETY: NSNumber is the value type Core Animation expects for a
        // scalar key path.
        unsafe {
            animation.setFromValue(Some(&NSNumber::new_f32(from)));
            animation.setToValue(Some(&NSNumber::new_f32(opacity)));
        }
        animation.setDuration(duration);
        // The interface's own --ease-out, so the controls settle the way the
        // chrome around them does.
        animation.setTimingFunction(Some(&CAMediaTimingFunction::functionWithControlPoints(
            0.22, 1.0, 0.36, 1.0,
        )));
        layer.addAnimation_forKey(&animation, Some(ns_string!("opacity")));
    }
    CATransaction::commit();
}

/// Installs the resting controls on a browser window. Idempotent, and a no-op
/// off the main thread or on a window without standard buttons.
pub fn install(window: &WebviewWindow) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let Ok(pointer) = window.ns_window() else {
        return;
    };
    // SAFETY: `ns_window` returns Tao's live NSWindow, borrowed for this
    // main-thread call only.
    let ns_window = unsafe { &*(pointer as *const NSWindow) };
    let Some(close) = ns_window.standardWindowButton(NSWindowButton::CloseButton) else {
        return;
    };
    // SAFETY: a main-thread read of the live close button's parent.
    let Some(parent) = (unsafe { close.superview() }) else {
        return;
    };
    if parent
        .subviews()
        .iter()
        .any(|view| view.isKindOfClass(WindowControls::class()))
    {
        return;
    }
    let controls = WindowControls::new(mtm, parent.bounds());
    controls.setAutoresizingMask(
        NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
    );
    controls.setLayerContentsRedrawPolicy(NSViewLayerContentsRedrawPolicy::DuringViewResize);
    parent.addSubview_positioned_relativeTo(&controls, NSWindowOrderingMode::Below, Some(&close));
    controls.ivars().chrome.replace(Some(window.clone()));
    controls.observe(ns_window);
    if ns_window
        .styleMask()
        .contains(NSWindowStyleMask::FullScreen)
    {
        controls.suspend(true);
    } else {
        controls.settle();
    }
}

/// Puts the controls away while the sidebar is hidden, which leaves the page
/// under the corner they rest in: invisible buttons there would still take
/// its clicks. They come back, at their inset, with the sidebar.
pub fn set_tucked(window: &WebviewWindow, tucked: bool) {
    if MainThreadMarker::new().is_none() {
        return;
    }
    let Ok(pointer) = window.ns_window() else {
        return;
    };
    // SAFETY: Tao's live NSWindow, borrowed for this main-thread call only.
    let ns_window = unsafe { &*(pointer as *const NSWindow) };
    let Some(close) = ns_window.standardWindowButton(NSWindowButton::CloseButton) else {
        return;
    };
    // SAFETY: a main-thread read of the live close button's parent.
    let Some(parent) = (unsafe { close.superview() }) else {
        return;
    };
    let Some(controls) = parent
        .subviews()
        .iter()
        .find_map(|view| view.downcast_ref::<WindowControls>().map(|c| c.retain()))
    else {
        return;
    };
    if controls.isHidden() == tucked {
        return;
    }
    for button in controls.buttons() {
        button.setHidden(tucked);
    }
    controls.setHidden(tucked);
    if !tucked && !controls.ivars().suspended.get() {
        controls.reinset();
        controls.resync(false);
        // They come in with the card rather than ahead of it; they leave at
        // once, so a pointer that has moved on is not left with them.
        if let Some(layer) = controls.layer() {
            if !NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion() {
                let fade = CABasicAnimation::animationWithKeyPath(Some(ns_string!("opacity")));
                // SAFETY: opacity animates between NSNumber values.
                unsafe {
                    fade.setFromValue(Some(&NSNumber::new_f64(0.0)));
                    fade.setToValue(Some(&NSNumber::new_f64(1.0)));
                }
                fade.setDuration(TUCK_SECONDS);
                fade.setTimingFunction(Some(&CAMediaTimingFunction::functionWithControlPoints(
                    0.2, 0.0, 0.0, 1.0,
                )));
                layer.addAnimation_forKey(&fade, Some(ns_string!("zephium.controls.untuck")));
            }
        }
    }
}

const TUCK_SECONDS: f64 = 0.22;

/// How far a hidden sidebar's card stands in from the window's side, and so
/// how far its controls move in with it to keep their place in its head. Its
/// head keeps the window's own height, so they stay at theirs.
const CARD_INSET: f64 = 8.0;

/// Moves the controls into the hidden sidebar's card, or back to their own
/// inset. Tao re-insets them from the content view's draw, which is how they
/// go back; until it draws, they keep the card's place.
pub fn set_in_card(window: &WebviewWindow, in_card: bool) {
    if MainThreadMarker::new().is_none() {
        return;
    }
    let Ok(pointer) = window.ns_window() else {
        return;
    };
    // SAFETY: Tao's live NSWindow, borrowed for this main-thread call only.
    place_in_card(unsafe { &*(pointer as *const NSWindow) }, in_card);
}

fn place_in_card(ns_window: &NSWindow, in_card: bool) {
    let buttons: Vec<_> = [
        NSWindowButton::CloseButton,
        NSWindowButton::MiniaturizeButton,
        NSWindowButton::ZoomButton,
    ]
    .into_iter()
    .filter_map(|kind| ns_window.standardWindowButton(kind))
    .collect();
    let Some(close) = buttons.first() else {
        return;
    };
    // SAFETY: main-thread reads of the live close button's ancestors.
    let Some(parent) = (unsafe { close.superview() }) else {
        return;
    };
    let Some(container) = (unsafe { parent.superview() }) else {
        return;
    };
    let controls = parent
        .subviews()
        .iter()
        .find_map(|view| view.downcast_ref::<WindowControls>().map(|c| c.retain()));
    // From Tao's own inset each time, so asking twice moves them once.
    if let Some(controls) = controls.as_ref() {
        controls.ivars().inset.set(in_card, || controls.reinset());
        controls.setNeedsDisplay(true);
    }
    if !in_card {
        return;
    }
    // Tao's inset puts the first button at its x and grows the titlebar to
    // the button's height plus its y; the card's place is that, moved over.
    let x = close.frame().origin.x + CARD_INSET;
    let y = container.frame().size.height - close.frame().size.height;
    let height = close.frame().size.height + y;
    let mut bar = container.frame();
    bar.size.height = height;
    bar.origin.y = ns_window.frame().size.height - height;
    container.setFrame(bar);
    let space = buttons
        .get(1)
        .map_or(20.0, |next| next.frame().origin.x - close.frame().origin.x);
    for (index, button) in buttons.iter().enumerate() {
        let mut origin = button.frame().origin;
        origin.x = x + index as f64 * space;
        button.setFrameOrigin(origin);
    }
    if let Some(controls) = controls {
        controls.setNeedsDisplay(true);
        // SAFETY: the view re-reads its buttons' rects for its hover area.
        let _: () = unsafe { msg_send![&*controls, updateTrackingAreas] };
    }
}

#[cfg(test)]
mod tests {
    use super::InsetState;

    #[test]
    fn focus_refresh_replays_the_latest_card_state() {
        let inset = InsetState::default();
        let mut applied = Vec::new();
        // Initial focus, enter the floating card, then leave it. Gaining and
        // losing focus must keep replaying the same requested position.
        for in_card in [false, true, false] {
            inset.set(in_card, || {});
            for _ in 0..2 {
                inset.refresh(false, Some(&"chrome"), |chrome, value| {
                    applied.push((*chrome, value));
                });
            }
        }
        assert_eq!(
            applied,
            [
                ("chrome", false),
                ("chrome", false),
                ("chrome", true),
                ("chrome", true),
                ("chrome", false),
                ("chrome", false),
            ]
        );
    }

    #[test]
    fn redraw_reentry_sees_the_new_card_state() {
        let inset = InsetState::default();
        let mut applied = Vec::new();
        for in_card in [true, false] {
            inset.set(in_card, || {
                // Model a focus/tracking callback during displayIfNeeded().
                inset.refresh(false, Some(&()), |_, value| applied.push(value));
            });
        }
        assert_eq!(applied, [true, false]);
    }

    #[test]
    fn fullscreen_and_detached_chrome_preserve_the_pending_inset() {
        let inset = InsetState::default();
        for in_card in [false, true] {
            inset.set(in_card, || {});
            inset.refresh(true, Some(&()), |_, _| {
                panic!("fullscreen owns the controls")
            });
            inset.refresh(false, None::<&()>, |_, _| panic!("chrome is detached"));
            inset.refresh(true, None::<&()>, |_, _| panic!("chrome is detached"));

            let mut applied = Vec::new();
            inset.refresh(false, Some(&()), |_, value| applied.push(value));
            assert_eq!(applied, [in_card]);
        }
    }
}
