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

/// A failed write of a directory's permissions as the command's error: a
/// refused lock is the typed `AppError::Permissions` (kind `unavailable`,
/// retrying is the remedy), any other failure keeps its text.
#[cfg(windows)]
fn acl_error(e: mimlys_exec::confine::windows::acl::AclError) -> AppError {
    use mimlys_exec::confine::windows::acl::AclError;
    match e {
        AclError::Refused(refusal) => AppError::Permissions(refusal),
        AclError::Failed(detail) => AppError::Io { detail },
    }
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
            acl::grant(&path, &sid, mode).map_err(acl_error)?;
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

/// The container as `toolchain list` and `status` show it, for a command that
/// only reads (REQ-PERS-013): the profile's name, its SID derived from the name
/// (the profile is not created) and the record file as it is. No directory's
/// permissions are read or changed here; `applied` reads them back one by one.
/// Returns `None` on other platforms.
#[cfg(windows)]
pub fn inspect(data_dir: &Path) -> Option<Container> {
    use mimlys_exec::confine::windows::{derive_sid, profile_name, sid_string};
    let profile = profile_name(data_dir);
    let sid = derive_sid(&profile).ok()?;
    let mut record = Record::load(data_dir);
    record.profile = profile.clone();
    record.sid = sid_string(&sid);
    Some(Container {
        profile,
        sid_text: record.sid.clone(),
        sid,
        record,
    })
}

/// Nothing to inspect: Landlock and Seatbelt take the paths from the specification.
#[cfg(not(windows))]
pub fn inspect(_data_dir: &Path) -> Option<Container> {
    None
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
    acl::grant(path, &container.sid, acl::Access::Read).map_err(acl_error)?;
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
    acl::revoke(path, &container.sid).map_err(acl_error)?;
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

/// How an entry reached the list of `toolchain revoke --all`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FoundBy {
    /// It is in the record file.
    Record,
    /// It is not in the record; the scan found it in the directory.
    Scan,
}

/// One entry to remove.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Planned {
    /// The directory.
    pub path: String,
    /// Whose entry it is.
    pub source: Source,
    /// Where the list got it from.
    pub found_by: FoundBy,
}

impl Planned {
    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({ "path": self.path, "source": self.source, "found_by": self.found_by })
    }

    /// The words of the plan lines.
    pub fn words(&self) -> (&'static str, &'static str) {
        (
            match self.source {
                Source::Runtime => "runtime",
                Source::User => "user",
            },
            match self.found_by {
                FoundBy::Record => "recorded",
                FoundBy::Scan => "found by scan",
            },
        )
    }
}

/// The list of `toolchain revoke --all` (REQ-CONF-008): every entry of the
/// record, and every candidate directory that carries the container's entry
/// now (`in_force`, read from the directory) although the record does not
/// list it. The candidates are a bounded set named by the caller, never a
/// walk of the disk. A directory appears once.
pub fn plan_revoke_all(
    record: &Record,
    candidates: &[(String, Source)],
    in_force: impl Fn(&str) -> bool,
) -> Vec<Planned> {
    let mut plan: Vec<Planned> = Vec::new();
    let listed = |plan: &[Planned], path: &str| plan.iter().any(|p| same_path(&p.path, path));
    for e in &record.entries {
        if std::path::Path::new(&e.path).is_file() {
            continue;
        }
        if !listed(&plan, &e.path) {
            plan.push(Planned {
                path: e.path.clone(),
                source: e.source,
                found_by: FoundBy::Record,
            });
        }
    }
    for (path, source) in candidates {
        if std::path::Path::new(path).is_file() {
            continue;
        }
        if !listed(&plan, path) && in_force(path) {
            plan.push(Planned {
                path: path.clone(),
                source: *source,
                found_by: FoundBy::Scan,
            });
        }
    }
    plan
}

/// What a run of `toolchain revoke --all` did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    /// Removed (a directory that no longer exists counts: nothing is left to undo).
    pub removed: Vec<Planned>,
    /// Not removed, with the reason.
    pub failed: Vec<(Planned, String)>,
    /// The container profile was deleted.
    pub profile_deleted: bool,
    /// The record file was deleted.
    pub record_deleted: bool,
}

impl Outcome {
    /// Everything on the list was removed.
    pub fn complete(&self) -> bool {
        self.failed.is_empty()
    }

    /// The documented JSON shape.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "removed": self.removed.iter().map(Planned::to_json).collect::<Vec<_>>(),
            "failed": self.failed.iter().map(|(p, reason)| serde_json::json!({ "path": p.path, "reason": reason })).collect::<Vec<_>>(),
            "profile_deleted": self.profile_deleted,
            "record_deleted": self.record_deleted,
        })
    }
}

