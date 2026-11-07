//! The macOS backend (RFC 011 Amendment 2, points 1 to 5; REQ-CONF-005):
//! one Seatbelt profile, generated from the specification, applied by the
//! worker to itself before its first frame and before any thread exists;
//! every host tool inherits it (a sandboxed process can apply no second
//! profile, to itself or to a child: measured by spike S1). The worker
//! reports from what it observes in itself, never from what it intended.
//!
//! Two halves, so that the first can be tested on every platform:
//!
//! - the **pure half** (compiled everywhere): [`canonical_spec`], [`generate`]
//!   (the profile text), [`escape`], [`denied_candidates`] (where the
//!   self-check looks) and [`report`] (what each outcome means for each
//!   clause). The unit tests of `macos/tests.rs` run in the Linux gate.
//! - the **platform half** (macOS only): [`apply`], which resolves
//!   `sandbox_init` at run time (`dlsym`, so that a macOS that removes it
//!   reports `seatbelt_missing` instead of failing to link), applies the
//!   profile and checks itself (a read the profile denies must fail, a
//!   loopback connection must be refused, the parent's `kern.procargs`
//!   must be unreadable), else reports `seatbelt_ineffective`. The foreign
//!   calls are in `macos/sys.rs` (REQ-CONF-010).
//!
//! Rule order matters: the last matching rule wins in SBPL, so every `deny`
//! that narrows an earlier `allow` comes after it.
//!
//! Not here (they wait for spike S1b, handoff 011c): K by sandbox
//! membership, M and C over the members, the measured part of the read set
//! and the dependency step's profile (Amendment 2 points 6 to 9). K, M and C
//! are reported `unavailable` and say which point they wait for.

#[cfg(target_os = "macos")]
use super::Applied;
use super::ConfinementSpec;
use mimlys_core::confine::ClauseState;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[cfg(target_os = "macos")]
mod sys;

#[cfg(all(target_os = "macos", feature = "probe-syscalls"))]
pub mod probe;

/// Why K reports `unavailable` for now (Amendment 2 point 6).
pub const K_LATER: &str = "arrives with point 6 of RFC 011 Amendment 2 (M6c)";
/// Why M and C report `unavailable` for now (Amendment 2 point 7).
pub const MC_LATER: &str = "arrives with point 7 of RFC 011 Amendment 2 (M6c)";
/// Why N reports `unavailable` for the dependency step's worker (point 9).
pub const FETCH_LATER: &str = "network allowed for the dependency step: its profile arrives with point 9 of RFC 011 Amendment 2 (M6c)";

/// The link `xcode-select` keeps to the active developer directory; read
/// without running a command (Amendment 2 point 8).
pub const XCODE_SELECT_LINK: &str = "/private/var/db/xcode_select_link";
/// Where the Command Line Tools live when no Xcode is selected.
pub const COMMAND_LINE_TOOLS: &str = "/Library/Developer/CommandLineTools";

/// The system part of the read set that is the same on every Mac
/// (Amendment 2 point 8): the dyld cache lives under `/System`
/// (`/System/Volumes/Preboot/Cryptexes`), `/etc` is `/private/etc`.
const SYSTEM_READ: [&str; 8] = [
    "/usr",
    "/bin",
    "/sbin",
    "/System",
    "/private/etc",
    "/dev/urandom",
    "/dev/random",
    "/dev/zero",
];

/// Why a profile cannot be generated from a specification. Every variant
/// refuses to guess: a profile that is not exactly what was asked for is
/// worse than none.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProfileError {
    /// A path in a set is not absolute.
    #[error("path is not absolute: {0}")]
    NotAbsolute(String),
    /// A path is not valid UTF-8 (the profile is text).
    #[error("path is not valid UTF-8: {0}")]
    NotUtf8(String),
    /// A path holds a control character (a newline or a NUL would end the string literal).
    #[error("path holds a control character: {0}")]
    ControlCharacter(String),
    /// A set holds the filesystem root, which would admit every file.
    #[error("a path set holds the filesystem root")]
    Root,
}

/// The system part of the read set (Amendment 2 point 8): the fixed
/// directories and devices, and the active developer directory, read from
/// the `xcode_select` link, else the Command Line Tools when they exist.
pub fn system_read_paths() -> Vec<PathBuf> {
    system_read_paths_from(
        std::fs::read_link(XCODE_SELECT_LINK).ok(),
        Path::new(COMMAND_LINE_TOOLS).is_dir(),
    )
}

