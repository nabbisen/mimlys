//! The Linux backend (RFC 011 §2; REQ-CONF-005): applied by the worker to
//! itself, in this order, each rung recorded: a delegated cgroup (memory,
//! pids, kill) or the fallbacks; the worker as child subreaper;
//! `no_new_privs`; a seccomp filter denying sockets outside `AF_UNIX`, the
//! privilege and namespace syscalls, `io_uring` (which creates sockets the
//! `socket` rule never sees) and cross-process memory access; Landlock last,
//! with the read and write sets, TCP `bind` and `connect` handled with no
//! rule where the ABI (4 and later) allows and network is denied, and
//! abstract Unix sockets scoped where the ABI allows (REQ-CONF-012). A
//! pathname Unix socket is reachable only inside the write set (REQ-CONF-001,
//! REQ-CONF-005): from ABI 9 Landlock handles `ResolveUnix` with a rule on the
//! write set alone; below it the filter denies `socket(AF_UNIX, ..)`
//! (`socketpair` reaches no path and stays). No
//! foreign-function code: `landlock`, `seccompiler` and `rustix` are safe
//! interfaces, and `libc` contributes syscall numbers only. The one
//! `unsafe` block is the `pre_exec` hook that puts a host tool's process
//! into the `tool` cgroup between fork and exec (REQ-CONF-010).

#![allow(unsafe_code)]

use super::{Applied, ConfinementSpec};
use landlock::{
    ABI, Access, AccessFs, AccessNet, CompatLevel, Compatible, Ruleset, RulesetAttr,
    RulesetCreatedAttr, RulesetStatus, path_beneath_rules,
};
use mimlys_core::confine::ClauseState;
use rustix::process::{Pid, Signal, kill_process};
use seccompiler::{
    BpfProgram, SeccompAction, SeccompCmpArgLen, SeccompCmpOp, SeccompCondition, SeccompFilter,
    SeccompRule, TargetArch,
};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Applies the backend to the calling process and reports what held.
pub fn apply(spec: &ConfinementSpec) -> Applied {
    let mut report: BTreeMap<String, ClauseState> = BTreeMap::new();

    // M, C and the strongest rung of K: a delegated cgroup, tried control by
    // control (RFC 011 §2): the directory and the move hold K's `cgroup.kill`
    // even where a session refuses `memory.max` or `pids.max`.
    let cgroup = join_cgroup(spec);
    let rlimit_memory = cgroup.memory.is_err();
    report.insert(
        "M".into(),
        match &cgroup.memory {
            Ok(()) => ClauseState::enforced("cgroup_memory_max"),
            Err(reason) => ClauseState::best_effort(&format!("rlimit_as ({reason})")),
        },
    );
    report.insert(
        "C".into(),
        match &cgroup.pids {
            Ok(()) => ClauseState::enforced("cgroup_pids_max"),
            Err(_) => ClauseState::best_effort("wall_clock"),
        },
    );

    // K: the subreaper always (the walk reaches re-parented orphans); the
    // `tool` sub-cgroup's `cgroup.kill` is the stronger primitive when it
    // exists: every process a host tool starts is in it, however it forks.
    let subreaper = rustix::process::set_child_subreaper(Some(rustix::process::getpid())).is_ok();

    // P (part one): no privilege gain for this process and everything it starts.
    let no_new_privs = rustix::thread::set_no_new_privs(true).is_ok();

    // The Landlock ABI the kernel offers (capped in test builds), asked for
    // once: it decides which primitive keeps a pathname Unix socket inside the
    // write set (REQ-CONF-001, REQ-CONF-005). From ABI 9 Landlock itself
    // handles `ResolveUnix`; below it the seccomp filter denies the creation
    // of a Unix socket (`socketpair` reaches no path and stays allowed).
    let abi = detect_abi();
    let resolve_unix = abi.is_some_and(|a| a >= ABI::V9);

    // N (part two) and P (part two): the seccomp filter.
    let seccomp = match install_seccomp(spec.deny_network, !resolve_unix) {
        Ok(()) => true,
        Err(reason) => {
            report.insert("seccomp_error".into(), ClauseState::unavailable(&reason));
            false
        }
    };
    report.remove("seccomp_error");
    report.insert(
        "P".into(),
        match (no_new_privs, seccomp) {
            (true, true) => ClauseState::enforced("no_new_privs+seccomp"),
            (true, false) => ClauseState::enforced("no_new_privs"),
            (false, _) => ClauseState::unavailable("no_new_privs_failed"),
        },
    );

    // F: Landlock last, so that the rules above could still read what they
    // needed; the worker's own cgroup stays writable for the tool sub-cgroup.
    // Where the ABI is 4 or later and network is denied, the same ruleset
    // also handles TCP `bind` and `connect` with no rule: a second layer
    // under the seccomp filter (UDP stays with seccomp).
    let landlock = apply_landlock(spec, cgroup.dir.as_deref(), abi);
    let landlock_net = matches!(
        &landlock,
        Ok(Landlock {
            status: RulesetStatus::FullyEnforced,
            net: true
        })
    );
    let landlock_full = matches!(
        &landlock,
        Ok(Landlock {
            status: RulesetStatus::FullyEnforced,
            ..
        })
    );
    let guard = unix_guard(resolve_unix, landlock_full, seccomp);
    report.insert("F".into(), filesystem_state(&landlock, guard));
    report.insert(
        "N".into(),
        network_state(spec.deny_network, seccomp, landlock_net, guard),
    );

    // K, decided after Landlock from what this process can still do: the
    // `tool` sub-cgroup is used only when its `cgroup.procs` and
    // `cgroup.kill` are writable now (detected, never assumed).
    let tool_cgroup = cgroup
        .dir
        .as_ref()
        .map(|d| d.join(TOOL_CGROUP))
        .filter(|t| {
            std::fs::OpenOptions::new()
                .write(true)
                .open(t.join("cgroup.procs"))
                .is_ok()
                && std::fs::OpenOptions::new()
                    .write(true)
                    .open(t.join("cgroup.kill"))
                    .is_ok()
        });
    report.insert(
        "K".into(),
        match (tool_cgroup.is_some(), subreaper) {
            (true, _) => ClauseState::enforced("cgroup_kill"),
            (false, true) => ClauseState::enforced("subreaper"),
            (false, false) => ClauseState::unavailable("subreaper_failed"),
        },
    );

    Applied {
        report,
        cgroup: cgroup.dir,
        tool_cgroup,
        rlimit_memory,
    }
}

