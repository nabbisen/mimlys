//! The directory permissions an AppContainer needs (RFC 011 §3, §7;
//! REQ-CONF-008): one access-control entry for the container SID on each
//! path of the read set (read and execute) and of the write set (full
//! access), added by `mimlys toolchain grant` and removed by `revoke`,
//! inheritable to what lies beneath. Nothing else in the directory's
//! security descriptor is touched, and `has_entry` reads the descriptor
//! back rather than trusting any record.
//!
//! Every `unsafe` block is justified (REQ-CONF-010).

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use windows_sys::Win32::Foundation::{
    ERROR_SUCCESS, GENERIC_ALL, GENERIC_EXECUTE, GENERIC_READ, LocalFree,
};
use windows_sys::Win32::Security::Authorization::{
    EXPLICIT_ACCESS_W, GRANT_ACCESS, GetExplicitEntriesFromAclW, GetNamedSecurityInfoW,
    REVOKE_ACCESS, SE_FILE_OBJECT, SetEntriesInAclW, SetNamedSecurityInfoW, TRUSTEE_IS_SID,
    TRUSTEE_IS_UNKNOWN, TRUSTEE_W,
};
use windows_sys::Win32::Security::{
    ACL, DACL_SECURITY_INFORMATION, EqualSid, PSECURITY_DESCRIPTOR, PSID,
    SUB_CONTAINERS_AND_OBJECTS_INHERIT,
};

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

fn set_entry(path: &Path, sid: &[u8], access: Access, mode: i32) -> Result<(), String> {
    let old = read_dacl(path)?;
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
    // SAFETY: one entry is given; the old ACL lives in `old`; `new_acl` receives an allocation freed below.
    let err = unsafe { SetEntriesInAclW(1, &entry, old.acl, &mut new_acl) };
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
            DACL_SECURITY_INFORMATION,
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
