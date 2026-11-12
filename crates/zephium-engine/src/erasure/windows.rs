//! Restrict engine-owned directories before handing them to WebView2.
use std::fs::{File, OpenOptions};
use std::io;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::Path;

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{LocalFree, HANDLE, HLOCAL};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo,
    SetSecurityInfo, SDDL_REVISION_1, SE_FILE_OBJECT,
};
use windows::Win32::Security::{
    EqualSid, GetSecurityDescriptorControl, GetSecurityDescriptorDacl, GetTokenInformation,
    IsValidSid, TokenUser, ACL, DACL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION,
    PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SE_DACL_PROTECTED,
    TOKEN_QUERY, TOKEN_USER,
};
use windows::Win32::Storage::FileSystem::{
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
    FILE_SHARE_READ, FILE_SHARE_WRITE, READ_CONTROL, WRITE_DAC,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

struct Allocation(HLOCAL);
impl Drop for Allocation {
    fn drop(&mut self) {
        // SAFETY: each guard owns one allocation returned by a LocalAlloc-based API.
        let _ = unsafe { LocalFree(Some(self.0)) };
    }
}

fn refused() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "cannot restrict engine directory",
    )
}

fn directory(path: &Path, write_acl: bool) -> io::Result<File> {
    let file = OpenOptions::new()
        .access_mode(
            READ_CONTROL.0 | FILE_READ_ATTRIBUTES.0 | if write_acl { WRITE_DAC.0 } else { 0 },
        )
        .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE).0)
        .custom_flags((FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT).0)
        .open(path)?;
    if !super::metadata_is_direct_directory(&file.metadata()?) {
        return Err(refused());
    }
    Ok(file)
}

fn security(file: &File) -> io::Result<(Allocation, PSECURITY_DESCRIPTOR, PSID, *mut ACL)> {
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    let mut owner = PSID::default();
    let mut acl = std::ptr::null_mut();
    // SAFETY: live borrowed handle and initialized outputs. The descriptor owns
    // the returned owner and ACL until the allocation guard is dropped.
    unsafe {
        GetSecurityInfo(
            HANDLE(file.as_raw_handle()),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            Some(&mut owner),
            None,
            Some(&mut acl),
            None,
            Some(&mut descriptor),
        )
    }
    .ok()
    .map_err(|_| refused())?;
    Ok((Allocation(HLOCAL(descriptor.0)), descriptor, owner, acl))
}

fn matches(
    descriptor: PSECURITY_DESCRIPTOR,
    acl: *mut ACL,
    expected: *mut ACL,
) -> io::Result<bool> {
    if acl.is_null() {
        return Ok(false);
    }
    let mut control = 0u16;
    let mut revision = 0;
    // SAFETY: both descriptors and their ACLs came from successful Windows APIs
    // and their allocation guards remain live throughout this comparison.
    unsafe {
        GetSecurityDescriptorControl(descriptor, &mut control, &mut revision)
            .map_err(|_| refused())?;
        Ok(control & SE_DACL_PROTECTED.0 != 0
            && std::slice::from_raw_parts(acl.cast::<u8>(), usize::from((*acl).AclSize))
                == std::slice::from_raw_parts(
                    expected.cast::<u8>(),
                    usize::from((*expected).AclSize),
                ))
    }
}

pub(super) fn restrict_directory(path: &Path) -> io::Result<()> {
    // Pin ancestors without delete sharing before opening the target. Do not
    // follow junctions or change ownership to make an unfamiliar tree usable.
    let ancestors: Vec<_> = path.ancestors().skip(1).collect();
    if ancestors.len() > 128 {
        return Err(refused());
    }
    let _pins = ancestors
        .into_iter()
        .rev()
        .map(|p| directory(p, false))
        .collect::<io::Result<Vec<_>>>()?;
    let file = directory(path, true)?;
    let mut token = HANDLE::default();
    // SAFETY: current process pseudo-handle and initialized token output.
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }
        .map_err(|_| refused())?;
    // SAFETY: OpenProcessToken transferred unique ownership of this handle.
    let token = unsafe { OwnedHandle::from_raw_handle(token.0) };
    // TOKEN_USER contains pointers; keep the output aligned and bounded.
    let mut user = vec![0usize; 65536 / std::mem::size_of::<usize>()];
    let mut length = 0;
    // SAFETY: live token and aligned writable buffer with the advertised capacity.
    unsafe {
        GetTokenInformation(
            HANDLE(token.as_raw_handle()),
            TokenUser,
            Some(user.as_mut_ptr().cast()),
            65536,
            &mut length,
        )
    }
    .map_err(|_| refused())?;
    if length < std::mem::size_of::<TOKEN_USER>() as u32 || length > 65536 {
        return Err(refused());
    }
    // SAFETY: successful TokenUser query initialized TOKEN_USER in the retained buffer.
    let sid = unsafe { (*user.as_ptr().cast::<TOKEN_USER>()).User.Sid };
    let (_old, descriptor, owner, acl) = security(&file)?;
    if owner.0.is_null()
        || sid.0.is_null()
        // SAFETY: Windows supplied both nonnull SIDs; their allocations remain live.
        || !unsafe { IsValidSid(owner).as_bool() && IsValidSid(sid).as_bool() }
        // SAFETY: both retained SIDs passed the validation above.
        || unsafe { EqualSid(owner, sid) }.is_err()
    {
        return Err(refused());
    }
    let mut text = PWSTR::null();
    // SAFETY: retained token SID and initialized LocalAlloc string output.
    unsafe { ConvertSidToStringSidW(sid, &mut text) }.map_err(|_| refused())?;
    let _text = Allocation(HLOCAL(text.0.cast()));
    // SAFETY: ConvertSidToStringSidW returns a terminated SID string.
    let sid = unsafe { text.to_string() }.map_err(|_| refused())?;
    let sddl: Vec<_> = format!("D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;{sid})")
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let mut desired = PSECURITY_DESCRIPTOR::default();
    // SAFETY: terminated SDDL from a native SID and initialized descriptor output.
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PCWSTR(sddl.as_ptr()),
            SDDL_REVISION_1,
            &mut desired,
            None,
        )
    }
    .map_err(|_| refused())?;
    let _desired = Allocation(HLOCAL(desired.0));
    let mut present = false.into();
    let mut defaulted = false.into();
    let mut expected = std::ptr::null_mut();
    // SAFETY: retained descriptor and initialized outputs into that descriptor.
    unsafe { GetSecurityDescriptorDacl(desired, &mut present, &mut expected, &mut defaulted) }
        .map_err(|_| refused())?;
    if !present.as_bool() || expected.is_null() {
        return Err(refused());
    }
    if matches(descriptor, acl, expected)? {
        return Ok(());
    }
    // SAFETY: exact pinned directory opened with WRITE_DAC. Only its DACL is
    // changed; Windows propagates inheritable ACEs to inheriting descendants.
    // Explicit/protected child ACLs remain runtime-owned, not silently reset.
    unsafe {
        SetSecurityInfo(
            HANDLE(file.as_raw_handle()),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            None,
            None,
            Some(expected),
            None,
        )
    }
    .ok()
    .map_err(|_| refused())?;
    let (_readback, descriptor, _, acl) = security(&file)?;
    if !matches(descriptor, acl, expected)? {
        return Err(refused());
    }
    Ok(())
}