/// The primitive that keeps a pathname Unix socket inside the write set,
/// named only when it holds: Landlock's `ResolveUnix` (the ruleset applied in
/// full on an ABI that has it), or the filter's denial of `socket(AF_UNIX, ..)`
/// (installed, on an ABI that does not). `None` when neither holds.
fn unix_guard(resolve_unix: bool, landlock_full: bool, seccomp: bool) -> Option<&'static str> {
    if resolve_unix && landlock_full {
        Some(UNIX_GUARD_LANDLOCK)
    } else if !resolve_unix && seccomp {
        Some(UNIX_GUARD_SECCOMP)
    } else {
        None
    }
}

/// Clause F: Landlock in full, and a Unix socket guard; without the guard the
/// clause is not enforced, however complete the rest of the ruleset is.
fn filesystem_state(landlock: &Result<Landlock, String>, guard: Option<&str>) -> ClauseState {
    match landlock {
        Ok(Landlock {
            status: RulesetStatus::FullyEnforced,
            ..
        }) => match guard {
            Some(UNIX_GUARD_LANDLOCK) => ClauseState::enforced(UNIX_GUARD_LANDLOCK),
            Some(guard) => ClauseState::enforced(&format!("landlock+{guard}")),
            None => ClauseState::unavailable("no_unix_socket_guard"),
        },
        Ok(Landlock {
            status: RulesetStatus::PartiallyEnforced,
            ..
        }) => ClauseState::best_effort("landlock_partial"),
        Ok(Landlock {
            status: RulesetStatus::NotEnforced,
            ..
        }) => ClauseState::unavailable("landlock_not_enforced"),
        Err(reason) => ClauseState::unavailable(reason),
    }
}

