//! The Windows backend (RFC 011 §3, §7; REQ-CONF-005): the worker is
//! spawned by the runtime inside an **AppContainer** without network
//! capabilities (clauses F and N through the container SID and the ACEs of
//! `acl`), in a **Job Object** with kill-on-close, breakaway forbidden and
//! the memory and process-count limits (K, M, C), under a **restricted,
//! low-integrity token** where the system accepts one together with the
//! container (P; the container's lowbox token is restricted by construction
//! otherwise), with an explicit environment (E). Windows is the one platform
//! where the parent must act, because the container and the token are
//! attributes of process creation; the worker still reports what it finds
//! itself in (`inspect`), so the status is never the parent's assumption.
//!
//! This is the one place in the crate that calls foreign functions; every
//! `unsafe` block is justified (REQ-CONF-010).

#![allow(unsafe_code)]

pub mod acl;

use super::ConfinementSpec;
use mimlys_core::confine::ClauseState;
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{FromRawHandle, OwnedHandle};
use std::path::Path;
use std::process::{ChildStderr, ChildStdin, ChildStdout, ExitStatus};
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, HANDLE, HANDLE_FLAG_INHERIT, LocalFree, STILL_ACTIVE,
    SetHandleInformation, WAIT_OBJECT_0,
};
use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows_sys::Win32::Security::Isolation::{
    CreateAppContainerProfile, DeriveAppContainerSidFromAppContainerName,
};
use windows_sys::Win32::Security::{
    CreateRestrictedToken, CreateWellKnownSid, DISABLE_MAX_PRIVILEGE, EqualSid, FreeSid,
    GetLengthSid, GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation,
    IsTokenRestricted, PSID, SECURITY_ATTRIBUTES, SECURITY_CAPABILITIES, SECURITY_MAX_SID_SIZE,
    SID_AND_ATTRIBUTES, SetTokenInformation, TOKEN_ADJUST_DEFAULT, TOKEN_ADJUST_SESSIONID,
    TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE, TOKEN_GROUPS, TOKEN_MANDATORY_LABEL, TOKEN_QUERY,
    TokenCapabilities, TokenIntegrityLevel, TokenIsAppContainer, WinCapabilityInternetClientSid,
    WinLowLabelSid,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, IsProcessInJob, JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
    JOB_OBJECT_LIMIT_BREAKAWAY_OK, JOB_OBJECT_LIMIT_JOB_MEMORY, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectExtendedLimitInformation, QueryInformationJobObject, SetInformationJobObject,
    TerminateJobObject,
};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::SystemServices::{SE_GROUP_ENABLED, SE_GROUP_INTEGRITY};
use windows_sys::Win32::System::Threading::{
    CREATE_NO_WINDOW, CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessAsUserW,
    CreateProcessW, DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT, GetCurrentProcess,
    GetExitCodeProcess, InitializeProcThreadAttributeList, OpenProcessToken,
    PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES,
    PROCESS_INFORMATION, ResumeThread, STARTF_USESTDHANDLES, STARTUPINFOEXW, TerminateProcess,
    UpdateProcThreadAttribute, WaitForSingleObject,
};

/// An owned Job Object handle.
pub struct Job {
    handle: HANDLE,
}

// SAFETY: a Job Object handle is a kernel object reference that may be used from any thread.
unsafe impl Send for Job {}
unsafe impl Sync for Job {}

impl Job {
    /// Creates an anonymous job with kill-on-close; breakaway flags are not set, so no child may leave it.
    pub fn new() -> Result<Self, String> {
        Self::with_limits(None, None)
    }

