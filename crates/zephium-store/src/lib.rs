//! Bounded, crash-aware SQLite persistence behind one dedicated actor.

mod actor;
mod bounded_json;
mod essentials;
mod hub;
pub use hub::media::{admit as admit_media, MediaAdmissionError, MediaStore};
pub use hub::profile_avatars::prepare_avatar_photo;
mod legacy;
mod migrations;
mod pane;
#[cfg(target_os = "windows")]
mod windows_file_identity;

pub use actor::SqliteStore;
#[cfg(all(windows, feature = "work-execution"))]
pub use hub::WindowsWorkStorage;

mod work_frames;
mod work_migration_v2;
pub use work_frames::{WorkFrameRecord, WorkFrameStore};
