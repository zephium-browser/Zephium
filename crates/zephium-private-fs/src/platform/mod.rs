#[cfg(target_os = "macos")]
mod macos_acl;
#[cfg(unix)]
mod unix;
#[cfg(target_os = "windows")]
mod windows;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DirectoryMode {
    Writable,
    Sealed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RegularMode {
    Writable,
    Sealed,
}

#[cfg(unix)]
pub(crate) use unix::*;
#[cfg(all(target_os = "windows", feature = "windows-work-test-fixtures"))]
pub use windows::native_storage::NativeWorkStorageTestSession;
#[cfg(target_os = "windows")]
pub use windows::native_storage::{
    NativeApplication, NativeSession, NativeStorageAnchor, NativeStorageFile,
    NativeWorkStorageAnchor, NativeWorkStorageFile,
};
#[cfg(target_os = "windows")]
pub use windows::prepare_user_data_root;
#[cfg(target_os = "windows")]
pub(crate) use windows::*;

#[cfg(not(any(unix, target_os = "windows")))]
compile_error!("zephium-private-fs supports only Unix and Windows targets");

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
mod tree_removal;
#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
pub(crate) use tree_removal::{remove_tree_bounded, TreeRemovalFaults};