/// [`system_read_paths`] from what it depends on (tested on every platform):
/// the target of the `xcode_select` link, when it is an absolute path, else
/// the Command Line Tools directory when it exists.
pub fn system_read_paths_from(
    developer_link: Option<PathBuf>,
    tools_present: bool,
) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = SYSTEM_READ.iter().map(PathBuf::from).collect();
    match developer_link.filter(|p| is_posix_absolute(p)) {
        Some(dir) => paths.push(dir),
        None if tools_present => paths.push(PathBuf::from(COMMAND_LINE_TOOLS)),
        None => {}
    }
    paths
}

/// Whether a path is absolute as macOS spells it. Text, not the host's rule:
/// the generator is compiled everywhere and its tests run on every platform,
/// where `/usr` is not an absolute path to Windows.
fn is_posix_absolute(path: &Path) -> bool {
    path.to_str().is_some_and(|s| s.starts_with('/'))
}

/// A path with one spelling: symbolic links resolved, as Seatbelt matches
/// resolved paths (`/tmp`, `/var` and `/etc` are links into `/private`).
/// A path that does not exist yet is resolved at its longest existing
/// ancestor and keeps its remaining components; a relative path stays as
/// it is (the generator refuses it).
pub fn canonical(path: &Path) -> PathBuf {
    if let Ok(resolved) = path.canonicalize() {
        return resolved;
    }
    let mut rest: Vec<&std::ffi::OsStr> = Vec::new();
    let mut current = path;
    while let (Some(name), Some(parent)) = (current.file_name(), current.parent()) {
        rest.push(name);
        if let Ok(mut resolved) = parent.canonicalize() {
            resolved.extend(rest.iter().rev());
            return resolved;
        }
        current = parent;
    }
    path.to_path_buf()
}

/// The specification with every path canonical ([`canonical`]), sorted and
/// without duplicates.
pub fn canonical_spec(spec: &ConfinementSpec) -> ConfinementSpec {
    let canon = |paths: &[PathBuf]| -> Vec<PathBuf> {
        paths
            .iter()
            .map(|p| canonical(p))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    };
    ConfinementSpec {
        read_paths: canon(&spec.read_paths),
        write_paths: canon(&spec.write_paths),
        data_dir: if spec.data_dir.as_os_str().is_empty() {
            PathBuf::new()
        } else {
            canonical(&spec.data_dir)
        },
        ..spec.clone()
    }
}

