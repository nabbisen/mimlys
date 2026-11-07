//! The directory permissions an AppContainer needs (RFC 011 §3, §7;
//! REQ-CONF-008): one access-control entry for the container SID on each
//! path of the read set (read and execute) and of the write set (full
//! access), added by `mimlys toolchain grant` and removed by `revoke`,
//! inheritable to what lies beneath. Nothing else in the directory's
//! security descriptor is touched: the entries the directory inherits stay
//! inherited and its protection flag stays as it was; and `has_entry` reads
//! the descriptor back rather than trusting any record.
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
//! system refuses to create or to wait on leaves no holder to wait for; the
//! write goes ahead and the log says so (`dacl_wait::Decision`).
//! The merge starts from the directory's explicit entries alone, and the
//! system recomputes the inherited ones from the parent on the write.
//! The write marks the descriptor auto-inherited, as every write through
//! this interface does; `icacls` then shows the entries the directory
//! inherits as `(I)`, which they were before the write too (their flag),
//! and the entries themselves are what they were.
//!
//! Every `unsafe` block is justified (REQ-CONF-010).

use crate::confine::dacl_wait::{Attempt, Decision, decide_with_limit, refusal_reason};
use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_SUCCESS, GENERIC_ALL, GENERIC_EXECUTE, GENERIC_READ, HANDLE, LocalFree,
    WAIT_ABANDONED, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Security::Authorization::{
    EXPLICIT_ACCESS_W, GRANT_ACCESS, GetExplicitEntriesFromAclW, GetNamedSecurityInfoW,
    REVOKE_ACCESS, SE_FILE_OBJECT, SetEntriesInAclW, SetNamedSecurityInfoW, TRUSTEE_IS_SID,
    TRUSTEE_IS_UNKNOWN, TRUSTEE_W,
};
use windows_sys::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, ACL_REVISION, ACL_SIZE_INFORMATION, AclSizeInformation,
    AddAce, CONTAINER_INHERIT_ACE, DACL_SECURITY_INFORMATION, EqualSid, GetAce, GetAclInformation,
    GetLengthSid, GetSecurityDescriptorControl, INHERITED_ACE, InitializeAcl, OBJECT_INHERIT_ACE,
    PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SE_DACL_PROTECTED,
    SUB_CONTAINERS_AND_OBJECTS_INHERIT, TOKEN_USER, TokenUser,
    UNPROTECTED_DACL_SECURITY_INFORMATION,
};
use windows_sys::Win32::System::SystemServices::ACCESS_ALLOWED_ACE_TYPE;
use windows_sys::Win32::System::Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject};

/// `FILE_ALL_ACCESS` (winnt.h): every specific right on a file or directory.
/// The `Win32_Storage_FileSystem` feature of `windows-sys` is not enabled
/// for this one constant.
const FILE_ALL_ACCESS: u32 = 0x001F_01FF;

/// What an entry grants.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Access {
    /// Read and execute: the toolchain and the system paths.
    Read,
    /// Full access: the workspaces and the temporary directory.
    Write,
}

impl Access {
    fn mask(self) -> u32 {
        match self {
            Access::Read => GENERIC_READ | GENERIC_EXECUTE,
            Access::Write => GENERIC_ALL,
        }
    }
}

