//! The daemon's transport on Windows: a named pipe (RFC 014, Amendment 1, item 4; external design
//! §19.5, "Windows"; REQ-SEC-016, REQ-CONF-010; THR-39).
//!
//! **This module and the confinement modules are the only places of the workspace that call
//! foreign functions** (`scripts/check-unsafe.sh`); each `unsafe` block says why it is sound. The
//! rest of the daemon (`mimlys::daemon`) forbids `unsafe` and uses this module's safe API:
//!
//! - [`PipeListener`]: the daemon's end. The pipe is created as its **first instance**
//!   (`FILE_FLAG_FIRST_PIPE_INSTANCE`: a process that already holds the name makes the creation
//!   fail, never the daemon's pipe a second one), with a **DACL that admits the user's SID alone**
//!   passed through `SECURITY_ATTRIBUTES`, never the default (which lets Everyone read). A token of
//!   an AppContainer is not admitted by it, so a confined tool cannot open the pipe.
//! - [`PipeStream`]: one connection, either end. Reads and writes are overlapped, so that a
//!   timeout (a client's, the daemon's) and a shutdown from another thread end a wait, as a socket's
//!   do; a peer that has gone is the end of the stream on a read and a broken pipe on a write.
//! - **Who is at the other end:** [`PipeStream::peer_process_id`] (`GetNamedPipeClientProcessId` on
//!   the daemon's end, `GetNamedPipeServerProcessId` on the client's) and [`process_user_sid`], the
//!   user of that process's token. A process that cannot be opened is an error, which the
//!   callers refuse.
//! - **The daemon's own tree** is a process in a Job Object the daemon made: [`process_in_any_job`]
//!   (`IsProcessInJob`). The worker's job is its own; a tool server's is a second, nested job
//!   ([`JobMark`]) because the job `process-wrap` makes for it is not reachable.

#![allow(unsafe_code)]

use std::ffi::OsStr;
use std::io::{self, Read, Write};
use std::os::windows::ffi::OsStrExt;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_BROKEN_PIPE, ERROR_FILE_NOT_FOUND, ERROR_IO_PENDING,
    ERROR_NO_DATA, ERROR_OPERATION_ABORTED, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED,
    ERROR_PIPE_NOT_CONNECTED, ERROR_SUCCESS, GENERIC_READ, GENERIC_WRITE, HANDLE,
    INVALID_HANDLE_VALUE, LocalFree, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo,
    SDDL_REVISION_1, SE_KERNEL_OBJECT,
};
use windows_sys::Win32::Security::Cryptography::{
    BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom,
};
use windows_sys::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACL, ACL_SIZE_INFORMATION, AclSizeInformation, DACL_SECURITY_INFORMATION,
    GetAce, GetAclInformation, GetTokenInformation, PSECURITY_DESCRIPTOR, PSID,
    SECURITY_ATTRIBUTES, TOKEN_QUERY, TokenUser,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, OPEN_EXISTING,
    PIPE_ACCESS_DUPLEX, ReadFile, SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT, WriteFile,
};
use windows_sys::Win32::System::Console::{
    GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, SetStdHandle,
};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, IsProcessInJob,
    JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JobObjectBasicAccountingInformation,
    QueryInformationJobObject,
};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, GetNamedPipeClientProcessId, GetNamedPipeServerProcessId,
    PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES,
    PIPE_WAIT, WaitNamedPipeW,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, GetCurrentProcess, INFINITE, OpenProcess, OpenProcessToken,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_QUOTA, PROCESS_TERMINATE, SetEvent,
    WaitForMultipleObjects, WaitForSingleObject,
};

/// The size of each direction's buffer of a pipe instance.
const PIPE_BUFFER: u32 = 64 * 1024;

/// A kernel handle owned by this value, closed when it drops.
struct Owned(HANDLE);

// SAFETY: a kernel handle is a reference to an object that any thread may use; the calls made
// with it here are the system's, which synchronise their own use.
unsafe impl Send for Owned {}
// SAFETY: as above: the handle is only passed to system calls that are safe to make concurrently.
unsafe impl Sync for Owned {}

