# Handoff 011m — No Unix socket outside the write set; a tool's tree ends with it; a tool writes only its own task's workspace (release 0.10)

**RFC.** [RFC 011](../../done/011-confinement-backends.md) (confinement backends) · **Found by.** The independent audit before 0.10, which audited commit `f5a94fa`. The owner accepted the triage on 2026-11-14.

**Specification** (amended before this handoff):
- external design §13.2 (F, K) and §13.3, the paragraph "Three rules from the audit before 0.10";
- REQ-CONF-009;
- the threat register's residuals (Windows, A-06; macOS, A-21, scheduled for 0.11).

**The defects, verified in the code at `f5a94fa`:**
- **A-01 and X-02, critical.** `confine/linux.rs:376-395` (`detect_abi`) stops at ABI V7, so the Landlock ruleset (`:430-475`) never handles `ResolveUnix`, even on a kernel with ABI 9 or later. The seccomp filter (`:336-371`) checks only `socket`'s address family, and `connect` is never filtered.
  - A confined tool connects to any pathname Unix socket: the user's D-Bus session bus, tmux, ssh-agent, gpg-agent, `docker.sock`.
  - Through the session bus, `systemd-run --user` starts a process outside every clause (F, N, P and K). Re-scrubbing the environment does not help, because the tool can set the variables again and the bus's path is fixed.
  - Meanwhile F and N report `enforced`.
- **X-01, high.** `host_tool.rs:390-398`: a tool that exits by itself has its tree left alive. Only an interrupt, a deadline or a macOS watchdog verdict calls `kill_tree`. A `setsid` descendant then writes the workspaces after the action's "after" snapshot, outside the action model.
- **A-37.** The results of `kill_cgroup` and `kill_descendants` are dropped (`:242`, `:249-252`). An `Err` from `try_wait` (`:400-404`) returns without killing the tree.
- **A-36.** The tool layer has full rights on `mimlys-<pid>` (`linux.rs:443`, `:483`), which it does not need (`attach_child_to_cgroup` uses a descriptor opened before `restrict_child`).
- **A-06.** `app.rs:262` gives the worker one write set for every task (`<data>/workspaces`, `<data>/tmp`). A tool of task A can read and change task B's tree, and the dependency step can send it out.
- **A-05.** The serving worker's own clause report is dropped (`worker.rs:381-387`). The tier comes from one cached probe (`app.rs:665-667`), and the dependency-step worker is never probed (`app.rs:645-660`). The worker serves whatever `apply` returned (`worker_main.rs:163-191`).
- **A-22 (Windows).** `host_tool.rs:76-102` accepts a `.cmd` or `.bat` command, which `CreateProcessW` runs through `cmd.exe`. `quote_arg` (`windows.rs:270-302`) quotes for the C runtime only.

## Items

1. **No Unix socket outside the write set** (A-01, X-02).
   - **Where the kernel has Landlock ABI 9 or later:** `detect_abi` asks up to ABI V9, and both layers handle `AccessFs::ResolveUnix`. Only the write set's paths carry the right, so a socket a tool makes in its own workspace still works, and nothing else resolves.
   - **Below ABI 9:** seccomp denies `socket(AF_UNIX, …)`. `socketpair` stays allowed, since it reaches no path. Abstract sockets stay scoped where the ABI allows.
   - **The probe and `status`** name the primitive that holds, for example `landlock_resolve_unix` or `seccomp_no_unix_socket`. F and N say `enforced` only when one of the two holds.
   - **Measure before you decide nothing breaks.** Run `cargo test --offline` (the M6a demonstration) and the dependency step under each rung. Say whether any toolchain we document needs a Unix socket outside its workspace. The CI kernels are below ABI 9 and run the seccomp rung; this machine runs ABI 9 or later.
   - **Give a test a way to force the lower rung** (a `test-util` knob capping the ABI), so that both rungs run on one kernel.
