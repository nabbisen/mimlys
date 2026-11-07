//! The foreign calls of the macOS backend (REQ-CONF-010): `sandbox_init`,
//! resolved at run time with `dlsym` so that a macOS that removes the
//! deprecated symbol is a reported state (`seatbelt_missing`) and not a link
//! failure, and the `kern.procargs2` read the self-check uses as its control.
//! Each `unsafe` block carries its `SAFETY:` line.

#![allow(unsafe_code)]

use std::ffi::{CStr, CString, c_char, c_int, c_void};

/// `int sandbox_init(const char *profile, uint64_t flags, char **errorbuf)`.
/// With flags 0 the first argument is profile source text (measured by S1).
pub type SandboxInit = unsafe extern "C" fn(*const c_char, u64, *mut *mut c_char) -> c_int;
/// `void sandbox_free_error(char *errorbuf)`.
type SandboxFreeError = unsafe extern "C" fn(*mut c_char);

/// `dlsym` of a symbol in the process's own image and its libraries.
fn lookup(name: &CStr) -> *mut c_void {
    // SAFETY: `dlsym` with `RTLD_DEFAULT` reads a symbol table and a NUL-terminated name we own.
    unsafe { libc::dlsym(libc::RTLD_DEFAULT, name.as_ptr()) }
}

/// `sandbox_init`, when this macOS still has it.
pub fn resolve_sandbox_init() -> Option<SandboxInit> {
    let symbol = lookup(c"sandbox_init");
    if symbol.is_null() {
        return None;
    }
    // SAFETY: the symbol is `sandbox_init`, whose C signature `SandboxInit` states; a data
    // pointer and a function pointer have the same size on every macOS target.
    Some(unsafe { std::mem::transmute::<*mut c_void, SandboxInit>(symbol) })
}

/// Applies the profile text to the calling process. The error is the
/// system's message (with a position, for a profile it rejects).
pub fn apply(init: SandboxInit, profile: &str) -> Result<(), String> {
    let text = CString::new(profile).map_err(|_| "the profile holds a NUL".to_owned())?;
    let mut error: *mut c_char = std::ptr::null_mut();
    // SAFETY: `text` is a NUL-terminated string that outlives the call; `error` is a valid
    // out-pointer; flags 0 means "profile source text".
    let rc = unsafe { init(text.as_ptr(), 0, &mut error) };
    let message = if error.is_null() {
        String::new()
    } else {
        // SAFETY: on failure `sandbox_init` stores a NUL-terminated message in `error`.
        let message = unsafe { CStr::from_ptr(error) }
            .to_string_lossy()
            .into_owned();
        let free = lookup(c"sandbox_free_error");
        if !free.is_null() {
            // SAFETY: the symbol is `sandbox_free_error`, which frees exactly such a buffer.
            unsafe {
                std::mem::transmute::<*mut c_void, SandboxFreeError>(free)(error);
            }
        }
        message
    };
    if rc == 0 {
        Ok(())
    } else if message.is_empty() {
        Err(format!("sandbox_init returned {rc}"))
    } else {
        Err(message)
    }
}

/// The bytes `sysctl(KERN_PROCARGS2)` gives for a process (its arguments
/// and environment), or the `errno`.
pub fn procargs2(pid: i32) -> Result<Vec<u8>, i32> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
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
    if rc != 0 {
        return Err(std::io::Error::last_os_error().raw_os_error().unwrap_or(0));
    }
    // The environment can grow between the two calls: leave room.
    let mut buffer = vec![0u8; size + 4096];
    let mut size = buffer.len();
    // SAFETY: `buffer` is `size` bytes long and `size` says so; `mib` as above.
    let rc = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            buffer.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc != 0 {
        return Err(std::io::Error::last_os_error().raw_os_error().unwrap_or(0));
    }
    buffer.truncate(size);
    Ok(buffer)
}

/// Every pid on the machine (`proc_listallpids`), or the `errno`. The
/// profile allows the listing without a target; no listing restricted to the
/// sandbox exists (measured), so the members are the ones [`see`] can read.
pub fn list_pids() -> Result<Vec<i32>, i32> {
    // SAFETY: a null buffer with size 0 asks for the number of pids.
    let count = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
    if count <= 0 {
        return Err(std::io::Error::last_os_error().raw_os_error().unwrap_or(0));
    }
    // Processes start between the two calls: leave room.
    let mut pids = vec![0i32; count as usize + 512];
    let bytes = (pids.len() * std::mem::size_of::<i32>()) as libc::c_int;
    // SAFETY: `pids` is `bytes` bytes long and `bytes` says so.
    let listed = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), bytes) };
    if listed <= 0 {
        return Err(std::io::Error::last_os_error().raw_os_error().unwrap_or(0));
    }
    pids.truncate(listed as usize);
    Ok(pids)
}