impl Owned {
    fn raw(&self) -> HANDLE {
        self.0
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: the handle was returned by a call that makes one, is owned by this value and is
        // closed exactly once, here.
        unsafe { CloseHandle(self.0) };
    }
}

fn last() -> io::Error {
    io::Error::last_os_error()
}

fn code(error: &io::Error) -> Option<u32> {
    error.raw_os_error().and_then(|c| u32::try_from(c).ok())
}

fn wide(text: &str) -> Vec<u16> {
    OsStr::new(text)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// A new event, which an overlapped operation signals when it ends.
fn event(manual: bool) -> io::Result<Owned> {
    // SAFETY: no security attributes and no name are given; the call returns a null handle on
    // failure, which is checked.
    let handle = unsafe { CreateEventW(std::ptr::null(), i32::from(manual), 0, std::ptr::null()) };
    if handle.is_null() {
        return Err(last());
    }
    Ok(Owned(handle))
}

fn overlapped_with(event: &Owned) -> OVERLAPPED {
    // SAFETY: an all-zero OVERLAPPED is a valid value of this plain C struct.
    let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
    overlapped.hEvent = event.raw();
    overlapped
}

/// The milliseconds of a timeout for a wait: at least one, and `INFINITE` for none.
fn millis(timeout: Option<Duration>) -> u32 {
    timeout.map_or(INFINITE, |d| {
        u32::try_from(d.as_millis().max(1))
            .unwrap_or(u32::MAX - 1)
            .min(u32::MAX - 1)
    })
}

/// How long a wait for an overlapped operation goes on before it looks at the connection's
/// shutdown again: a shutdown from another thread cancels what waits at that moment, and one that
/// begins just after would otherwise wait for ever.
const SLICE: Duration = Duration::from_millis(100);

/// Ends an overlapped operation: the number of bytes it moved, or its error. `started` is what the
/// call returned. When the operation is pending it is waited for, up to `timeout` and until `shut`
/// is set; one that is not done by then is cancelled and waited for (the buffer is the caller's and
/// must not be used by the system after the call returns) and is [`io::ErrorKind::TimedOut`]
/// unless it ended first, or `ERROR_OPERATION_ABORTED` when it was the shutdown that cancelled it.
fn finish(
    handle: HANDLE,
    started: i32,
    overlapped: &mut OVERLAPPED,
    timeout: Option<Duration>,
    shut: &AtomicBool,
) -> io::Result<u32> {
    let mut moved = 0u32;
    if started == 0 {
        let error = last();
        if code(&error) != Some(ERROR_IO_PENDING) {
            return Err(error);
        }
        let deadline = timeout.map(|t| Instant::now() + t);
        loop {
            let slice = deadline.map_or(SLICE, |d| {
                d.saturating_duration_since(Instant::now()).min(SLICE)
            });
            // SAFETY: the event is the overlapped's own and stays open for the whole call.
            let waited = unsafe { WaitForSingleObject(overlapped.hEvent, millis(Some(slice))) };
            if waited == WAIT_OBJECT_0 {
                break;
            }
            let expired = deadline.is_some_and(|d| Instant::now() >= d);
            if waited == WAIT_TIMEOUT && !expired && !shut.load(Ordering::SeqCst) {
                continue;
            }
            // SAFETY: the overlapped is the one of the operation this handle started.
            unsafe { CancelIoEx(handle, overlapped) };
            // SAFETY: as above; the call waits for the cancelled operation to be over.
            let ended = unsafe { GetOverlappedResult(handle, overlapped, &mut moved, 1) };
            if ended != 0 {
                return Ok(moved);
            }
            let error = last();
            return Err(
                if code(&error) == Some(ERROR_OPERATION_ABORTED) && !shut.load(Ordering::SeqCst) {
                    io::Error::from(io::ErrorKind::TimedOut)
                } else {
                    error
                },
            );
        }
    }
    // SAFETY: the operation is over (it returned, or its event was signalled): the call only reads
    // its result.
    if unsafe { GetOverlappedResult(handle, overlapped, &mut moved, 0) } == 0 {
        return Err(last());
    }
    Ok(moved)
}

/// The role of an end of a pipe: it decides which call names the process at the other end.
#[derive(Clone, Copy, PartialEq, Eq)]
enum End {
    /// The daemon's.
    Server,
    /// The client's.
    Client,
}

struct Inner {
    handle: Owned,
    end: End,
    read_ms: AtomicU64,
    write_ms: AtomicU64,
    shut: AtomicBool,
}

/// One end of a connection of the daemon's pipe. Clones share the connection, as the clones of a
/// socket do: a shutdown by one ends the waits of the others.
#[derive(Clone)]
pub struct PipeStream {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for PipeStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PipeStream")
    }
}

