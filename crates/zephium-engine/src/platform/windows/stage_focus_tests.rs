//! Drive both hide paths without changing the test desktop's keyboard focus.

use super::*;
use webview2_com::Microsoft::Web::WebView2::Win32::*;
use windows::core::w;
use windows::Win32::Foundation::E_NOTIMPL;
use windows::Win32::UI::WindowsAndMessaging::WS_VISIBLE;
use windows_core::Ref;

struct FocusRecord {
    focus: HWND,
    foreground: HWND,
    targets: Vec<HWND>,
}

thread_local! {
    static FOCUS: RefCell<Option<FocusRecord>> = const { RefCell::new(None) };
}

pub(super) fn focus_snapshot() -> Option<(HWND, HWND)> {
    FOCUS.with(|record| {
        record
            .borrow()
            .as_ref()
            .map(|record| (record.focus, record.foreground))
    })
}

pub(super) fn record_focus_target(target: HWND) -> bool {
    FOCUS.with(|record| {
        let mut record = record.borrow_mut();
        let Some(record) = record.as_mut() else {
            return false;
        };
        record.targets.push(target);
        true
    })
}

struct Windows(Vec<HWND>);

impl Windows {
    fn create(&mut self, parent: Option<HWND>) -> HWND {
        // SAFETY: test-owned off-screen windows, never activated or focused.
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_NOACTIVATE,
                w!("STATIC"),
                w!(""),
                WS_VISIBLE | if parent.is_some() { WS_CHILD } else { WS_POPUP },
                -10000,
                -10000,
                100,
                100,
                parent,
                None,
                None,
                None,
            )
            .unwrap()
        };
        self.0.push(hwnd);
        hwnd
    }
}

impl Drop for Windows {
    fn drop(&mut self) {
        FOCUS.with(|record| *record.borrow_mut() = None);
        for hwnd in self.0.iter().rev() {
            // SAFETY: all handles belong to this fixture and thread.
            let _ = unsafe { DestroyWindow(*hwnd) };
        }
    }
}

#[windows_core::implement(ICoreWebView2Controller)]
struct Controller {
    on_hide: Box<dyn Fn()>,
}

impl ICoreWebView2Controller_Impl for Controller_Impl {
    fn SetIsVisible(&self, visible: BOOL) -> windows_core::Result<()> {
        if !visible.as_bool() {
            (self.on_hide)();
        }
        Ok(())
    }
    fn IsVisible(&self, _: *mut BOOL) -> windows_core::Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn Bounds(&self, _: *mut RECT) -> windows_core::Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn SetBounds(&self, _: &RECT) -> windows_core::Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn ZoomFactor(&self, _: *mut f64) -> windows_core::Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn SetZoomFactor(&self, _: f64) -> windows_core::Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn add_ZoomFactorChanged(
        &self,
        _: Ref<'_, ICoreWebView2ZoomFactorChangedEventHandler>,
        _: *mut i64,
    ) -> windows_core::Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn remove_ZoomFactorChanged(&self, _: i64) -> windows_core::Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn SetBoundsAndZoomFactor(&self, _: &RECT, _: f64) -> windows_core::Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn MoveFocus(&self, _: COREWEBVIEW2_MOVE_FOCUS_REASON) -> windows_core::Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn add_MoveFocusRequested(
        &self,
        _: Ref<'_, ICoreWebView2MoveFocusRequestedEventHandler>,
        _: *mut i64,
    ) -> windows_core::Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn remove_MoveFocusRequested(&self, _: i64) -> windows_core::Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn add_GotFocus(
        &self,
        _: Ref<'_, ICoreWebView2FocusChangedEventHandler>,
        _: *mut i64,
    ) -> windows_core::Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn remove_GotFocus(&self, _: i64) -> windows_core::Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn add_LostFocus(
        &self,
        _: Ref<'_, ICoreWebView2FocusChangedEventHandler>,
        _: *mut i64,
    ) -> windows_core::Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn remove_LostFocus(&self, _: i64) -> windows_core::Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn add_AcceleratorKeyPressed(
        &self,
        _: Ref<'_, ICoreWebView2AcceleratorKeyPressedEventHandler>,
        _: *mut i64,
    ) -> windows_core::Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn remove_AcceleratorKeyPressed(&self, _: i64) -> windows_core::Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn ParentWindow(&self, _: *mut HWND) -> windows_core::Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn SetParentWindow(&self, _: HWND) -> windows_core::Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn NotifyParentWindowPositionChanged(&self) -> windows_core::Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn Close(&self) -> windows_core::Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn CoreWebView2(&self) -> windows_core::Result<ICoreWebView2> {
        Err(E_NOTIMPL.into())
    }
}

