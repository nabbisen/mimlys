//! Raw-syscall helpers of the macOS clause tests (REQ-TEST-013), behind the
//! `probe-syscalls` feature that `mimlys/test-bins` enables; never part of a
//! release. The probe tool (`mimlys-probe-tool`) calls these safe functions
//! from inside a host tool to try what the Seatbelt profile must refuse:
//! the arguments and environment of another process
//! (`sysctl KERN_PROCARGS2`), its process information
//! (`proc_pidinfo`) and a signal to it. Each says what it tried and what the
//! kernel answered, as an `errno`, so a test can tell "denied by the
//! profile" from "not possible here".
//!
//! This is the one place the `mimlys` crate's tests reach foreign-function
//! code on macOS: the `mimlys` crate forbids `unsafe` (REQ-ENG-001,
//! REQ-CONF-010).

#![allow(unsafe_code)]

/// Whether `needle` appears in the arguments and environment of `pid`
/// (`KERN_PROCARGS2`), or the `errno` that refused the read.
pub fn procargs_contains(pid: i32, needle: &str) -> Result<bool, i32> {
    let bytes = super::sys::procargs2(pid)?;
    Ok(!needle.is_empty()
        && bytes
            .windows(needle.len())
            .any(|window| window == needle.as_bytes()))
}

/// Whether `proc_pidinfo(PROC_PIDTBSDINFO)` answers for `pid`, or the `errno`.
pub fn process_info(pid: i32) -> Result<(), i32> {
    // SAFETY: an all-zero `proc_bsdinfo` is a valid value of a plain C struct of integers.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    // SAFETY: `info` is `size` bytes long and `size` says so.
    let written = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            std::ptr::addr_of_mut!(info).cast(),
            size,
        )
    };
    if written == size {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error().raw_os_error().unwrap_or(0))
    }
}

/// Whether signal 0 (existence and permission, nothing delivered) to `pid`
/// is allowed, or the `errno`.
pub fn signal_zero(pid: i32) -> Result<(), i32> {
    // SAFETY: signal 0 only checks permission; it delivers nothing.
    if unsafe { libc::kill(pid, 0) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error().raw_os_error().unwrap_or(0))
    }
}