fn timeout_of(ms: u64) -> Option<Duration> {
    (ms != 0).then(|| Duration::from_millis(ms))
}

impl PipeStream {
    fn new(handle: Owned, end: End) -> Self {
        Self {
            inner: Arc::new(Inner {
                handle,
                end,
                read_ms: AtomicU64::new(0),
                write_ms: AtomicU64::new(0),
                shut: AtomicBool::new(false),
            }),
        }
    }

    /// Opens the daemon's pipe named `name`, waiting while every instance is busy, up to `within`.
    /// The server may only identify the client, never act as it (`SECURITY_IDENTIFICATION`).
    /// [`io::ErrorKind::NotFound`] when there is no such pipe, [`io::ErrorKind::PermissionDenied`]
    /// when its DACL refuses this user, [`io::ErrorKind::TimedOut`] when it stayed busy.
    pub fn connect(name: &str, within: Duration) -> io::Result<Self> {
        let wide_name = wide(name);
        let begun = Instant::now();
        loop {
            // SAFETY: the name is a null-terminated wide string that outlives the call; no
            // security attributes and no template are given; failure is the invalid handle.
            let handle = unsafe {
                CreateFileW(
                    wide_name.as_ptr(),
                    GENERIC_READ | GENERIC_WRITE,
                    0,
                    std::ptr::null(),
                    OPEN_EXISTING,
                    FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
                    std::ptr::null_mut(),
                )
            };
            if handle != INVALID_HANDLE_VALUE {
                return Ok(Self::new(Owned(handle), End::Client));
            }
            let error = last();
            match code(&error) {
                Some(ERROR_FILE_NOT_FOUND) => return Err(io::Error::from(io::ErrorKind::NotFound)),
                Some(ERROR_PIPE_BUSY) => {
                    let left = within.saturating_sub(begun.elapsed());
                    if left.is_zero() {
                        return Err(io::Error::from(io::ErrorKind::TimedOut));
                    }
                    // SAFETY: the name is a null-terminated wide string that outlives the call.
                    // A wait that ends without an instance free is tried again until `within`.
                    unsafe { WaitNamedPipeW(wide_name.as_ptr(), millis(Some(left))) };
                }
                Some(ERROR_ACCESS_DENIED) => {
                    return Err(io::Error::from(io::ErrorKind::PermissionDenied));
                }
                _ => return Err(error),
            }
        }
    }

    /// Another handle on the same connection.
    #[allow(clippy::unnecessary_wraps)]
    pub fn try_clone(&self) -> io::Result<Self> {
        Ok(self.clone())
    }