/// The record after a run: exactly what was not removed, so that a second
/// run retries exactly that. `None` when nothing is left.
pub fn record_after(record: &Record, outcome: &Outcome) -> Option<Record> {
    if outcome.failed.is_empty() {
        return None;
    }
    let mut kept = Record {
        profile: record.profile.clone(),
        sid: record.sid.clone(),
        entries: Vec::new(),
    };
    for (planned, _) in &outcome.failed {
        let entry = record
            .entries
            .iter()
            .find(|e| same_path(&e.path, &planned.path))
            .cloned()
            .unwrap_or_else(|| Entry {
                path: planned.path.clone(),
                access: Access::Read,
                source: planned.source,
                applied_at: now_text(),
            });
        kept.entries.push(entry);
    }
    Some(kept)
}

/// Removes every entry of the plan, then the container profile, then the
/// record file (Windows). A failure of one entry does not stop the others;
/// the record is rewritten to hold what failed, and the profile stays while
/// anything failed.
#[cfg(windows)]
pub fn revoke_all(container: &Container, data_dir: &Path, plan: &[Planned]) -> Outcome {
    use mimlys_exec::confine::windows::{acl, delete_profile};
    let mut outcome = Outcome::default();
    for planned in plan {
        let path = Path::new(&planned.path);
        if !path.exists() {
            outcome.removed.push(planned.clone());
            continue;
        }
        match acl::revoke(path, &container.sid) {
            Ok(()) => outcome.removed.push(planned.clone()),
            Err(reason) => outcome.failed.push((planned.clone(), reason.to_string())),
        }
    }
    match record_after(&container.record, &outcome) {
        Some(kept) => {
            let _ = kept.save(data_dir);
        }
        None => {
            outcome.profile_deleted = delete_profile(&container.profile).is_ok();
            outcome.record_deleted = match std::fs::remove_file(data_dir.join(RECORD_FILE)) {
                Ok(()) => true,
                Err(e) => e.kind() == std::io::ErrorKind::NotFound,
            };
        }
    }
    outcome
}

/// Nothing to remove on this platform (the command says so before it gets here).
#[cfg(not(windows))]
pub fn revoke_all(_container: &Container, _data_dir: &Path, _plan: &[Planned]) -> Outcome {
    Outcome::default()
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

/// Whether a host tool's `command` may join the read set (REQ-CONF-011,
/// external design §13.5): `None` when its canonical target, after its
/// links, lies under the fixed read paths or the tool's declared ones (or
/// when the command cannot be resolved here: the run says so then), else the
/// reason, naming the resolved path and the directory to declare. A
/// confinement matches the resolved path (Seatbelt, Landlock): a command that
/// links out of the read paths (Homebrew's `rustup` links into
/// `/opt/homebrew/Cellar/…`) would fail later with a bare permission error.
/// The same on every platform.
pub fn command_refusal(
    declaration: &mimlys_core::host_tool::HostToolDeclaration,
    fixed: &[PathBuf],
) -> Option<String> {
    let program = mimlys_exec::host_tool::resolve_command(declaration).ok()?;
    let target = std::fs::canonicalize(&program).ok()?;
    let allowed: Vec<String> = fixed
        .iter()
        .map(PathBuf::as_path)
        .chain(declaration.read_paths.iter().map(Path::new))
        .map(normalized)
        .collect();
    command_leaves_read_paths(&program, &target, &allowed)
}

/// The decision of [`command_refusal`] (pure): `target` is the command's
/// canonical target and `allowed` the read paths, both normalized.
pub fn command_leaves_read_paths(
    program: &Path,
    target: &Path,
    allowed: &[String],
) -> Option<String> {
    let fold = |text: String| {
        if cfg!(windows) {
            text.to_ascii_lowercase()
        } else {
            text
        }
    };
    let target_text = normalized(target);
    let inside = allowed
        .iter()
        .any(|a| Path::new(&fold(target_text.clone())).starts_with(fold(a.clone())));
    if inside {
        return None;
    }
    let directory = target.parent().unwrap_or(target);
    Some(format!(
        "the command {} resolves to {}, which is outside the read paths; declare {} in read_paths",
        program.display(),
        target_text,
        normalized(directory)
    ))
}

#[cfg_attr(not(windows), allow(dead_code))]
fn now_text() -> String {
    mimlys_core::clock::Clock::now(&mimlys_core::clock::SystemClock).to_rfc3339()
}

#[cfg(test)]
mod tests;