/// Clause N reads from what held: the filter alone is complete for the
/// network (it also denies `io_uring`, which creates sockets the `socket`
/// rule never sees), the Landlock layer is named when it held too, and so is
/// the primitive that keeps Unix sockets inside the write set, without which
/// N does not say `enforced`.
fn network_state(
    deny_network: bool,
    seccomp: bool,
    landlock_net: bool,
    guard: Option<&str>,
) -> ClauseState {
    if !deny_network {
        ClauseState::unavailable("network allowed for the dependency step")
    } else if !seccomp {
        ClauseState::unavailable("seccomp_unsupported")
    } else if let Some(guard) = guard {
        let layers = if landlock_net {
            vec!["seccomp", "landlock_net", guard]
        } else {
            vec!["seccomp", guard]
        };
        ClauseState::enforced(&layers.join("+"))
    } else {
        ClauseState::unavailable("no_unix_socket_guard")
    }
}

/// The sub-cgroup of the worker's cgroup that holds every host tool's tree.
pub const TOOL_CGROUP: &str = "tool";

/// What the cgroup rung produced: the worker's own cgroup when it could be
/// created and joined, and each limit on its own.
struct Cgroup {
    dir: Option<PathBuf>,
    memory: Result<(), String>,
    pids: Result<(), String>,
}

impl Cgroup {
    fn none(reason: &str) -> Self {
        Self {
            dir: None,
            memory: Err(reason.to_owned()),
            pids: Err(reason.to_owned()),
        }
    }
}

/// Puts the spawned child into the `tool` sub-cgroup before it runs
/// anything: `procs` is that cgroup's `cgroup.procs`, opened by the
/// worker; the child writes `0` (itself) into it between fork and exec.
pub fn attach_child_to_cgroup(cmd: &mut std::process::Command, procs: std::fs::File) {
    use std::os::unix::process::CommandExt;
    // The closure runs in the forked child before `exec`: one `write(2)` on
    // a descriptor the parent opened, no allocation, no lock, nothing that
    // is not async-signal-safe (REQ-CONF-010); the descriptor closes at exec.
    // SAFETY: `pre_exec` only requires the closure to be async-signal-safe, which it is.
    unsafe {
        cmd.pre_exec(move || {
            rustix::io::write(&procs, b"0")
                .map(|_| ())
                .map_err(std::io::Error::from)
        });
    }
}

/// Removes a worker's cgroup directories once they are empty (a cgroup with
/// processes cannot be removed; the error is then the caller's signal to
/// kill first).
pub fn remove_cgroup(dir: &Path) -> bool {
    let _ = std::fs::remove_dir(dir.join(TOOL_CGROUP));
    std::fs::remove_dir(dir).is_ok()
}

/// Removes the `mimlys-<pid>` cgroups of workers that no longer exist under
/// this process's own cgroup: a crash between a worker's start and the
/// runtime's record can leave an empty directory behind. A populated
/// cgroup, or one whose pid is alive, is left alone. Returns how many went.
pub fn sweep_stale_cgroups() -> usize {
    let Some(base) = own_cgroup_dir() else {
        return 0;
    };
    let Ok(entries) = std::fs::read_dir(&base) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name
            .to_str()
            .and_then(|n| n.strip_prefix("mimlys-"))
            .and_then(|p| p.parse::<u32>().ok())
        else {
            continue;
        };
        if Path::new("/proc").join(pid.to_string()).exists() {
            continue;
        }
        let dir = entry.path();
        let populated = |d: &Path| {
            std::fs::read_to_string(d.join("cgroup.procs")).is_ok_and(|s| !s.trim().is_empty())
        };
        if populated(&dir) || populated(&dir.join(TOOL_CGROUP)) {
            continue;
        }
        if remove_cgroup(&dir) {
            removed += 1;
        }
    }
    removed
}

/// This process's cgroup directory under `/sys/fs/cgroup` (cgroup v2).
fn own_cgroup_dir() -> Option<PathBuf> {
    let own = std::fs::read_to_string("/proc/self/cgroup").ok()?;
    let rel = own.lines().find_map(|l| l.strip_prefix("0::"))?.trim();
    Some(PathBuf::from("/sys/fs/cgroup").join(rel.trim_start_matches('/')))
}