    /// A job with kill-on-close and, when given, the tree's memory ceiling
    /// (LIM-H2, clause M) and its process count (LIM-H3, clause C).
    pub fn with_limits(
        memory_bytes: Option<u64>,
        max_processes: Option<u32>,
    ) -> Result<Self, String> {
        // SAFETY: CreateJobObjectW takes optional security attributes (null means default) and an
        // optional name (null means anonymous); it returns a null handle on failure, checked below.
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() {
            return Err(format!(
                "CreateJobObjectW failed: {}",
                std::io::Error::last_os_error()
            ));
        }
        // SAFETY: a zeroed JOBOBJECT_EXTENDED_LIMIT_INFORMATION is a valid value of this plain C struct.
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if let Some(bytes) = memory_bytes {
            info.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_JOB_MEMORY;
            info.JobMemoryLimit = usize::try_from(bytes).unwrap_or(usize::MAX);
        }
        if let Some(count) = max_processes {
            info.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
            info.BasicLimitInformation.ActiveProcessLimit = count;
        }
        // SAFETY: the pointer and the length describe one fully initialised struct of the class given.
        let ok = unsafe {
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                (&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if ok == 0 {
            let err = std::io::Error::last_os_error();
            // SAFETY: the handle was returned by CreateJobObjectW and is closed exactly once.
            unsafe { CloseHandle(handle) };
            return Err(format!("SetInformationJobObject failed: {err}"));
        }
        Ok(Self { handle })
    }

    /// Puts a process (and so its descendants) into the job.
    fn assign_handle(&self, process: HANDLE) -> Result<(), String> {
        // SAFETY: both handles are valid for the duration of the call.
        let ok = unsafe { AssignProcessToJobObject(self.handle, process) };
        if ok == 0 {
            return Err(format!(
                "AssignProcessToJobObject failed: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    }

    /// Terminates every process in the job.
    pub fn kill(&mut self) {
        // SAFETY: the job handle is valid until Drop closes it.
        unsafe { TerminateJobObject(self.handle, 1) };
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        // SAFETY: the handle is owned by this value and closed exactly once; kill-on-close ends the tree.
        unsafe { CloseHandle(self.handle) };
    }
}

/// A process started by [`spawn`]: its handle and its piped streams.
pub struct RawChild {
    process: OwnedHandle,
    pid: u32,
    /// The child's standard input, until taken.
    pub stdin: Option<ChildStdin>,
    /// The child's standard output, until taken.
    pub stdout: Option<ChildStdout>,
    /// The child's standard error, until taken.
    pub stderr: Option<ChildStderr>,
}

impl RawChild {
    /// The process id.
    pub fn id(&self) -> u32 {
        self.pid
    }

    /// The exit status when the process has ended, else `None`.
    pub fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        use std::os::windows::io::AsRawHandle;
        use std::os::windows::process::ExitStatusExt;
        let handle = self.process.as_raw_handle() as HANDLE;
        // SAFETY: the process handle is owned by this value and valid; a zero wait is a poll.
        if unsafe { WaitForSingleObject(handle, 0) } != WAIT_OBJECT_0 {
            return Ok(None);
        }
        let mut code: u32 = 0;
        // SAFETY: the handle is valid and `code` is a valid out pointer.
        if unsafe { GetExitCodeProcess(handle, &mut code) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        if code == STILL_ACTIVE as u32 {
            return Ok(None);
        }
        Ok(Some(ExitStatus::from_raw(code)))
    }

    /// Waits for the process to end.
    pub fn wait(&mut self) -> std::io::Result<ExitStatus> {
        loop {
            if let Some(status) = self.try_wait()? {
                return Ok(status);
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    /// Terminates the process (not its tree; the job does that).
    pub fn kill(&mut self) -> std::io::Result<()> {
        use std::os::windows::io::AsRawHandle;
        // SAFETY: the process handle is owned by this value and valid.
        if unsafe { TerminateProcess(self.process.as_raw_handle() as HANDLE, 1) } == 0 {
            let err = std::io::Error::last_os_error();
            if self.try_wait()?.is_some() {
                return Ok(());
            }
            return Err(err);
        }
        Ok(())
    }
}

/// How to start a process (RFC 011 §3).
pub struct SpawnOptions<'a> {
    /// The program, an absolute path.
    pub program: &'a Path,
    /// Its arguments.
    pub args: &'a [String],
    /// The explicit environment (clause E).
    pub env: &'a [(String, String)],
    /// The working directory.
    pub current_dir: Option<&'a Path>,
    /// The AppContainer to run in, as its SID, and whether the internet
    /// client capability is granted (the dependency step only).
    pub container: Option<(&'a [u8], bool)>,
    /// The job to belong to before the first instruction runs.
    pub job: Option<&'a Job>,
    /// Try a restricted, low-integrity primary token first (clause P).
    pub restricted_token: bool,
}

fn wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(std::iter::once(0)).collect()
}

/// One argument quoted the way the C runtime parses it.
fn quote_arg(arg: &str, out: &mut Vec<u16>) {
    let needs = arg.is_empty() || arg.chars().any(|c| matches!(c, ' ' | '\t' | '"' | '\n'));
    if needs {
        out.push(b'"' as u16);
    }
    let mut backslashes = 0;
    for c in arg.encode_utf16() {
        if c == b'\\' as u16 {
            backslashes += 1;
            out.push(c);
            continue;
        }
        if c == b'"' as u16 {
            out.extend(std::iter::repeat_n(b'\\' as u16, backslashes + 1));
        }
        backslashes = 0;
        out.push(c);
    }
    if needs {
        out.extend(std::iter::repeat_n(b'\\' as u16, backslashes));
        out.push(b'"' as u16);
    }
}

fn command_line(program: &Path, args: &[String]) -> Vec<u16> {
    let mut line = Vec::new();
    quote_arg(&program.to_string_lossy(), &mut line);
    for a in args {
        line.push(b' ' as u16);
        quote_arg(a, &mut line);
    }
    line.push(0);
    line
}

fn environment_block(env: &[(String, String)]) -> Vec<u16> {
    let mut entries: Vec<&(String, String)> = env.iter().collect();
    entries.sort_by_key(|a| a.0.to_uppercase());
    let mut block = Vec::new();
    for (k, v) in entries {
        block.extend(k.encode_utf16());
        block.push(b'=' as u16);
        block.extend(v.encode_utf16());
        block.push(0);
    }
    block.push(0);
    block
}

struct Pipe {
    read: HANDLE,
    write: HANDLE,
}

/// An inheritable anonymous pipe; the parent's end stops being inheritable.
fn pipe(parent_reads: bool) -> Result<Pipe, String> {
    let attrs = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: std::ptr::null_mut(),
        bInheritHandle: 1,
    };
    let mut read: HANDLE = std::ptr::null_mut();
    let mut write: HANDLE = std::ptr::null_mut();
    // SAFETY: both out pointers are valid; the attributes struct is fully initialised.
    if unsafe { CreatePipe(&mut read, &mut write, &attrs, 0) } == 0 {
        return Err(format!(
            "CreatePipe failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    let parent_end = if parent_reads { read } else { write };
    // SAFETY: the handle was just returned by CreatePipe and is valid.
    if unsafe { SetHandleInformation(parent_end, HANDLE_FLAG_INHERIT, 0) } == 0 {
        let err = std::io::Error::last_os_error();
        // SAFETY: both handles are valid and closed exactly once on this error path.
        unsafe {
            CloseHandle(read);
            CloseHandle(write);
        }
        return Err(format!("SetHandleInformation failed: {err}"));
    }
    Ok(Pipe { read, write })
}

/// A primary token restricted to no privileges at low integrity, from this
/// process's own token (clause P). `None` when the system refuses: the
/// caller then spawns with its own token and the worker reports the fact.
fn restricted_low_token() -> Option<OwnedHandle> {
    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: the pseudo-handle of the current process is always valid; `token` is a valid out pointer.
    let ok = unsafe {
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_DUPLICATE
                | TOKEN_QUERY
                | TOKEN_ASSIGN_PRIMARY
                | TOKEN_ADJUST_DEFAULT
                | TOKEN_ADJUST_SESSIONID,
            &mut token,
        )
    };
    if ok == 0 {
        return None;
    }
    // SAFETY: the handle was returned by OpenProcessToken and is owned from here on.
    let token = unsafe { OwnedHandle::from_raw_handle(token as _) };
    let mut restricted: HANDLE = std::ptr::null_mut();
    // SAFETY: the source handle is valid; no SIDs or privileges are listed (null pointers with zero counts).
    let ok = unsafe {
        use std::os::windows::io::AsRawHandle;
        CreateRestrictedToken(
            token.as_raw_handle() as HANDLE,
            DISABLE_MAX_PRIVILEGE,
            0,
            std::ptr::null(),
            0,
            std::ptr::null(),
            0,
            std::ptr::null(),
            &mut restricted,
        )
    };
    if ok == 0 {
        return None;
    }
    // SAFETY: the handle was returned by CreateRestrictedToken and is owned from here on.
    let restricted = unsafe { OwnedHandle::from_raw_handle(restricted as _) };
    let low = well_known_sid(WinLowLabelSid)?;
    let label = TOKEN_MANDATORY_LABEL {
        Label: SID_AND_ATTRIBUTES {
            Sid: low.as_ptr() as PSID,
            Attributes: SE_GROUP_INTEGRITY as u32,
        },
    };
    // SAFETY: the label points at a SID buffer that outlives the call; the length covers the struct and the SID.
    let ok = unsafe {
        use std::os::windows::io::AsRawHandle;
        SetTokenInformation(
            restricted.as_raw_handle() as HANDLE,
            TokenIntegrityLevel,
            (&label as *const TOKEN_MANDATORY_LABEL).cast(),
            (std::mem::size_of::<TOKEN_MANDATORY_LABEL>() + low.len()) as u32,
        )
    };
    (ok != 0).then_some(restricted)
}

/// A well-known SID as bytes.
fn well_known_sid(kind: i32) -> Option<Vec<u8>> {
    let mut buf = vec![0u8; SECURITY_MAX_SID_SIZE as usize];
    let mut len = buf.len() as u32;
    // SAFETY: the buffer is SECURITY_MAX_SID_SIZE bytes, the documented maximum; `len` is in and out.
    let ok = unsafe {
        CreateWellKnownSid(
            kind,
            std::ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &mut len,
        )
    };
    if ok == 0 {
        return None;
    }
    buf.truncate(len as usize);
    Some(buf)
}

/// Copies a SID the system allocated into owned bytes.
fn copy_sid(sid: PSID) -> Vec<u8> {
    // SAFETY: the SID pointer came from the system and is valid for GetLengthSid.
    let len = unsafe { GetLengthSid(sid) } as usize;
    let mut bytes = vec![0u8; len];
    // SAFETY: `len` bytes are readable at `sid`; the destination has `len` bytes.
    unsafe { std::ptr::copy_nonoverlapping(sid as *const u8, bytes.as_mut_ptr(), len) };
    bytes
}

/// The AppContainer profile name for a data directory (RFC 011 §3): a fixed
/// prefix and the first sixteen hex digits of the directory's hash, so that
/// one installation is one container and the name is valid wherever it goes.
pub fn profile_name(data_dir: &Path) -> String {
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(data_dir.to_string_lossy().to_lowercase().as_bytes());
    let hex: String = hash.iter().take(8).map(|b| format!("{b:02x}")).collect();
    format!("mimlys-{hex}")
}

/// Creates the AppContainer profile when it does not exist and returns its
/// SID as bytes (the same SID whether created now or earlier).
pub fn ensure_profile(name: &str) -> Result<Vec<u8>, String> {
    let name_w = wide(OsStr::new(name));
    let desc_w = wide(OsStr::new("mimlys worker: confined host tools"));
    let mut sid: PSID = std::ptr::null_mut();
    // SAFETY: the strings are null-terminated UTF-16; no capabilities are given; `sid` is a valid out pointer.
    let hr = unsafe {
        CreateAppContainerProfile(
            name_w.as_ptr(),
            name_w.as_ptr(),
            desc_w.as_ptr(),
            std::ptr::null(),
            0,
            &mut sid,
        )
    };
    let already = hr == hresult_from_win32(ERROR_ALREADY_EXISTS);
    if hr < 0 && !already {
        return Err(format!(
            "CreateAppContainerProfile failed: 0x{:08x}",
            hr as u32
        ));
    }
    if already {
        // SAFETY: the name is null-terminated UTF-16 and `sid` a valid out pointer.
        let hr = unsafe { DeriveAppContainerSidFromAppContainerName(name_w.as_ptr(), &mut sid) };
        if hr < 0 {
            return Err(format!(
                "DeriveAppContainerSidFromAppContainerName failed: 0x{:08x}",
                hr as u32
            ));
        }
    }
    let bytes = copy_sid(sid);
    // SAFETY: the SID was allocated by the system for the caller to free once with FreeSid.
    unsafe { FreeSid(sid) };
    Ok(bytes)
}

fn hresult_from_win32(code: u32) -> i32 {
    if code == 0 {
        0
    } else {
        ((code & 0x0000_ffff) | 0x8007_0000) as i32
    }
}

/// `S-1-15-2-…`: the SID as text, for records and the status.
pub fn sid_string(sid: &[u8]) -> String {
    let mut text: *mut u16 = std::ptr::null_mut();
    // SAFETY: the SID bytes are a valid SID (copied from the system); `text` receives an allocation freed below.
    if unsafe { ConvertSidToStringSidW(sid.as_ptr() as PSID, &mut text) } == 0 || text.is_null() {
        return String::new();
    }
    let mut len = 0;
    // SAFETY: the string is null-terminated.
    while unsafe { *text.add(len) } != 0 {
        len += 1;
    }
    // SAFETY: `len` UTF-16 units are readable at `text`.
    let s = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, len) });
    // SAFETY: the allocation came from ConvertSidToStringSidW and is freed exactly once.
    unsafe { LocalFree(text.cast()) };
    s
}

/// Starts a process suspended, puts it into the job, and only then lets it
/// run, so that nothing escapes the job in the first instructions; the
/// container and the token are process-creation attributes (RFC 011 §3).
pub fn spawn(opts: SpawnOptions<'_>) -> Result<RawChild, String> {
    let stdin = pipe(false)?;
    let stdout = pipe(true)?;
    let stderr = pipe(true)?;
    let child_handles: [HANDLE; 3] = [stdin.read, stdout.write, stderr.write];

    let mut size: usize = 0;
    let attribute_count = if opts.container.is_some() { 2 } else { 1 };
    // SAFETY: a null list with a size out pointer asks for the size, which is the documented first call.
    unsafe {
        InitializeProcThreadAttributeList(std::ptr::null_mut(), attribute_count, 0, &mut size)
    };
    let mut list = vec![0u8; size.max(1)];
    // SAFETY: the buffer has the size the system asked for.
    if unsafe {
        InitializeProcThreadAttributeList(list.as_mut_ptr().cast(), attribute_count, 0, &mut size)
    } == 0
    {
        return Err(format!(
            "InitializeProcThreadAttributeList failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    let list_ptr = list.as_mut_ptr().cast();
    // SAFETY: the list is initialised; the handle array outlives the CreateProcess call below.
    if unsafe {
        UpdateProcThreadAttribute(
            list_ptr,
            0,
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
            child_handles.as_ptr().cast(),
            std::mem::size_of::<[HANDLE; 3]>(),
            std::ptr::null_mut(),
            std::ptr::null(),
        )
    } == 0
    {
        return Err(format!(
            "UpdateProcThreadAttribute(handles) failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    let internet = well_known_sid(WinCapabilityInternetClientSid);
    let mut capability = SID_AND_ATTRIBUTES {
        Sid: std::ptr::null_mut(),
        Attributes: SE_GROUP_ENABLED as u32,
    };
    let mut caps = SECURITY_CAPABILITIES::default();
    if let Some((sid, allow_network)) = opts.container {
        caps.AppContainerSid = sid.as_ptr() as PSID;
        if allow_network && let Some(net) = &internet {
            capability.Sid = net.as_ptr() as PSID;
            caps.Capabilities = &mut capability;
            caps.CapabilityCount = 1;
        }
        // SAFETY: `caps` and the SIDs it points at outlive the CreateProcess call below.
        if unsafe {
            UpdateProcThreadAttribute(
                list_ptr,
                0,
                PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES as usize,
                (&caps as *const SECURITY_CAPABILITIES).cast(),
                std::mem::size_of::<SECURITY_CAPABILITIES>(),
                std::ptr::null_mut(),
                std::ptr::null(),
            )
        } == 0
        {
            return Err(format!(
                "UpdateProcThreadAttribute(capabilities) failed: {}",
                std::io::Error::last_os_error()
            ));
        }
    }

    let mut si = STARTUPINFOEXW::default();
    si.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
    si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    si.StartupInfo.hStdInput = stdin.read;
    si.StartupInfo.hStdOutput = stdout.write;
    si.StartupInfo.hStdError = stderr.write;
    si.lpAttributeList = list_ptr;
    let program_w = wide(opts.program.as_os_str());
    let mut line = command_line(opts.program, opts.args);
    let env_block = environment_block(opts.env);
    let cwd_w = opts.current_dir.map(|d| wide(d.as_os_str()));
    let flags = CREATE_SUSPENDED
        | CREATE_UNICODE_ENVIRONMENT
        | CREATE_NO_WINDOW
        | EXTENDED_STARTUPINFO_PRESENT;
    let mut pi = PROCESS_INFORMATION::default();

    let token = if opts.restricted_token {
        restricted_low_token()
    } else {
        None
    };
    let mut created = false;
    if let Some(token) = &token {
        use std::os::windows::io::AsRawHandle;
        // SAFETY: every pointer refers to a buffer that lives until the call returns; `pi` is a valid out pointer.
        created = unsafe {
            CreateProcessAsUserW(
                token.as_raw_handle() as HANDLE,
                program_w.as_ptr(),
                line.as_mut_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                1,
                flags,
                env_block.as_ptr().cast(),
                cwd_w.as_ref().map_or(std::ptr::null(), |c| c.as_ptr()),
                &si.StartupInfo,
                &mut pi,
            )
        } != 0;
    }
    if !created {
        // SAFETY: as above; the caller's own token applies.
        created = unsafe {
            CreateProcessW(
                program_w.as_ptr(),
                line.as_mut_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                1,
                flags,
                env_block.as_ptr().cast(),
                cwd_w.as_ref().map_or(std::ptr::null(), |c| c.as_ptr()),
                &si.StartupInfo,
                &mut pi,
            )
        } != 0;
    }
    let spawn_error = (!created).then(std::io::Error::last_os_error);
    // SAFETY: the list was initialised above and is deleted exactly once.
    unsafe { DeleteProcThreadAttributeList(list_ptr) };
    // SAFETY: the child's pipe ends are valid handles the parent no longer needs.
    unsafe {
        CloseHandle(stdin.read);
        CloseHandle(stdout.write);
        CloseHandle(stderr.write);
    }
    if let Some(err) = spawn_error {
        // SAFETY: the parent's pipe ends are valid and closed exactly once on this error path.
        unsafe {
            CloseHandle(stdin.write);
            CloseHandle(stdout.read);
            CloseHandle(stderr.read);
        }
        return Err(format!("CreateProcess failed: {err}"));
    }
    // SAFETY: CreateProcess returned these handles for the caller to own.
    let process = unsafe { OwnedHandle::from_raw_handle(pi.hProcess as _) };
    let thread = unsafe { OwnedHandle::from_raw_handle(pi.hThread as _) };
    if let Some(job) = opts.job
        && let Err(e) = job.assign_handle(pi.hProcess)
    {
        // SAFETY: the process is suspended and owned; it is ended before it ever ran.
        unsafe { TerminateProcess(pi.hProcess, 1) };
        return Err(e);
    }
    {
        use std::os::windows::io::AsRawHandle;
        // SAFETY: the thread handle is valid; the process starts running here, already in the job.
        unsafe { ResumeThread(thread.as_raw_handle() as HANDLE) };
    }
    drop(thread);
    // SAFETY: the parent's pipe ends are valid handles owned from here on by the stream types.
    let (stdin, stdout, stderr) = unsafe {
        (
            ChildStdin::from(OwnedHandle::from_raw_handle(stdin.write as _)),
            ChildStdout::from(OwnedHandle::from_raw_handle(stdout.read as _)),
            ChildStderr::from(OwnedHandle::from_raw_handle(stderr.read as _)),
        )
    };
    Ok(RawChild {
        process,
        pid: pi.dwProcessId,
        stdin: Some(stdin),
        stdout: Some(stdout),
        stderr: Some(stderr),
    })
}

/// What the running process finds itself in: the worker's self-report of
/// clauses F, N, K, P, M and C (RFC 011 §1: detected, never assumed).
pub fn inspect(spec: &ConfinementSpec) -> BTreeMap<String, ClauseState> {
    let mut report = BTreeMap::new();
    let token = own_token();
    let in_container = token
        .as_ref()
        .is_some_and(|t| token_u32(t, TokenIsAppContainer) == Some(1));
    let internet = token
        .as_ref()
        .is_some_and(|t| has_capability(t, WinCapabilityInternetClientSid));
    let low_integrity = token
        .as_ref()
        .and_then(integrity_rid)
        .is_some_and(|rid| rid <= 0x1000);
    let restricted = token.as_ref().is_some_and(|t| {
        use std::os::windows::io::AsRawHandle;
        // SAFETY: the token handle is valid.
        (unsafe { IsTokenRestricted(t.as_raw_handle() as HANDLE) }) != 0
    });
    report.insert(
        "F".into(),
        if in_container {
            ClauseState::enforced("appcontainer")
        } else {
            ClauseState::unavailable("not in an AppContainer")
        },
    );
    report.insert(
        "N".into(),
        if !spec.deny_network {
            ClauseState::unavailable("network allowed for the dependency step")
        } else if in_container && !internet {
            ClauseState::enforced("appcontainer")
        } else if in_container {
            ClauseState::unavailable("appcontainer with a network capability")
        } else {
            ClauseState::unavailable("not in an AppContainer")
        },
    );
    report.insert(
        "P".into(),
        match (in_container, low_integrity, restricted) {
            (true, true, true) => ClauseState::enforced("restricted_token+appcontainer"),
            (true, true, false) => ClauseState::enforced("appcontainer"),
            (true, false, _) => ClauseState::unavailable("appcontainer above low integrity"),
            (false, _, _) => ClauseState::unavailable("not in an AppContainer"),
        },
    );
    let job = own_job_limits();
    let (k, m, c) = match job {
        Some(info) => {
            let flags = info.BasicLimitInformation.LimitFlags;
            let kill = flags & JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE != 0;
            let breakaway =
                flags & (JOB_OBJECT_LIMIT_BREAKAWAY_OK | JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK) != 0;
            (
                if kill && !breakaway {
                    ClauseState::enforced("job_object")
                } else if breakaway {
                    ClauseState::unavailable("job allows breakaway")
                } else {
                    ClauseState::unavailable("job without kill-on-close")
                },
                if flags & JOB_OBJECT_LIMIT_JOB_MEMORY != 0 && info.JobMemoryLimit > 0 {
                    ClauseState::enforced("job_object")
                } else {
                    ClauseState::unavailable("job memory limit not set")
                },
                if flags & JOB_OBJECT_LIMIT_ACTIVE_PROCESS != 0 {
                    ClauseState::enforced("job_object")
                } else {
                    ClauseState::unavailable("job process limit not set")
                },
            )
        }
        None => (
            ClauseState::unavailable("not in a job"),
            ClauseState::unavailable("not in a job"),
            ClauseState::unavailable("not in a job"),
        ),
    };
    report.insert("K".into(), k);
    report.insert("M".into(), m);
    report.insert("C".into(), c);
    report
}

fn own_token() -> Option<OwnedHandle> {
    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: the pseudo-handle is valid; `token` is a valid out pointer.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return None;
    }
    // SAFETY: the handle was returned by OpenProcessToken and is owned from here on.
    Some(unsafe { OwnedHandle::from_raw_handle(token as _) })
}

fn token_bytes(token: &OwnedHandle, class: i32) -> Option<Vec<u8>> {
    use std::os::windows::io::AsRawHandle;
    let handle = token.as_raw_handle() as HANDLE;
    let mut len: u32 = 0;
    // SAFETY: a null buffer with zero length asks for the size (ERROR_INSUFFICIENT_BUFFER is expected).
    unsafe { GetTokenInformation(handle, class, std::ptr::null_mut(), 0, &mut len) };
    if len == 0 {
        return None;
    }
    let mut buf = vec![0u8; len as usize];
    // SAFETY: the buffer has the size the system asked for.
    if unsafe { GetTokenInformation(handle, class, buf.as_mut_ptr().cast(), len, &mut len) } == 0 {
        return None;
    }
    Some(buf)
}

fn token_u32(token: &OwnedHandle, class: i32) -> Option<u32> {
    let bytes = token_bytes(token, class)?;
    bytes
        .get(..4)
        .map(|b| u32::from_ne_bytes([b[0], b[1], b[2], b[3]]))
}

fn integrity_rid(token: &OwnedHandle) -> Option<u32> {
    let bytes = token_bytes(token, TokenIntegrityLevel)?;
    // SAFETY: the buffer holds a TOKEN_MANDATORY_LABEL whose SID pointer refers into the same buffer.
    let label = unsafe { &*(bytes.as_ptr() as *const TOKEN_MANDATORY_LABEL) };
    let sid = label.Label.Sid;
    if sid.is_null() {
        return None;
    }
    // SAFETY: the SID is valid for the sub-authority queries; the count is at least one for a label SID.
    unsafe {
        let count = *GetSidSubAuthorityCount(sid);
        if count == 0 {
            return None;
        }
        Some(*GetSidSubAuthority(sid, u32::from(count) - 1))
    }
}

fn has_capability(token: &OwnedHandle, kind: i32) -> bool {
    let Some(wanted) = well_known_sid(kind) else {
        return false;
    };
    let Some(bytes) = token_bytes(token, TokenCapabilities) else {
        return false;
    };
    // SAFETY: the buffer holds a TOKEN_GROUPS whose entries follow the count; `GroupCount` entries are valid.
    unsafe {
        let groups = &*(bytes.as_ptr() as *const TOKEN_GROUPS);
        let entries =
            std::slice::from_raw_parts(groups.Groups.as_ptr(), groups.GroupCount as usize);
        entries
            .iter()
            .any(|g| EqualSid(g.Sid, wanted.as_ptr() as PSID) != 0)
    }
}

fn own_job_limits() -> Option<JOBOBJECT_EXTENDED_LIMIT_INFORMATION> {
    let mut in_job: i32 = 0;
    // SAFETY: the pseudo-handle is valid; a null job handle asks about any job; `in_job` is a valid out pointer.
    if unsafe { IsProcessInJob(GetCurrentProcess(), std::ptr::null_mut(), &mut in_job) } == 0
        || in_job == 0
    {
        return None;
    }
    // SAFETY: a zeroed JOBOBJECT_EXTENDED_LIMIT_INFORMATION is a valid value of this plain C struct.
    let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: a null job handle means the calling process's job; the buffer matches the class.
    let ok = unsafe {
        QueryInformationJobObject(
            std::ptr::null_mut(),
            JobObjectExtendedLimitInformation,
            (&mut info as *mut JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            std::ptr::null_mut(),
        )
    };
    (ok != 0).then_some(info)
}
