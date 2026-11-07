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
//! Clauses K, M and C (RFC 011 Amendment 2, settled, points 6 and 7): the
//! worker is a member of its sandbox together with every host tool's tree.
//! No listing restricted to the sandbox exists, so the members are found by
//! listing every pid (`proc_listallpids`, which the profile allows without a
//! target) and keeping the ones `proc_pidinfo` can read, which the profile
//! lets the worker read for members only. K is the **membership kill loop**
//! ([`sweep`]): `SIGKILL` every member but the worker, repeat until a listing
//! finds none or LIM-O2 passes. M and C are watchdogs over the same members
//! ([`verdict`]), polled every LIM-H6. The decisions are pure and tested on
//! every platform; the calls are in `macos/sys.rs`.
//!
//! The kill loop is guarded: it does nothing unless this process verified,
//! after applying the profile, that the listing works and that processes
//! outside the sandbox can be neither inspected nor signalled
//! ([`membership_check`]). Without the profile the same loop would find and
//! kill every process of the user.

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

/// Why N reports `unavailable` for the dependency step's worker (as on Linux).
pub const FETCH_NETWORK: &str = "network allowed for the dependency step";
/// The system's certificate file the dependency step reads (settled point 9).
pub const CERT_FILE: &str = "/private/etc/ssl/cert.pem";

/// The link `xcode-select` keeps to the active developer directory; read
/// without running a command (Amendment 2 point 8).
pub const XCODE_SELECT_LINK: &str = "/private/var/db/xcode_select_link";
/// Where the Command Line Tools live when no Xcode is selected.
pub const COMMAND_LINE_TOOLS: &str = "/Library/Developer/CommandLineTools";
/// Rosetta's runtime, where it is installed (Amendment 2, settled, point 8):
/// a translated (Intel) process started under the profile aborts without
/// it (measured by 011c-1 under the Rosetta job).
pub const ROSETTA_RUNTIME: &str = "/Library/Apple/usr/libexec/oah";
/// The socket of the name-lookup daemon. Denied explicitly by the offline
/// profile (Amendment 2, settled, point 9), so that no later rule reopens
/// name resolution, an egress channel, silently.
pub const MDNSRESPONDER_SOCKET: &str = "/private/var/run/mDNSResponder";

/// The system part of the read set that is the same on every Mac: the
/// measured minimum of RFC 011 Amendment 2, settled, point 8 (spike S1b: the
/// necessity of each entry was measured by removing it from a working set).
/// `/usr`; `/bin` and `/sbin` (shells and tools that builds `exec`); the one
/// file of OpenSSL's configuration and the one file of the system version
/// that tools read; the random and zero devices. Not `/System` as a whole
/// (the dyld cache is mapped by the kernel), not `/private/etc` as a whole,
/// no file of the home directory. The root directory itself, as a literal,
/// is written by [`generate`].
const SYSTEM_READ: [&str; 8] = [
    "/usr",
    "/bin",
    "/sbin",
    "/private/etc/ssl/openssl.cnf",
    "/System/Library/CoreServices/SystemVersion.plist",
    "/dev/urandom",
    "/dev/random",
    "/dev/zero",
];

/// `xcrun`'s cache file, in the per-user temporary directory
/// (`confstr(_CS_DARWIN_USER_TEMP_DIR)`, not `$TMPDIR`): read as a literal
/// file, as the directory holds other programs' files (settled point 8).
pub const XCRUN_CACHE: &str = "xcrun_db";

/// The one mach service the profile admits (settled point 8): what `xcrun`
/// asks for to find the per-user directories. One of two interchangeable
/// ones; the other, `com.apple.system.opendirectoryd.membership`, can
/// consult a network directory on a managed Mac and is refused. A service
/// that can reach the network or the user's secrets on the caller's behalf
/// (`trustd`, `nsurlsessiond`, `securityd` and the like) is never admitted.
pub const MACH_SERVICES: [&str; 1] = ["com.apple.bsd.dirhelper"];

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

