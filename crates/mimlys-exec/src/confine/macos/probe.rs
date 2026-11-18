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

/// Starts `program` in a session of its own (`setsid` between fork and
/// exec; macOS has no `setsid` command), so that it leaves the caller's
/// process group and session. A child that then exits or is killed leaves
/// its own children re-parented to `launchd`.
pub fn spawn_in_new_session(program: &str, args: &[&str]) -> std::io::Result<std::process::Child> {
    use std::os::unix::process::CommandExt;
    let mut command = std::process::Command::new(program);
    command.args(args);
    // SAFETY: the closure runs between fork and exec and calls only `setsid`,
    // which is async-signal-safe; it allocates nothing and takes no lock.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
    command.spawn()
}

/// The size in bytes of the process list with names (`sysctl
/// KERN_PROC_ALL`), or the `errno` that refused it: the profile denies the
/// `kern.proc.` prefix, so the answer inside is a refusal.
pub fn process_list_size() -> Result<usize, i32> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROC, libc::KERN_PROC_ALL];
    let mut size: libc::size_t = 0;
    // SAFETY: `mib` has three elements as `namelen` says; a null buffer asks for the size.
    let rc = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            std::ptr::null_mut(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc == 0 {
        Ok(size)
    } else {
        Err(std::io::Error::last_os_error().raw_os_error().unwrap_or(0))
    }
}

/// The pids this process can inspect other than itself (the live members of
/// its sandbox, as the profile lets it see them): what a tool can learn of
/// the machine's processes. The two-workers test compares two sandboxes'
/// answers.
pub fn visible_pids() -> Vec<i32> {
    let own = std::process::id() as i32;
    super::sys::list_pids()
        .unwrap_or_default()
        .into_iter()
        .filter(|&pid| pid > 1 && pid != own)
        .filter(|&pid| matches!(super::sys::see(pid), super::Seen::Alive { .. }))
        .collect()
}

/// Applies a further profile to the calling process, as a host tool would to
/// narrow itself below the profile its worker applied: the system's answer
/// (`Ok` if it was applied, else its message). Measured on the macOS job
/// (REQ-CONF-006): a sandboxed process can apply no second profile.
pub fn apply_further_profile(profile: &str) -> Result<(), String> {
    let init =
        super::sys::resolve_sandbox_init().ok_or_else(|| "sandbox_init is missing".to_owned())?;
    super::sys::apply(init, profile)
}

/// The parent of `pid` (`proc_pidinfo`), when the profile lets this process read it.
pub fn parent_of(pid: i32) -> Option<i32> {
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
    (written == size).then_some(info.pbi_ppid as i32)
}

/// What a hostile tool does to the worker's cleanup (audit A-21): kills every
/// other child of its parent (the worker's sweeper is one) and then its parent,
/// the worker, with `SIGKILL`. A tool may signal the members of its own sandbox,
/// and the worker and its sweeper are members. Returns how many siblings it
/// signalled.
pub fn kill_siblings_and_parent() -> usize {
    let own = std::process::id() as i32;
    let parent = std::os::unix::process::parent_id() as i32;
    let mut signalled = 0;
    for pid in super::sys::list_pids().unwrap_or_default() {
        if pid <= 1 || pid == own || pid == parent {
            continue;
        }
        if matches!(super::sys::see(pid), super::Seen::Alive { .. })
            && parent_of(pid) == Some(parent)
        {
            super::sys::kill_pid(pid);
            signalled += 1;
        }
    }
    super::sys::kill_pid(parent);
    signalled
}
