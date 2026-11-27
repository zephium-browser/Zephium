//! The one composition target a page's container window can have. DWM draws
//! it above the WebView2 child windows, so everything that lights or shades
//! the page from above, the working glow and the hidden sidebar's shadow,
//! hangs its visual here instead of each claiming the window.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use windows::core::Interface as _;
use windows::System::DispatcherQueueController;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::WinRT::Composition::ICompositorDesktopInterop;
use windows::Win32::System::WinRT::{
    CreateDispatcherQueueController, DispatcherQueueOptions, DQTAT_COM_NONE, DQTYPE_THREAD_CURRENT,
};
use windows::UI::Composition::Desktop::DesktopWindowTarget;
use windows::UI::Composition::{Compositor, ContainerVisual};
use windows_numerics::Vector2;

pub(super) struct PageLayer {
    target: DesktopWindowTarget,
    root: ContainerVisual,
}

thread_local! {
    static COMPOSITOR: RefCell<Option<(DispatcherQueueController, Compositor)>> = const { RefCell::new(None) };
    static LAYERS: RefCell<HashMap<isize, Weak<PageLayer>>> = RefCell::new(HashMap::new());
}

pub(super) fn compositor() -> Option<Compositor> {
    COMPOSITOR.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            let options = DispatcherQueueOptions {
                dwSize: std::mem::size_of::<DispatcherQueueOptions>() as u32,
                threadType: DQTYPE_THREAD_CURRENT,
                apartmentType: DQTAT_COM_NONE,
            };
            // SAFETY: creates this thread's dispatcher queue, which the
            // compositor requires; it lives as long as the thread's layers.
            let queue = unsafe { CreateDispatcherQueueController(options) }.ok()?;
            *slot = Some((queue, Compositor::new().ok()?));
        }
        slot.as_ref().map(|(_, compositor)| compositor.clone())
    })
}

/// The layer on `window`, made on first use and closed with its last holder.
pub(super) fn layer(window: HWND) -> Option<Rc<PageLayer>> {
    let key = window.0 as isize;
    if let Some(layer) = LAYERS.with(|layers| layers.borrow().get(&key).and_then(Weak::upgrade)) {
        return Some(layer);
    }
    let compositor = compositor()?;
    let interop: ICompositorDesktopInterop = compositor.cast().ok()?;
    // SAFETY: a topmost target on this live container window: DWM draws it
    // above the WebView2 child windows, and it takes no input.
    let target = unsafe { interop.CreateDesktopWindowTarget(window, true) }.ok()?;
    let root = compositor.CreateContainerVisual().ok()?;
    root.SetRelativeSizeAdjustment(Vector2 { X: 1.0, Y: 1.0 })
        .ok()?;
    target.SetRoot(&root).ok()?;
    let layer = Rc::new(PageLayer { target, root });
    LAYERS.with(|layers| {
        let mut layers = layers.borrow_mut();
        layers.retain(|_, layer| layer.strong_count() > 0);
        layers.insert(key, Rc::downgrade(&layer));
    });
    Some(layer)
}

impl PageLayer {
    /// Puts `visual` on top of the layer, once.
    pub(super) fn attach(&self, visual: &ContainerVisual) -> Option<()> {
        let children = self.root.Children().ok()?;
        let _ = children.Remove(visual);
        children.InsertAtTop(visual).ok()
    }

    pub(super) fn detach(&self, visual: &ContainerVisual) {
        if let Ok(children) = self.root.Children() {
            let _ = children.Remove(visual);
        }
    }
}

impl Drop for PageLayer {
    fn drop(&mut self) {
        let _ = self.target.SetRoot(None);
        let _ = self.target.Close();
    }
}