/// An SBPL string literal's contents: `\` and `"` are escaped. (A control
/// character cannot be escaped here; [`generate`] refuses it.)
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c == '\\' || c == '"' {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// A path set as escaped literals, sorted, without duplicates; each path is
/// checked (absolute, UTF-8, no control character, not the root).
fn literals(paths: &[PathBuf]) -> Result<Vec<String>, ProfileError> {
    let mut out = BTreeSet::new();
    for path in paths {
        let shown = path.to_string_lossy().into_owned();
        let text = path
            .to_str()
            .ok_or_else(|| ProfileError::NotUtf8(shown.clone()))?;
        if !is_posix_absolute(path) {
            return Err(ProfileError::NotAbsolute(shown));
        }
        if text.chars().any(char::is_control) {
            return Err(ProfileError::ControlCharacter(shown));
        }
        if text.trim_end_matches('/').is_empty() {
            return Err(ProfileError::Root);
        }
        out.insert(escape(text.trim_end_matches('/')));
    }
    Ok(out.into_iter().collect())
}

/// ` (subpath "a") (subpath "b")`: the filters of one rule. Never empty
/// where it is used: an `allow` with no filter allows everything, so a rule
/// whose set is empty is not written at all.
fn subpaths(literals: &[String]) -> String {
    literals
        .iter()
        .map(|l| format!(" (subpath \"{l}\")"))
        .collect()
}

/// The profile text for a specification whose paths are already canonical
/// ([`canonical_spec`]); pure. The rules, in the order that makes the last
/// match the right one (Amendment 2 points 3 and 4):
///
/// 1. `(deny default)`;
/// 2. processes: `fork`, and `exec` of the read and write sets;
/// 3. files: metadata anywhere (as on Linux, where Landlock does not cover
///    it), the root directory itself (the loader reads it), the read set
///    readable, the write set readable and writable;
/// 4. `sysctl-read` allowed, then `kern.procargs*` denied (the environment of
///    another process);
/// 5. other processes: `signal` and `process-info*` only for the sandbox's
///    members (the tool's own tree);
/// 6. network: denied, then Unix sockets under the write set only. No
///    `mach-lookup` and no `mDNSResponder` socket anywhere: DNS is an
///    egress channel (measured). The dependency step's worker
///    (`deny_network` false) gets no network rule either until Amendment 2
///    point 9 is settled.
pub fn generate(spec: &ConfinementSpec) -> Result<String, ProfileError> {
    let reads = literals(&spec.read_paths)?;
    let writes = literals(&spec.write_paths)?;
    let executable: Vec<String> = reads
        .iter()
        .chain(writes.iter())
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();

    let mut p = String::new();
    p.push_str("(version 1)\n(deny default)\n\n");
    p.push_str(";; processes\n(allow process-fork)\n");
    if !executable.is_empty() {
        p.push_str(&format!("(allow process-exec*{})\n", subpaths(&executable)));
    }
    p.push_str("\n;; files\n(allow file-read-metadata)\n(allow file-read* (literal \"/\"))\n");
    if !reads.is_empty() {
        p.push_str(&format!("(allow file-read*{})\n", subpaths(&reads)));
    }
    if !writes.is_empty() {
        p.push_str(&format!(
            "(allow file-read*{w})\n(allow file-write*{w})\n",
            w = subpaths(&writes)
        ));
    }
    p.push_str(
        "\n;; kernel information, except the arguments and environment of other processes\n\
         (allow sysctl-read)\n\
         (deny sysctl-read (sysctl-name-prefix \"kern.procargs\"))\n",
    );
    p.push_str(
        "\n;; other processes: the sandbox's members only\n\
         (allow signal (target same-sandbox))\n\
         (deny process-info*)\n\
         (allow process-info* (target same-sandbox))\n",
    );
    p.push_str("\n;; network\n");
    if spec.deny_network {
        p.push_str("(deny network*)\n");
    } else {
        p.push_str(";; the dependency step's network rules: Amendment 2 point 9\n");
    }
    for w in &writes {
        p.push_str(&format!(
            "(allow network-bind (local unix-socket (subpath \"{w}\")))\n\
             (allow network-outbound (remote unix-socket (subpath \"{w}\")))\n\
             (allow network-inbound (local unix-socket (subpath \"{w}\")))\n"
        ));
    }
    Ok(p)
}

/// The profile for a specification as given: canonical paths, then [`generate`].
pub fn profile(spec: &ConfinementSpec) -> Result<String, ProfileError> {
    generate(&canonical_spec(spec))
}

/// Directories the self-check may try to read after the profile is applied,
/// in order: the data directory itself (the runtime's store lives there; only
/// its `interpreters`, `tmp` and `workspaces` are in the sets) and
/// directories every Mac has. Those covered by the read or write set are left
/// out (a path under a set path, or equal to it, is readable by design). The
/// caller keeps the ones it can read *before* applying: the control.
pub fn denied_candidates(spec: &ConfinementSpec) -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if !spec.data_dir.as_os_str().is_empty() {
        candidates.push(spec.data_dir.clone());
    }
    candidates.extend(
        ["/Applications", "/Library", "/Users", "/private/var/db"]
            .iter()
            .map(PathBuf::from),
    );
    candidates
        .into_iter()
        .filter(|c| {
            !spec
                .read_paths
                .iter()
                .chain(spec.write_paths.iter())
                .any(|set| c.starts_with(set))
        })
        .collect()
}

/// What applying the profile produced, as the worker observed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// `sandbox_init` could not be resolved at run time.
    Missing,
    /// The specification cannot be written as a profile.
    BadProfile(String),
    /// The system rejected the profile (its message, with a position) or refused to apply it.
    Rejected(String),
    /// The profile was applied; each check is `None` when it held and the
    /// observation when it did not.
    Applied {
        /// The self-check of the file and process-information rules failed.
        filesystem: Option<String>,
        /// The self-check of the network rule failed.
        network: Option<String>,
    },
}