2. **The tree ends with the execution** (X-01, A-37).
   - At every end of a host tool, its own exit included, terminate its tree with the platform's primitive:
     - Linux: `cgroup.kill` of `tool`, or the descendant walk;
     - macOS: the membership sweep it already does;
     - Windows: the job (check, and say what happens today when the tool exits normally).
   - Check the result. A process that survives within LIM-O2 is logged at `warn` as `tree_kill_incomplete` on every platform.
   - An `Err` from `try_wait` kills the tree too.
3. **No cgroup right in the tool layer** (A-36). Remove the cgroup directory from the tool layer's rules. The worker keeps it.
4. **A host tool writes only its own task's workspace** (A-06).
   - **Linux:** the host tool's own Landlock layer (`restrict_child`) holds, of the data directory, only its task's workspace: its tree and its temporary directory. The dependency step's tool layer is narrowed the same way.
   - **macOS:** find what the platform allows, measured on the macOS job: a profile the tool's process applies before `exec`, or a worker per task. Do it if one works. If neither works, stop and report: it becomes a residual like Windows's, and the owner is told.
   - **Windows:** no change in 0.10. The residual is in the threat register, and the fix is scheduled for 0.11.
5. **Every worker is checked before it is used** (A-05).
   - The runtime compares each spawned worker's own clause report (the serving worker, and the dependency step's) with the mandatory clauses before it sends work. A short report fails the spawn with the reason.
   - The worker refuses `run_host_tool` when its own report is short (REQ-SEC-003: an error in confinement means no execution).
6. **No shell on Windows** (A-22). A declared command whose file is `.cmd` or `.bat` (case-insensitive, after resolution) is refused at load, naming the reason ("no shell is involved", REQ-CONF-006) and the remedy (declare the program the script runs).

**Also move here from the audit's A-39:**
- the host tool's `HOME` and `TMPDIR` directory (`tmp/exec-<worker pid>`, `host_tool.rs:173-174`) is removed after every host tool, as the other paths in `worker_main.rs` already do;
- it is per execution, never reused (external design §14.1: "ephemeral per-execution temp").

## Tests (by state, §3c; each names its requirement)

- **1:** a test socket server listening outside the sets. Do not use the real session bus, and never run `systemd-run` in a test.
  - A confined tool's connect to it is refused, on both rungs.
  - A socket inside the tool's own workspace connects on ABI 9 or later, and is refused on the seccomp rung (stated).
  - The abstract socket stays refused.
  - `status` names the primitive.
  - REQ-CONF-001, REQ-CONF-005.
- **2:** a tool that starts a `setsid` sleeper and exits. Right after the action ends, the tool's tree is empty: read the `tool` cgroup's `cgroup.procs`, or walk the descendants. A file the sleeper would write later never appears. Then a forced survivor: the warning is logged. REQ-CONF-009.
- **3:** the tool layer cannot open the worker's `cgroup.procs` for writing, given its path.
- **4:** two tasks. Task A's tool can neither list, read nor write task B's tree, and can still write its own (Linux; macOS if done). REQ-CONF-006.
- **5:** a worker whose report lacks a mandatory clause (a test knob) is refused before any work, with the reason. The dependency step's worker is checked the same way.
- **6:** a declaration with `command = "x.cmd"` is refused by `check` and at load, naming the reason. This runs on the Windows job; the rule itself is tested everywhere.

## Mutations

- `ResolveUnix` not handled;
- the seccomp fallback removed;
- no `kill_tree` at a normal exit;
- the tool layer given the whole `workspaces/`;
- the worker's report not checked;
- `.cmd` admitted.

## Book and CHANGELOG

- The confinement page states the three rules and the remaining Windows residual.
- `status`'s primitive names are documented.
- CHANGELOG: Fixed, citing the audit's ids.

Return: `.git-exclude/review-requests/011m-confinement-audit/README.md`.
