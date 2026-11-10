//! Which filesystem holds a path (REQ-WS-013, external design §14.6): the
//! name the platform gives it (`btrfs`, `apfs`, `NTFS` …), read without
//! writing anything. Linux reads `/proc/self/mountinfo`, macOS the output of
//! `mount`, Windows asks the volume. The parsers are plain functions over
//! text so that they are tested on every platform.

use std::path::{Path, PathBuf};

/// The name of the filesystem that holds `path`, lowercase, when the platform
/// can tell. The nearest existing ancestor is asked when `path` does not exist.
pub fn filesystem_name(path: &Path) -> Option<String> {
    let existing = nearest_existing(path)?;
    let canonical = std::fs::canonicalize(&existing).ok()?;
    platform(&canonical)
}

fn nearest_existing(path: &Path) -> Option<PathBuf> {
    let mut p = path.to_path_buf();
    loop {
        if p.exists() {
            return Some(p);
        }
        if !p.pop() {
            return None;
        }
    }
}

#[cfg(target_os = "linux")]
fn platform(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string("/proc/self/mountinfo").ok()?;
    from_mountinfo(&text, path)
}

#[cfg(target_os = "macos")]
fn platform(path: &Path) -> Option<String> {
    let out = std::process::Command::new("/sbin/mount").output().ok()?;
    from_mount_output(&String::from_utf8_lossy(&out.stdout), path)
}

#[cfg(windows)]
fn platform(path: &Path) -> Option<String> {
    crate::confine::windows::volume_filesystem(path).map(|n| n.to_lowercase())
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn platform(_path: &Path) -> Option<String> {
    None
}

/// The filesystem type of the mount that holds `path`, from the text of
/// `/proc/self/mountinfo`: the mount whose mount point is the longest prefix
/// of `path`, by path components. A later line wins a tie (a mount over a mount).
pub fn from_mountinfo(text: &str, path: &Path) -> Option<String> {
    let mut best: Option<(usize, String)> = None;
    for line in text.lines() {
        let Some((before, after)) = line.split_once(" - ") else {
            continue;
        };
        let (Some(mount_point), Some(fstype)) = (
            before.split_whitespace().nth(4).map(unescape_octal),
            after.split_whitespace().next(),
        ) else {
            continue;
        };
        if path.starts_with(Path::new(&mount_point)) {
            let depth = Path::new(&mount_point).components().count();
            if best.as_ref().is_none_or(|(d, _)| depth >= *d) {
                best = Some((depth, fstype.to_lowercase()));
            }
        }
    }
    best.map(|(_, name)| name)
}

/// `\040` and the like, as the kernel writes spaces and tabs in a mount point.
fn unescape_octal(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\'
            && i + 3 < bytes.len()
            && bytes[i + 1..i + 4]
                .iter()
                .all(|b| (b'0'..=b'7').contains(b))
        {
            let value =
                (bytes[i + 1] - b'0') * 64 + (bytes[i + 2] - b'0') * 8 + (bytes[i + 3] - b'0');
            out.push(value);
            i += 4;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The filesystem type of the mount that holds `path`, from the output of
/// macOS `mount` (`/dev/disk3s5 on /System/Volumes/Data (apfs, local, …)`).
pub fn from_mount_output(text: &str, path: &Path) -> Option<String> {
    let mut best: Option<(usize, String)> = None;
    for line in text.lines() {
        let Some((_, rest)) = line.split_once(" on ") else {
            continue;
        };
        let Some((mount_point, options)) = rest.rsplit_once(" (") else {
            continue;
        };
        let Some(fstype) = options
            .split([',', ')'])
            .next()
            .map(|t| t.trim().to_lowercase())
        else {
            continue;
        };
        if path.starts_with(Path::new(mount_point)) {
            let depth = Path::new(mount_point).components().count();
            if best.as_ref().is_none_or(|(d, _)| depth >= *d) {
                best = Some((depth, fstype));
            }
        }
    }
    best.map(|(_, name)| name)
}

#[cfg(test)]
mod tests;