/// Creates `<own cgroup>/mimlys-<pid>` and its `tool` sub-cgroup, moves
/// this process into the former, and sets each limit on its own: a session
/// that refuses a controller (the "no internal processes" rule of a scope
/// that still holds the terminal) keeps the directory and `cgroup.kill`.
fn join_cgroup(spec: &ConfinementSpec) -> Cgroup {
    // Test builds: `MIMLYS_PROBE_NO_CGROUP` skips the rung, so that the
    // `subreaper` rung of clause K (what a runner without a delegated cgroup
    // gets) is exercised on any machine (REQ-TEST-013).
    if cfg!(debug_assertions) && std::env::var_os("MIMLYS_PROBE_NO_CGROUP").is_some() {
        return Cgroup::none("injected (MIMLYS_PROBE_NO_CGROUP)");
    }
    let Some(base) = own_cgroup_dir() else {
        return Cgroup::none("cgroup_v1");
    };
    let Ok(controllers) = std::fs::read_to_string(base.join("cgroup.controllers")) else {
        return Cgroup::none("cgroup_not_delegated");
    };
    let dir = base.join(format!("mimlys-{}", std::process::id()));
    if std::fs::create_dir(&dir).is_err() {
        return Cgroup::none("cgroup_not_writable");
    }
    if std::fs::write(dir.join("cgroup.procs"), std::process::id().to_string()).is_err() {
        let _ = std::fs::remove_dir(&dir);
        return Cgroup::none("cgroup_move_failed");
    }
    let _ = std::fs::create_dir(dir.join(TOOL_CGROUP));
    let limit = |controller: &str, file: &str, value: String| -> Result<(), String> {
        if !controllers.split_whitespace().any(|c| c == controller) {
            return Err(format!("cgroup_{controller}_not_delegated"));
        }
        // Enabling the controller for the base's children fails where the
        // base still holds processes; the limit file then does not exist.
        let _ = std::fs::write(
            base.join("cgroup.subtree_control"),
            format!("+{controller}"),
        );
        std::fs::write(dir.join(file), value).map_err(|e| format!("{file}: {}", e.kind()))
    };
    Cgroup {
        memory: limit("memory", "memory.max", spec.memory_bytes.to_string()),
        pids: limit("pids", "pids.max", spec.max_processes.to_string()),
        dir: Some(dir),
    }
}

/// The syscalls the filter always denies, answered `EPERM`: the privilege and
/// namespace group of M6a, `io_uring` and cross-process memory (REQ-CONF-012).
const ALWAYS_DENIED: [i64; 22] = [
    libc::SYS_ptrace,
    libc::SYS_mount,
    libc::SYS_umount2,
    libc::SYS_pivot_root,
    libc::SYS_chroot,
    libc::SYS_keyctl,
    libc::SYS_add_key,
    libc::SYS_request_key,
    libc::SYS_bpf,
    libc::SYS_unshare,
    libc::SYS_setns,
    libc::SYS_kexec_load,
    libc::SYS_init_module,
    libc::SYS_finit_module,
    libc::SYS_delete_module,
    libc::SYS_swapon,
    libc::SYS_reboot,
    // REQ-CONF-012: a socket created by `IORING_OP_SOCKET` (Linux 5.19
    // and later) never passes the `socket` rule below, so the ring is
    // never created; `enter` and `register` are denied for clarity.
    libc::SYS_io_uring_setup,
    libc::SYS_io_uring_enter,
    libc::SYS_io_uring_register,
    // REQ-CONF-012: another process's memory, which `ptrace`'s denial
    // does not cover.
    libc::SYS_process_vm_readv,
    libc::SYS_process_vm_writev,
];

/// The primitive name when Landlock handles `ResolveUnix` (ABI 9 and later).
const UNIX_GUARD_LANDLOCK: &str = "landlock_resolve_unix";

/// The primitive name when the filter denies `socket(AF_UNIX, ..)` (below ABI 9).
const UNIX_GUARD_SECCOMP: &str = "seccomp_no_unix_socket";

