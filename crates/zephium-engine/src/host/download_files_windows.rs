//! Windows download staging. Directory leases pin every canonical ancestor;
//! cleanup addresses opened objects and never recursively walks a user folder.
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{LocalFree, ERROR_SUCCESS, HANDLE, HLOCAL};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo,
    SDDL_REVISION_1, SE_FILE_OBJECT,
};
use windows::Win32::Security::{
    EqualSid, GetAce, GetSecurityDescriptorControl, GetTokenInformation, IsValidSid, TokenUser,
    ACCESS_ALLOWED_ACE, ACL, DACL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION,
    PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES, SE_DACL_PROTECTED, TOKEN_QUERY, TOKEN_USER,
};
use windows::Win32::Storage::FileSystem::{
    CreateDirectoryW, FileDispositionInfo, FileIdInfo, GetFileInformationByHandleEx, MoveFileExW,
    SetFileInformationByHandle, DELETE, FILE_ALL_ACCESS, FILE_ATTRIBUTE_DIRECTORY,
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_DISPOSITION_INFO, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_FLAG_OPEN_REPARSE_POINT, FILE_ID_INFO, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE,
    FILE_SHARE_READ, FILE_SHARE_WRITE, MOVEFILE_WRITE_THROUGH, READ_CONTROL,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
    COINIT_APARTMENTTHREADED,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::Win32::UI::Shell::{AttachmentServices, IAttachmentExecute};
use zephium_core::downloads::{
    valid_filename, DownloadError, DownloadRecord, DownloadWriter, FileIdentity,
};
use zephium_core::ids::DownloadId;

const MAX_ANCESTORS: usize = 128;

pub(super) struct Destination {
    pub path: PathBuf,
    pub staging_path: PathBuf,
    pub staging_identity: FileIdentity,
    staging: File,
    _ancestors: Vec<File>,
    cleanup_on_drop: bool,
}

fn handle(file: &File) -> HANDLE {
    HANDLE(file.as_raw_handle())
}
fn wide(path: &Path) -> Result<Vec<u16>, DownloadError> {
    let mut value: Vec<_> = path.as_os_str().encode_wide().collect();
    if value.is_empty() || value.len() > 32766 || value.contains(&0) {
        return Err(DownloadError::Destination);
    }
    value.push(0);
    Ok(value)
}
fn io(error: std::io::Error) -> DownloadError {
    match error.raw_os_error() {
        Some(39 | 112) => DownloadError::DiskFull,
        Some(2 | 3) => DownloadError::MissingFile,
        // ERROR_ACCESS_DENIED, also raised by Controlled Folder Access.
        Some(5) => DownloadError::Permission,
        Some(32 | 33) => DownloadError::FileBusy,
        Some(223) => DownloadError::FileTooLarge,
        _ => DownloadError::Destination,
    }
}
fn win(error: windows::core::Error) -> DownloadError {
    io(std::io::Error::from_raw_os_error(
        (error.code().0 as u32 & 0xffff) as i32,
    ))
}
fn identity(file: &File) -> Result<FileIdentity, DownloadError> {
    let mut value = FILE_ID_INFO::default();
    // SAFETY: borrowed live handle and correctly aligned, sized output.
    unsafe {
        GetFileInformationByHandleEx(
            handle(file),
            FileIdInfo,
            (&mut value as *mut FILE_ID_INFO).cast(),
            std::mem::size_of_val(&value) as u32,
        )
    }
    .map_err(win)?;
    let metadata = file.metadata().map_err(io)?;
    let mut low = [0; 8];
    low.copy_from_slice(&value.FileId.Identifier[..8]);
    let mut high = [0; 8];
    high.copy_from_slice(&value.FileId.Identifier[8..]);
    Ok(FileIdentity {
        volume: value.VolumeSerialNumber,
        file: u64::from_le_bytes(low),
        file_high: u64::from_le_bytes(high),
        bytes: metadata.len(),
        modified: Some((metadata.last_write_time() as i64, 0)),
    })
}
fn same_node(a: &FileIdentity, b: &FileIdentity) -> bool {
    a.volume == b.volume && a.file == b.file && a.file_high == b.file_high
}
fn directory_identity(file: &File) -> Result<String, DownloadError> {
    let value = identity(file)?;
    Ok(format!(
        "{:016x}:{:016x}{:016x}",
        value.volume, value.file_high, value.file
    ))
}
fn open_directory(path: &Path, removable: bool) -> Result<File, DownloadError> {
    inspect_directory(path, removable, false)
}
fn inspect_directory(
    path: &Path,
    removable: bool,
    share_delete: bool,
) -> Result<File, DownloadError> {
    let file = OpenOptions::new()
        .access_mode(READ_CONTROL.0 | FILE_READ_ATTRIBUTES.0 | if removable { DELETE.0 } else { 0 })
        .share_mode(
            (FILE_SHARE_READ | FILE_SHARE_WRITE).0
                | if share_delete { FILE_SHARE_DELETE.0 } else { 0 },
        )
        .custom_flags((FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT).0)
        .open(path)
        .map_err(io)?;
    let attributes = file.metadata().map_err(io)?.file_attributes();
    if attributes & FILE_ATTRIBUTE_DIRECTORY.0 == 0
        || attributes & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0
    {
        return Err(DownloadError::ChangedFile);
    }
    Ok(file)
}
fn lock_ancestors(path: &Path) -> Result<Vec<File>, DownloadError> {
    let parents: Vec<_> = path.ancestors().collect();
    if parents.len() > MAX_ANCESTORS {
        return Err(DownloadError::Destination);
    }
    let mut files = Vec::with_capacity(parents.len());
    for parent in parents.into_iter().rev() {
        files.push(open_directory(parent, false)?);
    }
    Ok(files)
}
fn open_payload(path: &Path, write: bool) -> Result<File, DownloadError> {
    let file = OpenOptions::new()
        .read(true)
        .write(write)
        .share_mode((FILE_SHARE_READ | FILE_SHARE_DELETE).0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(path)
        .map_err(io)?;
    let metadata = file.metadata().map_err(io)?;
    if !metadata.is_file() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
        return Err(DownloadError::ChangedFile);
    }
    // A downloaded payload must not expose a hard-linked pre-existing file.
    let mut info = windows::Win32::Storage::FileSystem::BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: borrowed file handle and live output value.
    unsafe {
        windows::Win32::Storage::FileSystem::GetFileInformationByHandle(handle(&file), &mut info)
    }
    .map_err(win)?;
    if info.nNumberOfLinks != 1 {
        return Err(DownloadError::ChangedFile);
    }
    Ok(file)
}

struct LocalAllocation(HLOCAL);
impl Drop for LocalAllocation {
    fn drop(&mut self) {
        unsafe {
            let _ = LocalFree(Some(self.0));
        }
    }
}
struct CurrentUser {
    _token: OwnedHandle,
    bytes: Vec<usize>,
}
impl CurrentUser {
    fn open() -> Result<Self, DownloadError> {
        let mut token = HANDLE::default();
        // SAFETY: the pseudo process handle and output slot are valid.
        unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }.map_err(win)?;
        // SAFETY: OpenProcessToken transferred this handle.
        let token = unsafe { OwnedHandle::from_raw_handle(token.0) };
        let mut len = 0;
        // SAFETY: documented zero-capacity size query.
        let _ = unsafe {
            GetTokenInformation(HANDLE(token.as_raw_handle()), TokenUser, None, 0, &mut len)
        };
        if len < std::mem::size_of::<TOKEN_USER>() as u32 || len > 65536 {
            return Err(DownloadError::Protection);
        }
        let mut bytes = vec![0usize; (len as usize).div_ceil(std::mem::size_of::<usize>())];
        // SAFETY: the aligned allocation has the reported capacity.
        unsafe {
            GetTokenInformation(
                HANDLE(token.as_raw_handle()),
                TokenUser,
                Some(bytes.as_mut_ptr().cast()),
                len,
                &mut len,
            )
        }
        .map_err(win)?;
        let user = Self {
            _token: token,
            bytes,
        };
        if !unsafe { IsValidSid(user.sid()).as_bool() } {
            return Err(DownloadError::Protection);
        }
        Ok(user)
    }
    fn sid(&self) -> PSID {
        unsafe { (&*self.bytes.as_ptr().cast::<TOKEN_USER>()).User.Sid }
    }
    fn text(&self) -> Result<String, DownloadError> {
        let mut text = PWSTR::null();
        // SAFETY: validated SID; Windows allocates a terminated SID string.
        unsafe { ConvertSidToStringSidW(self.sid(), &mut text) }.map_err(win)?;
        let _allocation = LocalAllocation(HLOCAL(text.0.cast()));
        for len in 0..256 {
            if unsafe { *text.0.add(len) } == 0 {
                return String::from_utf16(unsafe { std::slice::from_raw_parts(text.0, len) })
                    .map_err(|_| DownloadError::Protection);
            }
        }
        Err(DownloadError::Protection)
    }
}

