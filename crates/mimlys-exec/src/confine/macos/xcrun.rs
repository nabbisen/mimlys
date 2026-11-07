//! `xcrun`'s cache, made by the runtime (RFC 011 Amendment 2, implementation
//! decisions, point 8; external design §13.3; LIM-O8).
//!
//! With an Xcode selected, `xcrun` resolves the developer tools through a
//! cache file, `xcrun_db`, in the per-user temporary directory. The profile
//! admits it read-only. Where it is absent (a Mac where `xcrun` never ran), a
//! sandboxed `xcrun` cannot create it and falls back to `xcodebuild`, which
//! fails (measured by 011c-5): the build fails. So the runtime creates it
//! itself, **outside the sandbox**, before it starts a worker: it runs
//! `/usr/bin/xcrun --find clang` with a cleared environment and waits at most
//! LIM-O8. (`--find cc`, which the handoff named, is not enough: the `cc`
//! shim asks for `clang`, and the cache is keyed by the tool asked for and by
//! the environment of the asker, so a cache made for `cc`, or by a shell with
//! its own environment, leaves the sandboxed `cc` to fail. Measured by 011c-6:
//! the demonstration completes with `env -i … xcrun --find clang`, and halts
//! with `--find cc`, and with a longer list of queries made under a runner's
//! environment.)
//!
//! Never a write rule for the cache in the profile: the user's own unconfined
//! `xcrun` trusts the file later, and a tool that could write it could choose
//! what that `xcrun` runs.
//!
//! The decisions are pure or use only the standard library, so they are
//! tested on every platform with a stand-in for `xcrun`; the macOS facts (the
//! developer link, the per-user temporary directory) are read in
//! `macos.rs`.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The cache file's name.
pub const FILE: &str = "xcrun_db";

/// Whether the cache is there, and whether it is needed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CacheState {
    /// No Xcode is selected (the Command Line Tools work without the cache),
    /// or the directories are not known.
    NotNeeded,
    /// An Xcode is selected and the file exists.
    Present,
    /// An Xcode is selected and the file does not exist.
    Missing {
        /// The file `xcrun` would create.
        file: PathBuf,
    },
}

/// The cache file an Xcode needs: `Some` only when the active developer
/// directory is an Xcode's (`…/Contents/Developer`, an absolute path) and the
/// per-user temporary directory is known and absolute. Text, not the host's
/// path rules (the function is tested on every platform).
pub fn cache_file(developer_dir: Option<&Path>, user_temp: Option<&Path>) -> Option<PathBuf> {
    let developer = developer_dir?.to_str()?.trim_end_matches('/');
    if !developer.starts_with('/') {
        return None;
    }
    developer
        .strip_suffix("/Contents/Developer")
        .filter(|app| !app.is_empty())?;
    let temp = user_temp?.to_str()?;
    if !temp.starts_with('/') {
        return None;
    }
    Some(PathBuf::from(format!(
        "{}/{FILE}",
        temp.trim_end_matches('/')
    )))
}

/// The state for a cache file that is or is not needed.
pub fn state_of(file: Option<PathBuf>) -> CacheState {
    match file {
        None => CacheState::NotNeeded,
        Some(file) if file.exists() => CacheState::Present,
        Some(file) => CacheState::Missing { file },
    }
}

/// Runs `xcrun --find clang` (outside any sandbox: the caller is the runtime)
/// and waits at most `wait`; a child still running then is killed and
/// reaped. Returns whether `file` exists afterwards. The environment is not
/// inherited (the runtime's holds `env:`-referenced secrets and `xcrun` needs
/// none of it).
pub fn warm(file: &Path, xcrun: &Path, wait: Duration) -> bool {
    let started = Instant::now();
    let spawned = std::process::Command::new(xcrun)
        .args(["--find", "clang"])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    let Ok(mut child) = spawned else {
        return file.exists();
    };
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() < wait => {
                std::thread::sleep(Duration::from_millis(20));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break;
            }
        }
    }
    file.exists()
}

/// Makes the cache if it is needed and absent, and says what is true
/// afterwards. `xcrun` and `wait` are the caller's (the macOS wrapper gives
/// the system's and LIM-O8).
pub fn ensure(file: Option<PathBuf>, xcrun: &Path, wait: Duration) -> CacheState {
    match state_of(file) {
        CacheState::Missing { file } => {
            warm(&file, xcrun, wait);
            state_of(Some(file))
        }
        other => other,
    }
}

#[cfg(test)]
mod tests;
