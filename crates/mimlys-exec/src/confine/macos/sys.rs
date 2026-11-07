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