fn create_private_directory(path: &Path) -> Result<(), DownloadError> {
    let user = CurrentUser::open()?.text()?;
    let sddl = wide(Path::new(&format!(
        "O:{user}D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;{user})"
    )))?;
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // SAFETY: bounded terminated SDDL and valid output pointer.
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PCWSTR(sddl.as_ptr()),
            SDDL_REVISION_1,
            &mut descriptor,
            None,
        )
    }
    .map_err(win)?;
    let _allocation = LocalAllocation(HLOCAL(descriptor.0));
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: false.into(),
    };
    let path = wide(path)?;
    // SAFETY: path and descriptor remain live through this create-only call.
    unsafe { CreateDirectoryW(PCWSTR(path.as_ptr()), Some(&attributes)) }.map_err(win)
}
fn verify_private_directory(file: &File) -> Result<(), DownloadError> {
    let user = CurrentUser::open()?;
    let mut owner = PSID::default();
    let mut dacl = std::ptr::null_mut::<ACL>();
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // SAFETY: borrowed handle and live output slots; descriptor has LocalFree ownership.
    let status = unsafe {
        GetSecurityInfo(
            handle(file),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            Some(&mut owner),
            None,
            Some(&mut dacl),
            None,
            Some(&mut descriptor),
        )
    };
    if status != ERROR_SUCCESS {
        return Err(DownloadError::Protection);
    }
    let _allocation = LocalAllocation(HLOCAL(descriptor.0));
    if dacl.is_null() || unsafe { EqualSid(owner, user.sid()) }.is_err() {
        return Err(DownloadError::Protection);
    }
    let mut control = 0u16;
    let mut revision = 0;
    unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) }
        .map_err(win)?;
    if control & SE_DACL_PROTECTED.0 == 0 || unsafe { (*dacl).AceCount } != 2 {
        return Err(DownloadError::Protection);
    }
    let mut found_user = false;
    let mut found_system = false;
    for index in 0..2 {
        let mut ace = std::ptr::null_mut();
        unsafe { GetAce(dacl, index, &mut ace) }.map_err(win)?;
        let ace = unsafe { &*ace.cast::<ACCESS_ALLOWED_ACE>() };
        if ace.Header.AceType != 0
            || usize::from(ace.Header.AceSize) < std::mem::size_of::<ACCESS_ALLOWED_ACE>()
            || ace.Mask != FILE_ALL_ACCESS.0
        {
            return Err(DownloadError::Protection);
        }
        let sid = PSID((&ace.SidStart as *const u32).cast_mut().cast());
        if !unsafe { IsValidSid(sid).as_bool() } {
            return Err(DownloadError::Protection);
        }
        if unsafe { EqualSid(sid, user.sid()) }.is_ok() {
            found_user = true;
        } else if unsafe {
            windows::Win32::Security::IsWellKnownSid(
                sid,
                windows::Win32::Security::WinLocalSystemSid,
            )
            .as_bool()
        } {
            found_system = true;
        } else {
            return Err(DownloadError::Protection);
        }
    }
    if found_user && found_system {
        Ok(())
    } else {
        Err(DownloadError::Protection)
    }
}
fn dispose(file: &File) -> Result<(), DownloadError> {
    let value = FILE_DISPOSITION_INFO { DeleteFile: true };
    // SAFETY: this exact opened object is marked for deletion, never a path traversal.
    unsafe {
        SetFileInformationByHandle(
            handle(file),
            FileDispositionInfo,
            (&value as *const FILE_DISPOSITION_INFO).cast(),
            std::mem::size_of_val(&value) as u32,
        )
    }
    .map_err(win)
}
fn cleanup_payload(path: &Path) -> Result<(), DownloadError> {
    let file = match OpenOptions::new()
        .access_mode(DELETE.0 | FILE_READ_ATTRIBUTES.0)
        // Refuse cleanup while any handle still has write access, even if
        // a stale runtime has survived its recorded browser process.
        .share_mode((FILE_SHARE_READ | FILE_SHARE_DELETE).0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(io(error)),
    };
    let metadata = file.metadata().map_err(io)?;
    if !metadata.is_file() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
        return Err(DownloadError::ChangedFile);
    }
    dispose(&file)
}

