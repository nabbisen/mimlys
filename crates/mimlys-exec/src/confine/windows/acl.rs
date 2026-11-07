//! The directory permissions an AppContainer needs (RFC 011 §3, §7;
//! REQ-CONF-008): one access-control entry for the container SID on each
//! path of the read set (read and execute) and of the write set (full
//! access), added by `mimlys toolchain grant` and removed by `revoke`,
//! inheritable to what lies beneath. Nothing else in the directory's
//! security descriptor is touched: the entries the directory inherits stay
//! inherited and its protection flag stays as it was; and `has_entry` reads
//! the descriptor back rather than trusting any record. A write is a merge
//! into the DACL read a moment earlier, so one writer at a time per
//! directory holds a named mutex across the runtimes of this machine (two
//! installations granting their containers one directory at once would
//! otherwise lose one another's entry).
//! The merge starts from the directory's explicit entries alone, and the
//! system recomputes the inherited ones from the parent on the write.
//! The write marks the descriptor auto-inherited, as every write through
//! this interface does; `icacls` then shows the entries the directory
//! inherits as `(I)`, which they were before the write too (their flag),
//! and the entries themselves are what they were.
//!
//! Every `unsafe` block is justified (REQ-CONF-010).

use mimlys_core::limits::LIM_O2_CANCEL_MS;
use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_SUCCESS, GENERIC_ALL, GENERIC_EXECUTE, GENERIC_READ, HANDLE, LocalFree,
    WAIT_ABANDONED, WAIT_OBJECT_0,
};
use windows_sys::Win32::Security::Authorization::{
    EXPLICIT_ACCESS_W, GRANT_ACCESS, GetExplicitEntriesFromAclW, GetNamedSecurityInfoW,
    REVOKE_ACCESS, SE_FILE_OBJECT, SetEntriesInAclW, SetNamedSecurityInfoW, TRUSTEE_IS_SID,
    TRUSTEE_IS_UNKNOWN, TRUSTEE_W,
};
use windows_sys::Win32::Security::{
    ACE_HEADER, ACL, ACL_REVISION, ACL_SIZE_INFORMATION, AclSizeInformation, AddAce,
    DACL_SECURITY_INFORMATION, EqualSid, GetAce, GetAclInformation, GetSecurityDescriptorControl,
    INHERITED_ACE, InitializeAcl, PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
    SE_DACL_PROTECTED, SUB_CONTAINERS_AND_OBJECTS_INHERIT, UNPROTECTED_DACL_SECURITY_INFORMATION,
};
use windows_sys::Win32::System::Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject};

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
    /// Takes the lock within LIM-O2; `None` when the system refuses the
    /// mutex or the wait runs out (the write goes ahead, noted in the log:
    /// a DACL write takes milliseconds and a holder that died abandons it).
    fn acquire(path: &Path) -> (Option<Self>, &'static str) {
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
                "no mutex for the permissions of {}: {}",
                path.display(),
                std::io::Error::last_os_error()
            );
            return (None, "NOMUTEX");
        }
        // SAFETY: the handle was just created and is valid.
        let wait_ms = std::env::var("D1_LOCK_MS")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(LIM_O2_CANCEL_MS as u32);
        match unsafe { WaitForSingleObject(handle, wait_ms) } {
            WAIT_OBJECT_0 => (Some(Self { handle }), "OBJECT_0"),
            WAIT_ABANDONED => (Some(Self { handle }), "ABANDONED"),
            _ => {
                tracing::warn!(
                    target: "mimlys_worker",
                    "the permissions of {} are being written by another runtime past LIM-O2; writing anyway",
                    path.display()
                );
                // SAFETY: the handle is valid and closed exactly once.
                unsafe { CloseHandle(handle) };
                (None, "TIMEOUT")
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

// ---- D1 DIAGNOSTIC (throwaway branch only; never to be merged) ----
mod d1 {
    use super::*;
    use std::io::Write;

    pub fn log(msg: &str) {
        let Ok(path) = std::env::var("D1_DIAG") else {
            return;
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let total = now.as_millis() as u64;
        let (s, ms) = (total / 1000, total % 1000);
        let line = format!(
            "{:02}:{:02}:{:02}.{:03} pid={} tid={:?} {}\n",
            (s / 3600) % 24,
            (s / 60) % 60,
            s % 60,
            ms,
            std::process::id(),
            std::thread::current().id(),
            msg
        );
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(path)
        {
            let _ = f.write_all(line.as_bytes());
        }
    }

    /// aces, explicit, inherited, AppContainer-SID aces, whether the SID has an explicit ACE.
    pub fn info(acl: *const ACL, sid: &[u8]) -> String {
        if acl.is_null() {
            return "acl=null".into();
        }
        let mut info = ACL_SIZE_INFORMATION {
            AceCount: 0,
            AclBytesInUse: 0,
            AclBytesFree: 0,
        };
        // SAFETY: as in explicit_entries.
        let ok = unsafe {
            GetAclInformation(
                acl,
                (&mut info as *mut ACL_SIZE_INFORMATION).cast(),
                std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
                AclSizeInformation,
            )
        };
        if ok == 0 {
            return "acl=unreadable".into();
        }
        let (mut explicit, mut inherited, mut containers, mut me_explicit, mut me_any) =
            (0u32, 0u32, 0u32, false, false);
        for index in 0..info.AceCount {
            let mut ace: *mut std::ffi::c_void = std::ptr::null_mut();
            // SAFETY: index below the count.
            if unsafe { GetAce(acl, index, &mut ace) } == 0 {
                continue;
            }
            // SAFETY: an entry starts with an ACE_HEADER.
            let header = unsafe { ace.cast::<ACE_HEADER>().read_unaligned() };
            let inh = u32::from(header.AceFlags) & INHERITED_ACE != 0;
            if inh {
                inherited += 1;
            } else {
                explicit += 1;
            }
            // ACCESS_ALLOWED_ACE_TYPE (0) and ACCESS_DENIED_ACE_TYPE (1): header, mask, then the SID.
            if header.AceType <= 1 {
                // SAFETY: the SID follows the 8-byte header and mask inside the entry.
                let sid_ptr = unsafe { (ace as *const u8).add(8) };
                // SAFETY: as above; at least 12 bytes of SID are present for any valid SID.
                let head = unsafe { std::slice::from_raw_parts(sid_ptr, 12) };
                if head[2..8] == [0, 0, 0, 0, 0, 15] && head[8..12] == [2, 0, 0, 0] {
                    containers += 1;
                }
                // SAFETY: both are valid SIDs.
                if unsafe { EqualSid(sid_ptr as PSID, sid.as_ptr() as PSID) } != 0 {
                    me_any = true;
                    if !inh {
                        me_explicit = true;
                    }
                }
            }
        }
        format!(
            "aces={} explicit={explicit} inherited={inherited} container_aces={containers} me_explicit={me_explicit} me_any={me_any}",
            info.AceCount
        )
    }

    pub struct Hold {
        pub start: std::time::Instant,
        pub tag: String,
    }
    impl Drop for Hold {
        fn drop(&mut self) {
            log(&format!(
                "{} RELEASE hold_ms={}",
                self.tag,
                self.start.elapsed().as_millis()
            ));
        }
    }
}

fn set_entry(path: &Path, sid: &[u8], access: Access, mode: i32) -> Result<(), String> {
    let t0 = std::time::Instant::now();
    let (_lock, lock_state) = DaclLock::acquire(path);
    let tag = format!(
        "{} {} sid={}",
        if mode == GRANT_ACCESS { "grant" } else { "revoke" },
        path.display(),
        super::sid_string(sid)
    );
    d1::log(&format!(
        "{tag} ACQUIRE lock={lock_state} wait_ms={}",
        t0.elapsed().as_millis()
    ));
    let _hold = d1::Hold {
        start: std::time::Instant::now(),
        tag: tag.clone(),
    };
    let old = read_dacl(path)?;
    d1::log(&format!("{tag} READ {}", d1::info(old.acl, sid)));
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
    let t_set = std::time::Instant::now();
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
    d1::log(&format!(
        "{tag} SET err={err} set_ms={} new_acl: {}",
        t_set.elapsed().as_millis(),
        d1::info(new_acl, sid)
    ));
    // SAFETY: the new ACL was allocated by SetEntriesInAclW for the caller to free once.
    unsafe { LocalFree(new_acl.cast()) };
    if let Ok(after) = read_dacl(path) {
        d1::log(&format!("{tag} READBACK {}", d1::info(after.acl, sid)));
    }
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
