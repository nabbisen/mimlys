//! The directory permissions host tools need on Windows (RFC 011 §3, §7;
//! REQ-CONF-008), kept reversible: the worker runs in an AppContainer whose
//! SID must be granted read access to the toolchain paths and full access
//! to the workspaces and the temporary directory. mimlys applies the
//! entries for its own directories at startup and the user applies the
//! declared toolchain paths with `mimlys toolchain grant`; every entry is
//! recorded under the data directory (`toolchain-acls.json`) so that
//! `revoke` and an uninstall leave the directories as they were. Whether an
//! entry is in force is read back from the directory, never from the record.
//! On Linux and macOS nothing is applied: the kernel takes the paths from
//! the specification.

use crate::app::AppError;
use std::path::{Path, PathBuf};

/// What an entry grants (the exec crate's vocabulary, serialized the same).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Access {
    /// Read and execute.
    Read,
    /// Full access.
    Write,
}

/// Who asked for an entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// mimlys's own directories, applied at startup.
    Runtime,
    /// A declared toolchain path, applied by `toolchain grant`.
    User,
}

/// One recorded entry.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Entry {
    /// The directory.
    pub path: String,
    /// What was granted.
    pub access: Access,
    /// Who asked.
    pub source: Source,
    /// When (RFC 3339).
    pub applied_at: String,
}

/// The record file.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Record {
    /// The AppContainer profile name.
    pub profile: String,
    /// The container SID as text.
    pub sid: String,
    /// The entries applied.
    #[serde(default)]
    pub entries: Vec<Entry>,
}

#[cfg_attr(not(windows), allow(dead_code))]
const RECORD_FILE: &str = "toolchain-acls.json";

#[cfg_attr(not(windows), allow(dead_code))]
impl Record {
    fn load(data_dir: &Path) -> Self {
        std::fs::read_to_string(data_dir.join(RECORD_FILE))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn save(&self, data_dir: &Path) -> Result<(), AppError> {
        let text = serde_json::to_string_pretty(self).expect("serializes");
        std::fs::write(data_dir.join(RECORD_FILE), text).map_err(|e| AppError::Io {
            detail: format!("cannot write {}: {}", RECORD_FILE, e.kind()),
        })
    }
}

/// The installation's container and its record, on Windows; absent elsewhere.
#[derive(Clone, Debug)]
pub struct Container {
    /// The profile name (`mimlys-<hash>`).
    pub profile: String,
    /// The container SID.
    pub sid: Vec<u8>,
    /// The SID as text.
    pub sid_text: String,
    /// The entries on record.
    pub record: Record,
}

/// The directories mimlys itself needs the container to reach.
pub struct RuntimePaths<'a> {
    /// `<data>/workspaces`: full access.
    pub workspaces: &'a Path,
    /// `<data>/tmp`: full access.
    pub tmp: &'a Path,
    /// `<data>/interpreters` and the configured modules' directories: read.
    pub read: Vec<PathBuf>,
}

/// Prepares the container on Windows: the profile exists, mimlys's own
/// directories carry their entries (recorded as `runtime`), and the record
/// is loaded. Returns `None` on other platforms.
#[cfg(windows)]
pub fn prepare(data_dir: &Path, paths: &RuntimePaths<'_>) -> Result<Option<Container>, AppError> {
    use mimlys_exec::confine::windows::{acl, ensure_profile, profile_name, sid_string};
    let profile = profile_name(data_dir);
    let sid = ensure_profile(&profile).map_err(|e| AppError::Io {
        detail: format!("cannot prepare the AppContainer {profile}: {e}"),
    })?;
    let mut record = Record::load(data_dir);
    record.profile = profile.clone();
    record.sid = sid_string(&sid);
    let mut wanted: Vec<(PathBuf, Access)> = vec![
        (paths.workspaces.to_path_buf(), Access::Write),
        (paths.tmp.to_path_buf(), Access::Write),
    ];
    wanted.extend(paths.read.iter().map(|p| (p.clone(), Access::Read)));
    let mut changed = false;
    for (path, access) in wanted {
        if !path.exists() {
            continue;
        }
        let applied = acl::has_entry(&path, &sid).map_err(|e| AppError::Io { detail: e })?;
        if !applied {
            let mode = match access {
                Access::Read => acl::Access::Read,
                Access::Write => acl::Access::Write,
            };
            acl::grant(&path, &sid, mode).map_err(|e| AppError::Io { detail: e })?;
        }
        let text = path.to_string_lossy().into_owned();
        if !record.entries.iter().any(|e| e.path == text) {
            record.entries.push(Entry {
                path: text,
                access,
                source: Source::Runtime,
                applied_at: now_text(),
            });
            changed = true;
        }
    }
    if changed || !data_dir.join(RECORD_FILE).exists() {
        record.save(data_dir)?;
    }
    Ok(Some(Container {
        profile,
        sid_text: record.sid.clone(),
        sid,
        record,
    }))
}

