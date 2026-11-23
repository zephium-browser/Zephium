use crate::{
    windows_target::{TargetNamespace, MAX_TARGET_UNITS},
    VaultError,
};
use std::ptr;
use std::sync::OnceLock;
use windows_sys::Win32::Foundation::{GetLastError, ERROR_NOT_FOUND};
use windows_sys::Win32::Security::Credentials::*;
use zeroize::{Zeroize, Zeroizing};

const _: () = assert!(MAX_TARGET_UNITS == CRED_MAX_GENERIC_TARGET_NAME_LENGTH as usize);
static NAMESPACE: OnceLock<TargetNamespace> = OnceLock::new();

/// Sets the vault namespace from the app's build identifier, once at startup.
/// Vault operations fail until a valid identity is set.
pub fn set_windows_identity(identifier: &str) -> Result<(), VaultError> {
    NAMESPACE
        .set(TargetNamespace::new(identifier)?)
        .map_err(|_| VaultError::Inaccessible)
}

fn namespace() -> Result<&'static TargetNamespace, VaultError> {
    NAMESPACE.get().ok_or(VaultError::Inaccessible)
}

fn error() -> VaultError {
    // SAFETY: GetLastError reads the calling thread's last native error.
    if unsafe { GetLastError() } == ERROR_NOT_FOUND {
        VaultError::Missing
    } else {
        VaultError::Inaccessible
    }
}

struct Records {
    allocation: *mut std::ffi::c_void,
    array: *mut *mut CREDENTIALW,
    single: *mut CREDENTIALW,
    count: usize,
}
impl Records {
    fn get(&self, index: usize) -> *mut CREDENTIALW {
        if index >= self.count {
            return ptr::null_mut();
        }
        if self.array.is_null() {
            return self.single;
        }
        // SAFETY: array contains exactly count live pointers returned by CredEnumerate.
        unsafe { *self.array.add(index) }
    }
}
impl Drop for Records {
    fn drop(&mut self) {
        for index in 0..self.count {
            let record = self.get(index);
            // SAFETY: These records and blobs belong to the successful Cred* allocation,
            // remain live until CredFree, and the documented blob bound is checked.
            unsafe {
                if let Some(record) = record.as_mut() {
                    if !record.CredentialBlob.is_null()
                        && record.CredentialBlobSize <= CRED_MAX_CREDENTIAL_BLOB_SIZE
                    {
                        std::slice::from_raw_parts_mut(
                            record.CredentialBlob,
                            record.CredentialBlobSize as usize,
                        )
                        .zeroize();
                    }
                }
            }
        }
        // SAFETY: The successful CredRead/Enumerate allocation is freed exactly once.
        unsafe {
            CredFree(self.allocation);
        }
    }
}

fn record(target: &str) -> Result<Records, VaultError> {
    let name = namespace()?.wide(target, false)?;
    let mut credential = ptr::null_mut();
    // SAFETY: name is a live terminated UTF-16 target; output points to live storage.
    if unsafe { CredReadW(name.as_ptr(), CRED_TYPE_GENERIC, 0, &mut credential) } == 0 {
        return Err(error());
    }
    if credential.is_null() {
        return Err(VaultError::Inaccessible);
    }
    Ok(Records {
        allocation: credential.cast(),
        array: ptr::null_mut(),
        single: credential,
        count: 1,
    })
}

/// Copies one bounded generic secret into zeroizing storage, then erases the OS buffer.
pub fn read(target: &str) -> Result<Zeroizing<Vec<u8>>, VaultError> {
    let records = record(target)?;
    // SAFETY: record() returns a nonnull live CredRead allocation guarded by Records.
    let credential = unsafe { &*records.get(0) };
    let size = credential.CredentialBlobSize as usize;
    if size > CRED_MAX_CREDENTIAL_BLOB_SIZE as usize
        || (size != 0 && credential.CredentialBlob.is_null())
    {
        return Err(VaultError::Invalid);
    }
    let mut bytes = Zeroizing::new(Vec::new());
    bytes
        .try_reserve_exact(size)
        .map_err(|_| VaultError::Capacity)?;
    if size != 0 {
        // SAFETY: CredRead owns this blob of checked size until Records drops.
        bytes.extend_from_slice(unsafe {
            std::slice::from_raw_parts(credential.CredentialBlob, size)
        });
    }
    Ok(bytes)
}

/// Tests exact presence without copying or inspecting any secret bytes.
pub fn present(target: &str) -> Result<bool, VaultError> {
    match record(target) {
        Ok(_) => Ok(true),
        Err(VaultError::Missing) => Ok(false),
        Err(e) => Err(e),
    }
}

