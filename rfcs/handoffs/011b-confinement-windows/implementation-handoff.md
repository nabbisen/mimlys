# Handoff 011b — The Windows backend (M6b)

**RFC.** [RFC 011](../../accepted/011-confinement-backends.md) §3, §7 · **Specification.** REQ-CONF-005 (Windows), REQ-CONF-008, REQ-TEST-013; external design §13.3, §19.2 (`toolchain`) · **Milestone.** M6b (release 0.7). Depends on 011a.

## 1. Scope

The Windows backend behind the contract of 011a: AppContainer (F, N), restricted token (P), Job Object limits (M, C; K exists), the redirected profile and temp paths (E), reversible ACL management for the toolchain (`mimlys toolchain grant|revoke`), the clause tests and the demonstration on `windows-latest` in CI. The platform-independent code of 011a is reused unchanged.

## 2. Module layout

```text
crates/mimlys-exec/src/confine/windows.rs   (grows) AppContainerProfile{name, sid} create/open/delete ; spawn_in_container(cmd, sid, job, token) with STARTUPINFOEX and PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES (no capabilities: no network) ; RestrictedToken (DISABLE_MAX_PRIVILEGE, low integrity) ; Job limits (JobMemoryLimit = LIM-H2, ActiveProcessLimit = LIM-H3) ; every unsafe block justified
crates/mimlys-exec/src/confine/windows/acl.rs  grant(path, sid, read|write) / revoke / list: ACEs added and removed with SetNamedSecurityInfoW, recorded in <data>/toolchain-acls.json (path, sid, inheritance) so that revoke and uninstall restore the DACL exactly
crates/mimlys/src/commands/toolchain.rs      grant <path> / revoke <path> / list: confirmation, the record, the status
```

## 3. Behaviours

- The worker is spawned confined by the runtime (the AppContainer and the token are spawn-time attributes); it reports what it finds: the container SID present (F, N `enforced (appcontainer)`), the integrity level low and the token restricted (P `enforced (restricted_token)`), the job limits set (M, C `enforced (job_object)`).
- Paths: the workspaces and temp directories get write ACEs for the SID at data-directory creation (recorded like toolchain ACEs); declared `read_paths` need `toolchain grant`; a declaration whose paths lack their ACEs makes the tool unavailable with `acl_not_applied` and `status` names the command to run.
- Environment: `TEMP`, `TMP`, `USERPROFILE`, `LOCALAPPDATA`, `APPDATA`, `CARGO_HOME`, `RUSTUP_HOME` set to directories inside the task's workspace temp or to the declared read paths; `SystemRoot` and `PATH` to the system directories and the declared toolchain only.
- File locking: before a rollback or a delete the job is terminated and the operation retried once (assessment §5).
- The dependency step: the one-shot worker is spawned with the internet client capability on its AppContainer.

## 4. Tests

The clause suite of 011a on Windows (the probe tool built for Windows); AppContainer quirks documented as they are found (a tool needing a writable profile gets the redirected one); `toolchain grant` and `revoke` leave the DACL byte-identical (compared before and after); the demonstration with the MSVC toolchain on the runner.

## 5. Return

Package under `.git-exclude/reviewed/m6b-011b/`, the report `docs/src/maintainers/m6b-report.md`, the book's Windows section, CHANGELOG 0.7.

## 7. Amendments made during implementation (2026-10-31)

- **Written on Linux, gated on Windows.** The backend was implemented against the platform's documentation and type-checked for `x86_64-pc-windows-gnu`; the clause suite and the demonstration run on `windows-latest` in CI (REQ-CONF-005). The first run's clause table completes the M6b report.
- **P is the container's token first.** `CreateProcessAsUserW` with a restricted low-integrity token *and* the AppContainer attribute is tried first; where the system refuses the pair, the worker starts with the caller's token inside the container, whose lowbox token has no privileges and cannot elevate, and reports `appcontainer` instead of `restricted_token+appcontainer`. Both are `enforced`; the primitive's name says which held.
- **The worker and every host tool start suspended** inside their job and run only once assigned (`CreateProcessW`, `CREATE_SUSPENDED`, `AssignProcessToJobObject`, `ResumeThread`): nothing can break away in its first instructions. The M2 path (spawn, then assign) is gone.
- **`ConfinementSpec.data_dir`** names the installation: the profile `mimlys-<sha256 prefix>` derives from it and the permission record lives beside it. **The write set is `<data>/workspaces` and `<data>/tmp`** on every platform now, so the container is never granted the user's whole `%TEMP%`.
- **mimlys's own entries are applied at startup** (workspaces and tmp full access; the interpreters, the configured modules' directories and the worker binary's directory read), recorded as `runtime` entries that `revoke` refuses; the toolchain paths are the user's `toolchain grant`. Whether an entry is in force is read back from the directory (`GetExplicitEntriesFromAclW`), never from the record.
- **An unavailable tool is invisible and ungrantable**: a declared path without its entry gives `acl_not_applied: <path>` in `status` and `toolchain list`, the definition omits the tool, and a `host_tool` grant for it is refused with the command to run (one grant never changes nothing; design review of 0.6).
- **No orphan record on Windows**: kill-on-close ends the tree when the runtime's handle closes, a crash included; `reap_orphans` is Linux work.
- **The K test runs through the deadline**, not the stop signals (a child process without a console cannot be sent Ctrl-C); the detached child's pid is read from the tool's output and checked with `tasklist`.
- **Loopback is closed to containers**; the dependency-step test asserts the effect and the offline sibling's failure, and the chapter says a `network = true` tool talks to the network, not to local servers.
- **File locking** (RFC 011 §3): no retry was needed; a tool's tree is killed at the execution's end, so nothing of the worker's holds a workspace file between executions.