impl Destination {
    pub(super) fn prepare(
        id: DownloadId,
        requested: PathBuf,
        expected_directory: Option<String>,
    ) -> Result<Self, DownloadError> {
        if !requested.is_absolute()
            || requested.as_os_str().len() > 4096
            || !requested
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(valid_filename)
        {
            return Err(DownloadError::Destination);
        }
        let parent =
            fs::canonicalize(requested.parent().ok_or(DownloadError::Destination)?).map_err(io)?;
        let ancestors = lock_ancestors(&parent)?;
        let parent_handle = ancestors.last().ok_or(DownloadError::Destination)?;
        if expected_directory.is_some_and(|expected| {
            directory_identity(parent_handle).ok().as_ref() != Some(&expected)
        }) {
            return Err(DownloadError::ChangedFile);
        }
        let staging_path = parent.join(format!(".zephium-download-{id}"));
        create_private_directory(&staging_path)?;
        let staging = open_directory(&staging_path, true)?;
        verify_private_directory(&staging)?;
        let staging_identity = identity(&staging)?;
        let path = parent.join(requested.file_name().ok_or(DownloadError::Destination)?);
        Ok(Self {
            path,
            staging_path,
            staging_identity,
            staging,
            _ancestors: ancestors,
            cleanup_on_drop: true,
        })
    }
    pub(super) fn abandon(mut self) {
        self.cleanup_on_drop = false;
    }

