//! POSIX tree primitive: the worker leads its own process group, which is
//! killed as a whole (clause K). No foreign-function code: `Command::process_group`
//! and `rustix` are safe interfaces.

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

/// Spawns the command as the leader of a new process group.
pub fn spawn_in_group(mut cmd: Command) -> Result<(Child, Group), ConfineError> {
    use std::os::unix::process::CommandExt;
    cmd.process_group(0);
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