/// The system part of the read set (settled point 8): the fixed entries,
/// the active developer directory (read from the `xcode_select` link, else
/// the Command Line Tools when they exist) and, for an Xcode, its
/// `Contents/SharedFrameworks`, `xcrun`'s cache file in the per-user
/// temporary directory, and Rosetta's runtime where it is installed.
pub fn system_read_paths() -> Vec<PathBuf> {
    #[cfg(target_os = "macos")]
    let user_temp = sys::user_temp_dir();
    #[cfg(not(target_os = "macos"))]
    let user_temp = None;
    system_read_paths_from(
        std::fs::read_link(XCODE_SELECT_LINK).ok(),
        Path::new(COMMAND_LINE_TOOLS).is_dir(),
        Path::new(ROSETTA_RUNTIME).is_dir(),
        user_temp,
    )
}

/// [`system_read_paths`] from what it depends on (tested on every platform):
/// the target of the `xcode_select` link, when it is an absolute path, else
/// the Command Line Tools directory when it exists; Rosetta's runtime when
/// it is installed; the per-user temporary directory, when known.
pub fn system_read_paths_from(
    developer_link: Option<PathBuf>,
    tools_present: bool,
    rosetta_present: bool,
    user_temp: Option<PathBuf>,
) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = SYSTEM_READ.iter().map(PathBuf::from).collect();
    match developer_link.filter(|p| is_posix_absolute(p)) {
        Some(dir) => {
            paths.extend(shared_frameworks(&dir));
            paths.push(dir);
        }
        None if tools_present => paths.push(PathBuf::from(COMMAND_LINE_TOOLS)),
        None => {}
    }
    if rosetta_present {
        paths.push(PathBuf::from(ROSETTA_RUNTIME));
    }
    paths.extend(
        user_temp
            .filter(|t| is_posix_absolute(t))
            .and_then(|t| t.to_str().map(str::to_owned))
            .map(|t| PathBuf::from(format!("{}/{XCRUN_CACHE}", t.trim_end_matches('/')))),
    );
    paths
}

