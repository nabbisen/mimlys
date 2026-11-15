//! The directory permissions an AppContainer needs (RFC 011 §3, §7;
//! REQ-CONF-008): one access-control entry for the container SID on each
//! path of the read set (read and execute) and of the write set (read,
//! write, execute and delete: never `WRITE_DAC`, `WRITE_OWNER` or
//! `GENERIC_ALL`, so a tool can change no file's permissions; the mask and
//! the reason for each right are in `acl_rules::WRITE_MASK`), added by
//! `mimlys toolchain grant` and removed by `revoke`, inheritable to what lies
//! beneath. Nothing else in the directory's security descriptor is touched:
//! the entries the directory inherits stay inherited and its protection flag
//! stays as it was; and `has_entry` reads the descriptor back rather than
//! trusting any record, and accepts an entry only with the wanted mask and
//! not inherited.
//!
//! A directory is opened once, by a handle that is neither made to follow
//! a link nor trusts its spelling (`FILE_FLAG_BACKUP_SEMANTICS`,
//! `FILE_FLAG_OPEN_REPARSE_POINT`; a link is resolved once, on a second open
//! of the same name), and the handle's identity (the volume's serial number
//! and the file id) names the lock; the permissions are read and written
//! through the handle (`GetSecurityInfo`, `SetSecurityInfo`), so that every
//! spelling of one directory (a different case, a short name, a trailing
//! separator, a junction, a `subst` drive) takes the same lock and the write
//! lands on the directory whose lock is held (A-23).
//!
//! A write is a merge into the DACL read a moment earlier, so one writer at
//! a time per directory holds a named mutex across the runtimes of this
//! machine, for the whole write (two installations granting their
//! containers one directory at once would otherwise lose one another's
//! entry; THR-36). The system propagates a write to every file beneath the
//! directory, so a write takes from 0.1 s to several seconds (D1), and a
//! second writer that read during the first one's call would read the base
//! as it was before it. A writer that finds the mutex held says so once in
//! the log, waits up to LIM-O7 and then **refuses** the write with
//! `acl_not_applied` and the reason (`the permissions of <dir> are being
//! written by another runtime`); it never writes on a base read while
//! another write was landing. The callers report the refusal as they report
//! any `acl_not_applied`: `toolchain grant` and `revoke` fail with it, and
//! the start of mimlys fails with it when its own directories cannot be
//! written (the next start applies them). An abandoned mutex (its holder
//! ended without releasing it) is acquired and the write proceeds: the
//! holder's write either landed or did not, and the writer reads the base
//! again under the lock, so it merges into whichever it was. A mutex the
//! system will not create or wait on is refused too, with a reason of its
//! own (`acl_not_applied: the permissions lock of <dir> cannot be taken:
//! <system error>`): there is no write without the lock (THR-36).
//! The start reads a directory's entry under the same lock as it writes it
//! ([`ensure_entry`]); a read that finds the descriptor changing under it is
//! repeated (`ERROR_INSUFFICIENT_BUFFER`, the "os error 122" of the CI).
//! The merge starts from the directory's explicit entries alone, and the
//! system recomputes the inherited ones from the parent on the write.
//! The write marks the descriptor auto-inherited, as every write through
//! this interface does; `icacls` then shows the entries the directory
//! inherits as `(I)`, which they were before the write too (their flag),
//! and the entries themselves are what they were.
//!
//! Every `unsafe` block is justified (REQ-CONF-010).

use crate::confine::acl_rules::{
    self, Ace, FILE_ALL_ACCESS, READ_ATTEMPTS, READ_CONTROL, WRITE_DAC, lock_name,
    retry_insufficient_buffer, wanted_mask,
};
pub use crate::confine::acl_rules::{Access, EntryState};
use crate::confine::dacl_wait::{Attempt, Cause, Decision, Refusal, decide};
use mimlys_core::limits::LIM_O7_PERMISSION_WRITE_WAIT_MS;
use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, ERROR_SUCCESS, HANDLE, INVALID_HANDLE_VALUE, LocalFree,
    WAIT_ABANDONED, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, EXPLICIT_ACCESS_W, GRANT_ACCESS,
    GetSecurityInfo, REVOKE_ACCESS, SDDL_REVISION_1, SE_FILE_OBJECT, SetEntriesInAclW,
    SetSecurityInfo, TRUSTEE_IS_SID, TRUSTEE_IS_UNKNOWN, TRUSTEE_W,
};
use windows_sys::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, ACL_REVISION, ACL_SIZE_INFORMATION, AclSizeInformation,
    AddAce, CONTAINER_INHERIT_ACE, DACL_SECURITY_INFORMATION, EqualSid, GetAce, GetAclInformation,
    GetLengthSid, GetSecurityDescriptorControl, INHERIT_ONLY_ACE, INHERITED_ACE, InitializeAcl,
    NO_INHERITANCE, OBJECT_INHERIT_ACE, OWNER_SECURITY_INFORMATION,
    PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SE_DACL_PROTECTED,
    SECURITY_ATTRIBUTES, SUB_CONTAINERS_AND_OBJECTS_INHERIT, TokenOwner, TokenUser,
    UNPROTECTED_DACL_SECURITY_INFORMATION,
};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, CreateDirectoryW, CreateFileW, FILE_ATTRIBUTE_DIRECTORY,
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_ID_INFO, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    FileIdInfo, GetFileInformationByHandle, GetFileInformationByHandleEx, OPEN_EXISTING,
};
use windows_sys::Win32::System::SystemServices::ACCESS_ALLOWED_ACE_TYPE;
use windows_sys::Win32::System::Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject};