    pub(super) fn payload(&self) -> PathBuf {
        self.staging_path.join("payload")
    }
    fn verify_namespace(&self) -> Result<(), DownloadError> {
        // The original stage handle requests DELETE and denies delete sharing.
        // A read-only observer must share DELETE to coexist with that lease;
        // it cannot itself rename/delete the pinned directory.
        let observed = inspect_directory(&self.staging_path, false, true)?;
        if !same_node(&identity(&observed)?, &self.staging_identity) {
            return Err(DownloadError::ChangedFile);
        }
        verify_private_directory(&observed)
    }
    pub(super) fn finish(
        mut self,
        source: &str,
        expected_bytes: Option<u64>,
    ) -> Result<(PathBuf, FileIdentity), DownloadError> {
        self.verify_namespace()?;
        let initial = open_payload(&self.payload(), false)?;
        let before = identity(&initial)?;
        if expected_bytes.is_some_and(|bytes| before.bytes != bytes) {
            return Err(DownloadError::Integrity);
        }
        drop(initial);
        protect(&self.payload(), source, &self.path)?;
        self.verify_namespace()?;
        let file = open_payload(&self.payload(), true)?;
        let after = identity(&file)?;
        if !same_node(&before, &after) || before.bytes != after.bytes {
            return Err(DownloadError::ChangedFile);
        }
        verify_protection(&self.payload())?;
        file.sync_all().map_err(io)?;
        let leaf = self
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(DownloadError::Destination)?
            .to_owned();
        for suffix in 0..10000 {
            let name = zephium_core::downloads::collision_filename(&leaf, suffix);
            let path = self
                .path
                .parent()
                .ok_or(DownloadError::Destination)?
                .join(name);
            let from = wide(&self.payload())?;
            let to = wide(&path)?;
            // SAFETY: held ancestor/staging leases pin both paths; no replace or
            // cross-volume-copy flag is supplied. An existing target is untouched.
            match unsafe {
                MoveFileExW(
                    PCWSTR(from.as_ptr()),
                    PCWSTR(to.as_ptr()),
                    MOVEFILE_WRITE_THROUGH,
                )
            } {
                Ok(()) => {
                    self.path = path;
                    return Ok((self.path.clone(), identity(&file)?));
                }
                Err(error) if matches!(error.code().0 as u32 & 0xffff, 80 | 183) => continue,
                Err(error) => return Err(win(error)),
            }
        }
        Err(DownloadError::Destination)
    }
}
impl Drop for Destination {
    fn drop(&mut self) {
        if self.cleanup_on_drop
            && self.verify_namespace().is_ok()
            && cleanup_payload(&self.payload()).is_ok()
        {
            let _ = dispose(&self.staging);
        }
    }
}
struct Apartment;
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}
fn protect(path: &Path, source: &str, destination: &Path) -> Result<(), DownloadError> {
    if zephium_core::permissions::PageOrigin::parse_exact(source).is_err() {
        return Err(DownloadError::Invalid);
    }
    // This runs on a dedicated file worker, not the UI's COM apartment.
    unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }
        .ok()
        .map_err(|_| DownloadError::Protection)?;
    let _apartment = Apartment;
    let service: IAttachmentExecute =
        unsafe { CoCreateInstance(&AttachmentServices, None, CLSCTX_INPROC_SERVER) }
            .map_err(|_| DownloadError::Protection)?;
    let file = wide(path)?;
    let origin = wide(Path::new(source))?;
    let filename = wide(Path::new(
        destination.file_name().ok_or(DownloadError::Destination)?,
    ))?;
    unsafe { service.SetFileName(PCWSTR(filename.as_ptr())) }
        .map_err(|_| DownloadError::Protection)?;
    unsafe { service.SetLocalPath(PCWSTR(file.as_ptr())) }
        .map_err(|_| DownloadError::Protection)?;
    unsafe { service.SetSource(PCWSTR(origin.as_ptr())) }.map_err(|_| DownloadError::Protection)?;
    unsafe { service.Save() }.map_err(|_| DownloadError::Protection)?;
    // Attachment Services may delete a blocked payload. Hold the surviving
    // base file while creating the stream so a deleted payload is never recreated.
    let _base = open_payload(path, false)?;
    // Always retain Internet-zone provenance, even for loopback fixtures or an
    // intranet response. No signed URL or credentials are written by this broker.
    let mut stream = path.as_os_str().to_owned();
    stream.push(":Zone.Identifier");
    let mut output = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .share_mode(FILE_SHARE_READ.0)
        .open(Path::new(&stream))
        .map_err(|_| DownloadError::Protection)?;
    write!(output, "[ZoneTransfer]\r\nZoneId=3\r\nHostUrl={source}\r\n")
        .map_err(|_| DownloadError::Protection)?;
    output.sync_all().map_err(|_| DownloadError::Protection)
}
fn verify_protection(path: &Path) -> Result<(), DownloadError> {
    let mut stream = path.as_os_str().to_owned();
    stream.push(":Zone.Identifier");
    let file = File::open(Path::new(&stream)).map_err(|_| DownloadError::Protection)?;
    let mut content = String::new();
    file.take(16385)
        .read_to_string(&mut content)
        .map_err(|_| DownloadError::Protection)?;
    if !protected_zone(&content) {
        return Err(DownloadError::Protection);
    }
    Ok(())
}
fn protected_zone(content: &str) -> bool {
    if content.len() > 16384 {
        return false;
    }
    let mut section = false;
    let mut seen_section = false;
    let mut zone = None;
    for line in content.lines().map(str::trim) {
        if line.starts_with('[') {
            section = line.eq_ignore_ascii_case("[ZoneTransfer]");
            if section && seen_section {
                return false;
            }
            seen_section |= section;
        } else if section {
            if let Some((key, value)) = line.split_once('=') {
                if key.trim().eq_ignore_ascii_case("ZoneId") {
                    if zone.is_some() {
                        return false;
                    }
                    zone = Some(value.trim());
                }
            }
        }
    }
    matches!(zone, Some("3" | "4"))
}

