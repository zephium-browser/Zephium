pub(crate) mod normalize_text;
mod parse_manifest;
mod pick_color;
mod pick_icon;
mod pick_name;

pub use parse_manifest::{parse_manifest, MAX_MANIFEST_BYTES};