/// Why a write of a directory's permissions did not happen.
#[derive(Debug)]
pub enum AclError {
    /// The directory's lock was not obtained: held past LIM-O7, or not
    /// available from the system (REQ-CONF-008, THR-36). The commands turn
    /// it into the error kind `unavailable` (external design §18).
    Refused(Refusal),
    /// The system refused or failed the read or the write.
    Failed(String),
}

impl std::fmt::Display for AclError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AclError::Refused(r) => r.fmt(f),
            AclError::Failed(text) => f.write_str(text),
        }
    }
}

impl std::error::Error for AclError {}

impl From<Refusal> for AclError {
    fn from(r: Refusal) -> Self {
        AclError::Refused(r)
    }
}

impl From<String> for AclError {
    fn from(text: String) -> Self {
        AclError::Failed(text)
    }
}

fn wide(path: &Path) -> Vec<u16> {
    OsStr::new(path)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// A directory (or a file) as its identity: the volume and the file id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Identity {
    volume: u64,
    file_id: [u8; 16],
}

/// What a handle says about its object.
struct Info {
    identity: Identity,
    is_dir: bool,
    is_link: bool,
}

/// Opens `path` itself for its permissions: `READ_CONTROL`, and `WRITE_DAC`
/// when `write`; shared with everyone; `FILE_FLAG_BACKUP_SEMANTICS` so that a
/// directory opens; and, when `reparse`, without following a link in the
/// last component.
fn open_handle(path: &Path, write: bool, reparse: bool) -> Result<OwnedHandle, String> {
    let path_w = wide(path);
    let mut access = READ_CONTROL | FILE_READ_ATTRIBUTES;
    if write {
        access |= WRITE_DAC;
    }
    let mut flags = FILE_FLAG_BACKUP_SEMANTICS;
    if reparse {
        flags |= FILE_FLAG_OPEN_REPARSE_POINT;
    }
    // SAFETY: the path is null-terminated UTF-16; no security attributes and no template file.
    let handle = unsafe {
        CreateFileW(
            path_w.as_ptr(),
            access,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            flags,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        let what = if write {
            "to change its permissions"
        } else {
            "to read its permissions"
        };
        return Err(format!(
            "cannot open {} {what}: {}",
            path.display(),
            std::io::Error::last_os_error()
        ));
    }
    // SAFETY: CreateFileW returned this handle for the caller to own.
    Ok(unsafe { OwnedHandle::from_raw_handle(handle as _) })
}

/// The identity, the kind and the link flag of the object a handle names.
fn info_of(handle: &OwnedHandle, path: &Path) -> Result<Info, String> {
    let raw = handle.as_raw_handle() as HANDLE;
    // SAFETY: a zeroed BY_HANDLE_FILE_INFORMATION is a valid value of this plain C struct.
    let mut basic: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: the handle is valid; the out struct matches the call.
    if unsafe { GetFileInformationByHandle(raw, &mut basic) } == 0 {
        return Err(format!(
            "cannot identify {}: {}",
            path.display(),
            std::io::Error::last_os_error()
        ));
    }
    let mut ids = FILE_ID_INFO::default();
    // The 128-bit id (ReFS has no unique 64-bit one); the 64-bit index where the system does not answer.
    // SAFETY: the handle is valid; the buffer is a FILE_ID_INFO of the size passed.
    let wide_id = unsafe {
        GetFileInformationByHandleEx(
            raw,
            FileIdInfo,
            (&mut ids as *mut FILE_ID_INFO).cast(),
            std::mem::size_of::<FILE_ID_INFO>() as u32,
        )
    } != 0;
    let identity = if wide_id {
        Identity {
            volume: ids.VolumeSerialNumber,
            file_id: ids.FileId.Identifier,
        }
    } else {
        let index = (u64::from(basic.nFileIndexHigh) << 32) | u64::from(basic.nFileIndexLow);
        let mut file_id = [0u8; 16];
        file_id[..8].copy_from_slice(&index.to_le_bytes());
        Identity {
            volume: u64::from(basic.dwVolumeSerialNumber),
            file_id,
        }
    };
    Ok(Info {
        identity,
        is_dir: basic.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0,
        is_link: basic.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0,
    })
}

/// Opens the object and returns its handle and what it says. The first open
/// does not follow a link; a link (a junction, a symbolic link) is resolved
/// by a second open of the same name, whose handle is the one returned, so
/// that the identity is always that of the handle the permissions are read
/// and written through.
fn open_object(path: &Path, write: bool) -> Result<(OwnedHandle, Info), String> {
    let first = open_handle(path, write, true)?;
    let info = info_of(&first, path)?;
    if !info.is_link {
        return Ok((first, info));
    }
    drop(first);
    let followed = open_handle(path, write, false)?;
    let info = info_of(&followed, path)?;
    Ok((followed, info))
}

/// A directory (or a file) opened for its permissions; its identity names
/// its lock.
struct Dir {
    read: OwnedHandle,
    path: PathBuf,
    identity: Identity,
    is_dir: bool,
}

impl Dir {
    fn open(path: &Path) -> Result<Self, String> {
        let (read, info) = open_object(path, false)?;
        Ok(Self {
            read,
            path: path.to_path_buf(),
            identity: info.identity,
            is_dir: info.is_dir,
        })
    }

    /// A handle that may write the permissions, of the same object: its
    /// identity is checked against the one the lock was named from.
    fn writer(&self) -> Result<OwnedHandle, String> {
        let (handle, info) = open_object(&self.path, true)?;
        if info.identity != self.identity {
            return Err(format!(
                "{} is no longer the directory whose lock is held: it was replaced while its permissions were being written; nothing was written",
                self.path.display()
            ));
        }
        Ok(handle)
    }

    fn lock_name(&self) -> String {
        lock_name(self.identity.volume, self.identity.file_id)
    }

    fn shown(&self) -> String {
        self.path.display().to_string()
    }
}

/// The directory's DACL and the descriptor that owns it.
struct Dacl {
    acl: *mut ACL,
    descriptor: PSECURITY_DESCRIPTOR,
}

impl Drop for Dacl {
    fn drop(&mut self) {
        if !self.descriptor.is_null() {
            // SAFETY: the descriptor was allocated by GetSecurityInfo for the caller to free once.
            unsafe { LocalFree(self.descriptor) };
        }
    }
}

/// Reads the DACL through the handle, again when the system answers
/// `ERROR_INSUFFICIENT_BUFFER` (the descriptor changed between the call that
/// sized it and the call that read it: a write landing on the same object).
fn read_dacl(dir: &Dir) -> Result<Dacl, String> {
    let handle = dir.read.as_raw_handle() as HANDLE;
    let mut acl: *mut ACL = std::ptr::null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    let err = retry_insufficient_buffer(
        READ_ATTEMPTS,
        |tries| {
            tracing::debug!(
                target: "mimlys_worker",
                "the permissions of {} changed while being read; reading again ({tries})",
                dir.shown()
            );
            std::thread::sleep(std::time::Duration::from_millis(10 * u64::from(tries)));
        },
        || {
            acl = std::ptr::null_mut();
            descriptor = std::ptr::null_mut();
            // SAFETY: the handle is valid; the out pointers are valid; only the DACL is requested.
            unsafe {
                GetSecurityInfo(
                    handle,
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    &mut acl,
                    std::ptr::null_mut(),
                    &mut descriptor,
                )
            }
        },
    );
    if err != ERROR_SUCCESS {
        return Err(format!(
            "GetSecurityInfo({}) failed: {}",
            dir.shown(),
            std::io::Error::from_raw_os_error(err as i32)
        ));
    }
    Ok(Dacl { acl, descriptor })
}

/// The named mutex that serializes the writers of one directory's DACL.
struct DaclLock {
    handle: HANDLE,
}

impl DaclLock {
    /// Takes the lock, waiting up to LIM-O7 when another writer holds it
    /// (one `info` line per wait). `Err` with the refusal when the wait runs
    /// out, or when the system will not create the mutex or wait on it:
    /// never a write on a stale base, never a write without the lock.
    fn acquire(dir: &Dir) -> Result<Self, Refusal> {
        Self::acquire_within(dir, LIM_O7_PERMISSION_WRITE_WAIT_MS)
    }

    /// [`acquire`](Self::acquire) with the wait given (LIM-O7 in production).
    fn acquire_within(dir: &Dir, limit_ms: u64) -> Result<Self, Refusal> {
        let shown = dir.shown();
        let name = wide(Path::new(&dir.lock_name()));
        // SAFETY: default security, not initially owned, a null-terminated name.
        let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
        if handle.is_null() {
            let error = std::io::Error::last_os_error().to_string();
            tracing::warn!(
                target: "mimlys_worker",
                "no mutex for the permissions of {shown}: {error}; the write is refused"
            );
            return Err(Refusal::new(Cause::LockUnavailable, &shown, &error));
        }
        let failure = std::cell::RefCell::new(String::new());
        let decision = decide(
            |timeout_ms| {
                // SAFETY: the handle was just created and is valid.
                match unsafe { WaitForSingleObject(handle, timeout_ms) } {
                    WAIT_OBJECT_0 => Attempt::Acquired,
                    WAIT_ABANDONED => Attempt::Abandoned,
                    WAIT_TIMEOUT => Attempt::Held,
                    _ => {
                        *failure.borrow_mut() = std::io::Error::last_os_error().to_string();
                        Attempt::Failed
                    }
                }
            },
            || {
                tracing::info!(
                    target: "mimlys_worker",
                    "waiting for another runtime's write of the permissions of {shown}"
                );
            },
            limit_ms,
        );
        match decision {
            Decision::Write => Ok(Self { handle }),
            Decision::WriteAfterAbandoned => {
                tracing::info!(
                    target: "mimlys_worker",
                    "the previous writer of the permissions of {shown} ended without releasing the lock; reading them again"
                );
                Ok(Self { handle })
            }
            Decision::Refuse(cause) => {
                // SAFETY: the handle is valid and closed exactly once.
                unsafe { CloseHandle(handle) };
                Err(Refusal::new(cause, &shown, &failure.borrow()))
            }
        }
    }
}

impl Drop for DaclLock {
    fn drop(&mut self) {
        // SAFETY: the mutex is owned by this thread and the handle is closed exactly once.
        unsafe {
            ReleaseMutex(self.handle);
            CloseHandle(self.handle);
        }
    }
}

/// The explicit entries of an ACL, as a new ACL: `SetEntriesInAclW` copies
/// an inherited entry as an explicit one, which would make the directory's
/// inheritance a set of copies that no `revoke` could undo; the system
/// recomputes the inherited entries from the parent on the write instead.
/// With `without`, the access entries of that SID are left out too: the
/// entry about to be written replaces whatever the SID held (an old, wider
/// one included), in the same write. A null ACL (everything allowed) stays
/// null.
fn explicit_entries(acl: *const ACL, without: Option<&[u8]>) -> Result<Option<Vec<u32>>, String> {
    if acl.is_null() {
        return Ok(None);
    }
    let mut info = ACL_SIZE_INFORMATION {
        AceCount: 0,
        AclBytesInUse: 0,
        AclBytesFree: 0,
    };
    // SAFETY: the ACL is valid for the caller's descriptor; the out struct matches the class.
    let ok = unsafe {
        GetAclInformation(
            acl,
            (&mut info as *mut ACL_SIZE_INFORMATION).cast(),
            std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
            AclSizeInformation,
        )
    };
    if ok == 0 {
        return Err(format!(
            "GetAclInformation failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    // The explicit entries fit in the old size; the length is a multiple of four as the ACL requires.
    let len = (info.AclBytesInUse as usize)
        .max(std::mem::size_of::<ACL>())
        .next_multiple_of(4);
    // Four-byte aligned, as an ACL is.
    let mut buffer = vec![0u32; len / 4];
    let new_acl = buffer.as_mut_ptr().cast::<ACL>();
    // SAFETY: the buffer is zeroed, aligned for the ACL header and of the length given.
    if unsafe { InitializeAcl(new_acl, len as u32, ACL_REVISION) } == 0 {
        return Err(format!(
            "InitializeAcl failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    for index in 0..info.AceCount {
        let mut ace: *mut std::ffi::c_void = std::ptr::null_mut();
        // SAFETY: the index is below the count the system reported; the out pointer is valid.
        if unsafe { GetAce(acl, index, &mut ace) } == 0 {
            return Err(format!(
                "GetAce failed: {}",
                std::io::Error::last_os_error()
            ));
        }
        // SAFETY: GetAce returned a pointer to an entry that starts with an ACE_HEADER.
        let header = unsafe { ace.cast::<ACE_HEADER>().read_unaligned() };
        tracing::trace!(
            target: "mimlys_worker",
            "dacl entry {index}: type {} flags {:#04x} size {}",
            header.AceType,
            header.AceFlags,
            header.AceSize
        );
        if u32::from(header.AceFlags) & INHERITED_ACE != 0 {
            continue;
        }
        if let Some(sid) = without
            && header.AceType == ACCESS_ALLOWED_ACE_TYPE as u8
        {
            // SAFETY: the entry is an ACCESS_ALLOWED_ACE (its type was checked); its SID starts at `SidStart`, inside the entry.
            let same = unsafe {
                let allowed = ace.cast::<ACCESS_ALLOWED_ACE>();
                EqualSid(
                    std::ptr::addr_of!((*allowed).SidStart) as PSID,
                    sid.as_ptr() as PSID,
                ) != 0
            };
            if same {
                continue;
            }
        }
        // SAFETY: the entry is `AceSize` bytes long and the new ACL has room for every entry of the old one.
        if unsafe {
            AddAce(
                new_acl,
                ACL_REVISION,
                u32::MAX,
                ace,
                u32::from(header.AceSize),
            )
        } == 0
        {
            return Err(format!(
                "AddAce failed: {}",
                std::io::Error::last_os_error()
            ));
        }
    }
    Ok(Some(buffer))
}

/// The access-allowed entries of the ACL that name `sid`.
fn entries_of(acl: *const ACL, sid: &[u8]) -> Result<Vec<Ace>, String> {
    if acl.is_null() {
        return Ok(Vec::new());
    }
    let mut info = ACL_SIZE_INFORMATION {
        AceCount: 0,
        AclBytesInUse: 0,
        AclBytesFree: 0,
    };
    // SAFETY: the ACL is valid for the caller's descriptor; the out struct matches the class.
    let ok = unsafe {
        GetAclInformation(
            acl,
            (&mut info as *mut ACL_SIZE_INFORMATION).cast(),
            std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
            AclSizeInformation,
        )
    };
    if ok == 0 {
        return Err(format!(
            "GetAclInformation failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    let mut found = Vec::new();
    for index in 0..info.AceCount {
        let mut ace: *mut std::ffi::c_void = std::ptr::null_mut();
        // SAFETY: the index is below the count the system reported; the out pointer is valid.
        if unsafe { GetAce(acl, index, &mut ace) } == 0 {
            return Err(format!(
                "GetAce failed: {}",
                std::io::Error::last_os_error()
            ));
        }
        // SAFETY: GetAce returned a pointer to an entry that starts with an ACE_HEADER.
        let header = unsafe { ace.cast::<ACE_HEADER>().read_unaligned() };
        if header.AceType != ACCESS_ALLOWED_ACE_TYPE as u8 {
            continue;
        }
        // SAFETY: the entry is an ACCESS_ALLOWED_ACE (its type was checked); its SID starts at `SidStart`, inside the entry.
        let (mask, same) = unsafe {
            let allowed = ace.cast::<ACCESS_ALLOWED_ACE>();
            let entry_sid = std::ptr::addr_of!((*allowed).SidStart) as PSID;
            (
                std::ptr::addr_of!((*allowed).Mask).read_unaligned(),
                EqualSid(entry_sid, sid.as_ptr() as PSID) != 0,
            )
        };
        if same {
            let flags = u32::from(header.AceFlags);
            found.push(Ace {
                mask,
                inherited: flags & INHERITED_ACE != 0,
                inherit_only: flags & INHERIT_ONLY_ACE != 0,
            });
        }
    }
    Ok(found)
}

/// What the directory's entries for the SID amount to, for the access asked
/// (read back from the directory, not from a record).
fn state_of(dir: &Dir, sid: &[u8], access: Access) -> Result<EntryState, String> {
    let dacl = read_dacl(dir)?;
    Ok(acl_rules::classify(access, &entries_of(dacl.acl, sid)?))
}

/// Writes `new_acl` as the object's DACL through a handle that may.
fn write_dacl(dir: &Dir, new_acl: *mut ACL, protection: u32) -> Result<(), String> {
    let handle = dir.writer()?;
    // SAFETY: the handle is valid and was opened with WRITE_DAC; only the DACL is replaced, with the ACL given.
    let err = unsafe {
        SetSecurityInfo(
            handle.as_raw_handle() as HANDLE,
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | protection,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            new_acl,
            std::ptr::null(),
        )
    };
    if err != ERROR_SUCCESS {
        return Err(format!(
            "SetSecurityInfo({}) failed: {}",
            dir.shown(),
            std::io::Error::from_raw_os_error(err as i32)
        ));
    }
    Ok(())
}

/// The trustee of a SID.
fn trustee(sid: &[u8]) -> TRUSTEE_W {
    TRUSTEE_W {
        pMultipleTrustee: std::ptr::null_mut(),
        MultipleTrusteeOperation: 0,
        TrusteeForm: TRUSTEE_IS_SID,
        TrusteeType: TRUSTEE_IS_UNKNOWN,
        ptstrName: sid.as_ptr() as *mut u16,
    }
}

/// The merge itself, under the directory's lock: the entry for the SID with
/// `mask`, in place of every entry the SID held (`GRANT_ACCESS`), or none
/// (`REVOKE_ACCESS`).
fn merge_entry(dir: &Dir, sid: &[u8], mask: u32, mode: i32) -> Result<(), String> {
    let old = read_dacl(dir)?;
    let mut explicit = explicit_entries(old.acl, (mode == GRANT_ACCESS).then_some(sid))?;
    let base: *mut ACL = explicit
        .as_mut()
        .map_or(std::ptr::null_mut(), |b| b.as_mut_ptr().cast());
    // The directory's protection flag, kept as it is: with `UNPROTECTED` the
    // system recomputes the inherited entries from the parent on the write.
    let mut control: u16 = 0;
    let mut revision: u32 = 0;
    // SAFETY: the descriptor lives in `old`; both out pointers are valid.
    let read_control =
        unsafe { GetSecurityDescriptorControl(old.descriptor, &mut control, &mut revision) };
    let protection = if read_control != 0 && control & SE_DACL_PROTECTED != 0 {
        PROTECTED_DACL_SECURITY_INFORMATION
    } else {
        UNPROTECTED_DACL_SECURITY_INFORMATION
    };
    tracing::debug!(
        target: "mimlys_worker",
        "dacl of {}: control {control:#06x}, mode {mode}, mask {mask:#010x}, explicit base {}",
        dir.shown(),
        if explicit.is_some() { "built" } else { "null" }
    );
    let entry = EXPLICIT_ACCESS_W {
        grfAccessPermissions: mask,
        grfAccessMode: mode,
        // A directory's entry is inherited by what is beneath it (a toolchain's
        // files need it: Windows checks the file). A file's entry (the worker, an
        // interpreter module: RFC 017) is on that file alone and inherited by
        // nothing.
        grfInheritance: if dir.is_dir {
            SUB_CONTAINERS_AND_OBJECTS_INHERIT
        } else {
            NO_INHERITANCE
        },
        Trustee: trustee(sid),
    };
    let mut new_acl: *mut ACL = std::ptr::null_mut();
    // SAFETY: one entry is given; the base ACL lives in `explicit` (or is null); `new_acl` receives an allocation freed below.
    let err = unsafe { SetEntriesInAclW(1, &entry, base, &mut new_acl) };
    if err != ERROR_SUCCESS {
        return Err(format!(
            "SetEntriesInAclW failed: {}",
            std::io::Error::from_raw_os_error(err as i32)
        ));
    }
    let written = write_dacl(dir, new_acl, protection);
    if !new_acl.is_null() {
        // SAFETY: the new ACL was allocated by SetEntriesInAclW for the caller to free once.
        unsafe { LocalFree(new_acl.cast()) };
    }
    written
}

/// Gives the SID the entry `access` asks for, in place of whatever entry it
/// held, under the directory's lock.
fn set_entry(path: &Path, sid: &[u8], access: Access, mode: i32) -> Result<(), AclError> {
    let dir = Dir::open(path)?;
    let _lock = DaclLock::acquire(&dir)?;
    merge_entry(&dir, sid, wanted_mask(access), mode).map_err(AclError::Failed)
}

/// What [`ensure_entry`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ensured {
    /// The wanted entry was already in force; nothing was written.
    AlreadyApplied,
    /// The directory had no entry of its own for the container; it was added.
    Applied,
    /// The directory held an entry that was not the wanted one (the
    /// `GENERIC_ALL` of 0.10 and earlier); it was replaced by the wanted one
    /// in one write.
    Replaced,
}

/// The start's path for one of mimlys's own directories or files: reads the
/// directory's entry and, when it is not the wanted one, writes it, **all
/// under the directory's lock**, so that the read can neither see a
/// descriptor another runtime is rewriting nor decide on a base that
/// changes before the write. An entry that is not the wanted one is replaced
/// in the same write, and said so once in the log.
pub fn ensure_entry(path: &Path, sid: &[u8], access: Access) -> Result<Ensured, AclError> {
    let dir = Dir::open(path)?;
    let _lock = DaclLock::acquire(&dir)?;
    let state = state_of(&dir, sid, access)?;
    let held = match state {
        EntryState::Applied => return Ok(Ensured::AlreadyApplied),
        EntryState::Absent => None,
        EntryState::Wrong { mask } => Some(mask),
    };
    if let Some(mask) = held {
        tracing::info!(
            target: "mimlys_worker",
            "replacing the container's permissions on {}: they were {mask:#x}, wider than read, write, execute and delete (a tool could change the permissions of its files); they are now {:#x}",
            dir.shown(),
            wanted_mask(access)
        );
    }
    merge_entry(&dir, sid, wanted_mask(access), GRANT_ACCESS)?;
    Ok(if held.is_some() {
        Ensured::Replaced
    } else {
        Ensured::Applied
    })
}

/// Adds the container's entry to the directory's DACL (in place of any entry
/// the container held there).
pub fn grant(path: &Path, sid: &[u8], access: Access) -> Result<(), AclError> {
    set_entry(path, sid, access, GRANT_ACCESS)
}

/// Removes every entry of the container SID from the directory's DACL.
pub fn revoke(path: &Path, sid: &[u8]) -> Result<(), AclError> {
    set_entry(path, sid, Access::Read, REVOKE_ACCESS)
}

/// What the directory's entries for the SID amount to for `access`, read
/// back from the directory (not from a record), without a lock: the read
/// commands wait for nothing (REQ-PERS-013), and a read that meets a
/// descriptor being rewritten is repeated.
pub fn entry_state(path: &Path, sid: &[u8], access: Access) -> Result<EntryState, String> {
    state_of(&Dir::open(path)?, sid, access)
}

/// Whether the directory carries the entry `access` asks for: an entry of
/// its own (not inherited) with the wanted mask and no `WRITE_DAC` or
/// `WRITE_OWNER`. The entry of 0.10 and earlier (`GENERIC_ALL`) is not it.
pub fn has_entry(path: &Path, sid: &[u8], access: Access) -> Result<bool, String> {
    Ok(entry_state(path, sid, access)? == EntryState::Applied)
}

/// Whether the directory carries any entry of its own for the SID, whatever
/// its mask (what `revoke --all` must find, the old wide entry included).
pub fn has_any_entry(path: &Path, sid: &[u8]) -> Result<bool, String> {
    Ok(entry_state(path, sid, Access::Read)? != EntryState::Absent)
}

/// The SID a token class names first (`TokenUser`: the user; `TokenOwner`:
/// the owner the system gives what the user creates), copied out of the
/// process token.
fn token_sid(class: i32) -> Result<Vec<u8>, String> {
    let token = super::own_token().ok_or("cannot open the process token")?;
    let bytes = super::token_bytes(&token, class).ok_or("cannot read the process token")?;
    if bytes.len() < std::mem::size_of::<PSID>() {
        return Err("the token's SID is shorter than a pointer".into());
    }
    // SAFETY: both TOKEN_USER and TOKEN_OWNER start with a SID pointer that refers into the same buffer. Read unaligned.
    let sid: PSID = unsafe { bytes.as_ptr().cast::<PSID>().read_unaligned() };
    if sid.is_null() {
        return Err("the token has no SID".into());
    }
    // SAFETY: the SID is valid (it lives in `bytes`).
    let len = unsafe { GetLengthSid(sid) } as usize;
    // SAFETY: the SID is `len` readable bytes, inside `bytes`.
    Ok(unsafe { std::slice::from_raw_parts(sid.cast::<u8>(), len) }.to_vec())
}

/// The current user's SID, copied out of the process token.
fn current_user_sid() -> Result<Vec<u8>, String> {
    token_sid(TokenUser)
}

/// The owner of the object a handle names, as a SID.
fn owner_of(dir: &Dir) -> Result<Vec<u8>, String> {
    let handle = dir.read.as_raw_handle() as HANDLE;
    let mut owner: PSID = std::ptr::null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    // SAFETY: the handle is valid; the out pointers are valid; only the owner is requested.
    let err = unsafe {
        GetSecurityInfo(
            handle,
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION,
            &mut owner,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut descriptor,
        )
    };
    if err != ERROR_SUCCESS {
        return Err(format!(
            "GetSecurityInfo({}) failed: {}",
            dir.shown(),
            std::io::Error::from_raw_os_error(err as i32)
        ));
    }
    let bytes = if owner.is_null() {
        Vec::new()
    } else {
        // SAFETY: the SID is valid (it lives in the descriptor, freed below).
        let len = unsafe { GetLengthSid(owner) } as usize;
        // SAFETY: the SID is `len` readable bytes, inside the descriptor.
        unsafe { std::slice::from_raw_parts(owner.cast::<u8>(), len) }.to_vec()
    };
    if !descriptor.is_null() {
        // SAFETY: the descriptor was allocated by GetSecurityInfo for the caller to free once.
        unsafe { LocalFree(descriptor) };
    }
    Ok(bytes)
}

/// Refuses a directory whose owner is not the current user (or the owner
/// the system gives the user's new objects). The owner of a directory can
/// always change its permissions, so a directory owned by anyone else cannot
/// be made private to the user (REQ-WS-001, A-23).
fn check_owner(dir: &Dir) -> Result<(), String> {
    let owner = owner_of(dir)?;
    let user = current_user_sid()?;
    let default_owner = token_sid(TokenOwner)?;
    if acl_rules::owner_is_acceptable(&owner, &user, &default_owner) {
        return Ok(());
    }
    Err(format!(
        "the data directory {} is owned by {}, not by the current user ({}); its owner can change who may reach it, so mimlys will not use it. Set runtime.data_dir to a directory you own, or take ownership of this one",
        dir.shown(),
        super::sid_string(&owner),
        super::sid_string(&user)
    ))
}

/// Whether the directory's owner is the current user; the refusal's text when not.
pub fn check_owner_of(path: &Path) -> Result<(), String> {
    check_owner(&Dir::open(path)?)
}

/// Whether the directory's DACL is already the one `protect_to_current_user`
/// writes: protected, one explicit allow entry, for this SID, inheritable.
/// Reading is cheap; a write propagates to everything beneath the directory,
/// so it is not repeated when nothing changed.
fn is_private_to(dir: &Dir, sid: &[u8]) -> Result<bool, String> {
    let dacl = read_dacl(dir)?;
    if dacl.acl.is_null() {
        return Ok(false);
    }
    let mut control: u16 = 0;
    let mut revision: u32 = 0;
    // SAFETY: the descriptor lives in `dacl`; both out pointers are valid.
    let read_control =
        unsafe { GetSecurityDescriptorControl(dacl.descriptor, &mut control, &mut revision) };
    if read_control == 0 || control & SE_DACL_PROTECTED == 0 {
        return Ok(false);
    }
    let mut info = ACL_SIZE_INFORMATION {
        AceCount: 0,
        AclBytesInUse: 0,
        AclBytesFree: 0,
    };
    // SAFETY: the ACL lives in `dacl`; the out struct matches the class.
    let ok = unsafe {
        GetAclInformation(
            dacl.acl,
            (&mut info as *mut ACL_SIZE_INFORMATION).cast(),
            std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
            AclSizeInformation,
        )
    };
    if ok == 0 || info.AceCount != 1 {
        return Ok(false);
    }
    let mut ace: *mut std::ffi::c_void = std::ptr::null_mut();
    // SAFETY: index 0 is below the count the system reported; the out pointer is valid.
    if unsafe { GetAce(dacl.acl, 0, &mut ace) } == 0 {
        return Ok(false);
    }
    // SAFETY: GetAce returned a pointer to an entry that starts with an ACE_HEADER.
    let header = unsafe { ace.cast::<ACE_HEADER>().read_unaligned() };
    let inherit = (OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE) as u8;
    if header.AceType != ACCESS_ALLOWED_ACE_TYPE as u8
        || u32::from(header.AceFlags) & INHERITED_ACE != 0
        || header.AceFlags & inherit != inherit
    {
        return Ok(false);
    }
    // SAFETY: the entry is an ACCESS_ALLOWED_ACE (its type was checked); its SID starts at `SidStart`, inside the entry.
    let same = unsafe {
        let allowed = ace.cast::<ACCESS_ALLOWED_ACE>();
        let entry_sid = std::ptr::addr_of!((*allowed).SidStart) as PSID;
        EqualSid(entry_sid, sid.as_ptr() as PSID) != 0
    };
    Ok(same)
}

/// Whether the directory is the one `create_protected_dir` makes (tests).
pub fn is_private_to_current_user(path: &Path) -> Result<bool, String> {
    let sid = current_user_sid()?;
    is_private_to(&Dir::open(path)?, &sid)
}

/// Gives the directory a protected DACL with one entry: the current user,
/// full control, inherited by everything created beneath (REQ-WS-001,
/// external design §14.1). Nothing is inherited from the parent, so no one
/// else (the system, the administrators, other users) is granted anything
/// through the directory; the owner keeps the right to change the DACL
/// whatever it says, and the current user's entry is in the DACL that
/// replaces the old one in the same call, so mimlys is never locked out.
/// Called at every open, before the runtime adds the container's entries on
/// the subdirectories, so that the entries it adds are the only others; when
/// the DACL already is this one nothing is written. A directory whose owner
/// is not the current user is refused before anything is written.
pub fn protect_to_current_user(path: &Path) -> Result<(), AclError> {
    let sid = current_user_sid()?;
    let dir = Dir::open(path)?;
    check_owner(&dir)?;
    let _lock = DaclLock::acquire(&dir)?;
    protect_locked(&dir, &sid).map_err(AclError::Failed)
}

/// The write of [`protect_to_current_user`], under the directory's lock.
fn protect_locked(dir: &Dir, sid: &[u8]) -> Result<(), String> {
    if is_private_to(dir, sid)? {
        return Ok(());
    }
    // A specific mask, not GENERIC_ALL: the system turns an inheritable
    // entry with a generic right into two (this directory, and inherit-only),
    // and `is_private_to` expects the one entry written here.
    let entry = EXPLICIT_ACCESS_W {
        grfAccessPermissions: FILE_ALL_ACCESS,
        grfAccessMode: GRANT_ACCESS,
        grfInheritance: SUB_CONTAINERS_AND_OBJECTS_INHERIT,
        Trustee: trustee(sid),
    };
    let mut new_acl: *mut ACL = std::ptr::null_mut();
    // SAFETY: one entry is given; there is no base ACL; `new_acl` receives an allocation freed below.
    let err = unsafe { SetEntriesInAclW(1, &entry, std::ptr::null(), &mut new_acl) };
    if err != ERROR_SUCCESS {
        return Err(format!(
            "SetEntriesInAclW failed: {}",
            std::io::Error::from_raw_os_error(err as i32)
        ));
    }
    let written = write_dacl(dir, new_acl, PROTECTED_DACL_SECURITY_INFORMATION);
    if !new_acl.is_null() {
        // SAFETY: the new ACL was allocated by SetEntriesInAclW for the caller to free once.
        unsafe { LocalFree(new_acl.cast()) };
    }
    written
}

/// Creates the directory (its parents exist) **with** its protected
/// descriptor, the current user's full control alone and inherited, so that
/// it is never visible with the permissions of its parent (A-23): the
/// descriptor goes to `CreateDirectoryW` as its security attributes. A
/// directory that already exists is left as it is (the caller then protects
/// it).
pub fn create_protected_dir(path: &Path) -> Result<(), AclError> {
    let sid = current_user_sid()?;
    // Protected (`P`), one allow entry for the user, full access (`FA`), for the directory and what is created beneath (`OICI`).
    let sddl = format!("D:P(A;OICI;FA;;;{})", super::sid_string(&sid));
    let sddl_w = wide(Path::new(&sddl));
    let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    // SAFETY: the string is null-terminated UTF-16; the out pointer is valid; `descriptor` receives an allocation freed below.
    let converted = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl_w.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            std::ptr::null_mut(),
        )
    };
    if converted == 0 {
        return Err(AclError::Failed(format!(
            "cannot build the private descriptor for {}: {}",
            path.display(),
            std::io::Error::last_os_error()
        )));
    }
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    let path_w = wide(path);
    // SAFETY: the path is null-terminated UTF-16; the attributes and the descriptor they point at outlive the call.
    let created = unsafe { CreateDirectoryW(path_w.as_ptr(), &attributes) };
    let error = std::io::Error::last_os_error();
    // SAFETY: the descriptor was allocated by the conversion for the caller to free once.
    unsafe { LocalFree(descriptor) };
    if created == 0 && error.raw_os_error() != Some(ERROR_ALREADY_EXISTS as i32) {
        return Err(AclError::Failed(format!(
            "cannot create {}: {error}",
            path.display()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