#[test]
fn hides_return_focus_after_settlement_including_a_revision_redrive() {
    for redrive in [false, true] {
        for deny in [
            "",
            "selected",
            "chrome",
            "background",
            "null",
            "visible",
            "destroyed",
        ] {
            let mut windows = Windows(Vec::new());
            let owner = windows.create(None);
            let page = windows.create(Some(owner));
            let renderer = windows.create(Some(page));
            let chrome = windows.create(Some(owner));
            let other = windows.create(None);
            let stage = Stage::new(owner, 8.0, |_, _| {});
            let id = ItemId::generate();
            let weak = Rc::downgrade(&stage.state);
            let supersede = Cell::new(redrive);
            let controller: ICoreWebView2Controller = Controller {
                on_hide: Box::new(move || {
                    if deny == "visible" {
                        // SAFETY: reshow this fixture's page without activation
                        // to exercise the final native visibility recheck.
                        let _ = unsafe {
                            ShowWindow(page, windows::Win32::UI::WindowsAndMessaging::SW_SHOWNA)
                        };
                    }
                    if supersede.replace(false) {
                        weak.upgrade().unwrap().borrow_mut().revision += 1;
                    }
                }),
            }
            .into();
            stage.state.borrow_mut().views.insert(
                id,
                HostView {
                    container: page,
                    controller,
                    serial: 1,
                    applied: Cell::new(AppliedPlacement::default()),
                    consecutive_failures: Cell::new(0),
                    terminal_failure_reported: Cell::new(false),
                    generation: Arc::new(AtomicBool::new(true)),
                    presentation_permit: Arc::new(AtomicBool::new(true)),
                },
            );
            if deny == "selected" {
                stage.state.borrow_mut().visible.insert(id);
            }
            FOCUS.with(|record| {
                *record.borrow_mut() = Some(FocusRecord {
                    focus: match deny {
                        "chrome" => chrome,
                        "null" => HWND::default(),
                        _ => renderer,
                    },
                    foreground: if deny == "background" { other } else { owner },
                    targets: Vec::new(),
                });
            });
            if deny == "destroyed" {
                // SAFETY: destroying the page also destroys its renderer.
                unsafe { DestroyWindow(page).unwrap() };
                windows.0.retain(|hwnd| *hwnd != page && *hwnd != renderer);
            }
            hide_views(&stage.state, &[id]);
            if redrive {
                FOCUS.with(|record| assert!(record.borrow().as_ref().unwrap().targets.is_empty()));
                assert!(stage.state.borrow().visibility_uncertain.contains(&id));
                // The queued layout uses apply_native_placement, the second
                // production hide path, at the new authoritative revision.
                sync(&stage.state);
            }
            FOCUS.with(|record| {
                let record = record.borrow();
                let expected = if deny.is_empty() { vec![owner] } else { vec![] };
                assert_eq!(
                    record.as_ref().unwrap().targets,
                    expected,
                    "{redrive}/{deny}"
                );
            });
        }
    }
}

#[test]
fn a_page_never_revealed_returns_focus_once_deselected() {
    for selected in [false, true] {
        let mut windows = Windows(Vec::new());
        let owner = windows.create(None);
        let page = windows.create(Some(owner));
        let renderer = windows.create(Some(page));
        // SAFETY: hide this fixture's page as a navigating page stays hidden.
        let _ = unsafe { ShowWindow(page, windows::Win32::UI::WindowsAndMessaging::SW_HIDE) };
        let stage = Stage::new(owner, 8.0, |_, _| {});
        let id = ItemId::generate();
        let controller: ICoreWebView2Controller = Controller {
            on_hide: Box::new(|| panic!("an already hidden page is not hidden again")),
        }
        .into();
        stage.state.borrow_mut().views.insert(
            id,
            HostView {
                container: page,
                controller,
                serial: 1,
                applied: Cell::new(AppliedPlacement {
                    window_visible: Some(false),
                    controller_visible: Some(false),
                    ..AppliedPlacement::default()
                }),
                consecutive_failures: Cell::new(0),
                terminal_failure_reported: Cell::new(false),
                generation: Arc::new(AtomicBool::new(true)),
                presentation_permit: Arc::new(AtomicBool::new(true)),
            },
        );
        if selected {
            stage.state.borrow_mut().visible.insert(id);
        }
        FOCUS.with(|record| {
            *record.borrow_mut() = Some(FocusRecord {
                focus: renderer,
                foreground: owner,
                targets: Vec::new(),
            });
        });
        hide_views(&stage.state, &[id]);
        FOCUS.with(|record| {
            let expected = if selected { vec![] } else { vec![owner] };
            assert_eq!(
                record.borrow().as_ref().unwrap().targets,
                expected,
                "{selected}"
            );
        });
    }
}