    /// The longest a read waits (none when `None`). A zero duration is refused, as a socket's is.
    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        store_timeout(&self.inner.read_ms, timeout)
    }

    /// The longest a write waits (none when `None`).
    pub fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        store_timeout(&self.inner.write_ms, timeout)
    }

    /// Ends the connection for every clone: a read that waits ends as the end of the stream, and
    /// no later read or write moves anything. The pipe itself closes when the last clone drops.
    #[allow(clippy::unnecessary_wraps)]
    pub fn shutdown(&self, _how: std::net::Shutdown) -> io::Result<()> {
        self.inner.shut.store(true, Ordering::SeqCst);
        // SAFETY: the handle is open as long as `self` is; no overlapped is named, so every
        // operation of the handle is cancelled.
        unsafe { CancelIoEx(self.inner.handle.raw(), std::ptr::null()) };
        Ok(())
    }

    /// The process at the other end, as the system names it: the client's on the daemon's end,
    /// the server's on the client's.
    pub fn peer_process_id(&self) -> io::Result<u32> {
        let mut pid = 0u32;
        let handle = self.inner.handle.raw();
        let ok = match self.inner.end {
            // SAFETY: the handle is an open pipe handle; `pid` is a valid out pointer.
            End::Server => unsafe { GetNamedPipeClientProcessId(handle, &mut pid) },
            // SAFETY: as above.
            End::Client => unsafe { GetNamedPipeServerProcessId(handle, &mut pid) },
        };
        if ok == 0 {
            return Err(last());
        }
        Ok(pid)
    }

    fn read_some(&self, buf: &mut [u8]) -> io::Result<usize> {
        let inner = &self.inner;
        if buf.is_empty() || inner.shut.load(Ordering::SeqCst) {
            return Ok(0);
        }
        let event = event(true)?;
        let mut overlapped = overlapped_with(&event);
        let len = u32::try_from(buf.len()).unwrap_or(u32::MAX);
        // SAFETY: the buffer is `len` writable bytes that outlive the call, and `finish` does not
        // return before the operation is over; the overlapped lives as long.
        let started = unsafe {
            ReadFile(
                inner.handle.raw(),
                buf.as_mut_ptr(),
                len,
                std::ptr::null_mut(),
                &mut overlapped,
            )
        };
        match finish(
            inner.handle.raw(),
            started,
            &mut overlapped,
            timeout_of(inner.read_ms.load(Ordering::SeqCst)),
            &inner.shut,
        ) {
            Ok(moved) => Ok(moved as usize),
            // A peer that has gone ends the stream; so does this end's own shutdown.
            Err(e)
                if matches!(
                    code(&e),
                    Some(ERROR_BROKEN_PIPE | ERROR_PIPE_NOT_CONNECTED | ERROR_NO_DATA)
                ) =>
            {
                Ok(0)
            }
            Err(e)
                if code(&e) == Some(ERROR_OPERATION_ABORTED)
                    && inner.shut.load(Ordering::SeqCst) =>
            {
                Ok(0)
            }
            Err(e) => Err(e),
        }
    }

    fn write_some(&self, buf: &[u8]) -> io::Result<usize> {
        let inner = &self.inner;
        if inner.shut.load(Ordering::SeqCst) {
            return Err(io::Error::from(io::ErrorKind::BrokenPipe));
        }
        if buf.is_empty() {
            return Ok(0);
        }
        let event = event(true)?;
        let mut overlapped = overlapped_with(&event);
        let len = u32::try_from(buf.len()).unwrap_or(u32::MAX);
        // SAFETY: the buffer is `len` readable bytes that outlive the call, and `finish` does not
        // return before the operation is over; the overlapped lives as long.
        let started = unsafe {
            WriteFile(
                inner.handle.raw(),
                buf.as_ptr(),
                len,
                std::ptr::null_mut(),
                &mut overlapped,
            )
        };
        match finish(
            inner.handle.raw(),
            started,
            &mut overlapped,
            timeout_of(inner.write_ms.load(Ordering::SeqCst)),
            &inner.shut,
        ) {
            Ok(moved) => Ok(moved as usize),
            // A peer that has gone: what the other end answers to a write to a pipe it closed.
            Err(e)
                if matches!(
                    code(&e),
                    Some(ERROR_BROKEN_PIPE | ERROR_PIPE_NOT_CONNECTED | ERROR_NO_DATA)
                ) =>
            {
                Err(io::Error::from(io::ErrorKind::BrokenPipe))
            }
            Err(e) => Err(e),
        }
    }
}

fn store_timeout(slot: &AtomicU64, timeout: Option<Duration>) -> io::Result<()> {
    match timeout {
        Some(d) if d.is_zero() => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "cannot set a zero duration timeout",
        )),
        Some(d) => {
            slot.store(
                u64::try_from(d.as_millis().max(1)).unwrap_or(u64::MAX),
                Ordering::SeqCst,
            );
            Ok(())
        }
        None => {
            slot.store(0, Ordering::SeqCst);
            Ok(())
        }
    }
}

impl Read for PipeStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.read_some(buf)
    }
}