/// Which `socket` calls the filter answers `EPERM`. `socketpair` is a
/// different system call, reaches no path, and is never part of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SocketRule {
    /// None: network allowed and Landlock handles Unix sockets.
    Open,
    /// Only `AF_UNIX` (network allowed, below Landlock ABI 9).
    DenyUnix,
    /// Everything but `AF_UNIX` (network denied, Landlock ABI 9 and later).
    DenyInet,
    /// Every call (network denied, below Landlock ABI 9).
    DenyAll,
}

impl SocketRule {
    fn of(deny_network: bool, deny_unix: bool) -> Self {
        match (deny_network, deny_unix) {
            (false, false) => SocketRule::Open,
            (false, true) => SocketRule::DenyUnix,
            (true, false) => SocketRule::DenyInet,
            (true, true) => SocketRule::DenyAll,
        }
    }
}

/// The filter: sockets outside `AF_UNIX` when network is denied, `AF_UNIX`
/// too below Landlock ABI 9 (`deny_unix`), and always, answered `EPERM`: the
/// privilege and namespace syscalls, `io_uring` and cross-process memory
/// access (also in the dependency step's worker, which needs none of them).
fn seccomp_program(deny_network: bool, deny_unix: bool) -> Result<BpfProgram, String> {
    let arch = match std::env::consts::ARCH {
        "x86_64" => TargetArch::x86_64,
        "aarch64" => TargetArch::aarch64,
        other => return Err(format!("seccomp_arch:{other}")),
    };
    let mut rules: BTreeMap<i64, Vec<SeccompRule>> = BTreeMap::new();
    let always = ALWAYS_DENIED;
    for nr in always {
        rules.insert(nr, vec![]);
    }
    if let Some(socket_rules) = socket_rules(SocketRule::of(deny_network, deny_unix))? {
        rules.insert(libc::SYS_socket, socket_rules);
    }
    let filter = SeccompFilter::new(
        rules,
        SeccompAction::Allow,
        SeccompAction::Errno(libc::EPERM as u32),
        arch,
    )
    .map_err(|e| format!("seccomp_filter:{e}"))?;
    filter
        .try_into()
        .map_err(|e: seccompiler::BackendError| format!("seccomp_compile:{e}"))
}

/// Installs the filter on the calling thread (and so, from the worker's main
/// thread before any other exists, on everything it starts).
fn install_seccomp(deny_network: bool, deny_unix: bool) -> Result<(), String> {
    let program = seccomp_program(deny_network, deny_unix)?;
    seccompiler::apply_filter(&program).map_err(|e| format!("seccomp_apply:{e}"))
}

/// The `socket` rules of one [`SocketRule`]; `None` when the call is not filtered.
fn socket_rules(rule: SocketRule) -> Result<Option<Vec<SeccompRule>>, String> {
    let domain = |op: SeccompCmpOp| {
        SeccompCondition::new(0, SeccompCmpArgLen::Dword, op, libc::AF_UNIX as u64)
            .map_err(|e| format!("seccomp_condition:{e}"))
    };
    let condition = match rule {
        SocketRule::Open => return Ok(None),
        // An empty list of rules is answered for every call, as for the other denied calls.
        SocketRule::DenyAll => return Ok(Some(vec![])),
        SocketRule::DenyUnix => domain(SeccompCmpOp::Eq)?,
        SocketRule::DenyInet => domain(SeccompCmpOp::Ne)?,
    };
    Ok(Some(vec![
        SeccompRule::new(vec![condition]).map_err(|e| format!("seccomp_rule:{e}"))?,
    ]))
}

/// The environment variable that caps the Landlock ABI in a test build
/// (REQ-TEST-013): the rung below ABI 9 then runs on any kernel. Read only
/// when `debug_assertions` are on, like the other `MIMLYS_PROBE_*` knobs.
pub const ABI_CAP_VARIABLE: &str = "MIMLYS_PROBE_MAX_LANDLOCK_ABI";

/// The cap a test build asks for, if any.
fn abi_cap() -> Option<i32> {
    if !cfg!(debug_assertions) {
        return None;
    }
    std::env::var(ABI_CAP_VARIABLE).ok()?.trim().parse().ok()
}

