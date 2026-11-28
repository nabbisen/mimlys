//! POSIX tree primitive: the worker leads a session and a process group of its own, which is
//! killed as a whole (clause K). No foreign-function code: `rustix` is a safe interface; the one
//! `unsafe` is the `pre_exec` that makes the call between the fork and the exec (REQ-CONF-010).

use super::ConfineError;
use rustix::process::{Pid, Signal, kill_process_group};
use std::process::{Child, Command};

/// A process group led by the worker.
pub struct Group {
    pgid: Option<Pid>,
}

impl Group {
    /// Kills every process in the group.
    pub fn kill(&mut self) {
        if let Some(pgid) = self.pgid.take() {
            let _ = kill_process_group(pgid, Signal::KILL);
        }
    }
}

/// Spawns the command as the leader of a new **session**, which is also a new process group of
/// its own (its id is the child's): the group is killed as a whole, and **the session is how the
/// daemon tells its own tree from its clients** (external design §19.5; REQ-SEC-016): the worker
/// leads one, and so does every host tool's group beneath it, in it. Before 0.14 the worker led a
/// process group in its parent's session (handoff 014c, item 1: the premise that it already led
/// a session was not true).
#[allow(unsafe_code)]
pub fn spawn_in_group(mut cmd: Command) -> Result<(Child, Group), ConfineError> {
    use std::os::unix::process::CommandExt;
    // The closure runs in the forked child before `exec`: one `setsid(2)`, which allocates
    // nothing and takes no lock (REQ-CONF-010). It makes the child a session leader and the
    // leader of a group of its own, so that no `process_group` call is made (it would fail for
    // a session leader).
    // SAFETY: `setsid` is async-signal-safe and touches only the calling process.
    unsafe {
        cmd.pre_exec(|| {
            rustix::process::setsid()
                .map(|_| ())
                .map_err(std::io::Error::from)
        });
    }
    let child = cmd.spawn().map_err(ConfineError::Spawn)?;
    let pgid = Pid::from_child(&child);
    Ok((child, Group { pgid: Some(pgid) }))
}

/// Clause P on Linux: the calling process (the worker) forbids privilege
/// gain for itself and everything it starts.
#[cfg(target_os = "linux")]
pub fn set_no_new_privs() -> std::io::Result<()> {
    rustix::thread::set_no_new_privs(true).map_err(std::io::Error::from)
}

/// Whether `no_new_privs` is set on the calling process.
#[cfg(target_os = "linux")]
pub fn no_new_privs() -> bool {
    rustix::thread::no_new_privs().unwrap_or(false)
}

#[cfg(not(target_os = "linux"))]
pub fn set_no_new_privs() -> std::io::Result<()> {
    Err(std::io::Error::other("no_new_privs is Linux only"))
}

#[cfg(not(target_os = "linux"))]
pub fn no_new_privs() -> bool {
    false
}

/// Creates `path` and any missing parent directory with mode 0700 from the
/// start (REQ-WS-001): the mode is given to `mkdir`, so there is no moment
/// in which the directory is more open than the owner. A directory that
/// already existed with another mode is brought to 0700 (that is the only
/// case in which a mode is applied afterwards).
pub fn create_private_dir(path: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    let existed = path.is_dir();
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)?;
    if existed && std::fs::metadata(path)?.permissions().mode() & 0o777 != 0o700 {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// REQ-SEC-016 (014c, item 1): the worker leads a session of its own, so that the daemon can
    /// tell its tree from its clients by the session; the session is a group of its own too, so
    /// that the kill of the group still reaches it.
    #[test]
    fn req_sec_016_a_spawned_worker_leads_a_session_and_a_group_of_its_own() {
        let mut cmd = Command::new("sleep");
        cmd.arg("30");
        let (mut child, mut group) = spawn_in_group(cmd).expect("sleep starts");
        let pid = Pid::from_child(&child);
        let ours = rustix::process::getsid(None).unwrap();
        let theirs = rustix::process::getsid(Some(pid)).unwrap();
        assert_eq!(theirs, pid, "the child leads its own session");
        assert_ne!(theirs, ours);
        assert_eq!(
            rustix::process::getpgid(Some(pid)).unwrap(),
            pid,
            "and its own group"
        );
        group.kill();
        let _ = child.wait();
    }
}
