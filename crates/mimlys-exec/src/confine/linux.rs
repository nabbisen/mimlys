//! The Linux backend (RFC 011 §2; REQ-CONF-005): applied by the worker to
//! itself, in this order, each rung recorded: a delegated cgroup (memory,
//! pids, kill) or the fallbacks; the worker as child subreaper;
//! `no_new_privs`; a seccomp filter denying sockets outside `AF_UNIX`, the
//! privilege and namespace syscalls, `io_uring` (which creates sockets the
//! `socket` rule never sees) and cross-process memory access; Landlock last,
//! with the read and write sets, TCP `bind` and `connect` handled with no
//! rule where the ABI (4 and later) allows and network is denied, and
//! abstract Unix sockets scoped where the ABI allows (REQ-CONF-012). No
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

    // N (part two) and P (part two): the seccomp filter.
    let seccomp = match install_seccomp(spec.deny_network) {
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
    let landlock = apply_landlock(spec, cgroup.dir.as_deref());
    let landlock_net = matches!(
        &landlock,
        Ok(Landlock {
            status: RulesetStatus::FullyEnforced,
            net: true
        })
    );
    report.insert(
        "F".into(),
        match &landlock {
            Ok(Landlock {
                status: RulesetStatus::FullyEnforced,
                ..
            }) => ClauseState::enforced("landlock"),
            Ok(Landlock {
                status: RulesetStatus::PartiallyEnforced,
                ..
            }) => ClauseState::best_effort("landlock_partial"),
            Ok(Landlock {
                status: RulesetStatus::NotEnforced,
                ..
            }) => ClauseState::unavailable("landlock_not_enforced"),
            Err(reason) => ClauseState::unavailable(reason),
        },
    );
    // N reads from what held: the filter alone is complete (it denies the
    // `socket` rule's bypass, `io_uring`), and the Landlock layer is named
    // when it held too.
    report.insert(
        "N".into(),
        if !spec.deny_network {
            ClauseState::unavailable("network allowed for the dependency step")
        } else if seccomp && landlock_net {
            ClauseState::enforced("seccomp+landlock_net")
        } else if seccomp {
            ClauseState::enforced("seccomp")
        } else {
            ClauseState::unavailable("seccomp_unsupported")
        },
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

/// The filter: sockets outside `AF_UNIX` when network is denied, and always,
/// answered `EPERM`: the privilege and namespace syscalls, `io_uring` and
/// cross-process memory access (also in the dependency step's worker, which
/// needs none of them).
fn install_seccomp(deny_network: bool) -> Result<(), String> {
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
    if deny_network {
        let not_unix = SeccompCondition::new(
            0,
            SeccompCmpArgLen::Dword,
            SeccompCmpOp::Ne,
            libc::AF_UNIX as u64,
        )
        .map_err(|e| format!("seccomp_condition:{e}"))?;
        rules.insert(
            libc::SYS_socket,
            vec![SeccompRule::new(vec![not_unix]).map_err(|e| format!("seccomp_rule:{e}"))?],
        );
    }
    let filter = SeccompFilter::new(
        rules,
        SeccompAction::Allow,
        SeccompAction::Errno(libc::EPERM as u32),
        arch,
    )
    .map_err(|e| format!("seccomp_filter:{e}"))?;
    let program: BpfProgram = filter
        .try_into()
        .map_err(|e: seccompiler::BackendError| format!("seccomp_compile:{e}"))?;
    seccompiler::apply_filter(&program).map_err(|e| format!("seccomp_apply:{e}"))
}

/// The highest Landlock ABI this kernel supports, found by asking for each
/// as a hard requirement from the newest down (the crate's own query is
/// private).
fn detect_abi() -> Option<ABI> {
    for abi in [
        ABI::V7,
        ABI::V6,
        ABI::V5,
        ABI::V4,
        ABI::V3,
        ABI::V2,
        ABI::V1,
    ] {
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
    detect_abi().is_some_and(|abi| abi >= ABI::V4)
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

/// The tool layer's ruleset descriptor, made before the worker restricts
/// itself and applied by each host tool's process between fork and exec.
static TOOL_LAYER: std::sync::OnceLock<std::os::fd::OwnedFd> = std::sync::OnceLock::new();

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

/// Landlock, in two layers (REQ-CONF-011, REQ-CONF-012): the host tools'
/// ruleset is made first and kept for [`restrict_child`]; then the worker
/// restricts itself with the same sets plus `/proc`.
fn apply_landlock(spec: &ConfinementSpec, cgroup: Option<&Path>) -> Result<Landlock, String> {
    let abi = detect_abi().ok_or_else(|| "landlock_unsupported".to_owned())?;
    let (tool_layer, _) = landlock_ruleset(spec, cgroup, abi, &[], false)?;
    let tool_fd: Option<std::os::fd::OwnedFd> = tool_layer.into();
    let tool_fd = tool_fd.ok_or_else(|| "landlock_tool_layer: no descriptor".to_owned())?;
    let _ = TOOL_LAYER.set(tool_fd);
    let (worker_layer, net) = landlock_ruleset(spec, cgroup, abi, &WORKER_ONLY_READ, true)?;
    let status = worker_layer
        .restrict_self()
        .map_err(|e| format!("landlock_restrict:{e}"))?;
    Ok(Landlock {
        status: status.ruleset,
        net,
    })
}

/// Restricts a host tool's process to the tool layer between fork and exec:
/// the worker's sets without what only the worker reads. The process is
/// then bound by both layers. A spawn whose restriction fails does not
/// start (fail closed). A worker without Landlock has no layer and nothing
/// to add. The ruleset descriptor is the worker's, inherited by the fork.
pub fn restrict_child(cmd: &mut std::process::Command) {
    use std::os::fd::AsRawFd;
    use std::os::unix::process::CommandExt;
    let Some(fd) = TOOL_LAYER.get() else {
        return;
    };
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

/// Reaps the worker's children that have ended and that nobody waits for:
/// the descendants of a tool that re-parented to the worker (a subreaper)
/// and were killed. Called once the tool's own process has been waited for,
/// when no other child of the worker is running (one execution at a time,
/// LIM-X2), so it cannot take the status of a child someone waits for.
/// Returns how many it reaped.
pub fn reap_ended_children() -> usize {
    use rustix::process::{WaitOptions, wait};
    let mut reaped = 0;
    while let Ok(Some(_)) = wait(WaitOptions::NOHANG) {
        reaped += 1;
    }
    reaped
}

/// The processes in a cgroup now, counted from its `cgroup.procs`.
fn cgroup_members(path: &Path) -> std::io::Result<usize> {
    Ok(std::fs::read_to_string(path.join("cgroup.procs"))?
        .lines()
        .filter(|l| !l.trim().is_empty())
        .count())
}

/// The processes left in a cgroup after a `cgroup.kill`, which the kernel
/// carries out asynchronously: counted until none remains or `grace` passes.
/// `Err` when the cgroup cannot be read: an unknown is never a pass.
pub fn cgroup_members_within(path: &Path, grace: Duration) -> Result<usize, String> {
    let started = Instant::now();
    loop {
        let members = cgroup_members(path).map_err(|e| format!("tool cgroup unreadable: {e}"))?;
        if members == 0 || started.elapsed() >= grace {
            return Ok(members);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// What remains of a host tool's tree after it was killed, within `grace`
/// (REQ-CONF-009): the worker's descendants, walked from `/proc` and killed
/// again until none remains (`kill` is false only in a test build that
/// leaves a survivor on purpose, and then they are only counted), and the
/// members of the `tool` cgroup, where there is one. The larger of the two.
pub fn tool_tree_remaining(
    tool_cgroup: Option<&Path>,
    kill: bool,
    grace: Duration,
) -> Result<usize, String> {
    let worker = std::process::id();
    let walked = if kill {
        kill_descendants(worker, grace)
    } else {
        descendants_of(worker).len()
    };
    let members = match tool_cgroup {
        Some(dir) => cgroup_members_within(dir, grace)?,
        None => 0,
    };
    Ok(walked.max(members))
}

#[cfg(feature = "probe-syscalls")]
pub mod probe;

#[cfg(test)]
mod tests;