impl Write for PipeStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.write_some(buf)
    }

    /// Nothing to do: a write is the system's once it returns. (`FlushFileBuffers` would wait for
    /// the other end to read it, and a client that never reads would hold the daemon.)
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The security attributes of the daemon's pipe: a descriptor whose DACL admits one SID.
struct Security {
    attributes: SECURITY_ATTRIBUTES,
    descriptor: PSECURITY_DESCRIPTOR,
}

// SAFETY: the descriptor is a block of memory the system allocated for this value alone, read by
// system calls and freed once, on drop; nothing else holds the pointers.
unsafe impl Send for Security {}
// SAFETY: as above: it is only read, by calls that may run concurrently.
unsafe impl Sync for Security {}

impl Security {
    /// A descriptor with a protected DACL of one entry: `GENERIC_ALL` for `sid` (a SID in its
    /// string form).
    fn for_user(sid: &str) -> io::Result<Self> {
        let sddl = wide(&format!("D:P(A;;GA;;;{sid})"));
        let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        // SAFETY: the string is null-terminated and outlives the call; the out pointer is valid;
        // the size is not asked for.
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(last());
        }
        Ok(Self {
            attributes: SECURITY_ATTRIBUTES {
                nLength: u32::try_from(std::mem::size_of::<SECURITY_ATTRIBUTES>()).unwrap_or(0),
                lpSecurityDescriptor: descriptor,
                bInheritHandle: 0,
            },
            descriptor,
        })
    }
}

impl Drop for Security {
    fn drop(&mut self) {
        // SAFETY: the descriptor was allocated by the system for the caller to free, once.
        unsafe { LocalFree(self.descriptor) };
    }
}

/// What stops a [`PipeListener`]'s `accept` from another thread.
#[derive(Clone)]
pub struct Stopper(Arc<Owned>);

impl Stopper {
    /// Ends the wait of `accept` (now, and every later one).
    pub fn stop(&self) {
        // SAFETY: the event is open as long as this value, a clone of its owner, lives.
        unsafe { SetEvent(self.0.raw()) };
    }
}

/// The daemon's end of its pipe: the first instance is made when it is bound, and each `accept`
/// makes the next before it hands the connected one over, so that the name is never without an
/// instance for longer than the system's own call takes.
pub struct PipeListener {
    name: Vec<u16>,
    security: Security,
    pending: Mutex<Option<Owned>>,
    stop: Arc<Owned>,
}

impl PipeListener {
    /// Creates the pipe `name` as its first instance, admitting the user `sid` alone. Fails when
    /// another process already holds the name (it is not the daemon's to take).
    pub fn bind(name: &str, sid: &str) -> io::Result<Self> {
        let listener = Self {
            name: wide(name),
            security: Security::for_user(sid)?,
            pending: Mutex::new(None),
            stop: Arc::new(event(true)?),
        };
        let first = listener.instance(true)?;
        *listener
            .pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(first);
        Ok(listener)
    }