/// What an outcome means for each clause (Amendment 2 points 2 to 5). F and
/// N are `enforced (seatbelt)` only when the profile was applied and the
/// self-check saw it hold; P is `enforced (seatbelt)` under the same
/// condition as F (the kernel refuses a setuid `execve` from a sandboxed
/// process; no rule is needed, and a clause test asserts it); K, M and C
/// wait for points 6 and 7.
pub fn report(deny_network: bool, outcome: &Outcome) -> BTreeMap<String, ClauseState> {
    let (f, n, p) = match outcome {
        Outcome::Missing => {
            let s = ClauseState::unavailable("seatbelt_missing");
            (s.clone(), s.clone(), s)
        }
        Outcome::BadProfile(why) => {
            let s = ClauseState::unavailable(&format!("seatbelt_profile: {why}"));
            (s.clone(), s.clone(), s)
        }
        Outcome::Rejected(why) => {
            let s = ClauseState::unavailable(&format!("seatbelt_rejected: {why}"));
            (s.clone(), s.clone(), s)
        }
        Outcome::Applied {
            filesystem,
            network,
        } => {
            let seatbelt = ClauseState::enforced("seatbelt");
            let f = match filesystem {
                None => seatbelt.clone(),
                Some(why) => ClauseState::unavailable(&format!("seatbelt_ineffective: {why}")),
            };
            let n = match (deny_network, network) {
                (false, _) => ClauseState::unavailable(FETCH_LATER),
                (true, None) => seatbelt.clone(),
                (true, Some(why)) => {
                    ClauseState::unavailable(&format!("seatbelt_ineffective: {why}"))
                }
            };
            let p = match filesystem {
                None => seatbelt,
                Some(why) => ClauseState::unavailable(&format!("seatbelt_ineffective: {why}")),
            };
            (f, n, p)
        }
    };
    BTreeMap::from([
        ("F".to_owned(), f),
        ("N".to_owned(), n),
        ("P".to_owned(), p),
        ("K".to_owned(), ClauseState::unavailable(K_LATER)),
        ("M".to_owned(), ClauseState::unavailable(MC_LATER)),
        ("C".to_owned(), ClauseState::unavailable(MC_LATER)),
    ])
}

/// Applies the backend to the calling process and reports what held
/// (Amendment 2 point 2). Called by the worker at start, before any thread
/// exists.
#[cfg(target_os = "macos")]
pub fn apply(spec: &ConfinementSpec) -> Applied {
    Applied {
        report: report(spec.deny_network, &apply_outcome(spec)),
        cgroup: None,
        tool_cgroup: None,
        rlimit_memory: false,
    }
}

/// The platform half: resolve, generate, check the controls, apply, check again.
#[cfg(target_os = "macos")]
fn apply_outcome(spec: &ConfinementSpec) -> Outcome {
    let Some(init) = sys::resolve_sandbox_init() else {
        return Outcome::Missing;
    };
    let canonical = canonical_spec(spec);
    let profile = match generate(&canonical) {
        Ok(text) => text,
        Err(e) => return Outcome::BadProfile(e.to_string()),
    };
    // The controls, before the profile: what can be read now is what the
    // profile must stop (a denial of something unreadable proves nothing).
    let readable: Vec<PathBuf> = denied_candidates(&canonical)
        .into_iter()
        .filter(|c| std::fs::read_dir(c).is_ok())
        .take(3)
        .collect();
    let parent = std::os::unix::process::parent_id() as i32;
    let procargs_control = sys::procargs2(parent).is_ok();

    if let Err(message) = sys::apply(init, &profile) {
        return Outcome::Rejected(message);
    }

    let mut filesystem = None;
    if readable.is_empty() {
        filesystem = Some("no readable directory outside the sets to check against".to_owned());
    }
    for dir in &readable {
        if std::fs::read_dir(dir).is_ok() {
            filesystem = Some(format!("{} is readable", dir.display()));
            break;
        }
    }
    if filesystem.is_none() && procargs_control && sys::procargs2(parent).is_ok() {
        filesystem = Some("another process's kern.procargs is readable".to_owned());
    }
    let network = spec.deny_network.then(loopback_connect_check).flatten();
    Outcome::Applied {
        filesystem,
        network,
    }
}

/// A connection to the loopback discard port must be refused by the profile
/// (`EPERM`), not by the network (`ECONNREFUSED`) nor allowed.
#[cfg(target_os = "macos")]
fn loopback_connect_check() -> Option<String> {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], 9));
    match std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(500)) {
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => None,
        Err(e) => Some(format!(
            "a loopback connection ended with {:?}, not a denial",
            e.kind()
        )),
        Ok(_) => Some("a loopback connection succeeded".to_owned()),
    }
}

#[cfg(test)]
mod tests;