fn wide(path: &Path) -> Vec<u16> {
    OsStr::new(path)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// The directory's DACL and the descriptor that owns it.
struct Dacl {
    acl: *mut ACL,
    descriptor: PSECURITY_DESCRIPTOR,
}

impl Drop for Dacl {
    fn drop(&mut self) {
        // SAFETY: the descriptor was allocated by GetNamedSecurityInfoW for the caller to free once.
        unsafe { LocalFree(self.descriptor) };
    }
}

fn read_dacl(path: &Path) -> Result<Dacl, String> {
    let path_w = wide(path);
    let mut acl: *mut ACL = std::ptr::null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    // SAFETY: the path is null-terminated UTF-16; the out pointers are valid; only the DACL is requested.
    let err = unsafe {
        GetNamedSecurityInfoW(
            path_w.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut acl,
            std::ptr::null_mut(),
            &mut descriptor,
        )
    };
    if err != ERROR_SUCCESS {
        return Err(format!(
            "GetNamedSecurityInfoW({}) failed: {}",
            path.display(),
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
    /// (one `info` line per wait). `Ok(None)` when the system gives no mutex
    /// to wait on (the write goes ahead, noted in the log); `Err` with the
    /// `acl_not_applied` reason when the wait runs out: never a write on a
    /// stale base.
    fn acquire(path: &Path) -> Result<Option<Self>, String> {
        let mut key: u64 = 0xcbf2_9ce4_8422_2325;
        for b in path.to_string_lossy().to_lowercase().bytes() {
            key ^= u64::from(b);
            key = key.wrapping_mul(0x0100_0000_01b3);
        }
        let name = wide(Path::new(&format!("Global\\mimlys-dacl-{key:016x}")));
        // SAFETY: default security, not initially owned, a null-terminated name.
        let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
        if handle.is_null() {
            tracing::debug!(
                target: "mimlys_worker",
                "no mutex for the permissions of {}: {}; writing without it",
                path.display(),
                std::io::Error::last_os_error()
            );
            return Ok(None);
        }
        let decision = decide_with_limit(
            |timeout_ms| {
                // SAFETY: the handle was just created and is valid.
                match unsafe { WaitForSingleObject(handle, timeout_ms) } {
                    WAIT_OBJECT_0 => Attempt::Acquired,
                    WAIT_ABANDONED => Attempt::Abandoned,
                    WAIT_TIMEOUT => Attempt::Held,
                    _ => Attempt::Failed,
                }
            },
            || {
                tracing::info!(
                    target: "mimlys_worker",
                    "waiting for another runtime's write of the permissions of {}",
                    path.display()
                );
            },
        );
        match decision {
            Decision::Write => Ok(Some(Self { handle })),
            Decision::WriteAfterAbandoned => {
                tracing::info!(
                    target: "mimlys_worker",
                    "the previous writer of the permissions of {} ended without releasing the lock; reading them again",
                    path.display()
                );
                Ok(Some(Self { handle }))
            }
            Decision::WriteUnlocked => {
                tracing::warn!(
                    target: "mimlys_worker",
                    "the wait for the permissions of {} failed: {}; writing without the lock",
                    path.display(),
                    std::io::Error::last_os_error()
                );
                // SAFETY: the handle is valid and closed exactly once.
                unsafe { CloseHandle(handle) };
                Ok(None)
            }
            Decision::Refuse => {
                // SAFETY: the handle is valid and closed exactly once.
                unsafe { CloseHandle(handle) };
                Err(refusal_reason(&path.display().to_string()))
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
/// A null ACL (everything allowed) stays null.
fn explicit_entries(acl: *const ACL) -> Result<Option<Vec<u8>>, String> {
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
    let mut buffer = vec![0u8; len];
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

fn set_entry(path: &Path, sid: &[u8], access: Access, mode: i32) -> Result<(), String> {
    let _lock = DaclLock::acquire(path)?;
    let old = read_dacl(path)?;
    let mut explicit = explicit_entries(old.acl)?;
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
        "dacl of {}: control {control:#06x}, mode {mode}, explicit base {}",
        path.display(),
        if explicit.is_some() { "built" } else { "null" }
    );
    let entry = EXPLICIT_ACCESS_W {
        grfAccessPermissions: access.mask(),
        grfAccessMode: mode,
        grfInheritance: SUB_CONTAINERS_AND_OBJECTS_INHERIT,
        Trustee: TRUSTEE_W {
            pMultipleTrustee: std::ptr::null_mut(),
            MultipleTrusteeOperation: 0,
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_UNKNOWN,
            ptstrName: sid.as_ptr() as *mut u16,
        },
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
    let mut path_w = wide(path);
    // SAFETY: the path is null-terminated UTF-16; only the DACL is replaced, with the ACL just built.
    let err = unsafe {
        SetNamedSecurityInfoW(
            path_w.as_mut_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | protection,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            new_acl,
            std::ptr::null(),
        )
    };
    // SAFETY: the new ACL was allocated by SetEntriesInAclW for the caller to free once.
    unsafe { LocalFree(new_acl.cast()) };
    if err != ERROR_SUCCESS {
        return Err(format!(
            "SetNamedSecurityInfoW({}) failed: {}",
            path.display(),
            std::io::Error::from_raw_os_error(err as i32)
        ));
    }
    Ok(())
}

/// Adds the container's entry to the directory's DACL.
pub fn grant(path: &Path, sid: &[u8], access: Access) -> Result<(), String> {
    set_entry(path, sid, access, GRANT_ACCESS)
}

/// Removes every entry of the container SID from the directory's DACL.
pub fn revoke(path: &Path, sid: &[u8]) -> Result<(), String> {
    set_entry(path, sid, Access::Read, REVOKE_ACCESS)
}

/// Whether the directory's DACL carries an explicit entry for the SID (read
/// back from the directory, not from a record).
pub fn has_entry(path: &Path, sid: &[u8]) -> Result<bool, String> {
    let dacl = read_dacl(path)?;
    if dacl.acl.is_null() {
        return Ok(false);
    }
    let mut count: u32 = 0;
    let mut entries: *mut EXPLICIT_ACCESS_W = std::ptr::null_mut();
    // SAFETY: the ACL lives in `dacl`; the out pointers are valid; `entries` receives an allocation freed below.
    let err = unsafe { GetExplicitEntriesFromAclW(dacl.acl, &mut count, &mut entries) };
    if err != ERROR_SUCCESS {
        return Err(format!(
            "GetExplicitEntriesFromAclW failed: {}",
            std::io::Error::from_raw_os_error(err as i32)
        ));
    }
    if count == 0 || entries.is_null() {
        // No explicit entry at all (a fresh directory inherits everything): nothing to free.
        return Ok(false);
    }
    // SAFETY: `count` is non-zero and `entries` non-null and readable for `count` entries; a SID trustee's name is a SID pointer.
    let found = unsafe {
        std::slice::from_raw_parts(entries, count as usize)
            .iter()
            .any(|e| {
                e.Trustee.TrusteeForm == TRUSTEE_IS_SID
                    && e.grfAccessMode == GRANT_ACCESS
                    && EqualSid(e.Trustee.ptstrName.cast(), sid.as_ptr() as PSID) != 0
            })
    };
    // SAFETY: the entries were allocated by GetExplicitEntriesFromAclW for the caller to free once.
    unsafe { LocalFree(entries.cast()) };
    Ok(found)
}

/// The current user's SID, copied out of the process token.
fn current_user_sid() -> Result<Vec<u8>, String> {
    let token = super::own_token().ok_or("cannot open the process token")?;
    let bytes =
        super::token_bytes(&token, TokenUser).ok_or("cannot read the user of the process token")?;
    if bytes.len() < std::mem::size_of::<TOKEN_USER>() {
        return Err("the token's user is shorter than a TOKEN_USER".into());
    }
    // SAFETY: the buffer is at least a TOKEN_USER long and holds one; its SID pointer refers into the same buffer. Read unaligned.
    let user = unsafe { bytes.as_ptr().cast::<TOKEN_USER>().read_unaligned() };
    let sid: PSID = user.User.Sid;
    if sid.is_null() {
        return Err("the token's user has no SID".into());
    }
    // SAFETY: the SID is valid (it lives in `bytes`).
    let len = unsafe { GetLengthSid(sid) } as usize;
    // SAFETY: the SID is `len` readable bytes, inside `bytes`.
    Ok(unsafe { std::slice::from_raw_parts(sid.cast::<u8>(), len) }.to_vec())
}

/// Whether the directory's DACL is already the one `protect_to_current_user`
/// writes: protected, one explicit allow entry, for this SID, inheritable.
/// Reading is cheap; a write propagates to everything beneath the directory,
/// so it is not repeated when nothing changed.
fn is_private_to(path: &Path, sid: &[u8]) -> Result<bool, String> {
    let dacl = read_dacl(path)?;
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

/// Gives the directory a protected DACL with one entry: the current user,
/// full control, inherited by everything created beneath (REQ-WS-001,
/// external design §14.1). Nothing is inherited from the parent, so no one
/// else (the system, the administrators, other users) is granted anything
/// through the directory; the owner keeps the right to change the DACL
/// whatever it says, and the current user's entry is in the DACL that
/// replaces the old one in the same call, so mimlys is never locked out.
/// Called at every open, before the runtime adds the container's entries on
/// the subdirectories, so that the entries it adds are the only others; when
/// the DACL already is this one nothing is written.
pub fn protect_to_current_user(path: &Path) -> Result<(), String> {
    let sid = current_user_sid()?;
    let _lock = DaclLock::acquire(path)?;
    if is_private_to(path, &sid)? {
        return Ok(());
    }
    // A specific mask, not GENERIC_ALL: the system turns an inheritable
    // entry with a generic right into two (this directory, and inherit-only),
    // and `is_private_to` expects the one entry written here.
    let entry = EXPLICIT_ACCESS_W {
        grfAccessPermissions: FILE_ALL_ACCESS,
        grfAccessMode: GRANT_ACCESS,
        grfInheritance: SUB_CONTAINERS_AND_OBJECTS_INHERIT,
        Trustee: TRUSTEE_W {
            pMultipleTrustee: std::ptr::null_mut(),
            MultipleTrusteeOperation: 0,
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_UNKNOWN,
            ptstrName: sid.as_ptr() as *mut u16,
        },
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
    let mut path_w = wide(path);
    // SAFETY: the path is null-terminated UTF-16; only the DACL is replaced, protected, with the ACL just built.
    let err = unsafe {
        SetNamedSecurityInfoW(
            path_w.as_mut_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            new_acl,
            std::ptr::null(),
        )
    };
    // SAFETY: the new ACL was allocated by SetEntriesInAclW for the caller to free once.
    unsafe { LocalFree(new_acl.cast()) };
    if err != ERROR_SUCCESS {
        return Err(format!(
            "SetNamedSecurityInfoW({}) failed: {}",
            path.display(),
            std::io::Error::from_raw_os_error(err as i32)
        ));
    }
    Ok(())
}