/// Nothing to prepare: Landlock and Seatbelt take the paths from the specification.
#[cfg(not(windows))]
pub fn prepare(_data_dir: &Path, _paths: &RuntimePaths<'_>) -> Result<Option<Container>, AppError> {
    Ok(None)
}

/// Whether the directory carries the container's entry now (read back from
/// the directory). `None` where no entry is needed (not Windows).
#[cfg(windows)]
pub fn applied(container: &Container, path: &Path) -> Option<bool> {
    mimlys_exec::confine::windows::acl::has_entry(path, &container.sid).ok()
}

/// No entry is needed on this platform.
#[cfg(not(windows))]
pub fn applied(_container: &Container, _path: &Path) -> Option<bool> {
    None
}

/// Grants the container read access to a toolchain path and records it (`toolchain grant`).
#[cfg(windows)]
pub fn grant(container: &mut Container, data_dir: &Path, path: &Path) -> Result<(), AppError> {
    use mimlys_exec::confine::windows::acl;
    acl::grant(path, &container.sid, acl::Access::Read).map_err(|e| AppError::Io { detail: e })?;
    let text = path.to_string_lossy().into_owned();
    if !container.record.entries.iter().any(|e| e.path == text) {
        container.record.entries.push(Entry {
            path: text,
            access: Access::Read,
            source: Source::User,
            applied_at: now_text(),
        });
    }
    container.record.save(data_dir)
}

/// Removes the container's entries from a path and the record (`toolchain revoke`).
#[cfg(windows)]
pub fn revoke(container: &mut Container, data_dir: &Path, path: &Path) -> Result<(), AppError> {
    use mimlys_exec::confine::windows::acl;
    acl::revoke(path, &container.sid).map_err(|e| AppError::Io { detail: e })?;
    let text = path.to_string_lossy().into_owned();
    container.record.entries.retain(|e| e.path != text);
    container.record.save(data_dir)
}

/// Nothing to apply on this platform.
#[cfg(not(windows))]
pub fn grant(_container: &mut Container, _data_dir: &Path, _path: &Path) -> Result<(), AppError> {
    Ok(())
}

/// Nothing to remove on this platform.
#[cfg(not(windows))]
pub fn revoke(_container: &mut Container, _data_dir: &Path, _path: &Path) -> Result<(), AppError> {
    Ok(())
}

/// The reason a declared host tool is unavailable on this machine, when a
/// declared read path lacks its entry: `acl_not_applied: <path>`.
pub fn unavailable_reason(
    container: Option<&Container>,
    declaration: &mimlys_core::host_tool::HostToolDeclaration,
) -> Option<String> {
    let container = container?;
    declaration
        .read_paths
        .iter()
        .find(|p| applied(container, Path::new(p)) == Some(false))
        .map(|p| format!("acl_not_applied: {p}"))
}

#[cfg_attr(not(windows), allow(dead_code))]
fn now_text() -> String {
    mimlys_core::clock::Clock::now(&mimlys_core::clock::SystemClock).to_rfc3339()
}