pub(super) fn select_directory(path: PathBuf) -> Result<(String, String), DownloadError> {
    let path = fs::canonicalize(path).map_err(io)?;
    let directory = open_directory(&path, false)?;
    Ok((
        path.to_str()
            .filter(|path| path.len() <= 4096)
            .ok_or(DownloadError::Destination)?
            .into(),
        directory_identity(&directory)?,
    ))
}
pub(super) fn verify_file(path: &Path, expected: &FileIdentity) -> Result<(), DownloadError> {
    let file = open_payload(path, false)?;
    let actual = identity(&file)?;
    if !same_node(&actual, expected)
        || actual.bytes != expected.bytes
        || expected
            .modified
            .is_some_and(|stamp| actual.modified != Some(stamp))
    {
        return Err(DownloadError::ChangedFile);
    }
    verify_protection(path)
}
pub(super) fn recover_staging(record: &DownloadRecord) -> Result<(), DownloadError> {
    if !record.writer_released
        && record
            .writer
            .as_ref()
            .is_some_and(|owner| !writer_exited(owner))
    {
        return Err(DownloadError::Unavailable);
    }
    let (Some(path), Some(expected)) = (&record.staging, &record.staging_identity) else {
        return Ok(());
    };
    if !record.state.terminal()
        || path.file_name().and_then(|s| s.to_str())
            != Some(format!(".zephium-download-{}", record.id).as_str())
    {
        return Err(DownloadError::Invalid);
    }
    let _ancestors = lock_ancestors(path.parent().ok_or(DownloadError::Destination)?)?;
    let directory = match open_directory(path, true) {
        Ok(file) => file,
        Err(DownloadError::MissingFile) => return Ok(()),
        Err(error) => return Err(error),
    };
    if !same_node(&identity(&directory)?, expected) {
        return Err(DownloadError::ChangedFile);
    }
    verify_private_directory(&directory)?;
    cleanup_payload(&path.join("payload"))?;
    dispose(&directory)
}