/// The highest Landlock ABI this kernel supports (not above the test cap),
/// found by asking for each as a hard requirement from the newest down (the
/// crate's own query is private). ABI 8 adds no access right to the
/// filesystem set, so it is not asked for apart from ABI 7.
fn detect_abi() -> Option<ABI> {
    detect_abi_capped(abi_cap())
}

/// [`detect_abi`] with the cap given.
fn detect_abi_capped(cap: Option<i32>) -> Option<ABI> {
    for abi in [
        ABI::V9,
        ABI::V7,
        ABI::V6,
        ABI::V5,
        ABI::V4,
        ABI::V3,
        ABI::V2,
        ABI::V1,
    ] {
        if cap.is_some_and(|cap| abi as i32 > cap) {
            continue;
        }
        let created = Ruleset::default()
            .set_compatibility(CompatLevel::HardRequirement)
            .handle_access(AccessFs::from_all(abi))
            .and_then(|r| r.create());
        if created.is_ok() {
            return Some(abi);
        }
    }
    None
}

/// Whether this kernel's Landlock has the TCP layer (ABI 4, Linux 6.7), for
/// the clause tests to know which primitive the N clause must report.
#[cfg(feature = "probe-syscalls")]
pub fn landlock_net_supported() -> bool {
    probe::landlock_abi_version() >= 4
}

/// Whether this kernel's Landlock handles `ResolveUnix` (ABI 9, Linux 7.1),
/// for the clause tests to know which primitive keeps Unix sockets inside the
/// write set. The kernel's own answer: the test cap is not applied.
#[cfg(feature = "probe-syscalls")]
pub fn landlock_resolve_unix_supported() -> bool {
    probe::landlock_abi_version() >= 9
}

/// What the Landlock rung produced: the status the kernel gave and whether
/// the TCP layer was part of the ruleset.
struct Landlock {
    status: RulesetStatus,
    /// TCP `bind` and `connect` were handled with an empty allow-set.
    net: bool,
}

/// What only the worker itself reads: `/proc`, where its subreaper walk finds
/// the descendants a cancelled host tool left (clause K). A host tool's
/// process gets the same sets without it, through a second layer
/// ([`restrict_child`]), so the processes of the machine are not part of
/// what project code can read (REQ-CONF-011): Landlock layers intersect.
const WORKER_ONLY_READ: [&str; 1] = ["/proc"];

/// What every host tool's layer is made from: the specification the worker
/// applied to itself and the Landlock ABI it asked for. The layer itself is
/// made for each host tool, for the task the tool belongs to
/// ([`restrict_child`]).
static TOOL_BASE: std::sync::OnceLock<(ConfinementSpec, ABI)> = std::sync::OnceLock::new();

/// A ruleset over the specification: every access right the kernel's ABI
/// knows handled as a hard requirement, the read set readable and
/// executable (plus `extra_read`), the write set fully accessible. With
/// `worker` set: TCP `bind` and `connect` handled with no rule when the ABI
/// is 4 or later and network is denied (REQ-CONF-012), and abstract Unix
/// sockets scoped where the ABI allows (so that no socket outside the
/// sandbox can be reached through them).
fn landlock_ruleset(
    spec: &ConfinementSpec,
    cgroup: Option<&Path>,
    abi: ABI,
    extra_read: &[&str],
    worker: bool,
) -> Result<(landlock::RulesetCreated, bool), String> {
    let existing = |paths: &[PathBuf]| -> Vec<PathBuf> {
        paths.iter().filter(|p| p.exists()).cloned().collect()
    };
    let mut read_paths = spec.read_paths.clone();
    read_paths.extend(extra_read.iter().map(PathBuf::from));
    let mut write_paths = spec.write_paths.clone();
    write_paths.extend(cgroup.map(Path::to_path_buf));
    let net = worker && spec.deny_network && abi >= ABI::V4;
    let mut ruleset = Ruleset::default()
        .set_compatibility(CompatLevel::HardRequirement)
        .handle_access(AccessFs::from_all(abi))
        .map_err(|e| format!("landlock_handle:{e}"))?;
    if net {
        ruleset = ruleset
            .handle_access(AccessNet::from_all(abi))
            .map_err(|e| format!("landlock_handle_net:{e}"))?;
    }
    if worker {
        ruleset = ruleset
            .set_compatibility(CompatLevel::BestEffort)
            .scope(landlock::Scope::AbstractUnixSocket)
            .map_err(|e| format!("landlock_scope:{e}"))?
            .set_compatibility(CompatLevel::HardRequirement);
    }
    let created = ruleset
        .create()
        .map_err(|e| format!("landlock_create:{e}"))?
        .add_rules(path_beneath_rules(
            existing(&read_paths),
            AccessFs::from_read(abi),
        ))
        .map_err(|e| format!("landlock_read_rules:{e}"))?
        .add_rules(path_beneath_rules(
            existing(&write_paths),
            AccessFs::from_all(abi),
        ))
        .map_err(|e| format!("landlock_write_rules:{e}"))?;
    Ok((created, net))
}