    fn instance(&self, first: bool) -> io::Result<Owned> {
        let open = PIPE_ACCESS_DUPLEX
            | FILE_FLAG_OVERLAPPED
            | if first {
                FILE_FLAG_FIRST_PIPE_INSTANCE
            } else {
                0
            };
        // The name is a null-terminated wide string owned by the listener, and the security
        // attributes point at a descriptor owned by it; failure is the invalid handle.
        // SAFETY: both pointers are valid for the whole call, as just said.
        let handle = unsafe {
            CreateNamedPipeW(
                self.name.as_ptr(),
                open,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                PIPE_BUFFER,
                PIPE_BUFFER,
                0,
                &self.security.attributes,
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(last());
        }
        Ok(Owned(handle))
    }

    /// What ends `accept` from another thread.
    pub fn stopper(&self) -> Stopper {
        Stopper(Arc::clone(&self.stop))
    }

    /// Waits for a client and returns its connection; [`io::ErrorKind::Interrupted`] when the
    /// [`Stopper`] was used.
    pub fn accept(&self) -> io::Result<PipeStream> {
        let waiting = self
            .pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let waiting = match waiting {
            Some(instance) => instance,
            None => self.instance(false)?,
        };
        let event = event(true)?;
        let mut overlapped = overlapped_with(&event);
        let handle = waiting.raw();
        // SAFETY: the handle is an open overlapped pipe instance; the overlapped outlives the
        // operation (it is waited for, or cancelled and waited for, before this function returns).
        let started = unsafe { ConnectNamedPipe(handle, &mut overlapped) };
        if started == 0 {
            let error = last();
            match code(&error) {
                // A client connected between the creation of the instance and this call.
                Some(ERROR_PIPE_CONNECTED) => {}
                Some(ERROR_IO_PENDING) => {
                    let handles = [event.raw(), self.stop.raw()];
                    // SAFETY: both handles are open events; the array outlives the call.
                    let woken = unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, INFINITE) };
                    let mut moved = 0u32;
                    if woken != WAIT_OBJECT_0 {
                        // Stopped: the connect is cancelled and waited for.
                        // SAFETY: the overlapped is the connect's own.
                        unsafe { CancelIoEx(handle, &overlapped) };
                        // SAFETY: as above; the call waits for the cancelled connect to be over.
                        unsafe { GetOverlappedResult(handle, &overlapped, &mut moved, 1) };
                        return Err(io::Error::from(io::ErrorKind::Interrupted));
                    }
                    // SAFETY: the connect is over (its event was signalled).
                    if unsafe { GetOverlappedResult(handle, &overlapped, &mut moved, 0) } == 0 {
                        return Err(last());
                    }
                }
                _ => return Err(error),
            }
        }
        // The next instance is made before this one is handed over; if it cannot be made now, the
        // next `accept` makes it.
        if let Ok(next) = self.instance(false) {
            *self.pending.lock().unwrap_or_else(PoisonError::into_inner) = Some(next);
        }
        Ok(PipeStream::new(waiting, End::Server))
    }

    /// The SIDs of the entries of the pipe's DACL, read back from the system (a test's proof that
    /// the pipe admits the user alone, and that the default was not taken).
    pub fn dacl_trustees(&self) -> io::Result<Vec<String>> {
        let pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(instance) = pending.as_ref() else {
            return Err(io::Error::other("no instance of the pipe is waiting"));
        };
        dacl_trustees_of(instance.raw())
    }
}

/// The SIDs of the access-allowed entries of the DACL of the object `handle` names.
fn dacl_trustees_of(handle: HANDLE) -> io::Result<Vec<String>> {
    let mut acl: *mut ACL = std::ptr::null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    // SAFETY: the handle is open; the out pointers are valid; only the DACL is asked for.
    let status = unsafe {
        GetSecurityInfo(
            handle,
            SE_KERNEL_OBJECT,
            DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut acl,
            std::ptr::null_mut(),
            &mut descriptor,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    let result = read_trustees(acl);
    if !descriptor.is_null() {
        // SAFETY: the descriptor was allocated by GetSecurityInfo for the caller to free, once.
        unsafe { LocalFree(descriptor) };
    }
    result
}

fn read_trustees(acl: *mut ACL) -> io::Result<Vec<String>> {
    if acl.is_null() {
        // No DACL at all admits everyone: the one answer a caller must not take for none.
        return Err(io::Error::other(
            "the object has no DACL (it admits everyone)",
        ));
    }
    // SAFETY: an all-zero ACL_SIZE_INFORMATION is a valid value of this plain C struct.
    let mut info: ACL_SIZE_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: the ACL is valid (it lives in the descriptor the caller still holds); the buffer
    // matches the class asked for.
    let ok = unsafe {
        GetAclInformation(
            acl,
            (&mut info as *mut ACL_SIZE_INFORMATION).cast(),
            u32::try_from(std::mem::size_of::<ACL_SIZE_INFORMATION>()).unwrap_or(0),
            AclSizeInformation,
        )
    };
    if ok == 0 {
        return Err(last());
    }
    let mut trustees = Vec::new();
    for index in 0..info.AceCount {
        let mut ace: *mut core::ffi::c_void = std::ptr::null_mut();
        // SAFETY: the ACL is valid and `index` is below its entry count.
        if unsafe { GetAce(acl, index, &mut ace) } == 0 {
            return Err(last());
        }
        // SAFETY: every entry begins with the header and, for the access-allowed type, the mask
        // and the SID; the SID starts at `SidStart`, inside the ACL's memory. Read in place.
        let sid: PSID =
            unsafe { std::ptr::addr_of_mut!((*ace.cast::<ACCESS_ALLOWED_ACE>()).SidStart).cast() };
        trustees.push(sid_string(sid)?);
    }
    Ok(trustees)
}

/// A SID in its string form (`S-1-5-21-…`).
fn sid_string(sid: PSID) -> io::Result<String> {
    let mut text: *mut u16 = std::ptr::null_mut();
    // SAFETY: the SID is valid for the call; the out pointer is valid.
    if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 {
        return Err(last());
    }
    let mut len = 0usize;
    // SAFETY: the system returned a null-terminated wide string, valid until it is freed below.
    while unsafe { *text.add(len) } != 0 {
        len += 1;
    }
    // SAFETY: `len` wide characters before the terminator are readable.
    let shown = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, len) });
    // SAFETY: the string was allocated by the system for the caller to free, once.
    unsafe { LocalFree(text.cast()) };
    Ok(shown)
}