pub(super) fn writer_for_process(process: u32) -> Result<DownloadWriter, DownloadError> {
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
    };
    if process == 0 {
        return Err(DownloadError::Unavailable);
    }
    // SAFETY: query/synchronize only; the returned handle identifies one exact incarnation.
    let raw = unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
            false,
            process,
        )
    }
    .map_err(win)?;
    let file = unsafe { OwnedHandle::from_raw_handle(raw.0) };
    Ok(DownloadWriter {
        process,
        created: process_created(HANDLE(file.as_raw_handle()))?,
    })
}
fn process_created(process: HANDLE) -> Result<u64, DownloadError> {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::GetProcessTimes;
    let mut created = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    // SAFETY: live process handle with query rights and four correctly sized outputs.
    unsafe { GetProcessTimes(process, &mut created, &mut exit, &mut kernel, &mut user) }
        .map_err(win)?;
    let value = (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime);
    if value == 0 {
        Err(DownloadError::Unavailable)
    } else {
        Ok(value)
    }
}
fn writer_exited(owner: &DownloadWriter) -> bool {
    use windows::Win32::System::Threading::{
        OpenProcess, WaitForSingleObject, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
    };
    let raw = match unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
            false,
            owner.process,
        )
    } {
        Ok(handle) => handle,
        Err(error) => return error.code().0 as u32 & 0xffff == 87,
    };
    let handle = unsafe { OwnedHandle::from_raw_handle(raw.0) };
    let process = HANDLE(handle.as_raw_handle());
    let Ok(created) = process_created(process) else {
        return false;
    };
    created != owner.created
        || unsafe { WaitForSingleObject(process, 0) } == windows::Win32::Foundation::WAIT_OBJECT_0
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn zone_verification_rejects_ambiguous_or_unrelated_policy() {
        assert!(protected_zone("[ZoneTransfer]\r\nZoneId=3\r\n"));
        assert!(protected_zone(
            "[ZoneTransfer]\nZoneId=4\nHostUrl=https://example.com\n"
        ));
        for value in [
            "[ZoneTransfer]\nZoneId=0\n",
            "[ZoneTransfer]\n[Other]\nZoneId=3",
            "[ZoneTransfer]\nZoneId=3\nZoneId=0",
            "[ZoneTransfer]\nZoneId=3\n[ZoneTransfer]\nZoneId=3",
        ] {
            assert!(!protected_zone(value));
        }
    }
    #[test]
    fn cleanup_cannot_remove_an_open_writer() {
        let root = tempfile::tempdir().unwrap();
        let pending =
            Destination::prepare(DownloadId::generate(), root.path().join("file.txt"), None)
                .unwrap();
        let writer = File::create(pending.payload()).unwrap();
        assert!(cleanup_payload(&pending.payload()).is_err());
        assert!(pending.payload().exists());
        drop(writer);
        assert!(cleanup_payload(&pending.payload()).is_ok());
    }
    #[test]
    fn windows_publication_is_exclusive_marked_and_identity_checked() {
        let root = tempfile::tempdir().unwrap();
        let output = root.path().join("report.txt");
        fs::write(&output, b"existing").unwrap();
        let pending = Destination::prepare(DownloadId::generate(), output.clone(), None).unwrap();
        fs::write(pending.payload(), b"download").unwrap();
        let (saved, receipt) = pending.finish("https://example.com", Some(8)).unwrap();
        assert_eq!(fs::read(output).unwrap(), b"existing");
        assert_eq!(fs::read(&saved).unwrap(), b"download");
        assert!(verify_file(&saved, &receipt).is_ok());
        fs::write(&saved, b"modified").unwrap();
        assert!(verify_file(&saved, &receipt).is_err());
    }
    #[test]
    fn recovery_refuses_a_substituted_staging_directory() {
        let root = tempfile::tempdir().unwrap();
        let pending =
            Destination::prepare(DownloadId::generate(), root.path().join("file.txt"), None)
                .unwrap();
        let mut wrong = pending.staging_identity.clone();
        wrong.file_high ^= 1;
        let record = DownloadRecord {
            id: DownloadId::parse(
                pending
                    .staging_path
                    .file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .trim_start_matches(".zephium-download-"),
            )
            .unwrap(),
            session: DownloadId::generate(),
            revision: 1,
            created_at: 1,
            filename: "file.txt".into(),
            source: "https://example.com".into(),
            source_is_context: false,
            state: zephium_core::downloads::DownloadState::Interrupted,
            received: 0,
            total: None,
            error: None,
            destination: None,
            staging: Some(pending.staging_path.clone()),
            staging_identity: Some(wrong),
            identity: None,
            writer: None,
            writer_released: false,
        };
        fs::write(pending.payload(), b"keep").unwrap();
        assert!(recover_staging(&record).is_err());
        assert_eq!(fs::read(pending.payload()).unwrap(), b"keep");
    }
    #[test]
    fn native_byte_mismatch_is_rejected_before_publication() {
        let root = tempfile::tempdir().unwrap();
        let output = root.path().join("file.txt");
        let pending = Destination::prepare(DownloadId::generate(), output.clone(), None).unwrap();
        fs::write(pending.payload(), b"short").unwrap();
        assert!(matches!(
            pending.finish("https://example.com", Some(100)),
            Err(DownloadError::Integrity)
        ));
        assert!(!output.exists());
    }
}