/// What the profile lets this process see of `pid` (`proc_pidinfo`): a
/// member answers, anything else is refused (Amendment 2 point 4).
pub fn see(pid: i32) -> super::Seen {
    /// `SZOMB` of `<sys/proc.h>`: the process has ended and waits to be reaped.
    const SZOMB: u32 = 5;
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
    if written != size {
        return super::Seen::Hidden;
    }
    if info.pbi_status == SZOMB {
        return super::Seen::Zombie;
    }
    // SAFETY: an all-zero `proc_taskinfo` is a valid value of a plain C struct of integers.
    let mut task: libc::proc_taskinfo = unsafe { std::mem::zeroed() };
    let task_size = std::mem::size_of::<libc::proc_taskinfo>() as libc::c_int;
    // SAFETY: `task` is `task_size` bytes long and `task_size` says so.
    let written = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTASKINFO,
            0,
            std::ptr::addr_of_mut!(task).cast(),
            task_size,
        )
    };
    let resident = if written == task_size {
        task.pti_resident_size
    } else {
        0
    };
    // The resident size alone under-reports when the system compresses the
    // process's pages (measured on a loaded runner: 3 GiB touched, under
    // 2 GiB resident). The physical footprint, which counts compressed pages
    // (what Activity Monitor and jetsam use), does not; the larger of the two
    // is the process's memory.
    super::Seen::Alive {
        resident: resident.max(physical_footprint(pid)),
    }
}

/// `ri_phys_footprint` of `proc_pid_rusage`, or 0 when the system does not say.
fn physical_footprint(pid: i32) -> u64 {
    // SAFETY: an all-zero `rusage_info_v0` is a valid value of a plain C struct of integers.
    let mut usage: libc::rusage_info_v0 = unsafe { std::mem::zeroed() };
    // SAFETY: for `RUSAGE_INFO_V0` the system fills a `rusage_info_v0`, which `usage` is (the
    // C parameter is a pointer to the structure, typed `rusage_info_t *`).
    let rc = unsafe {
        libc::proc_pid_rusage(
            pid,
            libc::RUSAGE_INFO_V0,
            std::ptr::addr_of_mut!(usage).cast(),
        )
    };
    if rc == 0 { usage.ri_phys_footprint } else { 0 }
}

/// Whether signal 0 (existence and permission, nothing delivered) to `pid` is allowed.
pub fn can_signal(pid: i32) -> bool {
    // SAFETY: signal 0 only checks permission; it delivers nothing.
    unsafe { libc::kill(pid, 0) == 0 }
}

/// `SIGKILL` to `pid`. The profile lets it reach members only; the kernel
/// refuses anything else (a reused pid of an outsider included).
pub fn kill_pid(pid: i32) {
    // SAFETY: `kill` takes two integers; the result (a refusal, or a process
    // that ended already) is ignored on purpose: the next listing shows what is left.
    unsafe {
        libc::kill(pid, libc::SIGKILL);
    }
}

/// Whether `sysctl kern.proc.all` (`KERN_PROC_ALL`, the process list with
/// names) answers: the profile denies the `kern.proc.` prefix (measured).
pub fn kern_proc_all_readable() -> bool {
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
    rc == 0
}

/// The per-user temporary directory, `confstr(_CS_DARWIN_USER_TEMP_DIR)`
/// (`/var/folders/xx/yyyy/T/`): not `$TMPDIR`, and what `xcrun` caches its
/// database under. `None` when the system does not say.
pub fn user_temp_dir() -> Option<std::path::PathBuf> {
    let mut buffer = vec![0u8; libc::PATH_MAX as usize];
    // SAFETY: `buffer` is `buffer.len()` bytes long and `confstr` writes at most that many,
    // NUL included; it returns the length it needs, NUL included, or 0 on failure.
    let needed = unsafe {
        libc::confstr(
            libc::_CS_DARWIN_USER_TEMP_DIR,
            buffer.as_mut_ptr().cast(),
            buffer.len(),
        )
    };
    if needed == 0 || needed > buffer.len() {
        return None;
    }
    buffer.truncate(needed - 1);
    String::from_utf8(buffer).ok().map(std::path::PathBuf::from)
}

/// How many threads this process has (`pti_threadnum`), or `None` when the
/// system does not say: the sweeper is forked only from a process of one.
pub fn own_thread_count() -> Option<i32> {
    // SAFETY: an all-zero `proc_taskinfo` is a valid value of a plain C struct of integers.
    let mut task: libc::proc_taskinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_taskinfo>() as libc::c_int;
    // SAFETY: `task` is `size` bytes long and `size` says so.
    let written = unsafe {
        libc::proc_pidinfo(
            std::process::id() as i32,
            libc::PROC_PIDTASKINFO,
            0,
            std::ptr::addr_of_mut!(task).cast(),
            size,
        )
    };
    (written == size).then_some(task.pti_threadnum)
}
