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

/// A directory's name as the record keeps it and the commands show it:
/// canonical where the directory exists (one spelling for the short and
/// the long forms of the same directory on Windows), without the verbatim
/// prefix. A path that cannot be canonicalized is kept as given.
pub fn normalized(path: &Path) -> String {
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let text = canonical.to_string_lossy();
    text.strip_prefix(r"\\?\UNC\")
        .map(|rest| format!(r"\\{rest}"))
        .unwrap_or_else(|| text.strip_prefix(r"\\?\").unwrap_or(&text).to_owned())
}

/// Whether two names denote one directory: compared normalized, and without
/// regard to case on Windows, where the filesystem has none.
pub fn same_path(a: &str, b: &str) -> bool {
    let (a, b) = (normalized(Path::new(a)), normalized(Path::new(b)));
    if cfg!(windows) {
        a.eq_ignore_ascii_case(&b)
    } else {
        a == b
    }
}

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
        let text = normalized(&path);
        if !record.entries.iter().any(|e| same_path(&e.path, &text)) {
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
    let text = normalized(path);
    if !container
        .record
        .entries
        .iter()
        .any(|e| same_path(&e.path, &text))
    {
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
    let text = normalized(path);
    container
        .record
        .entries
        .retain(|e| !same_path(&e.path, &text));
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

/// Whether a declared read path (`read_paths`, `toolchain grant`) may join
/// the read set (REQ-CONF-011, LIM-H5): `None` when it may, else the reason,
/// naming the path as written, what was found and the narrower path to
/// declare. The one decision is `mimlys_core::host_tool::check_read_path`
/// (pure); this lists the directory's entries (depth one) and resolves the
/// path and the home directory the way the commands show them.
pub fn read_path_refusal(written: &str) -> Option<String> {
    let home = directories::BaseDirs::new().map(|b| b.home_dir().to_path_buf());
    read_path_refusal_with(written, home.as_deref())
}

/// As [`read_path_refusal`], with the home directory given (tests).
pub fn read_path_refusal_with(written: &str, home: Option<&Path>) -> Option<String> {
    let path = PathBuf::from(normalized(Path::new(&crate::config::expand_home(written))));
    let home = home.map(|h| PathBuf::from(normalized(h)));
    let entries: Vec<String> = std::fs::read_dir(&path)
        .map(|dir| {
            dir.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    mimlys_core::host_tool::check_read_path(
        &path,
        home.as_deref(),
        &entries,
        cfg!(any(windows, target_os = "macos")),
    )
    .err()
    .map(|refusal| refusal.reason(written))
}

#[cfg_attr(not(windows), allow(dead_code))]
fn now_text() -> String {
    mimlys_core::clock::Clock::now(&mimlys_core::clock::SystemClock).to_rfc3339()
}

#[cfg(test)]
mod tests;