/// What a host tool may write of the data directory: its own task's
/// workspace tree and the temporary directory of the execution. The tool
/// reads what the worker's specification lets it read and writes nothing
/// else of the data directory: not another task's tree, not the task's own
/// metadata, not the runtime's temporary directory (REQ-CONF-006).
#[derive(Clone, Copy, Debug)]
pub struct TaskPaths<'a> {
    /// `<data>/workspaces/<task>/tree`: the tool's working directory.
    pub tree: &'a Path,
    /// `<data>/workspaces/<task>/tmp/<execution>`: its `HOME` and `TMPDIR`.
    pub tmp: &'a Path,
}

/// The specification of one task's tools: the worker's, with the write set's
/// entries inside the data directory (every task's workspace and the runtime's
/// temporary directory) replaced by the two of this task. The paths are
/// checked to be what they claim: the tree of one task directly under
/// `<data>/workspaces`, and the temporary directory inside that task's
/// directory. Anything else is refused, never widened.
fn tool_spec(base: &ConfinementSpec, task: TaskPaths<'_>) -> Result<ConfinementSpec, String> {
    if base.data_dir.as_os_str().is_empty() {
        return Err("no data directory in the specification".to_owned());
    }
    let task_dir = task
        .tree
        .parent()
        .filter(|dir| dir.parent() == Some(base.data_dir.join("workspaces").as_path()))
        .ok_or_else(|| format!("{} is not the tree of a task", task.tree.display()))?;
    if task.tree.file_name().is_none_or(|name| name != "tree") || !task.tmp.starts_with(task_dir) {
        return Err(format!(
            "{} and {} do not belong to one task",
            task.tree.display(),
            task.tmp.display()
        ));
    }
    for dir in [task.tree, task.tmp] {
        if !dir.is_dir() {
            return Err(format!("{} is not a directory", dir.display()));
        }
    }
    let mut spec = base.clone();
    spec.write_paths.retain(|p| !p.starts_with(&base.data_dir));
    spec.write_paths.push(task.tree.to_path_buf());
    spec.write_paths.push(task.tmp.to_path_buf());
    Ok(spec)
}

/// The host tools' ruleset for one task. It holds no right on the worker's
/// own cgroup directory, which only the worker's layer carries
/// (REQ-CONF-010): a tool's process joins the `tool` sub-cgroup through a
/// descriptor the worker opened.
fn tool_ruleset(
    base: &ConfinementSpec,
    abi: ABI,
    task: TaskPaths<'_>,
) -> Result<landlock::RulesetCreated, String> {
    let spec = tool_spec(base, task)?;
    landlock_ruleset(&spec, None, abi, &[], false).map(|(ruleset, _)| ruleset)
}

/// Landlock for the worker (REQ-CONF-011, REQ-CONF-012): the worker
/// restricts itself with the read and write sets of the specification plus
/// `/proc`, and keeps the specification for the layers of its host tools.
fn apply_landlock(
    spec: &ConfinementSpec,
    cgroup: Option<&Path>,
    abi: Option<ABI>,
) -> Result<Landlock, String> {
    let abi = abi.ok_or_else(|| "landlock_unsupported".to_owned())?;
    let (worker, net) = landlock_ruleset(spec, cgroup, abi, &WORKER_ONLY_READ, true)?;
    let _ = TOOL_BASE.set((spec.clone(), abi));
    let status = worker
        .restrict_self()
        .map_err(|e| format!("landlock_restrict:{e}"))?;
    Ok(Landlock {
        status: status.ruleset,
        net,
    })
}