/// An Xcode's `Contents/SharedFrameworks`, beside its `Contents/Developer`
/// (`xcodebuild`, which the `cc` shim starts, reads one framework there:
/// measured). `None` for any other shape of developer directory.
fn shared_frameworks(developer: &Path) -> Option<PathBuf> {
    // Text, not the host's rules: the tests of this function run on every platform.
    let text = developer.to_str()?.trim_end_matches('/');
    text.strip_suffix("/Contents/Developer")
        .filter(|app| !app.is_empty())
        .map(|app| PathBuf::from(format!("{app}/Contents/SharedFrameworks")))
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
///    another process) and `kern.proc.` denied (the process list with names);
/// 5. the one mach service ([`MACH_SERVICES`]); other processes: `signal` and
///    `process-info*` only for the sandbox's
///    members (the tool's own tree), and the pid listing, which has no
///    restricted form (the numbers of every pid are visible; the members are
///    the ones `proc_pidinfo` can read). The broad denial is `(with
///    no-report)`: the membership loop and the watchdogs inspect every pid at
///    every poll, and each refusal would otherwise be one record in the
///    system log (measured: thousands per test run);
/// 6. network: denied, then Unix sockets under the write set only, then the
///    `mDNSResponder` socket denied explicitly (after the allows, so that no
///    write path can reopen it). No other mach service: DNS is an
///    egress channel (measured). The dependency step's worker
///    (`deny_network` false) adds exactly three rules (settled point 9):
///    outbound TCP to port 443, a connect to the `mDNSResponder` socket and
///    the system's certificate file readable; the explicit denial is omitted.
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
         (deny sysctl-read (sysctl-name-prefix \"kern.procargs\"))\n\
         (deny sysctl-read (sysctl-name-prefix \"kern.proc.\"))\n",
    );
    p.push_str("\n;; mach services: the one `xcrun` asks for (settled point 8)\n");
    for service in MACH_SERVICES {
        p.push_str(&format!(
            "(allow mach-lookup (global-name \"{service}\"))\n"
        ));
    }
    p.push_str(
        "\n;; other processes: the sandbox's members only\n\
         (allow signal (target same-sandbox))\n\
         (deny process-info* (with no-report))\n\
         (allow process-info* (target same-sandbox))\n\
         (allow process-info-listpids)\n",
    );
    p.push_str("\n;; network\n");
    if spec.deny_network {
        p.push_str("(deny network*)\n");
    } else {
        // The dependency step (settled point 9): registries over HTTPS, name
        // lookup through the one socket, the system's certificates; no mach service.
        p.push_str(&format!(
            ";; the dependency step: HTTPS, one name-lookup socket, the system's certificates\n\
             (allow network-outbound (remote tcp \"*:443\"))\n\
             (allow network-outbound (remote unix-socket (literal \"{MDNSRESPONDER_SOCKET}\")))\n\
             (allow file-read-data (literal \"{CERT_FILE}\"))\n"
        ));
    }
    for w in &writes {
        p.push_str(&format!(
            "(allow network-bind (local unix-socket (subpath \"{w}\")))\n\
             (allow network-outbound (remote unix-socket (subpath \"{w}\")))\n\
             (allow network-inbound (local unix-socket (subpath \"{w}\")))\n"
        ));
    }
    if spec.deny_network {
        p.push_str(&format!(
            "(deny network-outbound (remote unix-socket (literal \"{MDNSRESPONDER_SOCKET}\")))\n"
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

/// What the kernel lets the worker see of one pid (the profile allows
/// `proc_pidinfo` for the sandbox's members only).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Seen {
    /// The call is refused or the process is gone: not a member.
    Hidden,
    /// A member that has ended and waits to be reaped; nothing to kill or count.
    Zombie,
    /// A live member and its memory in bytes.
    Alive {
        /// The larger of `pti_resident_size` and `ri_phys_footprint` (which
        /// counts pages the system compressed); 0 when both are refused.
        resident: u64,
    },
}

/// A live member of the sandbox other than the worker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Member {
    /// Its process id.
    pub pid: i32,
    /// Its memory in bytes (see [`Seen::Alive`]).
    pub resident: u64,
}

/// The members among a pid listing (Amendment 2, settled, point 6): the
/// pids `see` can read, live, other than the worker itself and `launchd`
/// (pid 1 and below are never members).
pub fn members_of(listing: &[i32], own: i32, mut see: impl FnMut(i32) -> Seen) -> Vec<Member> {
    listing
        .iter()
        .copied()
        .filter(|&pid| pid > 1 && pid != own)
        .filter_map(|pid| match see(pid) {
            Seen::Alive { resident } => Some(Member { pid, resident }),
            Seen::Hidden | Seen::Zombie => None,
        })
        .collect()
}

/// How a membership kill loop ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SweepReport {
    /// Signals sent.
    pub killed: usize,
    /// Listings that found members.
    pub rounds: usize,
    /// Whether the last listing succeeded and found no member. `false` is
    /// `tree_kill_incomplete`: LIM-O2 passed, or the listing failed.
    pub complete: bool,
}

/// The membership kill loop (Amendment 2, settled, point 6): list the
/// members, `SIGKILL` each, repeat until a listing finds none or `grace`
/// passes (LIM-O2). `list` is `None` when the listing itself fails. A fork
/// storm converged in 0.16 s when measured; re-parenting cannot escape
/// membership, and a reused pid of an outsider cannot be hit (the kernel
/// refuses to signal it).
pub fn sweep_with(
    grace: std::time::Duration,
    mut list: impl FnMut() -> Option<Vec<Member>>,
    mut kill: impl FnMut(i32),
) -> SweepReport {
    let started = std::time::Instant::now();
    let mut report = SweepReport {
        killed: 0,
        rounds: 0,
        complete: false,
    };
    loop {
        match list() {
            None => return report,
            Some(members) if members.is_empty() => {
                report.complete = true;
                return report;
            }
            Some(members) => {
                report.rounds += 1;
                for m in &members {
                    kill(m.pid);
                    report.killed += 1;
                }
            }
        }
        if started.elapsed() >= grace {
            // One last look: the final signals may have landed.
            report.complete = matches!(list(), Some(m) if m.is_empty());
            return report;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// What a watchdog found over the members.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Their summed memory (resident, compressed pages included) is above the ceiling (M; LIM-H2).
    Memory {
        /// The sum.
        resident: u64,
    },
    /// Their number is above the ceiling (C; LIM-H3).
    Count {
        /// The number.
        count: usize,
    },
}

/// The watchdogs' decision (Amendment 2, settled, point 7): memory first,
/// then count; strictly above a ceiling. A ceiling of 0 means none was
/// given and nothing is watched for it.
pub fn verdict(members: &[Member], memory_bytes: u64, max_processes: u32) -> Option<Verdict> {
    let resident: u64 = members.iter().map(|m| m.resident).sum();
    if memory_bytes > 0 && resident > memory_bytes {
        return Some(Verdict::Memory { resident });
    }
    if max_processes > 0 && members.len() > max_processes as usize {
        return Some(Verdict::Count {
            count: members.len(),
        });
    }
    None
}

/// What the worker observed about the membership mechanism after applying
/// the profile (all of it from inside).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MembershipFacts {
    /// `proc_listallpids` answered with at least one pid.
    pub listing_works: bool,
    /// The worker is in the listing and can inspect itself.
    pub sees_itself: bool,
    /// Some process outside the sandbox could be inspected or signalled.
    pub outsider_reachable: bool,
    /// A process outside the sandbox could be inspected and signalled
    /// before the profile (the control: a denial of the impossible proves nothing).
    pub outsider_control: bool,
}

/// Why the membership mechanism cannot be relied on, or `None` when it can
/// (Amendment 2 point 6: the worker verifies the restriction before relying on it).
pub fn membership_check(facts: &MembershipFacts) -> Option<String> {
    if !facts.listing_works {
        return Some("the pid listing is refused".to_owned());
    }
    if !facts.sees_itself {
        return Some("the worker cannot inspect itself".to_owned());
    }
    if !facts.outsider_control {
        return Some("no process outside the sandbox to check against".to_owned());
    }
    if facts.outsider_reachable {
        return Some("a process outside the sandbox can be inspected or signalled".to_owned());
    }
    None
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
        /// The membership mechanism cannot be relied on ([`membership_check`]).
        membership: Option<String>,
    },
}

/// What an outcome means for each clause (Amendment 2 points 2 to 7). F and
/// N are `enforced (seatbelt)` only when the profile was applied and the
/// self-check saw it hold; P is `enforced (seatbelt)` under the same
/// condition as F (the kernel refuses a setuid `execve` from a sandboxed
/// process; no rule is needed, and a clause test asserts it). K is
/// `enforced (seatbelt_membership)` when the worker verified the mechanism;
/// M and C are `best_effort` watchdogs over the same members (a poll can
/// miss a short excursion) and unavailable with K.
pub fn report(deny_network: bool, outcome: &Outcome) -> BTreeMap<String, ClauseState> {
    let (f, n, p, k, m, c) = match outcome {
        Outcome::Missing => {
            let s = ClauseState::unavailable("seatbelt_missing");
            (s.clone(), s.clone(), s.clone(), s.clone(), s.clone(), s)
        }
        Outcome::BadProfile(why) => {
            let s = ClauseState::unavailable(&format!("seatbelt_profile: {why}"));
            (s.clone(), s.clone(), s.clone(), s.clone(), s.clone(), s)
        }
        Outcome::Rejected(why) => {
            let s = ClauseState::unavailable(&format!("seatbelt_rejected: {why}"));
            (s.clone(), s.clone(), s.clone(), s.clone(), s.clone(), s)
        }
        Outcome::Applied {
            filesystem,
            network,
            membership,
        } => {
            let seatbelt = ClauseState::enforced("seatbelt");
            let f = match filesystem {
                None => seatbelt.clone(),
                Some(why) => ClauseState::unavailable(&format!("seatbelt_ineffective: {why}")),
            };
            let n = match (deny_network, network) {
                (false, _) => ClauseState::unavailable(FETCH_NETWORK),
                (true, None) => seatbelt.clone(),
                (true, Some(why)) => {
                    ClauseState::unavailable(&format!("seatbelt_ineffective: {why}"))
                }
            };
            let p = match filesystem {
                None => seatbelt,
                Some(why) => ClauseState::unavailable(&format!("seatbelt_ineffective: {why}")),
            };
            let (k, m, c) = match membership {
                None => (
                    ClauseState::enforced("seatbelt_membership"),
                    ClauseState::best_effort("rss_watchdog"),
                    ClauseState::best_effort("process_count_watchdog"),
                ),
                Some(why) => {
                    let s = ClauseState::unavailable(&format!("membership_unverified: {why}"));
                    (s.clone(), s.clone(), s)
                }
            };
            (f, n, p, k, m, c)
        }
    };
    BTreeMap::from([
        ("F".to_owned(), f),
        ("N".to_owned(), n),
        ("P".to_owned(), p),
        ("K".to_owned(), k),
        ("M".to_owned(), m),
        ("C".to_owned(), c),
    ])
}

/// Set once, by [`apply`], when the membership mechanism was verified from
/// inside the applied profile. The kill loop and the watchdogs refuse to act
/// without it: outside the profile "every pid `proc_pidinfo` can read" is
/// every process of the user.
#[cfg(target_os = "macos")]
static MEMBERSHIP_VERIFIED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

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
    let proc_list_control = sys::kern_proc_all_readable();
    let outsider_control =
        parent > 1 && sys::see(parent) != Seen::Hidden && sys::can_signal(parent);

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
    if filesystem.is_none() && proc_list_control && sys::kern_proc_all_readable() {
        filesystem = Some("the process list with names (kern.proc.all) is readable".to_owned());
    }
    let network = spec.deny_network.then(loopback_connect_check).flatten();

    let own = std::process::id() as i32;
    let facts = MembershipFacts {
        listing_works: sys::list_pids().is_ok_and(|l| !l.is_empty()),
        sees_itself: sys::see(own) != Seen::Hidden,
        outsider_reachable: [1, parent]
            .into_iter()
            .any(|pid| sys::see(pid) != Seen::Hidden || sys::can_signal(pid)),
        outsider_control,
    };
    let membership = membership_check(&facts);
    MEMBERSHIP_VERIFIED.store(membership.is_none(), std::sync::atomic::Ordering::SeqCst);
    Outcome::Applied {
        filesystem,
        network,
        membership,
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

/// The live members of this sandbox other than the worker, or `None` when
/// the mechanism was not verified (see [`MEMBERSHIP_VERIFIED`]) or the listing fails.
#[cfg(target_os = "macos")]
pub fn members() -> Option<Vec<Member>> {
    if !MEMBERSHIP_VERIFIED.load(std::sync::atomic::Ordering::SeqCst) {
        return None;
    }
    let listing = sys::list_pids().ok()?;
    Some(members_of(&listing, std::process::id() as i32, sys::see))
}

/// The membership kill loop over this sandbox ([`sweep_with`]); kills
/// nothing and reports incomplete when the mechanism was not verified.
#[cfg(target_os = "macos")]
pub fn sweep(grace: std::time::Duration) -> SweepReport {
    sweep_with(grace, members, sys::kill_pid)
}

/// The watchdogs over this sandbox's members ([`verdict`]); `None` when
/// nothing is above a ceiling or the members cannot be listed.
#[cfg(target_os = "macos")]
pub fn watch(memory_bytes: u64, max_processes: u32) -> Option<Verdict> {
    verdict(&members()?, memory_bytes, max_processes)
}

#[cfg(test)]
mod tests;