/// The user of the token `token` as a SID string.
fn token_user_sid(token: HANDLE) -> io::Result<String> {
    let mut size = 0u32;
    // SAFETY: a null buffer of length zero asks for the size, which the call reports.
    unsafe { GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut size) };
    if size == 0 {
        return Err(last());
    }
    let mut bytes = vec![0u8; size as usize];
    // SAFETY: the buffer has the size the system asked for.
    if unsafe { GetTokenInformation(token, TokenUser, bytes.as_mut_ptr().cast(), size, &mut size) }
        == 0
    {
        return Err(last());
    }
    if bytes.len() < std::mem::size_of::<PSID>() {
        return Err(io::Error::other(
            "the token's user is shorter than a pointer",
        ));
    }
    // SAFETY: TOKEN_USER begins with a SID pointer that refers into the same buffer; read
    // unaligned, as a byte buffer has no alignment.
    let sid: PSID = unsafe { bytes.as_ptr().cast::<PSID>().read_unaligned() };
    if sid.is_null() {
        return Err(io::Error::other("the token has no user"));
    }
    sid_string(sid)
}

/// The SID, as a string, of the user this process runs as.
pub fn current_user_sid() -> io::Result<String> {
    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: the pseudo-handle of the current process is always valid; `token` is a valid out
    // pointer.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(last());
    }
    let token = Owned(token);
    token_user_sid(token.raw())
}