/// Restricts a host tool's process, between fork and exec, to the worker's
/// sets without what only the worker reads, and to its own task's workspace
/// and the execution's temporary directory of the data directory: a second
/// layer, made now for this tool, so that the process is bound by both
/// (REQ-CONF-006, REQ-CONF-011). A restriction that cannot be made is an
/// error and the tool does not start (fail closed). A worker without
/// Landlock has no layer and nothing to add (it reports F, and the worker
/// does not run a host tool). The ruleset descriptor is the caller's to keep
/// open until the process has been spawned: the fork inherits it.
pub fn restrict_child(
    cmd: &mut std::process::Command,
    task: TaskPaths<'_>,
) -> Result<Option<std::os::fd::OwnedFd>, String> {
    use std::os::fd::AsRawFd;
    use std::os::unix::process::CommandExt;
    let Some((base, abi)) = TOOL_BASE.get() else {
        return Ok(None);
    };
    let fd: Option<std::os::fd::OwnedFd> = tool_ruleset(base, *abi, task)?.into();
    let fd = fd.ok_or_else(|| "landlock_tool_layer: no descriptor".to_owned())?;
    let raw = fd.as_raw_fd();
    // The closure runs in the forked child before `exec`: one raw system call
    // on an inherited descriptor, no allocation, no lock (REQ-CONF-010).
    // SAFETY: `landlock_restrict_self` is async-signal-safe and touches only this process.
    unsafe {
        cmd.pre_exec(move || {
            if libc::syscall(libc::SYS_landlock_restrict_self, raw, 0u32) == 0 {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error())
            }
        });
    }
    Ok(Some(fd))
}

/// Kills every descendant of `root` (not `root` itself) until none remains
/// or `grace` passes; returns how many were still alive at the end.
pub fn kill_descendants(root: u32, grace: Duration) -> usize {
    let started = Instant::now();
    loop {
        let descendants = descendants_of(root);
        if descendants.is_empty() {
            return 0;
        }
        for pid in &descendants {
            if let Some(pid) = Pid::from_raw(*pid as i32) {
                let _ = kill_process(pid, Signal::KILL);
            }
        }
        if started.elapsed() >= grace {
            return descendants_of(root).len();
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Every live process whose ancestry reaches `root`, from `/proc`.
pub fn descendants_of(root: u32) -> Vec<u32> {
    let mut parent: HashMap<u32, u32> = HashMap::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return vec![];
    };
    for entry in entries.flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        if let Some(ppid) = ppid_of(&entry.path()) {
            parent.insert(pid, ppid);
        }
    }
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for &pid in parent.keys() {
        let mut cur = pid;
        let mut hops = 0;
        while let Some(&pp) = parent.get(&cur) {
            if pp == root {
                if seen.insert(pid) {
                    out.push(pid);
                }
                break;
            }
            if pp <= 1 || hops > 64 {
                break;
            }
            cur = pp;
            hops += 1;
        }
    }
    out
}

/// The parent of a live process; a zombie (already dead, not yet reaped
/// by its parent) is not a descendant to kill.
fn ppid_of(proc_dir: &Path) -> Option<u32> {
    let stat = std::fs::read_to_string(proc_dir.join("stat")).ok()?;
    // `pid (comm) state ppid …`: the comm may contain spaces and parentheses.
    let after = stat.rsplit_once(')')?.1;
    let mut fields = after.split_whitespace();
    if fields.next()? == "Z" {
        return None;
    }
    fields.next()?.parse().ok()
}

/// `cgroup.kill` of a recorded cgroup, for a runtime terminating a tree it did not spawn (REQ-CONF-009).
pub fn kill_cgroup(path: &Path) -> bool {
    std::fs::write(path.join("cgroup.kill"), "1").is_ok()
}

#[cfg(feature = "probe-syscalls")]
pub mod probe;

#[cfg(test)]
mod tests;
