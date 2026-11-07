//! The sweeper (RFC 011 Amendment 2, implementation decisions, point 6;
//! REQ-CONF-009): a process that outlives the worker's death and cleans up
//! after it. On macOS the runtime is outside the worker's sandbox and cannot
//! list its members, so a worker that is killed or crashes would leave its
//! host tools alive (confined, but able to write the workspace after their
//! execution ended). Linux kills the worker's cgroup and Windows closes its
//! job in that case; here the worker forks, right after it verified its
//! sandbox and before any thread exists, one process that holds the **read
//! end of a pipe whose write end only the worker holds**. At end of file,
//! however the worker ended, the sweeper runs what it was given (the kill
//! loop, excluding itself) and exits.
//!
//! What makes it work, each a test:
//!
//! - the write end is close-on-exec, so no tool the worker starts inherits it
//!   (a living tool would hold the pipe open and the sweeper would never see
//!   end of file);
//! - the sweeper has its own process group (a group kill of the worker does
//!   not reach it) and holds none of the worker's standard streams (the
//!   runtime reads end of file on the worker's output only when every holder
//!   is gone, and would otherwise wait for the sweeper too);
//! - it acts only on end of file, never on an error of the read: a worker that
//!   is alive must not lose its tools to a broken descriptor.
//!
//! The module is generic Unix so that the pipe's behaviour is tested on Linux
//! too; only the action (the kill loop) is macOS's. Every `unsafe` block has
//! its `SAFETY:` line (REQ-CONF-010).

#![allow(unsafe_code)]

use std::io;

/// The sweeper as the worker sees it.
#[derive(Debug)]
pub struct Sweeper {
    /// Its process id (the worker's loop excludes it).
    pub pid: i32,
    /// The write end of the pipe, held open for the worker's life (never
    /// closed, never inherited); a raw descriptor on purpose.
    write_end: i32,
}

impl Sweeper {
    /// The write end's descriptor, for the test that checks it is
    /// close-on-exec.
    pub fn write_end(&self) -> i32 {
        self.write_end
    }
}

/// Forks the sweeper. In the child: its own process group, standard streams
/// on `/dev/null`, every other descriptor but the pipe's read end closed; it
/// waits for end of file and runs `on_end_of_file` only then, then exits.
///
/// The caller guarantees that the process has one thread (a fork in a process
/// with others leaves the child with locks no thread will release) and that
/// nothing it has not closed should be inherited by a long-lived child.
pub fn spawn(on_end_of_file: impl FnOnce()) -> io::Result<Sweeper> {
    let mut fds = [0i32; 2];
    // SAFETY: `fds` has room for the two descriptors `pipe` writes.
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let [read_end, write_end] = fds;
    for fd in fds {
        if let Err(e) = close_on_exec(fd) {
            close(read_end);
            close(write_end);
            return Err(e);
        }
    }
    // SAFETY: `fork` in a process of one thread (the caller's guarantee); the child
    // below runs single-threaded code only.
    let pid = unsafe { libc::fork() };
    match pid {
        -1 => {
            let error = io::Error::last_os_error();
            close(read_end);
            close(write_end);
            Err(error)
        }
        0 => {
            close(write_end);
            // SAFETY: `setpgid(0, 0)` puts the calling process in a group of its own.
            unsafe {
                libc::setpgid(0, 0);
            }
            detach_streams();
            close_all_but(read_end);
            if end_of_file(read_end) {
                on_end_of_file();
            }
            // SAFETY: `_exit` ends the child without running the parent's destructors or atexit handlers.
            unsafe { libc::_exit(0) }
        }
        child => {
            close(read_end);
            Ok(Sweeper {
                pid: child,
                write_end,
            })
        }
    }
}

fn close(fd: i32) {
    // SAFETY: closing a descriptor this module opened.
    unsafe {
        libc::close(fd);
    }
}

/// Sets `FD_CLOEXEC`: an `exec` closes the descriptor, so no host tool holds it.
fn close_on_exec(fd: i32) -> io::Result<()> {
    // SAFETY: `fcntl` with `F_GETFD`/`F_SETFD` on a descriptor this module opened takes and returns integers.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: as above.
    if unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// The child's standard streams on `/dev/null`: it must not keep the worker's
/// pipes to the runtime open after the worker is gone.
fn detach_streams() {
    // SAFETY: `open` of a NUL-terminated constant path; `dup2` onto the three standard descriptors; `close` of the one opened.
    unsafe {
        let null = libc::open(c"/dev/null".as_ptr(), libc::O_RDWR);
        if null >= 0 {
            for target in 0..=2 {
                libc::dup2(null, target);
            }
            if null > 2 {
                libc::close(null);
            }
        }
    }
}

/// Closes every descriptor above 2 but `keep`.
fn close_all_but(keep: i32) {
    // SAFETY: `sysconf` takes and returns integers.
    let limit = unsafe { libc::sysconf(libc::_SC_OPEN_MAX) };
    let limit = if limit <= 0 { 1024 } else { limit.min(4096) } as i32;
    for fd in 3..limit {
        if fd != keep {
            close(fd);
        }
    }
}

/// Waits until every write end is closed: `true` at end of file, `false`
/// when the read failed otherwise (then nothing is swept).
fn end_of_file(fd: i32) -> bool {
    let mut byte = 0u8;
    loop {
        // SAFETY: `byte` is one writable byte and the length says so.
        let n = unsafe { libc::read(fd, std::ptr::addr_of_mut!(byte).cast(), 1) };
        if n == 0 {
            return true;
        }
        if n < 0 && io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            return false;
        }
    }
}

#[cfg(test)]
mod tests;