/// The SID, as a string, of the user the process `pid` runs as: the user of its token. An error
/// when the process cannot be opened or its token cannot be read: the caller refuses such a peer.
pub fn process_user_sid(pid: u32) -> io::Result<String> {
    let process = open_process(pid, PROCESS_QUERY_LIMITED_INFORMATION)?;
    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: the process handle is open; `token` is a valid out pointer.
    if unsafe { OpenProcessToken(process.raw(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(last());
    }
    let token = Owned(token);
    token_user_sid(token.raw())
}

fn open_process(pid: u32, access: u32) -> io::Result<Owned> {
    // SAFETY: no handle is inherited; the call returns a null handle on failure, which is checked.
    let handle = unsafe { OpenProcess(access, 0, pid) };
    if handle.is_null() {
        return Err(last());
    }
    Ok(Owned(handle))
}

/// A Job Object the daemon made, as a number: what a check of the daemon's own tree is asked
/// against. It is only a value: using it after the job was closed is an error of the system's
/// (`IsProcessInJob` fails), never a fault of memory, and the check refuses on an error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JobRef(usize);

impl JobRef {
    /// The job of the handle `raw` (the worker's own Job Object, read from its owner).
    pub fn from_raw(raw: usize) -> Self {
        Self(raw)
    }

    /// The handle's value.
    pub fn raw(self) -> usize {
        self.0
    }
}

/// An empty Job Object of its own that holds one process and what it makes from then on: the
/// daemon's mark on a tool server's tree. It sets no limit and does not end the tree when it is
/// closed. It nests inside the job the process was already in (Windows 8 and later).
pub struct JobMark {
    handle: Owned,
}

impl JobMark {
    /// Puts the process `pid` into a new job of its own.
    pub fn put(pid: u32) -> io::Result<Self> {
        // SAFETY: no attributes and no name are given; the call returns a null handle on failure,
        // which is checked.
        let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if job.is_null() {
            return Err(last());
        }
        let job = Owned(job);
        let process = open_process(pid, PROCESS_SET_QUOTA | PROCESS_TERMINATE)?;
        // SAFETY: both handles are open for the call.
        if unsafe { AssignProcessToJobObject(job.raw(), process.raw()) } == 0 {
            return Err(last());
        }
        Ok(Self { handle: job })
    }

    /// Whether the job holds no process any more (every one has ended). `false` when the system
    /// cannot say: a mark is kept rather than dropped by mistake.
    pub fn is_empty(&self) -> bool {
        // SAFETY: an all-zero JOBOBJECT_BASIC_ACCOUNTING_INFORMATION is a valid value of this plain C struct.
        let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: the job handle is open as long as `self`; the buffer matches the class asked for.
        let ok = unsafe {
            QueryInformationJobObject(
                self.handle.raw(),
                JobObjectBasicAccountingInformation,
                (&mut info as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                u32::try_from(std::mem::size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>())
                    .unwrap_or(0),
                std::ptr::null_mut(),
            )
        };
        ok != 0 && info.ActiveProcesses == 0
    }

    /// The job, for [`process_in_any_job`], while this mark lives.
    pub fn job(&self) -> JobRef {
        JobRef(self.handle.raw() as usize)
    }
}

/// Whether the process `pid` is in one of `jobs`. An error when the process cannot be opened or
/// the system cannot say for one of them: the caller refuses a peer it cannot place.
pub fn process_in_any_job(pid: u32, jobs: &[JobRef]) -> io::Result<bool> {
    if jobs.is_empty() {
        return Ok(false);
    }
    let process = open_process(pid, PROCESS_QUERY_LIMITED_INFORMATION)?;
    let mut inside = false;
    let mut failure = None;
    for job in jobs {
        let mut answer = 0i32;
        // SAFETY: the process handle is open; the job is a handle value the system checks (a
        // closed one is an error, not a fault); `answer` is a valid out pointer.
        let ok = unsafe { IsProcessInJob(process.raw(), job.0 as HANDLE, &mut answer) };
        if ok == 0 {
            failure.get_or_insert_with(last);
        } else if answer != 0 {
            inside = true;
        }
    }
    // A job that holds the process settles it, whatever the others said.
    match (inside, failure) {
        (true, _) => Ok(true),
        (false, Some(error)) => Err(error),
        (false, None) => Ok(false),
    }
}

/// Fills `buf` from the system's random source.
pub fn random_bytes(buf: &mut [u8]) -> io::Result<()> {
    let len = u32::try_from(buf.len()).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    // SAFETY: the buffer is `len` writable bytes; no algorithm handle is given, so the system's
    // preferred generator is used.
    let status = unsafe {
        BCryptGenRandom(
            std::ptr::null_mut(),
            buf.as_mut_ptr(),
            len,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    };
    if status != 0 {
        return Err(io::Error::other(format!(
            "BCryptGenRandom failed: {status:#x}"
        )));
    }
    Ok(())
}

/// Points the standard streams of this process at `NUL` and closes what they were: the daemon
/// started detached has no terminal, and whoever started it (which reads its standard error until
/// it listens) sees the pipe end when this returns.
pub fn detach_standard_streams() -> io::Result<()> {
    use std::os::windows::io::IntoRawHandle;
    for which in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        let null = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("NUL")?
            .into_raw_handle() as HANDLE;
        // SAFETY: the standard handle of the process is read, and replaced by an open handle that
        // this call now owns; the old one is closed once, after it was replaced.
        unsafe {
            let old = GetStdHandle(which);
            if SetStdHandle(which, null) == 0 {
                let error = last();
                CloseHandle(null);
                return Err(error);
            }
            if !old.is_null() && old != INVALID_HANDLE_VALUE {
                CloseHandle(old);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
