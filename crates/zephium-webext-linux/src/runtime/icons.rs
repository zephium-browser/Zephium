//! Bounded decoding of packaged action icons into the toolbar's fixed raster.

use std::cell::Cell;
use std::path::Path;
use std::rc::Rc;

use gtk::gdk_pixbuf::prelude::*;
use gtk::gdk_pixbuf::{Pixbuf, PixbufLoader};
use zephium_core::extensions::{
    ExtensionActionIcon, EXTENSION_ACTION_ICON_HEIGHT, EXTENSION_ACTION_ICON_WIDTH,
};

const MAX_ICON_FILE_BYTES: u64 = 1024 * 1024;
const MAX_ICON_SOURCE_PIXELS: i32 = 1024;

/// Decodes `path` inside the package at `root`, or `None` when it is not a
/// small raster image the package really contains.
pub(crate) fn package_icon(root: &Path, path: &str) -> Option<ExtensionActionIcon> {
    let path = zephium_webext::manifest::normalize_resource(path)?;
    if path.starts_with('_') || path.to_ascii_lowercase().ends_with(".svg") {
        return None;
    }
    let full = root.join(&path);
    let metadata = std::fs::symlink_metadata(&full).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_ICON_FILE_BYTES {
        return None;
    }
    decode(&std::fs::read(full).ok()?)
}

pub(crate) fn decode(bytes: &[u8]) -> Option<ExtensionActionIcon> {
    let loader = PixbufLoader::new();
    let refused = Rc::new(Cell::new(false));
    let flag = refused.clone();
    loader.connect_size_prepared(move |loader, width, height| {
        if width <= 0
            || height <= 0
            || width > MAX_ICON_SOURCE_PIXELS
            || height > MAX_ICON_SOURCE_PIXELS
        {
            flag.set(true);
            loader.set_size(1, 1);
            return;
        }
        loader.set_size(
            EXTENSION_ACTION_ICON_WIDTH as i32,
            EXTENSION_ACTION_ICON_HEIGHT as i32,
        );
    });
    let written = loader.write(bytes).is_ok();
    let closed = loader.close().is_ok();
    if !written || !closed || refused.get() {
        return None;
    }
    rgba(&loader.pixbuf()?)
}

fn rgba(pixbuf: &Pixbuf) -> Option<ExtensionActionIcon> {
    let pixbuf = if pixbuf.width() as usize == EXTENSION_ACTION_ICON_WIDTH
        && pixbuf.height() as usize == EXTENSION_ACTION_ICON_HEIGHT
    {
        pixbuf.clone()
    } else {
        pixbuf.scale_simple(
            EXTENSION_ACTION_ICON_WIDTH as i32,
            EXTENSION_ACTION_ICON_HEIGHT as i32,
            gtk::gdk_pixbuf::InterpType::Bilinear,
        )?
    };
    if pixbuf.bits_per_sample() != 8 {
        return None;
    }
    let channels = pixbuf.n_channels() as usize;
    if !(channels == 3 || channels == 4) {
        return None;
    }
    let stride = pixbuf.rowstride() as usize;
    let bytes = pixbuf.read_pixel_bytes();
    let mut out =
        Vec::with_capacity(EXTENSION_ACTION_ICON_WIDTH * EXTENSION_ACTION_ICON_HEIGHT * 4);
    for y in 0..EXTENSION_ACTION_ICON_HEIGHT {
        for x in 0..EXTENSION_ACTION_ICON_WIDTH {
            let at = y * stride + x * channels;
            let pixel = bytes.get(at..at + channels)?;
            out.extend_from_slice(&pixel[..3]);
            out.push(if channels == 4 { pixel[3] } else { 255 });
        }
    }
    ExtensionActionIcon::from_rgba(out).ok()
}

#[cfg(test)]
mod tests {
    #[test]
    fn non_images_and_oversize_sources_are_refused() {
        assert!(super::decode(b"not an image").is_none());
        assert!(super::decode(&[]).is_none());
    }
}