/// Atomically replaces one generic credential, persisting for this Windows user.
pub fn write(target: &str, bytes: &[u8]) -> Result<(), VaultError> {
    let mut name = namespace()?.wide(target, false)?;
    if bytes.len() > CRED_MAX_CREDENTIAL_BLOB_SIZE as usize {
        return Err(VaultError::Invalid);
    }
    let mut blob = Zeroizing::new(bytes.to_vec());
    let credential = CREDENTIALW {
        Type: CRED_TYPE_GENERIC,
        TargetName: name.as_mut_ptr(),
        CredentialBlobSize: blob.len() as u32,
        CredentialBlob: blob.as_mut_ptr(),
        Persist: CRED_PERSIST_LOCAL_MACHINE,
        ..Default::default()
    };
    // SAFETY: All pointers reference live bounded buffers through this synchronous call.
    if unsafe { CredWriteW(&credential, 0) } == 0 {
        Err(error())
    } else {
        Ok(())
    }
}

/// Deletes the exact generic item; deleting a missing item succeeds.
pub fn delete(target: &str) -> Result<(), VaultError> {
    let name = namespace()?.wide(target, false)?;
    // SAFETY: name is a live terminated UTF-16 target; flags and type are fixed.
    if unsafe { CredDeleteW(name.as_ptr(), CRED_TYPE_GENERIC, 0) } != 0 {
        return Ok(());
    }
    match error() {
        VaultError::Missing => Ok(()),
        e => Err(e),
    }
}

/// Enumerates this build's namespace, returning logical target names.
pub fn list(prefix: &str) -> Result<Vec<String>, VaultError> {
    let namespace = namespace()?;
    let filter = namespace.wide(prefix, true)?;
    let mut count = 0;
    let mut allocation = ptr::null_mut();
    // SAFETY: filter is terminated UTF-16; output pointers are live local storage.
    if unsafe { CredEnumerateW(filter.as_ptr(), 0, &mut count, &mut allocation) } == 0 {
        return match error() {
            VaultError::Missing => Ok(Vec::new()),
            e => Err(e),
        };
    }
    if allocation.is_null() {
        return Err(VaultError::Inaccessible);
    }
    let records = Records {
        allocation: allocation.cast(),
        array: allocation,
        single: ptr::null_mut(),
        count: count as usize,
    };
    if count > 4096 {
        return Err(VaultError::Capacity);
    }
    let mut targets = Vec::new();
    for index in 0..records.count {
        let pointer = records.get(index);
        // SAFETY: Pointers belong to the guarded CredEnumerate allocation.
        let Some(record) = (unsafe { pointer.as_ref() }) else {
            return Err(VaultError::Inaccessible);
        };
        if record.Type != CRED_TYPE_GENERIC {
            continue;
        }
        if record.TargetName.is_null() {
            return Err(VaultError::Inaccessible);
        }
        let mut len = 0;
        // SAFETY: Native target names are terminated UTF-16 strings; scan respects the API bound.
        unsafe {
            while len <= MAX_TARGET_UNITS && *record.TargetName.add(len) != 0 {
                len += 1;
            }
        }
        if len > MAX_TARGET_UNITS {
            return Err(VaultError::Invalid);
        }
        // SAFETY: The preceding bounded scan found the native string's terminator.
        let target =
            String::from_utf16(unsafe { std::slice::from_raw_parts(record.TargetName, len) })
                .map_err(|_| VaultError::Invalid)?;
        if let Some(target) = namespace.listed_target(prefix, &target) {
            targets.push(target.to_owned());
        }
    }
    Ok(targets)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_generic_credential_roundtrip_uses_only_unique_nonsecret_fixture() {
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::time::{SystemTime, UNIX_EPOCH};
        static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(1);
        let generated_id = format!(
            "{:x}.{:x}.{:x}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("fixture clock")
                .as_nanos(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        );
        let target = format!("app.zephium.test.{generated_id}");
        set_windows_identity(&target).expect("isolated fixture identity");
        let _turn = crate::turn();
        assert!(
            !present(&target).expect("exact fixture absence"),
            "never overwrite an existing item"
        );
        struct FixtureCleanup(String);
        impl Drop for FixtureCleanup {
            fn drop(&mut self) {
                let _ = delete(&self.0);
            }
        }
        // Arm cleanup only after proving that this generated exact target was absent.
        let cleanup = FixtureCleanup(target);
        let fixture = b"nonsecret Windows generic credential fixture";
        write(&cleanup.0, fixture).expect("native fixture write");
        assert!(present(&cleanup.0).expect("native fixture presence"));
        assert_eq!(
            read(&cleanup.0).expect("native fixture read").as_slice(),
            fixture
        );
        delete(&cleanup.0).expect("native fixture deletion");
        assert!(!present(&cleanup.0).expect("native fixture absence after deletion"));
        assert!(matches!(read(&cleanup.0), Err(VaultError::Missing)));
        // This test never enumerates or modifies any pre-existing target.
    }
}
