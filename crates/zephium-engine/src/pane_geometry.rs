//! Native pane-size admission shared by all platform stages.
//!
//! Deeply nested, minimum-ratio splits can legitimately collapse below one
//! device pixel while a window is small. Passing a rounded zero extent to a
//! native webview is either rejected (WebView2) or silently promoted to a
//! misleading one-pixel surface (GTK). Treat that geometry as temporarily
//! non-paintable; the next authoritative layout can admit it again after the
//! window grows.

#[cfg(any(target_os = "macos", target_os = "windows", test))]
use zephium_core::geometry::Rect;

/// A horizontal stage transition that lays out pages once at the wider size.
#[cfg(any(target_os = "macos", target_os = "windows", test))]
#[derive(Clone, Copy, Debug)]
pub(crate) struct SidebarSlide {
    pub frame: Rect,
    pub from: f64,
    pub to: f64,
    pub clip: Rect,
}

#[cfg(any(target_os = "macos", target_os = "windows", test))]
impl SidebarSlide {
    pub fn between(current: Rect, target: Rect, beside: bool) -> Option<Self> {
        if current.width <= 0.0
            || target.width <= 0.0
            || current.height <= 0.0
            || current.y != target.y
            || current.height != target.height
            || (current.x == target.x && current.width == target.width)
        {
            return None;
        }
        // A fixed left edge means the sidebar moved the right edge instead.
        let distance = if current.x == target.x {
            target.x + target.width - current.x - current.width
        } else {
            target.x - current.x
        };
        let grows = target.width >= current.width;
        let frame = if grows { target } else { current };
        Some(Self {
            frame,
            from: if grows { -distance } else { 0.0 },
            to: if grows { 0.0 } else { distance },
            clip: if !grows && beside { target } else { frame },
        })
    }
}

/// A window-local clipping region; disjoint panes must get an empty region.
#[cfg(any(target_os = "windows", test))]
pub(crate) fn native_clip_bounds(
    (x, y, width, height): (i32, i32, i32, i32),
    stage: (i32, i32, i32, i32),
) -> (i32, i32, i32, i32) {
    let left = (stage.0 - x).max(0);
    let top = (stage.1 - y).max(0);
    let right = (stage.0 + stage.2 - x).min(width);
    let bottom = (stage.1 + stage.3 - y).min(height);
    if right <= left || bottom <= top {
        (0, 0, 0, 0)
    } else {
        (left, top, right, bottom)
    }
}

pub(crate) fn rounded_native_size(width: f64, height: f64, scale: f64) -> Option<(i32, i32)> {
    if !width.is_finite()
        || !height.is_finite()
        || !scale.is_finite()
        || width <= 0.0
        || height <= 0.0
        || scale <= 0.0
    {
        return None;
    }

    let width = (width * scale).round();
    let height = (height * scale).round();
    if !width.is_finite()
        || !height.is_finite()
        || width < 1.0
        || height < 1.0
        || width > i32::MAX as f64
        || height > i32::MAX as f64
    {
        return None;
    }

    Some((width as i32, height as i32))
}

#[cfg(test)]
mod tests {
    use super::rounded_native_size;

    #[test]
    fn rounded_zero_extent_is_temporarily_non_paintable() {
        assert_eq!(rounded_native_size(0.49, 20.0, 1.0), None);
        assert_eq!(rounded_native_size(20.0, 0.49, 1.0), None);
        assert_eq!(rounded_native_size(0.24, 20.0, 2.0), None);
    }

    #[test]
    fn a_later_larger_layout_is_admitted_again() {
        assert_eq!(rounded_native_size(0.49, 20.0, 1.0), None);
        assert_eq!(rounded_native_size(0.51, 20.0, 1.0), Some((1, 20)));
        assert_eq!(rounded_native_size(0.26, 20.0, 2.0), Some((1, 40)));
    }

    #[test]
    fn malformed_or_unrepresentable_geometry_fails_closed() {
        assert_eq!(rounded_native_size(f64::NAN, 10.0, 1.0), None);
        assert_eq!(rounded_native_size(10.0, f64::INFINITY, 1.0), None);
        assert_eq!(rounded_native_size(10.0, 10.0, 0.0), None);
        assert_eq!(rounded_native_size(i32::MAX as f64 + 1.0, 10.0, 1.0), None);
    }

    #[test]
    fn sidebar_slides_mirror_the_moving_edge_and_keep_the_wider_layout() {
        use super::SidebarSlide;
        use zephium_core::geometry::Rect;

        let wide = Rect::new(8.0, 8.0, 984.0, 784.0);
        for (narrow, sign) in [
            (Rect::new(272.0, 8.0, 720.0, 784.0), 1.0),
            (Rect::new(8.0, 8.0, 720.0, 784.0), -1.0),
        ] {
            let shrink = SidebarSlide::between(wide, narrow, true).unwrap();
            assert_eq!(shrink.frame, wide);
            assert_eq!((shrink.from, shrink.to), (0.0, sign * 264.0));
            assert_eq!(shrink.clip, narrow);
            let grow = SidebarSlide::between(narrow, wide, false).unwrap();
            assert_eq!(grow.frame, wide);
            assert_eq!((grow.from, grow.to), (sign * 264.0, 0.0));
            assert_eq!(grow.clip, wide);
        }
    }

    #[test]
    fn sidebar_slides_reject_initial_empty_and_non_horizontal_changes() {
        use super::SidebarSlide;
        use zephium_core::geometry::Rect;

        let wide = Rect::new(8.0, 8.0, 984.0, 784.0);
        for other in [
            wide,
            Rect::new(8.0, 8.0, 0.0, 784.0),
            Rect::new(8.0, 9.0, 720.0, 784.0),
            Rect::new(8.0, 8.0, 720.0, 780.0),
        ] {
            assert!(SidebarSlide::between(wide, other, false).is_none());
        }
        assert!(SidebarSlide::between(Rect::new(8.0, 8.0, 0.0, 784.0), wide, false,).is_none());
    }

    #[test]
    fn mirrored_split_panes_crossing_the_clip_boundary_leave_no_hit_area_outside_it() {
        use super::native_clip_bounds;

        let stage = (8, 8, 720, 784);
        assert_eq!(native_clip_bounds((-256, 8, 200, 784), stage), (0, 0, 0, 0));
        assert_eq!(native_clip_bounds((760, 8, 200, 784), stage), (0, 0, 0, 0));
        assert_eq!(
            native_clip_bounds((-92, 8, 200, 784), stage),
            (100, 0, 200, 784)
        );
        assert_eq!(
            native_clip_bounds((628, 8, 200, 784), stage),
            (0, 0, 100, 784)
        );
    }
}
