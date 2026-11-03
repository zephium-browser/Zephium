use crate::geometry::Rect;
use crate::layout::SidebarPosition;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChromeFrame {
    pub rect: Rect,
    pub fill_width: bool,
    pub sidebar_position: SidebarPosition,
    pub sidebar_hidden: bool,
    /// The frame belongs to a deliberate change of the window's shape that
    /// the content travels through. A chrome that would narrow keeps its
    /// width until the content has arrived, so what the chrome draws during
    /// the journey is never cut away beneath it.
    pub travel: bool,
}

pub trait Chrome {
    /// Admit the exact privileged-chrome frame to its native UI queue.
    /// `false` means no task owns the transition, so raw content must remain
    /// concealed and the caller must retry or retire it.
    fn position(&self, frame: ChromeFrame) -> bool;
}
